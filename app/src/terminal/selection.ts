import { CellFlags, type Row } from './frames';
import type { Replica } from './replica';
import { type RowSelection, rowText } from './runs';

/** A cell by absolute line number (the scrollback starts at `Replica.scrollbackBase`) and column. */
export interface CellPoint {
  line: number;
  col: number;
}

/** How a drag extends: by cell, by word (double click) or by whole line (triple click). */
export type SelectionUnit = 'cell' | 'word' | 'line';

/** ply's own cell-range selection; `anchor` stays where the drag began, `head` follows the pointer. */
export interface Selection {
  anchor: CellPoint;
  head: CellPoint;
  unit: SelectionUnit;
}

function before(a: CellPoint, b: CellPoint): boolean {
  return a.line < b.line || (a.line === b.line && a.col < b.col);
}

const WORD = /[\p{L}\p{N}_\-./~:@%+#]/u;

function isWordCell(row: Row | undefined, col: number): boolean {
  if (!row || col < 0 || col >= row.codepoints.length) return false;
  const cp = row.codepoints[col] as number;
  if ((row.flags[col] as number) & CellFlags.spacer) return isWordCell(row, col - 1);
  return cp !== 0 && WORD.test(String.fromCodePoint(cp));
}

/** The selection as an inclusive start and exclusive end, ordered, with word or line units expanded. */
export function selectionBounds(
  sel: Selection,
  replica: Replica,
): { start: CellPoint; end: CellPoint } {
  const [a, b] = before(sel.head, sel.anchor) ? [sel.head, sel.anchor] : [sel.anchor, sel.head];
  if (sel.unit === 'line') {
    return { start: { line: a.line, col: 0 }, end: { line: b.line, col: replica.cols } };
  }
  if (sel.unit === 'word') {
    const first = replica.line(a.line)?.row;
    const last = replica.line(b.line)?.row;
    let s = a.col;
    while (isWordCell(first, s - 1) && isWordCell(first, s)) s--;
    let e = b.col + 1;
    while (isWordCell(last, e - 1) && isWordCell(last, e)) e++;
    return { start: { line: a.line, col: s }, end: { line: b.line, col: e } };
  }
  return { start: a, end: { line: b.line, col: b.col + 1 } };
}

/** Columns of absolute line `line` that the selection covers, for drawing; `null` when none. */
export function selectionOnLine(
  bounds: { start: CellPoint; end: CellPoint } | null,
  line: number,
  cols: number,
): RowSelection {
  if (!bounds || line < bounds.start.line || line > bounds.end.line) return null;
  const from = line === bounds.start.line ? bounds.start.col : 0;
  const to = line === bounds.end.line ? bounds.end.col : cols;
  return to > from ? [from, to] : null;
}

/** The selected text: rows joined with newlines except after a soft-wrapped row, each line's trailing blanks trimmed; unfetched rows read as empty. */
export function selectedText(sel: Selection, replica: Replica): string {
  const { start, end } = selectionBounds(sel, replica);
  const out: string[] = [];
  for (let line = start.line; line <= end.line; line++) {
    const row = replica.line(line)?.row;
    const from = line === start.line ? start.col : 0;
    const to = line === end.line ? end.col : Number.POSITIVE_INFINITY;
    const wrapped = row?.wrapped === true && line < end.line;
    out.push(row ? rowText(row, from, to, !wrapped) : '');
    if (line < end.line && !wrapped) out.push('\n');
  }
  return out.join('');
}
