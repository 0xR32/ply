import type { DataConnection, DataConnectionState, DecodeStats, GridSize } from './data-client';
import {
  CellFlags,
  type ClientFrame,
  type KeyFrame,
  MAX_HISTORY_ROWS,
  Modes,
  type MouseFrame,
  type Row,
  type ServerFrame,
} from './frames';
import type { TerminalHost } from './host';
import { terminalLog } from './log';
import { Replica, ReplicaError } from './replica';
import { type Selection, selectedText, selectionBounds } from './selection';

/** What a session reports to its pane header (spec R-R11). */
export interface SessionCallbacks {
  onTitle?: (title: string) => void;
  onBell?: () => void;
  onExit?: (code: number) => void;
}

/** React render cost of one pane (spec R-R17), measured by the view from a change to its commit, in milliseconds. */
export interface RenderStats {
  renders: number;
  lastMs: number;
  maxMs: number;
  totalMs: number;
}

/** Decode and render timing of one pane, the numbers P1 is judged by. */
export interface TerminalStats {
  paneId: number;
  decode: DecodeStats;
  render: RenderStats;
}

const live = new Map<number, TerminalSession>();

/** Timing of every mounted terminal, by pane id: the test and bench hook of spec R-R17. */
export function terminalStats(): TerminalStats[] {
  return [...live.values()].map((s) => s.stats());
}

const pending = new Set<TerminalSession>();
let flushTimer: ReturnType<typeof setTimeout> | null = null;
let lastFlush = Number.NEGATIVE_INFINITY;
const FRAME_MS = 16;

// One timer for every pane, at most once per 60 Hz frame (P1, Ruling R30; plyd sends up to 120 Hz), all panes in one React batch.
function schedule(session: TerminalSession): void {
  pending.add(session);
  if (flushTimer) return;
  const wait = Math.max(0, lastFlush + FRAME_MS - performance.now());
  flushTimer = setTimeout(() => {
    flushTimer = null;
    lastFlush = performance.now();
    const due = [...pending];
    pending.clear();
    for (const s of due) s.notify();
  }, wait);
}

const HISTORY_WAIT_MS = 5_000;

/** One find result: columns `[from, to)` of absolute line `line`. */
export interface FindMatch {
  line: number;
  from: number;
  to: number;
}

function lineText(row: Row): { text: string; cols: number[] } {
  let text = '';
  const cols: number[] = [];
  for (let c = 0; c < row.codepoints.length; c++) {
    const f = row.flags[c] as number;
    if ((f & (CellFlags.spacer | CellFlags.spacerHead)) !== 0) continue;
    const cp = row.codepoints[c] as number;
    const extra = row.graphemes?.get(c);
    const piece =
      (cp === 0 ? ' ' : String.fromCodePoint(cp)) + (extra ? String.fromCodePoint(...extra) : '');
    for (let i = 0; i < piece.length; i++) cols.push(c);
    text += piece;
  }
  return { text, cols };
}

/**
 * One mounted pane: its C2 connection, replica, scroll position, selection and pending paste; the view reads its fields after each `version` change and calls its methods for input.
 */
export class TerminalSession {
  readonly replica = new Replica();
  /** Bumped on every visible change; the view's `useSyncExternalStore` snapshot. */
  version = 0;
  /** The C2 connection's last reported state. */
  state: DataConnectionState = { kind: 'connecting' };
  /** A Snapshot has arrived at least once, so the grid shows real content (possibly stale while reconnecting). */
  attachedOnce = false;
  /** Rows scrolled back from the live screen; 0 follows the output. */
  offset = 0;
  selection: Selection | null = null;
  /** Text plyd refused as unsafe (Ruling R21), waiting for the user to confirm or cancel. */
  pendingPaste: string | null = null;
  /** The code of an EXIT frame, once the process ended. */
  exitCode: number | null = null;
  /** Filled by the view through `recordRender`. */
  readonly render: RenderStats = { renders: 0, lastMs: 0, maxMs: 0, totalMs: 0 };
  private conn: DataConnection | null = null;
  private focused = false;
  private lastPaste: string | null = null;
  private fetching: number | null = null;
  private exhausted = new Set<number>();
  private waiters: { from: number; to: number; done: () => void }[] = [];
  private readonly listeners = new Set<() => void>();
  private disposed = false;

  constructor(
    private readonly host: TerminalHost,
    readonly paneId: number,
    private callbacks: SessionCallbacks,
  ) {}

  /** Replaces the header callbacks (the view passes fresh closures on every render). */
  setCallbacks(callbacks: SessionCallbacks): void {
    this.callbacks = callbacks;
  }

  /** Attaches with `size` on the first call and resizes afterwards; a size of zero cells is ignored. */
  resize(size: GridSize): void {
    if (this.disposed || size.cols < 1 || size.rows < 1) return;
    if (this.conn) {
      this.conn.resize(size);
      return;
    }
    live.set(this.paneId, this);
    this.conn = this.host.connect(
      { socketPath: this.host.socketPath, paneId: this.paneId, size },
      { onFrames: (frames) => this.onFrames(frames), onState: (state) => this.onState(state) },
    );
  }

  /** Detaches (spec R-R20: only visible panes stay attached); the session is unusable afterwards. */
  dispose(): void {
    this.disposed = true;
    if (live.get(this.paneId) === this) live.delete(this.paneId);
    pending.delete(this);
    this.conn?.close();
    this.conn = null;
    for (const w of this.waiters) w.done();
    this.waiters = [];
    this.listeners.clear();
  }

  /** Adds a change listener (called after the frame scheduler's flush); returns its removal. */
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** The current `version`, for `useSyncExternalStore`. */
  getVersion = (): number => this.version;

  /** Runs the listeners of a scheduled change; only the frame scheduler calls it. */
  notify(): void {
    for (const l of this.listeners) {
      try {
        l();
      } catch (error) {
        terminalLog('error', 'a terminal listener failed', {
          pane_id: this.paneId,
          error: String(error),
        });
      }
    }
  }

  /** Decode and render timing so far. */
  stats(): TerminalStats {
    return {
      paneId: this.paneId,
      decode: this.conn?.stats ?? { frames: 0, batches: 0, lastMs: 0, maxMs: 0, totalMs: 0 },
      render: this.render,
    };
  }

  private changed(): void {
    this.version++;
    if (!this.disposed) schedule(this);
  }

  private onState(state: DataConnectionState): void {
    this.state = state;
    if (state.kind === 'attached') {
      this.attachedOnce = true;
      if (this.focused) this.send({ kind: 'focus', focused: true });
    } else {
      this.replica.resetSequence();
      this.fetching = null;
    }
    this.changed();
  }

  private onFrames(frames: ServerFrame[]): void {
    let historyArrived = false;
    for (const f of frames) {
      switch (f.kind) {
        case 'snapshot':
        case 'delta':
        case 'history':
          this.applyScreen(f);
          if (f.kind === 'history') {
            historyArrived = true;
            if (f.lines.length === 0 && this.fetching !== null) this.exhausted.add(this.fetching);
          }
          break;
        case 'title':
          this.callbacks.onTitle?.(f.title);
          break;
        case 'bell':
          this.callbacks.onBell?.();
          break;
        case 'exit':
          this.exitCode = f.code;
          this.callbacks.onExit?.(f.code);
          break;
        case 'pasteRejected':
          this.pendingPaste = this.lastPaste;
          break;
        case 'attachRefused':
          break;
      }
    }
    if (historyArrived) {
      this.fetching = null;
      this.settleWaiters();
    }
    this.fetchVisibleHistory();
    this.changed();
  }

  private applyScreen(f: Extract<ServerFrame, { kind: 'snapshot' | 'delta' | 'history' }>): void {
    let change: ReturnType<Replica['apply']>;
    try {
      change = this.replica.apply(f);
    } catch (error) {
      if (error instanceof ReplicaError) {
        terminalLog('error', 'a C2 frame contradicts the replica', {
          pane_id: this.paneId,
          kind: f.kind,
          error: error.message,
        });
      }
      throw error;
    }
    if (change.kind === 'snapshot' || change.historyReset) this.exhausted.clear();
    if (change.pushed > 0 && this.offset > 0) this.offset += change.pushed;
    this.offset = Math.min(this.offset, this.replica.scrollbackRows);
    if ((this.replica.modes & Modes.altScreen) !== 0) this.offset = 0;
  }

  private send(frame: ClientFrame): boolean {
    return this.conn?.send(frame) ?? false;
  }

  /** Absolute line of the top visible row. */
  get viewTop(): number {
    return this.replica.screenTop - this.offset;
  }

  /** Scrolls ply's own scrollback by `rows` (positive shows older lines), clamped to what plyd keeps. */
  scrollBy(rows: number): void {
    const next = Math.min(Math.max(this.offset + rows, 0), this.replica.scrollbackRows);
    if (next === this.offset) return;
    this.offset = next;
    this.fetchVisibleHistory();
    this.changed();
  }

  private request(from: number, to: number): void {
    if (this.fetching !== null || this.state.kind !== 'attached') return;
    const gap = this.replica.missingHistory(from, to);
    if (!gap || this.exhausted.has(gap.start)) return;
    const count = Math.min(gap.count, MAX_HISTORY_ROWS);
    // plyd may answer with fewer rows than asked (1 MiB cap); the next call asks again from the first missing line.
    if (
      this.send({ kind: 'fetchHistory', start: gap.start - this.replica.scrollbackRows, count })
    ) {
      this.fetching = gap.start;
    }
  }

  private fetchVisibleHistory(): void {
    if (this.offset > 0) {
      const rows = this.replica.rows;
      this.request(this.viewTop - rows, this.viewTop + rows);
    }
    const w = this.waiters[0];
    if (w) this.request(w.from, w.to);
  }

  private settleWaiters(): void {
    this.waiters = this.waiters.filter((w) => {
      const gap = this.replica.missingHistory(w.from, w.to);
      if (gap && !this.exhausted.has(gap.start)) return true;
      w.done();
      return false;
    });
  }

  /** Fetches every scrollback line in `[from, to)`; resolves when they are all here, plyd has no more, or after 5 s. */
  loadHistory(from: number, to: number): Promise<void> {
    if (!this.replica.missingHistory(from, to)) return Promise.resolve();
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        terminalLog('warn', 'scrollback did not arrive in time', {
          pane_id: this.paneId,
          from,
          to,
        });
        this.waiters = this.waiters.filter((w) => w.done !== done);
        resolve();
      }, HISTORY_WAIT_MS);
      const done = () => {
        clearTimeout(timer);
        resolve();
      };
      this.waiters.push({ from, to, done });
      this.fetchVisibleHistory();
    });
  }

  /** Sends a key; a press returns the view to the live screen and drops the selection, as typing does in a terminal. */
  sendKey(frame: KeyFrame): void {
    if (frame.action !== 'release' && (this.offset !== 0 || this.selection)) {
      this.offset = 0;
      this.selection = null;
      this.changed();
    }
    this.send(frame);
  }

  /** Sends a mouse event; only called while the program reports the mouse (Modes). */
  sendMouse(frame: MouseFrame): void {
    this.send(frame);
  }

  /** Reports focus to plyd (FOCUS, encoded only when the program enabled mode 1004). */
  focus(focused: boolean): void {
    if (this.focused === focused) return;
    this.focused = focused;
    this.send({ kind: 'focus', focused });
    this.changed();
  }

  /** Whether the pane has focus, as last reported with `focus`. */
  get isFocused(): boolean {
    return this.focused;
  }

  /** Pastes `text` (bracketed when the program enabled it); plyd answers PASTE_REJECTED when it is unsafe. */
  paste(text: string, allowUnsafe = false): void {
    if (text === '') return;
    this.lastPaste = text;
    this.pendingPaste = null;
    this.offset = 0;
    this.send({ kind: 'paste', allowUnsafe, text });
    this.changed();
  }

  /** Resends the refused paste with `allow_unsafe` (the user confirmed, Ruling R21). */
  confirmPaste(): void {
    const text = this.pendingPaste;
    if (text !== null) this.paste(text, true);
  }

  /** Drops the refused paste without sending it. */
  cancelPaste(): void {
    if (this.pendingPaste === null) return;
    this.pendingPaste = null;
    this.changed();
  }

  /** Adds one render of the view, measured from a change to its commit. */
  recordRender(ms: number): void {
    const r = this.render;
    r.renders++;
    r.lastMs = ms;
    r.totalMs += ms;
    r.maxMs = Math.max(r.maxMs, ms);
  }

  /** Every case-insensitive match of `query` in the lines held now (screen plus fetched scrollback), oldest first. */
  findMatches(query: string): FindMatch[] {
    const needle = query.toLowerCase();
    if (needle === '') return [];
    const out: FindMatch[] = [];
    const r = this.replica;
    for (let line = 0; line < r.screenTop + r.rows; line++) {
      const row = r.line(line)?.row;
      if (!row) continue;
      const { text, cols } = lineText(row);
      const hay = text.toLowerCase();
      for (let at = hay.indexOf(needle); at >= 0; at = hay.indexOf(needle, at + needle.length)) {
        const from = cols[at] ?? 0;
        const last = cols[at + needle.length - 1] ?? from;
        out.push({ line, from, to: last + 1 });
      }
    }
    return out;
  }

  /** Scrolls so absolute line `line` is on screen, following the output again when it is on the live screen. */
  reveal(line: number): void {
    const r = this.replica;
    const top = this.viewTop;
    let offset = this.offset;
    if (line < top) offset = r.screenTop - line;
    else if (line >= top + r.rows) offset = Math.max(0, r.screenTop - (line - r.rows + 1));
    this.scrollBy(offset - this.offset);
  }

  /** Replaces the selection (null clears it). */
  select(selection: Selection | null): void {
    this.selection = selection;
    this.changed();
  }

  /** Selects every line plyd keeps, scrollback included (⌘A), and starts fetching the scrollback it covers. */
  selectAll(): void {
    const r = this.replica;
    if (r.rows === 0) return;
    this.select({
      anchor: { line: 0, col: 0 },
      head: { line: r.screenTop + r.rows - 1, col: Math.max(r.cols - 1, 0) },
      unit: 'cell',
    });
    void this.loadHistory(0, r.screenTop);
  }

  /** Copies the selection (⌘C), fetching the scrollback it covers first; does nothing without a selection and never sends ^C. */
  async copySelection(): Promise<boolean> {
    const sel = this.selection;
    if (!sel) return false;
    const { start, end } = selectionBounds(sel, this.replica);
    await this.loadHistory(start.line, Math.min(end.line + 1, this.replica.screenTop));
    const text = selectedText(sel, this.replica);
    try {
      await this.host.writeClipboard(text);
      return true;
    } catch (error) {
      terminalLog('error', 'writing the clipboard failed', {
        pane_id: this.paneId,
        error: String(error),
      });
      return false;
    }
  }

  /** Pastes the clipboard's text (⌘V). */
  async pasteClipboard(): Promise<void> {
    let text: string;
    try {
      text = await this.host.readClipboard();
    } catch (error) {
      terminalLog('error', 'reading the clipboard failed', {
        pane_id: this.paneId,
        error: String(error),
      });
      return;
    }
    if (!this.disposed) this.paste(text);
  }
}
