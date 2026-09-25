//! Session resume (spec 6.2, 6.3 `lost`, 11.3, WP9): journey J6 with the fake CLIs (kill plyd, restart it, the panes
//! show `lost`, `pane.resume` brings them back), a resumed Claude pane started with `--worktree` relaunching with the
//! same option and directory, a pane without a session id reopening as a fresh shell, and F3 (finished sessions stay
//! in `session.list {include_closed:true}` with status, times and exit codes across app and plyd restarts).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::fake::{
    Fake, WAIT_FOR_START, claude_session, codex_thread, hook_program, install, launches,
    wait_status,
};
use common::{Control, Data, Sandbox, eventually};
use ply_proto::control::{ErrorCode, Event};
use ply_proto::pane::PaneStatus;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

fn create(c: &mut Control, ws: u64, params: Value) -> u64 {
    let mut p = json!({"workspace_id": ws});
    if let (Some(p), Some(extra)) = (p.as_object_mut(), params.as_object()) {
        p.extend(extra.clone());
    }
    c.call("pane.create", p).unwrap()["id"].as_u64().unwrap()
}

fn listed(c: &mut Control, ws: u64, pane: u64) -> Value {
    c.call("pane.list", json!({"workspace_id": ws}))
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == pane)
        .cloned()
        .unwrap_or(Value::Null)
}

fn wait_listed(c: &mut Control, ws: u64, pane: u64, key: &str, want: &Value) {
    let ok = eventually(WAIT, || listed(c, ws, pane)[key] == *want);
    assert!(
        ok,
        "pane {pane} {key} never became {want}: {}",
        listed(c, ws, pane)
    );
}

#[test]
fn j6_killing_plyd_leaves_lost_panes_that_resume_brings_back() {
    let sb = Sandbox::new("j6");
    install(&sb);
    hook_program();
    let repo = sb.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut plyd = sb.start();
    let (mut c, ws) = sb.control();
    let claude = create(
        &mut c,
        ws,
        json!({"cli": "claude", "cwd": repo, "worktree": {"name": "feat-x"}}),
    );
    let codex = create(&mut c, ws, json!({"cli": "codex", "cwd": sb.home}));
    let shell = create(&mut c, ws, json!({"cli": "shell", "cwd": sb.home}));
    Fake::ready(&sb, codex).send("session");
    wait_listed(
        &mut c,
        ws,
        claude,
        "session_ref",
        &json!(claude_session(claude)),
    );
    wait_listed(&mut c, ws, claude, "worktree_seen", &json!("feat-x"));
    wait_listed(
        &mut c,
        ws,
        codex,
        "session_ref",
        &json!(codex_thread(codex)),
    );
    drop(c);

    plyd.child.kill().unwrap();
    plyd.child.wait().unwrap();
    let _plyd = sb.start();
    let mut c = Control::connect(&sb.control_socket()).unwrap();
    for pane in [claude, codex] {
        let p = listed(&mut c, ws, pane);
        assert_eq!(p["status"], "lost", "{p}");
    }
    wait_listed(&mut c, ws, shell, "status", &json!("idle"));
    assert_eq!(listed(&mut c, ws, claude)["worktree_seen"], "feat-x");
    let (_, first) = Data::attach(&sb.data_socket(), claude, 80, 24).unwrap();
    assert!(matches!(first, ply_proto::data::Frame::Snapshot(_)));

    let resumed = c.call("pane.resume", json!({"pane_id": claude})).unwrap();
    assert!(
        matches!(resumed["status"].as_str(), Some("starting" | "idle")),
        "{resumed}"
    );
    wait_status(&mut c, claude, PaneStatus::Idle, WAIT);
    let launch = launches(&sb, "claude");
    let last = launch.last().unwrap();
    assert!(
        last.contains(&format!(
            "--worktree feat-x --resume {}",
            claude_session(claude)
        )),
        "{last}"
    );
    let worktree = std::fs::canonicalize(repo.join(".claude/worktrees/feat-x")).unwrap();
    assert!(
        last.contains(&format!("cwd={} ", worktree.display())),
        "{last}"
    );
    let spec: Value = serde_json::from_slice(
        &std::fs::read(sb.ply_home.join(format!("run/panes/{claude}/launch.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(
        spec["cwd"],
        json!(repo),
        "the same directory as the first launch"
    );
    assert_eq!(spec["worktree"], "feat-x");
    assert_eq!(spec["resume"], json!(claude_session(claude)));
    assert_eq!(launch.len(), 2, "one launch and one resume: {launch:?}");

    c.call("pane.resume", json!({"pane_id": codex})).unwrap();
    wait_status(&mut c, codex, PaneStatus::Idle, WAIT);
    let last = launches(&sb, "codex").last().cloned().unwrap();
    assert!(
        last.contains(&format!("resume {}", codex_thread(codex))),
        "{last}"
    );
    let fake = Fake::ready(&sb, codex);
    fake.send(&format!(
        "record {}",
        json!({"type": "turn_context", "payload": {"model": "gpt-example"}})
    ));
    assert!(
        c.wait_event(WAIT, |e| matches!(e, Event::PaneMeta(m) if m.pane_id == codex && m.model.as_deref() == Some("gpt-example")))
            .is_some(),
        "the resumed pane follows its thread's rollout again"
    );

    let mut d = sb.attach_ready(shell);
    d.input(b"echo back-$((40+2))\r").unwrap();
    assert!(
        d.pump_until(WAIT, |d| d.shows("back-42")).unwrap(),
        "R50: the shell reopened by itself"
    );
    let not_lost = c
        .call("pane.resume", json!({"pane_id": shell}))
        .unwrap_err();
    assert_eq!(not_lost.code, ErrorCode::InvalidState);

    let again = c
        .call("pane.resume", json!({"pane_id": claude}))
        .unwrap_err();
    assert_eq!(again.code, ErrorCode::InvalidState);
}

#[test]
fn a_pane_without_a_session_id_reopens_as_a_fresh_shell_by_itself() {
    let sb = Sandbox::new("fresh");
    install(&sb);
    hook_program();
    let mut plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = create(
        &mut c,
        ws,
        json!({"cli": "claude", "cwd": sb.home, "prompt": WAIT_FOR_START}),
    );
    Fake::ready(&sb, pane);
    assert_eq!(listed(&mut c, ws, pane)["session_ref"], Value::Null);
    drop(c);
    plyd.child.kill().unwrap();
    plyd.child.wait().unwrap();
    let _plyd = sb.start();
    let mut c = Control::connect(&sb.control_socket()).unwrap();
    wait_listed(&mut c, ws, pane, "cli", &json!("shell"));
    wait_listed(&mut c, ws, pane, "status", &json!("idle"));
    assert_eq!(launches(&sb, "claude").len(), 1, "claude was not run again");
    let mut d = sb.attach_ready(pane);
    d.input(b"echo shell-$((6*7))\r").unwrap();
    assert!(d.pump_until(WAIT, |d| d.shows("shell-42")).unwrap());
    let sessions = c
        .call(
            "session.list",
            json!({"workspace_id": ws, "include_closed": true}),
        )
        .unwrap();
    assert_eq!(sessions[0]["cli"], "shell");
}

#[test]
fn f3_closed_sessions_keep_status_times_and_exit_codes_across_app_and_plyd_restarts() {
    let sb = Sandbox::new("f3");
    install(&sb);
    hook_program();
    let mut plyd = sb.start();
    let (mut c, ws) = sb.control();
    let claude = create(&mut c, ws, json!({"cli": "claude", "cwd": sb.home}));
    let codex = create(&mut c, ws, json!({"cli": "codex", "cwd": sb.home}));
    let shell = create(&mut c, ws, json!({"cli": "shell", "cwd": sb.home}));
    wait_listed(
        &mut c,
        ws,
        claude,
        "session_ref",
        &json!(claude_session(claude)),
    );
    Fake::ready(&sb, claude).send("exit 0");
    let fake = Fake::ready(&sb, codex);
    fake.send("session");
    wait_listed(
        &mut c,
        ws,
        codex,
        "session_ref",
        &json!(codex_thread(codex)),
    );
    fake.send("exit 2");
    let mut d = sb.attach_ready(shell);
    d.input(b"exit 3\r").unwrap();
    let want = [
        (claude, "claude", 0),
        (codex, "codex", 2),
        (shell, "shell", 3),
    ];
    for (pane, _, code) in want {
        wait_listed(&mut c, ws, pane, "exit_code", &json!(code));
        c.call("pane.close", json!({"pane_id": pane, "kill": false}))
            .unwrap();
    }
    drop(d);
    drop(c);

    let check = |c: &mut Control| {
        let open = c
            .call(
                "session.list",
                json!({"workspace_id": ws, "include_closed": false}),
            )
            .unwrap();
        assert_eq!(open, json!([]));
        let all = c
            .call(
                "session.list",
                json!({"workspace_id": ws, "include_closed": true}),
            )
            .unwrap();
        let all = all.as_array().unwrap();
        assert_eq!(all.len(), 3, "{all:?}");
        for (pane, cli, code) in want {
            let s = all.iter().find(|s| s["pane_id"] == pane).unwrap();
            assert_eq!(s["cli"], cli);
            assert_eq!(s["status"], "exited");
            assert_eq!(s["exit_code"], code);
            let created = s["created_at"].as_u64().unwrap();
            let closed = s["closed_at"].as_u64().unwrap();
            assert!(closed >= created, "{s}");
        }
        let session =
            |pane: u64| all.iter().find(|s| s["pane_id"] == pane).unwrap()["session_ref"].clone();
        assert_eq!(session(claude), json!(claude_session(claude)));
        assert_eq!(session(codex), json!(codex_thread(codex)));
        assert_eq!(session(shell), Value::Null);
    };
    let mut app = Control::connect(&sb.control_socket()).unwrap();
    check(&mut app);
    drop(app);
    assert!(plyd.stop().success());
    let _plyd = sb.start();
    let mut app = Control::connect(&sb.control_socket()).unwrap();
    check(&mut app);
}
