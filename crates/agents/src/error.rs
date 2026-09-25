use std::path::PathBuf;

use crate::version::CliVersion;

/// Every failure of this crate; plyd logs it with the pane id, and only `CliTooOld`/`InvalidLaunch` stop a spawn (C7).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A CLI payload (hook stdin, notify argument, rollout record) is not the JSON its reader expects.
    #[error("invalid {what}: {source}")]
    Json {
        /// What was being read, e.g. `"Claude hook payload"`.
        what: &'static str,
        /// The parser's error.
        #[source]
        source: serde_json::Error,
    },
    /// A payload parsed as JSON but lacks a field or holds a value its reader cannot use.
    #[error("invalid {what}: {detail}")]
    InvalidPayload {
        /// What was being read.
        what: &'static str,
        /// Which field or value was wrong.
        detail: String,
    },
    /// A C3 envelope was routed to the session of another CLI.
    #[error("a {found} envelope reached a {expected} session")]
    WrongCli {
        /// The CLI the session tracks.
        expected: &'static str,
        /// The CLI named in the envelope.
        found: &'static str,
    },
    /// A rollout line exceeded [`crate::codex::rollout::MAX_LINE_BYTES`] and was skipped without being buffered.
    #[error("rollout line of at least {len} bytes exceeds the {max}-byte cap")]
    RolloutLineTooLong {
        /// Bytes seen before the line was dropped.
        len: usize,
        /// The cap.
        max: usize,
    },
    /// The request cannot become a safe argv: an empty value, a NUL byte, a value that reads as a flag, or an unsupported option.
    #[error("invalid launch request: {0}")]
    InvalidLaunch(String),
    /// A path that must travel as UTF-8 text (argv, env, JSON) is not valid UTF-8.
    #[error("path is not valid UTF-8: {}", .0.display())]
    NonUtf8Path(PathBuf),
    /// A version string (install metadata or `--version` output) is not `major.minor.patch[-pre][+build]`.
    #[error("not a CLI version: {0:?}")]
    BadVersion(String),
    /// None of the known install layouts around the executable records its version.
    #[error("no version metadata found for {}", .exe.display())]
    VersionUnknown {
        /// The executable, symlinks resolved when that succeeded.
        exe: PathBuf,
    },
    /// The installed CLI is older than the supported minimum; the pane shows this and nothing is spawned (C7).
    #[error("{cli} {found} is older than the supported minimum {min}")]
    CliTooOld {
        /// `"claude"` or `"codex"`.
        cli: &'static str,
        /// The installed version.
        found: CliVersion,
        /// The oldest supported version.
        min: CliVersion,
    },
    /// Reading an install's metadata file failed for a reason other than the file being absent.
    #[error("cannot read {}: {source}", .path.display())]
    Io {
        /// The file that could not be read.
        path: PathBuf,
        /// The OS error.
        #[source]
        source: std::io::Error,
    },
}

/// Result alias used by every fallible function in this crate.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn json(what: &'static str) -> impl FnOnce(serde_json::Error) -> Error {
    move |source| Error::Json { what, source }
}

pub(crate) fn invalid(what: &'static str, detail: impl Into<String>) -> Error {
    Error::InvalidPayload {
        what,
        detail: detail.into(),
    }
}
