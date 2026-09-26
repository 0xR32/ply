import type { TerminalTheme } from '../ipc/proto.gen';
import { Attrs, CellFlags, type Color, type Row, type Style, underlineKind } from './frames';

/** The look of one `<text>` run: GPUIX styles only what is here (it has no italic, overline or underline colour). */
export interface RunStyle {
  color: string;
  /** `null` leaves the terminal background showing (the default background is never painted). */
  backgroundColor: string | null;
  bold: boolean;
  decoration: 'underline' | 'line-through' | null;
}

/** One `<text>` of a row: `cells` columns wide from column `col`, in one resolved style. */
export interface Run {
  text: string;
  col: number;
  cells: number;
  style: RunStyle;
}

const CUBE = [0, 95, 135, 175, 215, 255];

function hex2(v: number): string {
  return v.toString(16).padStart(2, '0');
}

function hexOf(r: number, g: number, b: number): string {
  return `#${hex2(r)}${hex2(g)}${hex2(b)}`;
}

function channels(hex: string): [number, number, number] {
  const n = Number.parseInt(hex.slice(1, 7), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** Colour `index` of the xterm 256-colour table: 0–15 from the theme, 16–231 the cube, 232–255 the grey ramp. */
export function indexedColour(theme: TerminalTheme, index: number): string {
  if (index < 16) return theme.ansi[index] ?? theme.fg;
  if (index < 232) {
    const i = index - 16;
    return hexOf(CUBE[Math.floor(i / 36)] ?? 0, CUBE[Math.floor(i / 6) % 6] ?? 0, CUBE[i % 6] ?? 0);
  }
  const v = 8 + 10 * (index - 232);
  return hexOf(v, v, v);
}

/** `top` at `alpha` over `bottom`, both `#RRGGBB`. */
export function blend(top: string, alpha: number, bottom: string): string {
  const t = channels(top);
  const b = channels(bottom);
  return hexOf(
    Math.round(t[0] * alpha + b[0] * (1 - alpha)),
    Math.round(t[1] * alpha + b[1] * (1 - alpha)),
    Math.round(t[2] * alpha + b[2] * (1 - alpha)),
  );
}

function colour(theme: TerminalTheme, c: Color): string | null {
  switch (c.kind) {
    case 'default':
      return null;
    case 'indexed':
      return indexedColour(theme, c.index);
    case 'rgb':
      return hexOf(c.r, c.g, c.b);
  }
}

/** Resolves symbolic C2 styles against one theme; equal-looking styles share one `RunStyle`, so runs compare by identity. */
export class StyleResolver {
  private readonly plain = new WeakMap<Style, RunStyle>();
  private readonly selected = new WeakMap<Style, RunStyle>();
  private readonly interned = new Map<string, RunStyle>();

  constructor(readonly theme: TerminalTheme) {}

  /** The run style of `style`, with the theme's selection colours when `inSelection`. */
  resolve(style: Style, inSelection: boolean): RunStyle {
    const cache = inSelection ? this.selected : this.plain;
    const hit = cache.get(style);
    if (hit) return hit;
    const made = this.intern(this.compute(style, inSelection));
    cache.set(style, made);
    return made;
  }

  private intern(s: RunStyle): RunStyle {
    const key = `${s.color}|${s.backgroundColor}|${s.bold}|${s.decoration}`;
    const known = this.interned.get(key);
    if (known) return known;
    this.interned.set(key, s);
    return s;
  }

  private compute(style: Style, inSelection: boolean): RunStyle {
    const t = this.theme;
    const a = style.attrs;
    let fg = colour(t, style.fg) ?? t.fg;
    let bg = colour(t, style.bg);
    if ((a & Attrs.inverse) !== 0) {
      const swapped = bg ?? t.bg;
      bg = fg;
      fg = swapped;
    }
    if ((a & Attrs.faint) !== 0) fg = blend(fg, 0.5, bg ?? t.bg);
    if (inSelection) {
      fg = t.selectionFg;
      bg = t.selectionBg;
    }
    if ((a & Attrs.invisible) !== 0) fg = bg ?? t.bg;
    const decoration =
      underlineKind(a) !== 0
        ? 'underline'
        : (a & Attrs.strikethrough) !== 0
          ? 'line-through'
          : null;
    return { color: fg, backgroundColor: bg, bold: (a & Attrs.bold) !== 0, decoration };
  }
}

// Codepoints Geist Mono draws at exactly one cell (advance 600/1000 em), from app/assets/fonts/GeistMono-Regular.ttf.
const NARROW = Uint32Array.from([
  0x20, 0x7e, 0xa0, 0xac, 0xae, 0x113, 0x116, 0x12b, 0x12e, 0x131, 0x134, 0x137, 0x139, 0x13e,
  0x141, 0x148, 0x14a, 0x14d, 0x150, 0x17e, 0x18f, 0x18f, 0x192, 0x192, 0x1a0, 0x1a1, 0x1af, 0x1b0,
  0x1cd, 0x1ce, 0x1e4, 0x1e9, 0x218, 0x21b, 0x237, 0x237, 0x259, 0x259, 0x2b9, 0x2b9, 0x2bc, 0x2bc,
  0x2c6, 0x2c8, 0x2d8, 0x2dd, 0x39b, 0x39b, 0x3a9, 0x3a9, 0x3bb, 0x3bc, 0x3c0, 0x3c0, 0x400, 0x45f,
  0x462, 0x463, 0x46a, 0x46b, 0x472, 0x475, 0x490, 0x493, 0x496, 0x497, 0x49a, 0x49b, 0x4a2, 0x4a3,
  0x4ae, 0x4b3, 0x4b6, 0x4b7, 0x4ba, 0x4bb, 0x4c0, 0x4c0, 0x4cf, 0x4cf, 0x4d8, 0x4d9, 0x4e2, 0x4e3,
  0x4e8, 0x4e9, 0x4ee, 0x4ef, 0xe3f, 0xe3f, 0x1e20, 0x1e21, 0x1e80, 0x1e85, 0x1e9e, 0x1e9e, 0x1ea0,
  0x1ef9, 0x2013, 0x2014, 0x2018, 0x201a, 0x201c, 0x201e, 0x2020, 0x2022, 0x2026, 0x2026, 0x2028,
  0x2029, 0x2030, 0x2030, 0x2032, 0x2033, 0x2039, 0x203a, 0x2044, 0x2044, 0x2070, 0x2070, 0x2074,
  0x2079, 0x2080, 0x2089, 0x20aa, 0x20aa, 0x20ac, 0x20ac, 0x20b1, 0x20b1, 0x20b4, 0x20b4, 0x20b9,
  0x20b9, 0x20bd, 0x20bd, 0x2107, 0x2107, 0x2116, 0x2117, 0x2122, 0x2122, 0x2153, 0x2155, 0x215b,
  0x215e, 0x2190, 0x2199, 0x219d, 0x219d, 0x21a9, 0x21aa, 0x21b0, 0x21b1, 0x21b3, 0x21b5, 0x21e4,
  0x21e5, 0x21e7, 0x21e7, 0x2202, 0x2202, 0x2206, 0x2206, 0x220f, 0x220f, 0x2211, 0x2212, 0x221a,
  0x221a, 0x221e, 0x221e, 0x222b, 0x222b, 0x2236, 0x2236, 0x2248, 0x2248, 0x2260, 0x2260, 0x2264,
  0x2265, 0x2326, 0x2327, 0x232b, 0x232b, 0x23ce, 0x23ce, 0x240b, 0x240c, 0x2423, 0x2423, 0x2460,
  0x2468, 0x24ea, 0x24ea, 0x24ff, 0x259f, 0x25b2, 0x25b3, 0x25b6, 0x25b7, 0x25bc, 0x25bd, 0x25c0,
  0x25c1, 0x25ca, 0x25cc, 0x25cf, 0x25cf, 0x2776, 0x277e, 0x3003, 0x3003, 0x301c, 0x301c, 0xa78b,
  0xa78c, 0xf8ff, 0xf8ff,
]);

/** Whether the terminal font draws `cp` at one cell, so it may share a run; any other glyph gets its own fixed-width run. */
export function isNarrowGlyph(cp: number): boolean {
  if (cp >= 0x20 && cp <= 0x7e) return true;
  let lo = 0;
  let hi = NARROW.length / 2 - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const start = NARROW[mid * 2] as number;
    const end = NARROW[mid * 2 + 1] as number;
    if (cp < start) hi = mid - 1;
    else if (cp > end) lo = mid + 1;
    else return true;
  }
  return false;
}

/** Columns `[from, to)` of a row inside the selection, or `null`. */
export type RowSelection = readonly [from: number, to: number] | null;

function cellText(row: Row, c: number, cp: number): string {
  if (cp === 0) return ' ';
  const base = String.fromCodePoint(cp);
  const extra = row.graphemes?.get(c);
  return extra ? base + String.fromCodePoint(...extra) : base;
}

function finish(runs: Run[], open: Run | null, parts: string[]): void {
  if (!open) return;
  open.text = parts.join('');
  const invisible =
    open.style.backgroundColor === null && open.style.decoration === null && !open.text.trim();
  // Runs sit at absolute columns, so an invisible one holds no place and would only cost a host node every frame.
  if (!invisible) runs.push(open);
}

/**
 * The fewest runs that draw `row`: cells of one resolved style merge, wide characters, grapheme clusters and glyphs the font may not draw at one cell (`isNarrowGlyph`) stand alone, spacers are skipped, and trailing default blanks and any run of blanks that paints nothing (no background, no line) are dropped (selected blanks stay).
 */
export function rowRuns(
  row: Row,
  cols: number,
  styleOf: (id: number) => Style,
  resolver: StyleResolver,
  selection: RowSelection = null,
): Run[] {
  const n = Math.min(row.codepoints.length, cols);
  const selFrom = selection ? Math.max(0, selection[0]) : 0;
  const selTo = selection ? Math.min(cols, selection[1]) : 0;
  const selected = (c: number) => c >= selFrom && c < selTo;
  const styleAt = (c: number): RunStyle =>
    resolver.resolve(styleOf(row.styles[c] as number), selected(c));

  let end = n;
  while (end > 0) {
    const c = end - 1;
    const f = row.flags[c] as number;
    const cp = row.codepoints[c] as number;
    const blank =
      (cp === 0 || cp === 0x20 || (f & CellFlags.spacerHead) !== 0) &&
      (f & (CellFlags.wide | CellFlags.grapheme)) === 0;
    if (!blank || selected(c)) break;
    const s = styleAt(c);
    if (s.backgroundColor !== null || s.decoration !== null) break;
    end = c;
  }

  const runs: Run[] = [];
  let open: Run | null = null;
  let parts: string[] = [];
  for (let c = 0; c < end; c++) {
    const f = row.flags[c] as number;
    if ((f & CellFlags.spacer) !== 0) continue;
    const head = (f & CellFlags.spacerHead) !== 0;
    const cp = head ? 0 : (row.codepoints[c] as number);
    const style = head ? resolver.resolve(styleOf(0), selected(c)) : styleAt(c);
    const wide = (f & CellFlags.wide) !== 0;
    const alone = wide || (f & CellFlags.grapheme) !== 0 || (cp !== 0 && !isNarrowGlyph(cp));
    const text = cellText(row, c, cp);
    if (!alone && open !== null && open.style === style) {
      open.cells++;
      parts.push(text);
      continue;
    }
    finish(runs, open, parts);
    open = null;
    parts = [];
    if (alone) runs.push({ text, col: c, cells: wide ? 2 : 1, style });
    else {
      open = { text: '', col: c, cells: 1, style };
      parts = [text];
    }
  }
  finish(runs, open, parts);
  if (selection && selTo > end && selTo > selFrom) {
    const from = Math.max(end, selFrom);
    const style = resolver.resolve(styleOf(0), true);
    const last = runs[runs.length - 1];
    if (last && last.style === style && last.col + last.cells === from) {
      last.cells += selTo - from;
      last.text += ' '.repeat(selTo - from);
    } else {
      runs.push({ text: ' '.repeat(selTo - from), col: from, cells: selTo - from, style });
    }
  }
  return runs;
}

/** A row's text for copying: spacers skipped, empty cells as spaces, `[from, to)` columns only, trailing spaces trimmed unless `trim` is false. */
export function rowText(row: Row, from = 0, to = Number.POSITIVE_INFINITY, trim = true): string {
  const parts: string[] = [];
  const end = Math.min(row.codepoints.length, to);
  for (let c = Math.max(0, from); c < end; c++) {
    const f = row.flags[c] as number;
    if ((f & (CellFlags.spacer | CellFlags.spacerHead)) !== 0) continue;
    parts.push(cellText(row, c, row.codepoints[c] as number));
  }
  const text = parts.join('');
  return trim ? text.replace(/ +$/, '') : text;
}
