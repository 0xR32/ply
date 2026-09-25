//! The JSON Codex appends to its `notify` program's argv after each completed turn (ADR-0004 item 1).

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Result, invalid, json};

/// The only `type` Codex 0.156.1 sends.
pub const AGENT_TURN_COMPLETE: &str = "agent-turn-complete";

/// A notify payload; kebab-case keys, unknown keys ignored so newer Codex versions still parse.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct NotifyPayload {
    /// Always [`AGENT_TURN_COMPLETE`] in 0.156.1.
    #[serde(rename = "type")]
    pub kind: String,
    /// Thread the turn ran in; the title-generation micro-turn has its own thread with no rollout (R28).
    pub thread_id: String,
    /// The completed turn.
    pub turn_id: String,
    /// The session's working directory.
    pub cwd: String,
    /// Front end, e.g. `"codex-tui"`; absent when Codex has none to report.
    #[serde(default)]
    pub client: Option<String>,
    /// The user messages of the turn.
    #[serde(default)]
    pub input_messages: Vec<String>,
    /// The assistant's final message; `null` when there was none.
    #[serde(default)]
    pub last_assistant_message: Option<String>,
}

impl NotifyPayload {
    /// Reads the payload from a C3 envelope's `payload`; fails with [`crate::Error::Json`] on a missing key.
    pub fn from_value(payload: &Value) -> Result<Self> {
        let parsed = Self::deserialize(payload).map_err(json("Codex notify payload"))?;
        if parsed.thread_id.is_empty() {
            return Err(invalid("Codex notify payload", "empty thread-id"));
        }
        Ok(parsed)
    }

    /// Whether this is a turn-complete notification (the only kind ply acts on).
    pub fn is_turn_complete(&self) -> bool {
        self.kind == AGENT_TURN_COMPLETE
    }
}
