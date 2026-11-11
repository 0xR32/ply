//! Replica rows → GPUI paint calls: background quads plus one shaped line per segment, glyphs snapped to the grid.

use gpui::{
    App, Bounds, Font, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, SharedString,
    StrikethroughStyle, TextRun, UnderlineStyle, Window, fill, point, px, size,
};
use ply_proto::data::{Attrs, CellFlags, Color, CursorShape, Modes, Underline};
use ply_proto::pane::{AccentName, Rgb, Settings, TerminalTheme};
use ply_term::{Palette, Replica};

pub const FONT_FAMILY: &str = "Geist Mono";

pub fn hsla(c: Rgb) -> Hsla {
    gpui::rgb((u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)).into()
}

fn hex(v: u32) -> Rgb {
    Rgb {
        r: (v >> 16) as u8,
        g: (v >> 8) as u8,
        b: v as u8,
    }
}

/// `terminalThemeFor` of `tokens.ts`: fixed ANSI, cursor in the accent, selection the accent at 25 % over the ground.
pub fn theme_for(settings: &Settings) -> TerminalTheme {
    let accent = match settings.accent {
        AccentName::Blue => 0x8AB4FF,
        AccentName::Mint => 0x7FD4B0,
        AccentName::Violet => 0xB4A5FF,
        AccentName::Sand => 0xE6D3A3,
    };
    let (text, term) = (hex(0xE6E8EF), hex(0x0C0E14));
    let ansi = [
        0x0A0B10, 0xF07A6A, 0x7FD4B0, 0xF2B35B, 0x8AB4FF, 0xB4A5FF, 0x86CDBB, 0xE6E8EF, 0x5C6273,
        0xF49B8F, 0x9EDEC3, 0xF5C581, 0xB3CEFF, 0xD6CEFF, 0xA3D9CB, 0xFFFFFF,
    ]
    .map(hex);
    TerminalTheme {
        ansi,
        fg: text,
        bg: term,
        cursor: hex(accent),
        cursor_text: term,
        selection_bg: blend(hex(accent), 0.25, term),
        selection_fg: text,
    }
}

#[derive(Clone, Copy)]
pub struct Metrics {
    pub font_size: Pixels,
    pub cell_w: Pixels,
    pub cell_h: Pixels,
}

struct Segment {
    col: u16,
    text: String,
    runs: Vec<TextRun>,
}

struct BgRect {
    col: u16,
    len: u16,
    color: Hsla,
}

fn font(weight: FontWeight, italic: bool) -> Font {
    Font {
        family: SharedString::new_static(FONT_FAMILY),
        features: FontFeatures::disable_ligatures(),
        fallbacks: None,
        weight,
        style: if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
    }
}

fn blend(top: Rgb, alpha: f32, bottom: Rgb) -> Rgb {
    let mix = |t: u8, b: u8| (f32::from(t) * alpha + f32::from(b) * (1.0 - alpha)).round() as u8;
    Rgb {
        r: mix(top.r, bottom.r),
        g: mix(top.g, bottom.g),
        b: mix(top.b, bottom.b),
    }
}

/// Where and how one pane is painted.
#[derive(Clone, Copy)]
pub struct View {
    pub bounds: Bounds<Pixels>,
    /// Absolute line at the top of a scrolled-back view; `None` follows the live screen.
    pub view_top: Option<u64>,
    pub focused: bool,
    pub cursor_on: bool,
}

/// Paints the live screen, or the view from absolute line `view_top`; returns the first scrollback line it lacked.
pub fn paint_screen(
    replica: &Replica,
    pal: &Palette,
    m: Metrics,
    v: View,
    window: &mut Window,
    cx: &mut App,
) -> Option<u64> {
    let View {
        bounds,
        view_top,
        focused,
        cursor_on,
    } = v;
    let (cols, rows) = replica.size();
    let default_bg = pal.bg;
    let live_top = replica.scrollback_base() + u64::from(replica.scrollback_rows());
    let top = view_top
        .unwrap_or(live_top)
        .clamp(replica.scrollback_base(), live_top);
    let mut missing = None;
    for y in 0..rows {
        let line = top + u64::from(y);
        let row = if line < live_top {
            let r = replica.history_row(line);
            if r.is_none() && missing.is_none() {
                missing = Some(line);
            }
            r
        } else {
            replica.row((line - live_top) as u16)
        };
        let Some(row) = row else { continue };
        let mut segs: Vec<Segment> = Vec::new();
        let mut bgs: Vec<BgRect> = Vec::new();
        let mut cur: Option<Segment> = None;
        let last = row
            .cells
            .iter()
            .rposition(|c| (c.codepoint != 0 && c.codepoint != u32::from(' ')) || c.style != 0)
            .map_or(0, |i| i + 1);
        for (x, cell) in row.cells[..last].iter().enumerate() {
            let x = x as u16;
            if x >= cols {
                break;
            }
            if cell.flags.contains(CellFlags::SPACER) || cell.flags.contains(CellFlags::SPACER_HEAD)
            {
                continue;
            }
            let style = replica.style(cell.style).unwrap_or_default();
            let attrs = style.attrs;
            let mut fg = pal.resolve_fg(style.fg);
            let mut bg = pal.resolve_bg(style.bg);
            let mut bg_set = style.bg != Color::Default;
            if attrs.contains(Attrs::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
                bg_set = true;
            }
            if attrs.contains(Attrs::FAINT) {
                fg = blend(fg, 0.5, bg);
            }
            if attrs.contains(Attrs::INVISIBLE) {
                fg = bg;
            }
            let wide = cell.flags.contains(CellFlags::WIDE);
            let width: u16 = if wide { 2 } else { 1 };
            if bg_set && bg != default_bg {
                let color = hsla(bg);
                match bgs.last_mut() {
                    Some(r) if r.col + r.len == x && r.color == color => r.len += width,
                    _ => bgs.push(BgRect {
                        col: x,
                        len: width,
                        color,
                    }),
                }
            }
            let mut text = String::new();
            text.push(
                char::from_u32(cell.codepoint)
                    .filter(|&c| c != '\0')
                    .unwrap_or(' '),
            );
            for &e in &cell.extra {
                if let Some(c) = char::from_u32(e) {
                    text.push(c);
                }
            }
            let underline = (attrs.underline() != Underline::None).then(|| UnderlineStyle {
                thickness: px(1.0),
                color: Some(hsla(pal.resolve_underline(style.underline_color, fg))),
                wavy: attrs.underline() == Underline::Curly,
            });
            let strike = attrs
                .contains(Attrs::STRIKETHROUGH)
                .then(|| StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(hsla(fg)),
                });
            let run = TextRun {
                len: text.len(),
                font: font(
                    if attrs.contains(Attrs::BOLD) {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    attrs.contains(Attrs::ITALIC),
                ),
                color: hsla(fg),
                background_color: None,
                underline,
                strikethrough: strike,
            };
            let alone = wide || cell.flags.contains(CellFlags::GRAPHEME);
            if alone {
                if let Some(s) = cur.take() {
                    segs.push(s);
                }
                segs.push(Segment {
                    col: x,
                    text,
                    runs: vec![run],
                });
                continue;
            }
            let seg = cur.get_or_insert_with(|| Segment {
                col: x,
                text: String::new(),
                runs: Vec::new(),
            });
            seg.text.push_str(&text);
            match seg.runs.last_mut() {
                Some(r)
                    if r.font == run.font
                        && r.color == run.color
                        && r.underline == run.underline
                        && r.strikethrough == run.strikethrough =>
                {
                    r.len += run.len;
                }
                _ => seg.runs.push(run),
            }
        }
        if let Some(s) = cur.take() {
            segs.push(s);
        }
        let row_y = bounds.origin.y + m.cell_h * y as f32;
        for r in &bgs {
            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.origin.x + m.cell_w * r.col as f32, row_y),
                    size(m.cell_w * r.len as f32, m.cell_h),
                ),
                r.color,
            ));
        }
        for s in segs {
            let line = window.text_system().shape_line(
                s.text.into(),
                m.font_size,
                &s.runs,
                Some(m.cell_w),
            );
            let origin = point(bounds.origin.x + m.cell_w * s.col as f32, row_y);
            let _ = line.paint(origin, m.cell_h, window, cx);
        }
    }

    let cursor = replica.cursor();
    let modes = replica.modes();
    let cursor_row = live_top - top + u64::from(cursor.row);
    if cursor.visible
        && modes.contains(Modes::CURSOR_VISIBLE)
        && (cursor_on || !focused)
        && cursor_row < u64::from(rows)
    {
        let at = point(
            bounds.origin.x + m.cell_w * cursor.col as f32,
            bounds.origin.y + m.cell_h * cursor_row as f32,
        );
        let color = hsla(pal.cursor);
        let quad = match (focused, cursor.shape) {
            (false, _) => gpui::outline(
                Bounds::new(at, size(m.cell_w, m.cell_h)),
                color,
                gpui::BorderStyle::Solid,
            ),
            (true, CursorShape::Bar) => fill(Bounds::new(at, size(px(2.0), m.cell_h)), color),
            (true, CursorShape::Underline) => fill(
                Bounds::new(
                    point(at.x, at.y + m.cell_h - px(2.0)),
                    size(m.cell_w, px(2.0)),
                ),
                color,
            ),
            (true, _) => fill(
                Bounds::new(at, size(m.cell_w, m.cell_h)),
                color.opacity(0.85),
            ),
        };
        window.paint_quad(quad);
    }
    missing
}
