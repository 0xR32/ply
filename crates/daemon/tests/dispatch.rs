//! The task queue against a real plyd and the fake CLIs (Ruling R60): tasks are queued, listed, reordered, cancelled
//! and paused over C1, survive a restart held, and are typed into an idle pane one per turn.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::fake::{Fake, WAIT_FOR_START, hook_program, install, wait_status};
use common::{Control, Sandbox};
use ply_proto::control::{ErrorCode, Event};
use ply_proto::pane::{PaneStatus, PauseReason, Task, TaskState};
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
