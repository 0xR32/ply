import type {
  Cursor,
  DeltaFrame,
  HistoryFrame,
  Row,
  SnapshotFrame,
  Style,
  StyleEntry,
} from './frames';

/** A row the view draws: the cells as sent, a version that changes whenever they are replaced, and a hash of their content. */
export interface ReplicaRow {
  readonly row: Row;
  readonly version: number;
  /** FNV-1a over the cells, the wrap flag and the style epoch: equal hashes almost always mean equal rows, so a scrolled row keeps its identity. */
  readonly hash: number;
}

/** Whether two rows hold the same cells (the check behind a hash match). */
export function sameCells(a: Row, b: Row): boolean {
  if (a === b) return true;
  const n = a.codepoints.length;
  if (n !== b.codepoints.length || a.wrapped !== b.wrapped) return false;
  for (let c = 0; c < n; c++) {
    if (
      a.codepoints[c] !== b.codepoints[c] ||
      a.styles[c] !== b.styles[c] ||
      a.flags[c] !== b.flags[c]
    ) {
      return false;
    }
  }
  if (a.graphemes === null || b.graphemes === null) return a.graphemes === b.graphemes;
  if (a.graphemes.size !== b.graphemes.size) return false;
  for (const [col, extra] of a.graphemes) {
    const other = b.graphemes.get(col);
    if (!other || other.length !== extra.length || other.some((cp, i) => cp !== extra[i]))
      return false;
  }
  return true;
}

function rowHash(row: Row, epoch: number): number {
  let h = (0x811c9dc5 ^ epoch) >>> 0;
  const mix = (v: number) => {
    h = Math.imul(h ^ v, 0x01000193) >>> 0;
  };
  mix(row.wrapped ? 1 : 0);
  const n = row.codepoints.length;
  for (let c = 0; c < n; c++) {
    mix(row.codepoints[c] as number);
    mix(((row.styles[c] as number) << 8) | (row.flags[c] as number));
  }
  if (row.graphemes)
    for (const [col, extra] of row.graphemes) for (const cp of extra) mix(cp ^ col);
  return h;
}

/** What one applied frame changed; `rows` are live-screen indexes, `historyRows` absolute line numbers. */
export interface ReplicaChange {
  kind: 'snapshot' | 'delta' | 'history';
  rows: readonly number[];
  historyRows: readonly number[];
  /** Lines pushed into scrollback by this frame (the screen's top line moved down by this much); 0 when unchanged or reset. */
  pushed: number;
  /** Fetched scrollback was dropped: all of it (a Snapshot) or the lines plyd no longer keeps (a Delta whose base moved past them). */
  historyReset: boolean;
}

/** A frame that contradicts the replica (bad sequence, unknown style, row out of range); resync with a new attach. */
export class ReplicaError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ReplicaError';
  }
}

const DEFAULT_STYLE: Style = {
  fg: { kind: 'default' },
  bg: { kind: 'default' },
  underlineColor: { kind: 'default' },
  attrs: 0,
};

const NO_CURSOR: Cursor = { col: 0, row: 0, shape: 'block', visible: false, blinking: false };

/** A row with no cells, drawn as blank (trailing default blanks are never sent). */
export function blankRow(index: number): Row {
  return {
    index,
    wrapped: false,
    codepoints: new Uint32Array(0),
    styles: new Uint16Array(0),
    flags: new Uint8Array(0),
    graphemes: null,
  };
}

/**
 * The app's copy of one pane's screen (spec R-R2), fed Snapshot, Delta and History frames in order; strict like ply-term's reference `Replica`, it throws `ReplicaError` and stays unchanged on a bad frame.
 */
export class Replica {
  cols = 0;
  rows = 0;
  cursor: Cursor = NO_CURSOR;
  modes = 0;
  scrollbackRows = 0;
  /** Absolute line of the oldest scrollback row plyd keeps (C2 `scrollback_base`). */
  scrollbackBase = 0;
  lastSeq: number | null = null;
  /** Bumped by every Snapshot, which replaces the style table (ids may then mean other styles). */
  styleEpoch = 0;
  private styles = new Map<number, Style>();
  private lines: ReplicaRow[] = [];
  private history = new Map<number, ReplicaRow>();
  private nextVersion = 1;

  /** Forgets the sequence number so the next attachment's Snapshot (whose numbering starts over) is accepted; the screen stays as it was. */
  resetSequence(): void {
    this.lastSeq = null;
  }

  /** Applies one screen frame; throws `ReplicaError` before any change when the frame does not fit the replica. */
  apply(frame: SnapshotFrame | DeltaFrame | HistoryFrame): ReplicaChange {
    switch (frame.kind) {
      case 'snapshot':
        return this.applySnapshot(frame);
      case 'delta':
        return this.applyDelta(frame);
      case 'history':
        return this.applyHistory(frame);
    }
  }

  /** The style for `id`; 0 is always the default style, an unknown id `undefined`. */
  style(id: number): Style | undefined {
    return id === 0 ? DEFAULT_STYLE : this.styles.get(id);
  }

  /** Live-screen row `y` (0 is the top), or `undefined` outside the grid. */
  screenRow(y: number): ReplicaRow | undefined {
    return this.lines[y];
  }

  /** A fetched scrollback row by absolute line number, or `undefined` when not fetched (or no longer kept). */
  historyRow(line: number): ReplicaRow | undefined {
    return this.history.get(line);
  }

  /** Any line by absolute number: `scrollbackBase..screenTop` is scrollback, `screenTop..+rows` the live screen. */
  line(line: number): ReplicaRow | undefined {
    const y = line - this.screenTop;
    return y >= 0 ? this.lines[y] : this.history.get(line);
  }

  /** Absolute line number of the live screen's top row. */
  get screenTop(): number {
    return this.scrollbackBase + this.scrollbackRows;
  }

  /** Absolute lines in `[from, to)` of scrollback that no History carried yet, as the first run of them. */
  missingHistory(from: number, to: number): { start: number; count: number } | null {
    const lo = Math.max(this.scrollbackBase, from);
    const hi = Math.min(this.screenTop, to);
    let start = -1;
    for (let l = lo; l < hi; l++) {
      if (this.history.has(l)) {
        if (start >= 0) return { start, count: l - start };
      } else if (start < 0) {
        start = l;
      }
    }
    return start >= 0 ? { start, count: hi - start } : null;
  }

  private checkSeq(seq: number): void {
    if (this.lastSeq !== null && seq <= this.lastSeq) {
      throw new ReplicaError(`sequence went from ${this.lastSeq} to ${seq}`);
    }
  }

  private added(table: Map<number, Style>, entries: readonly StyleEntry[]): Map<number, Style> {
    const out = new Map<number, Style>();
    for (const e of entries) {
      if (e.id === 0 || table.has(e.id) || out.has(e.id)) {
        throw new ReplicaError(`style id ${e.id} is 0 or already interned`);
      }
      out.set(e.id, e.style);
    }
    return out;
  }

  private checkRow(row: Row, cols: number, known: (id: number) => boolean): void {
    if (row.codepoints.length > cols) {
      throw new ReplicaError(
        `row ${row.index} has ${row.codepoints.length} cells in ${cols} columns`,
      );
    }
    let last = 0;
    for (const id of row.styles) {
      if (id === last || id === 0) continue;
      if (!known(id)) throw new ReplicaError(`row ${row.index} uses unknown style ${id}`);
      last = id;
    }
  }

  private make(row: Row): ReplicaRow {
    return { row, version: this.nextVersion++, hash: rowHash(row, this.styleEpoch) };
  }

  private applySnapshot(s: SnapshotFrame): ReplicaChange {
    this.checkSeq(s.seq);
    const styles = this.added(new Map(), s.styles);
    const known = (id: number) => styles.has(id);
    for (const row of s.lines) {
      if (row.index < 0 || row.index >= s.rows) {
        throw new ReplicaError(`row ${row.index} outside ${s.rows} rows`);
      }
      this.checkRow(row, s.cols, known);
    }
    this.styleEpoch++;
    const lines: ReplicaRow[] = [];
    for (let y = 0; y < s.rows; y++) lines.push(this.make(blankRow(y)));
    for (const row of s.lines) lines[row.index] = this.make(row);
    this.cols = s.cols;
    this.rows = s.rows;
    this.lines = lines;
    this.styles = styles;
    this.cursor = s.cursor;
    this.modes = s.modes;
    this.scrollbackRows = s.scrollbackRows;
    this.scrollbackBase = s.scrollbackBase;
    this.history.clear();
    this.lastSeq = s.seq;
    return {
      kind: 'snapshot',
      rows: lines.map((_, y) => y),
      historyRows: [],
      pushed: 0,
      historyReset: true,
    };
  }

  private applyDelta(d: DeltaFrame): ReplicaChange {
    if (this.lastSeq === null) throw new ReplicaError('a Delta before the first Snapshot');
    this.checkSeq(d.seq);
    if (d.scrollbackBase < this.scrollbackBase) {
      throw new ReplicaError(
        `scrollback base went from ${this.scrollbackBase} to ${d.scrollbackBase}`,
      );
    }
    const added = this.added(this.styles, d.stylesAdded);
    const known = (id: number) => this.styles.has(id) || added.has(id);
    for (const row of d.lines) {
      if (row.index < 0 || row.index >= this.rows) {
        throw new ReplicaError(`row ${row.index} outside ${this.rows} rows`);
      }
      this.checkRow(row, this.cols, known);
    }
    for (const [id, style] of added) this.styles.set(id, style);
    const rows: number[] = [];
    for (const row of d.lines) {
      this.lines[row.index] = this.make(row);
      rows.push(row.index);
    }
    const pushed = d.scrollbackBase + d.scrollbackRows - this.screenTop;
    let historyReset = false;
    if (d.scrollbackBase > this.scrollbackBase) {
      for (const line of this.history.keys()) {
        if (line < d.scrollbackBase) {
          this.history.delete(line);
          historyReset = true;
        }
      }
    }
    this.cursor = d.cursor;
    this.modes = d.modes;
    this.scrollbackRows = d.scrollbackRows;
    this.scrollbackBase = d.scrollbackBase;
    this.lastSeq = d.seq;
    return { kind: 'delta', rows, historyRows: [], pushed: Math.max(pushed, 0), historyReset };
  }

  private applyHistory(h: HistoryFrame): ReplicaChange {
    if (this.lastSeq === null) throw new ReplicaError('a History before the first Snapshot');
    const added = this.added(this.styles, h.stylesAdded);
    const known = (id: number) => this.styles.has(id) || added.has(id);
    for (const row of h.lines) this.checkRow(row, this.cols, known);
    for (const [id, style] of added) this.styles.set(id, style);
    const historyRows: number[] = [];
    h.lines.forEach((row, i) => {
      const line = h.start + i;
      if (line < this.scrollbackBase) return;
      this.history.set(line, this.make(row));
      historyRows.push(line);
    });
    return { kind: 'history', rows: [], historyRows, pushed: 0, historyReset: false };
  }
}
