//! Per-client C2 frame building (spec 4.2): Snapshots, Deltas of changed rows and History pages.
//!
//! A [`DeltaBuilder`] belongs to one attached client. It owns that client's style table (C2 style ids are ply's own,
//! interned by value, because libghostty-vt's style ids are page-local) and sequence numbers. A Snapshot resets the
//! table and carries all of it; a Delta carries only the rows whose generation in the [`Engine`] is newer than the
//! client's last frame, plus the styles first used since. A client that has not been sent anything, whose grid size
//! changed, or whose table is full (65 535 styles) gets a Snapshot instead of a Delta. Trailing default blank cells
//! are left out of every row.
//!
//! A History page never exceeds [`MAX_FRAME_LEN`]: it stops at the last row that fits, and its `styles_added` may
//! hold styles of rows it left out, which the client must still add to its table (later frames use those ids without
//! resending them). Snapshots and Deltas are bounded by the grid, at most 23 bytes a cell plus grapheme codepoints.
//! Every frame carries the engine's [`Engine::scrollback_base`], and History pages are addressed by absolute line, so
//! a client's fetched rows stay valid while the line cap drops the oldest ones.

use std::collections::HashMap;

use ply_proto::data::{
    Cell, CellFlags, Cursor, Delta, FetchHistory, Frame, History, MAX_FRAME_LEN, Modes, Row,
    Snapshot, Style, StyleEntry,
};

use crate::engine::{Engine, RowSink};
use crate::error::Result;

/// Bytes of a frame before its rows, generous for every kind, used to keep History pages under the frame cap.
const FRAME_OVERHEAD: usize = 64;
/// Encoded bytes of one style entry (`id:u16 · fg · bg · underline colour (4 each) · attrs:u16`), pinned by a test against the encoder.
const STYLE_ENTRY_LEN: usize = 16;
/// Encoded bytes of a row header (`index:i32 · flags:u8 · n:u16`).
const ROW_HEADER_LEN: usize = 7;
/// Encoded bytes of a cell without grapheme codepoints (`codepoint:u32 · style:u16 · flags:u8`).
const CELL_LEN: usize = 7;

/// A frame that brings a client up to date: a Delta normally, a Snapshot when one is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// The whole screen and a fresh style table.
    Snapshot(Snapshot),
    /// Changed rows and new styles.
    Delta(Delta),
}

impl From<Update> for Frame {
    fn from(update: Update) -> Self {
        match update {
            Update::Snapshot(s) => Frame::Snapshot(s),
            Update::Delta(d) => Frame::Delta(d),
        }
    }
}

/// One attached client's view of an [`Engine`]: its style table, sequence numbers and what it was last sent; see the module docs.
#[derive(Debug, Default)]
pub struct DeltaBuilder {
    styles: HashMap<Style, u16>,
    next_id: u16,
    seq: u64,
    sent_generation: u64,
    sent: Option<Sent>,
    table_full_logged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sent {
    cols: u16,
    rows: u16,
    cursor: Cursor,
    modes: Modes,
    scrollback_rows: u32,
    scrollback_base: u64,
}

impl DeltaBuilder {
    /// A builder for a client that has been sent nothing; its first frame must be [`DeltaBuilder::snapshot`].
    pub fn new() -> Self {
        Self::default()
    }

    /// The sequence number of the last Snapshot or Delta built (0 before the first), the value the client acknowledges.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The whole screen with a complete, fresh style table; answers ATTACH, RESIZE and plyd's forced resend (spec 4.2); errors: [`crate::Error::Ghostty`] when the render state cannot be read.
    pub fn snapshot(&mut self, engine: &mut Engine) -> Result<Snapshot> {
        engine.refresh()?;
        self.styles.clear();
        self.next_id = 1;
        self.table_full_logged = false;
        let (cols, rows) = engine.render_size()?;
        let mut sink = RowCollector::new(self, cols, engine.pane_id());
        engine.read_rows(|_| true, &mut sink)?;
        let (lines, styles) = sink.finish();
        let sent = Sent {
            cols,
            rows,
            cursor: engine.cursor()?,
            modes: engine.modes(),
            scrollback_rows: engine.scrollback_rows(),
            scrollback_base: engine.scrollback_base(),
        };
        self.seq += 1;
        self.sent_generation = engine.generation();
        self.sent = Some(sent);
        Ok(Snapshot {
            seq: self.seq,
            cols,
            rows,
            cursor: sent.cursor,
            modes: sent.modes,
            scrollback_rows: sent.scrollback_rows,
            scrollback_base: sent.scrollback_base,
            styles,
            lines,
        })
    }

    /// What changed since this client's last frame: rows newer than it, or a row-less Delta when only the cursor, modes or scrollback count or base changed (Ruling R39); `None` when nothing changed (R-R21); a Snapshot when one is required; errors: [`crate::Error::Ghostty`].
    pub fn delta(&mut self, engine: &mut Engine) -> Result<Option<Update>> {
        engine.refresh()?;
        let (cols, rows) = engine.render_size()?;
        let Some(last) = self.sent.filter(|s| s.cols == cols && s.rows == rows) else {
            return self.snapshot(engine).map(|s| Some(Update::Snapshot(s)));
        };
        if self.next_id == u16::MAX {
            return self.snapshot(engine).map(|s| Some(Update::Snapshot(s)));
        }
        let now = Sent {
            cols,
            rows,
            cursor: engine.cursor()?,
            modes: engine.modes(),
            scrollback_rows: engine.scrollback_rows(),
            scrollback_base: engine.scrollback_base(),
        };
        let since = self.sent_generation;
        let changed: Vec<bool> = engine
            .row_generations()
            .iter()
            .map(|&g| g > since)
            .collect();
        if !changed.contains(&true) && now == last {
            return Ok(None);
        }
        let mut sink = RowCollector::new(self, cols, engine.pane_id());
        engine.read_rows(
            |y| changed.get(usize::from(y)).copied().unwrap_or(false),
            &mut sink,
        )?;
        let (lines, styles_added) = sink.finish();
        self.seq += 1;
        self.sent_generation = engine.generation();
        self.sent = Some(now);
        Ok(Some(Update::Delta(Delta {
            seq: self.seq,
            cursor: now.cursor,
            modes: now.modes,
            scrollback_rows: now.scrollback_rows,
            scrollback_base: now.scrollback_base,
            styles_added,
            lines,
        })))
    }

    /// Scrollback lines answering FETCH_HISTORY `start..start + count` (absolute), clipped to what exists and to one frame (the client asks again for the rest); styles are interned into this client's table; errors: [`crate::Error::Ghostty`].
    pub fn history(&mut self, engine: &mut Engine, start: u64, count: u16) -> Result<History> {
        let base = engine.scrollback_base();
        let top = base + u64::from(engine.scrollback_rows());
        let lo = start.max(base);
        let hi = start.saturating_add(u64::from(count)).min(top);
        let (cols, _) = engine.size();
        // One more row can add at most a header, its cells and one new style per cell (graphemes are trimmed below).
        let row_margin = ROW_HEADER_LEN + usize::from(cols) * (CELL_LEN + STYLE_ENTRY_LEN);
        let mut sink = RowCollector::new(self, cols, engine.pane_id());
        if lo < hi {
            // Both ends lie within the scrollback, so their distance to the screen fits a u32.
            let relative = |line: u64| -i64::from(u32::try_from(top - line).unwrap_or(u32::MAX));
            engine.read_history(relative(lo), relative(hi), &mut sink, |s| {
                s.encoded_len + s.added.len() * STYLE_ENTRY_LEN + FRAME_OVERHEAD + row_margin
                    <= MAX_FRAME_LEN
            })?;
        }
        let (mut lines, styles_added) = sink.finish();
        let mut total = FRAME_OVERHEAD
            + styles_added.len() * STYLE_ENTRY_LEN
            + lines.iter().map(encoded_row_len).sum::<usize>();
        while total > MAX_FRAME_LEN {
            let Some(row) = lines.pop() else { break };
            total -= encoded_row_len(&row);
        }
        for (i, row) in lines.iter_mut().enumerate() {
            row.index = i32::try_from(i).unwrap_or(i32::MAX);
        }
        Ok(History {
            start: if lines.is_empty() { start } else { lo },
            lines,
            styles_added,
        })
    }

    /// [`DeltaBuilder::history`] for a decoded FETCH_HISTORY frame.
    pub fn fetch_history(
        &mut self,
        engine: &mut Engine,
        request: &FetchHistory,
    ) -> Result<History> {
        self.history(engine, request.start, request.count)
    }

    fn intern(&mut self, style: &Style, added: &mut Vec<StyleEntry>, pane_id: u64) -> u16 {
        if *style == Style::default() {
            return 0;
        }
        if let Some(&id) = self.styles.get(style) {
            return id;
        }
        if self.next_id == u16::MAX {
            if !self.table_full_logged {
                tracing::warn!(
                    pane_id,
                    styles = self.styles.len(),
                    "C2 style table is full; new styles draw as the default until the next Snapshot"
                );
                self.table_full_logged = true;
            }
            return 0;
        }
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        self.styles.insert(*style, id);
        added.push(StyleEntry { id, style: *style });
        id
    }
}

/// Collects rows from the engine into C2 rows, interning styles and trimming trailing default blanks.
struct RowCollector<'a> {
    builder: &'a mut DeltaBuilder,
    rows: Vec<Row>,
    added: Vec<StyleEntry>,
    current: Row,
    last_style: (Style, u16),
    cols: usize,
    encoded_len: usize,
    pane_id: u64,
}

impl<'a> RowCollector<'a> {
    fn new(builder: &'a mut DeltaBuilder, cols: u16, pane_id: u64) -> Self {
        Self {
            pane_id,
            builder,
            rows: Vec::new(),
            added: Vec::new(),
            current: Row::default(),
            last_style: (Style::default(), 0),
            cols: usize::from(cols),
            encoded_len: 0,
        }
    }

    fn finish(self) -> (Vec<Row>, Vec<StyleEntry>) {
        (self.rows, self.added)
    }
}

impl RowSink for RowCollector<'_> {
    fn begin_row(&mut self, index: i32, wrapped: bool) {
        self.current = Row {
            index,
            wrapped,
            cells: Vec::with_capacity(self.cols),
        };
    }

    fn cell(&mut self, codepoint: u32, style: &Style, flags: CellFlags, extra: &[u32]) {
        let id = if *style == self.last_style.0 {
            self.last_style.1
        } else {
            let id = self.builder.intern(style, &mut self.added, self.pane_id);
            self.last_style = (*style, id);
            id
        };
        let extra = if extra.len() > usize::from(u8::MAX) {
            tracing::debug!(
                pane_id = self.pane_id,
                len = extra.len(),
                "grapheme cluster longer than C2 carries; truncated to 255 codepoints"
            );
            &extra[..usize::from(u8::MAX)]
        } else {
            extra
        };
        self.current.cells.push(Cell {
            codepoint,
            style: id,
            flags,
            extra: extra.to_vec(),
        });
    }

    fn end_row(&mut self) {
        let mut row = std::mem::take(&mut self.current);
        while row.cells.last().is_some_and(is_default_blank) {
            row.cells.pop();
        }
        self.encoded_len += encoded_row_len(&row);
        self.rows.push(row);
    }
}

fn is_default_blank(cell: &Cell) -> bool {
    cell.codepoint == 0 && cell.style == 0 && cell.flags == CellFlags::empty()
}

fn encoded_row_len(row: &Row) -> usize {
    ROW_HEADER_LEN
        + row
            .cells
            .iter()
            .map(|c| {
                CELL_LEN
                    + if c.extra.is_empty() {
                        0
                    } else {
                        1 + 4 * c.extra.len()
                    }
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use ply_proto::data::{Color, Frame, History, Style, StyleEntry};

    use super::STYLE_ENTRY_LEN;

    #[test]
    fn the_style_entry_size_matches_the_encoder() {
        let encoded = |styles_added: Vec<StyleEntry>| {
            let mut wire = Vec::new();
            Frame::History(History {
                start: 1,
                lines: vec![],
                styles_added,
            })
            .encode(&mut wire)
            .unwrap();
            wire.len()
        };
        let entry = StyleEntry {
            id: 1,
            style: Style {
                fg: Color::Rgb(1, 2, 3),
                ..Style::default()
            },
        };
        assert_eq!(encoded(vec![entry]) - encoded(vec![]), STYLE_ENTRY_LEN);
    }
}
