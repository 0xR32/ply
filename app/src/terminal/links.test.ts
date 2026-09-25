import { describe, expect, test } from 'bun:test';
import { CellFlags, type Row } from './frames';
import { linkAt, trimUrl } from './links';
import { Replica } from './replica';

function textRow(index: number, text: string, wrapped = false): Row {
  const cps: number[] = [];
  const flags: number[] = [];
  for (const ch of text) {
    const cp = ch.codePointAt(0) ?? 0;
    const wide = /\p{Script=Han}/u.test(ch);
    cps.push(cp);
    flags.push(wide ? CellFlags.wide : 0);
    if (wide) {
      cps.push(0);
      flags.push(CellFlags.spacer);
    }
  }
  return {
    index,
    wrapped,
    codepoints: Uint32Array.from(cps),
    styles: new Uint16Array(cps.length),
    flags: Uint8Array.from(flags),
    graphemes: null,
  };
}

function replicaOf(cols: number, rows: Row[]): Replica {
  const r = new Replica();
  r.apply({
    kind: 'snapshot',
    seq: 1,
    cols,
    rows: rows.length,
    cursor: { col: 0, row: 0, shape: 'block', visible: false, blinking: false },
    modes: 0,
    scrollbackRows: 0,
    scrollbackBase: 0,
    styles: [],
    lines: rows,
  });
  return r;
}

const one = (text: string, col: number, cols = 80) =>
  linkAt(replicaOf(cols, [textRow(0, text)]), { line: 0, col });

describe('linkAt', () => {
  test('finds the URL under the cell and the columns it covers', () => {
    const line = 'PR: https://github.com/example/ply/pull/12 is up';
    expect(one(line, 10)).toEqual({
      url: 'https://github.com/example/ply/pull/12',
      segments: [{ line: 0, from: 4, to: 42 }],
    });
    expect(one(line, 3)).toBeNull();
    expect(one(line, 43)).toBeNull();
  });

  test('leaves out the punctuation around a URL but keeps balanced brackets', () => {
    expect(one('see https://example.com/a.', 8)?.url).toBe('https://example.com/a');
    expect(one('(https://example.com/a)', 5)?.url).toBe('https://example.com/a');
    expect(one('[docs](https://example.com/b) here', 10)?.url).toBe('https://example.com/b');
    expect(one('https://en.wikipedia.org/wiki/Rust_(language)', 3)?.url).toBe(
      'https://en.wikipedia.org/wiki/Rust_(language)',
    );
    expect(one("'https://example.com/q?a=1&b=2',", 3)?.url).toBe('https://example.com/q?a=1&b=2');
    expect(one('**https://example.com/c**', 4)?.url).toBe('https://example.com/c');
  });

  test('only http and https are links', () => {
    expect(one('file:///etc/hosts', 3)).toBeNull();
    expect(one('vscode://file/a.ts', 3)).toBeNull();
    expect(one('https:// nothing', 2)).toBeNull();
    expect(one('http://example.com', 2)?.url).toBe('http://example.com');
  });

  test('follows a soft wrap onto the next rows, and stops at a hard break', () => {
    const wrapped = replicaOf(20, [
      textRow(0, 'go https://example.c', true),
      textRow(1, 'om/runs/42 now'),
    ]);
    const expected = {
      url: 'https://example.com/runs/42',
      segments: [
        { line: 0, from: 3, to: 20 },
        { line: 1, from: 0, to: 10 },
      ],
    };
    expect(linkAt(wrapped, { line: 0, col: 5 })).toEqual(expected);
    expect(linkAt(wrapped, { line: 1, col: 2 })).toEqual(expected);
    const broken = replicaOf(20, [
      textRow(0, 'go https://example.c'),
      textRow(1, 'om/runs/42 now'),
    ]);
    expect(linkAt(broken, { line: 0, col: 5 })?.url).toBe('https://example.c');
    expect(linkAt(broken, { line: 1, col: 2 })).toBeNull();
  });

  test('a wrapped row whose trailing blanks were not sent still ends the URL', () => {
    const r = replicaOf(20, [textRow(0, 'https://example.com', true), textRow(1, 'next')]);
    expect(linkAt(r, { line: 0, col: 4 })?.url).toBe('https://example.com');
  });

  test('columns stay right after wide characters', () => {
    expect(one('完了 https://example.com/x', 9)).toEqual({
      url: 'https://example.com/x',
      segments: [{ line: 0, from: 5, to: 26 }],
    });
  });

  test('a cell past the row or on an unfetched line is no link', () => {
    const r = replicaOf(40, [textRow(0, 'https://example.com')]);
    expect(linkAt(r, { line: 0, col: 30 })).toBeNull();
    expect(linkAt(r, { line: 5, col: 0 })).toBeNull();
  });
});

describe('trimUrl', () => {
  test('strips trailing sentence punctuation one character at a time', () => {
    expect(trimUrl('https://example.com/a).')).toBe('https://example.com/a');
    expect(trimUrl('https://example.com/a?')).toBe('https://example.com/a');
    expect(trimUrl('https://example.com/#x')).toBe('https://example.com/#x');
  });
});
