//! Wire types for ply's three protocols and their version numbers.
//!
//! `ply-proto` is the single source of truth for both sides of every boundary in spec 3.3:
//!
//! - [`control`]: C1 control, JSON lines over `run/plyd.sock` (spec 4.1 plus ADR-0009's `pane.resume`,
//!   `settings.get` and `settings.set`). The app's TypeScript types are generated from it by ts-rs into
//!   `app/src/ipc/proto.gen.ts` (`bun run gen`), which CI keeps fresh.
//! - [`data`]: C2 screen data, binary frames over `run/data.sock` (spec 4.2 with ADR-0005's field corrections,
//!   Rulings R20 and R21), hand-encoded little-endian with no serialisation crate.
//! - [`hook`]: C3 hook ingress, the envelope `ply-hook` writes to `run/hook.sock` (spec 4.3).
//! - [`pane`]: the records C1 carries (panes, workspaces, tabs, layouts, sessions, the palette, settings).
//! - [`version`]: [`PROTOCOL_VERSION`], [`C2_VERSION`] and [`HOOK_VERSION`]. Version 1 of all three is frozen:
//!   a change follows spec rule 0.1.1 (an ADR and a spec bump).
//!
//! Every JSON struct rejects unknown fields and every C2 decoder rejects out-of-range values (INV-10); no C1 type
//! carries pty bytes (INV-2). The crate holds types and their codecs only: no I/O beyond reading a given stream,
//! no policy, and no dependency on any other ply crate (spec 3.2, 8.2). Nothing here panics on malformed input.

#![forbid(unsafe_code)]

pub mod control;
pub mod data;
mod error;
pub mod hook;
pub mod pane;
pub mod version;

pub use error::{Error, Result};
pub use version::{C2_VERSION, HOOK_VERSION, PROTOCOL_VERSION};
