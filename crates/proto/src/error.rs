use std::fmt;

/// Every way decoding or encoding a C1, C2 or C3 message can fail; no variant panics or allocates past a cap.
#[derive(Debug)]
pub enum Error {
    /// A C1 or C3 line is longer than its cap; `len` is the observed length in bytes, `max` the cap.
    LineTooLong {
        /// Bytes seen (for a streaming reader, at least `max + 1`).
        len: usize,
        /// The cap that was exceeded.
        max: usize,
    },
    /// A JSON line did not match its type: bad syntax, a missing field, an unknown field or a bad value.
    Json(serde_json::Error),
    /// A C2 frame header announced a payload above [`crate::data::MAX_FRAME_LEN`]; nothing was allocated.
    FrameTooLarge {
        /// The announced payload length.
        len: u64,
    },
    /// A C2 kind byte that no [`crate::data::Frame`] variant uses.
    UnknownFrameKind(u8),
    /// A C2 payload ended before its fixed layout was complete.
    Truncated {
        /// Kind byte of the frame being decoded.
        kind: u8,
    },
    /// A C2 payload had bytes left after its last field.
    TrailingBytes {
        /// Kind byte of the frame being decoded.
        kind: u8,
        /// Number of unread bytes.
        extra: usize,
    },
    /// A C2 field held a value outside its documented range (an enum tag, a flag bit, a count cap).
    InvalidValue {
        /// Kind byte of the frame being decoded.
        kind: u8,
        /// Name of the offending field.
        field: &'static str,
        /// The value found, widened to u64.
        value: u64,
    },
    /// A C2 text field was not valid UTF-8.
    InvalidUtf8 {
        /// Kind byte of the frame being decoded.
        kind: u8,
    },
    /// The peer speaks another version of a protocol (C1 `hello.v`, C2 ATTACH `v`, C3 envelope `v`).
    VersionMismatch {
        /// Which protocol: `"C1"`, `"C2"` or `"C3"`.
        protocol: &'static str,
        /// The version this build speaks.
        expected: u16,
        /// The version the peer sent.
        found: u16,
    },
    /// Reading from the underlying stream failed.
    Io(std::io::Error),
}

/// Result alias used by every fallible function in this crate.
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LineTooLong { len, max } => {
                write!(f, "line of {len} bytes exceeds the {max}-byte cap")
            }
            Self::Json(e) => write!(f, "invalid JSON message: {e}"),
            Self::FrameTooLarge { len } => {
                write!(f, "C2 frame of {len} bytes exceeds the 1 MiB cap")
            }
            Self::UnknownFrameKind(k) => write!(f, "unknown C2 frame kind 0x{k:02x}"),
            Self::Truncated { kind } => write!(f, "C2 frame 0x{kind:02x} is truncated"),
            Self::TrailingBytes { kind, extra } => {
                write!(f, "C2 frame 0x{kind:02x} has {extra} trailing bytes")
            }
            Self::InvalidValue { kind, field, value } => {
                write!(f, "C2 frame 0x{kind:02x}: invalid {field} {value}")
            }
            Self::InvalidUtf8 { kind } => write!(f, "C2 frame 0x{kind:02x}: text is not UTF-8"),
            Self::VersionMismatch {
                protocol,
                expected,
                found,
            } => write!(
                f,
                "{protocol} version mismatch: this build speaks {expected}, the peer sent {found}"
            ),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(e) => Some(e),
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
