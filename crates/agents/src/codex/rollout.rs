//! C4: Codex rollout files — record parsing, `update_plan` extraction (spec 6.4, R26), turn events (R48), the
//! tailer's pre-filter, line framing and discovery (R28).

use std::path::PathBuf;
use std::time::SystemTime;

use ply_proto::pane::Progress;
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::adapter::str_field;
use crate::error::{Error, Result, invalid, json};
use crate::plan::{ItemStatus, progress_of};

use super::literal::{parse_literal, skip_space};

/// Longest rollout line kept; longer lines (inline images) are skipped without being buffered.
pub const MAX_LINE_BYTES: usize = 8 << 20;

/// Every record `type` Codex 0.156.1 writes (`RolloutItemWire` in codex-rs/history); anything else is unknown.
pub const KNOWN_RECORD_TYPES: [&str; 12] = [
    "session_meta",
    "response_item",
    "inter_agent_communication",
    "inter_agent_communication_metadata",
    "compacted",
    "turn_context",
    "token_usage_record",
    "world_state",
    "retained_context",
    "security_risk_score",
    "event_msg",
    "realtime_item",
];

/// Name of the plan tool, as a `function_call` and inside code-mode `exec` input.
pub const UPDATE_PLAN: &str = "update_plan";

/// `event_msg` subtypes seen in Codex 0.156.1 rollouts; the turn ones drive the status (R48), any other is counted.
pub const KNOWN_EVENT_TYPES: [&str; 6] = [
    "task_started",
    "task_complete",
    "turn_aborted",
    "item_completed",
    "token_count",
    "thread_settings_applied",
];

/// A turn boundary from an `event_msg` record (Ruling R48); `turn_id` is Codex's, when the record carries one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEvent {
    /// `task_started`: the user's message started a turn.
    Started {
        /// The turn.
        turn_id: Option<String>,
    },
    /// `task_complete`: the turn finished.
    Complete {
        /// The turn.
        turn_id: Option<String>,
    },
    /// `turn_aborted`: the user interrupted the turn (Esc).
    Aborted {
        /// The turn.
        turn_id: Option<String>,
    },
}

/// The first record of every rollout: which thread the file belongs to and where it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetaRecord {
    /// The thread UUID (`payload.id`, equal to `payload.session_id`).
    pub thread_id: String,
    /// The session's working directory.
    pub cwd: String,
    /// The Codex version that wrote the file, when recorded.
    pub cli_version: Option<String>,
}

/// Per-turn settings; ply reads the model the turn runs on (spec 6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnContextRecord {
    /// Model name as Codex reports it, e.g. `"gpt-6-sol"`.
    pub model: Option<String>,
    /// Working directory of the turn.
    pub cwd: Option<String>,
}

/// One step of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    /// The step's text.
    pub step: String,
    /// `pending`, `in_progress` or `completed`.
    pub status: ItemStatus,
}

/// The arguments of one `update_plan` call: the whole plan, replacing any earlier one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Optional note the model attached.
    pub explanation: Option<String>,
    /// The steps in order.
    pub steps: Vec<PlanStep>,
}

impl Plan {
    /// Progress in the same terms as Claude's (spec 6.4); `None` for an empty plan.
    pub fn progress(&self) -> Option<Progress> {
        progress_of(self.steps.iter().map(|s| (s.status, s.step.as_str())))
    }
}

/// What one rollout line means to ply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RolloutRecord {
    /// `session_meta`.
    SessionMeta(SessionMetaRecord),
    /// `turn_context`.
    TurnContext(TurnContextRecord),
    /// A `response_item` that calls `update_plan`, in either shape.
    PlanUpdate(Plan),
    /// An `event_msg` that starts or ends a turn.
    Turn(TurnEvent),
    /// An `event_msg` subtype outside [`KNOWN_EVENT_TYPES`]; skipped and counted.
    UnknownEvent(String),
    /// A known record type (or a response item) ply has no use for.
    Ignored,
    /// A record type Codex 0.156.1 does not write; skipped and counted (C4).
    Unknown(String),
}

#[derive(Deserialize)]
struct Line<'a> {
    #[serde(rename = "type", borrow)]
    kind: &'a str,
    #[serde(borrow)]
    payload: &'a RawValue,
}

/// Parses one line (no newline); fails with [`Error::Json`] when it is not a `{type, payload}` record.
pub fn parse_record(line: &[u8]) -> Result<RolloutRecord> {
    let line: Line<'_> = serde_json::from_slice(line).map_err(json("rollout record"))?;
    let payload = || -> Result<Value> {
        serde_json::from_str(line.payload.get()).map_err(json("rollout payload"))
    };
    match line.kind {
        "session_meta" => {
            let p = payload()?;
            let thread_id = str_field(&p, "id")
                .or_else(|| str_field(&p, "session_id"))
                .ok_or_else(|| invalid("rollout session_meta", "no id"))?;
            let cwd =
                str_field(&p, "cwd").ok_or_else(|| invalid("rollout session_meta", "no cwd"))?;
            Ok(RolloutRecord::SessionMeta(SessionMetaRecord {
                thread_id: thread_id.to_owned(),
                cwd: cwd.to_owned(),
                cli_version: str_field(&p, "cli_version").map(str::to_owned),
            }))
        }
        "turn_context" => {
            let p = payload()?;
            Ok(RolloutRecord::TurnContext(TurnContextRecord {
                model: str_field(&p, "model").map(str::to_owned),
                cwd: str_field(&p, "cwd").map(str::to_owned),
            }))
        }
        "response_item" => match parse_update_plan(&payload()?)? {
            Some(plan) => Ok(RolloutRecord::PlanUpdate(plan)),
            None => Ok(RolloutRecord::Ignored),
        },
        "event_msg" => {
            let p = payload()?;
            let subtype =
                str_field(&p, "type").ok_or_else(|| invalid("rollout event_msg", "no type"))?;
            let turn_id = str_field(&p, "turn_id").map(str::to_owned);
            Ok(match subtype {
                "task_started" => RolloutRecord::Turn(TurnEvent::Started { turn_id }),
                "task_complete" => RolloutRecord::Turn(TurnEvent::Complete { turn_id }),
                "turn_aborted" => RolloutRecord::Turn(TurnEvent::Aborted { turn_id }),
                known if KNOWN_EVENT_TYPES.contains(&known) => RolloutRecord::Ignored,
                unknown => RolloutRecord::UnknownEvent(unknown.to_owned()),
            })
        }
        known if KNOWN_RECORD_TYPES.contains(&known) => Ok(RolloutRecord::Ignored),
        unknown => Ok(RolloutRecord::Unknown(unknown.to_owned())),
    }
}

#[derive(Deserialize)]
struct Subtype<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<&'a str>,
}

/// Whether [`parse_record`] can make anything of `line`, so the tailer drops the rest (known ignored records and `event_msg` subtypes, response items without `update_plan`) and passes unknown and malformed lines for the session to count.
pub fn plyd_reads(line: &[u8]) -> bool {
    let Ok(record) = serde_json::from_slice::<Line<'_>>(line) else {
        return true;
    };
    match record.kind {
        "session_meta" | "turn_context" => true,
        "response_item" => record.payload.get().contains(UPDATE_PLAN),
        "event_msg" => match serde_json::from_str::<Subtype<'_>>(record.payload.get()) {
            Ok(Subtype { kind: Some(kind) }) => {
                matches!(kind, "task_started" | "task_complete" | "turn_aborted")
                    || !KNOWN_EVENT_TYPES.contains(&kind)
            }
            _ => true,
        },
        known => !KNOWN_RECORD_TYPES.contains(&known),
    }
}

/// The plan of a `function_call` `update_plan` or a code-mode `exec` calling `tools.update_plan({…})` (last literal wins); `Ok(None)` otherwise.
pub fn parse_update_plan(item: &Value) -> Result<Option<Plan>> {
    match (str_field(item, "type"), str_field(item, "name")) {
        (Some("function_call"), Some(UPDATE_PLAN)) => {
            let args = str_field(item, "arguments")
                .ok_or_else(|| invalid("update_plan call", "no arguments"))?;
            let args: Value = serde_json::from_str(args).map_err(json("update_plan arguments"))?;
            plan_from_args(&args).map(Some)
        }
        (Some("custom_tool_call"), Some("exec")) => {
            let Some(input) = str_field(item, "input") else {
                return Ok(None);
            };
            match last_update_plan_literal(input) {
                Some(args) => plan_from_args(&args).map(Some),
                None if input.contains(UPDATE_PLAN) => Err(invalid(
                    "update_plan call",
                    "exec input passes no readable literal",
                )),
                None => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

fn last_update_plan_literal(input: &str) -> Option<Value> {
    let needle = "tools.update_plan";
    let mut found = None;
    for (at, _) in input.match_indices(needle) {
        let open = skip_space(input, at + needle.len());
        if !input[open..].starts_with('(') {
            continue;
        }
        if let Some((value, _)) = parse_literal(input, open + 1) {
            found = Some(value);
        }
    }
    found
}

#[derive(Deserialize)]
struct WireStep {
    step: String,
    status: String,
}

fn plan_from_args(args: &Value) -> Result<Plan> {
    let steps = args
        .get("plan")
        .ok_or_else(|| invalid("update_plan call", "no plan"))?;
    let steps = Vec::<WireStep>::deserialize(steps).map_err(json("update_plan plan"))?;
    Ok(Plan {
        explanation: str_field(args, "explanation").map(str::to_owned),
        steps: steps
            .into_iter()
            .map(|s| PlanStep {
                status: ItemStatus::parse(&s.status),
                step: s.step,
            })
            .collect(),
    })
}

/// Frames a tailed rollout into lines: bytes in any chunking, complete lines out; memory stays under [`MAX_LINE_BYTES`].
#[derive(Debug, Default)]
pub struct LineBuffer {
    partial: Vec<u8>,
    skipping: Option<usize>,
}

/// One framed line, or the size of a line that was dropped for exceeding [`MAX_LINE_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramedLine {
    /// A complete line without its newline (a trailing `\r` is kept).
    Line(Vec<u8>),
    /// A line of at least this many bytes was skipped; report it with [`Error::RolloutLineTooLong`].
    TooLong(usize),
}

impl LineBuffer {
    /// Appends `chunk` and returns the lines it completes, in order; an unterminated tail waits for the next chunk.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<FramedLine> {
        let mut out = Vec::new();
        let mut rest = chunk;
        while !rest.is_empty() {
            let newline = rest.iter().position(|&b| b == b'\n');
            let (piece, done) = match newline {
                Some(i) => (&rest[..i], true),
                None => (rest, false),
            };
            rest = newline.map_or(&[][..], |i| &rest[i + 1..]);
            if let Some(seen) = self.skipping.as_mut() {
                *seen += piece.len();
            } else if self.partial.len() + piece.len() > MAX_LINE_BYTES {
                self.skipping = Some(self.partial.len() + piece.len());
                self.partial = Vec::new();
            } else {
                self.partial.extend_from_slice(piece);
            }
            if done {
                match self.skipping.take() {
                    Some(len) => out.push(FramedLine::TooLong(len)),
                    None => out.push(FramedLine::Line(std::mem::take(&mut self.partial))),
                }
            }
        }
        out
    }

    /// Bytes held for an unterminated line (0 while skipping an oversized one).
    pub fn pending(&self) -> usize {
        self.partial.len()
    }
}

impl FramedLine {
    /// The line, or [`Error::RolloutLineTooLong`] for a dropped one.
    pub fn into_line(self) -> Result<Vec<u8>> {
        match self {
            Self::Line(line) => Ok(line),
            Self::TooLong(len) => Err(Error::RolloutLineTooLong {
                len,
                max: MAX_LINE_BYTES,
            }),
        }
    }
}

/// The thread UUID of a rollout file name `rollout-<YYYY-MM-DDThh-mm-ss>-<uuid>.jsonl`; `None` for other names.
pub fn rollout_thread_id(file_name: &str) -> Option<&str> {
    let stem = file_name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    let split = stem.len().checked_sub(36)?;
    let (ts, id) = stem.split_at_checked(split)?;
    (ts.ends_with('-') && is_uuid(id)).then_some(id)
}

/// Whether `s` is a hyphenated 8-4-4-4-12 hex UUID, the shape of Codex thread ids.
pub fn is_uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, len)| g.len() == len && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A rollout file plyd found under `$CODEX_HOME/sessions/`, with what its first line says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutCandidate {
    /// The file.
    pub path: PathBuf,
    /// When the file was created (its birth time).
    pub created: SystemTime,
    /// `session_meta.cwd` from its first record.
    pub cwd: String,
}

/// Pre-notify discovery (spec 6.2, R28): the newest candidate created at or after `spawned_at` whose cwd equals `cwd`.
pub fn pick_rollout_by_cwd<'a>(
    candidates: &'a [RolloutCandidate],
    spawned_at: SystemTime,
    cwd: &str,
) -> Option<&'a RolloutCandidate> {
    candidates
        .iter()
        .filter(|c| c.created >= spawned_at && c.cwd == cwd)
        .max_by_key(|c| c.created)
}
