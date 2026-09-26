import { describe, expect, test } from 'bun:test';
import { diffKind } from './diff';
import type { Color, Row, Style } from './frames';

const plain: Style = {
  fg: { kind: 'default' },
  bg: { kind: 'default' },
  underlineColor: { kind: 'default' },
  attrs: 0,
};

/** A row of `text` whose cell at `col` has style 1, drawn with `fg` (and `bg`). */
function row(text: string, col: number, fg: Color, bg: Color = { kind: 'default' }) {
  const cps = [...text].map((c) => c.codePointAt(0) ?? 0);
  const styles = new Uint16Array(cps.length);
  if (col >= 0) styles[col] = 1;
  const r: Row = {
    index: 0,
    wrapped: false,
    codepoints: Uint32Array.from(cps),
    styles,
    flags: new Uint8Array(cps.length),
    graphemes: null,
  };
  const marked: Style = { ...plain, fg, bg };
  return { r, styleOf: (id: number) => (id === 1 ? marked : plain) };
}

const green: Color = { kind: 'indexed', index: 2 };
const red: Color = { kind: 'indexed', index: 1 };

describe('diffKind', () => {
  test('a numbered line with a green + or a red - is an added or a removed line', () => {
    const add = row('  248 +    render(<AppShell />);', 6, green);
    expect(diffKind(add.r, add.styleOf)).toBe('add');
    const remove = row('   12 -  const old = 1;', 6, red);
    expect(diffKind(remove.r, remove.styleOf)).toBe('remove');
    const tight = row('      22 +changed line 22', 9, { kind: 'indexed', index: 10 });
    expect(diffKind(tight.r, tight.styleOf), "Claude Code's marker against the line").toBe('add');
    const tightRemove = row('       8 -line 8 of the example file', 9, {
      kind: 'indexed',
      index: 9,
    });
    expect(diffKind(tightRemove.r, tightRemove.styleOf)).toBe('remove');
    const bright = row('+ new line', 0, { kind: 'indexed', index: 10 });
    expect(diffKind(bright.r, bright.styleOf)).toBe('add');
    const rgb = row('  3 - gone', 4, { kind: 'rgb', r: 230, g: 90, b: 80 });
    expect(diffKind(rgb.r, rgb.styleOf)).toBe('remove');
  });

  test('a marker in any other colour, with a background of its own, or where no marker belongs is plain', () => {
    for (const [what, r] of [
      ['a list item', row('- item', 0, { kind: 'default' })],
      ['arithmetic', row('  12 - 4 = 8', 5, { kind: 'default' })],
      ['a green minus', row('  12 - x', 5, green)],
      ['a painted row', row('  12 + x', 5, green, { kind: 'indexed', index: 22 })],
      ['a plain +1', row('  +1 to that', 2, { kind: 'default' })],
      ['blank', row('', -1, green)],
    ] as const) {
      expect(diffKind(r.r, r.styleOf), what).toBeNull();
    }
  });
});
