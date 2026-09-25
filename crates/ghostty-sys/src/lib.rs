//! Raw bindings to libghostty-vt, Ghostty's terminal engine as a C library (ghostty 44f2a44, spec 2).
//!
//! This is the only crate in ply with FFI to libghostty-vt (spec 3.2, INV-17). `build.rs` downloads the pinned ghostty
//! source once, checks its SHA-256 and caches it (or takes `PLY_GHOSTTY_SRC`), runs `zig build -Demit-lib-vt` on it
//! offline (`--system` with a package directory, prefix and caches under `OUT_DIR`, never writing into the source;
//! ADR-0005 Decision 1) and links `libghostty-vt.a` statically.
//! This file declares, by hand, the part of the C API ply uses from `vt/terminal.h`, `render.h`, `screen.h`,
//! `style.h`, `modes.h`, `device.h`, `size_report.h`, `snapshot.h`, `key.h`, `mouse.h`, `focus.h`, `paste.h`,
//! `search.h`, `grid_ref.h`, `grid_ref_tracked.h`, `point.h`, `sys.h` and `types.h` (spec 12, ADR-0005 spec delta 11),
//! plus the types those headers take from `allocator.h`, `color.h`, `io.h` and `selection.h`, and `formatter.h` for
//! tests' plain text.
//!
//! It holds no logic and depends on no ply crate (spec 3.2, 8.2). Only `ply-term` uses it, and only through its
//! `engine` feature, which only `ply-daemon` enables; everything else in ply sees the safe `ply_term::Engine`.
//!
//! Every enum is a C `int` (`types.h`) and the Zig side trusts it: in the ReleaseFast build an enum argument outside
//! the declared constants (a `GhosttyKey` above 175, say) is undefined behaviour, not an error, so callers validate
//! enum values before passing them. Every sized struct starts with `size: usize`, which the caller sets to the
//! struct's size and a callee checks before reading further. The layouts and constants here are checked against the
//! library's own ABI manifest ([`ghostty_type_json`]) by `tests/layout.rs`; an upgrade of the pin reruns that test
//! (ADR-0008 Decision 5). No function is thread-safe for one handle: the caller serializes every call on a terminal
//! and on the objects created from it, and a handle is freed exactly once. Every out pointer must be valid for
//! writes of its documented type, every in pointer valid for reads for the whole call.

#![allow(non_camel_case_types, non_upper_case_globals)]

use std::ffi::{c_char, c_int, c_void};

/// Result code of most calls (`types.h`): [`GHOSTTY_SUCCESS`] or one of the negative errors below.
pub type GhosttyResult = c_int;
/// The call succeeded and filled every out pointer it documents.
pub const GHOSTTY_SUCCESS: GhosttyResult = 0;
/// An allocation failed; the call had no effect.
pub const GHOSTTY_OUT_OF_MEMORY: GhosttyResult = -1;
/// A handle, pointer or option value was invalid; the call had no effect.
pub const GHOSTTY_INVALID_VALUE: GhosttyResult = -2;
/// The output buffer was too small; the length out-parameter holds the size needed and nothing was written.
pub const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;
/// The requested value does not exist (for example no selected search match, or the end of a snapshot).
pub const GHOSTTY_NO_VALUE: GhosttyResult = -4;
/// A reader or writer callback returned false.
pub const GHOSTTY_IO_ERROR: GhosttyResult = -5;
/// A configured limit was exceeded.
pub const GHOSTTY_LIMIT_EXCEEDED: GhosttyResult = -6;
/// The request was refused as unsafe (paste text that could inject commands); nothing was written.
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
    /// A terminal (`types.h`): screen, scrollback, modes and parser state; freed with [`ghostty_terminal_free`].
    GhosttyTerminal, GhosttyTerminalImpl;
    /// A render state (`render.h`) that copies a terminal's viewport and consumes its dirty flags on update.
    GhosttyRenderState, GhosttyRenderStateImpl;
    /// Walks a render state's rows; positioned by [`ghostty_render_state_get`] with `ROW_ITERATOR`.
    GhosttyRenderStateRowIterator, GhosttyRenderStateRowIteratorImpl;
    /// Walks the current row's cells; positioned by [`ghostty_render_state_row_get`] with `CELLS`.
    GhosttyRenderStateRowCells, GhosttyRenderStateRowCellsImpl;
    /// Decodes a snapshot produced by [`ghostty_snapshot_encode_alloc`] into a new terminal.
    GhosttySnapshotDecoder, GhosttySnapshotDecoderImpl;
    /// A scrollback search bound to one terminal (`search.h`); must be freed before that terminal.
    GhosttySearch, GhosttySearchImpl;
    /// Turns key events into pty bytes against copied terminal modes (`key/encoder.h`).
    GhosttyKeyEncoder, GhosttyKeyEncoderImpl;
    /// One reusable key event for a key encoder (`key/event.h`).
    GhosttyKeyEvent, GhosttyKeyEventImpl;
    /// Turns mouse events into pty bytes against copied terminal modes; tracks the last reported cell.
    GhosttyMouseEncoder, GhosttyMouseEncoderImpl;
    /// One reusable mouse event for a mouse encoder (`mouse/event.h`).
    GhosttyMouseEvent, GhosttyMouseEventImpl;
    /// Formats a terminal's content as text (`formatter.h`); must be freed before that terminal.
    GhosttyFormatter, GhosttyFormatterImpl;
    /// A grid reference that follows its cell through scrolling, pruning and reflow (`grid_ref_tracked.h`); freed with [`ghostty_tracked_grid_ref_free`], before or after its terminal.
    GhosttyTrackedGridRef, GhosttyTrackedGridRefImpl;
}

/// A packed 8-byte grid cell (`screen.h`); its layout is private to the library, so read it only through [`ghostty_cell_get`].
pub type GhosttyCell = u64;
/// A packed 8-byte row header (`screen.h`); read it only through [`ghostty_row_get`].
pub type GhosttyRow = u64;
/// Modifier bitmask (`key/event.h`): bits 0–9 as `ply_proto::data::Mods`, bits 10–15 must be zero.
pub type GhosttyMods = u16;
/// A terminal mode packed by [`ghostty_mode`] (`modes.h`): number in bits 0–14, bit 15 set for ANSI modes.
pub type GhosttyMode = u16;

/// A borrowed byte string (`types.h`); `ptr` may be null when `len` is 0, and the bytes need not be UTF-8.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyString {
    /// First byte; valid for `len` bytes for as long as the API that produced or takes it says.
    pub ptr: *const u8,
    /// Length in bytes (no terminating NUL).
    pub len: usize,
}

/// An sRGB colour, 8 bits per channel, no alpha (`color.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct GhosttyColorRgb {
    /// Red channel, 0–255, gamma-encoded.
    pub r: u8,
    /// Green channel, 0–255, gamma-encoded.
    pub g: u8,
    /// Blue channel, 0–255, gamma-encoded.
    pub b: u8,
}

/// Tag of [`GhosttyStyleColor`] (`style.h`), selecting the valid union member.
pub type GhosttyStyleColorTag = c_int;
/// No colour set: the renderer uses the default for the slot.
pub const GHOSTTY_STYLE_COLOR_NONE: GhosttyStyleColorTag = 0;
/// A 256-colour palette index in `value.palette`, unresolved.
pub const GHOSTTY_STYLE_COLOR_PALETTE: GhosttyStyleColorTag = 1;
/// A true colour in `value.rgb`.
pub const GHOSTTY_STYLE_COLOR_RGB: GhosttyStyleColorTag = 2;

/// Payload of [`GhosttyStyleColor`]; only the member its tag names is meaningful.
#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyStyleColorValue {
    /// Palette index 0–255 (tag PALETTE).
    pub palette: u8,
    /// RGB value (tag RGB).
    pub rgb: GhosttyColorRgb,
    /// Keeps the union 8 bytes wide as in C; never read.
    pub _padding: u64,
}

/// A colour in a style: none, a palette index or RGB, never resolved against the palette.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyleColor {
    /// Which member of `value` is valid.
    pub tag: GhosttyStyleColorTag,
    /// The colour, per `tag`.
    pub value: GhosttyStyleColorValue,
}

/// SGR underline kind (`sgr.h` `GhosttySgrUnderline`): 0 none, 1 single, 2 double, 3 curly, 4 dotted, 5 dashed.
pub type GhosttySgrUnderline = c_int;

/// A cell's style (`style.h`), a sized struct filled by the library.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyle {
    /// Must equal `size_of::<GhosttyStyle>()` before a call fills it.
    pub size: usize,
    /// Text colour (SGR 30–38, 90–97).
    pub fg_color: GhosttyStyleColor,
    /// Cell background (SGR 40–48, 100–107).
    pub bg_color: GhosttyStyleColor,
    /// Underline colour (SGR 58); NONE follows the text colour.
    pub underline_color: GhosttyStyleColor,
    /// SGR 1 is in effect.
    pub bold: bool,
    /// SGR 3 is in effect.
    pub italic: bool,
    /// SGR 2 is in effect.
    pub faint: bool,
    /// SGR 5 is in effect.
    pub blink: bool,
    /// SGR 7 is in effect (swap fg and bg when drawing).
    pub inverse: bool,
    /// SGR 8 is in effect (draw no glyph).
    pub invisible: bool,
    /// SGR 9 is in effect.
    pub strikethrough: bool,
    /// SGR 53 is in effect.
    pub overline: bool,
    /// Underline kind, 0–5 ([`GhosttySgrUnderline`]).
    pub underline: GhosttySgrUnderline,
}

/// `GhosttyTerminalOption` (`terminal.h`): what [`ghostty_terminal_set`] configures; the comment names the value's type.
pub type GhosttyTerminalOption = c_int;
/// `void*` passed back to every callback; must outlive the terminal.
pub const GHOSTTY_TERMINAL_OPT_USERDATA: GhosttyTerminalOption = 0;
/// [`GhosttyTerminalWritePtyFn`]: query answers and reports for the pty.
pub const GHOSTTY_TERMINAL_OPT_WRITE_PTY: GhosttyTerminalOption = 1;
/// [`GhosttyTerminalBellFn`]: BEL received.
pub const GHOSTTY_TERMINAL_OPT_BELL: GhosttyTerminalOption = 2;
/// [`GhosttyTerminalXtversionFn`]: the XTVERSION (`CSI > q`) answer; unset answers "libghostty".
pub const GHOSTTY_TERMINAL_OPT_XTVERSION: GhosttyTerminalOption = 4;
/// [`GhosttyTerminalTitleChangedFn`]: OSC 0/2.
pub const GHOSTTY_TERMINAL_OPT_TITLE_CHANGED: GhosttyTerminalOption = 5;
/// [`GhosttyTerminalSizeFn`]: CSI 14/16/18 t and mode 2048 reports.
pub const GHOSTTY_TERMINAL_OPT_SIZE: GhosttyTerminalOption = 6;
/// [`GhosttyTerminalColorSchemeFn`]: `CSI ? 996 n`; unset leaves it unanswered.
pub const GHOSTTY_TERMINAL_OPT_COLOR_SCHEME: GhosttyTerminalOption = 7;
/// `GhosttyColorRgb*`: default foreground, answering OSC 10; null unsets (then OSC 10 goes unanswered).
pub const GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND: GhosttyTerminalOption = 11;
/// `GhosttyColorRgb*`: default background, answering OSC 11; null unsets.
pub const GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND: GhosttyTerminalOption = 12;
/// `GhosttyColorRgb*`: default cursor colour, answering OSC 12; null unsets.
pub const GHOSTTY_TERMINAL_OPT_COLOR_CURSOR: GhosttyTerminalOption = 13;
/// `GhosttyColorRgb[256]*`: default palette, answering OSC 4 and restored by OSC 104.
pub const GHOSTTY_TERMINAL_OPT_COLOR_PALETTE: GhosttyTerminalOption = 14;
/// `uint64_t*`: bytes of Kitty images kept; 0 disables the Kitty graphics protocol.
pub const GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT: GhosttyTerminalOption = 15;
/// `bool*`: Glyph Protocol APC handling; false ignores its sequences and clears registered glyphs.
pub const GHOSTTY_TERMINAL_OPT_GLYPH_PROTOCOL: GhosttyTerminalOption = 24;
/// [`GhosttyTerminalPwdChangedFn`]: OSC 7, OSC 9;9, OSC 1337 CurrentDir.
pub const GHOSTTY_TERMINAL_OPT_PWD_CHANGED: GhosttyTerminalOption = 25;
/// [`GhosttyTerminalClipboardWriteFn`]: OSC 52 (and OSC 5522) writes.
pub const GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE: GhosttyTerminalOption = 26;
/// `size_t*`: scrollback byte cap, page-granular; null removes it (the built-in default is 10 000 bytes).
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES: GhosttyTerminalOption = 27;
/// `size_t*`: scrollback line cap, pruned page by page (keeps somewhat fewer rows); null removes it.
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES: GhosttyTerminalOption = 28;
/// [`GhosttyTerminalDesktopNotificationFn`]: OSC 9 bodies that are not ConEmu commands, and OSC 777.
pub const GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION: GhosttyTerminalOption = 29;
/// [`GhosttyTerminalProgressReportFn`]: OSC 9;4.
pub const GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT: GhosttyTerminalOption = 30;
/// `size_t*`: bytes of an unfinished sequence kept for snapshots; 0 or null disables tracking.
pub const GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES: GhosttyTerminalOption = 31;
/// [`GhosttyTerminalModeConfig`]`*`: sets a mode's current value and its reset (RIS) default.
pub const GHOSTTY_TERMINAL_OPT_MODE_DEFAULT: GhosttyTerminalOption = 33;
/// [`GhosttyTerminalModeConfig`]`*`: sets a mode's current value only.
pub const GHOSTTY_TERMINAL_OPT_MODE: GhosttyTerminalOption = 34;
/// [`GhosttyString`]`*`: the name XTGETTCAP `TN` reports, copied, at most 128 bytes.
pub const GHOSTTY_TERMINAL_OPT_TERMINFO_NAME: GhosttyTerminalOption = 37;
/// `size_t*`: most decoded bytes one OSC 5522 clipboard write may buffer; null restores the 64 MiB default.
pub const GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE_MAX_BYTES: GhosttyTerminalOption = 39;

/// `GhosttyTerminalData` (`terminal.h`): what [`ghostty_terminal_get`] reads; the comment names the out type.
pub type GhosttyTerminalData = c_int;
/// `uint16_t*`: grid columns.
pub const GHOSTTY_TERMINAL_DATA_COLS: GhosttyTerminalData = 1;
/// `uint16_t*`: grid rows.
pub const GHOSTTY_TERMINAL_DATA_ROWS: GhosttyTerminalData = 2;
/// [`GhosttyTerminalScreen`]`*`: primary or alternate.
pub const GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN: GhosttyTerminalData = 6;
/// `bool*`: DECTCEM (mode 25) is set.
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
/// `size_t*`: scrollback rows (total minus viewport); 0 on the alternate screen.
pub const GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS: GhosttyTerminalData = 15;
/// `size_t*`: the continuation tracking limit; 0 when tracking is off.
pub const GHOSTTY_TERMINAL_DATA_CONTINUATION_MAX_BYTES: GhosttyTerminalData = 36;
/// [`GhosttyTerminalModeConfig`]`*` with `mode` set on input: its current value is written to `value`.
pub const GHOSTTY_TERMINAL_DATA_MODE: GhosttyTerminalData = 37;

/// `GhosttyTerminalScreen` (`terminal.h`).
pub type GhosttyTerminalScreen = c_int;
/// The primary screen, which keeps scrollback.
pub const GHOSTTY_TERMINAL_SCREEN_PRIMARY: GhosttyTerminalScreen = 0;
/// The alternate screen (DEC 1049 and friends), which has none.
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
/// Everything compressible is compressed until the activity token changes.
pub const GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE: GhosttyTerminalCompressionResult = 2;

/// Mode plus value for `OPT_MODE`, `OPT_MODE_DEFAULT` and `DATA_MODE`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyTerminalModeConfig {
    /// The mode, from [`ghostty_mode`].
    pub mode: GhosttyMode,
    /// True sets (DECSET) the mode, false resets it.
    pub value: bool,
}

/// Grid and cell size for size reports (`size_report.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttySizeReportSize {
    /// Grid rows.
    pub rows: u16,
    /// Grid columns.
    pub columns: u16,
    /// Cell width in pixels; reports multiply it by the column count.
    pub cell_width: u32,
    /// Cell height in pixels.
    pub cell_height: u32,
}

/// `GhosttySnapshotDecoderOption` (`snapshot.h`); set before decoding starts or the call returns INVALID_VALUE.
pub type GhosttySnapshotDecoderOption = c_int;
/// `size_t*`: largest continuation accepted; with RETAIN also the returned terminal's tracking limit.
pub const GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES: GhosttySnapshotDecoderOption = 0;
/// `bool*`: keep continuation tracking on the returned terminal.
pub const GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION: GhosttySnapshotDecoderOption = 1;

/// `GhosttyColorScheme` (`device.h`), the `CSI ? 996 n` answer.
pub type GhosttyColorScheme = c_int;
/// Report a light scheme (`CSI ? 997 ; 2 n`).
pub const GHOSTTY_COLOR_SCHEME_LIGHT: GhosttyColorScheme = 0;
/// Report a dark scheme (`CSI ? 997 ; 1 n`).
pub const GHOSTTY_COLOR_SCHEME_DARK: GhosttyColorScheme = 1;

/// An OSC 9 / OSC 777 request, borrowed for the callback only; a sized struct.
#[repr(C)]
pub struct GhosttyTerminalDesktopNotification {
    /// Size the library filled; read only fields that lie within it.
    pub size: usize,
    /// Notification title; empty for OSC 9.
    pub title: GhosttyString,
    /// Notification text, as the program sent it.
    pub body: GhosttyString,
}

/// `GhosttyTerminalProgressState` (`terminal.h`): 0 remove, 1 set, 2 error, 3 indeterminate, 4 pause.
pub type GhosttyTerminalProgressState = c_int;

/// An OSC 9;4 progress report, borrowed for the callback only; a sized struct.
#[repr(C)]
pub struct GhosttyTerminalProgressReport {
    /// Size the library filled; read only fields that lie within it.
    pub size: usize,
    /// The reported state.
    pub state: GhosttyTerminalProgressState,
    /// Percent 0–100, or -1 when the program omitted it.
    pub progress: i8,
}

/// `GhosttyClipboardLocation` (`terminal.h`).
pub type GhosttyClipboardLocation = c_int;
/// The standard clipboard (OSC 52 `c`).
pub const GHOSTTY_CLIPBOARD_LOCATION_STANDARD: GhosttyClipboardLocation = 0;
/// `GhosttyClipboardWriteResult` (`terminal.h`).
pub type GhosttyClipboardWriteResult = c_int;
/// The embedder accepted the write.
pub const GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS: GhosttyClipboardWriteResult = 0;

/// One MIME representation of a clipboard write, borrowed for the callback only.
#[repr(C)]
pub struct GhosttyClipboardContent {
    /// MIME type, e.g. `text/plain`.
    pub mime: GhosttyString,
    /// Decoded (not base64) data.
    pub data: GhosttyString,
}

/// The embedder's answer to a clipboard write; a sized struct.
#[repr(C)]
pub struct GhosttyClipboardWriteReply {
    /// Must equal `size_of::<GhosttyClipboardWriteReply>()`.
    pub size: usize,
    /// Accepted, or why not.
    pub result: GhosttyClipboardWriteResult,
    /// Remember the decision for the session; honoured only when the request's `can_remember` is true.
    pub remember: bool,
}

/// Answers a [`GhosttyClipboardWrite`]; valid only inside the clipboard-write callback, extra calls are ignored.
pub type GhosttyClipboardWriteReplyFn = Option<
    unsafe extern "C" fn(
        write: *const GhosttyClipboardWrite,
        reply: *const GhosttyClipboardWriteReply,
    ),
>;

/// A synchronous clipboard write request, borrowed for the callback only; a sized struct.
#[repr(C)]
pub struct GhosttyClipboardWrite {
    /// Size the library filled; read only fields that lie within it.
    pub size: usize,
    /// Destination clipboard.
    pub location: GhosttyClipboardLocation,
    /// `contents_len` representations of one value, to be committed together.
    pub contents: *const GhosttyClipboardContent,
    /// Number of representations; 0 means clear the destination.
    pub contents_len: usize,
    /// Name of the writing program, if the protocol carries one; empty otherwise.
    pub name: GhosttyString,
    /// The terminal already holds a session grant, so no prompt is needed.
    pub granted: bool,
    /// The reply's `remember` is honoured.
    pub can_remember: bool,
    /// Terminal-owned reply state; never read or written by the embedder.
    pub ctx: *const c_void,
    /// Answers the write; returning from the callback without calling it denies the write.
    pub reply: GhosttyClipboardWriteReplyFn,
}

/// Receives bytes for the pty (`data` valid for `len` bytes during the call); runs inside `vt_write`, must not block or re-enter.
pub type GhosttyTerminalWritePtyFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
);
/// Called once per BEL; must not re-enter the terminal.
pub type GhosttyTerminalBellFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
/// Called when OSC 0/2 changed the title; read it later with `DATA_TITLE`.
pub type GhosttyTerminalTitleChangedFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
/// Called when the pwd changed; read it later with `DATA_PWD`.
pub type GhosttyTerminalPwdChangedFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
/// Fills `*out_size` and returns true to answer a size query; false leaves the query unanswered.
pub type GhosttyTerminalSizeFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out_size: *mut GhosttySizeReportSize,
) -> bool;
/// Fills `*out_scheme` and returns true to answer `CSI ? 996 n`; false leaves it unanswered.
pub type GhosttyTerminalColorSchemeFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out_scheme: *mut GhosttyColorScheme,
) -> bool;
/// Returns the XTVERSION answer; its bytes must stay valid until the callback returns.
pub type GhosttyTerminalXtversionFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void) -> GhosttyString;
/// Receives an OSC 9 / 777 notification borrowed for the call.
pub type GhosttyTerminalDesktopNotificationFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    notification: *const GhosttyTerminalDesktopNotification,
);
/// Receives an OSC 9;4 progress report borrowed for the call.
pub type GhosttyTerminalProgressReportFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    report: *const GhosttyTerminalProgressReport,
);
/// Receives a clipboard write borrowed for the call; answer it through `write.reply` before returning.
pub type GhosttyTerminalClipboardWriteFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    write: *const GhosttyClipboardWrite,
);

/// `GhosttyRenderStateData` (`render.h`): what [`ghostty_render_state_get`] reads; the comment names the out type.
pub type GhosttyRenderStateData = c_int;
/// `uint16_t*`: columns of the copied viewport.
pub const GHOSTTY_RENDER_STATE_DATA_COLS: GhosttyRenderStateData = 1;
/// `uint16_t*`: rows of the copied viewport.
pub const GHOSTTY_RENDER_STATE_DATA_ROWS: GhosttyRenderStateData = 2;
/// [`GhosttyRenderStateDirty`]`*`: global dirty state since the last clean.
pub const GHOSTTY_RENDER_STATE_DATA_DIRTY: GhosttyRenderStateData = 3;
/// [`GhosttyRenderStateRowIterator`]`*`: (re)positions an existing iterator before the first row.
pub const GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR: GhosttyRenderStateData = 4;
/// [`GhosttyRenderStateCursor`]`*` with `size` set: the cursor.
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR: GhosttyRenderStateData = 18;

/// `GhosttyRenderStateDirty` (`render.h`).
pub type GhosttyRenderStateDirty = c_int;
/// Nothing changed since the last clean.
pub const GHOSTTY_RENDER_STATE_DIRTY_FALSE: GhosttyRenderStateDirty = 0;
/// Some rows changed; the row iterator's `next_dirty` yields them.
pub const GHOSTTY_RENDER_STATE_DIRTY_PARTIAL: GhosttyRenderStateDirty = 1;
/// Everything must be redrawn (scroll, screen switch, resize, palette change).
pub const GHOSTTY_RENDER_STATE_DIRTY_FULL: GhosttyRenderStateDirty = 2;

/// `GhosttyRenderStateRowData` (`render.h`): what [`ghostty_render_state_row_get`] reads for the current row.
pub type GhosttyRenderStateRowData = c_int;
/// [`GhosttyRow`]`*`: the row header (wrap flags).
pub const GHOSTTY_RENDER_STATE_ROW_DATA_RAW: GhosttyRenderStateRowData = 2;
/// [`GhosttyRenderStateRowCells`]`*`: positions an existing cell iterator on this row.
pub const GHOSTTY_RENDER_STATE_ROW_DATA_CELLS: GhosttyRenderStateRowData = 3;
/// [`GhosttyCellsView`]`*`: every raw cell of the row, valid until the next update.
pub const GHOSTTY_RENDER_STATE_ROW_DATA_CELLS_RAW: GhosttyRenderStateRowData = 5;

/// `GhosttyRenderStateRowCellsData` (`render.h`): what [`ghostty_render_state_row_cells_get`] reads for the selected cell.
pub type GhosttyRenderStateRowCellsData = c_int;
/// [`GhosttyStyle`]`*` with `size` set: the selected cell's style.
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE: GhosttyRenderStateRowCellsData = 2;
/// `uint32_t*`: codepoints of the cluster including the base; 0 without text.
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN: GhosttyRenderStateRowCellsData = 3;
/// `uint32_t*` buffer of at least `GRAPHEMES_LEN` entries: base codepoint first.
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF: GhosttyRenderStateRowCellsData = 4;

/// The cursor as a renderer sees it (`render.h`); a sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyRenderStateCursor {
    /// Must equal `size_of::<GhosttyRenderStateCursor>()` before a call fills it.
    pub size: usize,
    /// The cursor lies inside the viewport, so `viewport_x/y` are meaningful.
    pub viewport_has_value: bool,
    /// Viewport column, 0-based.
    pub viewport_x: u16,
    /// Viewport row, 0-based.
    pub viewport_y: u16,
    /// The cursor sits on the spacer half of a wide character.
    pub wide_tail: bool,
    /// DECTCEM says draw it.
    pub visible: bool,
    /// The program asked for a blinking cursor.
    pub blinking: bool,
    /// The terminal believes a password is being typed (echo off).
    pub password_input: bool,
    /// `GhosttyRenderStateCursorVisualStyle`: 0 bar, 1 block, 2 underline, 3 hollow block.
    pub visual_style: c_int,
}

/// A borrowed array of raw cells (`render.h`), valid until the render state's next update.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyCellsView {
    /// First cell; may be null when `len` is 0.
    pub ptr: *const GhosttyCell,
    /// Number of cells, the row's width.
    pub len: usize,
}

/// `GhosttyCellData` (`screen.h`): what [`ghostty_cell_get`] reads from a raw cell; the comment names the out type.
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

/// `GhosttyCellContentTag` (`screen.h`): what a cell holds.
pub type GhosttyCellContentTag = c_int;
/// A single codepoint, or none (0).
pub const GHOSTTY_CELL_CONTENT_CODEPOINT: GhosttyCellContentTag = 0;
/// A base codepoint plus extra grapheme codepoints.
pub const GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME: GhosttyCellContentTag = 1;
/// No text; a palette background (an erase with a background colour).
pub const GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE: GhosttyCellContentTag = 2;
/// No text; an RGB background.
pub const GHOSTTY_CELL_CONTENT_BG_COLOR_RGB: GhosttyCellContentTag = 3;

/// `GhosttyCellWide` (`screen.h`): the cell's part of a character.
pub type GhosttyCellWide = c_int;
/// A one-column character or an empty cell.
pub const GHOSTTY_CELL_WIDE_NARROW: GhosttyCellWide = 0;
/// First half of a two-column character.
pub const GHOSTTY_CELL_WIDE_WIDE: GhosttyCellWide = 1;
/// Second half of a wide character; draw nothing.
pub const GHOSTTY_CELL_WIDE_SPACER_TAIL: GhosttyCellWide = 2;
/// Row-end padding where a wide character wrapped to the next row; draw nothing.
pub const GHOSTTY_CELL_WIDE_SPACER_HEAD: GhosttyCellWide = 3;

/// `GhosttyRowData` (`screen.h`): what [`ghostty_row_get`] reads from a raw row header.
pub type GhosttyRowData = c_int;
/// `bool*`: the row soft-wraps onto the next.
pub const GHOSTTY_ROW_DATA_WRAP: GhosttyRowData = 1;

/// `GhosttyPointTag` (`point.h`): the coordinate space of a [`GhosttyPoint`].
pub type GhosttyPointTag = c_int;
/// The active area (the live screen), y 0 at its top.
pub const GHOSTTY_POINT_TAG_ACTIVE: GhosttyPointTag = 0;
/// Scrollback plus active area; y 0 is the oldest scrollback row.
pub const GHOSTTY_POINT_TAG_SCREEN: GhosttyPointTag = 2;
/// Scrollback only; y 0 is the oldest scrollback row.
pub const GHOSTTY_POINT_TAG_HISTORY: GhosttyPointTag = 3;

/// A cell coordinate (`point.h`), interpreted by the point's tag.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyPointCoordinate {
    /// 0-based column.
    pub x: u16,
    /// 0-based row in the tag's space.
    pub y: u32,
}

/// Payload of [`GhosttyPoint`].
#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyPointValue {
    /// The coordinate, for every tag ply uses.
    pub coordinate: GhosttyPointCoordinate,
    /// Keeps the union 16 bytes wide as in C; never read.
    pub _padding: [u64; 2],
}

/// A point in one coordinate space (`point.h`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyPoint {
    /// Coordinate space of `value`.
    pub tag: GhosttyPointTag,
    /// The coordinate.
    pub value: GhosttyPointValue,
}

/// An untracked reference to one cell (`grid_ref.h`), valid until the next mutating terminal call; a sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyGridRef {
    /// Must equal `size_of::<GhosttyGridRef>()` before a call fills it.
    pub size: usize,
    /// Page node; opaque to the embedder.
    pub node: *mut c_void,
    /// Column within the page.
    pub x: u16,
    /// Row within the page, not a screen row.
    pub y: u16,
}

/// A selection between two grid references (`selection.h`), used for search matches; a sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttySelection {
    /// Must equal `size_of::<GhosttySelection>()`.
    pub size: usize,
    /// First cell (inclusive); may come after `end` in terminal order.
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
    /// Destination for `cap` entries; null with `cap` 0 queries the size.
    pub ptr: *mut GhosttySelection,
    /// Capacity in entries.
    pub cap: usize,
    /// Entries written, or the capacity needed on OUT_OF_SPACE.
    pub len: usize,
}

/// `GhosttySearchOption` (`search.h`).
pub type GhosttySearchOption = c_int;
/// [`GhosttyString`]`*`: the needle, copied; ASCII letters match case-insensitively, other bytes exactly.
pub const GHOSTTY_SEARCH_OPT_NEEDLE: GhosttySearchOption = 0;
/// `GhosttySearchScroll*`: scroll policy of the select options.
pub const GHOSTTY_SEARCH_OPT_SELECT_SCROLL: GhosttySearchOption = 3;
/// `GhosttySearchData` (`search.h`).
pub type GhosttySearchData = c_int;
/// `size_t*`: matches found on the active screen.
pub const GHOSTTY_SEARCH_DATA_TOTAL_MATCHES: GhosttySearchData = 2;
/// [`GhosttySelectionBuffer`]`*`: every match, newest to oldest, valid until the terminal next changes.
pub const GHOSTTY_SEARCH_DATA_MATCHES: GhosttySearchData = 5;
/// `GhosttySearchScroll` value: select options never scroll the viewport.
pub const GHOSTTY_SEARCH_SCROLL_NONE: c_int = 1;

/// `GhosttyKeyAction` (`key/event.h`) 0 release, 1 press, 2 repeat, numbered as `ply_proto::data::KeyAction`; other values are UB.
pub type GhosttyKeyAction = c_int;
/// `GhosttyKey` (`key/event.h`): a physical key; only 0..=175 are defined at this pin and any other value is UB.
pub type GhosttyKey = c_int;
/// The key has no identity; the event's text is sent as is.
pub const GHOSTTY_KEY_UNIDENTIFIED: GhosttyKey = 0;
/// Return / Enter.
pub const GHOSTTY_KEY_ENTER: GhosttyKey = 58;
/// The highest defined [`GhosttyKey`] at this pin.
pub const GHOSTTY_KEY_MAX: GhosttyKey = 175;
/// `GhosttyKeyEncoderOption` (`key/encoder.h`).
pub type GhosttyKeyEncoderOption = c_int;
/// `GhosttyOptionAsAlt*`: which ⌥ acts as Alt; reset by every `setopt_from_terminal`.
pub const GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT: GhosttyKeyEncoderOption = 6;
/// `GhosttyOptionAsAlt` (`key/encoder.h`).
pub type GhosttyOptionAsAlt = c_int;
/// ⌥ types layout characters.
pub const GHOSTTY_OPTION_AS_ALT_FALSE: GhosttyOptionAsAlt = 0;
/// Both ⌥ keys act as Alt.
pub const GHOSTTY_OPTION_AS_ALT_TRUE: GhosttyOptionAsAlt = 1;
/// Only the left ⌥ acts as Alt.
pub const GHOSTTY_OPTION_AS_ALT_LEFT: GhosttyOptionAsAlt = 2;
/// Only the right ⌥ acts as Alt.
pub const GHOSTTY_OPTION_AS_ALT_RIGHT: GhosttyOptionAsAlt = 3;

/// `GhosttyMouseAction` (`mouse/event.h`) 0 press, 1 release, 2 motion, numbered as `ply_proto::data::MouseAction`; other values are UB.
pub type GhosttyMouseAction = c_int;
/// `GhosttyMouseButton` (`mouse/event.h`) 1–11, numbered as `ply_proto::data::MouseButton`; other values are UB.
pub type GhosttyMouseButton = c_int;
/// A pointer position in surface pixels (`mouse/event.h`); must be finite and within `i32` range after scaling.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyMousePosition {
    /// Pixels from the surface's left edge.
    pub x: f32,
    /// Pixels from the surface's top edge.
    pub y: f32,
}
/// `GhosttyMouseEncoderOption` (`mouse/encoder.h`).
pub type GhosttyMouseEncoderOption = c_int;
/// [`GhosttyMouseEncoderSize`]`*`: surface and cell geometry; cell sizes must be non-zero.
pub const GHOSTTY_MOUSE_ENCODER_OPT_SIZE: GhosttyMouseEncoderOption = 2;
/// `bool*`: some button is held (drag reporting under mode 1002).
pub const GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED: GhosttyMouseEncoderOption = 3;
/// `bool*`: drop motion events that stay in the last reported cell.
pub const GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL: GhosttyMouseEncoderOption = 4;

/// Surface geometry for the mouse encoder (`mouse/encoder.h`); a sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyMouseEncoderSize {
    /// Must equal `size_of::<GhosttyMouseEncoderSize>()`.
    pub size: usize,
    /// Surface width in pixels, padding included.
    pub screen_width: u32,
    /// Surface height in pixels, padding included.
    pub screen_height: u32,
    /// Cell width in pixels, non-zero.
    pub cell_width: u32,
    /// Cell height in pixels, non-zero.
    pub cell_height: u32,
    /// Pixels above the grid.
    pub padding_top: u32,
    /// Pixels below the grid.
    pub padding_bottom: u32,
    /// Pixels right of the grid.
    pub padding_right: u32,
    /// Pixels left of the grid.
    pub padding_left: u32,
}

/// `GhosttyFocusEvent` (`focus.h`).
pub type GhosttyFocusEvent = c_int;
/// Focus gained, encoded `CSI I`.
pub const GHOSTTY_FOCUS_GAINED: GhosttyFocusEvent = 0;
/// Focus lost, encoded `CSI O`.
pub const GHOSTTY_FOCUS_LOST: GhosttyFocusEvent = 1;

/// Receives `len` bytes at `data` (`io.h`); return false to fail the operation with IO_ERROR.
pub type GhosttyWriterFn =
    Option<unsafe extern "C" fn(userdata: *mut c_void, data: *const u8, len: usize) -> bool>;
/// A byte sink (`io.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyWriter {
    /// Called with each piece, in order.
    pub write: GhosttyWriterFn,
    /// Passed back to `write`, owned by the caller.
    pub userdata: *mut c_void,
}
/// Writes the representation `mime` into `writer` (`io.h`); return false to fail the paste with IO_ERROR.
pub type GhosttyMimeReaderFn = Option<
    unsafe extern "C" fn(userdata: *mut c_void, mime: GhosttyString, writer: GhosttyWriter) -> bool,
>;
/// A source of MIME representations (`io.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyMimeReader {
    /// Called at most once per paste.
    pub read: GhosttyMimeReaderFn,
    /// Passed back to `read`, owned by the caller.
    pub userdata: *mut c_void,
}

/// `GhosttyPasteSource` (`paste.h`).
pub type GhosttyPasteSource = c_int;
/// The user pasted from a clipboard (⌘V, menu).
pub const GHOSTTY_PASTE_SOURCE_CLIPBOARD: GhosttyPasteSource = 0;

/// A paste request (`paste.h`), borrowed for the call; a sized struct.
#[repr(C)]
pub struct GhosttyPaste {
    /// Must equal `size_of::<GhosttyPaste>()`.
    pub size: usize,
    /// Clipboard the contents came from.
    pub location: GhosttyClipboardLocation,
    /// Why the paste happened.
    pub source: GhosttyPasteSource,
    /// `mimes_len` MIME types available, preferred first; null only when `mimes_len` is 0.
    pub mimes: *const GhosttyString,
    /// Entries in `mimes`.
    pub mimes_len: usize,
    /// Produces the representation chosen; required when `mimes_len` is non-zero.
    pub reader: GhosttyMimeReader,
    /// Write text that could inject commands instead of returning REJECTED.
    pub allow_unsafe: bool,
}

/// `GhosttyFormatterFormat` (`formatter.h`).
pub type GhosttyFormatterFormat = c_int;
/// Plain text, no escape sequences.
pub const GHOSTTY_FORMATTER_FORMAT_PLAIN: GhosttyFormatterFormat = 0;

/// Screen state a VT-format output restores (`formatter.h`); all false for plain text; a sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyFormatterScreenExtra {
    /// Must equal `size_of::<GhosttyFormatterScreenExtra>()`.
    pub size: usize,
    /// Emit the cursor position.
    pub cursor: bool,
    /// Emit SGR styles.
    pub style: bool,
    /// Emit OSC 8 hyperlinks.
    pub hyperlink: bool,
    /// Emit character protection.
    pub protection: bool,
    /// Emit the Kitty keyboard flags.
    pub kitty_keyboard: bool,
    /// Emit character sets.
    pub charsets: bool,
}

/// Terminal state a VT-format output restores (`formatter.h`); all false for plain text; a sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyFormatterTerminalExtra {
    /// Must equal `size_of::<GhosttyFormatterTerminalExtra>()`.
    pub size: usize,
    /// Emit palette changes.
    pub palette: bool,
    /// Emit mode changes.
    pub modes: bool,
    /// Emit the scrolling region.
    pub scrolling_region: bool,
    /// Emit tab stops.
    pub tabstops: bool,
    /// Emit the pwd.
    pub pwd: bool,
    /// Emit keyboard modes.
    pub keyboard: bool,
    /// Screen-level extras.
    pub screen: GhosttyFormatterScreenExtra,
}

/// Options of a terminal formatter (`formatter.h`), passed by value; a sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyFormatterTerminalOptions {
    /// Must equal `size_of::<GhosttyFormatterTerminalOptions>()`.
    pub size: usize,
    /// Output format.
    pub emit: GhosttyFormatterFormat,
    /// Join soft-wrapped rows into one line.
    pub unwrap: bool,
    /// Drop trailing whitespace on non-blank lines.
    pub trim: bool,
    /// State to restore in VT output.
    pub extra: GhosttyFormatterTerminalExtra,
    /// Restricts output to a range; null formats the whole active screen with its scrollback.
    pub selection: *const GhosttySelection,
}

/// `GhosttySysOption` (`sys.h`): process-wide settings.
pub type GhosttySysOption = c_int;
/// [`GhosttySysLogFn`]: receives the library's log lines; set before any other call.
pub const GHOSTTY_SYS_OPT_LOG: GhosttySysOption = 2;
/// `GhosttySysLogLevel` (`sys.h`): 0 error, 1 warning, 2 info, 3 debug.
pub type GhosttySysLogLevel = c_int;
/// Receives one log line (`scope` and `message` valid for their lengths during the call); may run on any calling thread.
pub type GhosttySysLogFn = unsafe extern "C" fn(
    userdata: *mut c_void,
    level: GhosttySysLogLevel,
    scope: *const u8,
    scope_len: usize,
    message: *const u8,
    message_len: usize,
);

/// `ghostty_mode_new` from `modes.h`, a `static inline` in C: `value` masked to 15 bits, bit 15 set for ANSI modes.
pub const fn ghostty_mode(value: u16, ansi: bool) -> GhosttyMode {
    (value & 0x7FFF) | ((ansi as u16) << 15)
}
/// DEC private mode 25, cursor visible (DECTCEM).
pub const GHOSTTY_MODE_CURSOR_VISIBLE: GhosttyMode = ghostty_mode(25, false);
/// DEC private mode 1004, focus in/out reports.
pub const GHOSTTY_MODE_FOCUS_EVENT: GhosttyMode = ghostty_mode(1004, false);
/// DEC private mode 2004, bracketed paste.
pub const GHOSTTY_MODE_BRACKETED_PASTE: GhosttyMode = ghostty_mode(2004, false);
/// DEC private mode 2026, synchronized output.
pub const GHOSTTY_MODE_SYNC_OUTPUT: GhosttyMode = ghostty_mode(2026, false);
/// DEC private mode 2027, grapheme clustering.
pub const GHOSTTY_MODE_GRAPHEME_CLUSTER: GhosttyMode = ghostty_mode(2027, false);

unsafe extern "C" {
    /// The library's ABI manifest (`types.h`): a NUL-terminated JSON string in static storage, never freed.
    pub fn ghostty_type_json() -> *const c_char;
    /// Frees `len` bytes at `ptr` that the library allocated with `allocator` (null = default); `ptr` may be null.
    pub fn ghostty_free(allocator: *const GhosttyAllocator, ptr: *mut u8, len: usize);
    /// Sets a process-wide option (`sys.h`); `value` has the option's type; not synchronized with other calls.
    pub fn ghostty_sys_set(option: GhosttySysOption, value: *const c_void) -> GhosttyResult;

    /// Creates a `cols` × `rows` terminal into `*terminal` (writable); both sizes non-zero; free with [`ghostty_terminal_free`].
    pub fn ghostty_terminal_new(
        allocator: *const GhosttyAllocator,
        terminal: *mut GhosttyTerminal,
        cols: u16,
        rows: u16,
    ) -> GhosttyResult;
    /// Frees a terminal and its screens; null is ignored; everything created from it must be freed first.
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    /// Resizes a live terminal (sizes non-zero): the primary screen reflows, DEC 2026 ends, a mode 2048 report may be written.
    pub fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> GhosttyResult;
    /// Sets an option on a live terminal; `value` points to the option's documented type (or is a callback) and is read during the call.
    pub fn ghostty_terminal_set(
        terminal: GhosttyTerminal,
        option: GhosttyTerminalOption,
        value: *const c_void,
    ) -> GhosttyResult;
    /// Reads `data` of a live terminal into `out`, which must be writable as the data kind's documented type.
    pub fn ghostty_terminal_get(
        terminal: GhosttyTerminal,
        data: GhosttyTerminalData,
        out: *mut c_void,
    ) -> GhosttyResult;
    /// Feeds `len` bytes at `data` (readable) to a live terminal; never fails; callbacks run inside and must not re-enter it.
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);
    /// Writes into `*out_activity` a token that changes whenever compressible content changes.
    pub fn ghostty_terminal_compression_activity(
        terminal: GhosttyTerminal,
        out_activity: *mut u64,
    ) -> GhosttyResult;
    /// Runs one compression step over idle scrollback and writes the outcome to `*out_result`.
    pub fn ghostty_terminal_compress(
        terminal: GhosttyTerminal,
        mode: GhosttyTerminalCompressionMode,
        out_result: *mut GhosttyTerminalCompressionResult,
    ) -> GhosttyResult;
    /// Resolves `point` to an untracked grid reference in `*out_ref` (its `size` set by the caller); fails outside the grid.
    pub fn ghostty_terminal_grid_ref(
        terminal: GhosttyTerminal,
        point: GhosttyPoint,
        out_ref: *mut GhosttyGridRef,
    ) -> GhosttyResult;
    /// Converts a still-valid grid reference to a coordinate of the `tag` space in `*out`; fails when the cell lies outside it.
    pub fn ghostty_terminal_point_from_grid_ref(
        terminal: GhosttyTerminal,
        grid_ref: *const GhosttyGridRef,
        tag: GhosttyPointTag,
        out: *mut GhosttyPointCoordinate,
    ) -> GhosttyResult;
    /// Creates a tracked reference to `point` of the active screen into `*out_ref`; INVALID_VALUE outside the grid; free with [`ghostty_tracked_grid_ref_free`].
    pub fn ghostty_terminal_grid_ref_track(
        terminal: GhosttyTerminal,
        point: GhosttyPoint,
        out_ref: *mut GhosttyTrackedGridRef,
    ) -> GhosttyResult;
    /// Frees a tracked reference; null is ignored; valid after its terminal was freed.
    pub fn ghostty_tracked_grid_ref_free(grid_ref: GhosttyTrackedGridRef);
    /// Writes the tracked cell's coordinate in the `tag` space of the screen that owns it to `*out_point`; NO_VALUE once the cell was discarded.
    pub fn ghostty_tracked_grid_ref_point(
        grid_ref: GhosttyTrackedGridRef,
        tag: GhosttyPointTag,
        out_point: *mut GhosttyPointCoordinate,
    ) -> GhosttyResult;
    /// Moves a tracked reference of `terminal` to `point` of the active screen, clearing a lost state; unchanged on OUT_OF_MEMORY.
    pub fn ghostty_tracked_grid_ref_set(
        grid_ref: GhosttyTrackedGridRef,
        terminal: GhosttyTerminal,
        point: GhosttyPoint,
    ) -> GhosttyResult;
    /// Pastes per the live modes through the write-pty callback (required); REJECTED for unsafe text; `*out_written` may be null.
    pub fn ghostty_terminal_paste(
        terminal: GhosttyTerminal,
        paste: *const GhosttyPaste,
        out_written: *mut bool,
    ) -> GhosttyResult;

    /// Creates an empty render state into `*state`; free with [`ghostty_render_state_free`].
    pub fn ghostty_render_state_new(
        allocator: *const GhosttyAllocator,
        state: *mut GhosttyRenderState,
    ) -> GhosttyResult;
    /// Frees a render state; null is ignored.
    pub fn ghostty_render_state_free(state: GhosttyRenderState);
    /// Copies a live terminal's viewport into the state and consumes the terminal's dirty flags; needs exclusive terminal access.
    pub fn ghostty_render_state_update(
        state: GhosttyRenderState,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    /// Clears the global and every per-row dirty flag of the state.
    pub fn ghostty_render_state_clean(state: GhosttyRenderState) -> GhosttyResult;
    /// Reads `data` of the state into `out`, writable as the data kind's documented type.
    pub fn ghostty_render_state_get(
        state: GhosttyRenderState,
        data: GhosttyRenderStateData,
        out: *mut c_void,
    ) -> GhosttyResult;
    /// Creates a row iterator into `*out_iterator`; free with [`ghostty_render_state_row_iterator_free`].
    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const GhosttyAllocator,
        out_iterator: *mut GhosttyRenderStateRowIterator,
    ) -> GhosttyResult;
    /// Frees a row iterator; null is ignored.
    pub fn ghostty_render_state_row_iterator_free(iterator: GhosttyRenderStateRowIterator);
    /// Advances a positioned iterator to its next row; false past the last.
    pub fn ghostty_render_state_row_iterator_next(iterator: GhosttyRenderStateRowIterator) -> bool;
    /// Advances a positioned iterator to its next dirty row and writes its index to `*out_y`; false past the last.
    pub fn ghostty_render_state_row_iterator_next_dirty(
        iterator: GhosttyRenderStateRowIterator,
        out_y: *mut u16,
    ) -> bool;
    /// Reads `data` of the iterator's current row into `out`, writable as the data kind's documented type.
    pub fn ghostty_render_state_row_get(
        iterator: GhosttyRenderStateRowIterator,
        data: GhosttyRenderStateRowData,
        out: *mut c_void,
    ) -> GhosttyResult;
    /// Creates a cell iterator into `*out_cells`; free with [`ghostty_render_state_row_cells_free`].
    pub fn ghostty_render_state_row_cells_new(
        allocator: *const GhosttyAllocator,
        out_cells: *mut GhosttyRenderStateRowCells,
    ) -> GhosttyResult;
    /// Selects column `x` (less than the row's width) of the positioned row.
    pub fn ghostty_render_state_row_cells_select(
        cells: GhosttyRenderStateRowCells,
        x: u16,
    ) -> GhosttyResult;
    /// Reads `data` of the selected cell into `out`, writable as the data kind's documented type.
    pub fn ghostty_render_state_row_cells_get(
        cells: GhosttyRenderStateRowCells,
        data: GhosttyRenderStateRowCellsData,
        out: *mut c_void,
    ) -> GhosttyResult;
    /// Frees a cell iterator; null is ignored.
    pub fn ghostty_render_state_row_cells_free(cells: GhosttyRenderStateRowCells);

    /// Reads `data` of a packed cell into `out`, writable as the data kind's documented type (`screen.h`).
    pub fn ghostty_cell_get(
        cell: GhosttyCell,
        data: GhosttyCellData,
        out: *mut c_void,
    ) -> GhosttyResult;
    /// Reads `data` of a packed row header into `out`, writable as the data kind's documented type.
    pub fn ghostty_row_get(
        row: GhosttyRow,
        data: GhosttyRowData,
        out: *mut c_void,
    ) -> GhosttyResult;

    /// Writes the packed cell at a still-valid grid reference to `*out_cell` (`grid_ref.h`).
    pub fn ghostty_grid_ref_cell(
        grid_ref: *const GhosttyGridRef,
        out_cell: *mut GhosttyCell,
    ) -> GhosttyResult;
    /// Writes the packed row header at a still-valid grid reference to `*out_row`.
    pub fn ghostty_grid_ref_row(
        grid_ref: *const GhosttyGridRef,
        out_row: *mut GhosttyRow,
    ) -> GhosttyResult;
    /// Writes the cluster's codepoints (base first) into `buf` (`buf_len` slots); OUT_OF_SPACE stores the count needed in `*out_len`.
    pub fn ghostty_grid_ref_graphemes(
        grid_ref: *const GhosttyGridRef,
        buf: *mut u32,
        buf_len: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;
    /// Writes the style of the cell at a still-valid grid reference into `*out_style` (its `size` set by the caller).
    pub fn ghostty_grid_ref_style(
        grid_ref: *const GhosttyGridRef,
        out_style: *mut GhosttyStyle,
    ) -> GhosttyResult;

    /// Encodes a live terminal's full state (`snapshot.h`) into `*out_ptr`/`*out_len`, library memory freed with [`ghostty_free`].
    pub fn ghostty_snapshot_encode_alloc(
        terminal: GhosttyTerminal,
        allocator: *const GhosttyAllocator,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> GhosttyResult;
    /// Creates a decoder over `len` bytes at `ptr`, which must stay valid and unchanged until the decoder is freed.
    pub fn ghostty_snapshot_decoder_new_buf(
        allocator: *const GhosttyAllocator,
        decoder: *mut GhosttySnapshotDecoder,
        ptr: *const u8,
        len: usize,
    ) -> GhosttyResult;
    /// Sets a decoder option before decoding starts; `value` points to the option's documented type.
    pub fn ghostty_snapshot_decoder_set(
        decoder: GhosttySnapshotDecoder,
        option: GhosttySnapshotDecoderOption,
        value: *const c_void,
    ) -> GhosttyResult;
    /// Frees a decoder; null is ignored; a terminal it produced stays valid and is owned by the caller.
    pub fn ghostty_snapshot_decoder_free(decoder: GhosttySnapshotDecoder);
    /// Decodes the whole snapshot into a new terminal in `*terminal`, owned by the caller; fails on any malformed input.
    pub fn ghostty_snapshot_decoder_decode(
        decoder: GhosttySnapshotDecoder,
        terminal: *mut GhosttyTerminal,
    ) -> GhosttyResult;

    /// Creates a search over a live terminal into `*out_search`; free it with [`ghostty_search_free`] before the terminal.
    pub fn ghostty_search_new(
        allocator: *const GhosttyAllocator,
        out_search: *mut GhosttySearch,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    /// Frees a search; null is ignored.
    pub fn ghostty_search_free(search: GhosttySearch);
    /// Feeds the whole terminal and runs the search to completion; blocks for the scrollback's size.
    pub fn ghostty_search_run(search: GhosttySearch) -> GhosttyResult;
    /// Sets a search option; `value` points to the option's documented type (copied where the option says so).
    pub fn ghostty_search_set(
        search: GhosttySearch,
        option: GhosttySearchOption,
        value: *const c_void,
    ) -> GhosttyResult;
    /// Reads `data` of a search into `value`, writable as the data kind's documented type.
    pub fn ghostty_search_get(
        search: GhosttySearch,
        data: GhosttySearchData,
        value: *mut c_void,
    ) -> GhosttyResult;

    /// Creates a key encoder into `*encoder` (`key/encoder.h`); free with [`ghostty_key_encoder_free`].
    pub fn ghostty_key_encoder_new(
        allocator: *const GhosttyAllocator,
        encoder: *mut GhosttyKeyEncoder,
    ) -> GhosttyResult;
    /// Frees a key encoder; null is ignored.
    pub fn ghostty_key_encoder_free(encoder: GhosttyKeyEncoder);
    /// Sets one encoder option; `value` points to the option's documented type.
    pub fn ghostty_key_encoder_setopt(
        encoder: GhosttyKeyEncoder,
        option: GhosttyKeyEncoderOption,
        value: *const c_void,
    );
    /// Copies a live terminal's key-related modes (DECCKM, keypad, kitty flags) into the encoder and resets option-as-alt.
    pub fn ghostty_key_encoder_setopt_from_terminal(
        encoder: GhosttyKeyEncoder,
        terminal: GhosttyTerminal,
    );
    /// Encodes `event` into `out_buf` (`out_buf_size` bytes); OUT_OF_SPACE stores the size needed in `*out_len`.
    pub fn ghostty_key_encoder_encode(
        encoder: GhosttyKeyEncoder,
        event: GhosttyKeyEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;
    /// Creates a key event into `*event` (`key/event.h`); free with [`ghostty_key_event_free`].
    pub fn ghostty_key_event_new(
        allocator: *const GhosttyAllocator,
        event: *mut GhosttyKeyEvent,
    ) -> GhosttyResult;
    /// Frees a key event; null is ignored.
    pub fn ghostty_key_event_free(event: GhosttyKeyEvent);
    /// Sets press, repeat or release; `action` must be 0..=2.
    pub fn ghostty_key_event_set_action(event: GhosttyKeyEvent, action: GhosttyKeyAction);
    /// Sets the physical key; `key` must be 0..=[`GHOSTTY_KEY_MAX`], any other value is undefined behaviour.
    pub fn ghostty_key_event_set_key(event: GhosttyKeyEvent, key: GhosttyKey);
    /// Sets the modifiers held; bits 10–15 must be zero.
    pub fn ghostty_key_event_set_mods(event: GhosttyKeyEvent, mods: GhosttyMods);
    /// Sets the modifiers the layout consumed to produce the text; bits 10–15 must be zero.
    pub fn ghostty_key_event_set_consumed_mods(event: GhosttyKeyEvent, consumed_mods: GhosttyMods);
    /// Marks an IME composition in progress (such events encode nothing).
    pub fn ghostty_key_event_set_composing(event: GhosttyKeyEvent, composing: bool);
    /// Sets the text (`len` UTF-8 bytes at `utf8`), borrowed and read at the next encode.
    pub fn ghostty_key_event_set_utf8(event: GhosttyKeyEvent, utf8: *const c_char, len: usize);
    /// Sets the key's codepoint without Shift, truncated to 21 bits; 0 when unknown.
    pub fn ghostty_key_event_set_unshifted_codepoint(event: GhosttyKeyEvent, codepoint: u32);

    /// Creates a mouse encoder into `*encoder` (`mouse/encoder.h`); free with [`ghostty_mouse_encoder_free`].
    pub fn ghostty_mouse_encoder_new(
        allocator: *const GhosttyAllocator,
        encoder: *mut GhosttyMouseEncoder,
    ) -> GhosttyResult;
    /// Frees a mouse encoder; null is ignored.
    pub fn ghostty_mouse_encoder_free(encoder: GhosttyMouseEncoder);
    /// Sets one encoder option; `value` points to the option's documented type.
    pub fn ghostty_mouse_encoder_setopt(
        encoder: GhosttyMouseEncoder,
        option: GhosttyMouseEncoderOption,
        value: *const c_void,
    );
    /// Copies a live terminal's mouse tracking mode and format into the encoder.
    pub fn ghostty_mouse_encoder_setopt_from_terminal(
        encoder: GhosttyMouseEncoder,
        terminal: GhosttyTerminal,
    );
    /// Forgets the last reported cell, so the next motion is reported even in the same cell.
    pub fn ghostty_mouse_encoder_reset(encoder: GhosttyMouseEncoder);
    /// Encodes `event` into `out_buf` (`out_buf_size` bytes); SUCCESS with length 0 when the mode reports nothing.
    pub fn ghostty_mouse_encoder_encode(
        encoder: GhosttyMouseEncoder,
        event: GhosttyMouseEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;
    /// Creates a mouse event into `*event` (`mouse/event.h`); free with [`ghostty_mouse_event_free`].
    pub fn ghostty_mouse_event_new(
        allocator: *const GhosttyAllocator,
        event: *mut GhosttyMouseEvent,
    ) -> GhosttyResult;
    /// Frees a mouse event; null is ignored.
    pub fn ghostty_mouse_event_free(event: GhosttyMouseEvent);
    /// Sets press, release or motion; `action` must be 0..=2.
    pub fn ghostty_mouse_event_set_action(event: GhosttyMouseEvent, action: GhosttyMouseAction);
    /// Sets the button involved; `button` must be 1..=11 (use [`ghostty_mouse_event_clear_button`] for none).
    pub fn ghostty_mouse_event_set_button(event: GhosttyMouseEvent, button: GhosttyMouseButton);
    /// Marks the event as having no button (plain motion).
    pub fn ghostty_mouse_event_clear_button(event: GhosttyMouseEvent);
    /// Sets the modifiers held; bits 10–15 must be zero.
    pub fn ghostty_mouse_event_set_mods(event: GhosttyMouseEvent, mods: GhosttyMods);
    /// Sets the pointer position; both coordinates finite and small enough that pixel and cell math fits `i32`.
    pub fn ghostty_mouse_event_set_position(
        event: GhosttyMouseEvent,
        position: GhosttyMousePosition,
    );

    /// Encodes a focus report into `buf` (`buf_len` bytes, `focus.h`); OUT_OF_SPACE stores the size needed in `*out_written`.
    pub fn ghostty_focus_encode(
        event: GhosttyFocusEvent,
        buf: *mut c_char,
        buf_len: usize,
        out_written: *mut usize,
    ) -> GhosttyResult;

    /// Creates a formatter over a live terminal into `*formatter` (`formatter.h`); free it before the terminal.
    pub fn ghostty_formatter_terminal_new(
        allocator: *const GhosttyAllocator,
        formatter: *mut GhosttyFormatter,
        terminal: GhosttyTerminal,
        options: GhosttyFormatterTerminalOptions,
    ) -> GhosttyResult;
    /// Formats into library memory at `*out_ptr`/`*out_len`, freed with [`ghostty_free`]; reads the terminal, which must not change meanwhile.
    pub fn ghostty_formatter_format_alloc(
        formatter: GhosttyFormatter,
        allocator: *const GhosttyAllocator,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> GhosttyResult;
    /// Frees a formatter; null is ignored.
    pub fn ghostty_formatter_free(formatter: GhosttyFormatter);
}
