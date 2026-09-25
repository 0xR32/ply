//! Codex's plan usage: the `rate_limits` its `token_count` events record in the rollouts (Ruling R59).
//!
//! Every `event_msg` of type `token_count` carries the account's limits as the server last reported them,
//! `{limit_id, limit_name, primary, secondary, plan_type, …}`, each window `{used_percent, window_minutes, resets_at}`.
//! plyd reads the newest rollouts from their end and feeds each file's lines, newest first, to a [`FileScan`], which
//! keeps the file's newest record and the model of the turn that recorded it (`turn_context.model`). [`RolloutUsage`]
//! keeps the newest record per `limit_id` across files and every model seen with it. A field that is missing or of
//! another type leaves its window (or, without a window or timestamp, its record) out; nothing here is an error.

use std::collections::{BTreeMap, BTreeSet};

use ply_proto::pane::{CliUsage, UnixSeconds, UsageWindow};
use serde_json::Value;

use crate::adapter::str_field;
use crate::usage::{contains, parse_timestamp, window_label};

use super::rollout::{RolloutRecord, parse_record};

/// The `limit_id` of Codex's general limit; windows of any other limit carry its name in their label.
pub const DEFAULT_LIMIT_ID: &str = "codex";

/// One window of a rate-limit record.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitWindow {
    /// Percent used, as Codex reports it.
    pub used_percent: f64,
    /// Length of the window in minutes, when recorded.
    pub window_minutes: Option<u32>,
    /// When the window starts over (`resets_at`, else the record's time plus `resets_in_seconds`).
    pub resets_at: Option<UnixSeconds>,
}

/// The rate limits one `token_count` event recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimits {
    /// The record's `timestamp`.
    pub at: UnixSeconds,
    /// Which limit this is ([`DEFAULT_LIMIT_ID`] when the record names none).
    pub limit_id: String,
    /// Its display name, when Codex gives one.
    pub limit_name: Option<String>,
    /// `plan_type`, as Codex spells it.
    pub plan: Option<String>,
    /// `primary`, then `secondary`, whichever are present; never empty.
    pub windows: Vec<LimitWindow>,
}

/// The rate limits of a `token_count` rollout line (no newline); `None` for every other line and for one without a timestamp or a readable window.
pub fn parse_rate_limits(line: &[u8]) -> Option<RateLimits> {
    if !contains(line, b"\"token_count\"") || !contains(line, b"\"rate_limits\"") {
        return None;
    }
    let record: Value = serde_json::from_slice(line).ok()?;
    let payload = record.get("payload")?;
    if str_field(&record, "type") != Some("event_msg")
        || str_field(payload, "type") != Some("token_count")
    {
        return None;
    }
    let at = parse_timestamp(str_field(&record, "timestamp")?)?;
    let limits = payload.get("rate_limits")?;
    let windows: Vec<LimitWindow> = ["primary", "secondary"]
        .iter()
        .filter_map(|key| limit_window(limits.get(key)?, at))
        .collect();
    (!windows.is_empty()).then(|| RateLimits {
        at,
        limit_id: str_field(limits, "limit_id")
            .unwrap_or(DEFAULT_LIMIT_ID)
            .to_owned(),
        limit_name: str_field(limits, "limit_name").map(str::to_owned),
        plan: str_field(limits, "plan_type").map(str::to_owned),
        windows,
    })
}

fn limit_window(window: &Value, at: UnixSeconds) -> Option<LimitWindow> {
    let used = window.get("used_percent").and_then(Value::as_f64)?;
    let resets_in = || {
        window
            .get("resets_in_seconds")
            .and_then(Value::as_u64)
            .map(|s| at.saturating_add(s))
    };
    used.is_finite().then(|| LimitWindow {
        used_percent: used.max(0.0),
        window_minutes: window
            .get("window_minutes")
            .and_then(Value::as_u64)
            .and_then(|m| u32::try_from(m).ok()),
        resets_at: window
            .get("resets_at")
            .and_then(Value::as_u64)
            .or_else(resets_in),
    })
}

/// The model of a `turn_context` rollout line; `None` for every other line.
pub fn turn_model(line: &[u8]) -> Option<String> {
    if !contains(line, b"\"turn_context\"") {
        return None;
    }
    match parse_record(line) {
        Ok(RolloutRecord::TurnContext(turn)) => turn.model,
        _ => None,
    }
}

/// One rollout read backwards: its newest rate-limit record and the model of the turn that recorded it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileScan {
    latest: Option<RateLimits>,
    model: Option<String>,
}

impl FileScan {
    /// Takes the file's next line going backwards from its end; true once the scan has both, so the reader can stop.
    pub fn push(&mut self, line: &[u8]) -> bool {
        if self.latest.is_none() {
            self.latest = parse_rate_limits(line);
            return false;
        }
        self.model = turn_model(line);
        self.model.is_some()
    }

    /// The newest rate-limit record found so far.
    pub fn latest(&self) -> Option<&RateLimits> {
        self.latest.as_ref()
    }

    /// The model of the turn that recorded [`FileScan::latest`], once its `turn_context` was read.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
}

/// The newest record per `limit_id` over any number of [`FileScan`]s, with the models of the turns behind them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RolloutUsage {
    limits: BTreeMap<String, (RateLimits, BTreeSet<String>)>,
}

impl RolloutUsage {
    /// Adds one file's scan; a scan without a record changes nothing.
    pub fn add(&mut self, scan: FileScan) {
        let Some(latest) = scan.latest else {
            return;
        };
        let (kept, models) = self
            .limits
            .entry(latest.limit_id.clone())
            .or_insert_with(|| (latest.clone(), BTreeSet::new()));
        if latest.at > kept.at {
            *kept = latest;
        }
        models.extend(scan.model);
    }

    /// Codex's usage: `as_of` and `plan` from the newest record, the windows shortest first; `None` when no scan had a record.
    pub fn finish(self) -> Option<CliUsage> {
        let newest = self.limits.values().map(|(r, _)| r).max_by_key(|r| r.at)?;
        let (as_of, plan) = (newest.at, newest.plan.clone());
        let mut rows = Vec::new();
        for (id, (record, models)) in &self.limits {
            let general = id == DEFAULT_LIMIT_ID;
            for window in &record.windows {
                let length = window
                    .window_minutes
                    .map_or_else(|| "Limit".to_owned(), window_label);
                let label = if general {
                    length
                } else {
                    format!("{length} · {}", record.limit_name.as_deref().unwrap_or(id))
                };
                let order = (window.window_minutes.unwrap_or(u32::MAX), !general);
                rows.push((
                    order,
                    UsageWindow {
                        label,
                        window_minutes: window.window_minutes,
                        used_percent: window.used_percent,
                        resets_at: window.resets_at,
                        models: models.iter().cloned().collect(),
                    },
                ));
            }
        }
        rows.sort_by(|(a, x), (b, y)| a.cmp(b).then_with(|| x.label.cmp(&y.label)));
        Some(CliUsage {
            as_of,
            plan,
            windows: rows.into_iter().map(|(_, w)| w).collect(),
        })
    }
}
