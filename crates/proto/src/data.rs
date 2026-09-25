//! C2 screen data (spec 4.2 with ADR-0005's corrections, Rulings R20/R21): binary frames over `run/data.sock`.
//!
//! A frame is `len:u32 LE · kind:u8 · payload[len]` with `len <= MAX_FRAME_LEN`; every integer is little-endian,
//! every bool is one byte that must be 0 or 1, and text is UTF-8 filling the rest of the payload. There is no
//! serialisation crate: [`Frame::encode`] and [`Frame::decode`] are the layout, and `app/src/terminal/frames.ts`
//! must match them byte for byte. Decoding is strict: unknown kinds, out-of-range values, unknown flag bits,
//! truncated payloads and trailing bytes are errors (INV-10).
//!
//! Colours stay symbolic ([`Color`]); the app resolves them against its theme. Rows are indexed from the top of the
//! live screen (`0..rows`); scrollback rows have negative indexes, `-1` being the newest line above the screen.
//!
//! ```
//! use ply_proto::data::{Ack, Frame, FrameReader};
//!
//! let mut wire = Vec::new();
//! Frame::Ack(Ack { seq: 42 }).encode(&mut wire).expect("fits");
//! let mut reader = FrameReader::new(wire.as_slice());
//! assert_eq!(reader.read_frame().expect("valid"), Some(Frame::Ack(Ack { seq: 42 })));
//! assert_eq!(reader.read_frame().expect("clean end"), None);
//! ```

use std::io::{ErrorKind, Read};
use std::ops::BitOr;

use crate::error::{Error, Result};

/// Largest payload of one frame in bytes (1 MiB); the reader refuses a larger header before allocating.
pub const MAX_FRAME_LEN: usize = 1 << 20;

/// Bytes before the payload: `len:u32` and `kind:u8`.
pub const HEADER_LEN: usize = 5;

/// Most rows one FETCH_HISTORY may request.
pub const MAX_HISTORY_ROWS: u16 = 1000;

/// Highest libghostty-vt `GhosttyKey` value (`key/event.h`, 176 keys at ghostty 44f2a44).
pub const MAX_KEY_CODE: u16 = 175;

/// Highest Unicode scalar value; codepoints above it are rejected.
pub const MAX_CODEPOINT: u32 = 0x10_FFFF;

/// Kind bytes: 0x10–0x1F travel client → plyd, 0x20–0x2F plyd → client.
pub mod kind {
    /// Client → plyd: attach to a pane.
    pub const ATTACH: u8 = 0x10;
    /// Client → plyd: bytes written to the pty as is (`pane.answer` digits, programmatic input).
    pub const INPUT_RAW: u8 = 0x11;
    /// Client → plyd: new grid size.
    pub const RESIZE: u8 = 0x12;
    /// Client → plyd: request scrollback rows.
    pub const FETCH_HISTORY: u8 = 0x13;
    /// Client → plyd: acknowledge a Snapshot or Delta.
    pub const ACK: u8 = 0x14;
    /// Client → plyd: a key event, encoded by plyd.
    pub const KEY: u8 = 0x15;
    /// Client → plyd: a mouse event, encoded by plyd.
    pub const MOUSE: u8 = 0x16;
    /// Client → plyd: pasted text.
    pub const PASTE: u8 = 0x17;
    /// Client → plyd: focus gained or lost.
    pub const FOCUS: u8 = 0x18;
    /// plyd → client: the whole screen.
    pub const SNAPSHOT: u8 = 0x20;
    /// plyd → client: changed rows since the last frame.
    pub const DELTA: u8 = 0x21;
    /// plyd → client: scrollback rows answering FETCH_HISTORY.
    pub const HISTORY: u8 = 0x22;
    /// plyd → client: the terminal title changed.
    pub const TITLE: u8 = 0x23;
    /// plyd → client: BEL.
    pub const BELL: u8 = 0x24;
    /// plyd → client: the process exited.
    pub const EXIT: u8 = 0x25;
    /// plyd → client: a PASTE was refused as unsafe (Ruling R21).
    pub const PASTE_REJECTED: u8 = 0x26;
    /// plyd → client: ATTACH was refused; plyd closes the connection after it.
    pub const ATTACH_REFUSED: u8 = 0x27;
}

macro_rules! flag_set {
    ($(#[$doc:meta])* $name:ident: $repr:ty { $($(#[$fdoc:meta])* $flag:ident = $bit:expr;)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        pub struct $name($repr);

        impl $name {
            $($(#[$fdoc])* pub const $flag: Self = Self($bit);)*
            /// Every defined bit; decoding rejects any other.
            pub const ALL: Self = Self(0 $(| $bit)*);

            /// The empty set.
            pub const fn empty() -> Self {
                Self(0)
            }

            /// The raw bits.
            pub const fn bits(self) -> $repr {
                self.0
            }

            /// The set for `bits`, or `None` if a bit outside [`Self::ALL`] is set.
            pub const fn from_bits(bits: $repr) -> Option<Self> {
                if bits & !Self::ALL.0 == 0 { Some(Self(bits)) } else { None }
            }

            /// True when every bit of `other` is set in `self`.
            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }
        }

        impl BitOr for $name {
            type Output = Self;

            fn bitor(self, rhs: Self) -> Self {
                Self(self.0 | rhs.0)
            }
        }
    };
}

flag_set! {
    /// Modifier keys, bit-identical to libghostty-vt's `GhosttyMods` (`key/event.h`); side bits mean "right".
    Mods: u16 {
        /// Shift.
        SHIFT = 1 << 0;
        /// Control.
        CTRL = 1 << 1;
        /// Option / Alt.
        ALT = 1 << 2;
        /// Command / Super.
        SUPER = 1 << 3;
        /// Caps Lock is on.
        CAPS_LOCK = 1 << 4;
        /// Num Lock is on.
        NUM_LOCK = 1 << 5;
        /// The right Shift (meaningful only with SHIFT).
        SHIFT_SIDE = 1 << 6;
        /// The right Control (meaningful only with CTRL).
        CTRL_SIDE = 1 << 7;
        /// The right Option (meaningful only with ALT).
        ALT_SIDE = 1 << 8;
        /// The right Command (meaningful only with SUPER).
        SUPER_SIDE = 1 << 9;
    }
}

flag_set! {
    /// Per-cell layout flags from libghostty-vt's grid (spec R-R14).
    CellFlags: u8 {
        /// The first half of a wide character; the next cell is its spacer.
        WIDE = 1 << 0;
        /// The second half of a wide character; draw nothing.
        SPACER = 1 << 1;
        /// Padding at a row end where a wide character wrapped to the next row; draw nothing.
        SPACER_HEAD = 1 << 2;
        /// The cell holds a grapheme cluster: `Cell::extra` carries the codepoints after the base.
        GRAPHEME = 1 << 3;
    }
}

flag_set! {
    /// Terminal modes the view needs for display decisions only; plyd never trusts them back (spec R-R5).
    Modes: u16 {
        /// The alternate screen is active (no scrollback).
        ALT_SCREEN = 1 << 0;
        /// DECTCEM: the program wants the cursor shown.
        CURSOR_VISIBLE = 1 << 1;
        /// A mouse-reporting mode is on, so mouse events go to the program instead of ply's selection.
        MOUSE_REPORTING = 1 << 2;
        /// Bracketed paste (mode 2004) is on.
        BRACKETED_PASTE = 1 << 3;
    }
}

/// Underline style, the 3-bit field of [`Attrs`] (SGR 4:n).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Underline {
    /// No underline.
    #[default]
    None = 0,
    /// Single line.
    Single = 1,
    /// Double line.
    Double = 2,
    /// Curly line.
    Curly = 3,
    /// Dotted line.
    Dotted = 4,
    /// Dashed line.
    Dashed = 5,
}

/// Text attributes as a u16: eight flags plus a 3-bit [`Underline`] at bits 3–5; bits 11–15 are always zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Attrs(u16);

impl Attrs {
    /// SGR 1.
    pub const BOLD: Self = Self(1 << 0);
    /// SGR 2.
    pub const FAINT: Self = Self(1 << 1);
    /// SGR 3.
    pub const ITALIC: Self = Self(1 << 2);
    /// SGR 5.
    pub const BLINK: Self = Self(1 << 6);
    /// SGR 7.
    pub const INVERSE: Self = Self(1 << 7);
    /// SGR 8.
    pub const INVISIBLE: Self = Self(1 << 8);
    /// SGR 9.
    pub const STRIKETHROUGH: Self = Self(1 << 9);
    /// SGR 53.
    pub const OVERLINE: Self = Self(1 << 10);
    const UNDERLINE_SHIFT: u16 = 3;
    const UNDERLINE_MASK: u16 = 0b111 << Self::UNDERLINE_SHIFT;
    const DEFINED: u16 = (1 << 11) - 1;

    /// No attributes.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// The raw u16 as sent on the wire.
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Attributes for `bits`, or `None` when bits 11–15 are set or the underline field is above 5.
    pub const fn from_bits(bits: u16) -> Option<Self> {
        let underline = (bits & Self::UNDERLINE_MASK) >> Self::UNDERLINE_SHIFT;
        if bits & !Self::DEFINED == 0 && underline <= Underline::Dashed as u16 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// True when every flag of `other` is set (the underline field is ignored).
    pub const fn contains(self, other: Self) -> bool {
        let flags = other.0 & !Self::UNDERLINE_MASK;
        self.0 & flags == flags
    }

    /// The underline style.
    pub fn underline(self) -> Underline {
        match (self.0 & Self::UNDERLINE_MASK) >> Self::UNDERLINE_SHIFT {
            1 => Underline::Single,
            2 => Underline::Double,
            3 => Underline::Curly,
            4 => Underline::Dotted,
            5 => Underline::Dashed,
            _ => Underline::None,
        }
    }

    /// The same flags with the underline style replaced.
    pub const fn with_underline(self, u: Underline) -> Self {
        Self((self.0 & !Self::UNDERLINE_MASK) | ((u as u16) << Self::UNDERLINE_SHIFT))
    }
}

impl BitOr for Attrs {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// A symbolic colour: 4 bytes on the wire, `tag · a · b · c` (0 default; 1 indexed, `a` = index; 2 RGB).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Color {
    /// The theme's default for this slot (fg, bg or the underline following fg).
    #[default]
    Default,
    /// A 256-colour palette index; 0–15 resolve through the theme's ANSI table.
    Indexed(u8),
    /// A true colour.
    Rgb(u8, u8, u8),
}

/// The look of a run of cells; plyd interns styles per client and cells refer to them by id.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Style {
    /// Foreground.
    pub fg: Color,
    /// Background.
    pub bg: Color,
    /// Underline colour; `Default` means the foreground.
    pub underline_color: Color,
    /// Bold, italic, underline style and the rest.
    pub attrs: Attrs,
}

/// One interned style; id 0 is the default style, implied and never sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StyleEntry {
    /// Id cells refer to, unique per attached client; never 0.
    pub id: u16,
    /// The style.
    pub style: Style,
}

/// One grid cell: 7 bytes (`codepoint:u32 · style:u16 · flags:u8`), plus `n:u8 · n × u32` when GRAPHEME is set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Cell {
    /// Base codepoint; 0 for an empty cell.
    pub codepoint: u32,
    /// Style id from the client's table (0 = default).
    pub style: u16,
    /// Wide, spacer, spacer-head, grapheme.
    pub flags: CellFlags,
    /// Codepoints after the base of a grapheme cluster: non-empty exactly when `flags` has GRAPHEME, at most 255.
    pub extra: Vec<u32>,
}

/// One row: `index:i32 · flags:u8 (bit 0 = wraps onto the next row) · n:u16 · n cells`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Row {
    /// 0.. from the top of the live screen; negative for scrollback (-1 is the newest scrollback line).
    pub index: i32,
    /// The row's text continues on the next row (soft wrap), so copying joins them without a newline.
    pub wrapped: bool,
    /// Cells from column 0; trailing default blanks may be omitted.
    pub cells: Vec<Cell>,
}

/// Cursor shape, numbered as libghostty-vt's render-state visual style.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CursorShape {
    /// Vertical bar.
    Bar = 0,
    /// Filled block.
    #[default]
    Block = 1,
    /// Underline.
    Underline = 2,
    /// Hollow block (unfocused).
    BlockHollow = 3,
}

/// Cursor state: `col:u16 · row:u16 · shape:u8 · flags:u8` (bit 0 visible, bit 1 blinking).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Cursor {
    /// Column in the live screen.
    pub col: u16,
    /// Row in the live screen.
    pub row: u16,
    /// Shape to draw.
    pub shape: CursorShape,
    /// Whether to draw it now.
    pub visible: bool,
    /// Whether the program asked for blinking.
    pub blinking: bool,
}

/// ATTACH (0x10): `v:u16 · pane_id:u64 · cols:u16 · rows:u16 · cell_width_px:u16 · cell_height_px:u16`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Attach {
    /// C2 version, [`crate::C2_VERSION`].
    pub v: u16,
    /// Pane to attach to (C1 `Pane::id`).
    pub pane_id: u64,
    /// Grid columns the view shows.
    pub cols: u16,
    /// Grid rows the view shows.
    pub rows: u16,
    /// Width of one cell in pixels (not of the view), as libghostty-vt's resize takes it.
    pub cell_width_px: u16,
    /// Height of one cell in pixels.
    pub cell_height_px: u16,
}

/// RESIZE (0x12): `cols:u16 · rows:u16 · cell_width_px:u16 · cell_height_px:u16`; plyd reflows and sends a Snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Resize {
    /// New grid columns.
    pub cols: u16,
    /// New grid rows.
    pub rows: u16,
    /// Cell width in pixels.
    pub cell_width_px: u16,
    /// Cell height in pixels.
    pub cell_height_px: u16,
}

/// FETCH_HISTORY (0x13): `start:i64 · count:u16`; rows `start..start+count` (negative indexes), count at most 1000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FetchHistory {
    /// Index of the first row wanted (a scrollback index, so negative).
    pub start: i64,
    /// Number of rows, 1..=[`MAX_HISTORY_ROWS`].
    pub count: u16,
}

/// ACK (0x14): `seq:u64`, the newest Snapshot or Delta the client has applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ack {
    /// Sequence number being acknowledged.
    pub seq: u64,
}

/// Key action, numbered as libghostty-vt's `GhosttyKeyAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    /// Key up.
    Release = 0,
    /// Key down.
    Press = 1,
    /// Auto-repeat.
    Repeat = 2,
}

/// KEY (0x15): `key:u16 · mods:u16 · consumed_mods:u16 · action:u8 · flags:u8 · unshifted:u32 · text`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    /// Physical key as a `GhosttyKey` value, 0..=[`MAX_KEY_CODE`]; 0 (unidentified) sends `text` alone.
    pub key: u16,
    /// Modifiers held.
    pub mods: Mods,
    /// Modifiers the layout used to produce `text` (e.g. ⌥ for "@" on a German layout).
    pub consumed_mods: Mods,
    /// Press, repeat or release.
    pub action: KeyAction,
    /// An IME composition is in progress (flags bit 0); plyd encodes nothing for it.
    pub composing: bool,
    /// The key's codepoint without Shift, for kitty alternate-key reports; 0 when unknown.
    pub unshifted_codepoint: u32,
    /// Text the key produced after the layout, possibly empty.
    pub text: String,
}

/// Mouse action, numbered as libghostty-vt's `GhosttyMouseAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseAction {
    /// Button down.
    Press = 0,
    /// Button up.
    Release = 1,
    /// Pointer moved.
    Motion = 2,
}

/// Mouse button, numbered as libghostty-vt's `GhosttyMouseButton`; wheel steps are Four–Seven.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// No button (plain motion).
    None = 0,
    /// Primary.
    Left = 1,
    /// Secondary.
    Right = 2,
    /// Middle.
    Middle = 3,
    /// Wheel up.
    Four = 4,
    /// Wheel down.
    Five = 5,
    /// Wheel left.
    Six = 6,
    /// Wheel right.
    Seven = 7,
    /// Extra button 8.
    Eight = 8,
    /// Extra button 9.
    Nine = 9,
    /// Extra button 10.
    Ten = 10,
    /// Extra button 11.
    Eleven = 11,
}

/// MOUSE (0x16): `action:u8 · button:u8 · mods:u16 · col:u16 · row:u16 · x:f32 · y:f32`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseEvent {
    /// Press, release or motion.
    pub action: MouseAction,
    /// Button involved.
    pub button: MouseButton,
    /// Modifiers held.
    pub mods: Mods,
    /// Cell column under the pointer.
    pub col: u16,
    /// Cell row under the pointer.
    pub row: u16,
    /// Pointer x in pixels from the terminal's top-left (SGR-pixels mode 1016 needs it); finite.
    pub x: f32,
    /// Pointer y in pixels from the terminal's top-left; finite.
    pub y: f32,
}

/// PASTE (0x17): `allow_unsafe:u8 · text`; unsafe text without `allow_unsafe` gets PASTE_REJECTED (Ruling R21).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Paste {
    /// The user confirmed a paste libghostty-vt considers unsafe.
    pub allow_unsafe: bool,
    /// The pasted text.
    pub text: String,
}

/// FOCUS (0x18): `in:u8`; plyd reports it to the program only when it enabled mode 1004.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Focus {
    /// The view gained (true) or lost (false) focus.
    pub focused: bool,
}

/// SNAPSHOT (0x20): the whole screen, answering every ATTACH and RESIZE; it replaces the client's style table.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Snapshot {
    /// Sequence number, per attached client, strictly increasing across Snapshots and Deltas.
    pub seq: u64,
    /// Grid columns.
    pub cols: u16,
    /// Grid rows.
    pub rows: u16,
    /// Cursor.
    pub cursor: Cursor,
    /// Display modes.
    pub modes: Modes,
    /// Scrollback rows available above row 0 (indexes `-scrollback_rows..0`).
    pub scrollback_rows: u32,
    /// The complete style table.
    pub styles: Vec<StyleEntry>,
    /// Every screen row, `0..rows`.
    pub lines: Vec<Row>,
}

/// DELTA (0x21): the rows changed since the previous frame, with the cursor, modes and newly interned styles.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Delta {
    /// Sequence number (see [`Snapshot::seq`]).
    pub seq: u64,
    /// Cursor after the change.
    pub cursor: Cursor,
    /// Display modes after the change.
    pub modes: Modes,
    /// Scrollback rows available after the change.
    pub scrollback_rows: u32,
    /// Styles interned since the previous frame; ids never repeat within one attachment.
    pub styles_added: Vec<StyleEntry>,
    /// Changed rows, each replacing the row with the same index.
    pub lines: Vec<Row>,
}

/// HISTORY (0x22): scrollback rows answering FETCH_HISTORY, clipped to what exists (possibly none).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct History {
    /// Index of the first row returned.
    pub start: i64,
    /// Rows `start..start+len`, oldest first; their styles arrive in this frame's `styles_added` or earlier.
    pub lines: Vec<Row>,
    /// Styles new to the client; apply them even when discarding the rows (a page cut to fit may name styles of rows it left out, and later frames reuse the ids without resending).
    pub styles_added: Vec<StyleEntry>,
}

/// EXIT (0x25): `code:i32`; a signal death is 128 + signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Exit {
    /// Exit code.
    pub code: i32,
}

/// Why plyd refused an ATTACH.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefuseReason {
    /// ATTACH `v` differs from plyd's [`crate::C2_VERSION`].
    VersionMismatch = 1,
    /// No pane has that id.
    UnknownPane = 2,
    /// The first frame on the connection was not ATTACH.
    NotAttached = 3,
}

/// ATTACH_REFUSED (0x27): `reason:u8 · message`; plyd closes the connection after sending it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AttachRefused {
    /// Machine-readable reason.
    pub reason: RefuseReason,
    /// Human-readable explanation, shown to the user.
    pub message: String,
}

/// Every C2 frame; [`Frame::kind`] gives its kind byte and [`Frame::encode`] its exact layout.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// 0x10.
    Attach(Attach),
    /// 0x11: raw bytes for the pty.
    InputRaw(Vec<u8>),
    /// 0x12.
    Resize(Resize),
    /// 0x13.
    FetchHistory(FetchHistory),
    /// 0x14.
    Ack(Ack),
    /// 0x15.
    Key(KeyEvent),
    /// 0x16.
    Mouse(MouseEvent),
    /// 0x17.
    Paste(Paste),
    /// 0x18.
    Focus(Focus),
    /// 0x20.
    Snapshot(Snapshot),
    /// 0x21.
    Delta(Delta),
    /// 0x22.
    History(History),
    /// 0x23: the new title, UTF-8; coalesced to the Delta cadence.
    Title(String),
    /// 0x24: empty payload.
    Bell,
    /// 0x25.
    Exit(Exit),
    /// 0x26: empty payload.
    PasteRejected,
    /// 0x27.
    AttachRefused(AttachRefused),
}

struct Writer<'a>(&'a mut Vec<u8>);

impl Writer<'_> {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bool(&mut self, v: bool) {
        self.0.push(u8::from(v));
    }
    fn bytes(&mut self, v: &[u8]) {
        self.0.extend_from_slice(v);
    }
    fn count16(&mut self, kind: u8, field: &'static str, n: usize) -> Result<()> {
        let n = u16::try_from(n).map_err(|_| invalid(kind, field, n as u64))?;
        self.u16(n);
        Ok(())
    }
    fn color(&mut self, c: Color) {
        match c {
            Color::Default => self.bytes(&[0, 0, 0, 0]),
            Color::Indexed(i) => self.bytes(&[1, i, 0, 0]),
            Color::Rgb(r, g, b) => self.bytes(&[2, r, g, b]),
        }
    }
    fn style_entries(&mut self, kind: u8, entries: &[StyleEntry]) -> Result<()> {
        self.count16(kind, "style count", entries.len())?;
        for e in entries {
            self.u16(e.id);
            self.color(e.style.fg);
            self.color(e.style.bg);
            self.color(e.style.underline_color);
            self.u16(e.style.attrs.bits());
        }
        Ok(())
    }
    fn cursor(&mut self, c: &Cursor) {
        self.u16(c.col);
        self.u16(c.row);
        self.u8(c.shape as u8);
        self.u8(u8::from(c.visible) | (u8::from(c.blinking) << 1));
    }
    fn rows(&mut self, kind: u8, rows: &[Row]) -> Result<()> {
        self.count16(kind, "row count", rows.len())?;
        for row in rows {
            self.i32(row.index);
            self.bool(row.wrapped);
            self.count16(kind, "cell count", row.cells.len())?;
            for cell in &row.cells {
                self.u32(cell.codepoint);
                self.u16(cell.style);
                self.u8(cell.flags.bits());
                if cell.flags.contains(CellFlags::GRAPHEME) {
                    let n = u8::try_from(cell.extra.len())
                        .map_err(|_| invalid(kind, "grapheme length", cell.extra.len() as u64))?;
                    self.u8(n);
                    for cp in &cell.extra {
                        self.u32(*cp);
                    }
                } else if !cell.extra.is_empty() {
                    return Err(invalid(kind, "grapheme length", cell.extra.len() as u64));
                }
            }
        }
        Ok(())
    }
}

fn invalid(kind: u8, field: &'static str, value: u64) -> Error {
    Error::InvalidValue { kind, field, value }
}

struct Reader<'a> {
    kind: u8,
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.buf.len() < n {
            return Err(Error::Truncated { kind: self.kind });
        }
        let (head, rest) = self.buf.split_at(n);
        self.buf = rest;
        Ok(head)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.array()?))
    }
    fn f32(&mut self, field: &'static str) -> Result<f32> {
        let v = f32::from_le_bytes(self.array()?);
        if v.is_finite() {
            Ok(v)
        } else {
            Err(invalid(self.kind, field, u64::from(v.to_bits())))
        }
    }
    fn bool(&mut self, field: &'static str) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(invalid(self.kind, field, u64::from(v))),
        }
    }
    fn codepoint(&mut self, field: &'static str) -> Result<u32> {
        let cp = self.u32()?;
        if cp <= MAX_CODEPOINT {
            Ok(cp)
        } else {
            Err(invalid(self.kind, field, u64::from(cp)))
        }
    }
    fn rest_text(&mut self) -> Result<String> {
        let bytes = std::mem::take(&mut self.buf);
        String::from_utf8(bytes.to_vec()).map_err(|_| Error::InvalidUtf8 { kind: self.kind })
    }
    fn mods(&mut self, field: &'static str) -> Result<Mods> {
        let bits = self.u16()?;
        Mods::from_bits(bits).ok_or_else(|| invalid(self.kind, field, u64::from(bits)))
    }
    fn color(&mut self, field: &'static str) -> Result<Color> {
        let [tag, a, b, c] = self.array::<4>()?;
        match (tag, a, b, c) {
            (0, 0, 0, 0) => Ok(Color::Default),
            (1, i, 0, 0) => Ok(Color::Indexed(i)),
            (2, r, g, b) => Ok(Color::Rgb(r, g, b)),
            _ => Err(invalid(
                self.kind,
                field,
                u64::from(u32::from_le_bytes([tag, a, b, c])),
            )),
        }
    }
    fn style_entries(&mut self) -> Result<Vec<StyleEntry>> {
        let n = self.u16()?;
        let mut out = Vec::with_capacity(usize::from(n).min(self.buf.len() / 14));
        for _ in 0..n {
            let id = self.u16()?;
            if id == 0 {
                return Err(invalid(self.kind, "style id", 0));
            }
            let fg = self.color("fg")?;
            let bg = self.color("bg")?;
            let underline_color = self.color("underline colour")?;
            let bits = self.u16()?;
            let attrs = Attrs::from_bits(bits)
                .ok_or_else(|| invalid(self.kind, "attrs", u64::from(bits)))?;
            out.push(StyleEntry {
                id,
                style: Style {
                    fg,
                    bg,
                    underline_color,
                    attrs,
                },
            });
        }
        Ok(out)
    }
    fn cursor(&mut self) -> Result<Cursor> {
        let col = self.u16()?;
        let row = self.u16()?;
        let shape = match self.u8()? {
            0 => CursorShape::Bar,
            1 => CursorShape::Block,
            2 => CursorShape::Underline,
            3 => CursorShape::BlockHollow,
            v => return Err(invalid(self.kind, "cursor shape", u64::from(v))),
        };
        let flags = self.u8()?;
        if flags & !0b11 != 0 {
            return Err(invalid(self.kind, "cursor flags", u64::from(flags)));
        }
        Ok(Cursor {
            col,
            row,
            shape,
            visible: flags & 1 != 0,
            blinking: flags & 2 != 0,
        })
    }
    fn modes(&mut self) -> Result<Modes> {
        let bits = self.u16()?;
        Modes::from_bits(bits).ok_or_else(|| invalid(self.kind, "modes", u64::from(bits)))
    }
    fn rows(&mut self) -> Result<Vec<Row>> {
        let n = self.u16()?;
        let mut out = Vec::with_capacity(usize::from(n).min(self.buf.len() / 7));
        for _ in 0..n {
            let index = self.i32()?;
            let wrapped = self.bool("row flags")?;
            let cells_n = self.u16()?;
            let mut cells = Vec::with_capacity(usize::from(cells_n).min(self.buf.len() / 7));
            for _ in 0..cells_n {
                let codepoint = self.codepoint("codepoint")?;
                let style = self.u16()?;
                let bits = self.u8()?;
                let flags = CellFlags::from_bits(bits)
                    .ok_or_else(|| invalid(self.kind, "cell flags", u64::from(bits)))?;
                let mut extra = Vec::new();
                if flags.contains(CellFlags::GRAPHEME) {
                    let len = self.u8()?;
                    if len == 0 {
                        return Err(invalid(self.kind, "grapheme length", 0));
                    }
                    for _ in 0..len {
                        extra.push(self.codepoint("grapheme codepoint")?);
                    }
                }
                cells.push(Cell {
                    codepoint,
                    style,
                    flags,
                    extra,
                });
            }
            out.push(Row {
                index,
                wrapped,
                cells,
            });
        }
        Ok(out)
    }
}

impl Frame {
    /// The frame's kind byte (see [`kind`]).
    pub fn kind(&self) -> u8 {
        match self {
            Self::Attach(_) => kind::ATTACH,
            Self::InputRaw(_) => kind::INPUT_RAW,
            Self::Resize(_) => kind::RESIZE,
            Self::FetchHistory(_) => kind::FETCH_HISTORY,
            Self::Ack(_) => kind::ACK,
            Self::Key(_) => kind::KEY,
            Self::Mouse(_) => kind::MOUSE,
            Self::Paste(_) => kind::PASTE,
            Self::Focus(_) => kind::FOCUS,
            Self::Snapshot(_) => kind::SNAPSHOT,
            Self::Delta(_) => kind::DELTA,
            Self::History(_) => kind::HISTORY,
            Self::Title(_) => kind::TITLE,
            Self::Bell => kind::BELL,
            Self::Exit(_) => kind::EXIT,
            Self::PasteRejected => kind::PASTE_REJECTED,
            Self::AttachRefused(_) => kind::ATTACH_REFUSED,
        }
    }

    /// True for the kinds a client sends (0x10–0x1F); plyd refuses the others from a client.
    pub fn is_from_client(&self) -> bool {
        self.kind() < kind::SNAPSHOT
    }

    /// Appends `len · kind · payload` to `out`; on error `out` is left as it was.
    /// Fails with [`Error::FrameTooLarge`] above [`MAX_FRAME_LEN`] or [`Error::InvalidValue`] for a count above its field.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let start = out.len();
        out.extend_from_slice(&[0, 0, 0, 0, self.kind()]);
        let written = self.encode_payload(&mut Writer(out));
        let len = out.len() - start - HEADER_LEN;
        let checked = written.and_then(|()| {
            if len > MAX_FRAME_LEN {
                return Err(Error::FrameTooLarge { len: len as u64 });
            }
            u32::try_from(len).map_err(|_| Error::FrameTooLarge { len: len as u64 })
        });
        match checked {
            Ok(len32) => {
                out[start..start + 4].copy_from_slice(&len32.to_le_bytes());
                Ok(())
            }
            Err(e) => {
                out.truncate(start);
                Err(e)
            }
        }
    }

    fn encode_payload(&self, w: &mut Writer<'_>) -> Result<()> {
        let k = self.kind();
        match self {
            Self::Attach(a) => {
                w.u16(a.v);
                w.u64(a.pane_id);
                w.u16(a.cols);
                w.u16(a.rows);
                w.u16(a.cell_width_px);
                w.u16(a.cell_height_px);
            }
            Self::InputRaw(bytes) => w.bytes(bytes),
            Self::Resize(r) => {
                w.u16(r.cols);
                w.u16(r.rows);
                w.u16(r.cell_width_px);
                w.u16(r.cell_height_px);
            }
            Self::FetchHistory(f) => {
                w.i64(f.start);
                w.u16(f.count);
            }
            Self::Ack(a) => w.u64(a.seq),
            Self::Key(e) => {
                w.u16(e.key);
                w.u16(e.mods.bits());
                w.u16(e.consumed_mods.bits());
                w.u8(e.action as u8);
                w.u8(u8::from(e.composing));
                w.u32(e.unshifted_codepoint);
                w.bytes(e.text.as_bytes());
            }
            Self::Mouse(e) => {
                w.u8(e.action as u8);
                w.u8(e.button as u8);
                w.u16(e.mods.bits());
                w.u16(e.col);
                w.u16(e.row);
                w.f32(e.x);
                w.f32(e.y);
            }
            Self::Paste(p) => {
                w.bool(p.allow_unsafe);
                w.bytes(p.text.as_bytes());
            }
            Self::Focus(f) => w.bool(f.focused),
            Self::Snapshot(s) => {
                w.u64(s.seq);
                w.u16(s.cols);
                w.u16(s.rows);
                w.cursor(&s.cursor);
                w.u16(s.modes.bits());
                w.u32(s.scrollback_rows);
                w.style_entries(k, &s.styles)?;
                w.rows(k, &s.lines)?;
            }
            Self::Delta(d) => {
                w.u64(d.seq);
                w.cursor(&d.cursor);
                w.u16(d.modes.bits());
                w.u32(d.scrollback_rows);
                w.style_entries(k, &d.styles_added)?;
                w.rows(k, &d.lines)?;
            }
            Self::History(h) => {
                w.i64(h.start);
                w.style_entries(k, &h.styles_added)?;
                w.rows(k, &h.lines)?;
            }
            Self::Title(t) => w.bytes(t.as_bytes()),
            Self::Bell | Self::PasteRejected => {}
            Self::Exit(e) => w.i32(e.code),
            Self::AttachRefused(r) => {
                w.u8(r.reason as u8);
                w.bytes(r.message.as_bytes());
            }
        }
        Ok(())
    }

    /// Decodes one payload of kind `kind`; the caller has already enforced `payload.len() <= MAX_FRAME_LEN`.
    /// Fails with [`Error::UnknownFrameKind`], [`Error::Truncated`], [`Error::TrailingBytes`], [`Error::InvalidValue`] or [`Error::InvalidUtf8`].
    pub fn decode(kind: u8, payload: &[u8]) -> Result<Self> {
        if payload.len() > MAX_FRAME_LEN {
            return Err(Error::FrameTooLarge {
                len: payload.len() as u64,
            });
        }
        let mut r = Reader { kind, buf: payload };
        let frame = match kind {
            kind::ATTACH => Self::Attach(Attach {
                v: r.u16()?,
                pane_id: r.u64()?,
                cols: r.u16()?,
                rows: r.u16()?,
                cell_width_px: r.u16()?,
                cell_height_px: r.u16()?,
            }),
            kind::INPUT_RAW => Self::InputRaw(std::mem::take(&mut r.buf).to_vec()),
            kind::RESIZE => Self::Resize(Resize {
                cols: r.u16()?,
                rows: r.u16()?,
                cell_width_px: r.u16()?,
                cell_height_px: r.u16()?,
            }),
            kind::FETCH_HISTORY => {
                let start = r.i64()?;
                let count = r.u16()?;
                if count == 0 || count > MAX_HISTORY_ROWS {
                    return Err(invalid(kind, "history count", u64::from(count)));
                }
                Self::FetchHistory(FetchHistory { start, count })
            }
            kind::ACK => Self::Ack(Ack { seq: r.u64()? }),
            kind::KEY => {
                let key = r.u16()?;
                if key > MAX_KEY_CODE {
                    return Err(invalid(kind, "key", u64::from(key)));
                }
                let mods = r.mods("mods")?;
                let consumed_mods = r.mods("consumed mods")?;
                let action = match r.u8()? {
                    0 => KeyAction::Release,
                    1 => KeyAction::Press,
                    2 => KeyAction::Repeat,
                    v => return Err(invalid(kind, "key action", u64::from(v))),
                };
                let composing = r.bool("key flags")?;
                let unshifted_codepoint = r.codepoint("unshifted codepoint")?;
                let text = r.rest_text()?;
                Self::Key(KeyEvent {
                    key,
                    mods,
                    consumed_mods,
                    action,
                    composing,
                    unshifted_codepoint,
                    text,
                })
            }
            kind::MOUSE => {
                let action = match r.u8()? {
                    0 => MouseAction::Press,
                    1 => MouseAction::Release,
                    2 => MouseAction::Motion,
                    v => return Err(invalid(kind, "mouse action", u64::from(v))),
                };
                let button = match r.u8()? {
                    0 => MouseButton::None,
                    1 => MouseButton::Left,
                    2 => MouseButton::Right,
                    3 => MouseButton::Middle,
                    4 => MouseButton::Four,
                    5 => MouseButton::Five,
                    6 => MouseButton::Six,
                    7 => MouseButton::Seven,
                    8 => MouseButton::Eight,
                    9 => MouseButton::Nine,
                    10 => MouseButton::Ten,
                    11 => MouseButton::Eleven,
                    v => return Err(invalid(kind, "mouse button", u64::from(v))),
                };
                Self::Mouse(MouseEvent {
                    action,
                    button,
                    mods: r.mods("mods")?,
                    col: r.u16()?,
                    row: r.u16()?,
                    x: r.f32("x")?,
                    y: r.f32("y")?,
                })
            }
            kind::PASTE => Self::Paste(Paste {
                allow_unsafe: r.bool("allow_unsafe")?,
                text: r.rest_text()?,
            }),
            kind::FOCUS => Self::Focus(Focus {
                focused: r.bool("in")?,
            }),
            kind::SNAPSHOT => Self::Snapshot(Snapshot {
                seq: r.u64()?,
                cols: r.u16()?,
                rows: r.u16()?,
                cursor: r.cursor()?,
                modes: r.modes()?,
                scrollback_rows: r.u32()?,
                styles: r.style_entries()?,
                lines: r.rows()?,
            }),
            kind::DELTA => Self::Delta(Delta {
                seq: r.u64()?,
                cursor: r.cursor()?,
                modes: r.modes()?,
                scrollback_rows: r.u32()?,
                styles_added: r.style_entries()?,
                lines: r.rows()?,
            }),
            kind::HISTORY => Self::History(History {
                start: r.i64()?,
                styles_added: r.style_entries()?,
                lines: r.rows()?,
            }),
            kind::TITLE => Self::Title(r.rest_text()?),
            kind::BELL => Self::Bell,
            kind::EXIT => Self::Exit(Exit { code: r.i32()? }),
            kind::PASTE_REJECTED => Self::PasteRejected,
            kind::ATTACH_REFUSED => {
                let reason = match r.u8()? {
                    1 => RefuseReason::VersionMismatch,
                    2 => RefuseReason::UnknownPane,
                    3 => RefuseReason::NotAttached,
                    v => return Err(invalid(kind, "refuse reason", u64::from(v))),
                };
                Self::AttachRefused(AttachRefused {
                    reason,
                    message: r.rest_text()?,
                })
            }
            other => return Err(Error::UnknownFrameKind(other)),
        };
        if !r.buf.is_empty() {
            return Err(Error::TrailingBytes {
                kind,
                extra: r.buf.len(),
            });
        }
        Ok(frame)
    }
}

/// Reads whole frames from a blocking byte stream, refusing any header above [`MAX_FRAME_LEN`] before allocating.
#[derive(Debug)]
pub struct FrameReader<R> {
    inner: R,
    buf: Vec<u8>,
}

impl<R: Read> FrameReader<R> {
    /// Wraps a stream; the reader keeps one payload buffer, reused across frames.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buf: Vec::new(),
        }
    }

    /// Blocks for the next frame; `Ok(None)` is a clean end of stream at a frame boundary.
    /// Fails with [`Error::Io`] (also for an end of stream inside a frame), [`Error::FrameTooLarge`] or a decode error.
    pub fn read_frame(&mut self) -> Result<Option<Frame>> {
        let mut header = [0u8; HEADER_LEN];
        let mut filled = 0;
        while filled < HEADER_LEN {
            match self.inner.read(&mut header[filled..]) {
                Ok(0) if filled == 0 => return Ok(None),
                Ok(0) => return Err(Error::Io(ErrorKind::UnexpectedEof.into())),
                Ok(n) => filled += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::Io(e)),
            }
        }
        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let len = usize::try_from(len).map_err(|_| Error::FrameTooLarge {
            len: u64::from(len),
        })?;
        if len > MAX_FRAME_LEN {
            return Err(Error::FrameTooLarge { len: len as u64 });
        }
        self.buf.resize(len, 0);
        self.inner.read_exact(&mut self.buf)?;
        Frame::decode(header[4], &self.buf).map(Some)
    }

    /// Returns the wrapped stream; bytes of a partly read frame are lost.
    pub fn into_inner(self) -> R {
        self.inner
    }
}
