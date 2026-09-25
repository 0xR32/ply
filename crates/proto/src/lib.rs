//! Wire types for ply's three protocols and their version numbers.
//!
//! `ply-proto` is the single source of truth for both sides of every boundary in spec 3.3:
//! C1 control (JSON lines over `run/plyd.sock`, spec 4.1), C2 screen data (binary frames over
//! `run/data.sock`, spec 4.2) and C3 hook ingress (one JSON line over `run/hook.sock`, spec 4.3).
//! The TypeScript side of C1 and C3 is generated from these types.
//!
//! It holds types only (spec 3.2): no I/O, no policy and no dependency on any other ply crate
//! (spec 8.2). The types land in WP2; this crate is empty until then.

#![forbid(unsafe_code)]
