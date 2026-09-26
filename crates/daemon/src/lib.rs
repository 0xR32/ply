//! `ply-daemon`, the library behind `plyd`, ply's per-user background daemon (spec 3.1, 3.2 plyd row, WP4).
//!
//! plyd owns every pane's process and pty (C5), runs one libghostty-vt terminal per pane through `ply-term`, and
//! serves the app over two Unix sockets in the run directory: C1 control, JSON lines on `plyd.sock` (spec 4.1,
//! [`server::control`]), and C2 screen data, binary frames on `data.sock` (spec 4.2, [`server::data`] and
//! [`publisher`]); the agents' hooks arrive on a third, `hook.sock` (C3, spec 4.3). It stores workspaces, tabs and
//! every pane's session record in SQLite (spec 11.2, [`db`]), keeps its
//! settings and palette in `config.toml` ([`config`]), and keeps running when the app closes (spec 11.3): it is a
//! LaunchAgent ([`launchd`]) with no idle exit, holding a power assertion while an agent runs ([`power`]). One plyd
//! runs per data directory ([`lock`]); every path moves under `PLY_HOME` for tests ([`paths`]).
//!
//! It never renders, never links gpui (spec 3.2, 8.2, INV-3), never sends raw pty bytes over C1 (INV-2), never
//! writes the CLIs' own configuration (INV-8) and never creates or records the CLIs' worktrees (INV-7). `unsafe`
//! is denied crate-wide and allowed only in the one audited `pre_exec` block of [`pty`] (spec 9.1).
//!
//! Agent panes report what the CLI is doing (spec 6, WP6): the C3 hook server ([`server::hooks`]) takes Claude Code's
//! hooks and Codex's notify, the rollout tailer ([`tail`]) follows Codex's session file (C4), OSC 7 and OSC 9 come from
//! the engine ([`osc`]), and each pane task drives the spec 6.3 state machine ([`panes::state`]) through its agent
//! integration ([`panes::agent`]) into `pane.status`, `pane.progress` and `pane.meta`, with a branch label from git
//! ([`branch`]). After a restart, panes whose process is gone are `lost`, and `pane.resume` brings them back (WP9).
//! `usage.get` answers with the plan usage the CLIs report: Claude panes' status lines, else the CLIs' own files ([`usage`], Ruling R59), read-only.
//!
//! ```no_run
//! # async fn start() -> Result<(), ply_daemon::Error> {
//! use ply_daemon::daemon::{Options, run};
//! use ply_daemon::lock::InstanceLock;
//! use ply_daemon::paths::Paths;
//!
//! let paths = Paths::from_env(None)?;
//! std::fs::create_dir_all(&paths.data_dir).ok();
//! let _lock = InstanceLock::acquire(&paths.lock())?;
//! let hook_program = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join("ply-hook")));
//! run(Options { paths, hook_program: hook_program.unwrap_or_default(), keep_awake: None }).await
//! # }
//! ```

#![deny(unsafe_code)]

pub mod branch;
pub mod config;
pub mod daemon;
pub mod db;
mod error;
pub mod launchd;
pub mod lock;
pub mod login;
pub mod osc;
pub mod panes;
pub mod paths;
pub mod power;
pub mod pty;
pub mod publisher;
#[cfg(debug_assertions)]
pub mod replay;
pub mod server;
pub mod tail;
pub mod usage;

pub use error::{Error, Result};
