import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { terminalTheme } from '../theme/tokens';
import type { DeltaFrame, HistoryFrame, Row, SnapshotFrame, Style } from './frames';
import { Replica, ReplicaError } from './replica';
import { rowRuns, rowText, StyleResolver } from './runs';
import { fixtureServerFrames } from './test-host';

const SCREENS = join(import.meta.dir, '..', '..', '..', 'crates', 'term', 'tests', 'fixtures');

/** Row texts of a ply-term replay `.screen` file (`NN|text` lines), the Engine's own view of the final screen. */
function engineScreen(name: string): string[] {
  const rows: string[] = [];
  for (const line of readFileSync(join(SCREENS, name), 'utf8').split('\n')) {
    const m = /^(\d\d)\|(.*)$/.exec(line);
    if (m) rows[Number(m[1])] = m[2] ?? '';
  }
  return rows;
}

function screenText(r: Replica): string[] {
  return Array.from({ length: r.rows }, (_, y) => rowText(r.screenRow(y)?.row as Row));
}

function row(index: number, text: string, style = 0): Row {
  const cps = [...text].map((c) => c.codePointAt(0) ?? 0);
  return {
    index,
    wrapped: false,
    codepoints: Uint32Array.from(cps),
    styles: Uint16Array.from(cps.map(() => style)),
    flags: new Uint8Array(cps.length),
    graphemes: null,
  };
}

const red: Style = {
  fg: { kind: 'indexed', index: 1 },
  bg: { kind: 'default' },
  underlineColor: { kind: 'default' },
  attrs: 0,
};

function snapshot(seq: number, lines: Row[] = [row(0, 'hi', 1)]): SnapshotFrame {
  return {
    kind: 'snapshot',
    seq,
    cols: 4,
    rows: 2,
    cursor: { col: 1, row: 0, shape: 'block', visible: true, blinking: false },
    modes: 0,
    scrollbackRows: 0,
    styles: [{ id: 1, style: red }],
    lines,
  };
}

function delta(seq: number, patch: Partial<DeltaFrame> = {}): DeltaFrame {
  return {
    kind: 'delta',
    seq,
    cursor: { col: 0, row: 1, shape: 'bar', visible: true, blinking: true },
    modes: 0,
    scrollbackRows: 0,
    stylesAdded: [],
    lines: [],
    ...patch,
  };
}

describe('Replica against recorded agent streams', () => {
  for (const [name, screen] of [
    ['claude-80x24', 'claude-session.screen'],
    ['codex-120x40', 'codex-session.screen'],
  ] as const) {
    test(`${name}: Snapshot + Deltas rebuild the Engine's final screen`, () => {
      const replica = new Replica();
      for (const f of fixtureServerFrames(`${name}.stream.bin`)) {
        if (f.kind === 'snapshot' || f.kind === 'delta') replica.apply(f);
      }
      const truth = new Replica();
      const [t] = fixtureServerFrames(`${name}.truth.bin`);
      if (t?.kind !== 'snapshot') throw new Error('no truth snapshot');
      truth.apply(t);

      const expected = engineScreen(screen);
      expect(screenText(replica)).toEqual(expected);
      expect(replica.cursor).toEqual(truth.cursor);
      expect(replica.modes).toBe(truth.modes);
      expect(replica.scrollbackRows).toBe(truth.scrollbackRows);
    });
  }

  test('streamed and fresh replicas draw the same runs although their style ids differ', () => {
    for (const name of ['claude-80x24', 'codex-120x40']) {
      const streamed = new Replica();
      for (const f of fixtureServerFrames(`${name}.stream.bin`)) {
        if (f.kind === 'snapshot' || f.kind === 'delta') streamed.apply(f);
      }
      const fresh = new Replica();
      const [t] = fixtureServerFrames(`${name}.truth.bin`);
      if (t?.kind === 'snapshot') fresh.apply(t);
      const resolver = new StyleResolver(terminalTheme);
      const runsOf = (r: Replica) =>
        Array.from({ length: r.rows }, (_, y) =>
          rowRuns(r.screenRow(y)?.row as Row, r.cols, (id) => r.style(id) as Style, resolver).map(
            (run) => [run.text, run.col, run.cells, run.style],
          ),
        );
      expect(runsOf(streamed)).toEqual(runsOf(fresh));
    }
  });
});

describe('Replica bookkeeping', () => {
  test('a Delta bumps only the versions of the rows it carries', () => {
    const r = new Replica();
    r.apply(snapshot(1));
    const before = [r.screenRow(0)?.version, r.screenRow(1)?.version];
    const change = r.apply(
      delta(2, { lines: [row(1, 'ok', 2)], stylesAdded: [{ id: 2, style: red }] }),
    );
    expect(change.rows).toEqual([1]);
    expect(r.screenRow(0)?.version).toBe(before[0] as number);
    expect(r.screenRow(1)?.version).not.toBe(before[1] as number);
    expect(screenText(r)).toEqual(['hi', 'ok']);
    expect(r.cursor.shape).toBe('bar');
    expect(r.lastSeq).toBe(2);
  });

  test('bad frames are refused and leave the replica as it was', () => {
    const r = new Replica();
    expect(() => r.apply(delta(1))).toThrow(ReplicaError);
    r.apply(snapshot(3));
    const bad: DeltaFrame[] = [
      delta(3),
      delta(4, { lines: [row(0, 'x', 7)] }),
      delta(4, { lines: [row(2, 'x')] }),
      delta(4, { lines: [row(0, 'wider')] }),
      delta(4, { stylesAdded: [{ id: 1, style: red }] }),
      delta(4, { stylesAdded: [{ id: 0, style: red }] }),
    ];
    for (const d of bad) expect(() => r.apply(d)).toThrow(ReplicaError);
    expect(screenText(r)).toEqual(['hi', '']);
    expect(r.lastSeq).toBe(3);
    expect(() => r.apply(snapshot(2))).toThrow(ReplicaError);
    r.resetSequence();
    r.apply(snapshot(1, [row(1, 'new')]));
    expect(screenText(r)).toEqual(['', 'new']);
  });

  test('history rows keep their absolute line while scrollback grows and are dropped when it shrinks', () => {
    const r = new Replica();
    r.apply({ ...snapshot(1), scrollbackRows: 5 });
    const page: HistoryFrame = {
      kind: 'history',
      start: -2,
      stylesAdded: [{ id: 9, style: red }],
      lines: [row(-2, 'old', 9), row(-1, 'new')],
    };
    r.apply(page);
    expect(rowText(r.line(3)?.row as Row)).toBe('old');
    expect(rowText(r.line(4)?.row as Row)).toBe('new');
    expect(r.missingHistory(0, 5)).toEqual({ start: 0, count: 3 });
    expect(r.missingHistory(3, 5)).toBeNull();
    const grown = r.apply(delta(2, { scrollbackRows: 8 }));
    expect(grown.pushed).toBe(3);
    expect(rowText(r.historyRow(3)?.row as Row)).toBe('old');
    expect(r.screenTop).toBe(8);
    const shrunk = r.apply(delta(3, { scrollbackRows: 2 }));
    expect(shrunk.historyReset).toBe(true);
    expect(r.historyRow(3)).toBeUndefined();
    expect(r.style(9)).toEqual(red);
  });

  test('the recorded shell screen and its history pages', () => {
    const r = new Replica();
    const frames = fixtureServerFrames('shell-80x24.bin');
    for (const f of frames) if (f.kind !== 'title' && f.kind !== 'bell') r.apply(f as never);
    expect(r.scrollbackRows).toBe(299);
    expect(rowText(r.screenRow(r.rows - 1)?.row as Row)).toBe('example %');
    expect(rowText(r.line(0)?.row as Row)).toContain('dir000');
    expect(rowText(r.line(r.screenTop - 1)?.row as Row)).toContain('dir298');
    expect(r.line(100)).toBeUndefined();
  });
});
