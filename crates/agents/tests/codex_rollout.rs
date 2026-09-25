//! C4 rollouts and Codex notify (spec 6.2/6.4, ADR-0004, R26, R28): record parsing over the scrubbed S3b captures, both
//! `update_plan` shapes, line framing, discovery, and the thread binding that ignores the title-generation micro-turn.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ply_agents::codex::notify::NotifyPayload;
use ply_agents::codex::rollout::{
    FramedLine, LineBuffer, MAX_LINE_BYTES, RolloutCandidate, RolloutRecord, parse_record,
    parse_update_plan, pick_rollout_by_cwd, rollout_thread_id,
};
use ply_agents::plan::ItemStatus;
use ply_agents::{
    AdapterSignal, AgentEvent, AgentSession, Error, LaunchSpec, StatusSignal, adapter,
};
use ply_proto::hook::HookEnvelope;
use ply_proto::pane::{AgentCli, Cli, Progress};
use serde_json::{Value, json};

const REAL_THREAD: &str = "00000000-0000-7000-8000-000000000001";
const TITLE_THREAD: &str = "00000000-0000-7000-8000-000000000027";

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/codex")
        .join(name)
}

fn lines(name: &str) -> Vec<Vec<u8>> {
    std::fs::read(fixture_path(name))
        .unwrap()
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

fn notify(name: &str) -> HookEnvelope {
    HookEnvelope {
        v: 1,
        pane_id: 3,
        cli: AgentCli::Codex,
        event: None,
        payload: serde_json::from_slice(&std::fs::read(fixture_path(name)).unwrap()).unwrap(),
    }
}

fn spec(resume: Option<&str>) -> LaunchSpec {
    LaunchSpec {
        cli: Cli::Codex,
        argv: vec!["codex".into()],
        env: Default::default(),
        cwd: "/example/workspace".into(),
        worktree: None,
        resume: resume.map(str::to_owned),
    }
}

fn session(resume: Option<&str>) -> Box<dyn AgentSession> {
    adapter(AgentCli::Codex).new_session(&spec(resume))
}

fn feed_rollout(s: &mut dyn AgentSession, name: &str) -> Vec<AdapterSignal> {
    lines(name)
        .iter()
        .flat_map(|l| s.handle(AgentEvent::RolloutLine(l)).unwrap())
        .collect()
}

const TURN_COMPLETE: AdapterSignal = AdapterSignal::Status(StatusSignal::TurnComplete);

#[test]
fn every_captured_record_parses_and_none_is_unknown() {
    for name in [
        "rollout-basic-session-meta-turn-context.jsonl",
        "rollout-update-plan-code-mode.jsonl",
        "rollout-approval-and-resume-source.jsonl",
    ] {
        let records: Vec<RolloutRecord> = lines(name)
            .iter()
            .map(|l| parse_record(l).unwrap())
            .collect();
        assert!(
            matches!(records[0], RolloutRecord::SessionMeta(_)),
            "{name}"
        );
        let metas = records
            .iter()
            .filter(|r| matches!(r, RolloutRecord::SessionMeta(_)))
            .count();
        assert_eq!(metas, 1, "{name}");
        assert!(
            !records
                .iter()
                .any(|r| matches!(r, RolloutRecord::Unknown(_))),
            "{name}"
        );
    }
}

#[test]
fn session_meta_and_turn_context_carry_thread_cwd_and_model() {
    let records: Vec<RolloutRecord> = lines("rollout-basic-session-meta-turn-context.jsonl")
        .iter()
        .map(|l| parse_record(l).unwrap())
        .collect();
    let RolloutRecord::SessionMeta(meta) = &records[0] else {
        panic!()
    };
    assert_eq!(meta.thread_id, "00000000-0000-7000-8000-000000000014");
    assert_eq!(meta.cwd, "/example/workspace");
    assert_eq!(meta.cli_version.as_deref(), Some("0.156.1"));
    let context = records
        .iter()
        .find_map(|r| match r {
            RolloutRecord::TurnContext(c) => Some(c),
            _ => None,
        })
        .unwrap();
    assert_eq!(context.model.as_deref(), Some("gpt-6-sol"));
}

#[test]
fn code_mode_update_plan_becomes_progress() {
    let plans: Vec<_> = lines("rollout-update-plan-code-mode.jsonl")
        .iter()
        .filter_map(|l| match parse_record(l).unwrap() {
            RolloutRecord::PlanUpdate(plan) => Some(plan),
            _ => None,
        })
        .collect();
    assert_eq!(plans.len(), 1);
    let steps: Vec<(&str, ItemStatus)> = plans[0]
        .steps
        .iter()
        .map(|s| (s.step.as_str(), s.status))
        .collect();
    assert_eq!(
        steps,
        [
            ("add README", ItemStatus::InProgress),
            ("commit", ItemStatus::Pending)
        ]
    );
    assert_eq!(
        plans[0].progress(),
        Some(Progress {
            done: 0,
            total: 2,
            current: Some("add README".into())
        })
    );
}

#[test]
fn function_call_update_plan_reads_json_arguments() {
    let args = json!({"explanation": "two steps",
        "plan": [{"step": "add README", "status": "completed"}, {"step": "commit", "status": "in_progress"}]});
    let item = json!({"type": "function_call", "name": "update_plan", "arguments": args.to_string(), "call_id": "call_1"});
    let plan = parse_update_plan(&item).unwrap().unwrap();
    assert_eq!(plan.explanation.as_deref(), Some("two steps"));
    assert_eq!(
        plan.progress(),
        Some(Progress {
            done: 1,
            total: 2,
            current: Some("commit".into())
        })
    );
    let line = json!({"timestamp": "2026-09-25T07:45:47.442Z", "ordinal": 9, "type": "response_item", "payload": item});
    assert!(matches!(
        parse_record(line.to_string().as_bytes()).unwrap(),
        RolloutRecord::PlanUpdate(_)
    ));
}

#[test]
fn update_plan_edge_cases() {
    let exec = |input: &str| json!({"type": "custom_tool_call", "name": "exec", "input": input});
    let two = "await tools.update_plan({plan:[{step:'a',status:'completed'}]});\n\
               await tools.update_plan ({ plan: [ {step: `a`, status: \"completed\"}, {step: 'b', status: 'in_progress'}, ] });";
    let plan = parse_update_plan(&exec(two)).unwrap().unwrap();
    assert_eq!(plan.steps.len(), 2);
    for commented in [
        "await tools.update_plan /* the plan */ ({plan: [{step: 'a', status: 'pending'}]});",
        "await tools.update_plan // the plan\n  ({plan: [{step: 'a', status: 'pending'}]});",
    ] {
        let plan = parse_update_plan(&exec(commented)).unwrap().unwrap();
        assert_eq!(plan.steps.len(), 1, "{commented}");
    }

    assert!(
        parse_update_plan(&exec("const r = await tools.exec_command({cmd:\"ls\"});"))
            .unwrap()
            .is_none()
    );
    assert!(parse_update_plan(&exec("await tools.update_plan(next);")).is_err());
    assert!(parse_update_plan(&exec("await tools.update_plan({steps: []});")).is_err());
    assert!(
        parse_update_plan(&json!({"type": "function_call", "name": "shell", "arguments": "{}"}))
            .unwrap()
            .is_none()
    );
    assert!(
        parse_update_plan(
            &json!({"type": "function_call", "name": "update_plan", "arguments": "{bad"})
        )
        .is_err()
    );
    assert!(
        parse_update_plan(&json!({"type": "message", "role": "assistant"}))
            .unwrap()
            .is_none()
    );
    let empty = parse_update_plan(&exec("tools.update_plan({plan: []})"))
        .unwrap()
        .unwrap();
    assert_eq!(empty.progress(), None);
}

#[test]
fn unknown_types_are_skipped_and_counted_and_bad_lines_are_errors() {
    let unknown =
        br#"{"timestamp":"2026-09-25T07:00:00Z","ordinal":1,"type":"future_thing","payload":{}}"#;
    assert_eq!(
        parse_record(unknown).unwrap(),
        RolloutRecord::Unknown("future_thing".into())
    );
    assert!(matches!(parse_record(b"not json"), Err(Error::Json { .. })));
    assert!(parse_record(br#"{"type":"session_meta"}"#).is_err());
    assert!(parse_record(br#"{"type":"session_meta","payload":{"cwd":"/example"}}"#).is_err());

    let mut s = session(None);
    assert!(
        s.handle(AgentEvent::RolloutLine(unknown))
            .unwrap()
            .is_empty()
    );
    assert!(s.handle(AgentEvent::RolloutLine(b"{truncated")).is_err());
    let stats = s.stats();
    assert_eq!(
        (stats.unknown_rollout_records, stats.malformed_rollout_lines),
        (1, 1)
    );
}

#[test]
fn line_buffer_frames_any_chunking_and_drops_oversized_lines() {
    let mut buf = LineBuffer::default();
    assert!(buf.push(b"{\"a\":").is_empty());
    assert_eq!(buf.pending(), 5);
    assert_eq!(
        buf.push(b"1}\n{\"b\":2}\n{\"c\""),
        [
            FramedLine::Line(b"{\"a\":1}".to_vec()),
            FramedLine::Line(b"{\"b\":2}".to_vec())
        ]
    );
    assert_eq!(
        buf.push(b":3}\n"),
        [FramedLine::Line(b"{\"c\":3}".to_vec())]
    );

    let big = vec![b'x'; MAX_LINE_BYTES];
    assert!(buf.push(&big).is_empty());
    assert!(buf.push(b"yy").is_empty());
    assert_eq!(buf.pending(), 0);
    let out = buf.push(b"z\nok\n");
    assert_eq!(
        out,
        [
            FramedLine::TooLong(MAX_LINE_BYTES + 3),
            FramedLine::Line(b"ok".to_vec())
        ]
    );
    assert!(matches!(
        out[0].clone().into_line(),
        Err(Error::RolloutLineTooLong { .. })
    ));
}

#[test]
fn rollout_file_names_yield_their_thread() {
    let name = format!("rollout-2026-09-25T09-34-14-{REAL_THREAD}.jsonl");
    assert_eq!(rollout_thread_id(&name), Some(REAL_THREAD));
    for bad in [
        "rollout-2026-09-25T09-34-14.jsonl".to_owned(),
        format!("rollout-{REAL_THREAD}.json"),
        format!("session-2026-09-25T09-34-14-{REAL_THREAD}.jsonl"),
        "rollout-2026-09-25T09-34-14-0000000000007000800000000000000000.jsonl".to_owned(),
    ] {
        assert_eq!(rollout_thread_id(&bad), None, "{bad}");
    }
}

#[test]
fn cwd_discovery_takes_the_newest_rollout_after_spawn() {
    let spawn = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let at = |s: u64, cwd: &str, name: &str| RolloutCandidate {
        path: PathBuf::from(name),
        created: SystemTime::UNIX_EPOCH + Duration::from_secs(s),
        cwd: cwd.into(),
    };
    let candidates = [
        at(900, "/example/workspace", "before-spawn"),
        at(1_010, "/example/workspace", "first"),
        at(1_020, "/example/other", "other-cwd"),
        at(1_030, "/example/workspace", "newest"),
    ];
    let pick = pick_rollout_by_cwd(&candidates, spawn, "/example/workspace").unwrap();
    assert_eq!(pick.path, PathBuf::from("newest"));
    assert!(pick_rollout_by_cwd(&candidates, spawn, "/example/none").is_none());
}

#[test]
fn notify_payloads_parse_with_kebab_case_keys() {
    let real = NotifyPayload::from_value(&notify("notify-turn.json").payload).unwrap();
    assert!(real.is_turn_complete());
    assert_eq!(real.thread_id, REAL_THREAD);
    assert_eq!(real.cwd, "/example/workspace");
    assert_eq!(real.client.as_deref(), Some("codex-tui"));
    assert_eq!(real.input_messages, ["create a file a.txt containing hi"]);
    assert_eq!(
        real.last_assistant_message.as_deref(),
        Some("Created [a.txt](/example/workspace/a.txt) containing `hi`.")
    );
    let golden: Value = serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../proto/tests/golden/c3/hook.codex.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        NotifyPayload::from_value(&golden["payload"])
            .unwrap()
            .client
            .is_none()
    );
    assert!(NotifyPayload::from_value(&json!({"type": "agent-turn-complete"})).is_err());
    assert!(
        NotifyPayload::from_value(&json!({"type": "agent-turn-complete", "thread-id": "",
        "turn-id": "t", "cwd": "/x"}))
        .is_err()
    );
}

#[test]
fn title_turn_notify_is_ignored_and_the_real_thread_binds() {
    let mut s = session(None);
    assert_eq!(
        s.handle(AgentEvent::Hook(&notify("notify-title-turn.json")))
            .unwrap(),
        [AdapterSignal::FindRollout {
            thread_id: TITLE_THREAD.into()
        }]
    );
    let signals = feed_rollout(&mut *s, "rollout-approval-and-resume-source.jsonl");
    assert!(
        !signals.contains(&TURN_COMPLETE),
        "the title turn has no rollout: {signals:?}"
    );
    assert_eq!(s.meta().session_ref.as_deref(), Some(REAL_THREAD));
    assert_eq!(s.meta().model.as_deref(), Some("gpt-6-sol"));
    assert_eq!(s.meta().cwd.as_deref(), Some("/example/workspace"));

    assert_eq!(
        s.handle(AgentEvent::Hook(&notify("notify-turn.json")))
            .unwrap(),
        [TURN_COMPLETE]
    );
    assert!(
        s.handle(AgentEvent::Hook(&notify("notify-title-turn.json")))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_notify_before_its_rollout_completes_the_turn_once_bound() {
    let mut s = session(None);
    assert_eq!(
        s.handle(AgentEvent::Hook(&notify("notify-turn.json")))
            .unwrap(),
        [AdapterSignal::FindRollout {
            thread_id: REAL_THREAD.into()
        }]
    );
    let first = lines("rollout-approval-and-resume-source.jsonl").remove(0);
    let signals = s.handle(AgentEvent::RolloutLine(&first)).unwrap();
    assert!(
        matches!(signals.as_slice(), [AdapterSignal::Meta(_), TURN_COMPLETE]),
        "{signals:?}"
    );
    assert!(
        !s.handle(AgentEvent::RolloutLine(&first))
            .unwrap()
            .contains(&TURN_COMPLETE)
    );
}

#[test]
fn a_second_notify_before_binding_keeps_the_first_threads_turn() {
    for order in [
        ["notify-turn.json", "notify-title-turn.json"],
        ["notify-title-turn.json", "notify-turn.json"],
    ] {
        let mut s = session(None);
        for file in order {
            let signals = s.handle(AgentEvent::Hook(&notify(file))).unwrap();
            assert!(
                matches!(signals.as_slice(), [AdapterSignal::FindRollout { .. }]),
                "{signals:?}"
            );
        }
        let first = lines("rollout-approval-and-resume-source.jsonl").remove(0);
        let signals = s.handle(AgentEvent::RolloutLine(&first)).unwrap();
        assert!(
            signals.contains(&TURN_COMPLETE),
            "{order:?}: the real thread's turn survives the title turn's notify: {signals:?}"
        );
        assert_eq!(s.stats().forgotten_turns, 0);
    }
}

#[test]
fn a_resumed_pane_is_bound_from_the_start() {
    let mut s = session(Some(REAL_THREAD));
    assert_eq!(s.meta().session_ref.as_deref(), Some(REAL_THREAD));
    assert_eq!(
        s.handle(AgentEvent::Hook(&notify("notify-turn.json")))
            .unwrap(),
        [TURN_COMPLETE]
    );
    let other = lines("rollout-basic-session-meta-turn-context.jsonl").remove(0);
    assert!(
        s.handle(AgentEvent::RolloutLine(&other))
            .unwrap()
            .is_empty()
    );
    assert_eq!(s.meta().session_ref.as_deref(), Some(REAL_THREAD));
}

#[test]
fn plan_updates_in_the_rollout_emit_progress() {
    let mut s = session(None);
    let signals = feed_rollout(&mut *s, "rollout-update-plan-code-mode.jsonl");
    let progress: Vec<&Option<Progress>> = signals
        .iter()
        .filter_map(|x| match x {
            AdapterSignal::Progress(p) => Some(p),
            _ => None,
        })
        .collect();
    let want = Progress {
        done: 0,
        total: 2,
        current: Some("add README".into()),
    };
    assert_eq!(progress, [&Some(want.clone())]);
    assert_eq!(s.progress(), Some(&want));
}

#[test]
fn keys_and_first_output_drive_the_codex_rows() {
    let mut s = session(None);
    assert_eq!(
        s.handle(AgentEvent::FirstOutput).unwrap(),
        [AdapterSignal::Status(StatusSignal::Ready)]
    );
    assert_eq!(
        s.handle(AgentEvent::KeyTyped { enter: false }).unwrap(),
        [AdapterSignal::Status(StatusSignal::KeyTyped)]
    );
    assert_eq!(
        s.handle(AgentEvent::KeyTyped { enter: true }).unwrap(),
        [
            AdapterSignal::Status(StatusSignal::KeyTyped),
            AdapterSignal::Status(StatusSignal::PromptSubmitted)
        ]
    );
}

#[test]
fn claude_envelopes_and_non_turn_notifies_are_refused_or_counted() {
    let mut s = session(None);
    let claude = HookEnvelope {
        cli: AgentCli::Claude,
        ..notify("notify-turn.json")
    };
    assert!(matches!(
        s.handle(AgentEvent::Hook(&claude)),
        Err(Error::WrongCli { .. })
    ));
    let mut other = notify("notify-turn.json");
    other.payload["type"] = json!("approval-requested");
    assert!(s.handle(AgentEvent::Hook(&other)).unwrap().is_empty());
    assert_eq!(s.stats().unknown_hook_events, 1);
}
