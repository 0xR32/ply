//! A CLI updated under running panes: when the app connects, an agent pane at rest whose process runs another version
//! than the one its CLI's install now names is relaunched onto it through the CLI's own resume; a busy pane waits for
//! the next connection, and a pane holding the user's unsent typing is left alone.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use common::fake::{
    Fake, claude_payload, claude_session, codex_thread, hook_program, install, launches, no_status,
    wait_status,
};
use common::{Control, Data, Sandbox, eventually};
use ply_proto::pane::PaneStatus;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

/// How long a test waits to see that nothing is relaunched.
const QUIET: Duration = Duration::from_millis(1500);

/// Past the time a pane must stay quiet after the user's last key before it counts as at rest.
const SETTLED: Duration =
    ply_daemon::panes::dispatch::SETTLE.saturating_add(Duration::from_millis(300));

fn create(c: &mut Control, ws: u64, cli: &str, cwd: &Path) -> u64 {
    c.call(
        "pane.create",
        json!({"workspace_id": ws, "cli": cli, "cwd": cwd}),
    )
    .unwrap()["id"]
        .as_u64()
        .unwrap()
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

/// Asserts `pane` changes no status within [`QUIET`], counting only what happens from now on.
fn stays(c: &mut Control, pane: u64) {
    c.events.clear();
    no_status(c, pane, QUIET);
}

/// The launch log lines of `pane`.
fn runs_of(sb: &Sandbox, cli: &str, pane: u64) -> Vec<String> {
    let tag = format!("launch pane={pane} ");
    launches(sb, cli)
        .into_iter()
        .filter(|l| l.starts_with(&tag))
        .collect()
}

/// One prompt and its turn in the fake Claude Code of `pane`, so its session has a conversation to resume.
fn take_turn(sb: &Sandbox, c: &mut Control, ws: u64, pane: u64) {
    let fake = Fake::ready(sb, pane);
    let prompt = json!({"prompt": "hello"});
    fake.hook(
        "UserPromptSubmit",
        &claude_payload(pane, &sb.home, "UserPromptSubmit", prompt),
    );
    wait_listed(c, ws, pane, "status", &json!("running"));
    fake.hook("Stop", &claude_payload(pane, &sb.home, "Stop", json!({})));
    wait_listed(c, ws, pane, "status", &json!("idle"));
}

/// Lays fake-claude out as Claude Code's native installer does, `$HOME/bin/claude` → `…/claude/versions/<version>`, swapping the link as its updater does.
fn install_claude(sb: &Sandbox, version: &str) {
    let versions = sb.home.join(".local/share/claude/versions");
    std::fs::create_dir_all(&versions).unwrap();
    let exe = versions.join(version);
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake/fake-claude.sh");
    std::fs::copy(fake, &exe).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let next = sb.home.join("bin/.claude.next");
    let _ = std::fs::remove_file(&next);
    std::os::unix::fs::symlink(&exe, &next).unwrap();
    std::fs::rename(&next, sb.home.join("bin/claude")).unwrap();
}

/// Lays fake-codex out as Codex's standalone package, `$HOME/bin/codex` → `<pkg>/bin/codex` beside `<pkg>/codex-package.json`; a later call updates it in place.
fn install_codex(sb: &Sandbox, version: &str) {
    let pkg = sb.home.join("codex-pkg");
    let exe = pkg.join("bin/codex");
    if !exe.exists() {
        std::fs::create_dir_all(pkg.join("bin")).unwrap();
        std::fs::rename(sb.home.join("bin/codex"), &exe).unwrap();
        std::os::unix::fs::symlink(&exe, sb.home.join("bin/codex")).unwrap();
    }
    std::fs::write(
        pkg.join("codex-package.json"),
        json!({"version": version}).to_string(),
    )
    .unwrap();
}

#[test]
fn an_idle_pane_moves_to_its_clis_update_when_the_app_connects_and_a_busy_one_at_the_next() {
    let sb = Sandbox::new("upd");
    install(&sb);
    hook_program();
    install_claude(&sb, "2.1.300");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let idle = create(&mut c, ws, "claude", &sb.home);
    let busy = create(&mut c, ws, "claude", &sb.home);
    let shell = create(&mut c, ws, "shell", &sb.home);
    wait_listed(&mut c, ws, idle, "status", &json!("idle"));
    wait_listed(&mut c, ws, busy, "status", &json!("idle"));
    take_turn(&sb, &mut c, ws, idle);
    Fake::ready(&sb, idle).send("out before-the-update");
    let (mut view, first) = Data::attach(&sb.data_socket(), idle, 80, 24).unwrap();
    view.apply(&first, true).unwrap();
    assert!(
        view.pump_until(WAIT, |d| d.shows("before-the-update"))
            .unwrap()
    );
    Fake::ready(&sb, busy).hook(
        "UserPromptSubmit",
        &claude_payload(
            busy,
            &sb.home,
            "UserPromptSubmit",
            json!({"prompt": "a long job"}),
        ),
    );
    wait_listed(&mut c, ws, busy, "status", &json!("running"));

    install_claude(&sb, "2.1.301");
    let _cli = Control::connect(&sb.control_socket()).unwrap();
    stays(&mut c, idle);
    assert_eq!(
        runs_of(&sb, "claude", idle).len(),
        1,
        "only the app's connection reloads"
    );

    let _app = Control::connect_as(&sb.control_socket(), "ply-app").unwrap();
    wait_status(&mut c, idle, PaneStatus::Starting, WAIT);
    wait_status(&mut c, idle, PaneStatus::Idle, WAIT);
    let runs = runs_of(&sb, "claude", idle);
    assert_eq!(runs.len(), 2, "one launch and one reload: {runs:?}");
    assert!(
        runs[1].contains(&format!("--resume {}", claude_session(idle))),
        "{runs:?}"
    );
    assert_eq!(listed(&mut c, ws, idle)["cli"], "claude");
    assert_eq!(listed(&mut c, ws, idle)["exit_code"], Value::Null);
    Fake::ready(&sb, idle).send("out after-the-update");
    assert!(
        view.pump_until(WAIT, |d| d.shows("after-the-update"))
            .unwrap(),
        "{:?}",
        view.screen()
    );
    assert!(
        !view.shows("before-the-update"),
        "the reloaded pane starts on a reset terminal: {:?}",
        view.screen()
    );
    assert_eq!(view.exit, None, "a reload reports no exit to the view");
    let (mut fresh, first) = Data::attach(&sb.data_socket(), idle, 80, 24).unwrap();
    fresh.apply(&first, true).unwrap();
    assert_eq!(view.screen(), fresh.screen(), "the view followed the reset");
    assert_eq!(
        runs_of(&sb, "claude", busy).len(),
        1,
        "a running pane is not stopped"
    );
    assert_eq!(listed(&mut c, ws, busy)["status"], "running");
    assert_eq!(listed(&mut c, ws, shell)["status"], "idle");

    let _again = Control::connect_as(&sb.control_socket(), "ply-app").unwrap();
    stays(&mut c, idle);
    assert_eq!(
        runs_of(&sb, "claude", idle).len(),
        2,
        "the reloaded pane is current"
    );

    Fake::ready(&sb, busy).hook("Stop", &claude_payload(busy, &sb.home, "Stop", json!({})));
    wait_listed(&mut c, ws, busy, "status", &json!("idle"));
    stays(&mut c, busy);
    assert_eq!(
        runs_of(&sb, "claude", busy).len(),
        1,
        "no reload while the app stays open"
    );
    let _reopened = Control::connect_as(&sb.control_socket(), "ply-app").unwrap();
    wait_status(&mut c, busy, PaneStatus::Starting, WAIT);
    wait_status(&mut c, busy, PaneStatus::Idle, WAIT);
    let runs = runs_of(&sb, "claude", busy);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(
        runs[1].contains(&format!("--resume {}", claude_session(busy))),
        "{runs:?}"
    );
}

#[test]
fn unsent_typing_keeps_a_pane_on_its_old_version_until_it_is_cleared() {
    let sb = Sandbox::new("updtyped");
    install(&sb);
    hook_program();
    install_claude(&sb, "2.1.300");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = create(&mut c, ws, "claude", &sb.home);
    wait_listed(&mut c, ws, pane, "status", &json!("idle"));
    take_turn(&sb, &mut c, ws, pane);
    let (mut d, first) = Data::attach(&sb.data_socket(), pane, 80, 24).unwrap();
    d.apply(&first, true).unwrap();
    d.input(b"half a thought").unwrap();
    assert!(
        d.pump_until(WAIT, |d| d.shows("half a thought")).unwrap(),
        "the pty echoes what plyd wrote: {:?}",
        d.screen()
    );
    install_claude(&sb, "2.1.301");
    std::thread::sleep(SETTLED);
    let _app = Control::connect_as(&sb.control_socket(), "ply-app").unwrap();
    stays(&mut c, pane);
    assert_eq!(runs_of(&sb, "claude", pane).len(), 1);

    d.input(b"\x15").unwrap();
    std::thread::sleep(SETTLED);
    let _reopened = Control::connect_as(&sb.control_socket(), "ply-app").unwrap();
    wait_status(&mut c, pane, PaneStatus::Starting, WAIT);
    wait_status(&mut c, pane, PaneStatus::Idle, WAIT);
    assert_eq!(
        runs_of(&sb, "claude", pane).len(),
        2,
        "Ctrl+U cleared the input"
    );
}

#[test]
fn a_codex_pane_follows_an_in_place_update_of_its_package() {
    let sb = Sandbox::new("updcodex");
    install(&sb);
    hook_program();
    install_codex(&sb, "0.160.0");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = create(&mut c, ws, "codex", &sb.home);
    let fake = Fake::ready(&sb, pane);
    fake.send("session");
    wait_listed(&mut c, ws, pane, "session_ref", &json!(codex_thread(pane)));
    fake.send("turn task_started turn-1");
    wait_listed(&mut c, ws, pane, "status", &json!("running"));
    fake.send("turn task_complete turn-1");
    wait_listed(&mut c, ws, pane, "status", &json!("idle"));

    let _app = Control::connect_as(&sb.control_socket(), "ply-app").unwrap();
    stays(&mut c, pane);

    install_codex(&sb, "0.161.0");
    let _reopened = Control::connect_as(&sb.control_socket(), "ply-app").unwrap();
    wait_status(&mut c, pane, PaneStatus::Starting, WAIT);
    wait_status(&mut c, pane, PaneStatus::Idle, WAIT);
    let runs = runs_of(&sb, "codex", pane);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(
        runs[1].contains(&format!("resume {}", codex_thread(pane))),
        "{runs:?}"
    );
}

#[test]
fn a_session_without_a_prompt_stays_since_its_cli_has_nothing_to_resume() {
    let sb = Sandbox::new("updnew");
    install(&sb);
    hook_program();
    install_claude(&sb, "2.1.300");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let fresh = create(&mut c, ws, "claude", &sb.home);
    let cleared = create(&mut c, ws, "claude", &sb.home);
    wait_listed(&mut c, ws, fresh, "status", &json!("idle"));
    wait_listed(&mut c, ws, cleared, "status", &json!("idle"));
    take_turn(&sb, &mut c, ws, cleared);
    let after_clear = "00000000-0000-4000-8000-0000000000cc";
    let mut start = claude_payload(
        cleared,
        &sb.home,
        "SessionStart",
        json!({"source": "clear"}),
    );
    start["session_id"] = json!(after_clear);
    Fake::ready(&sb, cleared).hook("SessionStart", &start);
    wait_listed(&mut c, ws, cleared, "session_ref", &json!(after_clear));

    install_claude(&sb, "2.1.301");
    std::thread::sleep(SETTLED);
    let _app = Control::connect_as(&sb.control_socket(), "ply-app").unwrap();
    stays(&mut c, fresh);
    stays(&mut c, cleared);
    assert_eq!(runs_of(&sb, "claude", fresh).len(), 1);
    assert_eq!(runs_of(&sb, "claude", cleared).len(), 1);
}
