//! Agent integration for Claude Code and Codex: launch specs (C7, spec 6.1/6.2), the per-pane Claude hook settings, and
//! hook (C3), notify, OSC 9 (C8) and rollout (C4) events turned into [`AdapterSignal`]s for plyd's spec 6.3 machine.
//!
//! plyd picks an [`Adapter`] with [`adapter`], spawns what [`Adapter::launch`] returns after writing its files, and feeds
//! every event of the pane to that pane's [`AgentSession`]. Session metadata (model, worktree, cwd, session id) is only
//! ever what the CLI reports (spec 6.5, INV-7); progress stays hidden until the agent's own plan appears (R16, R26).
//! Plan usage is parsed from bytes plyd reads out of the CLIs' own files ([`usage`], Ruling R59), and the skills each CLI
//! offers from their skill, command and plugin registry files ([`skills`], Ruling R61).
//!
//! The crate never runs a CLI, never writes a file, and reads only the install metadata named in
//! [`Adapter::installed_version`] (R14); it depends on no ply crate but `ply-proto`, and never on gpui or tokio (spec 3.2,
//! 8.2). Nothing in it panics on malformed CLI output.
//!
//! ```
//! use ply_agents::{AdapterSignal, AgentEvent, StatusSignal, adapter};
//! use ply_proto::pane::AgentCli;
//!
//! let codex = adapter(AgentCli::Codex);
//! let spec = ply_agents::LaunchSpec {
//!     cli: ply_proto::pane::Cli::Codex,
//!     argv: vec!["/usr/local/bin/codex".into()],
//!     env: Default::default(),
//!     cwd: "/Users/example/project".into(),
//!     worktree: None,
//!     resume: None,
//! };
//! let mut session = codex.new_session(&spec);
//! let signals = session.handle(AgentEvent::Osc9("Codex wants to edit a.txt")).unwrap();
//! assert!(matches!(
//!     signals.as_slice(),
//!     [AdapterSignal::Status(StatusSignal::PermissionRequested { call: None, .. })]
//! ));
//! ```

#![forbid(unsafe_code)]

mod adapter;
pub mod claude;
pub mod codex;
mod error;
mod install;
pub mod meta;
pub mod plan;
pub mod skills;
pub mod usage;
pub mod version;

pub use adapter::{
    Adapter, AdapterSignal, AgentEvent, AgentSession, COLORTERM, ENV_HOOK_SOCK, ENV_PANE_ID,
    LAUNCH_FILE, Launch, LaunchRequest, LaunchSpec, PaneFile, SessionStats, StatusSignal, TERM,
    ToolCall, adapter, cli_name,
};
pub use error::{Error, Result};
pub use meta::SessionMeta;
pub use version::CliVersion;
