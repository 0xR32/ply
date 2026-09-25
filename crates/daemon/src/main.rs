//! `plyd`, ply's background daemon.
//!
//! plyd owns every pane's process and pty, runs one libghostty-vt terminal per pane, encodes
//! keys and mouse input against it, publishes Snapshots and Deltas over C2 (spec 4.2), serves the
//! C1 control plane (spec 4.1) and the C3 hook socket (spec 4.3), drives the status state machine
//! (spec 6.3) and stores sessions in SQLite (spec 11.2). It is a per-user LaunchAgent, so closing
//! the app stops nothing (spec 11.3).
//!
//! It never renders and never links gpui (spec 3.2, 8.2). The daemon lands in WP4; until then
//! this binary does nothing and exits 0.

#![forbid(unsafe_code)]

fn main() {}
