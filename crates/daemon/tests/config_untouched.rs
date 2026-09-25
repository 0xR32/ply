//! INV-8 (spec 10, WP6 verification): a whole Claude Code and Codex session through plyd leaves the user's
//! `~/.claude/settings.json`, `~/.claude.json` and `~/.codex/config.toml` byte for byte as they were. The CLIs are the
//! fakes of `tests/fake/` in a sandboxed `HOME` (so `CODEX_HOME` is `$HOME/.codex`); all their configuration comes
//! per invocation, the per-pane settings file lives only under plyd's run directory and holds nothing but hooks and
//! the theme.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use common::fake::{Fake, claude_payload, hook_program, install, wait_status, write_call};
use common::{Data, Sandbox};
use ply_proto::pane::PaneStatus;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

fn names(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_claude_and_a_codex_session_leave_the_users_cli_config_byte_identical() {
    let sb = Sandbox::new("inv8");
    install(&sb);
    hook_program();
    let claude_dir = sb.home.join(".claude");
    let codex_dir = sb.home.join(".codex");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::create_dir_all(&codex_dir).unwrap();
    let files = [
        (
            claude_dir.join("settings.json"),
            "{\n  \"model\": \"example\",\n  \"permissions\": {\"allow\": [\"Read\"]},\n  \"hooks\": {}\n}\n",
        ),
        (
            sb.home.join(".claude.json"),
            "{\"projects\": {\"/Users/example/project\": {\"hasTrustDialogAccepted\": true}}}\n",
        ),
        (
            codex_dir.join("config.toml"),
            "model = \"example\"\n\n[tui]\nnotification_method = \"bel\"\n",
        ),
    ];
    for (path, text) in &files {
        std::fs::write(path, text).unwrap();
    }
    let before: Vec<Vec<u8>> = files
        .iter()
        .map(|(p, _)| std::fs::read(p).unwrap())
        .collect();

    let mut plyd = sb.start();
    let (mut c, ws) = sb.control();
    let claude = c
        .call(
            "pane.create",
            json!({"workspace_id": ws, "cli": "claude", "cwd": sb.home, "prompt": "write a.txt"}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let fake = Fake::ready(&sb, claude);
    wait_status(&mut c, claude, PaneStatus::Running, WAIT);
    let generated = sb
        .ply_home
        .join("run/panes")
        .join(claude.to_string())
        .join("claude-settings.json");
    let settings: Value = serde_json::from_slice(&std::fs::read(&generated).unwrap()).unwrap();
    let keys: BTreeSet<&str> = settings
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, BTreeSet::from(["hooks", "theme"]), "spec 6.1");
    let hook = |event: &str, extra: Value| {
        fake.hook(event, &claude_payload(claude, &sb.home, event, extra));
    };
    hook("PermissionRequest", write_call(None));
    wait_status(&mut c, claude, PaneStatus::WaitingPermission, WAIT);
    c.call("pane.answer", json!({"pane_id": claude, "answer": "yes"}))
        .unwrap();
    hook("PostToolUse", write_call(Some("toolu_example1")));
    hook("Stop", json!({}));
    wait_status(&mut c, claude, PaneStatus::Idle, WAIT);
    hook("SessionEnd", json!({"reason": "prompt_input_exit"}));
    fake.send("exit 0");
    wait_status(&mut c, claude, PaneStatus::Exited, WAIT);

    let codex = c
        .call(
            "pane.create",
            json!({"workspace_id": ws, "cli": "codex", "cwd": sb.home}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let fake = Fake::ready(&sb, codex);
    wait_status(&mut c, codex, PaneStatus::Idle, WAIT);
    fake.send("session");
    let (mut d, first) = Data::attach(&sb.data_socket(), codex, 80, 24).unwrap();
    d.apply(&first, true).unwrap();
    d.input(b"\r").unwrap();
    wait_status(&mut c, codex, PaneStatus::Running, WAIT);
    fake.send("osc9 Approval requested: touch a.txt");
    wait_status(&mut c, codex, PaneStatus::WaitingPermission, WAIT);
    d.input(b"y").unwrap();
    fake.send("notify");
    wait_status(&mut c, codex, PaneStatus::Idle, WAIT);
    fake.send("exit 0");
    wait_status(&mut c, codex, PaneStatus::Exited, WAIT);
    for pane in [claude, codex] {
        c.call("pane.close", json!({"pane_id": pane, "kill": false}))
            .unwrap();
    }
    drop(d);
    drop(c);
    assert!(plyd.stop().success());

    for ((path, _), was) in files.iter().zip(&before) {
        assert_eq!(
            &std::fs::read(path).unwrap(),
            was,
            "{} changed",
            path.display()
        );
    }
    assert_eq!(
        names(&claude_dir),
        BTreeSet::from(["settings.json".to_owned()])
    );
    assert_eq!(
        names(&codex_dir),
        BTreeSet::from(["config.toml".to_owned(), "sessions".to_owned()]),
        "only the fake's own rollout was added"
    );
}
