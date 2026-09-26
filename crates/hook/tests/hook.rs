//! `ply-hook` end to end: the C3 line it writes (decoded by ply-proto), INV-12 (plyd down or stuck: exit 0 within
//! 250 ms of what a bare launch of the binary takes) and INV-14 (nothing on stdout or stderr, exit 0, on every path
//! including bad, oversized and missing input).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ply_proto::hook::{HookEnvelope, MAX_LINE_BYTES, MAX_PAYLOAD_BYTES};
use ply_proto::pane::AgentCli;
use serde_json::{Value, json};

const BUDGET: Duration = Duration::from_millis(250);

/// [`BUDGET`] plus what this machine adds by itself: its slowest bare launch and how late a 200 ms timer fires here.
fn budget() -> Duration {
    static OVERHEAD: OnceLock<Duration> = OnceLock::new();
    BUDGET
        + *OVERHEAD.get_or_init(|| {
            let launch = (0..3)
                .map(|_| {
                    let started = Instant::now();
                    let status = Command::new(env!("CARGO_BIN_EXE_ply-hook"))
                        .env_clear()
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .unwrap();
                    assert!(status.success());
                    started.elapsed()
                })
                .max()
                .unwrap_or_default();
            // The hook's deadline is a 200 ms timer, which a loaded CI runner fired over 100 ms late.
            let lateness = (0..3)
                .map(|_| {
                    let started = Instant::now();
                    thread::sleep(Duration::from_millis(200));
                    started.elapsed().saturating_sub(Duration::from_millis(200))
                })
                .max()
                .unwrap_or_default();
            launch + lateness
        })
}

struct Hook {
    dir: PathBuf,
}

impl Drop for Hook {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Hook {
    fn new(name: &str) -> Self {
        // Unix socket paths must stay under 104 bytes, which a deep target directory can exceed.
        let dir = PathBuf::from(format!("/tmp/ply-hook-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("hook.sock")
    }

    fn listen(&self) -> UnixListener {
        let listener = UnixListener::bind(self.socket()).unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ply-hook"));
        cmd.args(args)
            .env_clear()
            .env("PLY_PANE_ID", "7")
            .env("PLY_HOOK_SOCK", self.socket())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
}

fn run(mut cmd: Command, stdin: Option<Vec<u8>>) -> (Output, Duration) {
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let started = Instant::now();
    let mut child = cmd.spawn().unwrap();
    let writer = stdin.map(|bytes| {
        let mut pipe = child.stdin.take().unwrap();
        thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        })
    });
    let output = child.wait_with_output().unwrap();
    let elapsed = started.elapsed();
    if let Some(writer) = writer {
        writer.join().unwrap();
    }
    (output, elapsed)
}

fn assert_silent_success(output: &Output, what: &str) {
    assert_eq!(output.status.code(), Some(0), "{what}: exit status");
    assert!(
        output.stdout.is_empty(),
        "{what}: stdout {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "{what}: stderr {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn receive(listener: UnixListener) -> JoinHandle<Option<Vec<u8>>> {
    thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // macOS refuses socket options with EINVAL once the peer has closed, which the hook may already have.
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut line = Vec::new();
                    stream.read_to_end(&mut line).unwrap();
                    return Some(line);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2))
                }
                Err(e) => panic!("accept: {e}"),
            }
        }
        None
    })
}

fn nothing_sent(listener: &UnixListener) -> bool {
    matches!(listener.accept(), Err(e) if e.kind() == ErrorKind::WouldBlock)
}

fn claude_payload() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../agents/tests/fixtures/claude/PreToolUse.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn a_claude_hook_becomes_one_c3_line() {
    let hook = Hook::new("claude-line");
    let received = receive(hook.listen());
    let pretty = serde_json::to_vec_pretty(&claude_payload()).unwrap();
    assert!(pretty.contains(&b'\n'));
    let (output, _) = run(hook.command(&["claude", "PreToolUse"]), Some(pretty));
    assert_silent_success(&output, "claude hook");

    let line = received.join().unwrap().expect("no connection");
    assert_eq!(line.iter().filter(|&&b| b == b'\n').count(), 1);
    assert_eq!(line.last(), Some(&b'\n'));
    let envelope = HookEnvelope::decode_line(&line).unwrap();
    assert_eq!(
        envelope,
        HookEnvelope {
            v: 1,
            pane_id: 7,
            cli: AgentCli::Claude,
            event: Some("PreToolUse".into()),
            payload: claude_payload(),
        }
    );
}

#[test]
fn a_codex_notify_comes_from_the_last_argument() {
    let hook = Hook::new("codex-line");
    let received = receive(hook.listen());
    let payload = json!({"type": "agent-turn-complete", "thread-id": "00000000-0000-7000-8000-000000000001",
        "turn-id": "t", "cwd": "/example/workspace", "input-messages": [], "last-assistant-message": null});
    let (output, _) = run(hook.command(&["codex", &payload.to_string()]), None);
    assert_silent_success(&output, "codex notify");
    let envelope = HookEnvelope::decode_line(&received.join().unwrap().unwrap()).unwrap();
    assert_eq!(envelope.cli, AgentCli::Codex);
    assert_eq!(envelope.event, None);
    assert_eq!(envelope.payload, payload);
}

#[test]
fn the_payload_travels_byte_for_byte() {
    let hook = Hook::new("verbatim");
    let received = receive(hook.listen());
    let raw = r#"{"z":1,"a":123456789012345678901234567890,"s":"quote \" and \\n"}"#;
    let (output, _) = run(
        hook.command(&["claude", r#"Odd"Event"#]),
        Some(raw.as_bytes().to_vec()),
    );
    assert_silent_success(&output, "verbatim");
    let line = String::from_utf8(received.join().unwrap().unwrap()).unwrap();
    assert_eq!(
        line,
        format!(
            "{{\"v\":1,\"pane_id\":7,\"cli\":\"claude\",\"event\":\"Odd\\\"Event\",\"payload\":{raw}}}\n"
        )
    );
}

#[test]
fn inv12_exits_zero_fast_when_plyd_is_down() {
    let hook = Hook::new("down");
    let stale = UnixListener::bind(hook.socket()).unwrap();
    drop(stale);
    let body = serde_json::to_vec(&claude_payload()).unwrap();
    for (what, socket) in [
        ("stale socket file", hook.socket()),
        ("no socket file", hook.dir.join("absent.sock")),
        ("no run directory", hook.dir.join("gone/run/hook.sock")),
    ] {
        let mut cmd = hook.command(&["claude", "Notification"]);
        cmd.env("PLY_HOOK_SOCK", socket);
        let (output, elapsed) = run(cmd, Some(body.clone()));
        assert_silent_success(&output, what);
        assert!(
            elapsed < budget(),
            "{what}: took {elapsed:?}, budget {:?}",
            budget()
        );
    }
}

#[test]
fn inv12_an_open_stdin_cannot_hold_the_cli() {
    let hook = Hook::new("open-stdin");
    let listener = hook.listen();
    let mut cmd = hook.command(&["claude", "Stop"]);
    cmd.stdin(Stdio::piped());
    let started = Instant::now();
    let mut child: Child = cmd.spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let status = child.wait().unwrap();
    let elapsed = started.elapsed();
    drop(stdin);
    assert_eq!(status.code(), Some(0));
    assert!(
        elapsed < budget(),
        "took {elapsed:?}, budget {:?}",
        budget()
    );
    assert!(nothing_sent(&listener));
}

#[test]
fn inv12_a_plyd_that_never_reads_cannot_hold_the_cli() {
    let hook = Hook::new("stuck-reader");
    let _listener = hook.listen();
    let big = format!("\"{}\"", "x".repeat(MAX_PAYLOAD_BYTES - 2));
    let (output, elapsed) = run(
        hook.command(&["claude", "PostToolUse"]),
        Some(big.into_bytes()),
    );
    assert_silent_success(&output, "stuck reader");
    assert!(
        elapsed < budget(),
        "took {elapsed:?}, budget {:?}",
        budget()
    );
}

type EnvChange<'a> = (&'a str, Option<&'a str>);

struct Case<'a> {
    what: &'a str,
    args: &'a [&'a str],
    env: &'a [EnvChange<'a>],
    stdin: Option<Vec<u8>>,
}

#[test]
fn inv14_nothing_on_stdout_on_every_path() {
    let hook = Hook::new("silent");
    let listener = hook.listen();
    let good = Some(serde_json::to_vec(&claude_payload()).unwrap());
    let oversized = Some(format!("\"{}\"", "x".repeat(MAX_PAYLOAD_BYTES)).into_bytes());
    let long_event = "E".repeat(65);
    let long_event_args = ["claude", long_event.as_str()];
    let stop = &["claude", "Stop"][..];
    let case = |what, args, env, stdin| Case {
        what,
        args,
        env,
        stdin,
    };
    let cases = [
        case("bad json", stop, &[], Some(b"{not json".to_vec())),
        case("two values", stop, &[], Some(b"{} {}".to_vec())),
        case("empty stdin", stop, &[], Some(vec![])),
        case("not utf-8", stop, &[], Some(b"\"\xff\"".to_vec())),
        case("oversized", stop, &[], oversized),
        case("no pane id", stop, &[("PLY_PANE_ID", None)], good.clone()),
        case(
            "bad pane id",
            stop,
            &[("PLY_PANE_ID", Some("seven"))],
            good.clone(),
        ),
        case(
            "no socket var",
            stop,
            &[("PLY_HOOK_SOCK", None)],
            good.clone(),
        ),
        case(
            "empty socket var",
            stop,
            &[("PLY_HOOK_SOCK", Some(""))],
            good.clone(),
        ),
        case("unknown cli", &["gemini", "Stop"], &[], good.clone()),
        case("no arguments", &[], &[], good.clone()),
        case("long event", &long_event_args, &[], good.clone()),
        case("codex without payload", &["codex"], &[], None),
        case("codex bad payload", &["codex", "{nope"], &[], None),
    ];
    for Case {
        what,
        args,
        env,
        stdin,
    } in cases
    {
        let mut cmd = hook.command(args);
        for &(key, value) in env {
            match value {
                Some(value) => cmd.env(key, value),
                None => cmd.env_remove(key),
            };
        }
        let (output, elapsed) = run(cmd, stdin);
        assert_silent_success(&output, what);
        assert!(
            elapsed < budget(),
            "{what}: took {elapsed:?}, budget {:?}",
            budget()
        );
        assert!(nothing_sent(&listener), "{what}: an envelope was sent");
    }
}

#[test]
fn the_payload_cap_is_c3s() {
    let at_cap = format!("\"{}\"", "x".repeat(MAX_PAYLOAD_BYTES - 2));
    assert_eq!(at_cap.len(), MAX_PAYLOAD_BYTES);
    let hook = Hook::new("cap");
    let received = receive(hook.listen());
    let (output, _) = run(
        hook.command(&["claude", "PostToolUse"]),
        Some(at_cap.into_bytes()),
    );
    assert_silent_success(&output, "at cap");
    let line = received.join().unwrap().unwrap();
    assert!(line.len() <= MAX_LINE_BYTES);
    assert_eq!(
        HookEnvelope::decode_line(&line)
            .unwrap()
            .payload
            .as_str()
            .unwrap()
            .len(),
        MAX_PAYLOAD_BYTES - 2
    );

    let hook = Hook::new("over-cap");
    let listener = hook.listen();
    let over = format!("\"{}\"", "x".repeat(MAX_PAYLOAD_BYTES - 1));
    let (output, _) = run(
        hook.command(&["claude", "PostToolUse"]),
        Some(over.into_bytes()),
    );
    assert_silent_success(&output, "over cap");
    assert!(nothing_sent(&listener));
}

fn status_payload() -> Value {
    json!({
        "model": {"display_name": "Opus"},
        "rate_limits": {"five_hour": {"used_percentage": 42, "resets_at": 1_790_409_600}}
    })
}

#[test]
fn the_status_line_reports_its_payload_and_prints_only_the_users_own_output() {
    let hook = Hook::new("status-own");
    let received = receive(hook.listen());
    let seen = hook.dir.join("seen.json");
    let own = format!(
        "cat > '{}'; printf 'mine %s' \"$PLY_PANE_ID\"",
        seen.display()
    );
    let body = serde_json::to_vec(&status_payload()).unwrap();
    let (output, elapsed) = run(
        hook.command(&["statusline", own.as_str()]),
        Some(body.clone()),
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "mine 7");
    assert!(output.stderr.is_empty());
    assert!(
        elapsed < budget(),
        "took {elapsed:?}, budget {:?}",
        budget()
    );
    assert_eq!(
        std::fs::read(&seen).unwrap(),
        body,
        "the user's command reads the same payload"
    );

    let line = received.join().unwrap().expect("no connection");
    let envelope = HookEnvelope::decode_line(&line).unwrap();
    assert_eq!(
        (envelope.pane_id, envelope.cli, envelope.event.as_deref()),
        (7, AgentCli::Claude, Some("StatusLine"))
    );
    assert_eq!(envelope.payload, status_payload());
}

#[test]
fn a_status_line_without_a_command_of_its_own_prints_nothing_and_still_reports() {
    let hook = Hook::new("status-bare");
    let received = receive(hook.listen());
    let body = serde_json::to_vec(&status_payload()).unwrap();
    let (output, _) = run(hook.command(&["statusline"]), Some(body));
    assert_silent_success(&output, "bare status line");
    let line = received.join().unwrap().expect("no connection");
    assert_eq!(
        HookEnvelope::decode_line(&line).unwrap().event.as_deref(),
        Some("StatusLine")
    );
}

#[test]
fn the_status_line_exits_with_the_users_command_and_works_with_plyd_down() {
    let hook = Hook::new("status-down");
    let body = serde_json::to_vec(&status_payload()).unwrap();
    let (output, elapsed) = run(
        hook.command(&["statusline", "printf still; exit 3"]),
        Some(body),
    );
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "still");
    assert!(
        elapsed < budget(),
        "took {elapsed:?}, budget {:?}",
        budget()
    );
}

#[test]
fn a_status_line_outside_ply_reports_as_pane_0_to_the_named_or_default_socket() {
    let hook = Hook::new("status-out");
    let received = receive(hook.listen());
    let body = serde_json::to_vec(&status_payload()).unwrap();
    let mut cmd = hook.command(&["statusline"]);
    cmd.env_remove("PLY_PANE_ID");
    let (output, _) = run(cmd, Some(body.clone()));
    assert_silent_success(&output, "status line outside ply");
    let line = received.join().unwrap().expect("no connection");
    assert_eq!(HookEnvelope::decode_line(&line).unwrap().pane_id, 0);

    let home = hook.dir.join("h");
    let socket = home.join("Library/Application Support/ply/run/hook.sock");
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let received = receive(listener);
    let mut cmd = hook.command(&["statusline"]);
    cmd.env_remove("PLY_PANE_ID")
        .env_remove("PLY_HOOK_SOCK")
        .env("HOME", &home);
    let (output, _) = run(cmd, Some(body));
    assert_silent_success(&output, "status line with plyd's default socket");
    let line = received.join().unwrap().expect("no connection");
    let envelope = HookEnvelope::decode_line(&line).unwrap();
    assert_eq!(
        (envelope.pane_id, envelope.event.as_deref()),
        (0, Some("StatusLine"))
    );
}
