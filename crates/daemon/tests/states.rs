//! The spec 6.3 state machine end to end (WP6 verification): a real plyd, the fake CLIs of `tests/fake/` calling the
//! built `ply-hook`, emitting OSC 9 and writing a rollout in the sandbox's `CODEX_HOME`, and a C1 client watching
//! `pane.status`, `pane.progress` and `pane.meta`. One test per row of the table (R17's and R27's rows included), F1
//! (a hook turns the pane waiting within 250 ms), INV-12 (a hook with plyd down exits 0 at once, printing nothing),
//! the C7 version check, and Codex's rollout binding (R28).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::fake::{
    Fake, WAIT_FOR_START, claude_payload, claude_session, codex_thread, hook_program, install,
    next_status, no_status, wait_status, write_call,
};
use common::{Control, Data, Sandbox, eventually};
use ply_proto::control::{ErrorCode, Event};
use ply_proto::pane::PaneStatus::{self, Exited, Idle, Running, WaitingInput, WaitingPermission};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);
const F1: Duration = Duration::from_millis(250);

struct Env {
    sb: Sandbox,
    _plyd: common::Plyd,
    c: Control,
    ws: u64,
}

fn env(tag: &str) -> Env {
    let sb = Sandbox::new(tag);
    install(&sb);
    hook_program();
    let plyd = sb.start();
    let (c, ws) = sb.control();
    Env {
        sb,
        _plyd: plyd,
        c,
        ws,
    }
}

impl Env {
    fn pane(&mut self, cli: &str, extra: Value) -> u64 {
        let mut params = json!({"workspace_id": self.ws, "cli": cli, "cwd": self.sb.home});
        if let (Some(p), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            p.extend(extra.clone());
        }
        let pane = self.c.call("pane.create", params).unwrap();
        pane["id"].as_u64().unwrap()
    }

    /// A Claude pane that reached `idle` through its SessionStart.
    fn claude(&mut self) -> (u64, Fake) {
        let id = self.pane("claude", json!({}));
        let fake = Fake::ready(&self.sb, id);
        self.until(id, Idle);
        (id, fake)
    }

    /// A Codex pane that reached `idle` through its first output byte.
    fn codex(&mut self) -> (u64, Fake) {
        let id = self.pane("codex", json!({}));
        let fake = Fake::ready(&self.sb, id);
        self.until(id, Idle);
        (id, fake)
    }

    /// Waits for (and consumes) the `pane.status` of `pane` reaching `status`; every change reaches this client.
    fn until(&mut self, pane: u64, status: PaneStatus) -> Duration {
        wait_status(&mut self.c, pane, status, WAIT).1
    }

    /// The next `pane.status` of `pane`, asserted to be `status`; returns how long it took.
    fn next(&mut self, pane: u64, status: PaneStatus) -> Duration {
        let (s, took) = next_status(&mut self.c, pane, WAIT);
        assert_eq!(s.status, status, "{s:?}");
        took
    }

    fn listed(&mut self, pane: u64) -> Value {
        let panes = self
            .c
            .call("pane.list", json!({"workspace_id": self.ws}))
            .unwrap();
        panes
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == pane)
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn claude_hook(&self, fake: &Fake, pane: u64, event: &str, extra: Value) {
        fake.hook(event, &claude_payload(pane, &self.sb.home, event, extra));
    }

    fn type_raw(&self, pane: u64, bytes: &[u8]) {
        let (mut d, first) = Data::attach(&self.sb.data_socket(), pane, 80, 24).unwrap();
        d.apply(&first, true).unwrap();
        d.input(bytes).unwrap();
        d.settle(Duration::from_millis(50)).unwrap();
    }
}

fn todos(done: usize, total: usize) -> Value {
    let items: Vec<Value> = (0..total)
        .map(|i| {
            let status = if i < done {
                "completed"
            } else if i == done {
                "in_progress"
            } else {
                "pending"
            };
            json!({"content": format!("step {i}"), "status": status, "activeForm": format!("doing step {i}")})
        })
        .collect();
    json!({"tool_name": "TodoWrite", "tool_input": {"todos": items}, "tool_response": {}})
}

#[test]
fn claude_starts_in_starting_and_session_start_makes_it_idle_with_its_meta() {
    let mut e = env("st-start");
    let id = e.pane("claude", json!({"prompt": WAIT_FOR_START}));
    let fake = Fake::ready(&e.sb, id);
    assert_eq!(e.listed(id)["status"], "starting", "spawn → starting");
    no_status(&mut e.c, id, Duration::from_millis(300));
    fake.send("start");
    let took = e.next(id, Idle);
    assert!(took < F1, "SessionStart → idle took {took:?}");
    let pane = e.listed(id);
    assert_eq!(pane["session_ref"], claude_session(id));
    assert_eq!(pane["model_seen"], "claude-example-model");
}

#[test]
fn a_submitted_prompt_runs_and_stop_or_stop_failure_returns_to_idle() {
    let mut e = env("st-prompt");
    let (id, fake) = e.claude();
    e.claude_hook(&fake, id, "UserPromptSubmit", json!({"prompt": "hi"}));
    e.next(id, Running);
    e.claude_hook(&fake, id, "Stop", json!({"stop_hook_active": false}));
    e.next(id, Idle);
    e.claude_hook(&fake, id, "UserPromptSubmit", json!({"prompt": "again"}));
    e.next(id, Running);
    e.claude_hook(&fake, id, "StopFailure", json!({"error": "example"}));
    e.next(id, Idle);
    e.claude_hook(&fake, id, "Stop", json!({}));
    no_status(&mut e.c, id, Duration::from_millis(300));
}

#[test]
fn tool_use_runs_the_pane_from_idle_and_waiting_input() {
    let mut e = env("st-tool");
    let (id, fake) = e.claude();
    e.claude_hook(&fake, id, "PreToolUse", write_call(Some("toolu_example1")));
    e.next(id, Running);
    e.claude_hook(&fake, id, "Stop", json!({}));
    e.next(id, Idle);
    let question =
        json!({"message": "Claude needs your answer", "notification_type": "elicitation_dialog"});
    e.claude_hook(&fake, id, "Notification", question);
    e.next(id, WaitingInput);
    e.claude_hook(&fake, id, "PostToolUse", write_call(Some("toolu_example1")));
    e.next(id, Running);
}

#[test]
fn f1_a_permission_request_waits_within_250_ms_and_only_its_own_call_settles_it() {
    let mut e = env("st-perm");
    let (id, fake) = e.claude();
    e.claude_hook(
        &fake,
        id,
        "UserPromptSubmit",
        json!({"prompt": "write a.txt"}),
    );
    e.next(id, Running);
    for (settle, extra) in [
        ("PostToolUse", json!({"tool_response": {"success": true}})),
        ("PostToolUseFailure", json!({"error": "example"})),
        ("PermissionDenied", json!({"reason": "example"})),
    ] {
        e.claude_hook(&fake, id, "PermissionRequest", write_call(None));
        let (s, took) = next_status(&mut e.c, id, WAIT);
        assert_eq!(s.status, WaitingPermission);
        assert_eq!(s.detail.as_deref(), Some("Write"));
        eprintln!("F1 PermissionRequest → waiting_permission: {took:?} (fake CLI included)");
        assert!(took < F1, "F1: {took:?}");
        let mut other = write_call(Some("toolu_other"));
        other["tool_input"]["content"] = json!("something else\n");
        let mut payload = other;
        if let (Some(p), Some(x)) = (payload.as_object_mut(), extra.as_object()) {
            p.extend(x.clone());
        }
        e.claude_hook(&fake, id, settle, payload);
        no_status(&mut e.c, id, Duration::from_millis(300));
        let mut same = write_call(Some("toolu_example2"));
        if let (Some(p), Some(x)) = (same.as_object_mut(), extra.as_object()) {
            p.extend(x.clone());
        }
        e.claude_hook(&fake, id, settle, same);
        e.next(id, Running);
    }
}

#[test]
fn any_key_typed_or_an_answer_moves_a_waiting_pane_to_running() {
    let mut e = env("st-key");
    let (id, fake) = e.claude();
    e.claude_hook(&fake, id, "UserPromptSubmit", json!({"prompt": "go"}));
    e.next(id, Running);
    e.claude_hook(&fake, id, "PermissionRequest", write_call(None));
    e.next(id, WaitingPermission);
    e.type_raw(id, b"3");
    e.next(id, Running);
    let question =
        json!({"message": "Claude needs your answer", "notification_type": "elicitation_dialog"});
    e.claude_hook(&fake, id, "Notification", question);
    e.next(id, WaitingInput);
    e.type_raw(id, b"x");
    e.next(id, Running);
    e.claude_hook(&fake, id, "PermissionRequest", write_call(None));
    e.next(id, WaitingPermission);
    e.c.call("pane.answer", json!({"pane_id": id, "choice": 1}))
        .unwrap();
    e.next(id, Running);
    let refused =
        e.c.call("pane.answer", json!({"pane_id": id, "choice": 1}))
            .unwrap_err();
    assert_eq!(refused.code, ErrorCode::InvalidState);
}

#[test]
fn f1_notifications_ask_for_permission_or_input_but_idle_prompt_leaves_the_pane_idle() {
    let mut e = env("st-notif");
    let (id, fake) = e.claude();
    e.claude_hook(&fake, id, "UserPromptSubmit", json!({"prompt": "go"}));
    e.next(id, Running);
    let permission = json!({"message": "Claude needs your permission to use Write", "notification_type": "permission_prompt"});
    e.claude_hook(&fake, id, "Notification", permission.clone());
    let (s, _) = next_status(&mut e.c, id, WAIT);
    assert_eq!(
        (s.status, s.detail.as_deref()),
        (
            WaitingPermission,
            Some("Claude needs your permission to use Write")
        ),
        "R46: a permission prompt without its PermissionRequest hook still asks"
    );
    e.claude_hook(&fake, id, "Notification", permission);
    no_status(&mut e.c, id, Duration::from_millis(300));
    e.type_raw(id, b"1");
    e.next(id, Running);
    e.claude_hook(&fake, id, "Stop", json!({}));
    e.next(id, Idle);
    let idle_prompt =
        json!({"message": "Claude is waiting for your input", "notification_type": "idle_prompt"});
    e.claude_hook(&fake, id, "Notification", idle_prompt);
    no_status(&mut e.c, id, Duration::from_millis(300));
    assert_eq!(e.listed(id)["status"], "idle", "R46: still your turn");
    let question =
        json!({"message": "Claude needs your answer", "notification_type": "elicitation_dialog"});
    e.claude_hook(&fake, id, "Notification", question);
    let (s, took) = next_status(&mut e.c, id, WAIT);
    assert_eq!(s.status, WaitingInput);
    assert_eq!(s.detail.as_deref(), Some("Claude needs your answer"));
    eprintln!("F1 Notification → waiting_input: {took:?} (fake CLI included)");
    assert!(took < F1, "F1: {took:?}");
}

#[test]
fn f1_a_hook_fired_directly_turns_the_pane_waiting_within_250_ms() {
    let mut e = env("st-f1");
    let (id, fake) = e.claude();
    let sock = e.sb.ply_home.join("run/hook.sock");
    let mut times = Vec::new();
    for round in 0..10 {
        e.claude_hook(&fake, id, "UserPromptSubmit", json!({"prompt": "go"}));
        e.next(id, Running);
        let payload = claude_payload(id, &e.sb.home, "PermissionRequest", write_call(None));
        let started = Instant::now();
        let mut child = Command::new(hook_program())
            .args(["claude", "PermissionRequest"])
            .env_clear()
            .env("PLY_PANE_ID", id.to_string())
            .env("PLY_HOOK_SOCK", &sock)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let (s, _) = next_status(&mut e.c, id, WAIT);
        let took = started.elapsed();
        assert_eq!(s.status, WaitingPermission, "round {round}");
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success() && out.stdout.is_empty(), "INV-14");
        times.push(took);
        e.claude_hook(&fake, id, "PostToolUse", write_call(Some("toolu_example3")));
        e.next(id, Running);
        e.claude_hook(&fake, id, "Stop", json!({}));
        e.next(id, Idle);
    }
    times.sort();
    eprintln!(
        "F1 hook spawn → pane.status waiting_permission over {} runs: min {:?}, median {:?}, max {:?}",
        times.len(),
        times[0],
        times[times.len() / 2],
        times[times.len() - 1]
    );
    assert!(times[times.len() - 1] < F1, "F1: {times:?}");
}

#[test]
fn a_quiet_claude_pane_goes_idle_after_5_s_while_output_keeps_it_running() {
    let mut e = env("st-quiet");
    let (id, fake) = e.claude();
    e.claude_hook(&fake, id, "UserPromptSubmit", json!({"prompt": "go"}));
    e.next(id, Running);
    let started = Instant::now();
    fake.send("spin 65");
    no_status(&mut e.c, id, Duration::from_millis(5800));
    let (s, _) = next_status(&mut e.c, id, WAIT);
    let after = started.elapsed();
    assert_eq!(s.status, Idle, "R17: silent pty and no hook for 5 s");
    assert!(
        after >= Duration::from_millis(11_000),
        "output kept it running until the spin ended: {after:?}"
    );
}

#[test]
fn session_end_changes_nothing_and_the_exit_code_ends_the_pane() {
    let mut e = env("st-end");
    let (id, fake) = e.claude();
    e.claude_hook(
        &fake,
        id,
        "SessionEnd",
        json!({"reason": "prompt_input_exit"}),
    );
    no_status(&mut e.c, id, Duration::from_millis(300));
    fake.send("exit 0");
    let (s, _) = next_status(&mut e.c, id, WAIT);
    assert_eq!((s.status, s.exit_code), (Exited, Some(0)));
}

#[test]
fn a_clear_ends_one_session_and_the_next_session_start_keeps_the_machine_going() {
    let mut e = env("st-clear");
    let (id, fake) = e.claude();
    e.claude_hook(&fake, id, "UserPromptSubmit", json!({"prompt": "hi"}));
    e.next(id, Running);
    e.claude_hook(&fake, id, "SessionEnd", json!({"reason": "clear"}));
    no_status(&mut e.c, id, Duration::from_millis(300));
    let cleared = "00000000-0000-4000-8000-00000000abcd";
    e.claude_hook(
        &fake,
        id,
        "SessionStart",
        json!({"source": "clear", "session_id": cleared, "model": "claude-example-model"}),
    );
    e.next(id, Idle);
    assert_eq!(
        e.listed(id)["session_ref"],
        cleared,
        "resume follows the new session"
    );
    e.claude_hook(&fake, id, "UserPromptSubmit", json!({"prompt": "again"}));
    e.next(id, Running);
    e.claude_hook(&fake, id, "PermissionRequest", write_call(None));
    e.next(id, WaitingPermission);
    fake.send("exit 0");
    e.until(id, Exited);
}

/// CPU time plyd has used so far, from `ps` (`[[H:]M:]S.cc`).
fn cpu_ms(pid: u32) -> u64 {
    let out = Command::new("ps")
        .args(["-o", "cputime=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let secs = text
        .split(':')
        .fold(0.0, |acc, part| acc * 60.0 + part.parse::<f64>().unwrap());
    (secs * 1000.0) as u64
}

#[test]
fn a_session_end_while_running_leaves_the_pane_task_asleep() {
    let mut e = env("st-spin");
    let pid = e._plyd.child.id();
    let (id, fake) = e.claude();
    e.claude_hook(&fake, id, "UserPromptSubmit", json!({"prompt": "hi"}));
    e.next(id, Running);
    e.claude_hook(&fake, id, "SessionEnd", json!({"reason": "clear"}));
    let took = e.next(id, Idle);
    assert!(
        took >= Duration::from_millis(4500),
        "the R17 quiet timeout still applies after a SessionEnd: {took:?}"
    );
    let before = cpu_ms(pid);
    std::thread::sleep(Duration::from_secs(3));
    let used = cpu_ms(pid) - before;
    assert!(
        used < 300,
        "plyd used {used} ms of CPU in 3 s with nothing to do"
    );
    fake.send("exit 0");
}

#[test]
fn pty_end_of_file_without_session_end_is_exited_with_the_code() {
    let mut e = env("st-eof");
    let (id, fake) = e.claude();
    fake.send("exit 3");
    let (s, _) = next_status(&mut e.c, id, WAIT);
    assert_eq!((s.status, s.exit_code), (Exited, Some(3)));
    assert!(
        e.c.wait_event(
            WAIT,
            |ev| matches!(ev, Event::PaneExit(x) if x.pane_id == id && x.code == 3)
        )
        .is_some()
    );
}

#[test]
fn todo_write_progress_goes_out_at_most_4_times_a_second_and_keeps_its_last_value() {
    let mut e = env("st-prog");
    let (id, fake) = e.claude();
    for done in 1..=10 {
        e.claude_hook(&fake, id, "PostToolUse", todos(done, 10));
    }
    let mut seen: Vec<(Instant, Value)> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(Event::PaneProgress(p)) = e.c.wait_event(
            Duration::from_millis(100),
            |ev| matches!(ev, Event::PaneProgress(p) if p.pane_id == id),
        ) {
            seen.push((Instant::now(), serde_json::to_value(&p.progress).unwrap()));
        }
    }
    assert!(!seen.is_empty(), "no pane.progress");
    for pair in seen.windows(2) {
        let gap = pair[1].0 - pair[0].0;
        assert!(
            gap >= Duration::from_millis(200),
            "two events {gap:?} apart"
        );
    }
    assert_eq!(
        seen.last().unwrap().1,
        json!({"done": 10, "total": 10}),
        "{seen:?}"
    );
    assert!(
        seen.len() < 10,
        "the burst was coalesced: {} events",
        seen.len()
    );
    assert_eq!(e.listed(id)["progress"], json!({"done": 10, "total": 10}));
}

fn git(dir: &std::path::Path, home: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn a_cwd_change_updates_the_directory_worktree_label_and_branch() {
    let mut e = env("st-cwd");
    let repo = e.sb.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &e.sb.home, &["init", "-q", "-b", "example-branch"]);
    git(
        &repo,
        &e.sb.home,
        &[
            "-c",
            "user.name=example",
            "-c",
            "user.email=example@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "example",
        ],
    );
    let id = e.pane("claude", json!({"cwd": repo}));
    let fake = Fake::ready(&e.sb, id);
    let branch = e.c.wait_event(WAIT, |ev| {
        matches!(ev, Event::PaneMeta(m) if m.pane_id == id && m.branch.as_deref() == Some("example-branch"))
    });
    assert!(branch.is_some(), "the branch label of the spawn directory");
    let worktree = repo.join(".claude/worktrees/feat-x");
    std::fs::create_dir_all(&worktree).unwrap();
    let cwd = std::fs::canonicalize(&worktree).unwrap();
    e.claude_hook(
        &fake,
        id,
        "CwdChanged",
        json!({"old_cwd": repo, "new_cwd": cwd}),
    );
    let meta = e.c.wait_event(WAIT, |ev| {
        matches!(ev, Event::PaneMeta(m) if m.pane_id == id && m.worktree.as_deref() == Some("feat-x"))
    });
    let Some(Event::PaneMeta(meta)) = meta else {
        panic!("no pane.meta with the worktree");
    };
    assert_eq!(meta.cwd, cwd.display().to_string());
    assert_eq!(meta.model.as_deref(), Some("claude-example-model"));
    let pane = e.listed(id);
    assert_eq!(pane["worktree_seen"], "feat-x");
    assert!(
        eventually(WAIT, || e.listed(id)["branch"] == "example-branch"),
        "the branch of the new directory"
    );
}

#[test]
fn a_cli_below_the_minimum_version_is_refused_at_spawn() {
    let sb = Sandbox::new("st-old");
    let bin = install(&sb);
    let versions = sb.home.join(".local/share/claude/versions");
    std::fs::create_dir_all(&versions).unwrap();
    let old = versions.join("2.1.100");
    std::fs::rename(bin.join("claude"), &old).unwrap();
    std::os::unix::fs::symlink(&old, bin.join("claude")).unwrap();
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let refused = c
        .call(
            "pane.create",
            json!({"workspace_id": ws, "cli": "claude", "cwd": sb.home}),
        )
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::CliTooOld, "{}", refused.msg);
    assert!(refused.msg.contains("2.1.100"), "{}", refused.msg);
    assert_eq!(
        c.call("pane.list", json!({"workspace_id": ws})).unwrap(),
        json!([])
    );
}

#[test]
fn inv_12_a_hook_with_plyd_down_exits_0_at_once_and_prints_nothing() {
    let mut e = env("st-down");
    let (id, _fake) = e.claude();
    drop(e.c);
    let mut plyd = e._plyd;
    assert!(plyd.stop().success());
    let started = Instant::now();
    let out = Command::new(hook_program())
        .args(["claude", "Notification"])
        .env_clear()
        .env("PLY_PANE_ID", id.to_string())
        .env("PLY_HOOK_SOCK", e.sb.ply_home.join("run/hook.sock"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let took = started.elapsed();
    assert!(out.status.success());
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
    assert!(took < Duration::from_millis(250), "{took:?}");
}

#[test]
fn codex_first_output_makes_it_idle_enter_runs_it_and_it_never_times_out() {
    let mut e = env("cx-start");
    let (id, fake) = e.codex();
    e.type_raw(id, b"hello");
    no_status(&mut e.c, id, Duration::from_millis(300));
    e.type_raw(id, b"\r");
    e.next(id, Running);
    no_status(&mut e.c, id, Duration::from_millis(6000));
    fake.send("exit 4");
    let (s, _) = next_status(&mut e.c, id, WAIT);
    assert_eq!((s.status, s.exit_code), (Exited, Some(4)));
}

#[test]
fn codex_osc9_approvals_questions_plans_and_turn_ends_move_the_pane() {
    let mut e = env("cx-osc9");
    let (id, fake) = e.codex();
    e.type_raw(id, b"\r");
    e.next(id, Running);
    fake.send("osc9 Approval requested: rm -rf build");
    let (s, took) = next_status(&mut e.c, id, WAIT);
    assert_eq!(s.status, WaitingPermission);
    assert_eq!(
        s.detail.as_deref(),
        Some("Approval requested: rm -rf build")
    );
    eprintln!("F1 Codex OSC 9 approval → waiting_permission: {took:?} (fake CLI included)");
    assert!(took < F1, "{took:?}");
    e.type_raw(id, b"y");
    e.next(id, Running);
    for (body, then) in [
        ("Codex wants to edit a.txt", WaitingPermission),
        ("Question: which file?", WaitingInput),
        ("Plan mode prompt: Implement the plan?", WaitingInput),
    ] {
        fake.send(&format!("osc9 {body}"));
        let (s, _) = next_status(&mut e.c, id, WAIT);
        assert_eq!((s.status, s.detail.as_deref()), (then, Some(body)));
        e.type_raw(id, b"1");
        e.next(id, Running);
    }
    fake.send("osc9 Created a.txt containing hi.");
    e.next(id, Idle);
    fake.send("osc9 Approval requested: late");
    no_status(&mut e.c, id, Duration::from_millis(300));
}

fn wait_session_ref(e: &mut Env, id: u64, want: &str) {
    assert!(
        eventually(WAIT, || e.listed(id)["session_ref"] == want),
        "pane {id} never bound to {want}: {}",
        e.listed(id)
    );
}

#[test]
fn codex_notify_ends_the_bound_threads_turn_and_ignores_the_title_turn() {
    let mut e = env("cx-notify");
    let (id, fake) = e.codex();
    fake.send("session");
    wait_session_ref(&mut e, id, &codex_thread(id));
    e.type_raw(id, b"\r");
    e.next(id, Running);
    fake.send("notify 00000000-0000-7000-8000-00000000aaaa");
    no_status(&mut e.c, id, Duration::from_millis(800));
    fake.send("notify");
    e.next(id, Idle);
}

#[test]
fn codex_notify_before_its_rollout_exists_completes_the_turn_once_it_appears() {
    let mut e = env("cx-defer");
    let (id, fake) = e.codex();
    e.type_raw(id, b"\r");
    e.next(id, Running);
    fake.send("notify");
    no_status(&mut e.c, id, Duration::from_millis(800));
    fake.send("session");
    e.next(id, Idle);
    wait_session_ref(&mut e, id, &codex_thread(id));
}

#[test]
fn codex_rollout_reports_the_model_and_the_plan_in_both_shapes() {
    let mut e = env("cx-roll");
    let (id, fake) = e.codex();
    fake.send("session");
    let cwd = std::fs::canonicalize(&e.sb.home).unwrap();
    fake.send(&format!(
        "record {}",
        json!({"timestamp": "2026-09-25T00:00:00.000Z", "type": "turn_context", "payload": {"model": "gpt-example", "cwd": cwd}})
    ));
    let meta = e.c.wait_event(WAIT, |ev| {
        matches!(ev, Event::PaneMeta(m) if m.pane_id == id && m.model.as_deref() == Some("gpt-example"))
    });
    assert!(meta.is_some(), "turn_context.model → pane.meta");
    let plan = json!({"plan": [
        {"step": "a", "status": "completed"},
        {"step": "b", "status": "in_progress"},
        {"step": "c", "status": "pending"}
    ]});
    fake.send(&format!(
        "record {}",
        json!({"type": "response_item", "payload": {"type": "function_call", "name": "update_plan", "arguments": plan.to_string(), "call_id": "call_example1"}})
    ));
    let progress = e.c.wait_event(
        WAIT,
        |ev| matches!(ev, Event::PaneProgress(p) if p.pane_id == id),
    );
    let Some(Event::PaneProgress(p)) = progress else {
        panic!("no pane.progress");
    };
    assert_eq!(
        serde_json::to_value(&p.progress).unwrap(),
        json!({"done": 1, "total": 3, "current": "b"})
    );
    let input = "await tools.update_plan({plan: [{step: 'a', status: 'completed'}, {step: 'b', status: 'completed'}]});";
    fake.send(&format!(
        "record {}",
        json!({"type": "response_item", "payload": {"type": "custom_tool_call", "name": "exec", "input": input, "call_id": "call_example2"}})
    ));
    let progress = e.c.wait_event(
        WAIT,
        |ev| matches!(ev, Event::PaneProgress(p) if p.pane_id == id),
    );
    let Some(Event::PaneProgress(p)) = progress else {
        panic!("no pane.progress for the code-mode plan");
    };
    assert_eq!(
        serde_json::to_value(&p.progress).unwrap(),
        json!({"done": 2, "total": 2})
    );
    assert_eq!(e.listed(id)["model_seen"], "gpt-example");
}
