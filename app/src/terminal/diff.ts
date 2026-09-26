import { CellFlags, type Color, type Row, type Style } from './frames';

/** How a row reads as part of a diff a CLI drew: an added line, a removed one, or neither. */
export type DiffKind = 'add' | 'remove' | null;

/** Cells read from a row's start to find its marker; Claude Code's and Codex's line numbers fit well within. */
const LEAD = 24;

// An optional line number, then the marker, which Claude Code sets right against the line: "  22 +changed", "- old".
const MARKER = /^ *(?:\d+ +)?([+-])/;

function hue(c: Color): 'green' | 'red' | null {
  if (c.kind === 'indexed') {
    if (c.index === 2 || c.index === 10) return 'green';
    if (c.index === 1 || c.index === 9) return 'red';
    return null;
  }
  if (c.kind === 'rgb') {
    if (c.g > c.r + 40 && c.g > c.b) return 'green';
    if (c.r > c.g + 40 && c.r > c.b) return 'red';
  }
  return null;
}

/** Whether `row` is a diff line: a line number or nothing, then a `+` drawn green or a `-` drawn red with no background of its own; the colour check keeps "12 - 4", "- item" and "+1" in prose plain. */
export function diffKind(row: Row, styleOf: (id: number) => Style): DiffKind {
  const n = Math.min(row.codepoints.length, LEAD);
  let text = '';
  for (let c = 0; c < n; c++) {
    if (((row.flags[c] as number) & (CellFlags.wide | CellFlags.grapheme)) !== 0) break;
    const cp = row.codepoints[c] as number;
    text += cp === 0 ? ' ' : String.fromCodePoint(cp);
  }
  const m = MARKER.exec(text);
  if (!m?.[1]) return null;
  const col = m[0].length - 1;
  const style = styleOf(row.styles[col] as number);
  if (style.bg.kind !== 'default') return null;
  const shade = hue(style.fg);
  if (m[1] === '+' && shade === 'green') return 'add';
  if (m[1] === '-' && shade === 'red') return 'remove';
  return null;
}
