//! The Claude account switch against a real plyd and fake-claude: `account.list` reads `claude auth status`,
//! `account.switch` runs `claude auth login` in a tab of its own, and a new sign-in restarts every Claude pane on it
//! once at rest (resuming a session it can); a failed, cancelled or unchanged sign-in moves
//! nothing. Codex's sign-in, against fake-codex: `codex.account` reads `$CODEX_HOME/auth.json`, `codex.sign_in` runs
//! `codex login` in a tab of its own, and a new account restarts every Codex pane on it the same way.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::fake::{
    Fake, claude_payload, claude_session, codex_thread, hook_program, install, launches,
    sign_in_done, wait_status,
};
use common::{Control, Sandbox, eventually};
use ply_proto::control::{AccountChanged, CodexChanged, ErrorCode, Event};
use ply_proto::pane::{PaneStatus, SignInOutcome};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

const PERSONAL: &str = r#"{"loggedIn":true,"authMethod":"claude.ai","email":"someone@example.com","orgId":"00000000-0000-4000-8000-0000000000a1","orgName":"someone@example.com's Organization","subscriptionType":"max"}"#;

const TEAM: &str = r#"{"loggedIn":true,"authMethod":"claude.ai","email":"someone@example.com","orgId":"00000000-0000-4000-8000-0000000000a2","orgName":"Example Team","subscriptionType":"team"}"#;

fn signed_in(sb: &Sandbox, status: &str) {
    std::fs::write(sb.home.join("fake-claude-auth.json"), status).unwrap();
}

fn account_of(status: &str) -> Value {
    let s: Value = serde_json::from_str(status).unwrap();
    json!({
        "email": s["email"],
        "org_id": s["orgId"],
        "org_name": s["orgName"],
        "plan": s["subscriptionType"],
    })
}

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

fn runs_of(sb: &Sandbox, pane: u64) -> Vec<String> {
    runs_in(sb, "claude", pane)
}

fn runs_in(sb: &Sandbox, cli: &str, pane: u64) -> Vec<String> {
    let tag = format!("launch pane={pane} ");
    launches(sb, cli)
        .into_iter()
        .filter(|l| l.starts_with(&tag))
        .collect()
}

fn turn(sb: &Sandbox, pane: u64, event: &str, extra: Value) {
    Fake::ready(sb, pane).hook(event, &claude_payload(pane, &sb.home, event, extra));
}

fn take_turn(sb: &Sandbox, c: &mut Control, ws: u64, pane: u64) {
    turn(sb, pane, "UserPromptSubmit", json!({"prompt": "hello"}));
    wait_listed(c, ws, pane, "status", &json!("running"));
    turn(sb, pane, "Stop", json!({}));
    wait_listed(c, ws, pane, "status", &json!("idle"));
}

fn switch(c: &mut Control, ws: u64, account: Option<Value>) -> u64 {
    let mut p = json!({"workspace_id": ws});
    if let Some(account) = account {
        p["account"] = account;
    }
    c.call("account.switch", p).unwrap()["id"].as_u64().unwrap()
}

fn changed(c: &mut Control) -> AccountChanged {
    match c.wait_event(WAIT, |e| matches!(e, Event::AccountChanged(_))) {
        Some(Event::AccountChanged(a)) => a,
        other => panic!("no account.changed: {other:?}"),
    }
}

fn sign_ins(sb: &Sandbox) -> Vec<String> {
    std::fs::read_to_string(sb.home.join("fake-claude-auth.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_new_sign_in_restarts_every_claude_pane_on_it_once_at_rest() {
    let sb = Sandbox::new("acct");
    install(&sb);
    hook_program();
    signed_in(&sb, PERSONAL);
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();

    let listed_before = c.call("account.list", json!({})).unwrap();
    assert_eq!(listed_before["current"], account_of(PERSONAL));
    assert_eq!(listed_before["known"], json!([account_of(PERSONAL)]));

    let talked = create(&mut c, ws, "claude", &sb.home);
    let busy = create(&mut c, ws, "claude", &sb.home);
    let fresh = create(&mut c, ws, "claude", &sb.home);
    let shell = create(&mut c, ws, "shell", &sb.home);
    for pane in [talked, busy, fresh] {
        wait_listed(&mut c, ws, pane, "status", &json!("idle"));
    }
    take_turn(&sb, &mut c, ws, talked);
    take_turn(&sb, &mut c, ws, busy);
    turn(
        &sb,
        busy,
        "UserPromptSubmit",
        json!({"prompt": "a long job"}),
    );
    wait_listed(&mut c, ws, busy, "status", &json!("running"));

    let sign_in = switch(&mut c, ws, Some(account_of(TEAM)));
    let pane = listed(&mut c, ws, sign_in);
    assert_eq!(pane["cli"], "shell");
    let layout = c.call("layout.get", json!({"workspace_id": ws})).unwrap();
    let tab = layout["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == pane["tab_id"])
        .cloned()
        .unwrap();
    assert_eq!(tab["name"], "Claude sign-in");
    assert!(eventually(WAIT, || !sign_ins(&sb).is_empty()));
    assert_eq!(
        sign_ins(&sb),
        ["login --claudeai --email someone@example.com"]
    );
    assert_eq!(
        c.call("account.list", json!({})).unwrap()["signing_in"],
        sign_in
    );
    let again = c
        .call("account.switch", json!({"workspace_id": ws}))
        .unwrap_err();
    assert_eq!(again.code, ErrorCode::InvalidState, "{again:?}");

    c.events.clear();
    sign_in_done(&sb, &format!("ok {TEAM}"));
    let done = changed(&mut c);
    assert_eq!(done.outcome, SignInOutcome::SignedIn);
    assert_eq!(done.pane_id, sign_in);
    assert_eq!(done.panes, 3, "every live Claude pane, not the shell");
    assert_eq!(
        serde_json::to_value(&done.account).unwrap(),
        account_of(TEAM)
    );
    assert_eq!(
        listed(&mut c, ws, sign_in),
        Value::Null,
        "the sign-in pane closed"
    );

    wait_status(&mut c, talked, PaneStatus::Starting, WAIT);
    wait_listed(&mut c, ws, talked, "status", &json!("idle"));
    let runs = runs_of(&sb, talked);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(
        runs[1].contains(&format!("--resume {}", claude_session(talked))),
        "{runs:?}"
    );
    wait_listed(&mut c, ws, fresh, "status", &json!("idle"));
    assert!(eventually(WAIT, || runs_of(&sb, fresh).len() == 2));
    assert!(
        !runs_of(&sb, fresh)[1].contains("--resume"),
        "a session without a prompt has nothing to resume, so it starts afresh: {:?}",
        runs_of(&sb, fresh)
    );
    assert_eq!(listed(&mut c, ws, shell)["status"], "idle");

    assert_eq!(runs_of(&sb, busy).len(), 1, "a running pane is not stopped");
    assert_eq!(listed(&mut c, ws, busy)["status"], "running");
    turn(&sb, busy, "Stop", json!({}));
    wait_status(&mut c, busy, PaneStatus::Starting, WAIT);
    wait_listed(&mut c, ws, busy, "status", &json!("idle"));
    let runs = runs_of(&sb, busy);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(
        runs[1].contains(&format!("--resume {}", claude_session(busy))),
        "{runs:?}"
    );

    let after = c.call("account.list", json!({})).unwrap();
    assert_eq!(after["current"], account_of(TEAM));
    assert_eq!(
        after["known"],
        json!([account_of(TEAM), account_of(PERSONAL)]),
        "one email, a personal and a Team account"
    );
    assert_eq!(after.get("signing_in"), None);
    let stored = std::fs::read_to_string(sb.ply_home.join("accounts.json")).unwrap();
    assert!(stored.contains("Example Team"), "{stored}");
}

#[test]
fn a_failed_cancelled_or_unchanged_sign_in_restarts_nothing() {
    let sb = Sandbox::new("acctno");
    install(&sb);
    hook_program();
    signed_in(&sb, PERSONAL);
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = create(&mut c, ws, "claude", &sb.home);
    wait_listed(&mut c, ws, pane, "status", &json!("idle"));
    take_turn(&sb, &mut c, ws, pane);

    let failed = switch(&mut c, ws, None);
    sign_in_done(&sb, "fail 3");
    let done = changed(&mut c);
    assert_eq!(done.outcome, SignInOutcome::Failed);
    assert_eq!(done.panes, 0);
    let kept = listed(&mut c, ws, failed);
    assert_eq!(
        kept["status"], "exited",
        "the failed sign-in stays to be read"
    );
    assert_eq!(kept["exit_code"], 3);
    assert_eq!(
        sign_ins(&sb),
        ["login --claudeai"],
        "no account named, no email"
    );
    c.call("pane.close", json!({"pane_id": failed, "kill": false}))
        .unwrap();

    let cancelled = switch(&mut c, ws, Some(account_of(TEAM)));
    assert!(eventually(WAIT, || sb.home.join("fake-login.cmd").exists()));
    c.call("pane.close", json!({"pane_id": cancelled, "kill": true}))
        .unwrap();
    assert_eq!(changed(&mut c).outcome, SignInOutcome::Cancelled);
    std::fs::remove_file(sb.home.join("fake-login.cmd")).unwrap();

    let unchanged = switch(&mut c, ws, Some(account_of(TEAM)));
    sign_in_done(&sb, &format!("ok {PERSONAL}"));
    let done = changed(&mut c);
    assert_eq!(done.outcome, SignInOutcome::Unchanged);
    assert_eq!(done.panes, 0);
    assert_eq!(listed(&mut c, ws, unchanged), Value::Null);

    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        runs_of(&sb, pane).len(),
        1,
        "the Claude pane kept its process"
    );
    assert_eq!(listed(&mut c, ws, pane)["status"], "idle");
}

const CODEX_PLUS: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJlbWFpbCI6InNvbWVvbmVAZXhhbXBsZS5jb20iLCJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9wbGFuX3R5cGUiOiJwbHVzIn19.c2lnbmF0dXJl";

const CODEX_TEAM: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJlbWFpbCI6Im90aGVyQGV4YW1wbGUuY29tIiwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfcGxhbl90eXBlIjoidGVhbSJ9fQ.c2lnbmF0dXJl";

const CODEX_API_KEY: &str = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-example","tokens":null}"#;

fn codex_auth(id_token: &str) -> String {
    json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": id_token,
            "access_token": "example-access-token",
            "refresh_token": "example-refresh-token",
            "account_id": "00000000-0000-4000-8000-0000000000c1"
        },
        "last_refresh": "2026-11-01T00:00:00Z"
    })
    .to_string()
}

fn codex_changed(c: &mut Control) -> CodexChanged {
    match c.wait_event(WAIT, |e| matches!(e, Event::CodexChanged(_))) {
        Some(Event::CodexChanged(a)) => a,
        other => panic!("no codex.changed: {other:?}"),
    }
}

fn codex_sign_in(c: &mut Control, ws: u64) -> u64 {
    c.call("codex.sign_in", json!({"workspace_id": ws}))
        .unwrap()["id"]
        .as_u64()
        .unwrap()
}

fn codex_logins(sb: &Sandbox) -> Vec<String> {
    std::fs::read_to_string(sb.home.join("fake-codex-auth.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_new_codex_sign_in_restarts_every_codex_pane_on_it_once_at_rest() {
    let sb = Sandbox::new("cxacct");
    install(&sb);
    hook_program();
    let codex_home = sb.root.join("codex-home");
    std::fs::create_dir_all(&codex_home).unwrap();
    let auth = codex_home.join("auth.json");
    std::fs::write(&auth, codex_auth(CODEX_PLUS)).unwrap();
    let _plyd = sb.start_with(&[("CODEX_HOME", codex_home.to_str().unwrap())]);
    let (mut c, ws) = sb.control();

    assert_eq!(
        c.call("codex.account", json!({})).unwrap(),
        json!({"current": {"email": "someone@example.com", "plan": "plus", "api_key": false}})
    );

    let talked = create(&mut c, ws, "codex", &sb.home);
    // A directory of its own, so its rollout tailer cannot claim the other pane's new rollout.
    let fresh_dir = sb.home.join("fresh");
    std::fs::create_dir_all(&fresh_dir).unwrap();
    let fresh = create(&mut c, ws, "codex", &fresh_dir);
    let claude = create(&mut c, ws, "claude", &sb.home);
    let fake = Fake::ready(&sb, talked);
    fake.send("session");
    wait_listed(
        &mut c,
        ws,
        talked,
        "session_ref",
        &json!(codex_thread(talked)),
    );
    fake.send("turn task_started turn-1");
    wait_listed(&mut c, ws, talked, "status", &json!("running"));
    fake.send("turn task_complete turn-1");
    for pane in [talked, fresh, claude] {
        wait_listed(&mut c, ws, pane, "status", &json!("idle"));
    }

    let sign_in = codex_sign_in(&mut c, ws);
    let pane = listed(&mut c, ws, sign_in);
    assert_eq!(pane["cli"], "shell");
    let layout = c.call("layout.get", json!({"workspace_id": ws})).unwrap();
    let tab = layout["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == pane["tab_id"])
        .cloned()
        .unwrap();
    assert_eq!(tab["name"], "Codex sign-in");
    assert!(eventually(WAIT, || !codex_logins(&sb).is_empty()));
    assert_eq!(codex_logins(&sb), ["login"], "codex login and nothing else");
    assert_eq!(
        c.call("codex.account", json!({})).unwrap()["signing_in"],
        sign_in
    );
    let again = c
        .call("codex.sign_in", json!({"workspace_id": ws}))
        .unwrap_err();
    assert_eq!(again.code, ErrorCode::InvalidState, "{again:?}");

    c.events.clear();
    sign_in_done(&sb, &format!("ok {}", codex_auth(CODEX_TEAM)));
    let done = codex_changed(&mut c);
    assert_eq!(done.outcome, SignInOutcome::SignedIn);
    assert_eq!(done.pane_id, sign_in);
    assert_eq!(done.panes, 2, "every live Codex pane, not the Claude one");
    assert_eq!(
        serde_json::to_value(&done.account).unwrap(),
        json!({"email": "other@example.com", "plan": "team", "api_key": false})
    );
    assert_eq!(
        listed(&mut c, ws, sign_in),
        Value::Null,
        "the sign-in pane closed"
    );

    wait_status(&mut c, talked, PaneStatus::Starting, WAIT);
    wait_listed(&mut c, ws, talked, "status", &json!("idle"));
    let runs = runs_in(&sb, "codex", talked);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(
        runs[1].contains(&format!("resume {}", codex_thread(talked))),
        "{runs:?}"
    );
    assert!(eventually(WAIT, || runs_in(&sb, "codex", fresh).len() == 2));
    assert!(
        !runs_in(&sb, "codex", fresh)[1].contains("resume"),
        "a thread without a turn has nothing to resume, so it starts afresh"
    );
    wait_listed(&mut c, ws, fresh, "status", &json!("idle"));
    assert_eq!(
        runs_of(&sb, claude).len(),
        1,
        "the Claude pane kept its process"
    );

    assert_eq!(
        c.call("codex.account", json!({})).unwrap(),
        json!({"current": {"email": "other@example.com", "plan": "team", "api_key": false}})
    );
    assert_eq!(
        std::fs::read_to_string(&auth).unwrap().trim_end(),
        codex_auth(CODEX_TEAM),
        "plyd left Codex's own auth.json as codex login wrote it"
    );
}

#[test]
fn a_failed_cancelled_or_unchanged_codex_sign_in_restarts_nothing() {
    let sb = Sandbox::new("cxacctno");
    install(&sb);
    hook_program();
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    assert_eq!(
        c.call("codex.account", json!({})).unwrap(),
        json!({}),
        "no auth.json is no sign-in"
    );
    let codex_home = sb.home.join(".codex");
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::write(codex_home.join("auth.json"), CODEX_API_KEY).unwrap();
    assert_eq!(
        c.call("codex.account", json!({})).unwrap(),
        json!({"current": {"api_key": true}})
    );
    let pane = create(&mut c, ws, "codex", &sb.home);
    let fake = Fake::ready(&sb, pane);
    fake.send("session");
    wait_listed(&mut c, ws, pane, "session_ref", &json!(codex_thread(pane)));
    wait_listed(&mut c, ws, pane, "status", &json!("idle"));

    let failed = codex_sign_in(&mut c, ws);
    sign_in_done(&sb, "fail 3");
    let done = codex_changed(&mut c);
    assert_eq!(done.outcome, SignInOutcome::Failed);
    assert_eq!((done.account, done.panes), (None, 0));
    let kept = listed(&mut c, ws, failed);
    assert_eq!(
        kept["status"], "exited",
        "the failed sign-in stays to be read"
    );
    assert_eq!(kept["exit_code"], 3);
    c.call("pane.close", json!({"pane_id": failed, "kill": false}))
        .unwrap();

    let cancelled = codex_sign_in(&mut c, ws);
    assert!(eventually(WAIT, || sb.home.join("fake-login.cmd").exists()));
    c.call("pane.close", json!({"pane_id": cancelled, "kill": true}))
        .unwrap();
    assert_eq!(codex_changed(&mut c).outcome, SignInOutcome::Cancelled);
    std::fs::remove_file(sb.home.join("fake-login.cmd")).unwrap();

    let unchanged = codex_sign_in(&mut c, ws);
    sign_in_done(&sb, &format!("ok {CODEX_API_KEY}"));
    let done = codex_changed(&mut c);
    assert_eq!(done.outcome, SignInOutcome::Unchanged);
    assert_eq!(done.panes, 0);
    assert_eq!(listed(&mut c, ws, unchanged), Value::Null);

    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        runs_in(&sb, "codex", pane).len(),
        1,
        "the Codex pane kept its process"
    );
    assert_eq!(listed(&mut c, ws, pane)["status"], "idle");
}
