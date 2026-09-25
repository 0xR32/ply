//! Pane processes on a pseudo-terminal (C5), with the one audited `unsafe` block of plyd.
//!
//! [`spawn_pane`] opens a pty pair with rustix (`openpt(RDWR|NOCTTY)` + `FD_CLOEXEC`, `grantpt`, `unlockpt`,
//! `ptsname`, the slave opened `RDWR|NOCTTY|CLOEXEC`), sets the window size on the master, and starts the child with
//! `Command::env_clear()` and exactly the environment it is given, the slave as stdin/stdout/stderr, and a
//! `pre_exec` hook that makes the child a session leader (`setsid`) with the pty as its controlling terminal
//! (`TIOCSCTTY`). That hook is the audited `unsafe` block: it runs in the forked child, after std has dup2'd the
//! slave onto fd 0, and calls only the async-signal-safe `setsid(2)` and `ioctl(2)`, allocating nothing and taking
//! no lock. The parent's slave handles are closed right after the spawn so the master sees end-of-file when
//! the session ends. Three dedicated std threads per pane do the blocking work: a reader feeding pty output into a
//! bounded channel of [`OUTPUT_CHANNEL_CAPACITY`] chunks, a writer draining a bounded input channel into the master
//! (so a child that stops reading never blocks plyd's pane task), and a waiter reaping the child and reporting its
//! exit code (a signal death is 128 + signal). The child is its own process group, so [`PaneProcess::signal_group`]
//! reaches everything it started in the foreground.

use std::collections::BTreeMap;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;

use ply_proto::pane::PaneId;
use rustix::fs::{Mode, OFlags, open};
use rustix::io::{Errno, FdFlags, fcntl_setfd, read, write};
use rustix::process::{Pid, Signal, kill_process_group};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{Winsize, tcsetwinsize};
use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};

/// Chunks of pty output buffered between the reader thread and the pane task (spec 9.1).
pub const OUTPUT_CHANNEL_CAPACITY: usize = 64;

/// Writes buffered between the pane task and the writer thread.
pub const INPUT_CHANNEL_CAPACITY: usize = 64;

/// Largest single read from the master.
pub const READ_CHUNK: usize = 64 * 1024;

/// Exit code reported when the child could not be waited for.
pub const EXIT_UNKNOWN: i32 = -1;

/// A grid size with the pixel size of one cell (C2 ATTACH/RESIZE semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// Columns, at least 1.
    pub cols: u16,
    /// Rows, at least 1.
    pub rows: u16,
    /// Width of one cell in pixels (0 when unknown).
    pub cell_width_px: u16,
    /// Height of one cell in pixels (0 when unknown).
    pub cell_height_px: u16,
}

impl Geometry {
    /// The kernel window size: rows, columns and the whole grid's pixel size (saturating).
    pub fn winsize(self) -> Winsize {
        Winsize {
            ws_row: self.rows,
            ws_col: self.cols,
            ws_xpixel: self.cols.saturating_mul(self.cell_width_px),
            ws_ypixel: self.rows.saturating_mul(self.cell_height_px),
        }
    }
}

/// What to run: argv (program first), the complete environment, the working directory and the initial size.
#[derive(Debug, Clone, Copy)]
pub struct SpawnSpec<'a> {
    /// The pane, for thread names and log fields.
    pub pane_id: PaneId,
    /// argv; `argv[0]` is the program, exec'd without a shell. Must not be empty.
    pub argv: &'a [String],
    /// The child's whole environment; nothing of plyd's leaks through.
    pub env: &'a BTreeMap<String, String>,
    /// Working directory; must exist.
    pub cwd: &'a Path,
    /// Initial window size.
    pub geometry: Geometry,
}

/// A running pane process: its pid (also its process group) and the pty master.
#[derive(Debug)]
pub struct PaneProcess {
    pid: Pid,
    master: Arc<OwnedFd>,
}

/// The pane task's ends of the three threads.
#[derive(Debug)]
pub struct PtyChannels {
    /// pty output in arrival order; closes at end-of-file (the session ended and every slave handle closed).
    pub output: mpsc::Receiver<Vec<u8>>,
    /// Bytes for the pty, written in order; dropping it ends the writer thread.
    pub input: mpsc::Sender<Vec<u8>>,
    /// The exit code once the child is reaped; 128 + signal for a signal death, [`EXIT_UNKNOWN`] if waiting failed.
    pub exit: oneshot::Receiver<i32>,
}

impl PaneProcess {
    /// The child's pid, which is also its session and process group id.
    pub fn pid(&self) -> i32 {
        self.pid.as_raw_nonzero().get()
    }

    /// Sets the window size (`TIOCSWINSZ`); the kernel sends the foreground group SIGWINCH.
    /// Fails with [`Error::Pty`] (e.g. once the session has ended).
    pub fn resize(&self, geometry: Geometry) -> Result<()> {
        tcsetwinsize(&*self.master, geometry.winsize()).map_err(|e| Error::Pty {
            what: "TIOCSWINSZ",
            source: e.into(),
        })
    }

    /// Sends `signal` to the child's process group; fails with [`Error::Pty`] when the group no longer exists.
    pub fn signal_group(&self, signal: Signal) -> Result<()> {
        kill_process_group(self.pid, signal).map_err(|e| Error::Pty {
            what: "kill",
            source: e.into(),
        })
    }
}

/// Starts `spec` on a new pty (see the module docs) and returns the process with its channels; blocks only for fork/exec.
/// Fails with [`Error::Pty`] when the pty cannot be set up and [`Error::Spawn`] when the program cannot start.
pub fn spawn_pane(spec: &SpawnSpec<'_>) -> Result<(PaneProcess, PtyChannels)> {
    let pty = |what: &'static str| {
        move |e: Errno| Error::Pty {
            what,
            source: e.into(),
        }
    };
    let Some(program) = spec.argv.first() else {
        return Err(Error::Spawn {
            program: String::new(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "empty argv"),
        });
    };
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).map_err(pty("openpt"))?;
    fcntl_setfd(&master, FdFlags::CLOEXEC).map_err(pty("fcntl"))?;
    grantpt(&master).map_err(pty("grantpt"))?;
    unlockpt(&master).map_err(pty("unlockpt"))?;
    let slave_path = ptsname(&master, Vec::new()).map_err(pty("ptsname"))?;
    let slave = open(
        slave_path.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(pty("open slave"))?;
    // macOS refuses TIOCSWINSZ on a master whose slave has never been opened (ADR-0005).
    tcsetwinsize(&master, spec.geometry.winsize()).map_err(pty("TIOCSWINSZ"))?;

    let stdio = |fd: &OwnedFd| {
        fd.try_clone()
            .map(Stdio::from)
            .map_err(|source| Error::Pty {
                what: "dup slave",
                source,
            })
    };
    let mut cmd = Command::new(program);
    cmd.args(&spec.argv[1..])
        .env_clear()
        .envs(spec.env)
        .current_dir(spec.cwd)
        .stdin(stdio(&slave)?)
        .stdout(stdio(&slave)?)
        .stderr(Stdio::from(slave));
    // SAFETY: runs in the forked child; setsid(2) and ioctl(2) are async-signal-safe and fd 0 is already the slave.
    #[allow(unsafe_code)]
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()?;
            rustix::process::ioctl_tiocsctty(rustix::stdio::stdin())?;
            Ok(())
        });
    }
    let child = cmd.spawn().map_err(|source| Error::Spawn {
        program: program.clone(),
        source,
    })?;
    drop(cmd);
    let Some(pid) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) else {
        return Err(Error::Spawn {
            program: program.clone(),
            source: io::Error::other("the kernel returned an invalid pid"),
        });
    };

    let master = Arc::new(master);
    let (output_tx, output) = mpsc::channel(OUTPUT_CHANNEL_CAPACITY);
    let (input, input_rx) = mpsc::channel(INPUT_CHANNEL_CAPACITY);
    let (exit_tx, exit) = oneshot::channel();
    let pane_id = spec.pane_id;
    start_thread("pty-read", pane_id, {
        let master = Arc::clone(&master);
        move || read_loop(pane_id, &master, &output_tx)
    })?;
    start_thread("pty-write", pane_id, {
        let master = Arc::clone(&master);
        move || write_loop(pane_id, &master, input_rx)
    })?;
    start_thread("pty-wait", pane_id, move || {
        wait_loop(pane_id, child, exit_tx)
    })?;
    tracing::info!(pane_id, pid = pid.as_raw_nonzero().get(), program = %program, "pane process started");
    Ok((
        PaneProcess { pid, master },
        PtyChannels {
            output,
            input,
            exit,
        },
    ))
}

fn start_thread(
    role: &'static str,
    pane_id: PaneId,
    body: impl FnOnce() + Send + 'static,
) -> Result<()> {
    thread::Builder::new()
        .name(format!("{role}-{pane_id}"))
        .spawn(body)
        .map(drop)
        .map_err(|source| Error::Pty {
            what: "thread spawn",
            source,
        })
}

fn read_loop(pane_id: PaneId, master: &OwnedFd, tx: &mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        match read(master, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(Errno::INTR) => {}
            Err(Errno::IO) => {
                tracing::debug!(pane_id, "pty slave closed");
                break;
            }
            Err(e) => {
                tracing::warn!(pane_id, error = %e, "pty read failed");
                break;
            }
        }
    }
    tracing::debug!(pane_id, "pty reader finished");
}

fn write_loop(pane_id: PaneId, master: &OwnedFd, mut rx: mpsc::Receiver<Vec<u8>>) {
    while let Some(bytes) = rx.blocking_recv() {
        let mut rest = bytes.as_slice();
        while !rest.is_empty() {
            match write(master, rest) {
                Ok(n) => rest = &rest[n..],
                Err(Errno::INTR) => {}
                Err(e) => {
                    tracing::warn!(pane_id, error = %e, dropped = rest.len(), "pty write failed");
                    return;
                }
            }
        }
    }
}

fn wait_loop(pane_id: PaneId, mut child: Child, tx: oneshot::Sender<i32>) {
    let code = match child.wait() {
        Ok(status) => match (status.code(), status.signal()) {
            (Some(code), _) => code,
            (None, Some(signal)) => 128 + signal,
            (None, None) => EXIT_UNKNOWN,
        },
        Err(e) => {
            tracing::error!(pane_id, error = %e, "waiting for the pane process failed");
            EXIT_UNKNOWN
        }
    };
    tracing::info!(pane_id, code, "pane process exited");
    if tx.send(code).is_err() {
        tracing::debug!(pane_id, "the pane task was gone before its process exited");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn env() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("TERM".to_owned(), "xterm-256color".to_owned()),
            ("PLY_EXAMPLE".to_owned(), "1".to_owned()),
        ])
    }

    async fn collect(ch: &mut PtyChannels) -> String {
        let mut out = Vec::new();
        while let Ok(Some(chunk)) =
            tokio::time::timeout(Duration::from_secs(5), ch.output.recv()).await
        {
            out.extend_from_slice(&chunk);
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn the_child_gets_a_controlling_tty_the_size_and_exactly_its_env() {
        let argv: Vec<String> = [
            "/bin/sh",
            "-c",
            "stty size; env | sort; ps -o pid=,pgid=,tpgid= -p $$; echo ctty > /dev/tty",
        ]
        .map(str::to_owned)
        .into();
        let env = env();
        let spec = SpawnSpec {
            pane_id: 1,
            argv: &argv,
            env: &env,
            cwd: Path::new("/"),
            geometry: Geometry {
                cols: 111,
                rows: 33,
                cell_width_px: 8,
                cell_height_px: 17,
            },
        };
        let (process, mut ch) = spawn_pane(&spec).unwrap();
        let text = collect(&mut ch).await;
        assert_eq!(ch.exit.await.unwrap(), 0);
        assert!(text.contains("33 111"), "{text}");
        assert!(text.contains("PLY_EXAMPLE=1"), "{text}");
        assert!(text.contains("ctty"), "{text}");
        let pid = process.pid().to_string();
        let ps = text
            .lines()
            .find(|l| l.trim_start().starts_with(&pid))
            .unwrap();
        let ids: Vec<&str> = ps.split_whitespace().collect();
        assert_eq!(ids, [pid.as_str(); 3], "pid = pgid = tpgid");
        let vars: Vec<&str> = text
            .lines()
            .filter(|l| l.contains('='))
            .map(|l| l.split('=').next().unwrap_or(""))
            .collect();
        for v in vars {
            assert!(
                ["PATH", "TERM", "PLY_EXAMPLE", "PWD", "SHLVL", "_", "OLDPWD"].contains(&v),
                "unexpected variable {v} in {text}"
            );
        }
    }

    #[tokio::test]
    async fn a_signal_death_reports_128_plus_the_signal() {
        let argv: Vec<String> = ["/bin/sh", "-c", "sleep 30"].map(str::to_owned).into();
        let env = env();
        let spec = SpawnSpec {
            pane_id: 2,
            argv: &argv,
            env: &env,
            cwd: Path::new("/"),
            geometry: Geometry {
                cols: 80,
                rows: 24,
                cell_width_px: 0,
                cell_height_px: 0,
            },
        };
        let (process, ch) = spawn_pane(&spec).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        process.signal_group(Signal::KILL).unwrap();
        assert_eq!(ch.exit.await.unwrap(), 128 + 9);
    }

    #[test]
    fn a_missing_program_is_a_spawn_error() {
        let argv = vec!["/no/such/program-example".to_owned()];
        let env = env();
        let spec = SpawnSpec {
            pane_id: 3,
            argv: &argv,
            env: &env,
            cwd: Path::new("/"),
            geometry: Geometry {
                cols: 80,
                rows: 24,
                cell_width_px: 0,
                cell_height_px: 0,
            },
        };
        assert!(matches!(spawn_pane(&spec), Err(Error::Spawn { .. })));
    }
}
