//! ply's headless terminal core.
//!
//! With the `engine` feature (enabled by `ply-daemon` only), `ply-term` wraps libghostty-vt in a
//! safe per-pane `Engine`, turns its render state into C2 Snapshots and Deltas (spec 4.2) and
//! encodes KEY, MOUSE, PASTE and FOCUS input through libghostty-vt's encoders (spec 5.2, R-R5 to
//! R-R10). Without it, the crate still provides the reference `Replica` that applies C2 frames to
//! a grid, and the palette.
//!
//! It never does I/O and never depends on gpui or tokio (spec 3.2, 8.2); ghostty-sys is reachable
//! only through the `engine` feature (INV-17). The engine, the delta builder and the replica land
//! in WP3; this crate is empty until then.

#![forbid(unsafe_code)]
