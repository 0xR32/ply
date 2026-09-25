//! Where plyd keeps its files (spec 11.1): the data directory, the run directory with the sockets, and the logs.
//!
//! By default the data directory is `~/Library/Application Support/ply` and the logs go to `~/Library/Logs/ply`.
//! `PLY_HOME` moves everything (the logs to `$PLY_HOME/logs`, matching the app), which is how tests and development
//! daemons stay off the installed one; `--run-dir` moves only the run directory. The run directory is mode 0700 and
//! every socket path in it stays under the 104-byte macOS limit.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use ply_proto::pane::PaneId;

use crate::error::{Error, Result, io};

/// Env var that moves every ply path under one directory (tests, development).
pub const ENV_PLY_HOME: &str = "PLY_HOME";

/// Longest socket path macOS accepts: `sun_path` holds 104 bytes including the terminating NUL.
pub const MAX_SOCKET_PATH: usize = 103;

/// The resolved paths of one plyd; plain data, cheap to clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Holds `ply.db`, `config.toml` and the instance lock.
    pub data_dir: PathBuf,
    /// Holds the sockets and `panes/<id>/`; mode 0700.
    pub run_dir: PathBuf,
    /// Holds `plyd.YYYY-MM-DD.log`.
    pub log_dir: PathBuf,
    /// True when `PLY_HOME` chose the data directory: plyd then never installs a LaunchAgent or holds a power assertion.
    pub sandboxed: bool,
}

impl Paths {
    /// Resolves from the process environment (`PLY_HOME`, else `HOME`); `run_dir` overrides the run directory.
    /// Fails with [`Error::NoHome`] when neither variable names an absolute path.
    pub fn from_env(run_dir: Option<PathBuf>) -> Result<Self> {
        Self::resolve(
            std::env::var_os(ENV_PLY_HOME).as_deref(),
            std::env::var_os("HOME").as_deref(),
            run_dir,
        )
    }

    /// Resolves from explicit values, the pure core of [`Paths::from_env`]; an empty `ply_home` counts as unset.
    /// Fails with [`Error::NoHome`] when the chosen base is missing or relative.
    pub fn resolve(
        ply_home: Option<&OsStr>,
        home: Option<&OsStr>,
        run_dir: Option<PathBuf>,
    ) -> Result<Self> {
        let (data_dir, log_dir, sandboxed) = match ply_home.filter(|v| !v.is_empty()) {
            Some(base) => {
                let base = PathBuf::from(base);
                if !base.is_absolute() {
                    return Err(Error::NoHome(format!(
                        "{ENV_PLY_HOME} must be an absolute path, got {}",
                        base.display()
                    )));
                }
                (base.clone(), base.join("logs"), true)
            }
            None => {
                let home = home.filter(|v| !v.is_empty()).map(PathBuf::from);
                let Some(home) = home.filter(|h| h.is_absolute()) else {
                    return Err(Error::NoHome(
                        "HOME is not set to an absolute path, and neither is PLY_HOME".to_owned(),
                    ));
                };
                let library = home.join("Library");
                (
                    library.join("Application Support").join("ply"),
                    library.join("Logs").join("ply"),
                    false,
                )
            }
        };
        let run_dir = run_dir.unwrap_or_else(|| data_dir.join("run"));
        Ok(Self {
            data_dir,
            run_dir,
            log_dir,
            sandboxed,
        })
    }

    /// The SQLite database (schema v1, spec 11.2).
    pub fn database(&self) -> PathBuf {
        self.data_dir.join("ply.db")
    }

    /// The settings file, which also stores the last `theme.set` palette.
    pub fn config(&self) -> PathBuf {
        self.data_dir.join("config.toml")
    }

    /// The single-instance lock; it sits beside the database, the resource two plyds must never share.
    pub fn lock(&self) -> PathBuf {
        self.data_dir.join("plyd.lock")
    }

    /// C1, JSON lines (spec 4.1).
    pub fn control_socket(&self) -> PathBuf {
        self.run_dir.join("plyd.sock")
    }

    /// C2, binary frames (spec 4.2).
    pub fn data_socket(&self) -> PathBuf {
        self.run_dir.join("data.sock")
    }

    /// C3, hook ingress (spec 4.3); agent panes get it as `PLY_HOOK_SOCK`, and `ply-hook` writes to it.
    pub fn hook_socket(&self) -> PathBuf {
        self.run_dir.join("hook.sock")
    }

    /// `run/panes/<id>/`: the pane's generated Claude settings and its `launch.json` (Ruling R6).
    pub fn pane_dir(&self, pane_id: PaneId) -> PathBuf {
        self.run_dir.join("panes").join(pane_id.to_string())
    }

    /// Creates the data, run (forced to mode 0700), `run/panes` and log directories and checks every socket path's length.
    /// Fails with [`Error::Io`] or [`Error::SocketPathTooLong`]; blocks on the filesystem.
    pub fn prepare(&self) -> Result<()> {
        for sock in [
            self.control_socket(),
            self.data_socket(),
            self.hook_socket(),
        ] {
            check_socket_path(&sock)?;
        }
        for dir in [&self.data_dir, &self.log_dir] {
            fs::create_dir_all(dir).map_err(io("cannot create", dir))?;
        }
        create_private_dir(&self.run_dir)?;
        create_private_dir(&self.run_dir.join("panes"))
    }
}

/// Fails with [`Error::SocketPathTooLong`] when `path` cannot be bound as a Unix socket on macOS.
pub fn check_socket_path(path: &Path) -> Result<()> {
    let len = path.as_os_str().len();
    if len > MAX_SOCKET_PATH {
        return Err(Error::SocketPathTooLong {
            path: path.to_path_buf(),
            len,
        });
    }
    Ok(())
}

/// Creates `dir` (and its parents) and sets it to mode 0700 even when it already existed; fails with [`Error::Io`].
pub fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(io("cannot create", dir))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(io("cannot restrict", dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ply_home_moves_every_path_and_marks_the_sandbox() {
        let p = Paths::resolve(
            Some(OsStr::new("/tmp/example")),
            Some(OsStr::new("/Users/example")),
            None,
        )
        .unwrap();
        assert!(p.sandboxed);
        assert_eq!(p.database(), Path::new("/tmp/example/ply.db"));
        assert_eq!(p.config(), Path::new("/tmp/example/config.toml"));
        assert_eq!(p.control_socket(), Path::new("/tmp/example/run/plyd.sock"));
        assert_eq!(p.data_socket(), Path::new("/tmp/example/run/data.sock"));
        assert_eq!(p.log_dir, Path::new("/tmp/example/logs"));
        assert_eq!(p.pane_dir(7), Path::new("/tmp/example/run/panes/7"));
    }

    #[test]
    fn without_ply_home_the_library_paths_of_spec_11_1_apply() {
        let p = Paths::resolve(None, Some(OsStr::new("/Users/example")), None).unwrap();
        assert!(!p.sandboxed);
        assert_eq!(
            p.data_dir,
            Path::new("/Users/example/Library/Application Support/ply")
        );
        assert_eq!(p.log_dir, Path::new("/Users/example/Library/Logs/ply"));
        let q = Paths::resolve(
            Some(OsStr::new("")),
            Some(OsStr::new("/Users/example")),
            Some("/tmp/r".into()),
        )
        .unwrap();
        assert!(!q.sandboxed);
        assert_eq!(q.control_socket(), Path::new("/tmp/r/plyd.sock"));
    }

    #[test]
    fn relative_or_missing_bases_are_refused() {
        assert!(matches!(
            Paths::resolve(Some(OsStr::new("rel")), None, None),
            Err(Error::NoHome(_))
        ));
        assert!(matches!(
            Paths::resolve(None, None, None),
            Err(Error::NoHome(_))
        ));
    }

    #[test]
    fn socket_paths_over_103_bytes_are_refused() {
        let long = format!("/tmp/{}/plyd.sock", "x".repeat(90));
        assert!(matches!(
            check_socket_path(Path::new(&long)),
            Err(Error::SocketPathTooLong { .. })
        ));
        assert!(check_socket_path(Path::new("/tmp/example/run/plyd.sock")).is_ok());
    }
}
