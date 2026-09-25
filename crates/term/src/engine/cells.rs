//! Turning libghostty-vt cells and styles into C2 values (ADR-0005 Decision 3, "three rules for the DeltaBuilder").
//!
//! Colours stay symbolic: a style's palette index becomes [`Color::Indexed`], never an RGB lookup, so the
//! `*_FG_COLOR` / `*_BG_COLOR` resolvers are never used. An erased cell with only a background keeps that colour in
//! its content tag and reports no styling, so its background comes from the tag. Styles are values here; C2 style
//! ids are interned per client by the DeltaBuilder, because `GhosttyStyleId` is page-local.

use std::ffi::{c_int, c_void};

use ghostty_sys as sys;
use ply_proto::data::{Attrs, CellFlags, Color, Style, Underline};

/// The fields of a raw cell the C2 encoding needs.
#[derive(Debug, Clone, Copy)]
pub(super) struct RawCell {
    pub(super) codepoint: u32,
    pub(super) tag: c_int,
    pub(super) wide: c_int,
    pub(super) styled: bool,
}

/// A `ghostty_cell_get` the library refused: the data kind asked for and its `GhosttyResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CellReadError {
    pub(super) data: sys::GhosttyCellData,
    pub(super) code: sys::GhosttyResult,
}

/// Reads one field of a packed cell into `out`, which must be the data kind's documented type.
fn cell_get<T>(
    raw: sys::GhosttyCell,
    data: sys::GhosttyCellData,
    out: &mut T,
) -> Result<(), CellReadError> {
    // SAFETY: `ghostty_cell_get` reads the packed value; callers pass an `out` of the type `data` documents.
    let code = unsafe { sys::ghostty_cell_get(raw, data, (out as *mut T).cast::<c_void>()) };
    if code == sys::GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(CellReadError { data, code })
    }
}

impl RawCell {
    /// Decodes `raw` through `ghostty_cell_get`; fails with the first field the library refuses (the caller draws a blank).
    pub(super) fn decode(raw: sys::GhosttyCell) -> Result<Self, CellReadError> {
        let mut cell = Self {
            codepoint: 0,
            tag: sys::GHOSTTY_CELL_CONTENT_CODEPOINT,
            wide: sys::GHOSTTY_CELL_WIDE_NARROW,
            styled: false,
        };
        cell_get(raw, sys::GHOSTTY_CELL_DATA_CODEPOINT, &mut cell.codepoint)?;
        cell_get(raw, sys::GHOSTTY_CELL_DATA_CONTENT_TAG, &mut cell.tag)?;
        cell_get(raw, sys::GHOSTTY_CELL_DATA_WIDE, &mut cell.wide)?;
        cell_get(raw, sys::GHOSTTY_CELL_DATA_HAS_STYLING, &mut cell.styled)?;
        Ok(cell)
    }

    /// An empty, unstyled, narrow cell: the C2 default blank.
    pub(super) fn is_blank(&self) -> bool {
        self.codepoint == 0
            && self.tag == sys::GHOSTTY_CELL_CONTENT_CODEPOINT
            && self.wide == sys::GHOSTTY_CELL_WIDE_NARROW
            && !self.styled
    }

    /// The layout flags for this cell (GRAPHEME is added by the caller once it has read the extra codepoints).
    pub(super) fn flags(&self) -> CellFlags {
        match self.wide {
            sys::GHOSTTY_CELL_WIDE_WIDE => CellFlags::WIDE,
            sys::GHOSTTY_CELL_WIDE_SPACER_TAIL => CellFlags::SPACER,
            sys::GHOSTTY_CELL_WIDE_SPACER_HEAD => CellFlags::SPACER_HEAD,
            _ => CellFlags::empty(),
        }
    }

    /// The background a bg-only cell carries in its content tag, if it is one; fails when the library refuses the read.
    pub(super) fn tag_background(
        &self,
        raw: sys::GhosttyCell,
    ) -> Result<Option<Color>, CellReadError> {
        match self.tag {
            sys::GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE => {
                let mut index = 0u8;
                cell_get(raw, sys::GHOSTTY_CELL_DATA_COLOR_PALETTE, &mut index)?;
                Ok(Some(Color::Indexed(index)))
            }
            sys::GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => {
                let mut rgb = sys::GhosttyColorRgb::default();
                cell_get(raw, sys::GHOSTTY_CELL_DATA_COLOR_RGB, &mut rgb)?;
                Ok(Some(Color::Rgb(rgb.r, rgb.g, rgb.b)))
            }
            _ => Ok(None),
        }
    }

    /// True when the cell holds a grapheme cluster whose extra codepoints must be read.
    pub(super) fn has_graphemes(&self) -> bool {
        self.tag == sys::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME
    }
}

/// A zeroed `GhosttyStyle` with its `size` set, ready to be filled by the library.
pub(super) fn empty_style() -> sys::GhosttyStyle {
    sys::GhosttyStyle {
        size: size_of::<sys::GhosttyStyle>(),
        fg_color: none_color(),
        bg_color: none_color(),
        underline_color: none_color(),
        bold: false,
        italic: false,
        faint: false,
        blink: false,
        inverse: false,
        invisible: false,
        strikethrough: false,
        overline: false,
        underline: 0,
    }
}

fn none_color() -> sys::GhosttyStyleColor {
    sys::GhosttyStyleColor {
        tag: sys::GHOSTTY_STYLE_COLOR_NONE,
        value: sys::GhosttyStyleColorValue { _padding: 0 },
    }
}

/// The C2 style of a libghostty-vt style.
pub(super) fn style(s: &sys::GhosttyStyle) -> Style {
    let mut attrs = Attrs::empty();
    for (on, flag) in [
        (s.bold, Attrs::BOLD),
        (s.faint, Attrs::FAINT),
        (s.italic, Attrs::ITALIC),
        (s.blink, Attrs::BLINK),
        (s.inverse, Attrs::INVERSE),
        (s.invisible, Attrs::INVISIBLE),
        (s.strikethrough, Attrs::STRIKETHROUGH),
        (s.overline, Attrs::OVERLINE),
    ] {
        if on {
            attrs = attrs | flag;
        }
    }
    let underline = match s.underline {
        1 => Underline::Single,
        2 => Underline::Double,
        3 => Underline::Curly,
        4 => Underline::Dotted,
        5 => Underline::Dashed,
        _ => Underline::None,
    };
    Style {
        fg: color(&s.fg_color),
        bg: color(&s.bg_color),
        underline_color: color(&s.underline_color),
        attrs: attrs.with_underline(underline),
    }
}

fn color(c: &sys::GhosttyStyleColor) -> Color {
    match c.tag {
        // SAFETY: the tag names the initialised member; both members are plain bytes.
        sys::GHOSTTY_STYLE_COLOR_PALETTE => Color::Indexed(unsafe { c.value.palette }),
        sys::GHOSTTY_STYLE_COLOR_RGB => {
            // SAFETY: as above.
            let rgb = unsafe { c.value.rgb };
            Color::Rgb(rgb.r, rgb.g, rgb.b)
        }
        _ => Color::Default,
    }
}
