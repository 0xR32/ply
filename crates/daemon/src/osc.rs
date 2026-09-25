//! What plyd reads out of a pane's pty stream besides the screen: OSC 7 and OSC 9 (C8), and typed input (spec 6.3).
//!
//! plyd never scans the byte stream itself: libghostty-vt parses every sequence and reports OSC 7 through its
//! `OPT_PWD_CHANGED` callback and OSC 9/777 through `OPT_DESKTOP_NOTIFICATION` (ADR-0005), which `ply-term` collects
//! into [`ply_term::EngineOutput`]. This module turns those reports into what the registry and the agent session take:
//! a local path for OSC 7 and the bodies of OSC 9 notifications (OSC 777 carries a title and is some other program's;
//! bodies libghostty-vt parsed as ConEmu sub-commands never arrive, R27). It also decides which input counts as a key
//! typed and which as Enter, the Codex "Enter typed" and the R17 "any key typed" signals.

use ply_term::Notification;

/// The local path of an OSC 7 `file://host/path` URI, percent-decoded; `None` for other schemes or a relative path.
pub fn decode_osc7(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = path.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The bodies of the OSC 9 notifications among `notifications` (those without a title), in arrival order.
pub fn osc9_bodies(notifications: &[Notification]) -> impl Iterator<Item = &str> {
    notifications
        .iter()
        .filter(|n| n.title.is_empty())
        .map(|n| n.body.as_str())
}

/// Whether bytes written to the pty for one input submit a line: a carriage return, or Enter in the kitty protocol.
pub fn is_enter(bytes: &[u8]) -> bool {
    bytes.contains(&b'\r') || bytes == b"\x1b[13u"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc7_uris_decode_to_local_paths() {
        assert_eq!(
            decode_osc7("file://example-host/Users/example/My%20Code"),
            Some("/Users/example/My Code".to_owned())
        );
        assert_eq!(decode_osc7("file:///tmp"), Some("/tmp".to_owned()));
        assert_eq!(decode_osc7("kitty-shell-cwd://host/tmp"), None);
        assert_eq!(decode_osc7("file://host"), None);
    }

    #[test]
    fn only_untitled_notifications_are_osc_9() {
        let n = |title: &str, body: &str| Notification {
            title: title.to_owned(),
            body: body.to_owned(),
        };
        let all = [
            n("", "Approval requested: ls"),
            n("build", "done"),
            n("", "Question: which one?"),
        ];
        let bodies: Vec<&str> = osc9_bodies(&all).collect();
        assert_eq!(bodies, ["Approval requested: ls", "Question: which one?"]);
    }

    #[test]
    fn enter_is_a_carriage_return_in_any_encoding() {
        assert!(is_enter(b"\r"));
        assert!(is_enter(b"hello\r"));
        assert!(is_enter(b"\x1b[13u"));
        assert!(!is_enter(b"\n"), "shift-enter inserts a newline");
        assert!(
            !is_enter(b"\x1b[13;2u"),
            "shift-enter in the kitty protocol"
        );
        assert!(!is_enter(b"a"));
    }
}
