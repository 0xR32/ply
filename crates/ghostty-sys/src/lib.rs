//! Raw bindings to libghostty-vt, Ghostty's terminal engine as a C library (ghostty 44f2a44, spec 2).
//!
//! This is the only crate in ply with FFI to libghostty-vt (spec 3.2, INV-17). `build.rs` runs
//! `zig build -Demit-lib-vt` on `vendor/libghostty-vt` offline (`--system` with a package directory, prefix and caches
//! under `OUT_DIR`, never writing into the vendor tree; ADR-0005 Decision 1) and links `libghostty-vt.a` statically.
//! This file declares, by hand, the part of the C API ply uses from `vt/terminal.h`, `render.h`, `screen.h`,
//! `style.h`, `modes.h`, `device.h`, `size_report.h`, `snapshot.h`, `key.h`, `mouse.h`, `focus.h`, `paste.h`,
//! `search.h`, `grid_ref.h`, `point.h`, `sys.h` and `types.h` (spec 12, ADR-0005 spec delta 11), plus the types those
//! headers take from `allocator.h`, `color.h`, `io.h` and `selection.h`.
//!
//! It holds no logic and depends on no ply crate (spec 3.2, 8.2). Only `ply-term` uses it, and only through its
//! `engine` feature, which only `ply-daemon` enables; everything else in ply sees the safe `ply_term::Engine`.
//!
//! Every enum is a C `int` (`types.h`); every sized struct starts with `size: usize`, which the caller sets to the
//! struct's size. The layouts and constants here are checked against the library's own ABI manifest
//! ([`ghostty_type_json`]) by `tests/layout.rs`; an upgrade of the pin reruns that test (ADR-0008 Decision 5).
//! None of these functions is thread-safe for one handle: the caller serializes every call on a terminal and the
//! objects created from it.

#![allow(non_camel_case_types, non_upper_case_globals)]

use std::ffi::{c_char, c_int, c_void};

/// Result code of most calls (`types.h`); [`GHOSTTY_SUCCESS`] or a negative error.
pub type GhosttyResult = c_int;
/// The call succeeded.
pub const GHOSTTY_SUCCESS: GhosttyResult = 0;
/// An allocation failed.
pub const GHOSTTY_OUT_OF_MEMORY: GhosttyResult = -1;
/// A handle, pointer or option value was invalid.
pub const GHOSTTY_INVALID_VALUE: GhosttyResult = -2;
/// The output buffer was too small; the length out-parameter holds the size needed.
pub const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;
/// The requested value does not exist (for example no selected search match).
pub const GHOSTTY_NO_VALUE: GhosttyResult = -4;
/// A reader or writer callback failed.
pub const GHOSTTY_IO_ERROR: GhosttyResult = -5;
/// A configured limit was exceeded.
pub const GHOSTTY_LIMIT_EXCEEDED: GhosttyResult = -6;
/// The request was refused as unsafe (paste text that could inject commands).
pub const GHOSTTY_REJECTED: GhosttyResult = -7;

/// Opaque allocator (`allocator.h`); ply always passes a null pointer, which selects libghostty-vt's default.
#[repr(C)]
pub struct GhosttyAllocator {
    _private: [u8; 0],
}

macro_rules! opaque_handle {
    ($($(#[$doc:meta])* $name:ident, $impl:ident;)*) => {$(
        #[doc(hidden)]
        #[repr(C)]
        pub struct $impl {
            _private: [u8; 0],
        }
        $(#[$doc])*
        pub type $name = *mut $impl;
    )*};
}

opaque_handle! {
    /// A terminal (`types.h`): screen, scrollback, modes and parser state. Freed with [`ghostty_terminal_free`].
    GhosttyTerminal, GhosttyTerminalImpl;
    /// A render state (`render.h`) that copies a terminal's viewport and consumes its dirty flags on update.
    GhosttyRenderState, GhosttyRenderStateImpl;
    /// Iterates the rows of a render state; filled by [`ghostty_render_state_get`] with `ROW_ITERATOR`.
    GhosttyRenderStateRowIterator, GhosttyRenderStateRowIteratorImpl;
    /// Iterates the cells of the current row; filled by [`ghostty_render_state_row_get`] with `CELLS`.
    GhosttyRenderStateRowCells, GhosttyRenderStateRowCellsImpl;
    /// Decodes a snapshot produced by [`ghostty_snapshot_encode_alloc`] into a new terminal.
    GhosttySnapshotDecoder, GhosttySnapshotDecoderImpl;
    /// A scrollback search bound to one terminal (`search.h`).
    GhosttySearch, GhosttySearchImpl;
    /// Key encoder (`key/encoder.h`).
    GhosttyKeyEncoder, GhosttyKeyEncoderImpl;
    /// Key event fed to a key encoder (`key/event.h`).
    GhosttyKeyEvent, GhosttyKeyEventImpl;
    /// Mouse encoder (`mouse/encoder.h`).
    GhosttyMouseEncoder, GhosttyMouseEncoderImpl;
    /// Mouse event fed to a mouse encoder (`mouse/event.h`).
    GhosttyMouseEvent, GhosttyMouseEventImpl;
}

/// A packed 8-byte grid cell (`screen.h`); read it only through [`ghostty_cell_get`].
pub type GhosttyCell = u64;
/// A packed 8-byte row header (`screen.h`); read it only through [`ghostty_row_get`].
pub type GhosttyRow = u64;
/// Modifier bitmask (`key/event.h`), bit-identical to `ply_proto::data::Mods`.
pub type GhosttyMods = u16;
/// A terminal mode packed by [`ghostty_mode`] (`modes.h`).
pub type GhosttyMode = u16;

/// A borrowed byte string (`types.h`); `ptr` may be null when `len` is 0.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyString {
    /// First byte.
    pub ptr: *const u8,
    /// Length in bytes.
    pub len: usize,
}

/// An sRGB colour (`color.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct GhosttyColorRgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

/// Tag of [`GhosttyStyleColor`] (`style.h`).
pub type GhosttyStyleColorTag = c_int;
/// No colour set: the default for its slot.
pub const GHOSTTY_STYLE_COLOR_NONE: GhosttyStyleColorTag = 0;
/// A 256-colour palette index in `value.palette`.
pub const GHOSTTY_STYLE_COLOR_PALETTE: GhosttyStyleColorTag = 1;
/// A true colour in `value.rgb`.
pub const GHOSTTY_STYLE_COLOR_RGB: GhosttyStyleColorTag = 2;

/// Payload of [`GhosttyStyleColor`], selected by its tag.
#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyStyleColorValue {
    /// Palette index (tag PALETTE).
    pub palette: u8,
    /// True colour (tag RGB).
    pub rgb: GhosttyColorRgb,
    /// Keeps the union 8 bytes wide.
    pub _padding: u64,
}

/// A colour in a style: none, a palette index or RGB; never resolved against the palette.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyleColor {
    /// Which member of `value` is valid.
    pub tag: GhosttyStyleColorTag,
    /// The colour.
    pub value: GhosttyStyleColorValue,
}

/// SGR underline kind (`sgr.h` `GhosttySgrUnderline`): 0 none, 1 single, 2 double, 3 curly, 4 dotted, 5 dashed.
pub type GhosttySgrUnderline = c_int;

/// A cell's style (`style.h`), a sized struct.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyle {
    /// `size_of::<GhosttyStyle>()`.
    pub size: usize,
    /// Foreground.
    pub fg_color: GhosttyStyleColor,
    /// Background.
    pub bg_color: GhosttyStyleColor,
    /// Underline colour.
    pub underline_color: GhosttyStyleColor,
    /// SGR 1.
    pub bold: bool,
    /// SGR 3.
    pub italic: bool,
    /// SGR 2.
    pub faint: bool,
    /// SGR 5.
    pub blink: bool,
    /// SGR 7.
    pub inverse: bool,
    /// SGR 8.
    pub invisible: bool,
    /// SGR 9.
    pub strikethrough: bool,
    /// SGR 53.
    pub overline: bool,
    /// Underline kind, 0–5.
    pub underline: GhosttySgrUnderline,
}

/// `GhosttyTerminalOption` (`terminal.h`): what [`ghostty_terminal_set`] configures.
pub type GhosttyTerminalOption = c_int;
/// `void*` passed back to every callback.
pub const GHOSTTY_TERMINAL_OPT_USERDATA: GhosttyTerminalOption = 0;
/// [`GhosttyTerminalWritePtyFn`]: query answers and reports for the pty.
pub const GHOSTTY_TERMINAL_OPT_WRITE_PTY: GhosttyTerminalOption = 1;
/// [`GhosttyTerminalBellFn`]: BEL.
pub const GHOSTTY_TERMINAL_OPT_BELL: GhosttyTerminalOption = 2;
/// [`GhosttyTerminalXtversionFn`]: the XTVERSION (`CSI > q`) answer.
pub const GHOSTTY_TERMINAL_OPT_XTVERSION: GhosttyTerminalOption = 4;
/// [`GhosttyTerminalTitleChangedFn`]: OSC 0/2.
pub const GHOSTTY_TERMINAL_OPT_TITLE_CHANGED: GhosttyTerminalOption = 5;
/// [`GhosttyTerminalSizeFn`]: CSI 14/16/18 t and mode 2048 reports.
pub const GHOSTTY_TERMINAL_OPT_SIZE: GhosttyTerminalOption = 6;
/// [`GhosttyTerminalColorSchemeFn`]: `CSI ? 996 n`.
pub const GHOSTTY_TERMINAL_OPT_COLOR_SCHEME: GhosttyTerminalOption = 7;
/// `GhosttyColorRgb*`: default foreground (answers OSC 10); null unsets.
pub const GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND: GhosttyTerminalOption = 11;
/// `GhosttyColorRgb*`: default background (answers OSC 11).
pub const GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND: GhosttyTerminalOption = 12;
/// `GhosttyColorRgb*`: default cursor colour (answers OSC 12).
pub const GHOSTTY_TERMINAL_OPT_COLOR_CURSOR: GhosttyTerminalOption = 13;
/// `GhosttyColorRgb[256]*`: default palette (answers OSC 4).
pub const GHOSTTY_TERMINAL_OPT_COLOR_PALETTE: GhosttyTerminalOption = 14;
/// `uint64_t*`: bytes of Kitty images kept; 0 disables Kitty graphics storage.
pub const GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT: GhosttyTerminalOption = 15;
/// `bool*`: Glyph Protocol APC handling on or off.
pub const GHOSTTY_TERMINAL_OPT_GLYPH_PROTOCOL: GhosttyTerminalOption = 24;
/// [`GhosttyTerminalPwdChangedFn`]: OSC 7, OSC 9;9, OSC 1337 CurrentDir.
pub const GHOSTTY_TERMINAL_OPT_PWD_CHANGED: GhosttyTerminalOption = 25;
/// [`GhosttyTerminalClipboardWriteFn`]: OSC 52 writes.
pub const GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE: GhosttyTerminalOption = 26;
/// `size_t*`: scrollback byte cap; null removes it (the default is 10 000 bytes).
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES: GhosttyTerminalOption = 27;
/// `size_t*`: scrollback line cap, pruned page by page; null removes it.
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES: GhosttyTerminalOption = 28;
/// [`GhosttyTerminalDesktopNotificationFn`]: OSC 9 and OSC 777.
pub const GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION: GhosttyTerminalOption = 29;
/// [`GhosttyTerminalProgressReportFn`]: OSC 9;4.
pub const GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT: GhosttyTerminalOption = 30;
/// `size_t*`: bytes of an unfinished sequence kept for snapshots; 0 or null disables tracking.
pub const GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES: GhosttyTerminalOption = 31;
/// [`GhosttyTerminalModeConfig`]`*`: a mode's current value and its reset (RIS) default.
pub const GHOSTTY_TERMINAL_OPT_MODE_DEFAULT: GhosttyTerminalOption = 33;
/// [`GhosttyTerminalModeConfig`]`*`: a mode's current value only.
pub const GHOSTTY_TERMINAL_OPT_MODE: GhosttyTerminalOption = 34;
/// [`GhosttyString`]`*`: the name XTGETTCAP `TN` reports.
pub const GHOSTTY_TERMINAL_OPT_TERMINFO_NAME: GhosttyTerminalOption = 37;

/// `GhosttyTerminalData` (`terminal.h`): what [`ghostty_terminal_get`] reads.
pub type GhosttyTerminalData = c_int;
/// `uint16_t*`: columns.
pub const GHOSTTY_TERMINAL_DATA_COLS: GhosttyTerminalData = 1;
/// `uint16_t*`: rows.
pub const GHOSTTY_TERMINAL_DATA_ROWS: GhosttyTerminalData = 2;
/// [`GhosttyTerminalScreen`]`*`: primary or alternate.
pub const GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN: GhosttyTerminalData = 6;
/// `bool*`: DECTCEM (mode 25).
pub const GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE: GhosttyTerminalData = 7;
/// `uint8_t*`: Kitty keyboard protocol flags; 0 when the protocol is off.
pub const GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS: GhosttyTerminalData = 8;
/// `bool*`: any mouse tracking mode (X10, normal, button, any) is on.
pub const GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING: GhosttyTerminalData = 11;
/// [`GhosttyString`]`*`: the title, borrowed until the next mutating call.
pub const GHOSTTY_TERMINAL_DATA_TITLE: GhosttyTerminalData = 12;
/// [`GhosttyString`]`*`: the pwd as reported (a raw URI for OSC 7), borrowed until the next mutating call.
pub const GHOSTTY_TERMINAL_DATA_PWD: GhosttyTerminalData = 13;
/// `size_t*`: rows of the active screen including scrollback.
pub const GHOSTTY_TERMINAL_DATA_TOTAL_ROWS: GhosttyTerminalData = 14;
/// `size_t*`: scrollback rows (total minus viewport).
pub const GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS: GhosttyTerminalData = 15;
/// `size_t*`: the continuation tracking limit; 0 when tracking is off.
pub const GHOSTTY_TERMINAL_DATA_CONTINUATION_MAX_BYTES: GhosttyTerminalData = 36;
/// [`GhosttyTerminalModeConfig`]`*` with `mode` set on input: the mode's current value.
pub const GHOSTTY_TERMINAL_DATA_MODE: GhosttyTerminalData = 37;

/// `GhosttyTerminalScreen` (`terminal.h`).
pub type GhosttyTerminalScreen = c_int;
/// The primary screen (has scrollback).
pub const GHOSTTY_TERMINAL_SCREEN_PRIMARY: GhosttyTerminalScreen = 0;
/// The alternate screen (no scrollback).
pub const GHOSTTY_TERMINAL_SCREEN_ALTERNATE: GhosttyTerminalScreen = 1;

/// `GhosttyTerminalCompressionMode` (`terminal.h`).
pub type GhosttyTerminalCompressionMode = c_int;
/// One bounded step; repeat while the result is PENDING.
pub const GHOSTTY_TERMINAL_COMPRESSION_MODE_INCREMENTAL: GhosttyTerminalCompressionMode = 0;
/// `GhosttyTerminalCompressionResult` (`terminal.h`).
pub type GhosttyTerminalCompressionResult = c_int;
/// This build cannot compress.
pub const GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED: GhosttyTerminalCompressionResult = 0;
/// More work remains.
pub const GHOSTTY_TERMINAL_COMPRESSION_RESULT_PENDING: GhosttyTerminalCompressionResult = 1;
/// Everything compressible is compressed.
pub const GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE: GhosttyTerminalCompressionResult = 2;

/// Mode plus value for `OPT_MODE`, `OPT_MODE_DEFAULT` and `DATA_MODE`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyTerminalModeConfig {
    /// The mode, from [`ghostty_mode`].
    pub mode: GhosttyMode,
    /// Set (true) or reset.
    pub value: bool,
}

/// Grid and cell size for size reports (`size_report.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttySizeReportSize {
    /// Rows.
    pub rows: u16,
    /// Columns.
    pub columns: u16,
    /// Cell width in pixels.
    pub cell_width: u32,
    /// Cell height in pixels.
    pub cell_height: u32,
}

/// `GhosttySnapshotDecoderOption` (`snapshot.h`); set before decoding starts.
pub type GhosttySnapshotDecoderOption = c_int;
/// `size_t*`: largest continuation accepted; with RETAIN also the returned terminal's tracking limit.
pub const GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES: GhosttySnapshotDecoderOption = 0;
/// `bool*`: keep continuation tracking on the returned terminal.
pub const GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION: GhosttySnapshotDecoderOption = 1;

/// `GhosttyColorScheme` (`device.h`), the `CSI ? 996 n` answer.
pub type GhosttyColorScheme = c_int;
/// Light scheme.
pub const GHOSTTY_COLOR_SCHEME_LIGHT: GhosttyColorScheme = 0;
/// Dark scheme.
pub const GHOSTTY_COLOR_SCHEME_DARK: GhosttyColorScheme = 1;

/// An OSC 9 / OSC 777 request, borrowed for the callback only; a sized struct.
#[repr(C)]
pub struct GhosttyTerminalDesktopNotification {
    /// Struct size.
    pub size: usize,
    /// Title (empty for OSC 9).
    pub title: GhosttyString,
    /// Body.
    pub body: GhosttyString,
}

/// `GhosttyTerminalProgressState` (`terminal.h`): 0 remove, 1 set, 2 error, 3 indeterminate, 4 pause.
pub type GhosttyTerminalProgressState = c_int;

/// An OSC 9;4 progress report, borrowed for the callback only; a sized struct.
#[repr(C)]
pub struct GhosttyTerminalProgressReport {
    /// Struct size.
    pub size: usize,
    /// Progress state.
    pub state: GhosttyTerminalProgressState,
    /// Percent 0–100, or -1 when omitted.
    pub progress: i8,
}

/// `GhosttyClipboardLocation` (`terminal.h`).
pub type GhosttyClipboardLocation = c_int;
/// The standard clipboard (OSC 52 `c`).
pub const GHOSTTY_CLIPBOARD_LOCATION_STANDARD: GhosttyClipboardLocation = 0;
/// `GhosttyClipboardWriteResult` (`terminal.h`).
pub type GhosttyClipboardWriteResult = c_int;
/// The write was accepted.
pub const GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS: GhosttyClipboardWriteResult = 0;

/// One MIME representation of a clipboard write, borrowed for the callback only.
#[repr(C)]
pub struct GhosttyClipboardContent {
    /// MIME type, e.g. `text/plain`.
    pub mime: GhosttyString,
    /// Decoded data.
    pub data: GhosttyString,
}

/// The answer to a clipboard write; a sized struct.
#[repr(C)]
pub struct GhosttyClipboardWriteReply {
    /// Struct size.
    pub size: usize,
    /// Accepted or why not.
    pub result: GhosttyClipboardWriteResult,
    /// Remember the decision for the session (only with `can_remember`).
    pub remember: bool,
}

/// Answers a [`GhosttyClipboardWrite`]; must be called inside the clipboard-write callback.
pub type GhosttyClipboardWriteReplyFn = Option<
    unsafe extern "C" fn(
        write: *const GhosttyClipboardWrite,
        reply: *const GhosttyClipboardWriteReply,
    ),
>;

/// A synchronous OSC 52 write request, borrowed for the callback only; a sized struct.
#[repr(C)]
pub struct GhosttyClipboardWrite {
    /// Struct size.
    pub size: usize,
    /// Destination clipboard.
    pub location: GhosttyClipboardLocation,
    /// `contents_len` representations of one value.
    pub contents: *const GhosttyClipboardContent,
    /// Number of representations; 0 means clear the destination.
    pub contents_len: usize,
    /// Name of the writing program, if the protocol carries one.
    pub name: GhosttyString,
    /// The terminal already holds a session grant.
    pub granted: bool,
    /// `remember` in the reply is honoured.
    pub can_remember: bool,
    /// Terminal-owned; do not access.
    pub ctx: *const c_void,
    /// Answers the write; returning without calling it denies the write.
    pub reply: GhosttyClipboardWriteReplyFn,
}

/// Bytes the terminal writes back to the pty; runs inside `vt_write` and must not block.
pub type GhosttyTerminalWritePtyFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
);
/// BEL received.
pub type GhosttyTerminalBellFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
/// The title changed; read it with `DATA_TITLE`.
pub type GhosttyTerminalTitleChangedFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
/// The pwd changed; read it with `DATA_PWD`.
pub type GhosttyTerminalPwdChangedFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
/// Fill `out_size` and return true to answer a size query; false leaves it unanswered.
pub type GhosttyTerminalSizeFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out_size: *mut GhosttySizeReportSize,
) -> bool;
/// Fill `out_scheme` and return true to answer `CSI ? 996 n`.
pub type GhosttyTerminalColorSchemeFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out_scheme: *mut GhosttyColorScheme,
) -> bool;
/// The XTVERSION answer; the bytes must stay valid until the callback returns.
pub type GhosttyTerminalXtversionFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void) -> GhosttyString;
/// An OSC 9 / 777 notification.
pub type GhosttyTerminalDesktopNotificationFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    notification: *const GhosttyTerminalDesktopNotification,
);
/// An OSC 9;4 progress report.
pub type GhosttyTerminalProgressReportFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    report: *const GhosttyTerminalProgressReport,
);
/// An OSC 52 clipboard write; answer it through `write.reply` before returning.
pub type GhosttyTerminalClipboardWriteFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    write: *const GhosttyClipboardWrite,
);

/// `GhosttyRenderStateData` (`render.h`): what [`ghostty_render_state_get`] reads.
pub type GhosttyRenderStateData = c_int;
/// `uint16_t*`: columns of the copied viewport.
pub const GHOSTTY_RENDER_STATE_DATA_COLS: GhosttyRenderStateData = 1;
/// `uint16_t*`: rows of the copied viewport.
pub const GHOSTTY_RENDER_STATE_DATA_ROWS: GhosttyRenderStateData = 2;
/// [`GhosttyRenderStateDirty`]`*`: global dirty state since the last clean.
pub const GHOSTTY_RENDER_STATE_DATA_DIRTY: GhosttyRenderStateData = 3;
/// [`GhosttyRenderStateRowIterator`]`*`: (re)positions an iterator before the first row.
pub const GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR: GhosttyRenderStateData = 4;
/// [`GhosttyRenderStateCursor`]`*` (sized): the cursor.
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR: GhosttyRenderStateData = 18;

/// `GhosttyRenderStateDirty` (`render.h`).
pub type GhosttyRenderStateDirty = c_int;
/// Nothing changed.
pub const GHOSTTY_RENDER_STATE_DIRTY_FALSE: GhosttyRenderStateDirty = 0;
/// Some rows changed; the row iterator's `next_dirty` yields them.
pub const GHOSTTY_RENDER_STATE_DIRTY_PARTIAL: GhosttyRenderStateDirty = 1;
/// Everything must be redrawn (scroll, screen switch, resize, palette change).
pub const GHOSTTY_RENDER_STATE_DIRTY_FULL: GhosttyRenderStateDirty = 2;

/// `GhosttyRenderStateRowData` (`render.h`): what [`ghostty_render_state_row_get`] reads for the current row.
pub type GhosttyRenderStateRowData = c_int;
/// [`GhosttyRow`]`*`: the row header (wrap flags).
pub const GHOSTTY_RENDER_STATE_ROW_DATA_RAW: GhosttyRenderStateRowData = 2;
/// [`GhosttyRenderStateRowCells`]`*`: positions a cell iterator on this row.
pub const GHOSTTY_RENDER_STATE_ROW_DATA_CELLS: GhosttyRenderStateRowData = 3;
/// [`GhosttyCellsView`]`*`: every raw cell of the row, valid until the next update.
pub const GHOSTTY_RENDER_STATE_ROW_DATA_CELLS_RAW: GhosttyRenderStateRowData = 5;

/// `GhosttyRenderStateRowCellsData` (`render.h`): what [`ghostty_render_state_row_cells_get`] reads.
pub type GhosttyRenderStateRowCellsData = c_int;
/// [`GhosttyStyle`]`*` (sized): the selected cell's style.
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE: GhosttyRenderStateRowCellsData = 2;
/// `uint32_t*`: codepoints of the cluster including the base; 0 without text.
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN: GhosttyRenderStateRowCellsData = 3;
/// `uint32_t*` buffer of at least `GRAPHEMES_LEN` entries: base codepoint first.
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF: GhosttyRenderStateRowCellsData = 4;

/// The cursor as a renderer sees it (`render.h`); a sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyRenderStateCursor {
    /// Struct size.
    pub size: usize,
    /// The cursor lies inside the viewport, so `viewport_x/y` are meaningful.
    pub viewport_has_value: bool,
    /// Column in the viewport.
    pub viewport_x: u16,
    /// Row in the viewport.
    pub viewport_y: u16,
    /// The cursor sits on the spacer half of a wide character.
    pub wide_tail: bool,
    /// DECTCEM says draw it.
    pub visible: bool,
    /// The program asked for blinking.
    pub blinking: bool,
    /// The terminal believes a password is being typed.
    pub password_input: bool,
    /// `GhosttyRenderStateCursorVisualStyle`: 0 bar, 1 block, 2 underline, 3 hollow block.
    pub visual_style: c_int,
}

/// A borrowed array of raw cells (`render.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyCellsView {
    /// First cell.
    pub ptr: *const GhosttyCell,
    /// Number of cells.
    pub len: usize,
}

/// `GhosttyCellData` (`screen.h`): what [`ghostty_cell_get`] reads from a raw cell.
pub type GhosttyCellData = c_int;
/// `uint32_t*`: base codepoint; 0 for an empty cell.
pub const GHOSTTY_CELL_DATA_CODEPOINT: GhosttyCellData = 1;
/// [`GhosttyCellContentTag`]`*`.
pub const GHOSTTY_CELL_DATA_CONTENT_TAG: GhosttyCellData = 2;
/// [`GhosttyCellWide`]`*`.
pub const GHOSTTY_CELL_DATA_WIDE: GhosttyCellData = 3;
/// `bool*`: the cell has a non-default style (bg-only cells report false).
pub const GHOSTTY_CELL_DATA_HAS_STYLING: GhosttyCellData = 5;
/// `uint8_t*`: palette index of a bg-only cell (content tag BG_COLOR_PALETTE).
pub const GHOSTTY_CELL_DATA_COLOR_PALETTE: GhosttyCellData = 10;
/// [`GhosttyColorRgb`]`*`: colour of a bg-only cell (content tag BG_COLOR_RGB).
pub const GHOSTTY_CELL_DATA_COLOR_RGB: GhosttyCellData = 11;

/// `GhosttyCellContentTag` (`screen.h`).
pub type GhosttyCellContentTag = c_int;
/// A single codepoint (or none).
pub const GHOSTTY_CELL_CONTENT_CODEPOINT: GhosttyCellContentTag = 0;
/// A codepoint plus extra grapheme codepoints.
pub const GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME: GhosttyCellContentTag = 1;
/// No text; background from the palette (an erase with a background colour).
pub const GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE: GhosttyCellContentTag = 2;
/// No text; RGB background.
pub const GHOSTTY_CELL_CONTENT_BG_COLOR_RGB: GhosttyCellContentTag = 3;

/// `GhosttyCellWide` (`screen.h`).
pub type GhosttyCellWide = c_int;
/// One column.
pub const GHOSTTY_CELL_WIDE_NARROW: GhosttyCellWide = 0;
/// First half of a two-column character.
pub const GHOSTTY_CELL_WIDE_WIDE: GhosttyCellWide = 1;
/// Second half of a wide character.
pub const GHOSTTY_CELL_WIDE_SPACER_TAIL: GhosttyCellWide = 2;
/// Row-end padding where a wide character wrapped to the next row.
pub const GHOSTTY_CELL_WIDE_SPACER_HEAD: GhosttyCellWide = 3;

/// `GhosttyRowData` (`screen.h`): what [`ghostty_row_get`] reads from a raw row.
pub type GhosttyRowData = c_int;
/// `bool*`: the row soft-wraps onto the next.
pub const GHOSTTY_ROW_DATA_WRAP: GhosttyRowData = 1;

/// `GhosttyPointTag` (`point.h`): the coordinate space of a [`GhosttyPoint`].
pub type GhosttyPointTag = c_int;
/// The active area (the live screen).
pub const GHOSTTY_POINT_TAG_ACTIVE: GhosttyPointTag = 0;
/// Scrollback plus active area; y 0 is the oldest scrollback row.
pub const GHOSTTY_POINT_TAG_SCREEN: GhosttyPointTag = 2;
/// Scrollback only; y 0 is the oldest scrollback row.
pub const GHOSTTY_POINT_TAG_HISTORY: GhosttyPointTag = 3;

/// A cell coordinate (`point.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyPointCoordinate {
    /// Column.
    pub x: u16,
    /// Row in the tag's space.
    pub y: u32,
}

/// Payload of [`GhosttyPoint`].
#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyPointValue {
    /// The coordinate.
    pub coordinate: GhosttyPointCoordinate,
    /// Keeps the union 16 bytes wide.
    pub _padding: [u64; 2],
}

/// A point in one coordinate space (`point.h`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyPoint {
    /// Coordinate space.
    pub tag: GhosttyPointTag,
    /// The coordinate.
    pub value: GhosttyPointValue,
}

/// An untracked reference to one cell (`grid_ref.h`), valid until the next mutating terminal call; a sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyGridRef {
    /// Struct size.
    pub size: usize,
    /// Page node; opaque.
    pub node: *mut c_void,
    /// Column within the page.
    pub x: u16,
    /// Row within the page.
    pub y: u16,
}

/// A selection between two grid references (`selection.h`), used for search matches; a sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttySelection {
    /// Struct size.
    pub size: usize,
    /// First cell (inclusive); may come after `end`.
    pub start: GhosttyGridRef,
    /// Last cell (inclusive).
    pub end: GhosttyGridRef,
    /// Rectangular rather than linear.
    pub rectangle: bool,
}

/// A caller-owned buffer of selections (`selection.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttySelectionBuffer {
    /// Destination; null with `cap` 0 queries the size.
    pub ptr: *mut GhosttySelection,
    /// Capacity in entries.
    pub cap: usize,
    /// Entries written, or the capacity needed on OUT_OF_SPACE.
    pub len: usize,
}

/// `GhosttySearchOption` (`search.h`).
pub type GhosttySearchOption = c_int;
/// [`GhosttyString`]`*`: the needle, copied; ASCII letters match case-insensitively.
pub const GHOSTTY_SEARCH_OPT_NEEDLE: GhosttySearchOption = 0;
/// `GhosttySearchScroll*`: scroll policy of the select options.
pub const GHOSTTY_SEARCH_OPT_SELECT_SCROLL: GhosttySearchOption = 3;
/// `GhosttySearchData` (`search.h`).
pub type GhosttySearchData = c_int;
/// `size_t*`: matches on the active screen.
pub const GHOSTTY_SEARCH_DATA_TOTAL_MATCHES: GhosttySearchData = 2;
/// [`GhosttySelectionBuffer`]`*`: every match, newest to oldest.
pub const GHOSTTY_SEARCH_DATA_MATCHES: GhosttySearchData = 5;
/// `GhosttySearchScroll` value: never scroll the viewport.
pub const GHOSTTY_SEARCH_SCROLL_NONE: c_int = 1;

/// `GhosttyKeyAction` (`key/event.h`), numbered as `ply_proto::data::KeyAction`.
pub type GhosttyKeyAction = c_int;
/// `GhosttyKey` (`key/event.h`): a physical key, 0–175 at this pin.
pub type GhosttyKey = c_int;
/// The key has no identity; the event's text is sent as is.
pub const GHOSTTY_KEY_UNIDENTIFIED: GhosttyKey = 0;
/// Return / Enter.
pub const GHOSTTY_KEY_ENTER: GhosttyKey = 58;
/// `GhosttyKeyEncoderOption` (`key/encoder.h`).
pub type GhosttyKeyEncoderOption = c_int;
/// `GhosttyOptionAsAlt*`: which ⌥ acts as Alt; reset by every `setopt_from_terminal`.
pub const GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT: GhosttyKeyEncoderOption = 6;
/// `GhosttyOptionAsAlt` (`key/encoder.h`).
pub type GhosttyOptionAsAlt = c_int;
/// ⌥ types layout characters.
pub const GHOSTTY_OPTION_AS_ALT_FALSE: GhosttyOptionAsAlt = 0;
/// Both ⌥ keys are Alt.
pub const GHOSTTY_OPTION_AS_ALT_TRUE: GhosttyOptionAsAlt = 1;
/// The left ⌥ is Alt.
pub const GHOSTTY_OPTION_AS_ALT_LEFT: GhosttyOptionAsAlt = 2;
/// The right ⌥ is Alt.
pub const GHOSTTY_OPTION_AS_ALT_RIGHT: GhosttyOptionAsAlt = 3;

/// `GhosttyMouseAction` (`mouse/event.h`), numbered as `ply_proto::data::MouseAction`.
pub type GhosttyMouseAction = c_int;
/// `GhosttyMouseButton` (`mouse/event.h`), numbered as `ply_proto::data::MouseButton` (1–11).
pub type GhosttyMouseButton = c_int;
/// A pointer position in surface pixels (`mouse/event.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyMousePosition {
    /// Pixels from the left edge.
    pub x: f32,
    /// Pixels from the top edge.
    pub y: f32,
}
/// `GhosttyMouseEncoderOption` (`mouse/encoder.h`).
pub type GhosttyMouseEncoderOption = c_int;
/// [`GhosttyMouseEncoderSize`]`*`: surface and cell geometry.
pub const GHOSTTY_MOUSE_ENCODER_OPT_SIZE: GhosttyMouseEncoderOption = 2;
/// `bool*`: some button is held (drag reporting).
pub const GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED: GhosttyMouseEncoderOption = 3;
/// `bool*`: drop motion events that stay in the last reported cell.
pub const GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL: GhosttyMouseEncoderOption = 4;

/// Surface geometry for the mouse encoder (`mouse/encoder.h`); a sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyMouseEncoderSize {
    /// Struct size.
    pub size: usize,
    /// Surface width in pixels.
    pub screen_width: u32,
    /// Surface height in pixels.
    pub screen_height: u32,
    /// Cell width in pixels.
    pub cell_width: u32,
    /// Cell height in pixels.
    pub cell_height: u32,
    /// Top padding in pixels.
    pub padding_top: u32,
    /// Bottom padding in pixels.
    pub padding_bottom: u32,
    /// Right padding in pixels.
    pub padding_right: u32,
    /// Left padding in pixels.
    pub padding_left: u32,
}

/// `GhosttyFocusEvent` (`focus.h`).
pub type GhosttyFocusEvent = c_int;
/// Focus gained (`CSI I`).
pub const GHOSTTY_FOCUS_GAINED: GhosttyFocusEvent = 0;
/// Focus lost (`CSI O`).
pub const GHOSTTY_FOCUS_LOST: GhosttyFocusEvent = 1;

/// Receives bytes (`io.h`); return false to fail the operation.
pub type GhosttyWriterFn =
    Option<unsafe extern "C" fn(userdata: *mut c_void, data: *const u8, len: usize) -> bool>;
/// A byte sink (`io.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyWriter {
    /// Called with each piece.
    pub write: GhosttyWriterFn,
    /// Passed back to `write`.
    pub userdata: *mut c_void,
}
/// Produces one MIME representation into `writer` (`io.h`); return false to fail the paste.
pub type GhosttyMimeReaderFn = Option<
    unsafe extern "C" fn(userdata: *mut c_void, mime: GhosttyString, writer: GhosttyWriter) -> bool,
>;
/// A source of MIME representations (`io.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyMimeReader {
    /// Called at most once per paste.
    pub read: GhosttyMimeReaderFn,
    /// Passed back to `read`.
    pub userdata: *mut c_void,
}

/// `GhosttyPasteSource` (`paste.h`).
pub type GhosttyPasteSource = c_int;
/// The user pasted from a clipboard.
pub const GHOSTTY_PASTE_SOURCE_CLIPBOARD: GhosttyPasteSource = 0;

/// A paste request (`paste.h`), borrowed for the call; a sized struct.
#[repr(C)]
pub struct GhosttyPaste {
    /// Struct size.
    pub size: usize,
    /// Clipboard the contents came from.
    pub location: GhosttyClipboardLocation,
    /// Why the paste happened.
    pub source: GhosttyPasteSource,
    /// MIME types available, preferred first.
    pub mimes: *const GhosttyString,
    /// Entries in `mimes`.
    pub mimes_len: usize,
    /// Produces the chosen representation.
    pub reader: GhosttyMimeReader,
    /// Write text that could inject commands.
    pub allow_unsafe: bool,
}

/// `GhosttySysOption` (`sys.h`): process-wide settings.
pub type GhosttySysOption = c_int;
/// [`GhosttySysLogFn`]: receives the library's log lines.
pub const GHOSTTY_SYS_OPT_LOG: GhosttySysOption = 2;
/// `GhosttySysLogLevel` (`sys.h`): 0 error, 1 warning, 2 info, 3 debug.
pub type GhosttySysLogLevel = c_int;
/// Receives one log line; may run on any thread that calls into the library.
pub type GhosttySysLogFn = unsafe extern "C" fn(
    userdata: *mut c_void,
    level: GhosttySysLogLevel,
    scope: *const u8,
    scope_len: usize,
    message: *const u8,
    message_len: usize,
);

/// `ghostty_mode_new` from `modes.h`, a `static inline` in C: the mode number with bit 15 set for ANSI modes.
pub const fn ghostty_mode(value: u16, ansi: bool) -> GhosttyMode {
    (value & 0x7FFF) | ((ansi as u16) << 15)
}
/// DEC 25: cursor visible.
pub const GHOSTTY_MODE_CURSOR_VISIBLE: GhosttyMode = ghostty_mode(25, false);
/// DEC 1004: focus events.
pub const GHOSTTY_MODE_FOCUS_EVENT: GhosttyMode = ghostty_mode(1004, false);
/// DEC 2004: bracketed paste.
pub const GHOSTTY_MODE_BRACKETED_PASTE: GhosttyMode = ghostty_mode(2004, false);
/// DEC 2026: synchronized output.
pub const GHOSTTY_MODE_SYNC_OUTPUT: GhosttyMode = ghostty_mode(2026, false);
/// DEC 2027: grapheme clustering.
pub const GHOSTTY_MODE_GRAPHEME_CLUSTER: GhosttyMode = ghostty_mode(2027, false);

unsafe extern "C" {
    /// The library's ABI manifest as JSON (`types.h`): struct sizes and offsets and enum values; static storage.
    pub fn ghostty_type_json() -> *const c_char;
    /// Frees memory the library allocated for the caller (`allocator.h`), e.g. an encoded snapshot.
    pub fn ghostty_free(allocator: *const GhosttyAllocator, ptr: *mut u8, len: usize);
    /// Sets a process-wide option (`sys.h`); not synchronized with other library calls.
    pub fn ghostty_sys_set(option: GhosttySysOption, value: *const c_void) -> GhosttyResult;

    /// Creates a terminal of `cols` × `rows` (`terminal.h`).
    pub fn ghostty_terminal_new(
        allocator: *const GhosttyAllocator,
        terminal: *mut GhosttyTerminal,
        cols: u16,
        rows: u16,
    ) -> GhosttyResult;
    /// Frees a terminal; null is ignored.
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    /// Resizes; the primary screen reflows, DEC 2026 is cleared and a mode 2048 report may be written.
    pub fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> GhosttyResult;
    /// Sets an option; `value`'s type depends on `option`.
    pub fn ghostty_terminal_set(
        terminal: GhosttyTerminal,
        option: GhosttyTerminalOption,
        value: *const c_void,
    ) -> GhosttyResult;
    /// Reads a value; `out`'s type depends on `data`.
    pub fn ghostty_terminal_get(
        terminal: GhosttyTerminal,
        data: GhosttyTerminalData,
        out: *mut c_void,
    ) -> GhosttyResult;
    /// Feeds pty output; never fails; callbacks run synchronously inside and must not re-enter the terminal.
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);
    /// A token that changes whenever compressible content changes.
    pub fn ghostty_terminal_compression_activity(
        terminal: GhosttyTerminal,
        out_activity: *mut u64,
    ) -> GhosttyResult;
    /// One compression step over idle scrollback.
    pub fn ghostty_terminal_compress(
        terminal: GhosttyTerminal,
        mode: GhosttyTerminalCompressionMode,
        out_result: *mut GhosttyTerminalCompressionResult,
    ) -> GhosttyResult;
    /// Resolves a point to an untracked grid reference.
    pub fn ghostty_terminal_grid_ref(
        terminal: GhosttyTerminal,
        point: GhosttyPoint,
        out_ref: *mut GhosttyGridRef,
    ) -> GhosttyResult;
    /// Converts a grid reference to a coordinate in the `tag` space; fails when the cell lies outside it.
    pub fn ghostty_terminal_point_from_grid_ref(
        terminal: GhosttyTerminal,
        grid_ref: *const GhosttyGridRef,
        tag: GhosttyPointTag,
        out: *mut GhosttyPointCoordinate,
    ) -> GhosttyResult;
    /// Pastes per the live modes; output goes through the write-pty callback; REJECTED for unsafe text.
    pub fn ghostty_terminal_paste(
        terminal: GhosttyTerminal,
        paste: *const GhosttyPaste,
        out_written: *mut bool,
    ) -> GhosttyResult;

    /// Creates an empty render state (`render.h`).
    pub fn ghostty_render_state_new(
        allocator: *const GhosttyAllocator,
        state: *mut GhosttyRenderState,
    ) -> GhosttyResult;
    /// Frees a render state.
    pub fn ghostty_render_state_free(state: GhosttyRenderState);
    /// Copies the terminal's viewport and consumes its dirty flags into this state.
    pub fn ghostty_render_state_update(
        state: GhosttyRenderState,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    /// Clears the global and every per-row dirty flag.
    pub fn ghostty_render_state_clean(state: GhosttyRenderState) -> GhosttyResult;
    /// Reads a value; `out`'s type depends on `data`.
    pub fn ghostty_render_state_get(
        state: GhosttyRenderState,
        data: GhosttyRenderStateData,
        out: *mut c_void,
    ) -> GhosttyResult;
    /// Creates a row iterator.
    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const GhosttyAllocator,
        out_iterator: *mut GhosttyRenderStateRowIterator,
    ) -> GhosttyResult;
    /// Frees a row iterator.
    pub fn ghostty_render_state_row_iterator_free(iterator: GhosttyRenderStateRowIterator);
    /// Advances to the next row; false past the last.
    pub fn ghostty_render_state_row_iterator_next(iterator: GhosttyRenderStateRowIterator) -> bool;
    /// Advances to the next dirty row and stores its index; false past the last.
    pub fn ghostty_render_state_row_iterator_next_dirty(
        iterator: GhosttyRenderStateRowIterator,
        out_y: *mut u16,
    ) -> bool;
    /// Reads a value of the current row.
    pub fn ghostty_render_state_row_get(
        iterator: GhosttyRenderStateRowIterator,
        data: GhosttyRenderStateRowData,
        out: *mut c_void,
    ) -> GhosttyResult;
    /// Creates a cell iterator.
    pub fn ghostty_render_state_row_cells_new(
        allocator: *const GhosttyAllocator,
        out_cells: *mut GhosttyRenderStateRowCells,
    ) -> GhosttyResult;
    /// Selects the cell at column `x` of the positioned row.
    pub fn ghostty_render_state_row_cells_select(
        cells: GhosttyRenderStateRowCells,
        x: u16,
    ) -> GhosttyResult;
    /// Reads a value of the selected cell.
    pub fn ghostty_render_state_row_cells_get(
        cells: GhosttyRenderStateRowCells,
        data: GhosttyRenderStateRowCellsData,
        out: *mut c_void,
    ) -> GhosttyResult;
    /// Frees a cell iterator.
    pub fn ghostty_render_state_row_cells_free(cells: GhosttyRenderStateRowCells);

    /// Reads a field of a raw cell (`screen.h`).
    pub fn ghostty_cell_get(
        cell: GhosttyCell,
        data: GhosttyCellData,
        out: *mut c_void,
    ) -> GhosttyResult;
    /// Reads a field of a raw row header (`screen.h`).
    pub fn ghostty_row_get(
        row: GhosttyRow,
        data: GhosttyRowData,
        out: *mut c_void,
    ) -> GhosttyResult;

    /// The raw cell at a grid reference (`grid_ref.h`).
    pub fn ghostty_grid_ref_cell(
        grid_ref: *const GhosttyGridRef,
        out_cell: *mut GhosttyCell,
    ) -> GhosttyResult;
    /// The raw row header at a grid reference.
    pub fn ghostty_grid_ref_row(
        grid_ref: *const GhosttyGridRef,
        out_row: *mut GhosttyRow,
    ) -> GhosttyResult;
    /// The cluster's codepoints, base first; OUT_OF_SPACE stores the count needed in `out_len`.
    pub fn ghostty_grid_ref_graphemes(
        grid_ref: *const GhosttyGridRef,
        buf: *mut u32,
        buf_len: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;
    /// The cell's style (`out_style.size` set by the caller).
    pub fn ghostty_grid_ref_style(
        grid_ref: *const GhosttyGridRef,
        out_style: *mut GhosttyStyle,
    ) -> GhosttyResult;

    /// Encodes the terminal's full state (`snapshot.h`) into library-allocated memory freed with [`ghostty_free`].
    pub fn ghostty_snapshot_encode_alloc(
        terminal: GhosttyTerminal,
        allocator: *const GhosttyAllocator,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> GhosttyResult;
    /// Creates a decoder over a borrowed buffer that must outlive it.
    pub fn ghostty_snapshot_decoder_new_buf(
        allocator: *const GhosttyAllocator,
        decoder: *mut GhosttySnapshotDecoder,
        ptr: *const u8,
        len: usize,
    ) -> GhosttyResult;
    /// Sets a decoder option; only before decoding starts.
    pub fn ghostty_snapshot_decoder_set(
        decoder: GhosttySnapshotDecoder,
        option: GhosttySnapshotDecoderOption,
        value: *const c_void,
    ) -> GhosttyResult;
    /// Frees a decoder; a terminal it produced stays valid.
    pub fn ghostty_snapshot_decoder_free(decoder: GhosttySnapshotDecoder);
    /// Decodes the whole snapshot into a new terminal owned by the caller.
    pub fn ghostty_snapshot_decoder_decode(
        decoder: GhosttySnapshotDecoder,
        terminal: *mut GhosttyTerminal,
    ) -> GhosttyResult;

    /// Creates a search over `terminal` (`search.h`); it must be freed before the terminal.
    pub fn ghostty_search_new(
        allocator: *const GhosttyAllocator,
        out_search: *mut GhosttySearch,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    /// Frees a search.
    pub fn ghostty_search_free(search: GhosttySearch);
    /// Runs the search over the whole terminal to completion.
    pub fn ghostty_search_run(search: GhosttySearch) -> GhosttyResult;
    /// Sets an option; `value`'s type depends on `option`.
    pub fn ghostty_search_set(
        search: GhosttySearch,
        option: GhosttySearchOption,
        value: *const c_void,
    ) -> GhosttyResult;
    /// Reads a value; `value`'s type depends on `data`.
    pub fn ghostty_search_get(
        search: GhosttySearch,
        data: GhosttySearchData,
        value: *mut c_void,
    ) -> GhosttyResult;

    /// Creates a key encoder (`key/encoder.h`).
    pub fn ghostty_key_encoder_new(
        allocator: *const GhosttyAllocator,
        encoder: *mut GhosttyKeyEncoder,
    ) -> GhosttyResult;
    /// Frees a key encoder.
    pub fn ghostty_key_encoder_free(encoder: GhosttyKeyEncoder);
    /// Sets one encoder option.
    pub fn ghostty_key_encoder_setopt(
        encoder: GhosttyKeyEncoder,
        option: GhosttyKeyEncoderOption,
        value: *const c_void,
    );
    /// Copies the terminal's key-related modes (DECCKM, keypad, kitty flags) into the encoder.
    pub fn ghostty_key_encoder_setopt_from_terminal(
        encoder: GhosttyKeyEncoder,
        terminal: GhosttyTerminal,
    );
    /// Encodes one event; OUT_OF_SPACE stores the size needed in `out_len`.
    pub fn ghostty_key_encoder_encode(
        encoder: GhosttyKeyEncoder,
        event: GhosttyKeyEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;
    /// Creates a key event (`key/event.h`).
    pub fn ghostty_key_event_new(
        allocator: *const GhosttyAllocator,
        event: *mut GhosttyKeyEvent,
    ) -> GhosttyResult;
    /// Frees a key event.
    pub fn ghostty_key_event_free(event: GhosttyKeyEvent);
    /// Press, repeat or release.
    pub fn ghostty_key_event_set_action(event: GhosttyKeyEvent, action: GhosttyKeyAction);
    /// The physical key.
    pub fn ghostty_key_event_set_key(event: GhosttyKeyEvent, key: GhosttyKey);
    /// Modifiers held.
    pub fn ghostty_key_event_set_mods(event: GhosttyKeyEvent, mods: GhosttyMods);
    /// Modifiers the layout consumed to produce the text.
    pub fn ghostty_key_event_set_consumed_mods(event: GhosttyKeyEvent, consumed_mods: GhosttyMods);
    /// An IME composition is in progress.
    pub fn ghostty_key_event_set_composing(event: GhosttyKeyEvent, composing: bool);
    /// Text the key produced; borrowed until the next encode.
    pub fn ghostty_key_event_set_utf8(event: GhosttyKeyEvent, utf8: *const c_char, len: usize);
    /// The key's codepoint without Shift.
    pub fn ghostty_key_event_set_unshifted_codepoint(event: GhosttyKeyEvent, codepoint: u32);

    /// Creates a mouse encoder (`mouse/encoder.h`).
    pub fn ghostty_mouse_encoder_new(
        allocator: *const GhosttyAllocator,
        encoder: *mut GhosttyMouseEncoder,
    ) -> GhosttyResult;
    /// Frees a mouse encoder.
    pub fn ghostty_mouse_encoder_free(encoder: GhosttyMouseEncoder);
    /// Sets one encoder option.
    pub fn ghostty_mouse_encoder_setopt(
        encoder: GhosttyMouseEncoder,
        option: GhosttyMouseEncoderOption,
        value: *const c_void,
    );
    /// Copies the terminal's mouse tracking mode and format into the encoder.
    pub fn ghostty_mouse_encoder_setopt_from_terminal(
        encoder: GhosttyMouseEncoder,
        terminal: GhosttyTerminal,
    );
    /// Encodes one event; SUCCESS with length 0 when the mode reports nothing.
    pub fn ghostty_mouse_encoder_encode(
        encoder: GhosttyMouseEncoder,
        event: GhosttyMouseEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;
    /// Creates a mouse event (`mouse/event.h`).
    pub fn ghostty_mouse_event_new(
        allocator: *const GhosttyAllocator,
        event: *mut GhosttyMouseEvent,
    ) -> GhosttyResult;
    /// Frees a mouse event.
    pub fn ghostty_mouse_event_free(event: GhosttyMouseEvent);
    /// Press, release or motion.
    pub fn ghostty_mouse_event_set_action(event: GhosttyMouseEvent, action: GhosttyMouseAction);
    /// The button involved.
    pub fn ghostty_mouse_event_set_button(event: GhosttyMouseEvent, button: GhosttyMouseButton);
    /// No button (plain motion).
    pub fn ghostty_mouse_event_clear_button(event: GhosttyMouseEvent);
    /// Modifiers held.
    pub fn ghostty_mouse_event_set_mods(event: GhosttyMouseEvent, mods: GhosttyMods);
    /// Pointer position in surface pixels.
    pub fn ghostty_mouse_event_set_position(
        event: GhosttyMouseEvent,
        position: GhosttyMousePosition,
    );

    /// Encodes a focus report (`focus.h`); OUT_OF_SPACE stores the size needed.
    pub fn ghostty_focus_encode(
        event: GhosttyFocusEvent,
        buf: *mut c_char,
        buf_len: usize,
        out_written: *mut usize,
    ) -> GhosttyResult;
}
