//! Helpers for plyd's integration tests: a sandboxed home and `PLY_HOME`, a plyd child process, and the clients.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

pub mod client;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub use client::{Control, Data};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// The palette every test sends; the background answers OSC 11 as `rgb:0c0c/0e0e/1414`.
pub fn theme() -> Value {
    json!({
        "ansi": [
            "#0A0B10", "#F07A6A", "#7FD4B0", "#F2B35B", "#8AB4FF", "#B4A5FF", "#86CDBB", "#E6E8EF",
            "#8D93A4", "#F28B7D", "#8FDBBB", "#F4BE72", "#9CC0FF", "#C0B3FF", "#96D4C4", "#F2F3F7"
        ],
        "fg": "#E6E8EF",
        "bg": "#0C0E14",
        "cursor": "#8AB4FF",
        "cursorText": "#0C0E14",
        "selectionBg": "#2A3550",
        "selectionFg": "#E6E8EF"
    })
}

/// A throwaway directory holding a fake home and `PLY_HOME`; removed on drop. Nothing touches the real home.
pub struct Sandbox {
    /// The sandbox root.
    pub root: PathBuf,
    /// `HOME` of plyd and its panes.
    pub home: PathBuf,
    /// `PLY_HOME`.
    pub ply_home: PathBuf,
}

impl Sandbox {
    /// A new sandbox under the system temp directory; `tag` keeps paths short and readable.
    pub fn new(tag: &str) -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("ply-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let ply_home = root.join("ply");
        std::fs::create_dir_all(&home).unwrap();
        let sandbox = Self {
            root,
            home,
            ply_home,
        };
        assert!(
            sandbox.data_socket().as_os_str().len() < 104,
            "sandbox socket path too long: {}",
            sandbox.data_socket().display()
        );
        sandbox
    }

    /// `run/plyd.sock`.
    pub fn control_socket(&self) -> PathBuf {
        self.ply_home.join("run").join("plyd.sock")
    }

    /// `run/data.sock`.
    pub fn data_socket(&self) -> PathBuf {
        self.ply_home.join("run").join("data.sock")
    }

    /// `plyd <args>` with a cleared environment: the sandbox home and `PLY_HOME`, `/bin/sh` as login shell.
    pub fn plyd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_plyd"));
        cmd.args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PLY_HOME", &self.ply_home)
            .env("SHELL", "/bin/sh")
            .env("USER", "example")
            .env("LANG", "en_US.UTF-8")
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("TMPDIR", std::env::temp_dir())
            .env("PLY_LOG", "debug")
            .stdin(Stdio::null());
        cmd
    }

    /// Starts `plyd --foreground` and waits until its control socket accepts connections.
    pub fn start(&self) -> Plyd {
        self.start_with(&[])
    }

    /// [`Sandbox::start`] with extra environment variables for plyd.
    pub fn start_with(&self, env: &[(&str, &str)]) -> Plyd {
        let log = std::fs::File::create(self.root.join(format!(
            "plyd-{}.stderr",
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
        .unwrap();
        let mut cmd = self.plyd(&["--foreground"]);
        cmd.envs(env.iter().copied())
            .stdout(Stdio::null())
            .stderr(log);
        let mut plyd = Plyd {
            child: cmd.spawn().unwrap(),
        };
        let up = eventually(Duration::from_secs(20), || {
            std::os::unix::net::UnixStream::connect(self.control_socket()).is_ok()
        });
        assert!(up, "plyd did not come up in {}", self.root.display());
        assert!(plyd.is_running());
        plyd
    }

    /// A C1 client that has sent the test palette; returns it with the default workspace's id.
    pub fn control(&self) -> (Control, u64) {
        let mut c = Control::connect(&self.control_socket()).unwrap();
        c.call("theme.set", json!({"palette": theme()})).unwrap();
        let ws = c.call("workspace.list", json!({})).unwrap();
        let id = ws[0]["id"].as_u64().unwrap();
        (c, id)
    }

    /// Creates a shell pane in the sandbox home and returns its id.
    pub fn shell(&self, c: &mut Control, workspace: u64) -> u64 {
        let pane = c
            .call(
                "pane.create",
                json!({"workspace_id": workspace, "cli": "shell", "cwd": self.home}),
            )
            .unwrap();
        pane["id"].as_u64().unwrap()
    }

    /// Attaches at 80 × 24 and waits for the shell prompt (`$ ` or `# `) to appear.
    pub fn attach_ready(&self, pane: u64) -> Data {
        let (mut d, first) = Data::attach(&self.data_socket(), pane, 80, 24).unwrap();
        d.apply(&first, true).unwrap();
        assert!(
            d.pump_until(Duration::from_secs(10), |d| d
                .screen()
                .iter()
                .any(|r| r.trim_end().ends_with('$') || r.trim_end().ends_with('#')))
                .unwrap(),
            "no prompt: {:?}",
            d.screen()
        );
        d
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A running plyd; stopped with SIGTERM (then SIGKILL) when dropped.
pub struct Plyd {
    /// The process.
    pub child: Child,
}

impl Plyd {
    /// Whether the process is still running.
    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }

    /// Sends SIGTERM and waits up to 10 s for the exit.
    pub fn stop(&mut self) -> ExitStatus {
        let pid = rustix::process::Pid::from_raw(self.child.id() as i32).unwrap();
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        self.child.wait().unwrap()
    }
}

impl Drop for Plyd {
    fn drop(&mut self) {
        if self.is_running() {
            self.stop();
        }
    }
}

/// Polls `f` every 20 ms for up to `timeout`.
pub fn eventually(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

/// Reads `path` once it exists and is non-empty.
pub fn read_when_ready(path: &Path, timeout: Duration) -> Option<String> {
    let mut text = None;
    eventually(timeout, || {
        text = std::fs::read_to_string(path).ok().filter(|t| !t.is_empty());
        text.is_some()
    });
    text
}
