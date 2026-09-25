//! Plan usage (Ruling R59): Claude Code's `cachedUsageUtilization` cache and the `rate_limits` of Codex's `token_count`
//! events, over a scrubbed synthetic `.claude.json` and synthetic rollouts beside the S3b captures.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use ply_agents::claude::usage::parse_usage_cache;
use ply_agents::codex::usage::{
    FileScan, LimitWindow, RateLimits, RolloutUsage, parse_rate_limits, turn_model,
};
use ply_proto::pane::{CliUsage, UsageWindow};

fn fixture(path: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(path),
    )
    .unwrap()
}

/// Lines of a rollout newest first, the unterminated last one included, as plyd's tail-first reader hands them over.
fn newest_first(path: &str) -> Vec<Vec<u8>> {
    let mut lines: Vec<Vec<u8>> = fixture(path)
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    lines.reverse();
    lines
}

fn scan(path: &str) -> FileScan {
    let mut scan = FileScan::default();
    for line in newest_first(path) {
        if scan.push(&line) {
            break;
        }
    }
    scan
}

fn window(
    label: &str,
    minutes: u32,
    used: f64,
    resets_at: Option<u64>,
    models: &[&str],
) -> UsageWindow {
    UsageWindow {
        label: label.to_owned(),
        window_minutes: Some(minutes),
        used_percent: used,
        resets_at,
        models: models.iter().map(|m| (*m).to_owned()).collect(),
    }
}

#[test]
fn claude_usage_takes_the_four_windows_and_nothing_else() {
    let usage = parse_usage_cache(&fixture("claude/dot-claude.json"))
        .unwrap()
        .unwrap();
    assert_eq!(
        usage,
        CliUsage {
            as_of: 1_790_350_000,
            plan: None,
            windows: vec![
                window("Session · 5h", 300, 81.0, Some(1_790_362_200), &[]),
                window("Week · all models", 10_080, 30.0, Some(1_790_409_600), &[]),
                window("Week · Sonnet", 10_080, 104.0, Some(1_790_409_600), &[]),
            ],
        }
    );
    let wire = serde_json::to_string(&usage).unwrap();
    assert!(
        !wire.contains("00000000-0000-4000-8000-00000000c1a0"),
        "the account id never leaves plyd"
    );
}

#[test]
fn a_claude_json_without_the_cache_or_in_another_shape_has_no_usage() {
    for text in [
        r#"{"numStartups":1}"#,
        r#"{"cachedUsageUtilization":{"fetchedAtMs":1790350000123,"utilisation":{"five_hour":{"utilization":81}}}}"#,
        r#"{"cachedUsageUtilization":{"fetchedAtMs":1790350000123,"utilization":{"five_hour":{"percent":81}}}}"#,
        r#"{"cachedUsageUtilization":[1,2,3]}"#,
    ] {
        assert_eq!(parse_usage_cache(text.as_bytes()).unwrap(), None, "{text}");
    }
    assert!(
        parse_usage_cache(b"{\"cachedUsageUtilization\":").is_err(),
        "a cut-off file"
    );
}

#[test]
fn every_token_count_of_the_s3b_captures_reads_as_the_weekly_codex_limit() {
    for name in [
        "codex/rollout-basic-session-meta-turn-context.jsonl",
        "codex/rollout-approval-and-resume-source.jsonl",
        "codex/rollout-update-plan-code-mode.jsonl",
    ] {
        let records: Vec<RateLimits> = newest_first(name)
            .iter()
            .filter_map(|l| parse_rate_limits(l))
            .collect();
        assert!(!records.is_empty(), "{name}");
        for r in records {
            assert_eq!(r.limit_id, "codex", "{name}");
            assert_eq!(r.plan.as_deref(), Some("self_serve_business_prolite"));
            assert_eq!(r.windows.len(), 1, "secondary is null: {name}");
            assert_eq!(r.windows[0].window_minutes, Some(10_080));
            assert_eq!(r.windows[0].used_percent, 0.0);
        }
    }
}

#[test]
fn only_event_token_counts_with_a_window_and_a_timestamp_are_rate_limits() {
    let lines = newest_first("codex/rollout-usage-two-limits.jsonl");
    let found: Vec<(u64, String)> = lines
        .iter()
        .filter_map(|l| parse_rate_limits(l))
        .map(|r| (r.at, r.limit_id))
        .collect();
    assert_eq!(
        found,
        [
            (1_790_331_000, "codex".to_owned()),
            (1_790_330_700, "codex_mini".to_owned()),
            (1_790_330_400, "codex".to_owned()),
        ],
        "the response_item decoy, the cut-off line and the null rate_limits are skipped"
    );
    let no_time = br#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":1.0}}}}"#;
    assert_eq!(parse_rate_limits(no_time), None);
    let no_window = br#"{"timestamp":"2026-09-25T10:00:00Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"window_minutes":300},"secondary":null}}}"#;
    assert_eq!(parse_rate_limits(no_window), None);
    let relative = br#"{"timestamp":"2026-09-25T10:00:00Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":2.5,"resets_in_seconds":60}}}}"#;
    assert_eq!(
        parse_rate_limits(relative).unwrap().windows,
        [LimitWindow {
            used_percent: 2.5,
            window_minutes: None,
            resets_at: Some(1_790_330_460),
        }]
    );
}

#[test]
fn turn_models_come_only_from_turn_context_records() {
    let models: Vec<String> = newest_first("codex/rollout-usage-two-limits.jsonl")
        .iter()
        .filter_map(|l| turn_model(l))
        .collect();
    assert_eq!(models, ["gpt-6-sol", "gpt-6-mini", "gpt-6-sol"]);
}

#[test]
fn a_file_scan_stops_at_the_turn_behind_its_newest_record() {
    let lines = newest_first("codex/rollout-usage-two-limits.jsonl");
    let mut scan = FileScan::default();
    let used = lines.iter().position(|l| scan.push(l)).unwrap();
    assert_eq!(
        used, 3,
        "task_complete, the record, the decoy, then its turn_context"
    );
    assert_eq!(scan.latest().map(|r| r.at), Some(1_790_331_000));
    assert_eq!(scan.model(), Some("gpt-6-sol"));
    let mini = self::scan("codex/rollout-usage-mini.jsonl");
    assert_eq!(
        mini.latest().map(|r| r.limit_id.as_str()),
        Some("codex_mini")
    );
    assert_eq!(
        mini.model(),
        Some("gpt-6-mini"),
        "the unterminated last line is ignored"
    );
}

#[test]
fn codex_usage_is_the_newest_record_per_limit_with_every_model_seen_with_it() {
    let mut usage = RolloutUsage::default();
    usage.add(scan("codex/rollout-usage-mini.jsonl"));
    usage.add(scan("codex/rollout-basic-session-meta-turn-context.jsonl"));
    usage.add(scan("codex/rollout-usage-two-limits.jsonl"));
    usage.add(FileScan::default());
    let usage = usage.finish().unwrap();
    assert_eq!(usage.as_of, 1_790_331_000);
    assert_eq!(usage.plan.as_deref(), Some("plus"));
    assert_eq!(
        usage.windows,
        [
            window(
                "Session · 5h",
                300,
                12.0,
                Some(1_790_341_200),
                &["gpt-6-sol"]
            ),
            window(
                "Session · 5h · GPT-6 mini",
                300,
                5.5,
                Some(1_790_334_300),
                &["gpt-6-mini"]
            ),
            window("Week", 10_080, 41.0, Some(1_790_926_095), &["gpt-6-sol"]),
        ]
    );
    assert_eq!(RolloutUsage::default().finish(), None);
}
