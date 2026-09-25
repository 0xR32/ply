import { describe, expect, test } from 'bun:test';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  Attrs,
  CellFlags,
  type Color,
  decodeFrame,
  encodeFrame,
  type Frame,
  FrameError,
  FrameReader,
  HEADER_LEN,
  Kind,
  MAX_FRAME_LEN,
  Modes,
  Mods,
  type Row,
  type StyleEntry,
} from './frames';

const GOLDEN = join(import.meta.dir, '..', '..', '..', 'crates', 'proto', 'tests', 'golden', 'c2');
const MAX_SAFE = Number.MAX_SAFE_INTEGER;

type CellSpec = [codepoint: number, style: number, flags: number, extra?: number[]];

function row(index: number, wrapped: boolean, cells: CellSpec[]): Row {
  const graphemes = new Map<number, readonly number[]>();
  cells.forEach(([, , , extra], i) => {
    if (extra) graphemes.set(i, extra);
  });
  return {
    index,
    wrapped,
    codepoints: Uint32Array.from(cells.map((c) => c[0])),
    styles: Uint16Array.from(cells.map((c) => c[1])),
    flags: Uint8Array.from(cells.map((c) => c[2])),
    graphemes: graphemes.size > 0 ? graphemes : null,
  };
}

const rgbOf = (r: number, g: number, b: number): Color => ({ kind: 'rgb', r, g, b });
const indexed = (index: number): Color => ({ kind: 'indexed', index });
const plain: Color = { kind: 'default' };

// Mirrors `goldens()` in crates/proto/tests/golden_c2.rs value for value.
const styles: StyleEntry[] = [
  {
    id: 1,
    style: {
      fg: indexed(1),
      bg: indexed(200),
      underlineColor: rgbOf(1, 2, 3),
      attrs: Attrs.bold | Attrs.italic | Attrs.strikethrough | Attrs.overline | (3 << 3),
    },
  },
  {
    id: 2,
    style: {
      fg: rgbOf(0xe6, 0xe8, 0xef),
      bg: plain,
      underlineColor: plain,
      attrs: Attrs.faint | Attrs.blink | Attrs.inverse | Attrs.invisible | (5 << 3),
    },
  },
  { id: 65535, style: { fg: plain, bg: plain, underlineColor: plain, attrs: 0 } },
];

const rows = (first: number): Row[] => [
  row(first, true, [
    [0x41, 1, 0],
    [0x4e2d, 0, CellFlags.wide],
    [0, 0, CellFlags.spacer],
    [0x1f468, 2, CellFlags.wide | CellFlags.grapheme, [0x200d, 0x1f469, 0x200d, 0x1f467]],
    [0, 0, CellFlags.spacer],
    [0x65, 65535, CellFlags.grapheme, [0x0301]],
    [0x10ffff, 0, 0],
    [0, 0, CellFlags.spacerHead],
  ]),
  row(first + 1, false, []),
];

const expected: Record<string, Frame> = {
  attach: {
    kind: 'attach',
    v: 2,
    paneId: MAX_SAFE,
    cols: 168,
    rows: 50,
    cellWidthPx: 8,
    cellHeightPx: 19,
  },
  'input-raw': { kind: 'inputRaw', bytes: Uint8Array.from([0x31, 0x00, 0x1b, 0xff]) },
  resize: { kind: 'resize', cols: 65535, rows: 1, cellWidthPx: 7, cellHeightPx: 16 },
  'fetch-history': { kind: 'fetchHistory', start: MAX_SAFE, count: 1000 },
  ack: { kind: 'ack', seq: MAX_SAFE },
  key: {
    kind: 'key',
    key: 175,
    mods: Mods.alt | Mods.altSide | Mods.shift | Mods.superSide,
    consumedMods: Mods.alt,
    action: 'repeat',
    composing: false,
    unshiftedCodepoint: 0x78,
    text: '≈',
  },
  'key-composing': {
    kind: 'key',
    key: 0,
    mods: 0,
    consumedMods: 0,
    action: 'release',
    composing: true,
    unshiftedCodepoint: 0x10ffff,
    text: '',
  },
  mouse: {
    kind: 'mouse',
    action: 'motion',
    button: 11,
    mods: Mods.ctrl | Mods.ctrlSide | Mods.capsLock | Mods.numLock,
    col: 5,
    row: 3,
    x: 47.5,
    y: -0.25,
  },
  paste: { kind: 'paste', allowUnsafe: true, text: 'a\nb\u001b[201~ü' },
  focus: { kind: 'focus', focused: true },
  snapshot: {
    kind: 'snapshot',
    seq: 1,
    cols: 168,
    rows: 2,
    cursor: { col: 7, row: 1, shape: 'bar', visible: true, blinking: true },
    modes: Modes.altScreen | Modes.cursorVisible | Modes.mouseReporting | Modes.bracketedPaste,
    scrollbackRows: 10_300,
    scrollbackBase: 123_456_789,
    styles,
    lines: rows(0),
  },
  delta: {
    kind: 'delta',
    seq: MAX_SAFE,
    cursor: { col: 0, row: 0, shape: 'blockHollow', visible: false, blinking: false },
    modes: Modes.cursorVisible,
    scrollbackRows: 0xffffffff,
    scrollbackBase: MAX_SAFE,
    stylesAdded: [
      {
        id: 3,
        style: {
          fg: plain,
          bg: rgbOf(0x0c, 0x0e, 0x14),
          underlineColor: indexed(255),
          attrs: 2 << 3,
        },
      },
    ],
    lines: rows(1),
  },
  history: { kind: 'history', start: 123_466_787, stylesAdded: styles, lines: rows(0) },
  title: { kind: 'title', title: 'claude — example ✳' },
  bell: { kind: 'bell' },
  exit: { kind: 'exit', code: -129 },
  'paste-rejected': { kind: 'pasteRejected' },
  'attach-refused': { kind: 'attachRefused', reason: 'unknownPane', message: 'no pane 9' },
  'clipboard-write': { kind: 'clipboardWrite', text: 'git log --oneline\n✓ copied' },
};

function golden(name: string): Uint8Array {
  return new Uint8Array(readFileSync(join(GOLDEN, `${name}.bin`)));
}

function payloadOf(wire: Uint8Array): { kind: number; payload: Uint8Array } {
  return { kind: wire[4] as number, payload: wire.subarray(HEADER_LEN) };
}

describe('C2 golden frames (written by ply-proto)', () => {
  test('there is one golden per expectation and every kind is covered', () => {
    const onDisk = readdirSync(GOLDEN)
      .filter((f) => f.endsWith('.bin'))
      .map((f) => f.slice(0, -4))
      .sort();
    expect(onDisk).toEqual(Object.keys(expected).sort());
    const kinds = new Set(Object.values(expected).map((f) => Kind[f.kind]));
    expect(kinds.size).toBe(Object.keys(Kind).length);
  });

  for (const [name, frame] of Object.entries(expected)) {
    test(`${name}: decodes to the Rust value and re-encodes byte for byte`, () => {
      const wire = golden(name);
      const { kind, payload } = payloadOf(wire);
      expect(wire.length - HEADER_LEN).toBe(
        new DataView(wire.buffer, wire.byteOffset).getUint32(0, true),
      );
      expect(kind).toBe(Kind[frame.kind]);
      expect(decodeFrame(kind, payload)).toEqual(frame);
      expect(encodeFrame(frame)).toEqual(wire);
      expect(encodeFrame(decodeFrame(kind, payload))).toEqual(wire);
    });
  }
});

function rejects(kind: number, payload: number[] | Uint8Array, reason: FrameError['reason']) {
  try {
    decodeFrame(kind, payload instanceof Uint8Array ? payload : Uint8Array.from(payload));
  } catch (error) {
    expect(error).toBeInstanceOf(FrameError);
    expect((error as FrameError).reason).toBe(reason);
    return;
  }
  throw new Error(`0x${kind.toString(16)} ${JSON.stringify(payload)} was accepted`);
}

describe('C2 decoding is strict (INV-10)', () => {
  test('unknown kinds, truncation and trailing bytes', () => {
    rejects(0x19, [], 'unknownKind');
    rejects(0x29, [], 'unknownKind');
    rejects(Kind.clipboardWrite, [0xc3], 'invalidUtf8');
    rejects(Kind.ack, [1, 0, 0, 0, 0, 0, 0], 'truncated');
    rejects(Kind.ack, [1, 0, 0, 0, 0, 0, 0, 0, 9], 'trailingBytes');
    rejects(Kind.bell, [0], 'trailingBytes');
    const snap = payloadOf(golden('snapshot')).payload;
    rejects(Kind.snapshot, snap.subarray(0, snap.length - 1), 'truncated');
  });

  test('out-of-range values', () => {
    rejects(Kind.focus, [2], 'invalidValue');
    rejects(Kind.fetchHistory, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 'invalidValue');
    rejects(Kind.fetchHistory, [0, 0, 0, 0, 0, 0, 0, 0, 0xe9, 0x03], 'invalidValue');
    rejects(Kind.fetchHistory, [0, 0, 0, 0, 0, 0, 0x20, 0, 1, 0], 'invalidValue');
    const oneRow = (index: number) => [5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, index, 0, 0, 0, 0, 0, 0];
    expect(decodeFrame(Kind.history, Uint8Array.from(oneRow(0))).kind).toBe('history');
    rejects(Kind.history, oneRow(1), 'invalidValue');
    rejects(Kind.ack, [0, 0, 0, 0, 0, 0, 0x20, 0], 'invalidValue');
    rejects(Kind.key, [176, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0], 'invalidValue');
    rejects(Kind.key, [1, 0, 0, 4, 0, 0, 1, 0, 0, 0, 0, 0], 'invalidValue');
    rejects(Kind.key, [1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0], 'invalidValue');
    rejects(Kind.key, [1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0x11, 0], 'invalidValue');
    rejects(Kind.mouse, [0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 'invalidValue');
    rejects(Kind.mouse, [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0x7f, 0, 0, 0, 0], 'invalidValue');
    rejects(Kind.attachRefused, [4], 'invalidValue');
    rejects(Kind.title, [0xff], 'invalidUtf8');
  });

  test('bad cell, style, cursor and mode bits', () => {
    const header = [
      ...[1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
      ...[0, 0, 0, 0, 0, 0, 0, 0],
    ];
    const noStyles = [0, 0];
    const oneRow = (cell: number[]) => [1, 0, 0, 0, 0, 0, 0, 1, 0, ...cell];
    const ok = [...header, ...noStyles, ...oneRow([0x41, 0, 0, 0, 0, 0, 0])];
    expect(decodeFrame(Kind.snapshot, Uint8Array.from(ok)).kind).toBe('snapshot');
    rejects(
      Kind.snapshot,
      [...header, ...noStyles, ...oneRow([0, 0, 0x11, 0, 0, 0, 0])],
      'invalidValue',
    );
    rejects(
      Kind.snapshot,
      [...header, ...noStyles, ...oneRow([0x41, 0, 0, 0, 0, 0, 0x10])],
      'invalidValue',
    );
    rejects(
      Kind.snapshot,
      [...header, ...noStyles, ...oneRow([0x41, 0, 0, 0, 0, 0, 8, 0])],
      'invalidValue',
    );
    const style = (id: number, attrs: number) => [1, 0, id, 0, ...new Array(12).fill(0), attrs, 0];
    rejects(Kind.snapshot, [...header, ...style(0, 0), 0, 0], 'invalidValue');
    rejects(Kind.snapshot, [...header, ...style(1, 6 << 3), 0, 0], 'invalidValue');
    const badColour = [1, 0, 1, 0, 1, 1, 1, 0, ...new Array(8).fill(0), 0, 0];
    rejects(Kind.snapshot, [...header, ...badColour, 0, 0], 'invalidValue');
    const badShape = [...header];
    badShape[16] = 4;
    rejects(Kind.snapshot, [...badShape, 0, 0, 0, 0], 'invalidValue');
    const badModes = [...header];
    badModes[18] = 0x10;
    rejects(Kind.snapshot, [...badModes, 0, 0, 0, 0], 'invalidValue');
  });

  test('the encoder refuses values that do not fit instead of wrapping them', () => {
    const bad: Frame[] = [
      { kind: 'resize', cols: 65536, rows: 1, cellWidthPx: 1, cellHeightPx: 1 },
      { kind: 'ack', seq: -1 },
      { kind: 'ack', seq: 2 ** 53 },
      { kind: 'fetchHistory', start: -1, count: 1 },
      {
        kind: 'history',
        start: 0,
        stylesAdded: [],
        lines: [
          {
            index: 1,
            wrapped: false,
            codepoints: new Uint32Array(0),
            styles: new Uint16Array(0),
            flags: new Uint8Array(0),
            graphemes: null,
          },
        ],
      },
      { kind: 'exit', code: 2 ** 31 },
      { kind: 'mouse', action: 'press', button: 1, mods: 0, col: 0, row: 0, x: Number.NaN, y: 0 },
    ];
    for (const f of bad) expect(() => encodeFrame(f)).toThrow(FrameError);
    const huge: Frame = { kind: 'paste', allowUnsafe: false, text: 'x'.repeat(MAX_FRAME_LEN) };
    expect(() => encodeFrame(huge)).toThrow(FrameError);
  });
});

describe('FrameReader', () => {
  test('reassembles frames split at every byte boundary and across chunks', () => {
    const names = ['snapshot', 'ack', 'delta', 'bell', 'history', 'title'];
    const wire = Buffer.concat(names.map(golden));
    for (const step of [1, 2, 3, 7, 64, wire.length]) {
      const reader = new FrameReader();
      const out: Frame[] = [];
      for (let at = 0; at < wire.length; at += step) {
        reader.push(wire.subarray(at, at + step));
        for (let f = reader.next(); f; f = reader.next()) out.push(f);
      }
      expect(out.map((f) => f.kind)).toEqual([
        'snapshot',
        'ack',
        'delta',
        'bell',
        'history',
        'title',
      ]);
      expect(reader.buffered).toBe(0);
    }
  });

  test('refuses a header above 1 MiB before the payload arrives', () => {
    const reader = new FrameReader();
    reader.push(Uint8Array.from([0x01, 0x00, 0x10, 0x00, Kind.snapshot]));
    expect(() => reader.next()).toThrow(FrameError);
  });

  test('grows past its initial buffer for a frame close to the cap', () => {
    const text = 'y'.repeat(MAX_FRAME_LEN - 1);
    const wire = encodeFrame({ kind: 'paste', allowUnsafe: false, text });
    const reader = new FrameReader();
    for (let at = 0; at < wire.length; at += 65_536) reader.push(wire.subarray(at, at + 65_536));
    expect(reader.next()).toEqual({ kind: 'paste', allowUnsafe: false, text });
  });
});
