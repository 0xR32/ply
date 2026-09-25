//! Claude Code hook payloads (the scrubbed S2b captures) to spec 6.3 signals and `pane.meta`, per ADR-0003 and R15–R17.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use ply_agents::{
    AdapterSignal, AgentEvent, AgentSession, LaunchSpec, SessionMeta, StatusSignal, adapter,
};
use ply_proto::hook::HookEnvelope;
use ply_proto::pane::{AgentCli, Cli};
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude")
        .join(name);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn envelope(event: &str, payload: Value) -> HookEnvelope {
    HookEnvelope {
        v: 1,
        pane_id: 7,
        cli: AgentCli::Claude,
        event: Some(event.to_owned()),
        payload,
    }
}

fn session() -> Box<dyn AgentSession> {
    adapter(AgentCli::Claude).new_session(&LaunchSpec {
        cli: Cli::Claude,
        argv: vec!["claude".into()],
        env: Default::default(),
        cwd: "/Users/example/ply-scratch/s2b/project2".into(),
        worktree: None,
        resume: None,
    })
}

fn feed(session: &mut dyn AgentSession, event: &str, file: &str) -> Vec<AdapterSignal> {
    session
        .handle(AgentEvent::Hook(&envelope(event, fixture(file))))
        .unwrap()
}

fn statuses(signals: &[AdapterSignal]) -> Vec<StatusSignal> {
    signals
        .iter()
        .filter_map(|s| match s {
            AdapterSignal::Status(status) => Some(status.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn session_start_reports_ready_model_and_session_id() {
    let mut s = session();
    let signals = feed(&mut *s, "SessionStart", "SessionStart.json");
    let meta = SessionMeta {
        session_ref: Some("00000000-0000-4000-8000-000000000000".into()),
        model: Some("claude-opus-5-5[1m]".into()),
        cwd: Some("/Users/example/ply-scratch/s2b/project2".into()),
        worktree: None,
    };
    assert_eq!(
        signals,
        [
            AdapterSignal::Meta(meta.clone()),
            AdapterSignal::Status(StatusSignal::Ready)
        ]
    );
    assert_eq!(s.meta(), &meta);
    assert_eq!(
        statuses(&feed(&mut *s, "SessionStart", "SessionStart_resume.json")),
        [StatusSignal::Ready]
    );
    let mut compacted = fixture("SessionStart.json");
    compacted["source"] = json!("compact");
    let signals = s
        .handle(AgentEvent::Hook(&envelope("SessionStart", compacted)))
        .unwrap();
    assert!(
        statuses(&signals).is_empty(),
        "a compaction continues the turn: {signals:?}"
    );
}

#[test]
fn each_registered_event_maps_to_its_spec_6_3_signal() {
    let mut s = session();
    let cases: [(&str, &str, &str); 7] = [
        (
            "UserPromptSubmit",
            "UserPromptSubmit.json",
            "PromptSubmitted",
        ),
        ("PreToolUse", "PreToolUse.json", "ToolUse"),
        (
            "PermissionRequest",
            "PermissionRequest.json",
            "PermissionRequested",
        ),
        (
            "PostToolUseFailure",
            "PostToolUseFailure.json",
            "CallSettled",
        ),
        (
            "Notification",
            "Notification_idle_prompt.json",
            "InputRequested",
        ),
        ("Stop", "Stop.json", "TurnComplete"),
        ("SessionEnd", "SessionEnd.json", "SessionEnded"),
    ];
    for (event, file, want) in cases {
        let got = statuses(&feed(&mut *s, event, file));
        assert_eq!(got.len(), 1, "{event}: {got:?}");
        assert!(
            format!("{:?}", got[0]).starts_with(want),
            "{event}: {got:?}"
        );
    }
    assert_eq!(s.stats().unknown_hook_events, 0);
}

#[test]
fn permission_request_carries_the_call_that_post_tool_use_settles() {
    let mut s = session();
    let request = statuses(&feed(
        &mut *s,
        "PermissionRequest",
        "PermissionRequest.json",
    ));
    let [
        StatusSignal::PermissionRequested {
            call: Some(pending),
            detail,
        },
    ] = request.as_slice()
    else {
        panic!("{request:?}");
    };
    assert_eq!(pending.id, None);
    assert_eq!(pending.name, "Write");
    assert_eq!(detail.as_deref(), Some("Write"));

    let post = statuses(&feed(&mut *s, "PostToolUse", "PostToolUse.json"));
    let [
        StatusSignal::ToolUse(used),
        StatusSignal::CallSettled(settled),
    ] = post.as_slice()
    else {
        panic!("{post:?}");
    };
    assert_eq!(used, settled);
    assert_eq!(settled.id.as_deref(), Some("toolu_example1"));
    assert!(settled.same_call(pending));

    let failure = statuses(&feed(
        &mut *s,
        "PostToolUseFailure",
        "PostToolUseFailure.json",
    ));
    let [StatusSignal::CallSettled(read)] = failure.as_slice() else {
        panic!("{failure:?}");
    };
    assert!(!read.same_call(pending));

    let denied = json!({"session_id": "s", "cwd": "/Users/example", "tool_name": "Write",
        "tool_input": fixture("PermissionRequest.json")["tool_input"]});
    let signals = s
        .handle(AgentEvent::Hook(&envelope("PermissionDenied", denied)))
        .unwrap();
    let [.., AdapterSignal::Status(StatusSignal::CallSettled(call))] = signals.as_slice() else {
        panic!("{signals:?}");
    };
    assert!(call.same_call(pending));
}

#[test]
fn permission_prompt_notifications_leave_the_status_alone() {
    let mut s = session();
    assert!(
        statuses(&feed(
            &mut *s,
            "Notification",
            "Notification_permission_prompt.json"
        ))
        .is_empty()
    );
    let idle = statuses(&feed(
        &mut *s,
        "Notification",
        "Notification_idle_prompt.json",
    ));
    assert_eq!(
        idle,
        [StatusSignal::InputRequested {
            detail: Some("Claude is waiting for your input".into())
        }]
    );
}

#[test]
fn session_end_reports_the_reason() {
    let mut s = session();
    assert_eq!(
        statuses(&feed(&mut *s, "SessionEnd", "SessionEnd.json")),
        [StatusSignal::SessionEnded {
            reason: Some("prompt_input_exit".into())
        }]
    );
}

#[test]
fn unregistered_events_are_counted_not_acted_on() {
    let mut s = session();
    let signals = feed(&mut *s, "WorktreeCreate", "WorktreeCreate.json");
    assert!(statuses(&signals).is_empty());
    assert_eq!(s.stats().unknown_hook_events, 1);
}

#[test]
fn worktree_label_and_cwd_come_from_the_reported_cwd() {
    let mut s = session();
    let mut payload = fixture("SessionStart.json");
    payload["cwd"] = json!("/Users/example/repo/.claude/worktrees/feat-x");
    s.handle(AgentEvent::Hook(&envelope("SessionStart", payload)))
        .unwrap();
    assert_eq!(s.meta().worktree.as_deref(), Some("feat-x"));

    let changed = json!({"session_id": "other", "cwd": "/Users/example/repo/.claude/worktrees/feat-x",
        "new_cwd": "/Users/example/repo", "hook_event_name": "CwdChanged"});
    let signals = s
        .handle(AgentEvent::Hook(&envelope("CwdChanged", changed)))
        .unwrap();
    assert!(statuses(&signals).is_empty());
    assert_eq!(s.meta().cwd.as_deref(), Some("/Users/example/repo"));
    assert_eq!(s.meta().worktree, None);
    assert_eq!(
        s.meta().session_ref.as_deref(),
        Some("00000000-0000-4000-8000-000000000000")
    );
}

#[test]
fn a_new_session_start_replaces_the_session_id() {
    let mut s = session();
    feed(&mut *s, "SessionStart", "SessionStart.json");
    let mut cleared = fixture("SessionStart.json");
    cleared["session_id"] = json!("00000000-0000-4000-8000-000000000009");
    s.handle(AgentEvent::Hook(&envelope("SessionStart", cleared)))
        .unwrap();
    assert_eq!(
        s.meta().session_ref.as_deref(),
        Some("00000000-0000-4000-8000-000000000009")
    );
}

#[test]
fn keys_drive_the_r17_fallback_and_pty_events_are_ignored() {
    let mut s = session();
    for enter in [false, true] {
        let signals = s.handle(AgentEvent::KeyTyped { enter }).unwrap();
        assert_eq!(signals, [AdapterSignal::Status(StatusSignal::KeyTyped)]);
    }
    assert!(s.handle(AgentEvent::FirstOutput).unwrap().is_empty());
    assert!(
        s.handle(AgentEvent::Osc9("Claude needs your permission"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn malformed_envelopes_are_errors_that_change_nothing() {
    let mut s = session();
    let codex = HookEnvelope {
        cli: AgentCli::Codex,
        ..envelope("Stop", fixture("Stop.json"))
    };
    assert!(s.handle(AgentEvent::Hook(&codex)).is_err());
    assert!(
        s.handle(AgentEvent::Hook(&envelope("Stop", json!([1, 2]))))
            .is_err()
    );
    assert!(
        s.handle(AgentEvent::Hook(&envelope(
            "PreToolUse",
            json!({"cwd": "/x"})
        )))
        .is_err()
    );
    let unnamed = HookEnvelope {
        event: None,
        ..envelope("", json!({"session_id": "s"}))
    };
    assert!(s.handle(AgentEvent::Hook(&unnamed)).is_err());
    assert_eq!(s.meta().session_ref, None);
    let named_in_payload = HookEnvelope {
        event: None,
        ..envelope("", fixture("Stop.json"))
    };
    assert_eq!(
        statuses(&s.handle(AgentEvent::Hook(&named_in_payload)).unwrap()),
        [StatusSignal::TurnComplete]
    );
}
