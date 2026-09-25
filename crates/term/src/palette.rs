use ply_proto::data::Color;
use ply_proto::pane::{Rgb, TerminalTheme};

const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// A pane's colours (spec R-R4): the theme's ANSI 0–15, defaults, cursor and selection, plus the derived xterm 256-colour table; plain `Send + Sync` data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// ANSI colours 0–15.
    pub ansi: [Rgb; 16],
    /// Default foreground; answers OSC 10.
    pub fg: Rgb,
    /// Default background; answers OSC 11.
    pub bg: Rgb,
    /// Cursor colour; answers OSC 12.
    pub cursor: Rgb,
    /// Text drawn under a block cursor.
    pub cursor_text: Rgb,
    /// Selection fill.
    pub selection_bg: Rgb,
    /// Text inside the selection.
    pub selection_fg: Rgb,
}

impl Palette {
    /// The palette of a C1 theme, field for field.
    pub fn from_theme(theme: &TerminalTheme) -> Self {
        Self {
            ansi: theme.ansi,
            fg: theme.fg,
            bg: theme.bg,
            cursor: theme.cursor,
            cursor_text: theme.cursor_text,
            selection_bg: theme.selection_bg,
            selection_fg: theme.selection_fg,
        }
    }

    /// Colour `index` of the 256-colour table: 0–15 from [`Palette::ansi`], 16–231 the xterm cube (levels 0, 95, 135, 175, 215, 255), 232–255 greys 8..=238 in steps of 10.
    pub fn indexed(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[usize::from(index)],
            16..=231 => {
                let i = index - 16;
                let level = |v: u8| CUBE_LEVELS[usize::from(v)];
                Rgb {
                    r: level(i / 36),
                    g: level((i / 6) % 6),
                    b: level(i % 6),
                }
            }
            _ => {
                let v = 8 + 10 * (index - 232);
                Rgb { r: v, g: v, b: v }
            }
        }
    }

    /// The whole 256-colour table, as libghostty-vt's `OPT_COLOR_PALETTE` takes it.
    pub fn table(&self) -> [Rgb; 256] {
        let mut table = [Rgb { r: 0, g: 0, b: 0 }; 256];
        for (i, slot) in (0..=255u8).zip(table.iter_mut()) {
            *slot = self.indexed(i);
        }
        table
    }

    /// A foreground colour: `Default` is [`Palette::fg`].
    pub fn resolve_fg(&self, color: Color) -> Rgb {
        self.resolve(color, self.fg)
    }

    /// A background colour: `Default` is [`Palette::bg`].
    pub fn resolve_bg(&self, color: Color) -> Rgb {
        self.resolve(color, self.bg)
    }

    /// An underline colour: `Default` follows the cell's resolved foreground `fg`.
    pub fn resolve_underline(&self, color: Color, fg: Rgb) -> Rgb {
        self.resolve(color, fg)
    }

    /// True when the default background's relative luminance is below one half; answers the `CSI ? 996 n` colour-scheme query.
    pub fn is_dark(&self) -> bool {
        let Rgb { r, g, b } = self.bg;
        2126 * u32::from(r) + 7152 * u32::from(g) + 722 * u32::from(b) < 10_000 * 255 / 2
    }

    fn resolve(&self, color: Color, default: Rgb) -> Rgb {
        match color {
            Color::Default => default,
            Color::Indexed(i) => self.indexed(i),
            Color::Rgb(r, g, b) => Rgb { r, g, b },
        }
    }
}

impl From<&TerminalTheme> for Palette {
    fn from(theme: &TerminalTheme) -> Self {
        Self::from_theme(theme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    fn example() -> Palette {
        let mut ansi = [rgb(0, 0, 0); 16];
        for (i, c) in ansi.iter_mut().enumerate() {
            *c = rgb(i as u8, 0, 0);
        }
        Palette {
            ansi,
            fg: rgb(200, 200, 200),
            bg: rgb(10, 10, 10),
            cursor: rgb(1, 2, 3),
            cursor_text: rgb(4, 5, 6),
            selection_bg: rgb(7, 8, 9),
            selection_fg: rgb(10, 11, 12),
        }
    }

    #[test]
    fn the_table_follows_xterm() {
        let p = example();
        assert_eq!(p.indexed(3), rgb(3, 0, 0));
        assert_eq!(p.indexed(16), rgb(0, 0, 0));
        assert_eq!(p.indexed(21), rgb(0, 0, 255));
        assert_eq!(p.indexed(196), rgb(255, 0, 0));
        assert_eq!(p.indexed(231), rgb(255, 255, 255));
        assert_eq!(p.indexed(232), rgb(8, 8, 8));
        assert_eq!(p.indexed(255), rgb(238, 238, 238));
        assert_eq!(p.table()[124], p.indexed(124));
    }

    #[test]
    fn symbolic_colours_resolve_against_the_slot_default() {
        let p = example();
        assert_eq!(p.resolve_fg(Color::Default), p.fg);
        assert_eq!(p.resolve_bg(Color::Default), p.bg);
        assert_eq!(p.resolve_bg(Color::Indexed(1)), rgb(1, 0, 0));
        assert_eq!(p.resolve_fg(Color::Rgb(9, 8, 7)), rgb(9, 8, 7));
        assert_eq!(
            p.resolve_underline(Color::Default, rgb(5, 5, 5)),
            rgb(5, 5, 5)
        );
    }

    #[test]
    fn darkness_follows_the_background() {
        let mut p = example();
        assert!(p.is_dark());
        p.bg = rgb(250, 250, 245);
        assert!(!p.is_dark());
    }
}
