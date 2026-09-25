//! The safe per-pane libghostty-vt terminal (feature `engine`, plyd only; INV-17).
//!
//! This module and its children are the only place in ply-term with `unsafe`: every libghostty-vt handle is owned
//! by an [`Engine`] and freed on drop. An `Engine` is `Send` but not `Sync`, so all calls on one terminal, through
//! `&self` or `&mut self`, run on one thread at a time, as the library requires; calls that mutate the terminal take
//! `&mut self`. Callbacks write only into state the engine owns (see `effects`). Nothing here blocks or does I/O.
//!
//! An [`Engine`] is configured the way ADR-0005 Decision 4 lists before any child byte arrives: effect callbacks,
//! the palette (so OSC 4/10/11 are answered, R-R4), XTVERSION `ply <version>` and grapheme clustering (mode 2027) as
//! the reset default (R22), `TERMINFO_NAME` `xterm-256color`, no scrollback byte cap and a line cap of
//! `scrollback_lines + 300` (R-R22), continuation tracking for snapshots, OSC 5522 clipboard writes capped at
//! [`CLIPBOARD_WRITE_MAX_BYTES`], and Kitty graphics and the Glyph Protocol off (C2 carries neither). The library's
//! log goes to `tracing` under the target `libghostty_vt`; the hook is installed before the first terminal exists.
//!
//! Dirty tracking: libghostty-vt's render state consumes the terminal's dirty flags, so each engine owns exactly one
//! and records, per viewport row, the generation at which it last changed; every [`crate::DeltaBuilder`] (one per
//! attached client) sends the rows newer than its last frame, so several clients never steal each other's changes.
//!
//! Absolute lines: the library does not count the lines its line cap prunes, so after every write and resize the engine
//! reads how far a tracked reference at the top of the live screen moved up and adds that to
//! [`Engine::scrollback_base`], then moves the reference back to the top of the live screen.

#![allow(unsafe_code)]

mod cells;
mod effects;
mod encoders;
mod render;

use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::{self, NonNull};
use std::sync::Once;

use ghostty_sys as sys;
use ply_proto::data::{CellFlags, Cursor, History, Modes, Snapshot, Style};
use ply_proto::pane::OptionAsMeta;

use self::cells::{CellReadError, RawCell, RefusedReads};
use self::effects::Effects;
use self::encoders::{KeyEncoder, MouseEncoder, Surface};
pub(crate) use self::encoders::{KeyInput, MouseInput, PasteResult};
use self::render::{Dirty, RenderState};
use crate::delta::DeltaBuilder;
use crate::error::{Error, Result};
use crate::palette::Palette;

/// The XTVERSION (`CSI > q`) answer, `ply <version>` (Ruling R22).
pub const XTVERSION: &str = concat!("ply ", env!("CARGO_PKG_VERSION"));

/// The terminfo name XTGETTCAP `TN` reports; plyd sets `TERM` to the same (C5).
pub const TERMINFO_NAME: &str = "xterm-256color";

/// Rows added to `scrollback_lines` for libghostty-vt's line cap, which prunes whole pages and keeps fewer rows than asked (ADR-0005).
pub const SCROLLBACK_SLACK: u32 = 300;

/// Bytes of an unfinished escape sequence kept so a snapshot taken mid-sequence restores it.
const CONTINUATION_MAX_BYTES: usize = 64 * 1024;

/// Most decoded bytes one OSC 5522 clipboard write may buffer (the library's default is 64 MiB); OSC 52 is bounded by the sequence length instead.
pub const CLIPBOARD_WRITE_MAX_BYTES: usize = 4 * 1024 * 1024;

/// OSC 9 / OSC 777 desktop notification (C8); OSC 9 bodies libghostty-vt parses as ConEmu commands never arrive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// Title (OSC 777 only; empty for OSC 9), lossy UTF-8.
    pub title: String,
    /// Body, lossy UTF-8.
    pub body: String,
}

/// State of an OSC 9;4 progress report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    /// The program cleared its progress; hide the indicator.
    Remove,
    /// Normal progress at `percent`.
    Set,
    /// The task failed; show `percent`, if any, as an error.
    Error,
    /// Busy with no measurable progress (`percent` is `None`).
    Indeterminate,
    /// The task is paused; keep showing `percent` as paused.
    Pause,
}

/// An OSC 9;4 progress report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressReport {
    /// The state the program reported.
    pub state: ProgressState,
    /// Percent 0–100 when the program gave one.
    pub percent: Option<u8>,
}

/// One MIME representation of an OSC 52 clipboard write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardContent {
    /// MIME type, e.g. `text/plain`, lossy UTF-8.
    pub mime: String,
    /// The data, already base64-decoded, in the representation `mime` names.
    pub data: Vec<u8>,
}

/// An OSC 52 clipboard write, already accepted (writes are allowed, reads refused: R-R11); empty `contents` means clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardWrite {
    /// Representations of one value, all to be written together.
    pub contents: Vec<ClipboardContent>,
}

/// What one [`Engine::write`] (or resize) produced besides screen changes; screen changes reach clients through [`crate::DeltaBuilder`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineOutput {
    /// Bytes to write back to the pty, in order: query answers (DA, DSR, OSC 4/10/11, `CSI ? u`, size reports), mode reports and XTVERSION.
    pub reply: Vec<u8>,
    /// BEL characters received during the call, 0 when none.
    pub bells: u32,
    /// The new title (OSC 0/2, lossy UTF-8) when it changed, however often it changed during the write.
    pub title: Option<String>,
    /// The new working directory as reported (OSC 7 is a raw `file://` URI, not decoded) when it changed.
    pub pwd: Option<String>,
    /// Desktop notifications, in arrival order.
    pub notifications: Vec<Notification>,
    /// The last progress report of the write.
    pub progress: Option<ProgressReport>,
    /// Clipboard writes, in arrival order.
    pub clipboard_writes: Vec<ClipboardWrite>,
}

impl EngineOutput {
    /// True when the write produced nothing besides screen changes.
    pub fn is_empty(&self) -> bool {
        self.reply.is_empty()
            && self.bells == 0
            && self.title.is_none()
            && self.pwd.is_none()
            && self.notifications.is_empty()
            && self.progress.is_none()
            && self.clipboard_writes.is_empty()
    }
}

/// Result of one idle-compression step (R-R22, Ruling R19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// More work remains; call again.
    Pending,
    /// Everything compressible is compressed until the activity token changes.
    Complete,
    /// This build cannot compress.
    Unsupported,
}

/// One search match in absolute lines (the scrollback from [`Engine::scrollback_base`], then the live screen); start and end are inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SearchMatch {
    /// Absolute line of the first matched cell.
    pub start_line: u64,
    /// 0-based column of the first matched cell.
    pub start_col: u16,
    /// Absolute line of the last matched cell (after `start_line` when the match wraps).
    pub end_line: u64,
    /// 0-based column of the last matched cell.
    pub end_col: u16,
}

/// Receives a grid, row by row and cell by cell, from the engine's readers.
pub(crate) trait RowSink {
    /// A row starts: its index relative to the live screen (negative in the scrollback) and soft-wrap flag.
    fn begin_row(&mut self, index: i32, wrapped: bool);
    /// The next cell of the row, from column 0.
    fn cell(&mut self, codepoint: u32, style: &Style, flags: CellFlags, extra: &[u32]);
    /// The row is complete.
    fn end_row(&mut self);
}

/// A terminal handle and the effects allocation its callbacks write into, freed in that order.
struct Terminal {
    raw: sys::GhosttyTerminal,
    effects: Option<NonNull<Effects>>,
}

impl Terminal {
    fn new(raw: sys::GhosttyTerminal) -> Self {
        Self { raw, effects: None }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: the terminal is owned here and freed once; the engine drops everything made from it first.
        unsafe { sys::ghostty_terminal_free(self.raw) };
        if let Some(effects) = self.effects.take() {
            // SAFETY: the allocation came from Box::leak and nothing can reach it once the terminal is gone.
            drop(unsafe { Box::from_raw(effects.as_ptr()) });
        }
    }
}

/// A tracked reference at the top of the live screen, which follows its row as the scrollback grows and is pruned.
struct Anchor {
    raw: sys::GhosttyTrackedGridRef,
}

impl Anchor {
    /// The tracked row's y in screen space (scrollback plus live screen, 0 the oldest row); `None` once it was discarded.
    fn screen_y(&self, pane_id: u64) -> Option<u32> {
        let mut point = sys::GhosttyPointCoordinate::default();
        // SAFETY: the reference is live until drop; the out pointer is a point coordinate.
        let code = unsafe {
            sys::ghostty_tracked_grid_ref_point(
                self.raw,
                sys::GHOSTTY_POINT_TAG_SCREEN,
                &raw mut point,
            )
        };
        match code {
            sys::GHOSTTY_SUCCESS => Some(point.y),
            sys::GHOSTTY_NO_VALUE => None,
            code => {
                tracing::warn!(
                    pane_id,
                    code,
                    "libghostty-vt could not place the scrollback anchor"
                );
                None
            }
        }
    }
}

impl Drop for Anchor {
    fn drop(&mut self) {
        // SAFETY: the reference is owned here and freed once; the library allows this before or after its terminal.
        unsafe { sys::ghostty_tracked_grid_ref_free(self.raw) };
    }
}

/// One pane's libghostty-vt terminal with its render state and encoders; `Send`, not `Sync`, never blocks.
pub struct Engine {
    pane_id: u64,
    cols: u16,
    rows: u16,
    cell_width: u16,
    cell_height: u16,
    scrollback_lines: u32,
    option_as_alt: sys::GhosttyOptionAsAlt,
    buttons: u16,
    generation: u64,
    row_generation: Vec<u64>,
    scrollback_base: u64,
    anchor_y: u32,
    anchor: Option<Anchor>,
    key: KeyEncoder,
    mouse: MouseEncoder,
    render: RenderState,
    effects: NonNull<Effects>,
    term: Terminal,
    not_sync: PhantomData<*mut ()>,
}

// SAFETY: every handle is owned by this Engine, which is !Sync (the marker), so one thread uses it at a time; libghostty-vt has no thread affinity.
unsafe impl Send for Engine {}

impl Engine {
    /// A `cols` × `rows` terminal configured as the module docs list; errors: [`Error::InvalidSize`] for a zero size, [`Error::Ghostty`] if the library refuses.
    pub fn new(
        pane_id: u64,
        cols: u16,
        rows: u16,
        scrollback_lines: u32,
        palette: &Palette,
    ) -> Result<Self> {
        install_log_hook();
        if cols == 0 || rows == 0 {
            return Err(Error::InvalidSize { cols, rows });
        }
        let mut raw = ptr::null_mut();
        // SAFETY: `raw` receives a new terminal owned by the `Terminal` guard below.
        check("ghostty_terminal_new", unsafe {
            sys::ghostty_terminal_new(ptr::null(), &raw mut raw, cols, rows)
        })?;
        Self::configure(pane_id, Terminal::new(raw), scrollback_lines, palette)
    }

    /// A terminal decoded from [`Engine::save`] output (possibly from an earlier plyd), re-configured as [`Engine::new`] does; errors: [`Error::Ghostty`] when the bytes do not decode.
    pub fn restore(
        pane_id: u64,
        state: &[u8],
        scrollback_lines: u32,
        palette: &Palette,
    ) -> Result<Self> {
        install_log_hook();
        let mut decoder = ptr::null_mut();
        // SAFETY: `state` outlives the decoder, which is freed below before returning.
        check("ghostty_snapshot_decoder_new_buf", unsafe {
            sys::ghostty_snapshot_decoder_new_buf(
                ptr::null(),
                &raw mut decoder,
                state.as_ptr(),
                state.len(),
            )
        })?;
        let limit = CONTINUATION_MAX_BYTES;
        let retain = true;
        let mut raw = ptr::null_mut();
        // SAFETY: the decoder is live and not yet started; options take their documented types; `raw` becomes ours.
        let code = unsafe {
            let set_limit = sys::ghostty_snapshot_decoder_set(
                decoder,
                sys::GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES,
                (&raw const limit).cast(),
            );
            let set_retain = sys::ghostty_snapshot_decoder_set(
                decoder,
                sys::GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION,
                (&raw const retain).cast(),
            );
            match (set_limit, set_retain) {
                (sys::GHOSTTY_SUCCESS, sys::GHOSTTY_SUCCESS) => {
                    sys::ghostty_snapshot_decoder_decode(decoder, &raw mut raw)
                }
                (sys::GHOSTTY_SUCCESS, other) | (other, _) => other,
            }
        };
        // SAFETY: the decoder is freed once; the terminal it produced stays valid.
        unsafe { sys::ghostty_snapshot_decoder_free(decoder) };
        if let Err(e) = check("ghostty_snapshot_decoder_decode", code) {
            tracing::warn!(pane_id, error = %e, "saved terminal state did not decode");
            return Err(e);
        }
        Self::configure(pane_id, Terminal::new(raw), scrollback_lines, palette)
    }

    fn configure(
        pane_id: u64,
        mut term: Terminal,
        scrollback_lines: u32,
        palette: &Palette,
    ) -> Result<Self> {
        let effects = NonNull::from(Box::leak(Box::new(Effects {
            pane_id,
            dark: palette.is_dark(),
            ..Effects::default()
        })));
        term.effects = Some(effects);
        let mut engine = Self {
            pane_id,
            cols: 0,
            rows: 0,
            cell_width: 0,
            cell_height: 0,
            scrollback_lines,
            option_as_alt: sys::GHOSTTY_OPTION_AS_ALT_FALSE,
            buttons: 0,
            generation: 0,
            row_generation: Vec::new(),
            scrollback_base: 0,
            anchor_y: 0,
            anchor: None,
            key: KeyEncoder::new()?,
            mouse: MouseEncoder::new()?,
            render: RenderState::new()?,
            effects,
            term,
            not_sync: PhantomData,
        };
        // SAFETY: the terminal is live and `effects` is owned by the engine until drop, after the terminal is freed.
        unsafe { effects::install(engine.term.raw, engine.effects.as_ptr()) }
            .map_err(|(call, code)| Error::Ghostty { call, code })?;
        engine.cols = engine.get_u16(sys::GHOSTTY_TERMINAL_DATA_COLS)?;
        engine.rows = engine.get_u16(sys::GHOSTTY_TERMINAL_DATA_ROWS)?;
        engine.sync_size_report();
        let lines = usize::try_from(scrollback_lines.saturating_add(SCROLLBACK_SLACK))
            .unwrap_or(usize::MAX);
        let continuation = CONTINUATION_MAX_BYTES;
        let clipboard_max = CLIPBOARD_WRITE_MAX_BYTES;
        let kitty_storage = 0u64;
        let glyph_protocol = false;
        let grapheme = sys::GhosttyTerminalModeConfig {
            mode: sys::GHOSTTY_MODE_GRAPHEME_CLUSTER,
            value: true,
        };
        let terminfo = sys::GhosttyString {
            ptr: TERMINFO_NAME.as_ptr(),
            len: TERMINFO_NAME.len(),
        };
        engine.set(sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, ptr::null())?;
        engine.set(
            sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
            (&raw const lines).cast(),
        )?;
        // Re-enabling tracking on a restored terminal whose parser is mid-sequence would drop that sequence.
        if engine.get_usize(sys::GHOSTTY_TERMINAL_DATA_CONTINUATION_MAX_BYTES) != continuation {
            engine.set(
                sys::GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES,
                (&raw const continuation).cast(),
            )?;
        }
        engine.set(
            sys::GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE_MAX_BYTES,
            (&raw const clipboard_max).cast(),
        )?;
        engine.set(
            sys::GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
            (&raw const kitty_storage).cast(),
        )?;
        engine.set(
            sys::GHOSTTY_TERMINAL_OPT_GLYPH_PROTOCOL,
            (&raw const glyph_protocol).cast(),
        )?;
        engine.set(
            sys::GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
            (&raw const grapheme).cast(),
        )?;
        engine.set(
            sys::GHOSTTY_TERMINAL_OPT_TERMINFO_NAME,
            (&raw const terminfo).cast(),
        )?;
        engine.set_palette(palette)?;
        engine.track_scrollback();
        Ok(engine)
    }

    /// The pane this engine belongs to, as given at creation (used in log fields).
    pub fn pane_id(&self) -> u64 {
        self.pane_id
    }

    /// Grid size as `(cols, rows)`, as last set by [`Engine::new`], [`Engine::restore`] or [`Engine::resize`].
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Cell size in pixels as `(width, height)`; `(0, 0)` until the first [`Engine::resize`] that sets it, and size queries stay unanswered until then.
    pub fn cell_size(&self) -> (u16, u16) {
        (self.cell_width, self.cell_height)
    }

    /// The configured scrollback cap in lines (the library keeps up to [`SCROLLBACK_SLACK`] more).
    pub fn scrollback_lines(&self) -> u32 {
        self.scrollback_lines
    }

    /// Feeds pty output (any split, any size); returns what it produced for the pty and the pane besides screen changes. Never fails.
    /// [`Engine::scrollback_base`] stays exact while one write adds fewer lines than the scrollback holds; past that it skips every line the terminal held before.
    pub fn write(&mut self, bytes: &[u8]) -> EngineOutput {
        // SAFETY: the terminal is live and no reference to the effects is held across the call.
        unsafe { sys::ghostty_terminal_vt_write(self.term.raw, bytes.as_ptr(), bytes.len()) };
        self.track_scrollback();
        self.drain()
    }

    /// Resizes to `cols` × `rows` cells of `cell_width_px` × `cell_height_px` (the primary screen reflows, DEC 2026 ends); the output may hold a mode 2048 report; errors: [`Error::InvalidSize`] for a zero size, [`Error::Ghostty`] if refused (the old size stays).
    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u16,
        cell_height_px: u16,
    ) -> Result<EngineOutput> {
        if cols == 0 || rows == 0 {
            return Err(Error::InvalidSize { cols, rows });
        }
        let before = (self.cols, self.rows, self.cell_width, self.cell_height);
        // The size callback answers the mode 2048 report the resize itself emits, so it must see the new size.
        (self.cols, self.rows, self.cell_width, self.cell_height) =
            (cols, rows, cell_width_px, cell_height_px);
        self.sync_size_report();
        // SAFETY: the terminal is live and no reference to the effects is held across the call.
        let code = unsafe {
            sys::ghostty_terminal_resize(
                self.term.raw,
                cols,
                rows,
                u32::from(cell_width_px),
                u32::from(cell_height_px),
            )
        };
        if let Err(e) = check("ghostty_terminal_resize", code) {
            tracing::warn!(pane_id = self.pane_id, cols, rows, error = %e, "resize refused; keeping the old size");
            (self.cols, self.rows, self.cell_width, self.cell_height) = before;
            self.sync_size_report();
            return Err(e);
        }
        self.track_scrollback();
        Ok(self.drain())
    }

    /// Sets the default colours the terminal answers OSC 4/10/11/12 and the colour-scheme query from (R-R4); OSC overrides a program made stay; errors: [`Error::Ghostty`] if refused.
    pub fn set_palette(&mut self, palette: &Palette) -> Result<()> {
        let rgb = |c: ply_proto::pane::Rgb| sys::GhosttyColorRgb {
            r: c.r,
            g: c.g,
            b: c.b,
        };
        let (fg, bg, cursor) = (rgb(palette.fg), rgb(palette.bg), rgb(palette.cursor));
        let table = palette.table().map(rgb);
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
            (&raw const fg).cast(),
        )?;
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
            (&raw const bg).cast(),
        )?;
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
            (&raw const cursor).cast(),
        )?;
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
            table.as_ptr().cast(),
        )?;
        self.effects_mut().dark = palette.is_dark();
        Ok(())
    }

    /// Which ⌥ keys encode as Alt/Meta for this pane's key events (spec K5); `Off` keeps ⌥ for layout characters.
    pub fn set_option_as_meta(&mut self, option: OptionAsMeta) {
        self.option_as_alt = match option {
            OptionAsMeta::Off => sys::GHOSTTY_OPTION_AS_ALT_FALSE,
            OptionAsMeta::Left => sys::GHOSTTY_OPTION_AS_ALT_LEFT,
            OptionAsMeta::Right => sys::GHOSTTY_OPTION_AS_ALT_RIGHT,
            OptionAsMeta::Both => sys::GHOSTTY_OPTION_AS_ALT_TRUE,
        };
    }

    /// The current title (OSC 0/2), lossy UTF-8; empty when none was set.
    pub fn title(&self) -> String {
        self.get_string(sys::GHOSTTY_TERMINAL_DATA_TITLE)
    }

    /// The current working directory as reported (OSC 7 raw URI); empty when none was set.
    pub fn pwd(&self) -> String {
        self.get_string(sys::GHOSTTY_TERMINAL_DATA_PWD)
    }

    /// The C2 display modes: alternate screen, DECTCEM, any mouse tracking, bracketed paste.
    pub fn modes(&self) -> Modes {
        let mut modes = Modes::empty();
        if self.alternate_screen() {
            modes = modes | Modes::ALT_SCREEN;
        }
        for (data, flag) in [
            (
                sys::GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE,
                Modes::CURSOR_VISIBLE,
            ),
            (
                sys::GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING,
                Modes::MOUSE_REPORTING,
            ),
        ] {
            if self.get_bool(data) {
                modes = modes | flag;
            }
        }
        if self.mode(sys::GHOSTTY_MODE_BRACKETED_PASTE) {
            modes = modes | Modes::BRACKETED_PASTE;
        }
        modes
    }

    fn alternate_screen(&self) -> bool {
        let mut screen = sys::GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        // SAFETY: the terminal is live; the out pointer is a GhosttyTerminalScreen.
        let code = unsafe {
            sys::ghostty_terminal_get(
                self.term.raw,
                sys::GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                (&raw mut screen).cast(),
            )
        };
        code == sys::GHOSTTY_SUCCESS && screen == sys::GHOSTTY_TERMINAL_SCREEN_ALTERNATE
    }

    /// Scrollback rows above the screen: absolute lines `scrollback_base..scrollback_base + scrollback_rows`; 0 on the alternate screen.
    pub fn scrollback_rows(&self) -> u32 {
        let mut rows = 0usize;
        // SAFETY: the terminal is live; the out pointer is a size_t.
        let code = unsafe {
            sys::ghostty_terminal_get(
                self.term.raw,
                sys::GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS,
                (&raw mut rows).cast(),
            )
        };
        if code != sys::GHOSTTY_SUCCESS {
            tracing::warn!(
                pane_id = self.pane_id,
                code,
                "libghostty-vt did not report the scrollback size"
            );
            return 0;
        }
        u32::try_from(rows).unwrap_or(u32::MAX)
    }

    /// Lines dropped from the top of the scrollback (pruned by the line cap or erased) since this engine was created: the absolute line of the oldest scrollback row (C2 `scrollback_base`); never decreases. See [`Engine::write`] for when it skips lines.
    pub fn scrollback_base(&self) -> u64 {
        self.scrollback_base
    }

    /// Adds the lines dropped since the last call to the base and moves the anchor back to the top of the live screen.
    fn track_scrollback(&mut self) {
        if let Some(anchor) = &self.anchor {
            let dropped = match anchor.screen_y(self.pane_id) {
                Some(y) => {
                    let dropped = self.anchor_y.saturating_sub(y);
                    self.anchor_y = y;
                    dropped
                }
                None => {
                    tracing::debug!(
                        pane_id = self.pane_id,
                        "the scrollback anchor was discarded; every line held before counts as dropped"
                    );
                    self.anchor = None;
                    self.anchor_y.saturating_add(u32::from(self.rows))
                }
            };
            self.scrollback_base = self.scrollback_base.saturating_add(u64::from(dropped));
        }
        // The anchor belongs to the screen it was set on; the alternate screen has no scrollback to count.
        if self.alternate_screen() {
            return;
        }
        let top = sys::GhosttyPoint {
            tag: sys::GHOSTTY_POINT_TAG_ACTIVE,
            value: sys::GhosttyPointValue {
                coordinate: sys::GhosttyPointCoordinate { x: 0, y: 0 },
            },
        };
        let code = match &self.anchor {
            // SAFETY: the reference and the terminal are live; the point is inside the active area.
            Some(anchor) => unsafe {
                sys::ghostty_tracked_grid_ref_set(anchor.raw, self.term.raw, top)
            },
            None => {
                let mut raw = ptr::null_mut();
                // SAFETY: the terminal is live; `raw` receives a new reference owned by the `Anchor` below.
                let code = unsafe {
                    sys::ghostty_terminal_grid_ref_track(self.term.raw, top, &raw mut raw)
                };
                if code == sys::GHOSTTY_SUCCESS && !raw.is_null() {
                    self.anchor = Some(Anchor { raw });
                }
                code
            }
        };
        if code != sys::GHOSTTY_SUCCESS {
            tracing::warn!(
                pane_id = self.pane_id,
                code,
                "libghostty-vt refused the scrollback anchor; dropped lines may go uncounted"
            );
            return;
        }
        if let Some(y) = self.anchor.as_ref().and_then(|a| a.screen_y(self.pane_id)) {
            self.anchor_y = y;
        }
    }

    /// True while the program holds a DEC 2026 synchronized update open; plyd sends no Delta then (R-R18).
    pub fn is_synchronized_update_open(&self) -> bool {
        self.mode(sys::GHOSTTY_MODE_SYNC_OUTPUT)
    }

    /// Ends an open DEC 2026 update, for plyd's 150 ms cap (R-R18); a no-op when none is open; errors: [`Error::Ghostty`] if refused.
    pub fn end_synchronized_update(&mut self) -> Result<()> {
        let off = sys::GhosttyTerminalModeConfig {
            mode: sys::GHOSTTY_MODE_SYNC_OUTPUT,
            value: false,
        };
        self.set(sys::GHOSTTY_TERMINAL_OPT_MODE, (&raw const off).cast())
    }

    /// A token that changes whenever compressible content changes; plyd restarts its idle timer when it moves (R-R22).
    pub fn compression_activity(&self) -> u64 {
        let mut token = 0u64;
        // SAFETY: the terminal is live; the out pointer is a u64.
        let code =
            unsafe { sys::ghostty_terminal_compression_activity(self.term.raw, &raw mut token) };
        if code != sys::GHOSTTY_SUCCESS {
            tracing::warn!(
                pane_id = self.pane_id,
                code,
                "libghostty-vt did not report compression activity"
            );
        }
        token
    }

    /// One bounded incremental compression step over idle scrollback (milliseconds at most); repeat while it returns [`Compression::Pending`]; errors: [`Error::Ghostty`].
    pub fn compress_idle(&mut self) -> Result<Compression> {
        let mut result = sys::GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED;
        // SAFETY: the terminal is live; the out pointer is a compression result.
        check("ghostty_terminal_compress", unsafe {
            sys::ghostty_terminal_compress(
                self.term.raw,
                sys::GHOSTTY_TERMINAL_COMPRESSION_MODE_INCREMENTAL,
                &raw mut result,
            )
        })?;
        Ok(match result {
            sys::GHOSTTY_TERMINAL_COMPRESSION_RESULT_PENDING => Compression::Pending,
            sys::GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE => Compression::Complete,
            _ => Compression::Unsupported,
        })
    }

    /// Every match of `needle` (ASCII letters case-insensitive, else byte-exact) on the active screen and its scrollback, newest first; empty for an empty needle; blocks for the scrollback's size; errors: [`Error::Ghostty`].
    pub fn search(&mut self, needle: &str) -> Result<Vec<SearchMatch>> {
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let mut search = ptr::null_mut();
        // SAFETY: the terminal is live; the search is freed below before the terminal can be.
        check("ghostty_search_new", unsafe {
            sys::ghostty_search_new(ptr::null(), &raw mut search, self.term.raw)
        })?;
        let result = self.run_search(search, needle);
        // SAFETY: the search was created above and is freed once.
        unsafe { sys::ghostty_search_free(search) };
        result
    }

    fn run_search(&mut self, search: sys::GhosttySearch, needle: &str) -> Result<Vec<SearchMatch>> {
        let text = sys::GhosttyString {
            ptr: needle.as_ptr(),
            len: needle.len(),
        };
        let never_scroll = sys::GHOSTTY_SEARCH_SCROLL_NONE;
        let mut buffer = sys::GhosttySelectionBuffer {
            ptr: ptr::null_mut(),
            cap: 0,
            len: 0,
        };
        // SAFETY: the search is live and bound to our terminal; option values have their documented types.
        unsafe {
            check(
                "ghostty_search_set(NEEDLE)",
                sys::ghostty_search_set(
                    search,
                    sys::GHOSTTY_SEARCH_OPT_NEEDLE,
                    (&raw const text).cast(),
                ),
            )?;
            check(
                "ghostty_search_set(SELECT_SCROLL)",
                sys::ghostty_search_set(
                    search,
                    sys::GHOSTTY_SEARCH_OPT_SELECT_SCROLL,
                    (&raw const never_scroll).cast(),
                ),
            )?;
            check("ghostty_search_run", sys::ghostty_search_run(search))?;
        }
        // SAFETY: a null buffer with capacity 0 asks for the size needed.
        let code = unsafe {
            sys::ghostty_search_get(
                search,
                sys::GHOSTTY_SEARCH_DATA_MATCHES,
                (&raw mut buffer).cast(),
            )
        };
        if code != sys::GHOSTTY_OUT_OF_SPACE {
            check("ghostty_search_get(MATCHES)", code)?;
            return Ok(Vec::new());
        }
        let empty_ref = sys::GhosttyGridRef {
            size: size_of::<sys::GhosttyGridRef>(),
            node: ptr::null_mut(),
            x: 0,
            y: 0,
        };
        let mut selections = vec![
            sys::GhosttySelection {
                size: size_of::<sys::GhosttySelection>(),
                start: empty_ref,
                end: empty_ref,
                rectangle: false,
            };
            buffer.len
        ];
        buffer.ptr = selections.as_mut_ptr();
        buffer.cap = selections.len();
        // SAFETY: the buffer now has `cap` sized selection slots.
        check("ghostty_search_get(MATCHES)", unsafe {
            sys::ghostty_search_get(
                search,
                sys::GHOSTTY_SEARCH_DATA_MATCHES,
                (&raw mut buffer).cast(),
            )
        })?;
        selections.truncate(buffer.len);
        let base = self.scrollback_base;
        let mut matches = Vec::with_capacity(selections.len());
        for selection in &selections {
            let (Some(start), Some(end)) = (
                self.screen_point(&selection.start),
                self.screen_point(&selection.end),
            ) else {
                continue;
            };
            matches.push(SearchMatch {
                start_line: base + u64::from(start.y),
                start_col: start.x,
                end_line: base + u64::from(end.y),
                end_col: end.x,
            });
        }
        Ok(matches)
    }

    fn screen_point(&self, grid_ref: &sys::GhosttyGridRef) -> Option<sys::GhosttyPointCoordinate> {
        let mut point = sys::GhosttyPointCoordinate::default();
        // SAFETY: the grid reference came from the search just run, before any mutating call.
        let code = unsafe {
            sys::ghostty_terminal_point_from_grid_ref(
                self.term.raw,
                grid_ref,
                sys::GHOSTTY_POINT_TAG_SCREEN,
                &raw mut point,
            )
        };
        (code == sys::GHOSTTY_SUCCESS).then_some(point)
    }

    /// The terminal's full state (screen, scrollback, modes, title, colours, unfinished sequence) for [`Engine::restore`], valid only for the same libghostty-vt pin (ADR-0005); errors: [`Error::Ghostty`].
    pub fn save(&mut self) -> Result<Vec<u8>> {
        let (mut data, mut len) = (ptr::null_mut(), 0usize);
        // SAFETY: the terminal is live; on success the library hands us `len` bytes at `data` to free with ghostty_free.
        check("ghostty_snapshot_encode_alloc", unsafe {
            sys::ghostty_snapshot_encode_alloc(
                self.term.raw,
                ptr::null(),
                &raw mut data,
                &raw mut len,
            )
        })?;
        if data.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: `data` holds `len` initialised bytes owned by us until freed right after the copy.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
        // SAFETY: frees the allocation the library made for this call, once.
        unsafe { sys::ghostty_free(ptr::null(), data, len) };
        Ok(bytes)
    }

    /// The whole screen as a C2 Snapshot with its own complete style table (seq 1), for one-off readers; attached clients use their [`DeltaBuilder`]; errors: [`Error::Ghostty`].
    pub fn snapshot(&mut self) -> Result<Snapshot> {
        DeltaBuilder::new().snapshot(self)
    }

    /// Scrollback lines `start..start + count` (absolute, clipped to what exists and to one frame) with their own style table, for one-off readers; errors: [`Error::Ghostty`].
    pub fn scroll_history(&mut self, start: u64, count: u16) -> Result<History> {
        DeltaBuilder::new().history(self, start, count)
    }

    pub(crate) fn refresh(&mut self) -> Result<()> {
        let dirty = self.render.update(self.term.raw)?;
        let (_, rows) = self.render.size()?;
        if self.row_generation.len() != usize::from(rows) {
            self.generation += 1;
            self.row_generation = vec![self.generation; usize::from(rows)];
        }
        match dirty {
            Dirty::Clean => {}
            Dirty::Full => {
                self.generation += 1;
                self.row_generation.fill(self.generation);
            }
            Dirty::Partial => {
                self.generation += 1;
                let (generation, rows) = (self.generation, &mut self.row_generation);
                self.render.dirty_rows(|y| {
                    if let Some(slot) = rows.get_mut(usize::from(y)) {
                        *slot = generation;
                    }
                })?;
            }
        }
        self.render.clean()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn row_generations(&self) -> &[u64] {
        &self.row_generation
    }

    pub(crate) fn render_size(&self) -> Result<(u16, u16)> {
        self.render.size()
    }

    pub(crate) fn cursor(&self) -> Result<Cursor> {
        self.render.cursor()
    }

    pub(crate) fn read_rows<S: RowSink>(
        &mut self,
        want: impl Fn(u16) -> bool,
        sink: &mut S,
    ) -> Result<()> {
        self.render.read_rows(self.pane_id, want, sink)
    }

    /// Feeds scrollback rows `lo..hi` (indexes relative to the live screen, -1 the newest, already clipped) into `sink`; stops early when `keep_going` says so.
    pub(crate) fn read_history<S: RowSink>(
        &mut self,
        lo: i64,
        hi: i64,
        sink: &mut S,
        mut keep_going: impl FnMut(&S) -> bool,
    ) -> Result<()> {
        let scrollback = i64::from(self.scrollback_rows());
        let mut graphemes = vec![0u32; 8];
        for index in lo..hi {
            if !keep_going(sink) {
                break;
            }
            let (Ok(row_index), Ok(y)) = (i32::try_from(index), u32::try_from(scrollback + index))
            else {
                continue;
            };
            let mut wrapped = false;
            let mut header: sys::GhosttyRow = 0;
            let code = match self.history_ref(0, y) {
                // SAFETY: the reference is fresh; out pointers have their documented types.
                Some(first) => unsafe {
                    match sys::ghostty_grid_ref_row(&first, &raw mut header) {
                        sys::GHOSTTY_SUCCESS => sys::ghostty_row_get(
                            header,
                            sys::GHOSTTY_ROW_DATA_WRAP,
                            (&raw mut wrapped).cast(),
                        ),
                        other => other,
                    }
                },
                None => sys::GHOSTTY_INVALID_VALUE,
            };
            if code != sys::GHOSTTY_SUCCESS {
                tracing::warn!(
                    pane_id = self.pane_id,
                    row = index,
                    code,
                    "libghostty-vt could not read a history row's wrap flag; treated as unwrapped"
                );
                wrapped = false;
            }
            sink.begin_row(row_index, wrapped);
            let mut refused = RefusedReads::default();
            for x in 0..self.cols {
                let Some(cell_ref) = self.history_ref(x, y) else {
                    refused.note(CellReadError {
                        data: 0,
                        code: sys::GHOSTTY_INVALID_VALUE,
                    });
                    sink.cell(0, &Style::default(), CellFlags::empty(), &[]);
                    continue;
                };
                let mut raw: sys::GhosttyCell = 0;
                // SAFETY: the reference is fresh; the out pointer is a GhosttyCell.
                check("ghostty_grid_ref_cell", unsafe {
                    sys::ghostty_grid_ref_cell(&cell_ref, &raw mut raw)
                })?;
                let cell = match RawCell::decode(raw) {
                    Ok(cell) if !cell.is_blank() => cell,
                    Ok(_) => {
                        sink.cell(0, &Style::default(), CellFlags::empty(), &[]);
                        continue;
                    }
                    Err(e) => {
                        refused.note(e);
                        sink.cell(0, &Style::default(), CellFlags::empty(), &[]);
                        continue;
                    }
                };
                let mut style = Style::default();
                if cell.styled {
                    let mut raw_style = cells::empty_style();
                    // SAFETY: the reference is fresh; `raw_style` is a sized GhosttyStyle.
                    check("ghostty_grid_ref_style", unsafe {
                        sys::ghostty_grid_ref_style(&cell_ref, &raw mut raw_style)
                    })?;
                    style = cells::style(&raw_style);
                }
                match cell.tag_background(raw) {
                    Ok(Some(bg)) => style.bg = bg,
                    Ok(None) => {}
                    Err(e) => refused.note(e),
                }
                let mut flags = cell.flags();
                let mut extra: &[u32] = &[];
                if cell.has_graphemes() {
                    let mut len = 0usize;
                    // SAFETY: the buffer has `graphemes.len()` slots; OUT_OF_SPACE reports the count needed.
                    let mut code = unsafe {
                        sys::ghostty_grid_ref_graphemes(
                            &cell_ref,
                            graphemes.as_mut_ptr(),
                            graphemes.len(),
                            &raw mut len,
                        )
                    };
                    if code == sys::GHOSTTY_OUT_OF_SPACE {
                        graphemes.resize(len, 0);
                        // SAFETY: as above, with a buffer of the reported size.
                        code = unsafe {
                            sys::ghostty_grid_ref_graphemes(
                                &cell_ref,
                                graphemes.as_mut_ptr(),
                                graphemes.len(),
                                &raw mut len,
                            )
                        };
                    }
                    check("ghostty_grid_ref_graphemes", code)?;
                    let len = len.min(graphemes.len());
                    if len > 1 {
                        extra = &graphemes[1..len];
                        flags = flags | CellFlags::GRAPHEME;
                    }
                }
                sink.cell(cell.codepoint, &style, flags, extra);
            }
            refused.log(self.pane_id, index, "history");
            sink.end_row();
        }
        Ok(())
    }

    fn history_ref(&self, x: u16, y: u32) -> Option<sys::GhosttyGridRef> {
        let mut out = sys::GhosttyGridRef {
            size: size_of::<sys::GhosttyGridRef>(),
            node: ptr::null_mut(),
            x: 0,
            y: 0,
        };
        let point = sys::GhosttyPoint {
            tag: sys::GHOSTTY_POINT_TAG_HISTORY,
            value: sys::GhosttyPointValue {
                coordinate: sys::GhosttyPointCoordinate { x, y },
            },
        };
        // SAFETY: the terminal is live; `out` is a sized grid reference.
        let code = unsafe { sys::ghostty_terminal_grid_ref(self.term.raw, point, &raw mut out) };
        (code == sys::GHOSTTY_SUCCESS).then_some(out)
    }

    pub(crate) fn kitty_keyboard_flags(&self) -> u8 {
        let mut flags = 0u8;
        // SAFETY: the terminal is live; the out pointer is a u8.
        let code = unsafe {
            sys::ghostty_terminal_get(
                self.term.raw,
                sys::GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
                (&raw mut flags).cast(),
            )
        };
        if code != sys::GHOSTTY_SUCCESS {
            tracing::warn!(
                pane_id = self.pane_id,
                code,
                "libghostty-vt did not report the kitty keyboard flags"
            );
        }
        flags
    }

    pub(crate) fn focus_reporting(&self) -> bool {
        self.mode(sys::GHOSTTY_MODE_FOCUS_EVENT)
    }

    pub(crate) fn encode_key(&mut self, input: &KeyInput<'_>) -> Result<Vec<u8>> {
        self.key.encode(self.term.raw, self.option_as_alt, input)
    }

    pub(crate) fn encode_mouse(&mut self, input: &MouseInput) -> Result<Vec<u8>> {
        let surface = Surface {
            cols: self.cols,
            rows: self.rows,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        };
        self.mouse.encode(self.term.raw, surface, input)
    }

    pub(crate) fn pressed_buttons(&mut self) -> &mut u16 {
        &mut self.buttons
    }

    /// Forgets every held mouse button and the last reported cell; plyd calls it when a client detaches or the view loses focus, so a release seen elsewhere does not leave a drag stuck.
    pub fn reset_mouse_buttons(&mut self) {
        self.buttons = 0;
        self.mouse.reset();
    }

    /// The active screen and its scrollback as plain text from libghostty-vt's own formatter (trailing whitespace trimmed, soft wraps kept as line breaks), an oracle independent of the C2 path; errors: [`Error::Ghostty`] when the formatter fails.
    pub fn plain_text(&mut self) -> Result<String> {
        let options = sys::GhosttyFormatterTerminalOptions {
            size: size_of::<sys::GhosttyFormatterTerminalOptions>(),
            emit: sys::GHOSTTY_FORMATTER_FORMAT_PLAIN,
            unwrap: false,
            trim: true,
            extra: sys::GhosttyFormatterTerminalExtra {
                size: size_of::<sys::GhosttyFormatterTerminalExtra>(),
                screen: sys::GhosttyFormatterScreenExtra {
                    size: size_of::<sys::GhosttyFormatterScreenExtra>(),
                    ..Default::default()
                },
                ..Default::default()
            },
            selection: ptr::null(),
        };
        let mut formatter = ptr::null_mut();
        // SAFETY: the terminal is live; the formatter is freed below, before the terminal can be.
        check("ghostty_formatter_terminal_new", unsafe {
            sys::ghostty_formatter_terminal_new(
                ptr::null(),
                &raw mut formatter,
                self.term.raw,
                options,
            )
        })?;
        let (mut data, mut len) = (ptr::null_mut(), 0usize);
        // SAFETY: the formatter is live and the terminal does not change during the call.
        let code = unsafe {
            sys::ghostty_formatter_format_alloc(formatter, ptr::null(), &raw mut data, &raw mut len)
        };
        // SAFETY: the formatter was created above and is freed once.
        unsafe { sys::ghostty_formatter_free(formatter) };
        check("ghostty_formatter_format_alloc", code)?;
        if data.is_null() {
            return Ok(String::new());
        }
        // SAFETY: `data` holds `len` initialised bytes owned by us until freed right after the copy.
        let text =
            String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(data, len) }).into_owned();
        // SAFETY: frees the allocation the library made for this call, once.
        unsafe { sys::ghostty_free(ptr::null(), data, len) };
        Ok(text)
    }

    pub(crate) fn encode_focus(&self, gained: bool) -> Result<Vec<u8>> {
        encoders::focus(gained)
    }

    /// Pastes `text`; on [`PasteResult::Written`] the returned bytes are what to write to the pty.
    pub(crate) fn paste(
        &mut self,
        text: &str,
        allow_unsafe: bool,
    ) -> Result<(PasteResult, Vec<u8>)> {
        let before = self.effects_mut().reply.len();
        let result = encoders::paste(self.term.raw, text, allow_unsafe)?;
        let bytes = self.effects_mut().reply.split_off(before);
        Ok((result, bytes))
    }

    fn drain(&mut self) -> EngineOutput {
        let fx = self.effects_mut();
        let mut out = EngineOutput {
            reply: std::mem::take(&mut fx.reply),
            bells: std::mem::take(&mut fx.bells),
            title: None,
            pwd: None,
            notifications: std::mem::take(&mut fx.notifications),
            progress: fx.progress.take(),
            clipboard_writes: std::mem::take(&mut fx.clipboard_writes),
        };
        let (title_changed, pwd_changed) = (
            std::mem::take(&mut fx.title_changed),
            std::mem::take(&mut fx.pwd_changed),
        );
        if title_changed {
            out.title = Some(self.title());
        }
        if pwd_changed {
            out.pwd = Some(self.pwd());
        }
        out
    }

    fn effects_mut(&mut self) -> &mut Effects {
        // SAFETY: the effects allocation is owned by this engine; no libghostty-vt call runs while `&mut self` is used here.
        unsafe { self.effects.as_mut() }
    }

    fn sync_size_report(&mut self) {
        let size = sys::GhosttySizeReportSize {
            rows: self.rows,
            columns: self.cols,
            cell_width: u32::from(self.cell_width),
            cell_height: u32::from(self.cell_height),
        };
        self.effects_mut().size = size;
    }

    fn set(&mut self, option: sys::GhosttyTerminalOption, value: *const c_void) -> Result<()> {
        // SAFETY: the terminal is live; callers pass a value of the type `option` documents, alive for the call.
        let code = unsafe { sys::ghostty_terminal_set(self.term.raw, option, value) };
        if code != sys::GHOSTTY_SUCCESS {
            tracing::warn!(
                pane_id = self.pane_id,
                option,
                code,
                "libghostty-vt refused a terminal option"
            );
        }
        check("ghostty_terminal_set", code)
    }

    fn mode(&self, mode: sys::GhosttyMode) -> bool {
        let mut config = sys::GhosttyTerminalModeConfig { mode, value: false };
        // SAFETY: the terminal is live; `config` names the mode to read.
        let code = unsafe {
            sys::ghostty_terminal_get(
                self.term.raw,
                sys::GHOSTTY_TERMINAL_DATA_MODE,
                (&raw mut config).cast(),
            )
        };
        if code != sys::GHOSTTY_SUCCESS {
            tracing::warn!(
                pane_id = self.pane_id,
                mode,
                code,
                "libghostty-vt did not report a mode"
            );
        }
        config.value
    }

    fn get_bool(&self, data: sys::GhosttyTerminalData) -> bool {
        let mut value = false;
        // SAFETY: the terminal is live; callers pass a data kind whose out type is bool.
        let code =
            unsafe { sys::ghostty_terminal_get(self.term.raw, data, (&raw mut value).cast()) };
        if code != sys::GHOSTTY_SUCCESS {
            tracing::warn!(
                pane_id = self.pane_id,
                data,
                code,
                "libghostty-vt did not report a flag"
            );
        }
        value
    }

    fn get_usize(&self, data: sys::GhosttyTerminalData) -> usize {
        let mut value = 0usize;
        // SAFETY: the terminal is live; callers pass a data kind whose out type is size_t.
        let code =
            unsafe { sys::ghostty_terminal_get(self.term.raw, data, (&raw mut value).cast()) };
        if code != sys::GHOSTTY_SUCCESS {
            tracing::warn!(
                pane_id = self.pane_id,
                data,
                code,
                "libghostty-vt did not report a size"
            );
        }
        value
    }

    fn get_u16(&self, data: sys::GhosttyTerminalData) -> Result<u16> {
        let mut value = 0u16;
        // SAFETY: the terminal is live; callers pass a data kind whose out type is uint16_t.
        check("ghostty_terminal_get", unsafe {
            sys::ghostty_terminal_get(self.term.raw, data, (&raw mut value).cast())
        })?;
        Ok(value)
    }

    fn get_string(&self, data: sys::GhosttyTerminalData) -> String {
        let mut s = sys::GhosttyString {
            ptr: ptr::null(),
            len: 0,
        };
        // SAFETY: the terminal is live; the out pointer is a GhosttyString.
        let code = unsafe { sys::ghostty_terminal_get(self.term.raw, data, (&raw mut s).cast()) };
        if code != sys::GHOSTTY_SUCCESS {
            tracing::warn!(
                pane_id = self.pane_id,
                data,
                code,
                "libghostty-vt did not report a string"
            );
            return String::new();
        }
        if s.ptr.is_null() || s.len == 0 {
            return String::new();
        }
        // SAFETY: the string is borrowed until the next mutating call; it is copied before returning.
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(s.ptr, s.len) }).into_owned()
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("pane_id", &self.pane_id)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("cell_size", &(self.cell_width, self.cell_height))
            .finish_non_exhaustive()
    }
}

pub(crate) fn check(call: &'static str, code: sys::GhosttyResult) -> Result<()> {
    if code == sys::GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(Error::Ghostty { call, code })
    }
}

fn install_log_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: `log_line` matches GhosttySysLogFn and lives for the whole process.
        let code = unsafe {
            sys::ghostty_sys_set(
                sys::GHOSTTY_SYS_OPT_LOG,
                (log_line as sys::GhosttySysLogFn as *const c_void).cast(),
            )
        };
        if code != sys::GHOSTTY_SUCCESS {
            tracing::warn!(code, "libghostty-vt refused the log hook; its log is lost");
        }
    });
}

unsafe extern "C" fn log_line(
    _userdata: *mut c_void,
    level: sys::GhosttySysLogLevel,
    scope: *const u8,
    scope_len: usize,
    message: *const u8,
    message_len: usize,
) {
    let text = |p: *const u8, n: usize| {
        if p.is_null() || n == 0 {
            String::new()
        } else {
            // SAFETY: the library passes `n` readable bytes at `p` for this call.
            String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, n) }).into_owned()
        }
    };
    let (scope, message) = (text(scope, scope_len), text(message, message_len));
    match level {
        0 => tracing::error!(target: "libghostty_vt", %scope, "{message}"),
        1 => tracing::warn!(target: "libghostty_vt", %scope, "{message}"),
        2 => tracing::info!(target: "libghostty_vt", %scope, "{message}"),
        _ => tracing::debug!(target: "libghostty_vt", %scope, "{message}"),
    }
}
