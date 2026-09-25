import { describe, expect, test } from 'bun:test';
import { terminalTheme } from '../theme/tokens';
import { Attrs, CellFlags, type Color, type Row, type Style } from './frames';
import {
  blend,
  indexedColour,
  isNarrowGlyph,
  type Run,
  rowRuns,
  rowText,
  StyleResolver,
} from './runs';

type Cell = [text: string, style?: number, flags?: number, extra?: number[]];

function row(cells: Cell[], wrapped = false): Row {
  const graphemes = new Map<number, readonly number[]>();
  cells.forEach(([, , , extra], i) => {
    if (extra) graphemes.set(i, extra);
  });
  return {
    index: 0,
    wrapped,
    codepoints: Uint32Array.from(cells.map(([t]) => (t === '' ? 0 : (t.codePointAt(0) ?? 0)))),
    styles: Uint16Array.from(cells.map(([, s]) => s ?? 0)),
    flags: Uint8Array.from(cells.map(([, , f]) => f ?? 0)),
    graphemes: graphemes.size > 0 ? graphemes : null,
  };
}

function text(s: string, style = 0): Cell[] {
  return [...s].map((c) => [c, style]);
}

const plain: Color = { kind: 'default' };
const style = (patch: Partial<Style>): Style => ({
  fg: plain,
  bg: plain,
  underlineColor: plain,
  attrs: 0,
  ...patch,
});

const STYLES: Record<number, Style> = {
  0: style({}),
  1: style({ fg: { kind: 'indexed', index: 1 } }),
  2: style({ fg: { kind: 'indexed', index: 1 }, attrs: Attrs.italic | Attrs.blink }),
  3: style({ bg: { kind: 'rgb', r: 1, g: 2, b: 3 } }),
  4: style({ attrs: Attrs.inverse }),
  5: style({ fg: { kind: 'indexed', index: 2 }, attrs: Attrs.faint }),
  6: style({ attrs: Attrs.invisible, bg: { kind: 'indexed', index: 4 } }),
  7: style({ attrs: Attrs.bold | Attrs.strikethrough | (1 << 3) }),
  8: style({ attrs: Attrs.strikethrough }),
  9: style({ fg: { kind: 'indexed', index: 196 }, bg: { kind: 'indexed', index: 244 } }),
};
const styleOf = (id: number) => STYLES[id] ?? style({});
const t = terminalTheme;
const hex = (...c: number[]) => `#${c.map((v) => v.toString(16).padStart(2, '0')).join('')}`;

function runs(r: Row, cols = 40, selection: readonly [number, number] | null = null) {
  return rowRuns(r, cols, styleOf, new StyleResolver(t), selection);
}

function shape(list: Run[]): [string, number, number][] {
  return list.map((r) => [r.text, r.col, r.cells]);
}

describe('rowRuns', () => {
  test('merges cells of one resolved style and drops trailing default blanks', () => {
    const r = row([...text('ab'), ...text('cd', 1), ...text('  ', 0), ['', 0], ...text('   ')]);
    expect(shape(runs(r))).toEqual([
      ['ab', 0, 2],
      ['cd', 2, 2],
    ]);
    expect(runs(row([...text('   '), ['']])).length).toBe(0);
  });

  test('keeps blanks inside a row and blanks that paint a background', () => {
    const r = row([...text('a'), ...text('   '), ...text('b'), ...text('  ', 3)]);
    expect(shape(runs(r))).toEqual([
      ['a   b', 0, 5],
      ['  ', 5, 2],
    ]);
  });

  test('styles that look the same merge even with different ids (italic and blink are not drawn)', () => {
    expect(shape(runs(row([...text('x', 1), ...text('y', 2)])))).toEqual([['xy', 0, 2]]);
  });

  test('wide characters, grapheme clusters and glyphs the font may not draw at one cell stand alone', () => {
    const r = row([
      ...text('a'),
      ['中', 0, CellFlags.wide],
      ['', 0, CellFlags.spacer],
      ['👨', 0, CellFlags.wide | CellFlags.grapheme, [0x200d, 0x1f469]],
      ['', 0, CellFlags.spacer],
      ['e', 0, CellFlags.grapheme, [0x301]],
      ...text('⏺b'),
      ['', 0, CellFlags.spacerHead],
    ]);
    expect(shape(runs(r))).toEqual([
      ['a', 0, 1],
      ['中', 1, 2],
      ['👨‍👩', 3, 2],
      ['e\u0301', 5, 1],
      ['⏺', 6, 1],
      ['b', 7, 1],
    ]);
  });

  test('box drawing, blocks and Latin stay in one run; Braille does not', () => {
    expect(shape(runs(row(text('╭──┤ é ▀█')))).length).toBe(1);
    expect(isNarrowGlyph(0x2500)).toBe(true);
    expect(isNarrowGlyph(0x259f)).toBe(true);
    expect(isNarrowGlyph(0x280b)).toBe(false);
    expect(isNarrowGlyph(0x23fa)).toBe(false);
    expect(isNarrowGlyph(0x4e2d)).toBe(false);
  });

  test('resolves inverse, faint, invisible, bold and decorations', () => {
    const [inverse] = runs(row(text('i', 4)));
    expect(inverse?.style).toEqual({
      color: t.bg,
      backgroundColor: t.fg,
      bold: false,
      decoration: null,
    });
    const [faint] = runs(row(text('f', 5)));
    expect(faint?.style.color).toBe(blend(t.ansi[2] as string, 0.5, t.bg));
    const [hidden] = runs(row(text('h', 6)));
    expect(hidden?.style.color).toBe(t.ansi[4] as string);
    const [both] = runs(row(text('u', 7)));
    expect(both?.style).toMatchObject({ bold: true, decoration: 'underline' });
    const [strike] = runs(row(text('s', 8)));
    expect(strike?.style.decoration).toBe('line-through');
  });

  test('indexed colours follow the xterm table', () => {
    const [r] = runs(row(text('x', 9)));
    expect(r?.style.color).toBe(hex(255, 0, 0));
    expect(r?.style.backgroundColor).toBe(hex(128, 128, 128));
    expect(indexedColour(t, 16)).toBe(hex(0, 0, 0));
    expect(indexedColour(t, 231)).toBe(hex(255, 255, 255));
    expect(indexedColour(t, 255)).toBe(hex(238, 238, 238));
    expect(indexedColour(t, 3)).toBe(t.ansi[3] as string);
  });

  test('the selection splits runs and paints the selected blanks at the row end', () => {
    const r = row(text('hello world'));
    const list = runs(r, 20, [3, 15]);
    expect(shape(list)).toEqual([
      ['hel', 0, 3],
      ['lo world    ', 3, 12],
    ]);
    expect(list[1]?.style.backgroundColor).toBe(t.selectionBg);
    expect(list[1]?.style.color).toBe(t.selectionFg);
    expect(shape(runs(row([]), 10, [0, 10]))).toEqual([['          ', 0, 10]]);
  });

  test('never draws past the grid', () => {
    expect(shape(runs(row(text('abcdef')), 3))).toEqual([['abc', 0, 3]]);
  });
});

describe('rowText', () => {
  test('skips spacers, joins grapheme clusters and trims trailing blanks', () => {
    const r = row([
      ['中', 0, CellFlags.wide],
      ['', 0, CellFlags.spacer],
      ['e', 0, CellFlags.grapheme, [0x301]],
      ['', 0],
      ...text('x  '),
    ]);
    expect(rowText(r)).toBe('中e\u0301 x');
    expect(rowText(r, 2)).toBe('e\u0301 x');
    expect(rowText(r, 0, 3, false)).toBe('中e\u0301');
  });
});
