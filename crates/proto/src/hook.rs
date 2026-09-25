//! C3 hook ingress (spec 4.3): one JSON line from `ply-hook` to plyd on `run/hook.sock`.
//!
//! The envelope wraps the CLI's own payload untouched: Claude Code's hook stdin JSON, or the JSON argument Codex
//! passes to its notify program. `ply-hook` builds the line with serde_json alone (it never links this crate), so
//! the field names here are the contract both sides keep.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{Error, Result};
use crate::pane::{AgentCli, PaneId};

/// Longest accepted C3 line in bytes, newline included; plyd stops reading a connection at this size.
pub const MAX_LINE_BYTES: usize = 1 << 20;

/// Largest raw CLI payload `ply-hook` forwards, leaving room for the envelope inside [`MAX_LINE_BYTES`].
pub const MAX_PAYLOAD_BYTES: usize = MAX_LINE_BYTES - 4096;

/// `{"v":1,"pane_id":2,"cli":"claude","event":"Notification","payload":{…}}`; `event` is absent for Codex notify.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct HookEnvelope {
    /// Envelope version, [`crate::HOOK_VERSION`].
    pub v: u8,
    /// Pane that ran the hook, from `PLY_PANE_ID`.
    pub pane_id: PaneId,
    /// CLI that sent it (`ply-hook`'s first argument).
    pub cli: AgentCli,
    /// Hook event name (`ply-hook`'s second argument), e.g. `"PermissionRequest"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// The CLI's payload exactly as received; never interpreted by `ply-hook`.
    pub payload: serde_json::Value,
}

impl HookEnvelope {
    /// Parses one line and checks `v`; fails with [`Error::LineTooLong`], [`Error::Json`] or [`Error::VersionMismatch`].
    pub fn decode_line(line: &[u8]) -> Result<Self> {
        if line.len() > MAX_LINE_BYTES {
            return Err(Error::LineTooLong {
                len: line.len(),
                max: MAX_LINE_BYTES,
            });
        }
        let env: Self = serde_json::from_slice(line)?;
        crate::version::check_version("C3", u16::from(crate::HOOK_VERSION), u16::from(env.v))?;
        Ok(env)
    }

    /// Serialises the envelope as one line ending in `\n`; fails with [`Error::LineTooLong`] above the cap.
    pub fn encode_line(&self) -> Result<Vec<u8>> {
        let mut out = serde_json::to_vec(self)?;
        out.push(b'\n');
        if out.len() > MAX_LINE_BYTES {
            return Err(Error::LineTooLong {
                len: out.len(),
                max: MAX_LINE_BYTES,
            });
        }
        Ok(out)
    }
}
