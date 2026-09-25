//! Claude Code's plan usage: the `cachedUsageUtilization` object it keeps in its `.claude.json` (Ruling R59).
//!
//! The cache is Claude Code's internal state, not a published format, so it is read tolerantly: the four windows in
//! [`WINDOWS`] are taken when they hold a number, every other key (the account id among them) is never read, and a
//! window that is missing, `null` or renamed is simply absent. The age is the cache's own `fetchedAtMs`.

use ply_proto::pane::{CliUsage, UnixSeconds, UsageWindow};
use serde::Deserialize;
use serde_json::Value;

use crate::error::{Result, json};
use crate::usage::{SESSION_MINUTES, WEEK_MINUTES, parse_timestamp};

/// The `.claude.json` key Claude Code caches its plan usage under.
pub const USAGE_CACHE_KEY: &str = "cachedUsageUtilization";

/// The windows shown, in display order: the key under `utilization`, its label and its length in minutes.
pub const WINDOWS: [(&str, &str, u32); 4] = [
    ("five_hour", "Session · 5h", SESSION_MINUTES),
    ("seven_day", "Week · all models", WEEK_MINUTES),
    ("seven_day_opus", "Week · Opus", WEEK_MINUTES),
    ("seven_day_sonnet", "Week · Sonnet", WEEK_MINUTES),
];

#[derive(Deserialize)]
struct ClaudeJson {
    #[serde(rename = "cachedUsageUtilization", default)]
    cache: Option<Value>,
}

/// The usage in `.claude.json`'s bytes, keeping only the cache object in memory; `Ok(None)` without a cache, its `fetchedAtMs` or a readable window; errors: [`crate::Error::Json`] when they are not a JSON object.
pub fn parse_usage_cache(bytes: &[u8]) -> Result<Option<CliUsage>> {
    let file: ClaudeJson =
        serde_json::from_slice(bytes).map_err(json("Claude Code .claude.json"))?;
    Ok(file.cache.as_ref().and_then(usage_of))
}

fn usage_of(cache: &Value) -> Option<CliUsage> {
    let fetched_ms = cache.get("fetchedAtMs").and_then(Value::as_f64)?;
    if !fetched_ms.is_finite() || fetched_ms < 0.0 {
        return None;
    }
    let utilization = cache.get("utilization")?;
    let windows: Vec<UsageWindow> = WINDOWS
        .iter()
        .filter_map(|&(key, label, minutes)| {
            let window = utilization.get(key)?;
            let used = window.get("utilization").and_then(Value::as_f64)?;
            used.is_finite().then(|| UsageWindow {
                label: label.to_owned(),
                window_minutes: Some(minutes),
                used_percent: used.max(0.0),
                resets_at: window
                    .get("resets_at")
                    .and_then(Value::as_str)
                    .and_then(parse_timestamp),
                models: Vec::new(),
            })
        })
        .collect();
    (!windows.is_empty()).then(|| CliUsage {
        as_of: (fetched_ms / 1000.0) as UnixSeconds,
        plan: None,
        windows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_cache_timestamp_or_windows_mean_no_usage() {
        for text in [
            r#"{}"#,
            r#"{"cachedUsageUtilization":null}"#,
            r#"{"cachedUsageUtilization":{"utilization":{"five_hour":{"utilization":5}}}}"#,
            r#"{"cachedUsageUtilization":{"fetchedAtMs":1790000000000}}"#,
            r#"{"cachedUsageUtilization":{"fetchedAtMs":1790000000000,"utilization":{"five_hour":null,"tangelo":{"utilization":3}}}}"#,
            r#"{"cachedUsageUtilization":{"fetchedAtMs":"soon","utilization":{"five_hour":{"utilization":5}}}}"#,
        ] {
            assert_eq!(parse_usage_cache(text.as_bytes()).unwrap(), None, "{text}");
        }
        assert!(parse_usage_cache(b"not json").is_err());
    }

    #[test]
    fn a_window_without_a_number_is_left_out_and_a_bad_reset_time_is_unknown() {
        let text = r#"{"cachedUsageUtilization":{"fetchedAtMs":1790350000999,"utilization":{
            "five_hour":{"utilization":"81"},
            "seven_day":{"utilization":30.5,"resets_at":"next week"},
            "seven_day_sonnet":{"utilization":-2,"resets_at":"2026-09-26T08:00:00+00:00"}}}}"#;
        let usage = parse_usage_cache(text.as_bytes()).unwrap().unwrap();
        assert_eq!(usage.as_of, 1_790_350_000);
        let rows: Vec<(&str, f64, Option<u64>)> = usage
            .windows
            .iter()
            .map(|w| (w.label.as_str(), w.used_percent, w.resets_at))
            .collect();
        assert_eq!(
            rows,
            [
                ("Week · all models", 30.5, None),
                ("Week · Sonnet", 0.0, Some(1_790_409_600))
            ]
        );
    }
}
