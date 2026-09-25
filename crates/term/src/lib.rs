//! ply's headless terminal core (spec 3.2 `crates/term`, WP3): the pane terminal, C2 frame building and the
//! reference replica.
//!
//! With the `engine` feature, which only `ply-daemon` enables (INV-17, spec 8.2), the crate wraps libghostty-vt:
//!
//! - [`Engine`]: one pane's terminal. [`Engine::write`] feeds pty output and returns an [`EngineOutput`] with the
//!   bytes to write back (answers to DA, DSR, OSC 4/10/11, `CSI ? u` and size queries, all from the pane's
//!   [`Palette`], spec R-R4) and the title, bell, working-directory (OSC 7), notification (OSC 9/777, C8), progress
//!   and clipboard-write (OSC 52, R-R11) events. It also resizes with reflow (R-R12), reports DEC 2026 (R-R18),
//!   compresses idle scrollback (R-R22), searches, and saves and restores its full state across a plyd restart.
//! - [`DeltaBuilder`]: one per attached client; builds that client's C2 Snapshot, Deltas (dirty rows plus newly
//!   interned styles) and History pages (spec 4.2).
//! - [`encode_input`]: KEY, MOUSE, PASTE and FOCUS frames to pty bytes against the pane's live modes (R-R5 to
//!   R-R10), including the ⇧⏎ → LF rule (R-R6).
//!
//! Always available: [`Replica`], the reference C2 decoder that applies Snapshot, Delta and History frames into a
//! grid (used by plyd's tests and the CLI test client; the app has its own TypeScript replica), and [`Palette`].
//!
//! The crate never does I/O, never spawns threads, and depends on neither gpui nor tokio (spec 3.2, 8.2). Every
//! call is synchronous and bounded; an [`Engine`] is `Send` but not `Sync`, so plyd serializes each pane's calls.
//! `unsafe` is denied crate-wide and allowed only inside the `engine` module, which owns every libghostty-vt handle
//! behind a safe API (spec 9.1); all other code, including [`DeltaBuilder`] and [`encode_input`], is safe Rust.
//!
//! ```
//! # #[cfg(feature = "engine")]
//! # fn main() -> Result<(), ply_term::Error> {
//! use ply_proto::data::Frame;
//! use ply_proto::pane::Rgb;
//! use ply_term::{DeltaBuilder, Engine, Palette, Replica};
//!
//! let grey = |v: u8| Rgb { r: v, g: v, b: v };
//! let palette = Palette {
//!     ansi: [grey(128); 16],
//!     fg: grey(220),
//!     bg: grey(16),
//!     cursor: grey(240),
//!     cursor_text: grey(16),
//!     selection_bg: grey(64),
//!     selection_fg: grey(220),
//! };
//! let mut engine = Engine::new(1, 80, 24, 10_000, &palette)?;
//! let out = engine.write(b"hello\x1b[6n");
//! assert_eq!(out.reply, b"\x1b[1;6R"); // the cursor-position report goes back to the pty
//!
//! let mut client = DeltaBuilder::new();
//! let mut replica = Replica::new();
//! replica.apply(&Frame::Snapshot(client.snapshot(&mut engine)?))?;
//! engine.write(b"\r\nworld");
//! if let Some(update) = client.delta(&mut engine)? {
//!     replica.apply(&update.into())?;
//! }
//! assert_eq!(replica.row_text(0), "hello");
//! assert_eq!(replica.row_text(1), "world");
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "engine"))]
//! # fn main() {}
//! ```

#![deny(unsafe_code)]
// The crate docs link the `engine` items, which a build without the feature does not have.
#![cfg_attr(not(feature = "engine"), allow(rustdoc::broken_intra_doc_links))]

#[cfg(feature = "engine")]
mod delta;
#[cfg(feature = "engine")]
mod engine;
mod error;
#[cfg(feature = "engine")]
mod input;
mod palette;
mod replica;

#[cfg(feature = "engine")]
pub use delta::{DeltaBuilder, Update};
#[cfg(feature = "engine")]
pub use engine::{
    ClipboardContent, ClipboardWrite, Compression, Engine, EngineOutput, Notification,
    ProgressReport, ProgressState, SCROLLBACK_SLACK, SearchMatch, TERMINFO_NAME, XTVERSION,
};
pub use error::{Error, Result};
#[cfg(feature = "engine")]
pub use input::{Encoded, Input, encode_input};
pub use palette::Palette;
pub use replica::{Replica, ReplicaRow, ResolvedCell, cells_text};
