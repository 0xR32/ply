//! Selection, copy, find and links over a pane's replica, by absolute line and cell.

use ply_proto::data::CellFlags;
use ply_term::{Replica, ReplicaRow};

/// A cell by absolute line (scrollback and live screen counted together) and column.
pub type Pos = (u64, u16);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    pub anchor: Pos,
    pub head: Pos,
}

impl Selection {
    pub fn at(pos: Pos) -> Self {
        Self {
            anchor: pos,
            head: pos,
        }
    }

    /// Start and end in reading order, both inclusive.
    pub fn ordered(&self) -> (Pos, Pos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// The first absolute line of the live screen.
pub fn live_top(r: &Replica) -> u64 {
    r.scrollback_base() + u64::from(r.scrollback_rows())
}

/// The row at absolute `line`: scrollback from the fetched pages, else the live screen.
pub fn row_at(r: &Replica, line: u64) -> Option<&ReplicaRow> {
    let top = live_top(r);
    if line < top {
        r.history_row(line)
    } else {
        r.row(u16::try_from(line - top).ok()?)
    }
}

/// The row's text and, per char, the column it sits in; spacer cells add nothing and an empty cell is a space.
fn row_text(row: &ReplicaRow) -> (Vec<char>, Vec<u16>) {
    let (mut text, mut cols) = (Vec::new(), Vec::new());
    for (x, cell) in row.cells.iter().enumerate() {
        if cell.flags.contains(CellFlags::SPACER) || cell.flags.contains(CellFlags::SPACER_HEAD) {
            continue;
        }
        let col = u16::try_from(x).unwrap_or(u16::MAX);
        text.push(
            char::from_u32(cell.codepoint)
                .filter(|&c| c != '\0')
                .unwrap_or(' '),
        );
        cols.push(col);
        for c in cell.extra.iter().filter_map(|&e| char::from_u32(e)) {
            text.push(c);
            cols.push(col);
        }
    }
    (text, cols)
}

/// The selected text: rows joined with newlines except after a soft-wrapped row, each line's trailing blanks trimmed.
pub fn copy_text(r: &Replica, sel: &Selection) -> String {
    let ((l0, c0), (l1, c1)) = sel.ordered();
    let mut out = String::new();
    for line in l0..=l1 {
        let row = row_at(r, line);
        if let Some(row) = row {
            let (text, cols) = row_text(row);
            let from = if line == l0 { c0 } else { 0 };
            let to = if line == l1 { c1 } else { u16::MAX };
            let piece: String = text
                .iter()
                .zip(&cols)
                .filter(|&(_, &c)| c >= from && c <= to)
                .map(|(&ch, _)| ch)
                .collect();
            out.push_str(piece.trim_end());
        }
        if line < l1 && !row.is_some_and(|r| r.wrapped) {
            out.push('\n');
        }
    }
    out
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || "_-./~:@%+#".contains(c)
}

/// The word around `pos` (letters, digits and `_-./~:@%+#`), or the cell alone when it is not part of one.
pub fn word_at(r: &Replica, pos: Pos) -> Selection {
    let Some(row) = row_at(r, pos.0) else {
        return Selection::at(pos);
    };
    let (text, cols) = row_text(row);
    let Some(i) = cols.iter().rposition(|&c| c <= pos.1) else {
        return Selection::at(pos);
    };
    if !text.get(i).copied().is_some_and(word_char) {
        return Selection::at(pos);
    }
    let (mut a, mut b) = (i, i);
    while a > 0 && word_char(text[a - 1]) {
        a -= 1;
    }
    while b + 1 < text.len() && word_char(text[b + 1]) {
        b += 1;
    }
    Selection {
        anchor: (pos.0, cols[a]),
        head: (pos.0, cols[b]),
    }
}

/// Every case-insensitive match of `query` in lines `from..to`, oldest first, each from its first to its last cell.
pub fn find(r: &Replica, query: &str, from: u64, to: u64) -> Vec<Selection> {
    let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
    let q: Vec<char> = query.chars().map(lower).collect();
    let mut out = Vec::new();
    if q.is_empty() {
        return out;
    }
    for line in from..to {
        let Some(row) = row_at(r, line) else { continue };
        let (text, cols) = row_text(row);
        let text: Vec<char> = text.into_iter().map(lower).collect();
        let mut i = 0;
        while i + q.len() <= text.len() {
            if text[i..i + q.len()] == q[..] {
                out.push(Selection {
                    anchor: (line, cols[i]),
                    head: (line, cols[i + q.len() - 1]),
                });
                i += q.len();
            } else {
                i += 1;
            }
        }
    }
    out
}

/// A link's cells: absolute line and the first and last column on it.
pub type Segment = (u64, u16, u16);

const MAX_WRAPPED_ROWS: u64 = 64;

fn url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c)
}

fn unbalanced(url: &[char], open: char, close: char) -> bool {
    url.iter().fold(0i32, |d, &c| {
        if c == open {
            d + 1
        } else if c == close {
            d - 1
        } else {
            d
        }
    }) < 0
}

/// The http(s) URL under `pos` in its soft-wrapped line, with the cells it covers; sentence punctuation and an
/// unpartnered `)` or `]` at its end are left out.
pub fn link_at(r: &Replica, pos: Pos) -> Option<(String, Vec<Segment>)> {
    let mut first = pos.0;
    while pos.0 - first < MAX_WRAPPED_ROWS
        && first > r.scrollback_base()
        && row_at(r, first - 1).is_some_and(|row| row.wrapped)
    {
        first -= 1;
    }
    let (mut text, mut at): (Vec<char>, Vec<Pos>) = (Vec::new(), Vec::new());
    let mut line = first;
    while let Some(row) = row_at(r, line) {
        let (t, cols) = row_text(row);
        at.extend(cols.iter().map(|&c| (line, c)));
        text.extend(t);
        if !row.wrapped || line - first >= 2 * MAX_WRAPPED_ROWS {
            break;
        }
        line += 1;
    }
    let mut i = 0;
    while i < text.len() {
        let rest: String = text[i..text.len().min(i + 8)].iter().collect();
        if !(rest.starts_with("https://") || rest.starts_with("http://")) {
            i += 1;
            continue;
        }
        let mut end = i;
        while end < text.len() && url_char(text[end]) {
            end += 1;
        }
        loop {
            let url = &text[i..end];
            match url.last() {
                Some('.' | ',' | ':' | ';' | '!' | '?' | '\'' | '*') => end -= 1,
                Some(')') if unbalanced(url, '(', ')') => end -= 1,
                Some(']') if unbalanced(url, '[', ']') => end -= 1,
                _ => break,
            }
        }
        if at[i..end].contains(&pos) || (at[i] <= pos && pos <= at[end - 1]) {
            let mut segments: Vec<Segment> = Vec::new();
            for &(l, c) in &at[i..end] {
                match segments.last_mut() {
                    Some(seg) if seg.0 == l => seg.2 = c,
                    _ => segments.push((l, c, c)),
                }
            }
            return Some((text[i..end].iter().collect(), segments));
        }
        i = end.max(i + 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_proto::data::{Cell, CellFlags, Cursor, Frame, Modes, Row, Snapshot, StyleEntry};

    fn screen(lines: &[&str], wrapped_first: bool) -> Replica {
        let rows: Vec<Row> = lines
            .iter()
            .enumerate()
            .map(|(i, l)| Row {
                index: i as i32,
                wrapped: i == 0 && wrapped_first,
                cells: l
                    .chars()
                    .map(|c| Cell {
                        codepoint: c as u32,
                        style: 0,
                        flags: CellFlags::empty(),
                        extra: Vec::new(),
                    })
                    .collect(),
            })
            .collect();
        let mut r = Replica::new();
        r.apply(&Frame::Snapshot(Snapshot {
            seq: 1,
            cols: 80,
            rows: lines.len() as u16,
            cursor: Cursor::default(),
            modes: Modes::empty(),
            scrollback_rows: 0,
            scrollback_base: 0,
            styles: Vec::<StyleEntry>::new(),
            lines: rows,
        }))
        .unwrap();
        r
    }

    #[test]
    fn copy_joins_rows_with_newlines_except_after_a_soft_wrap_and_trims_trailing_blanks() {
        let r = screen(&["hello wor", "ld   ", "next  "], true);
        let all = Selection {
            anchor: (0, 0),
            head: (2, 19),
        };
        assert_eq!(copy_text(&r, &all), "hello world\nnext");
        let middle = Selection {
            anchor: (0, 6),
            head: (0, 8),
        };
        assert_eq!(copy_text(&r, &middle), "wor");
    }

    #[test]
    fn a_double_click_takes_the_word_with_path_characters() {
        let r = screen(&["see ~/src/app.rs now"], false);
        assert_eq!(
            word_at(&r, (0, 6)),
            Selection {
                anchor: (0, 4),
                head: (0, 15)
            }
        );
        assert_eq!(word_at(&r, (0, 3)), Selection::at((0, 3)));
    }

    #[test]
    fn find_is_case_insensitive_and_oldest_first() {
        let r = screen(&["Error one", "no", "an error"], false);
        let found = find(&r, "error", 0, 3);
        assert_eq!(
            found,
            vec![
                Selection {
                    anchor: (0, 0),
                    head: (0, 4)
                },
                Selection {
                    anchor: (2, 3),
                    head: (2, 7)
                },
            ]
        );
    }

    #[test]
    fn a_link_is_found_under_the_pointer_without_its_trailing_punctuation() {
        let r = screen(&["see (https://ex.com/a_(b)). ok"], false);
        let (url, segs) = link_at(&r, (0, 10)).unwrap();
        assert_eq!(url, "https://ex.com/a_(b)");
        assert_eq!(segs, vec![(0, 5, 24)]);
        assert!(link_at(&r, (0, 1)).is_none());
    }

    #[test]
    fn a_soft_wrapped_link_opens_whole() {
        let r = screen(&["go https://exa", "mple.com/x now"], true);
        assert_eq!(link_at(&r, (1, 2)).unwrap().0, "https://example.com/x");
    }
}
