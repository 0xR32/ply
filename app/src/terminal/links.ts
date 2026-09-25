import { CellFlags } from './frames';
import type { Replica } from './replica';
import type { CellPoint } from './selection';

/** One row's part of a link: absolute line, columns `[from, to)`. */
export interface LinkSegment {
  line: number;
  from: number;
  to: number;
}

/** An http(s) URL found in the pane's text, with the cells it covers (several rows when soft-wrapped). */
export interface TerminalLink {
  url: string;
  segments: LinkSegment[];
}

const URL_RE = /https?:\/\/[A-Za-z0-9\-._~:/?#[\]@!$&'()*+,;=%]+/g;
const TRAILING = /[.,:;!?'*]$/;
const MAX_WRAPPED_ROWS = 64;

function unbalanced(url: string, open: string, close: string): boolean {
  let depth = 0;
  for (const ch of url) {
    if (ch === open) depth++;
    else if (ch === close) depth--;
  }
  return depth < 0;
}

/** The URL with the punctuation that ends a sentence or closes a bracket around it removed. */
export function trimUrl(raw: string): string {
  let url = raw;
  for (;;) {
    if (TRAILING.test(url)) url = url.slice(0, -1);
    else if (url.endsWith(')') && unbalanced(url, '(', ')')) url = url.slice(0, -1);
    else if (url.endsWith(']') && unbalanced(url, '[', ']')) url = url.slice(0, -1);
    else return url;
  }
}

/** The text of the soft-wrapped line holding `line`, with the cell each UTF-16 unit came from. */
function logicalLine(replica: Replica, line: number): { text: string; cells: CellPoint[] } {
  let first = line;
  while (
    line - first < MAX_WRAPPED_ROWS &&
    first > replica.scrollbackBase &&
    replica.line(first - 1)?.row.wrapped
  ) {
    first--;
  }
  const parts: string[] = [];
  const cells: CellPoint[] = [];
  const push = (s: string, at: CellPoint) => {
    parts.push(s);
    for (let i = 0; i < s.length; i++) cells.push(at);
  };
  for (let l = first; l - first < 2 * MAX_WRAPPED_ROWS; l++) {
    const row = replica.line(l)?.row;
    if (!row) break;
    const n = row.codepoints.length;
    for (let c = 0; c < n; c++) {
      if (((row.flags[c] as number) & (CellFlags.spacer | CellFlags.spacerHead)) !== 0) continue;
      const cp = row.codepoints[c] as number;
      const extra = row.graphemes?.get(c);
      const text = cp === 0 ? ' ' : String.fromCodePoint(cp, ...(extra ?? []));
      push(text, { line: l, col: c });
    }
    if (!row.wrapped) break;
    // Trailing blanks are not sent, but on a wrapped row they still separate words.
    for (let c = n; c < replica.cols; c++) push(' ', { line: l, col: c });
  }
  return { text: parts.join(''), cells };
}

/** The URL under `at`, or `null`; only http and https are links, so a click can never hand another scheme to the system. */
export function linkAt(replica: Replica, at: CellPoint): TerminalLink | null {
  const row = replica.line(at.line)?.row;
  if (!row || at.col >= row.codepoints.length) return null;
  const { text, cells } = logicalLine(replica, at.line);
  const index = cells.findIndex((p) => p.line === at.line && p.col === at.col);
  if (index < 0) return null;
  for (const m of text.matchAll(URL_RE)) {
    const start = m.index;
    if (start > index) break;
    const url = trimUrl(m[0]);
    if (index >= start + url.length || !/^https?:\/\/[A-Za-z0-9]/.test(url)) continue;
    const segments: LinkSegment[] = [];
    for (let i = start; i < start + url.length; i++) {
      const p = cells[i] as CellPoint;
      const last = segments[segments.length - 1];
      if (last && last.line === p.line) last.to = p.col + 1;
      else segments.push({ line: p.line, from: p.col, to: p.col + 1 });
    }
    return { url, segments };
  }
  return null;
}
