//! Agent integration for the Claude Code and Codex CLIs.
//!
//! `ply-agents` builds each CLI's launch spec (argv, env, cwd; spec 6.1 and 6.2, contract C7),
//! generates the per-pane Claude Code hook settings, parses Codex rollout records (C4) and notify
//! payloads, classifies OSC 9 messages (C8) and tracks the session metadata a pane reports (model,
//! worktree, directory; spec 6.5).
//!
//! It does no I/O beyond reading files it is given and never depends on gpui, `ply-term` or tokio
//! (spec 3.2, 8.2). The adapters land in WP6; this crate is empty until then.

#![forbid(unsafe_code)]
