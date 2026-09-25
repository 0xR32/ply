//! Claude Code progress (spec 6.4, R16): TodoWrite and the Task tools from PostToolUse payloads, hidden while neither appears.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use ply_agents::claude::progress::TodoState;
use ply_agents::{AdapterSignal, AgentEvent, AgentSession, LaunchSpec, adapter};
use ply_proto::hook::HookEnvelope;
use ply_proto::pane::{AgentCli, Cli, Progress};
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude")
        .join(name);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn session() -> Box<dyn AgentSession> {
    adapter(AgentCli::Claude).new_session(&LaunchSpec {
        cli: Cli::Claude,
        argv: vec!["claude".into()],
        env: Default::default(),
        cwd: "/Users/example/project".into(),
        worktree: None,
        resume: None,
    })
}

fn post_tool_use(tool: &str, input: Value, response: Value) -> HookEnvelope {
    let mut payload = fixture("PostToolUse.json");
    payload["tool_name"] = json!(tool);
    payload["tool_input"] = input;
    payload["tool_response"] = response;
    HookEnvelope {
        v: 1,
        pane_id: 7,
        cli: AgentCli::Claude,
        event: Some("PostToolUse".into()),
        payload,
    }
}

fn progress(done: u32, total: u32, current: Option<&str>) -> Option<Progress> {
    Some(Progress {
        done,
        total,
        current: current.map(str::to_owned),
    })
}

fn todos(items: &[(&str, &str)]) -> Value {
    let todos: Vec<Value> = items
        .iter()
        .map(|(content, status)| json!({"content": content, "status": status, "activeForm": format!("Doing {content}")}))
        .collect();
    json!({ "todos": todos })
}

#[test]
fn progress_stays_hidden_while_no_plan_tool_appears() {
    let mut s = session();
    for (event, file) in [
        ("SessionStart", "SessionStart.json"),
        ("UserPromptSubmit", "UserPromptSubmit.json"),
        ("PreToolUse", "PreToolUse.json"),
        ("PostToolUse", "PostToolUse.json"),
        ("PostToolUseFailure", "PostToolUseFailure.json"),
        ("Stop", "Stop.json"),
    ] {
        let env = HookEnvelope {
            v: 1,
            pane_id: 7,
            cli: AgentCli::Claude,
            event: Some(event.into()),
            payload: fixture(file),
        };
        let signals = s.handle(AgentEvent::Hook(&env)).unwrap();
        assert!(
            !signals
                .iter()
                .any(|x| matches!(x, AdapterSignal::Progress(_))),
            "{event}"
        );
    }
    assert_eq!(s.progress(), None);
}

#[test]
fn todo_write_replaces_the_whole_list() {
    let mut s = session();
    let first = post_tool_use(
        "TodoWrite",
        todos(&[
            ("read code", "completed"),
            ("write fix", "in_progress"),
            ("run tests", "pending"),
        ]),
        json!({}),
    );
    let signals = s.handle(AgentEvent::Hook(&first)).unwrap();
    let want = progress(1, 3, Some("write fix"));
    assert_eq!(signals.last(), Some(&AdapterSignal::Progress(want.clone())));
    assert_eq!(s.progress(), want.as_ref());

    let same = s.handle(AgentEvent::Hook(&first)).unwrap();
    assert!(!same.iter().any(|x| matches!(x, AdapterSignal::Progress(_))));

    let done = post_tool_use(
        "TodoWrite",
        todos(&[("read code", "completed"), ("write fix", "completed")]),
        json!({}),
    );
    s.handle(AgentEvent::Hook(&done)).unwrap();
    assert_eq!(s.progress(), progress(2, 2, None).as_ref());

    let cleared = s
        .handle(AgentEvent::Hook(&post_tool_use(
            "TodoWrite",
            todos(&[]),
            json!({}),
        )))
        .unwrap();
    assert_eq!(cleared.last(), Some(&AdapterSignal::Progress(None)));
}

#[test]
fn task_tools_build_a_list_from_single_changes() {
    let mut state = TodoState::default();
    assert!(
        state
            .apply(
                "TaskCreate",
                &json!({"subject": "Read code"}),
                Some(&json!({"task": {"id": "1"}}))
            )
            .unwrap()
    );
    assert!(
        state
            .apply(
                "TaskCreate",
                &json!({"subject": "Write fix"}),
                Some(&json!("Task #2 created successfully"))
            )
            .unwrap()
    );
    assert!(
        state
            .apply("TaskCreate", &json!({"subject": "Run tests"}), None)
            .unwrap()
    );
    assert_eq!(state.progress(), progress(0, 3, None));

    assert!(
        state
            .apply(
                "TaskUpdate",
                &json!({"taskId": "1", "status": "completed"}),
                None
            )
            .unwrap()
    );
    assert!(
        state
            .apply(
                "TaskUpdate",
                &json!({"taskId": "2", "status": "in_progress"}),
                None
            )
            .unwrap()
    );
    assert_eq!(state.progress(), progress(1, 3, Some("Write fix")));

    assert!(
        state
            .apply(
                "TaskUpdate",
                &json!({"taskId": "3", "status": "deleted"}),
                None
            )
            .unwrap()
    );
    assert_eq!(state.progress(), progress(1, 2, Some("Write fix")));

    assert!(
        state
            .apply(
                "TaskUpdate",
                &json!({"taskId": 9, "status": "pending", "subject": "Late"}),
                None
            )
            .unwrap()
    );
    assert_eq!(state.progress(), progress(1, 3, Some("Write fix")));
    assert!(
        !state
            .apply(
                "TaskUpdate",
                &json!({"taskId": "9", "status": "pending"}),
                None
            )
            .unwrap()
    );
}

#[test]
fn the_last_used_source_wins() {
    let mut state = TodoState::default();
    state
        .apply("TaskCreate", &json!({"subject": "task"}), None)
        .unwrap();
    state
        .apply(
            "TodoWrite",
            &todos(&[("a", "completed"), ("b", "pending")]),
            None,
        )
        .unwrap();
    assert_eq!(state.progress(), progress(1, 2, None));
    state
        .apply(
            "TaskUpdate",
            &json!({"taskId": "1", "status": "in_progress"}),
            None,
        )
        .unwrap();
    assert_eq!(state.progress(), progress(0, 1, Some("task")));
}

#[test]
fn other_tools_and_unreadable_calls_leave_the_plan_alone() {
    let mut state = TodoState::default();
    assert!(
        !state
            .apply("Write", &json!({"file_path": "/Users/example/a"}), None)
            .unwrap()
    );
    assert!(
        !state
            .apply("TaskStop", &json!({"task_id": "1"}), None)
            .unwrap()
    );
    assert!(
        state
            .apply("TodoWrite", &json!({"items": []}), None)
            .is_err()
    );
    assert!(
        state
            .apply("TaskUpdate", &json!({"status": "completed"}), None)
            .is_err()
    );
    assert_eq!(state.progress(), None);

    let mut s = session();
    let bad = post_tool_use("TodoWrite", json!({"todos": "nope"}), json!({}));
    let signals = s.handle(AgentEvent::Hook(&bad)).unwrap();
    assert!(
        signals
            .iter()
            .any(|x| matches!(x, AdapterSignal::Status(_))),
        "status survives: {signals:?}"
    );
    assert_eq!(s.stats().unreadable_progress, 1);
    assert_eq!(s.progress(), None);
}
