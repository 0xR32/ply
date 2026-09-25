//! The reference C2 decoder's grid: what a client that applied every SNAPSHOT, DELTA and HISTORY frame shows.
//!
//! A [`Replica`] starts empty; the first Snapshot sizes it and replaces its style table, each Delta replaces the
//! rows it carries and adds its new styles, and History frames fill a scrollback cache keyed by row index. Frames
//! are validated strictly (sequence order, known style ids, rows within the grid), so a bug in the encoder side shows
//! up as an [`Error`] rather than a silently wrong screen. Rows keep the cells as sent: trailing default blanks may
//! be missing and read back as blanks. Each row carries a content hash over its resolved cells (style values, not
//! ids), stable across Snapshots, which a renderer can use as its run-cache key.
//!
//! plyd's tests and the CLI test client use it; the app has its own TypeScript replica (WP5).

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};

use ply_proto::data::{
    Cell, CellFlags, Cursor, Delta, Frame, History, Modes, Row, Snapshot, Style, StyleEntry,
};

use crate::error::{Error, Result};

/// One row of a [`Replica`]: the cells as sent plus a hash of their resolved content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplicaRow {
    /// The row soft-wraps onto the next.
    pub wrapped: bool,
    /// Cells from column 0; columns past the end are default blanks.
    pub cells: Vec<Cell>,
    /// Hash of `wrapped` and every cell with its style resolved, trailing default blanks ignored.
    pub hash: u64,
}

/// A cell with its style resolved from the style table, for comparing grids whose style ids differ.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedCell {
    /// Base codepoint; 0 for an empty cell.
    pub codepoint: u32,
    /// The cell's style.
    pub style: Style,
    /// Wide, spacer, spacer-head, grapheme.
    pub flags: CellFlags,
    /// Codepoints after the base of a grapheme cluster.
    pub extra: Vec<u32>,
}

/// The grid a C2 client holds; see the module docs for the rules it enforces.
#[derive(Debug, Clone, Default)]
pub struct Replica {
    cols: u16,
    rows: u16,
    lines: Vec<ReplicaRow>,
    styles: HashMap<u16, Style>,
    cursor: Cursor,
    modes: Modes,
    scrollback_rows: u32,
    history: BTreeMap<i64, ReplicaRow>,
    last_seq: Option<u64>,
}

impl Replica {
    /// An empty replica that has seen no frame yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a SNAPSHOT, DELTA or HISTORY frame; fails with [`Error::NotScreenFrame`] for any other kind and leaves the replica unchanged on every error.
    pub fn apply(&mut self, frame: &Frame) -> Result<()> {
        match frame {
            Frame::Snapshot(s) => self.apply_snapshot(s),
            Frame::Delta(d) => self.apply_delta(d),
            Frame::History(h) => self.apply_history(h),
            other => Err(Error::NotScreenFrame { kind: other.kind() }),
        }
    }

    /// Replaces the whole grid, the style table and the history cache; fails on a sequence regression, a duplicate or zero style id, or a bad row.
    pub fn apply_snapshot(&mut self, snapshot: &Snapshot) -> Result<()> {
        self.check_seq(snapshot.seq)?;
        let empty = HashMap::new();
        let styles = new_styles(&empty, &snapshot.styles)?;
        let lookup = Styles {
            table: &styles,
            added: &empty,
        };
        let blank = ReplicaRow {
            wrapped: false,
            cells: Vec::new(),
            hash: row_hash(false, &[], &lookup)?,
        };
        let mut lines = vec![blank; usize::from(snapshot.rows)];
        for row in &snapshot.lines {
            let y = screen_index(row.index, snapshot.rows)?;
            lines[y] = make_row(row, snapshot.cols, &lookup)?;
        }
        self.cols = snapshot.cols;
        self.rows = snapshot.rows;
        self.lines = lines;
        self.styles = styles;
        self.cursor = snapshot.cursor;
        self.modes = snapshot.modes;
        self.scrollback_rows = snapshot.scrollback_rows;
        self.history.clear();
        self.last_seq = Some(snapshot.seq);
        Ok(())
    }

    /// Replaces the rows a Delta carries and adds its styles; fails before the first Snapshot, on a sequence regression, a re-used style id or a bad row.
    pub fn apply_delta(&mut self, delta: &Delta) -> Result<()> {
        if self.last_seq.is_none() {
            return Err(Error::NoSnapshot {
                kind: ply_proto::data::kind::DELTA,
            });
        }
        self.check_seq(delta.seq)?;
        let added = new_styles(&self.styles, &delta.styles_added)?;
        let lookup = Styles {
            table: &self.styles,
            added: &added,
        };
        let mut replaced = Vec::with_capacity(delta.lines.len());
        for row in &delta.lines {
            let y = screen_index(row.index, self.rows)?;
            replaced.push((y, make_row(row, self.cols, &lookup)?));
        }
        for (y, row) in replaced {
            self.lines[y] = row;
        }
        self.styles.extend(added);
        self.cursor = delta.cursor;
        self.modes = delta.modes;
        self.scrollback_rows = delta.scrollback_rows;
        self.last_seq = Some(delta.seq);
        Ok(())
    }

    /// Stores scrollback rows by index and adds their styles; fails before the first Snapshot, on a re-used style id or a non-negative row index.
    pub fn apply_history(&mut self, history: &History) -> Result<()> {
        if self.last_seq.is_none() {
            return Err(Error::NoSnapshot {
                kind: ply_proto::data::kind::HISTORY,
            });
        }
        let added = new_styles(&self.styles, &history.styles_added)?;
        let lookup = Styles {
            table: &self.styles,
            added: &added,
        };
        let mut rows = Vec::with_capacity(history.lines.len());
        for row in &history.lines {
            if row.index >= 0 {
                return Err(Error::RowOutOfRange {
                    index: row.index,
                    rows: self.rows,
                });
            }
            rows.push((i64::from(row.index), make_row(row, self.cols, &lookup)?));
        }
        self.history.extend(rows);
        self.styles.extend(added);
        Ok(())
    }

    /// Grid size as `(cols, rows)`; `(0, 0)` before the first Snapshot.
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Sequence number of the last Snapshot or Delta applied, the value a client acknowledges.
    pub fn last_seq(&self) -> Option<u64> {
        self.last_seq
    }

    /// Cursor after the last frame.
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Display modes after the last frame.
    pub fn modes(&self) -> Modes {
        self.modes
    }

    /// Scrollback rows plyd reported with the last Snapshot or Delta.
    pub fn scrollback_rows(&self) -> u32 {
        self.scrollback_rows
    }

    /// Screen row `y` (0 is the top), or `None` outside the grid.
    pub fn row(&self, y: u16) -> Option<&ReplicaRow> {
        self.lines.get(usize::from(y))
    }

    /// A cached scrollback row by its C2 index (negative), or `None` when no History carried it.
    pub fn history_row(&self, index: i64) -> Option<&ReplicaRow> {
        self.history.get(&index)
    }

    /// The style for `id`; id 0 is always the default style.
    pub fn style(&self, id: u16) -> Option<Style> {
        if id == 0 {
            Some(Style::default())
        } else {
            self.styles.get(&id).copied()
        }
    }

    /// Row `y` with styles resolved and trailing default blanks dropped, or `None` outside the grid.
    pub fn resolved_row(&self, y: u16) -> Option<Vec<ResolvedCell>> {
        let row = self.row(y)?;
        let empty = HashMap::new();
        let lookup = Styles {
            table: &self.styles,
            added: &empty,
        };
        resolve_cells(&row.cells, &lookup).ok()
    }

    /// Row `y` as text: spacer cells skipped, empty cells as spaces, trailing spaces trimmed; empty outside the grid.
    pub fn row_text(&self, y: u16) -> String {
        self.row(y)
            .map(|r| cells_text(&r.cells))
            .unwrap_or_default()
    }

    /// Every screen row as text, top to bottom (see [`Replica::row_text`]).
    pub fn screen_text(&self) -> Vec<String> {
        (0..self.rows).map(|y| self.row_text(y)).collect()
    }

    fn check_seq(&self, seq: u64) -> Result<()> {
        match self.last_seq {
            Some(last) if seq <= last => Err(Error::SequenceRegressed { last, got: seq }),
            _ => Ok(()),
        }
    }
}

/// The text of a row of cells: spacers skipped, empty cells as spaces, grapheme codepoints appended, trailing spaces trimmed.
pub fn cells_text(cells: &[Cell]) -> String {
    let mut text = String::with_capacity(cells.len());
    for cell in cells {
        if cell.flags.contains(CellFlags::SPACER) || cell.flags.contains(CellFlags::SPACER_HEAD) {
            continue;
        }
        text.push(
            char::from_u32(cell.codepoint)
                .filter(|_| cell.codepoint != 0)
                .unwrap_or(' '),
        );
        text.extend(cell.extra.iter().filter_map(|&cp| char::from_u32(cp)));
    }
    text.truncate(text.trim_end_matches(' ').len());
    text
}

fn screen_index(index: i32, rows: u16) -> Result<usize> {
    usize::try_from(index)
        .ok()
        .filter(|&y| y < usize::from(rows))
        .ok_or(Error::RowOutOfRange { index, rows })
}

/// A style table plus the entries a frame adds, looked up without copying the table.
struct Styles<'a> {
    table: &'a HashMap<u16, Style>,
    added: &'a HashMap<u16, Style>,
}

impl Styles<'_> {
    fn get(&self, id: u16) -> Option<Style> {
        self.table.get(&id).or_else(|| self.added.get(&id)).copied()
    }
}

/// The entries of `entries` as a map, refusing id 0 and any id already in `table` or repeated.
fn new_styles(table: &HashMap<u16, Style>, entries: &[StyleEntry]) -> Result<HashMap<u16, Style>> {
    let mut added = HashMap::with_capacity(entries.len());
    for entry in entries {
        if entry.id == 0
            || table.contains_key(&entry.id)
            || added.insert(entry.id, entry.style).is_some()
        {
            return Err(Error::DuplicateStyle { id: entry.id });
        }
    }
    Ok(added)
}

fn make_row(row: &Row, cols: u16, styles: &Styles<'_>) -> Result<ReplicaRow> {
    if row.cells.len() > usize::from(cols) {
        return Err(Error::RowTooWide {
            index: row.index,
            cells: row.cells.len(),
            cols,
        });
    }
    Ok(ReplicaRow {
        wrapped: row.wrapped,
        cells: row.cells.clone(),
        hash: row_hash(row.wrapped, &row.cells, styles)?,
    })
}

fn resolve_cells(cells: &[Cell], styles: &Styles<'_>) -> Result<Vec<ResolvedCell>> {
    let mut out = Vec::with_capacity(cells.len());
    for cell in cells {
        let style = if cell.style == 0 {
            Style::default()
        } else {
            styles
                .get(cell.style)
                .ok_or(Error::UnknownStyle { id: cell.style })?
        };
        out.push(ResolvedCell {
            codepoint: cell.codepoint,
            style,
            flags: cell.flags,
            extra: cell.extra.clone(),
        });
    }
    let blank = ResolvedCell {
        codepoint: 0,
        style: Style::default(),
        flags: CellFlags::empty(),
        extra: Vec::new(),
    };
    while out.last() == Some(&blank) {
        out.pop();
    }
    Ok(out)
}

fn row_hash(wrapped: bool, cells: &[Cell], styles: &Styles<'_>) -> Result<u64> {
    let mut hasher = DefaultHasher::new();
    wrapped.hash(&mut hasher);
    resolve_cells(cells, styles)?.hash(&mut hasher);
    Ok(hasher.finish())
}

#[cfg(test)]
mod tests {
    use ply_proto::data::{Attrs, Color, CursorShape};

    use super::*;

    fn cell(c: char, style: u16) -> Cell {
        Cell {
            codepoint: c as u32,
            style,
            flags: CellFlags::empty(),
            extra: Vec::new(),
        }
    }

    fn red() -> Style {
        Style {
            fg: Color::Indexed(1),
            ..Style::default()
        }
    }

    fn snapshot(seq: u64) -> Snapshot {
        Snapshot {
            seq,
            cols: 4,
            rows: 2,
            cursor: Cursor {
                col: 1,
                row: 0,
                shape: CursorShape::Block,
                visible: true,
                blinking: false,
            },
            modes: Modes::CURSOR_VISIBLE,
            scrollback_rows: 0,
            styles: vec![StyleEntry {
                id: 1,
                style: red(),
            }],
            lines: vec![
                Row {
                    index: 0,
                    wrapped: false,
                    cells: vec![cell('h', 1), cell('i', 0)],
                },
                Row {
                    index: 1,
                    wrapped: false,
                    cells: vec![],
                },
            ],
        }
    }

    #[test]
    fn a_snapshot_then_a_delta_rebuild_the_screen() {
        let mut r = Replica::new();
        r.apply(&Frame::Snapshot(snapshot(1))).unwrap();
        assert_eq!(r.screen_text(), vec!["hi", ""]);
        let bold = Style {
            attrs: Attrs::BOLD,
            ..Style::default()
        };
        r.apply(&Frame::Delta(Delta {
            seq: 2,
            cursor: Cursor::default(),
            modes: Modes::empty(),
            scrollback_rows: 3,
            styles_added: vec![StyleEntry { id: 2, style: bold }],
            lines: vec![Row {
                index: 1,
                wrapped: true,
                cells: vec![cell('o', 2), cell('k', 1)],
            }],
        }))
        .unwrap();
        assert_eq!(r.screen_text(), vec!["hi", "ok"]);
        assert_eq!(r.resolved_row(1).unwrap()[0].style, bold);
        assert_eq!(r.scrollback_rows(), 3);
        assert_eq!(r.last_seq(), Some(2));
    }

    #[test]
    fn the_row_hash_ignores_ids_and_trailing_blanks() {
        let mut a = Replica::new();
        a.apply_snapshot(&snapshot(1)).unwrap();
        let mut s = snapshot(1);
        s.styles[0].id = 9;
        s.lines[0].cells = vec![cell('h', 9), cell('i', 0), Cell::default(), Cell::default()];
        let mut b = Replica::new();
        b.apply_snapshot(&s).unwrap();
        assert_eq!(a.row(0).unwrap().hash, b.row(0).unwrap().hash);
        assert_ne!(a.row(0).unwrap().hash, a.row(1).unwrap().hash);
    }

    #[test]
    fn strict_frames_are_refused_without_changing_the_grid() {
        let mut r = Replica::new();
        let delta = Delta {
            seq: 5,
            cursor: Cursor::default(),
            modes: Modes::empty(),
            scrollback_rows: 0,
            styles_added: vec![],
            lines: vec![],
        };
        assert!(matches!(
            r.apply_delta(&delta),
            Err(Error::NoSnapshot { .. })
        ));
        r.apply_snapshot(&snapshot(3)).unwrap();
        assert!(matches!(
            r.apply_delta(&Delta {
                seq: 3,
                ..delta.clone()
            }),
            Err(Error::SequenceRegressed { last: 3, got: 3 })
        ));
        let bad_style = Delta {
            seq: 4,
            lines: vec![Row {
                index: 0,
                wrapped: false,
                cells: vec![cell('x', 7)],
            }],
            ..delta.clone()
        };
        assert!(matches!(
            r.apply_delta(&bad_style),
            Err(Error::UnknownStyle { id: 7 })
        ));
        let out_of_range = Delta {
            seq: 4,
            lines: vec![Row {
                index: 2,
                wrapped: false,
                cells: vec![],
            }],
            ..delta.clone()
        };
        assert!(matches!(
            r.apply_delta(&out_of_range),
            Err(Error::RowOutOfRange { index: 2, rows: 2 })
        ));
        let reused = Delta {
            seq: 4,
            styles_added: vec![StyleEntry {
                id: 1,
                style: red(),
            }],
            ..delta
        };
        assert!(matches!(
            r.apply_delta(&reused),
            Err(Error::DuplicateStyle { id: 1 })
        ));
        assert_eq!(r.screen_text(), vec!["hi", ""]);
        assert_eq!(r.last_seq(), Some(3));
        assert!(matches!(
            r.apply(&Frame::Bell),
            Err(Error::NotScreenFrame { kind: 0x24 })
        ));
    }

    #[test]
    fn history_rows_are_cached_by_index() {
        let mut r = Replica::new();
        r.apply_snapshot(&snapshot(1)).unwrap();
        r.apply_history(&History {
            start: -2,
            lines: vec![
                Row {
                    index: -2,
                    wrapped: false,
                    cells: vec![cell('a', 3)],
                },
                Row {
                    index: -1,
                    wrapped: false,
                    cells: vec![cell('b', 0)],
                },
            ],
            styles_added: vec![StyleEntry {
                id: 3,
                style: red(),
            }],
        })
        .unwrap();
        assert_eq!(cells_text(&r.history_row(-2).unwrap().cells), "a");
        assert_eq!(cells_text(&r.history_row(-1).unwrap().cells), "b");
        assert!(r.history_row(-3).is_none());
        r.apply_snapshot(&snapshot(2)).unwrap();
        assert!(r.history_row(-1).is_none());
    }
}
