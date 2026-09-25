//! `ply-hook`, the forwarder that Claude Code hooks and Codex's notify program run.
//!
//! `ply-hook <cli> [event]` reads one payload (stdin, at most 1 MiB), wraps it in the C3 envelope
//! (spec 4.3) with `PLY_PANE_ID`, writes one line to `PLY_HOOK_SOCK` and exits.
//!
//! It never blocks or fails the CLI: it exits 0 on every path within 200 ms and never prints to
//! stdout (INV-12, INV-14). It depends on std and serde_json only (spec 8.2). The forwarder lands
//! in WP6; until then this binary does nothing and exits 0.

#![forbid(unsafe_code)]

fn main() {}
