//! The task queue against a real plyd and the fake CLIs (Ruling R60): tasks are queued, listed, reordered, cancelled
//! and paused over C1, survive a restart held, and are typed into an idle pane one per turn.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::fake::{Fake, WAIT_FOR_START, hook_program, install, typed, wait_status};
use common::{Control, Data, Sandbox, eventually};
use ply_proto::control::{ErrorCode, Event};
use ply_proto::pane::{BlockReason, PaneStatus, PauseReason, Task, TaskState};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

fn code(err: &ply_proto::control::ErrorBody) -> ErrorCode {
    err.code
}

fn task_of(v: Value) -> Task {
    serde_json::from_value(v).unwrap()
}

fn changed(c: &mut Control, id: u64, state: TaskState) -> Task {
    match c.wait_event(
        WAIT,
        |e| matches!(e, Event::TaskChanged(t) if t.id == id && t.state == state),
    ) {
        Some(Event::TaskChanged(t)) => *t,
        other => panic!("task {id} did not become {state:?}: {other:?}"),
    }
}

fn add(c: &mut Control, ws: u64, pane: u64, text: &str) -> Task {
    task_of(
        c.call(
            "task.add",
            json!({"workspace_id": ws, "target": {"pane": pane}, "text": text}),
        )
        .unwrap(),
    )
}

fn claude_pane(c: &mut Control, sb: &Sandbox, ws: u64, prompt: Option<&str>) -> u64 {
    let mut p = json!({"workspace_id": ws, "cli": "claude", "cwd": sb.home});
    if let Some(prompt) = prompt {
        p["prompt"] = json!(prompt);
    }
    c.call("pane.create", p).unwrap()["id"].as_u64().unwrap()
}

#[test]
fn tasks_are_queued_listed_moved_cancelled_and_paused_over_c1() {
    let sb = Sandbox::new("queue");
    install(&sb);
    hook_program();
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = claude_pane(&mut c, &sb, ws, Some(WAIT_FOR_START));
    let _fake = Fake::ready(&sb, pane);

    let a = add(&mut c, ws, pane, "/review-pr #212");
    let b = add(&mut c, ws, pane, "then write the summary\nin two lines");
    assert_eq!((a.state, a.position, b.position), (TaskState::Queued, 0, 1));
    changed(&mut c, b.id, TaskState::Queued);

    c.call("task.move", json!({"task_id": b.id, "position": 0}))
        .unwrap();
    let list = c.call("task.list", json!({"workspace_id": ws})).unwrap();
    let order: Vec<u64> = list["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_u64().unwrap())
        .collect();
    assert_eq!(order, [b.id, a.id]);

    c.call("task.cancel", json!({"task_id": b.id})).unwrap();
    changed(&mut c, b.id, TaskState::Cancelled);
    let again = c.call("task.cancel", json!({"task_id": b.id})).unwrap_err();
    assert_eq!(code(&again), ErrorCode::InvalidState);

    c.call("queue.pause", json!({"pane_id": pane, "paused": true}))
        .unwrap();
    let paused = c.wait_event(
        WAIT,
        |e| matches!(e, Event::QueueChanged(q) if q.pane_id == pane),
    );
    assert!(
        matches!(&paused, Some(Event::QueueChanged(q)) if q.paused == Some(PauseReason::User)),
        "{paused:?}"
    );

    let shell = sb.shell(&mut c, ws);
    let err = c
        .call(
            "task.add",
            json!({"workspace_id": ws, "target": {"pane": shell}, "text": "ls"}),
        )
        .unwrap_err();
    assert_eq!(code(&err), ErrorCode::BadRequest);
    let err = c
        .call(
            "task.add",
            json!({"workspace_id": ws, "target": {"pane": pane}, "text": "a\rb"}),
        )
        .unwrap_err();
    assert_eq!(
        code(&err),
        ErrorCode::BadRequest,
        "a carriage return is a key, not text"
    );
}

#[test]
fn a_restart_keeps_the_queued_tasks_and_holds_their_queue() {
    let sb = Sandbox::new("queue-restart");
    install(&sb);
    hook_program();
    let mut plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = claude_pane(&mut c, &sb, ws, None);
    let _fake = Fake::ready(&sb, pane);
    wait_status(&mut c, pane, PaneStatus::Idle, WAIT);
    c.call("queue.pause", json!({"pane_id": pane, "paused": true}))
        .unwrap();
    let a = add(&mut c, ws, pane, "first");
    let b = add(&mut c, ws, pane, "second");
    drop(c);
    assert!(plyd.stop().success());

    let _plyd = sb.start();
    let mut c = Control::connect(&sb.control_socket()).unwrap();
    let list = c.call("task.list", json!({"workspace_id": ws})).unwrap();
    let tasks: Vec<Task> = serde_json::from_value(list["tasks"].clone()).unwrap();
    let ids: Vec<(u64, TaskState, u32)> =
        tasks.iter().map(|t| (t.id, t.state, t.position)).collect();
    assert_eq!(
        ids,
        [(a.id, TaskState::Queued, 0), (b.id, TaskState::Queued, 1)]
    );
    assert_eq!(
        list["queues"],
        json!([{"pane_id": pane, "paused": "restored"}])
    );
}

const TURN: Duration = Duration::from_secs(20);

#[test]
fn two_tasks_are_typed_into_a_claude_pane_one_per_turn() {
    let sb = Sandbox::new("typed-claude");
    install(&sb);
    hook_program();
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = claude_pane(&mut c, &sb, ws, None);
    let fake = Fake::ready(&sb, pane);
    wait_status(&mut c, pane, PaneStatus::Idle, WAIT);
    fake.send("submit 1");

    let a = add(&mut c, ws, pane, "/review-pr #212");
    let b = add(&mut c, ws, pane, "then write the summary\nin two lines");
    let sent_a = changed(&mut c, a.id, TaskState::Sent);
    changed(&mut c, a.id, TaskState::Running);
    let ended_a = match c.wait_event(
        TURN,
        |e| matches!(e, Event::TaskChanged(t) if t.id == a.id && t.state == TaskState::Ended),
    ) {
        Some(Event::TaskChanged(t)) => *t,
        other => panic!("task a did not end: {other:?}"),
    };
    let sent_b = changed(&mut c, b.id, TaskState::Sent);
    changed(&mut c, b.id, TaskState::Running);
    assert!(
        c.wait_event(
            TURN,
            |e| matches!(e, Event::TaskChanged(t) if t.id == b.id && t.state == TaskState::Ended)
        )
        .is_some()
    );
    assert!(sent_a.sent_at.is_some());
    assert!(
        sent_b.sent_at >= ended_a.ended_at,
        "the second task waits for the first turn to end"
    );
    assert_eq!(
        typed(&sb, "claude"),
        ["/review-pr #212", "then write the summary\\nin two lines"],
        "typed verbatim, a paste's lines kept together"
    );
}

#[test]
fn unsent_typing_blocks_the_queue_until_the_user_sends_the_task() {
    let sb = Sandbox::new("typed-block");
    install(&sb);
    hook_program();
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = claude_pane(&mut c, &sb, ws, None);
    let fake = Fake::ready(&sb, pane);
    wait_status(&mut c, pane, PaneStatus::Idle, WAIT);
    fake.send("submit 1");
    let (mut d, first) = Data::attach(&sb.data_socket(), pane, 80, 24).unwrap();
    d.apply(&first, true).unwrap();
    d.input(b"half-").unwrap();

    let task = add(&mut c, ws, pane, "typed anyway");
    let blocked = c.wait_event(
        WAIT,
        |e| matches!(e, Event::QueueChanged(q) if q.pane_id == pane),
    );
    assert!(
        matches!(&blocked, Some(Event::QueueChanged(q)) if q.blocked == Some(BlockReason::Typing)),
        "{blocked:?}"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        typed(&sb, "claude").is_empty(),
        "nothing typed over the user's input"
    );

    c.call("task.send", json!({"task_id": task.id})).unwrap();
    changed(&mut c, task.id, TaskState::Running);
    assert!(eventually(WAIT, || !typed(&sb, "claude").is_empty()));
    assert_eq!(
        typed(&sb, "claude"),
        ["half-typed anyway"],
        "sent over the user's text, as they chose"
    );
}

#[test]
fn a_task_the_cli_never_acknowledges_fails_and_pauses_the_queue() {
    let sb = Sandbox::new("typed-fail");
    install(&sb);
    hook_program();
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = claude_pane(&mut c, &sb, ws, None);
    let fake = Fake::ready(&sb, pane);
    wait_status(&mut c, pane, PaneStatus::Idle, WAIT);
    fake.send("submit none");
    let a = add(&mut c, ws, pane, "never acknowledged");
    let b = add(&mut c, ws, pane, "never typed");
    changed(&mut c, a.id, TaskState::Sent);
    let failed = match c.wait_event(
        TURN,
        |e| matches!(e, Event::TaskChanged(t) if t.id == a.id && t.state == TaskState::Failed),
    ) {
        Some(Event::TaskChanged(t)) => *t,
        other => panic!("task a did not fail: {other:?}"),
    };
    assert!(failed.detail.unwrap().starts_with("not submitted"));
    let list = c.call("task.list", json!({"workspace_id": ws})).unwrap();
    assert_eq!(
        list["queues"],
        json!([{"pane_id": pane, "paused": "failed"}])
    );
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(typed(&sb, "claude"), ["never acknowledged"]);
    let still = c.call("task.list", json!({"workspace_id": ws})).unwrap();
    let b_state = still["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == b.id)
        .unwrap()["state"]
        .clone();
    assert_eq!(b_state, "queued");
}

#[test]
fn a_pool_task_goes_to_the_free_pane_of_its_cli_in_its_folder() {
    let sb = Sandbox::new("typed-pool");
    install(&sb);
    hook_program();
    let project = sb.home.join("project");
    let inside = project.join("sub");
    let elsewhere = sb.home.join("elsewhere");
    for dir in [&inside, &elsewhere] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let open = |c: &mut Control, cli: &str, cwd: &std::path::Path| {
        c.call(
            "pane.create",
            json!({"workspace_id": ws, "cli": cli, "cwd": cwd}),
        )
        .unwrap()["id"]
            .as_u64()
            .unwrap()
    };
    let mut panes = Vec::new();
    for (cli, cwd) in [
        ("claude", &elsewhere),
        ("codex", &project),
        ("claude", &inside),
    ] {
        let pane = open(&mut c, cli, cwd);
        let fake = Fake::ready(&sb, pane);
        wait_status(&mut c, pane, PaneStatus::Idle, WAIT);
        if cli == "codex" {
            fake.send("session");
        }
        fake.send("submit 1");
        panes.push(pane);
    }
    let near = panes[2];
    let task = task_of(
        c.call(
            "task.add",
            json!({"workspace_id": ws, "target": {"pool": {"cli": "claude", "cwd": project}}, "text": "/triage"}),
        )
        .unwrap(),
    );
    assert_eq!(task.pane_id, None);
    let running = changed(&mut c, task.id, TaskState::Running);
    assert_eq!(
        running.pane_id,
        Some(near),
        "the Claude pane in the folder, not the one elsewhere or Codex"
    );
    assert!(running.pool.is_some());
    assert!(
        c.wait_event(
            TURN,
            |e| matches!(e, Event::TaskChanged(t) if t.id == task.id && t.state == TaskState::Ended)
        )
        .is_some()
    );
    assert_eq!(typed(&sb, "claude"), ["/triage"], "typed once");
    assert!(typed(&sb, "codex").is_empty());
}

fn codex_pane(c: &mut Control, sb: &Sandbox, ws: u64) -> u64 {
    c.call(
        "pane.create",
        json!({"workspace_id": ws, "cli": "codex", "cwd": sb.home}),
    )
    .unwrap()["id"]
        .as_u64()
        .unwrap()
}

fn queues(c: &mut Control, ws: u64) -> Value {
    c.call("task.list", json!({"workspace_id": ws})).unwrap()["queues"].clone()
}

fn ended(c: &mut Control, id: u64) {
    assert!(
        c.wait_event(
            TURN,
            |e| matches!(e, Event::TaskChanged(t) if t.id == id && t.state == TaskState::Ended)
        )
        .is_some(),
        "task {id} did not end"
    );
}

#[test]
fn a_resumed_codex_pane_takes_no_task_before_its_prompt() {
    let sb = Sandbox::new("typed-resumed");
    install(&sb);
    hook_program();
    let mut plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = codex_pane(&mut c, &sb, ws);
    let fake = Fake::ready(&sb, pane);
    wait_status(&mut c, pane, PaneStatus::Idle, WAIT);
    fake.send("session");
    fake.send("turn task_started u1");
    fake.send("turn task_complete u1");
    for status in [PaneStatus::Running, PaneStatus::Idle] {
        wait_status(&mut c, pane, status, WAIT);
    }
    drop(c);
    plyd.child.kill().unwrap();
    plyd.child.wait().unwrap();

    let _plyd = sb.start();
    let mut c = Control::connect(&sb.control_socket()).unwrap();
    assert!(eventually(WAIT, || {
        c.call("pane.list", json!({"workspace_id": ws})).unwrap()[0]["status"] == "idle"
    }));
    let fake = Fake::ready(&sb, pane);
    fake.send("submit 1");
    let task = add(&mut c, ws, pane, "not into a startup screen");
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        typed(&sb, "codex").is_empty(),
        "the resumed Codex may still show a trust, hooks or update screen"
    );
    assert_eq!(
        queues(&mut c, ws),
        json!([{"pane_id": pane, "blocked": "startup"}])
    );

    fake.send("turn task_started u2");
    fake.send("turn task_complete u2");
    changed(&mut c, task.id, TaskState::Running);
    ended(&mut c, task.id);
    assert_eq!(typed(&sb, "codex"), ["not into a startup screen"]);
}

#[test]
fn a_codex_task_queued_before_its_first_turn_goes_after_it_and_runs_once_its_rollout_starts_a_turn()
{
    let sb = Sandbox::new("typed-first-turn");
    install(&sb);
    hook_program();
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = codex_pane(&mut c, &sb, ws);
    let fake = Fake::ready(&sb, pane);
    wait_status(&mut c, pane, PaneStatus::Idle, WAIT);
    let task = add(&mut c, ws, pane, "$review-pr");
    std::thread::sleep(Duration::from_secs(2));
    assert!(typed(&sb, "codex").is_empty());
    assert_eq!(
        queues(&mut c, ws),
        json!([{"pane_id": pane, "blocked": "startup"}])
    );

    fake.send("session");
    fake.send("submit 1");
    fake.send("turn task_started u1");
    fake.send("turn task_complete u1");
    changed(&mut c, task.id, TaskState::Sent);
    changed(&mut c, task.id, TaskState::Running);
    ended(&mut c, task.id);
    assert_eq!(typed(&sb, "codex"), ["$review-pr"]);
}

#[test]
fn a_pool_task_goes_to_a_pane_opened_after_it() {
    let sb = Sandbox::new("typed-pool-later");
    install(&sb);
    hook_program();
    let project = sb.home.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let task = task_of(
        c.call(
            "task.add",
            json!({"workspace_id": ws, "target": {"pool": {"cli": "claude", "cwd": project}}, "text": "/triage"}),
        )
        .unwrap(),
    );
    let pane = c
        .call(
            "pane.create",
            json!({"workspace_id": ws, "cli": "claude", "cwd": project}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let fake = Fake::ready(&sb, pane);
    wait_status(&mut c, pane, PaneStatus::Idle, WAIT);
    fake.send("submit 1");
    let running = changed(&mut c, task.id, TaskState::Running);
    assert_eq!(running.pane_id, Some(pane));
    ended(&mut c, task.id);
    assert_eq!(typed(&sb, "claude"), ["/triage"]);
}
