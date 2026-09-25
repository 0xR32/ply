//! The single-instance lock: one plyd per data directory.
//!
//! plyd takes an exclusive `flock` on `<data dir>/plyd.lock` before it opens the database or binds a socket, and
//! writes its pid into the file. The kernel drops the lock when the process ends, however it ends, so a crash never
//! leaves a stale lock behind. A second plyd finds the lock taken and refuses with [`Error::AlreadyRunning`], naming
//! the running instance's pid.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;

use crate::error::{Error, Result, io};

/// Holds the lock for as long as it lives; dropping it (or the process ending) releases it.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Takes the lock at `path` without blocking and records this process's pid in it.
    /// Fails with [`Error::AlreadyRunning`] when another process (or another handle in this one) holds it, else [`Error::Io`].
    pub fn acquire(path: &Path) -> Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(io("cannot open", path))?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(Errno::WOULDBLOCK) => {
                let mut text = String::new();
                let pid = match file.read_to_string(&mut text) {
                    Ok(_) => text.trim().parse().ok(),
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "cannot read the running plyd's pid");
                        None
                    }
                };
                let data_dir = path.parent().unwrap_or(path).to_path_buf();
                return Err(Error::AlreadyRunning { data_dir, pid });
            }
            Err(e) => return Err(io("cannot lock", path)(e.into())),
        }
        file.set_len(0)
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(drop))
            .and_then(|()| writeln!(file, "{}", std::process::id()))
            .map_err(io("cannot write", path))?;
        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_is_refused_with_the_first_pid_until_it_drops() {
        let dir = std::env::temp_dir().join(format!("ply-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plyd.lock");
        let first = InstanceLock::acquire(&path).unwrap();
        match InstanceLock::acquire(&path) {
            Err(Error::AlreadyRunning { pid, data_dir }) => {
                assert_eq!(pid, Some(std::process::id()));
                assert_eq!(data_dir, dir);
            }
            other => panic!("expected AlreadyRunning, got {other:?}"),
        }
        drop(first);
        let again = InstanceLock::acquire(&path).unwrap();
        assert_eq!(again.path(), path);
        drop(again);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
