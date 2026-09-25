//! The fake CLIs of plyd's tests (spec 9.3): `tests/fake/fake-claude.sh` and `tests/fake/fake-codex.sh`, installed as
//! `claude` and `codex` on the sandbox's login `PATH`.
//!
//! Both append a `launch pane=<id> cwd=<resolved cwd> argv=<args>` line to `$HOME/fake-claude.log` or
//! `$HOME/fake-codex.log`, print one line (the pane's first output), create the FIFO `$HOME/fake-<pane id>.cmd` and
//! then run one command per line written to it, so a test drives them without typing into the pane (typing is a
//! state-machine signal of its own):
//!
//! | Command | fake-claude | fake-codex |
//! |---|---|---|
//! | `start` | fires SessionStart (and UserPromptSubmit for a prompt) when started with the prompt `wait-for-start` | — |
//! | `hook <Event> <json>` | pipes `<json>` into the command its `--settings` file registers for `<Event>`, as Claude Code does | — |
//! | `session` | — | creates `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<thread>.jsonl` with its `session_meta` |
//! | `newthread <thread>` | — | switches to another thread, as `/new` does, and creates its rollout like `session` |
//! | `record <json>` | — | appends one record line to the current rollout |
//! | `turn <subtype> <turn id>` | — | appends an `event_msg` record (`task_started`, `task_complete`, `turn_aborted`) |
//! | `notify [thread]` | — | runs the `-c notify=[…]` program with an `agent-turn-complete` payload for `thread` (default its own) |
//! | `osc9 <body>` | — | prints `ESC ] 9 ; <body> BEL` |
//! | `out <text>` | prints a line | prints a line |
//! | `spin <n>` | prints a dot every 100 ms, n times | — |
//! | `exit <code>` | exits | exits |
//!
//! fake-claude enters `<cwd>/.claude/worktrees/<name>` for `--worktree <name>`, then fires SessionStart (session id
//! [`claude_session`] of the pane, or the one given to `--resume`, model `claude-example-model`) and, with a prompt,
//! UserPromptSubmit; the prompt [`WAIT_FOR_START`] holds both back until the `start` command. fake-codex's thread is [`codex_thread`] of the pane, or the one given to `resume`; a resumed one
//! appends to the rollout it already has.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ply_proto::control::{Event, PaneStatusChanged};
use ply_proto::pane::PaneStatus;
use serde_json::{Value, json};

use super::{Control, Sandbox, eventually};

/// The prompt that makes fake-claude wait for the `start` command before its first hook.
pub const WAIT_FOR_START: &str = "wait-for-start";

/// The session id fake-claude reports for a fresh pane.
pub fn claude_session(pane: u64) -> String {
    format!("00000000-0000-4000-8000-{pane:012}")
}

/// The thread id fake-codex uses for a fresh pane.
pub fn codex_thread(pane: u64) -> String {
    format!("00000000-0000-7000-8000-{pane:012}")
}

/// Installs the fakes as `claude` and `codex` in `$HOME/bin` and puts that directory on the login `PATH`.
pub fn install(sb: &Sandbox) -> PathBuf {
    let bin = sb.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake");
    for (name, script) in [("claude", "fake-claude.sh"), ("codex", "fake-codex.sh")] {
        let path = bin.join(name);
        std::fs::copy(fake.join(script), &path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(
        sb.home.join(".profile"),
        "PATH=\"$HOME/bin:$PATH\"\nexport PATH\n",
    )
    .unwrap();
    bin
}

/// The `ply-hook` plyd hands its panes: the one cargo built beside plyd (`cargo build -p ply-hook` builds it).
pub fn hook_program() -> PathBuf {
    let hook = Path::new(env!("CARGO_BIN_EXE_plyd")).with_file_name("ply-hook");
    assert!(
        hook.is_file(),
        "{} is missing; build it with `cargo build -p ply-hook` (nextest --workspace does)",
        hook.display()
    );
    hook
}

/// A fake CLI's command FIFO.
pub struct Fake {
    fifo: PathBuf,
}

impl Fake {
    /// Waits until the fake of `pane` is ready for commands.
    pub fn ready(sb: &Sandbox, pane: u64) -> Self {
        let fifo = sb.home.join(format!("fake-{pane}.cmd"));
        assert!(
            eventually(Duration::from_secs(10), || fifo.exists()),
            "the fake CLI of pane {pane} did not start"
        );
        Self { fifo }
    }

    /// Sends one command line.
    pub fn send(&self, line: &str) {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.fifo)
            .unwrap();
        f.write_all(format!("{line}\n").as_bytes()).unwrap();
    }

    /// `hook <event> <payload>` with the payload on one line.
    pub fn hook(&self, event: &str, payload: &Value) {
        self.send(&format!("hook {event} {payload}"));
    }
}

/// A Claude hook payload for `event` in pane `pane`'s session, with `extra` fields merged in.
pub fn claude_payload(pane: u64, cwd: &Path, event: &str, extra: Value) -> Value {
    let mut p = json!({
        "session_id": claude_session(pane),
        "transcript_path": "/Users/example/.claude/projects/example/session.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": event,
    });
    if let (Some(p), Some(extra)) = (p.as_object_mut(), extra.as_object()) {
        p.extend(extra.clone());
    }
    p
}

/// A `Write` tool call's hook fields (`tool_name`, `tool_input`, and `tool_use_id` when given).
pub fn write_call(id: Option<&str>) -> Value {
    let mut v = json!({
        "tool_name": "Write",
        "tool_input": {"file_path": "/Users/example/project/a.txt", "content": "hi\n"},
    });
    if let (Some(id), Some(obj)) = (id, v.as_object_mut()) {
        obj.insert("tool_use_id".to_owned(), json!(id));
    }
    v
}

/// The next `pane.status` of `pane` within `timeout`, with the time it took to arrive.
pub fn next_status(c: &mut Control, pane: u64, timeout: Duration) -> (PaneStatusChanged, Duration) {
    let started = Instant::now();
    match c.wait_event(
        timeout,
        |e| matches!(e, Event::PaneStatus(s) if s.pane_id == pane),
    ) {
        Some(Event::PaneStatus(s)) => (s, started.elapsed()),
        other => panic!("no pane.status for pane {pane} within {timeout:?}: {other:?}"),
    }
}

/// Waits for `pane.status` of `pane` reaching `status`, skipping others; returns the event and the wait.
pub fn wait_status(
    c: &mut Control,
    pane: u64,
    status: PaneStatus,
    timeout: Duration,
) -> (PaneStatusChanged, Duration) {
    let started = Instant::now();
    match c.wait_event(
        timeout,
        |e| matches!(e, Event::PaneStatus(s) if s.pane_id == pane && s.status == status),
    ) {
        Some(Event::PaneStatus(s)) => (s, started.elapsed()),
        other => panic!("pane {pane} did not reach {status:?} within {timeout:?}: {other:?}"),
    }
}

/// Asserts no `pane.status` of `pane` arrives within `quiet`.
pub fn no_status(c: &mut Control, pane: u64, quiet: Duration) {
    let got = c.wait_event(
        quiet,
        |e| matches!(e, Event::PaneStatus(s) if s.pane_id == pane),
    );
    assert!(got.is_none(), "unexpected status change: {got:?}");
}

/// The lines of a fake's launch log (`fake-claude.log` or `fake-codex.log`).
pub fn launches(sb: &Sandbox, cli: &str) -> Vec<String> {
    std::fs::read_to_string(sb.home.join(format!("fake-{cli}.log")))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}
