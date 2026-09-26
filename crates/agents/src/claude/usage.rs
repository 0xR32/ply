//! Claude Code's plan usage, from two places (Ruling R59).
//!
//! - **Live:** the `rate_limits` of the status line payload Claude Code sends its status line command, which
//!   `ply-hook statusline` forwards ([`status_line_windows`]). They follow every API response of the session.
//! - **Cached:** the `cachedUsageUtilization` object it keeps in its `.claude.json` ([`parse_usage_cache`]), refreshed
//!   when Claude Code asks for it, so it can be hours old; its age is the cache's own `fetchedAtMs`.
//!
//! Neither is a published format, so both are read tolerantly: the windows in [`WINDOWS`] are taken when they hold a
//! number, every other key (the account id among them) is never read, and a window that is missing, `null` or renamed
//! is simply absent.

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

/// The windows of a status line payload's `rate_limits` (`used_percentage`, `resets_at` in Unix seconds or RFC 3339), in [`WINDOWS`] order; empty without any, as for an API-key login or before the session's first response.
pub fn status_line_windows(payload: &Value) -> Vec<UsageWindow> {
    let Some(limits) = payload.get("rate_limits") else {
        return Vec::new();
    };
    WINDOWS
        .iter()
        .filter_map(|&(key, label, minutes)| {
            let window = limits.get(key)?;
            let used = window.get("used_percentage").and_then(Value::as_f64)?;
            used.is_finite().then(|| UsageWindow {
                label: label.to_owned(),
                window_minutes: Some(minutes),
                used_percent: used.max(0.0),
                resets_at: window.get("resets_at").and_then(reset_time),
                models: Vec::new(),
            })
        })
        .collect()
}

/// Unix seconds from a number (milliseconds when it is too large to be seconds) or an RFC 3339 string.
fn reset_time(value: &Value) -> Option<UnixSeconds> {
    match value {
        Value::Number(n) => {
            let n = n.as_f64().filter(|n| n.is_finite() && *n >= 0.0)?;
            let seconds = if n > 1e11 { n / 1000.0 } else { n };
            Some(seconds as UnixSeconds)
        }
        Value::String(s) => parse_timestamp(s),
        _ => None,
    }
}

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
    fn a_status_line_payload_gives_its_windows_in_order_and_nothing_without_rate_limits() {
        let payload: Value = serde_json::from_str(
            r#"{"model":{"display_name":"Opus"},"rate_limits":{
                "seven_day":{"used_percentage":83,"resets_at":1790500000},
                "five_hour":{"used_percentage":41.5,"resets_at":"2026-09-26T08:00:00+00:00"},
                "seven_day_opus":{"used_percentage":"12"},
                "tangelo":{"used_percentage":3}}}"#,
        )
        .unwrap();
        let windows = status_line_windows(&payload);
        let rows: Vec<(&str, f64, Option<u64>)> = windows
            .iter()
            .map(|w| (w.label.as_str(), w.used_percent, w.resets_at))
            .collect();
        assert_eq!(
            rows,
            [
                ("Session · 5h", 41.5, Some(1_790_409_600)),
                ("Week · all models", 83.0, Some(1_790_500_000)),
            ]
        );
        let ms: Value = serde_json::from_str(
            r#"{"rate_limits":{"five_hour":{"used_percentage":-4,"resets_at":1790500000123}}}"#,
        )
        .unwrap();
        let w = &status_line_windows(&ms)[0];
        assert_eq!((w.used_percent, w.resets_at), (0.0, Some(1_790_500_000)));
        for empty in [
            r#"{}"#,
            r#"{"rate_limits":null}"#,
            r#"{"rate_limits":{"five_hour":{}}}"#,
        ] {
            assert!(
                status_line_windows(&serde_json::from_str(empty).unwrap()).is_empty(),
                "{empty}"
            );
        }
    }

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
