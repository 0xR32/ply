//! C8 OSC 9 classification (ADR-0004's table, Ruling R27) and what each class signals to the spec 6.3 machine.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use ply_agents::codex::osc9::{Osc9Kind, classify_osc9};
use ply_agents::{AdapterSignal, AgentEvent, LaunchSpec, StatusSignal, adapter};
use ply_proto::pane::{AgentCli, Cli};

#[test]
fn every_fixed_prefix_of_the_table_classifies() {
    let cases = [
        (
            "Approval requested: cargo test --workspace",
            Osc9Kind::Approval,
        ),
        ("Codex wants to edit src/main.rs", Osc9Kind::Approval),
        ("Codex wants to edit 3 files", Osc9Kind::Approval),
        ("Approval requested by github", Osc9Kind::Approval),
        (
            "Plan mode prompt: Implement the parser?",
            Osc9Kind::PlanPrompt,
        ),
        ("Question: Which database?", Osc9Kind::Question),
    ];
    for (body, kind) in cases {
        assert_eq!(classify_osc9(body), kind, "{body}");
    }
}

#[test]
fn every_other_body_is_a_completed_turn() {
    for body in [
        "Agent turn complete",
        "Created [a.txt](/example/workspace/a.txt) containing `hi`.",
        "",
        "Approval requested",
        "approval requested: ls",
        "question: lower case",
        "Codex wants to",
        " Question: leading space",
    ] {
        assert_eq!(classify_osc9(body), Osc9Kind::TurnComplete, "{body:?}");
    }
}

#[test]
fn the_observed_bodies_classify_as_recorded() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/codex/osc9-observed-messages.txt");
    let text = std::fs::read_to_string(path).unwrap();
    let kinds: Vec<Osc9Kind> = text.lines().map(classify_osc9).collect();
    assert_eq!(
        kinds,
        [
            Osc9Kind::Approval,
            Osc9Kind::Approval,
            Osc9Kind::TurnComplete
        ]
    );
}

#[test]
fn sessions_turn_classes_into_status_signals() {
    let spec = LaunchSpec {
        cli: Cli::Codex,
        argv: vec!["codex".into()],
        env: Default::default(),
        cwd: "/example/workspace".into(),
        worktree: None,
        resume: None,
    };
    let mut s = adapter(AgentCli::Codex).new_session(&spec);
    let status = |s: &mut dyn ply_agents::AgentSession, body: &str| {
        s.handle(AgentEvent::Osc9(body)).unwrap()
    };
    assert_eq!(
        status(&mut *s, "Codex wants to edit a.txt"),
        [AdapterSignal::Status(StatusSignal::PermissionRequested {
            call: None,
            detail: Some("Codex wants to edit a.txt".into())
        })]
    );
    assert_eq!(
        status(&mut *s, "Question: Which database?"),
        [AdapterSignal::Status(StatusSignal::InputRequested {
            detail: Some("Question: Which database?".into())
        })]
    );
    assert_eq!(
        status(&mut *s, "Plan mode prompt: Ship it?"),
        [AdapterSignal::Status(StatusSignal::InputRequested {
            detail: Some("Plan mode prompt: Ship it?".into())
        })]
    );
    assert_eq!(
        status(&mut *s, "Done."),
        [AdapterSignal::Status(StatusSignal::TurnComplete)]
    );
}
