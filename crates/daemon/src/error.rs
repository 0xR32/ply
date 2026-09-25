use std::path::PathBuf;

/// Every failure plyd's library code reports; `main.rs` turns a startup error into a message and an exit code.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem or socket operation failed; `what` names the operation, `path` the file when there is one.
    #[error("{what} {}: {source}", path.display())]
    Io {
        /// What plyd was doing, e.g. `"cannot create"`.
        what: &'static str,
        /// The file or socket involved.
        path: PathBuf,
        /// The OS error.
        #[source]
        source: std::io::Error,
    },
    /// Neither `PLY_HOME` nor `HOME` names an absolute directory, so plyd has nowhere to keep its files.
    #[error("{0}")]
    NoHome(String),
    /// A socket path reaches the macOS limit of 104 bytes (NUL included), so it cannot be bound.
    #[error("socket path {} is {len} bytes; macOS allows at most 103", path.display())]
    SocketPathTooLong {
        /// The offending path.
        path: PathBuf,
        /// Its length in bytes.
        len: usize,
    },
    /// Another plyd holds the single-instance lock of this data directory.
    #[error("plyd is already running for {} (pid {})", data_dir.display(), pid.map_or_else(|| "unknown".to_owned(), |p| p.to_string()))]
    AlreadyRunning {
        /// The data directory both instances would share.
        data_dir: PathBuf,
        /// The running instance's pid as it recorded it, when readable.
        pid: Option<u32>,
    },
    /// SQLite refused an operation.
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    /// The database was written by a newer plyd; this build refuses to open it (spec 11.2, 15).
    #[error(
        "the database has schema version {found}, newer than the {supported} this plyd knows; install a newer ply"
    )]
    SchemaTooNew {
        /// `schema_version` found in the file.
        found: i64,
        /// The newest version this build migrates to.
        supported: i64,
    },
    /// A value read back from the database is not one plyd writes (a corrupted or foreign file).
    #[error("database holds an invalid {what}: {value:?}")]
    BadRow {
        /// The column or kind of value.
        what: &'static str,
        /// The value found.
        value: String,
    },
    /// `config.toml` could not be serialised.
    #[error("cannot write the settings: {0}")]
    ConfigWrite(#[from] toml::ser::Error),
    /// A JSON document plyd writes (launch spec) could not be serialised.
    #[error("cannot serialise {what}: {source}")]
    Json {
        /// What was being written.
        what: &'static str,
        /// The serialiser's error.
        #[source]
        source: serde_json::Error,
    },
    /// A pty system call failed.
    #[error("pty {what} failed: {source}")]
    Pty {
        /// The call, e.g. `"openpt"`.
        what: &'static str,
        /// The OS error.
        #[source]
        source: std::io::Error,
    },
    /// The child process could not be started.
    #[error("cannot start {program}: {source}")]
    Spawn {
        /// argv\[0\].
        program: String,
        /// The OS error from fork/exec.
        #[source]
        source: std::io::Error,
    },
    /// A libghostty-vt engine call failed.
    #[error(transparent)]
    Term(#[from] ply_term::Error),
    /// A C1/C2 message could not be encoded or decoded.
    #[error(transparent)]
    Proto(#[from] ply_proto::Error),
    /// An agent adapter refused a launch.
    #[error(transparent)]
    Agents(#[from] ply_agents::Error),
    /// `launchctl` failed while installing the LaunchAgent.
    #[error("launchctl {args} failed with status {code}: {stderr}")]
    Launchctl {
        /// The arguments, space-joined.
        args: String,
        /// Its exit status (-1 when it died from a signal).
        code: i32,
        /// What it printed on stderr.
        stderr: String,
    },
    /// `plyd install-agent` refuses to run in this environment.
    #[error("{0}")]
    InstallRefused(String),
}

/// `Result` with [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn io(
    what: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.into();
    move |source| Error::Io { what, path, source }
}
