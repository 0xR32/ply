//! The three protocol versions and the one comparison every handshake uses.

use crate::error::{Error, Result};

/// C1 version: `hello.v` must equal it, else plyd answers `version_mismatch` on request id 0 and closes.
pub const PROTOCOL_VERSION: u16 = 1;

/// C2 version carried by ATTACH; plyd refuses any other value with an ATTACH_REFUSED frame. 2 added `scrollback_base` and CLIPBOARD_WRITE.
pub const C2_VERSION: u16 = 2;

/// C3 envelope version (`v`); plyd drops envelopes of any other version.
pub const HOOK_VERSION: u8 = 1;

/// Compares a peer's version with ours; `protocol` (`"C1"`, `"C2"`, `"C3"`) is named in the error.
/// Fails with [`Error::VersionMismatch`] when `found != expected`.
pub fn check_version(protocol: &'static str, expected: u16, found: u16) -> Result<()> {
    if expected == found {
        Ok(())
    } else {
        Err(Error::VersionMismatch {
            protocol,
            expected,
            found,
        })
    }
}
