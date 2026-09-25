import { homedir } from 'node:os';
import { join } from 'node:path';
import type { Socket } from 'bun';
import {
  C2_VERSION,
  type ClientFrame,
  encodeFrame,
  FrameError,
  FrameReader,
  type RefuseReason,
  type ServerFrame,
} from './frames';
import { terminalLog } from './log';

/** The C2 socket: `$PLY_HOME/run/data.sock` when set, else `~/Library/Application Support/ply/run/data.sock`. */
export function dataSocketPath(env: NodeJS.ProcessEnv = process.env): string {
  const home = env.PLY_HOME ?? join(homedir(), 'Library', 'Application Support', 'ply');
  return join(home, 'run', 'data.sock');
}

/** The grid a view shows: whole cells, and one cell's size in whole pixels (ATTACH/RESIZE carry the cell, not the view). */
export interface GridSize {
  cols: number;
  rows: number;
  cellWidthPx: number;
  cellHeightPx: number;
}

/** Where one pane's C2 connection stands; `down` retries by itself, `refused` and `closed` never do. */
export type DataConnectionState =
  | { kind: 'connecting' }
  | { kind: 'attached' }
  | { kind: 'down'; reason: string; retryInMs: number }
  | { kind: 'refused'; reason: RefuseReason; message: string }
  | { kind: 'closed' };

/** Decode cost of the frames received so far (spec R-R17), in milliseconds on the main thread. */
export interface DecodeStats {
  frames: number;
  batches: number;
  lastMs: number;
  maxMs: number;
  totalMs: number;
}

/** What a connection reports back; both callbacks run on the main thread and must not throw. */
export interface DataConnectionHandlers {
  /** The frames of one socket read, in wire order; ACK for the newest Snapshot/Delta follows once this returns. */
  onFrames(frames: ServerFrame[]): void;
  onState(state: DataConnectionState): void;
}

/** Where to connect and how to retry (spec: backoff 100 ms doubling to 2 s). */
export interface DataConnectionOptions {
  socketPath: string;
  paneId: number;
  size: GridSize;
  initialBackoffMs?: number;
  maxBackoffMs?: number;
}

/** One pane's attachment over C2 (spec 4.2); frames sent while not attached are dropped, never queued. */
export interface DataConnection {
  readonly state: DataConnectionState;
  readonly stats: DecodeStats;
  /** Sends a client frame when attached; returns whether it went out. */
  send(frame: ClientFrame): boolean;
  /** Records the new grid and sends RESIZE when attached (plyd answers with a Snapshot); an unchanged size sends nothing. */
  resize(size: GridSize): void;
  /** Detaches for good: closes the socket, cancels retries, reports `closed`. */
  close(): void;
}

function sameSize(a: GridSize, b: GridSize): boolean {
  return (
    a.cols === b.cols &&
    a.rows === b.rows &&
    a.cellWidthPx === b.cellWidthPx &&
    a.cellHeightPx === b.cellHeightPx
  );
}

class UnixDataConnection implements DataConnection {
  state: DataConnectionState = { kind: 'connecting' };
  readonly stats: DecodeStats = { frames: 0, batches: 0, lastMs: 0, maxMs: 0, totalMs: 0 };
  private socket: Socket<undefined> | null = null;
  private reader = new FrameReader();
  private outbox: Uint8Array = new Uint8Array(0);
  private closed = false;
  private attached = false;
  private backoffMs: number;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private size: GridSize;
  private sentSize: GridSize | null = null;
  private readonly initialBackoffMs: number;
  private readonly maxBackoffMs: number;

  constructor(
    private readonly options: DataConnectionOptions,
    private readonly handlers: DataConnectionHandlers,
  ) {
    this.size = options.size;
    this.initialBackoffMs = options.initialBackoffMs ?? 100;
    this.maxBackoffMs = options.maxBackoffMs ?? 2_000;
    this.backoffMs = this.initialBackoffMs;
    void this.connect();
  }

  send(frame: ClientFrame): boolean {
    if (!this.attached || !this.socket) return false;
    let bytes: Uint8Array;
    try {
      bytes = encodeFrame(frame);
    } catch (error) {
      terminalLog('error', 'a C2 frame did not encode', {
        pane_id: this.options.paneId,
        kind: frame.kind,
        error: String(error),
      });
      return false;
    }
    this.write(bytes);
    return true;
  }

  resize(size: GridSize): void {
    if (sameSize(size, this.size)) return;
    this.size = size;
    if (this.send({ kind: 'resize', ...size })) this.sentSize = size;
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.attached = false;
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    const socket = this.socket;
    this.socket = null;
    socket?.end();
    this.setState({ kind: 'closed' });
  }

  private setState(state: DataConnectionState): void {
    this.state = state;
    try {
      this.handlers.onState(state);
    } catch (error) {
      terminalLog('error', 'a C2 state listener failed', {
        pane_id: this.options.paneId,
        error: String(error),
      });
    }
  }

  private async connect(): Promise<void> {
    if (this.closed) return;
    this.reader = new FrameReader();
    this.outbox = new Uint8Array(0);
    let socket: Socket<undefined>;
    try {
      socket = await Bun.connect({
        unix: this.options.socketPath,
        socket: {
          data: (s, chunk) => this.onData(s, chunk),
          close: (s) => this.onClose(s),
          drain: () => this.flush(),
          error: (_s, error) =>
            terminalLog('warn', 'C2 socket error', {
              pane_id: this.options.paneId,
              error: String(error),
            }),
        },
      });
    } catch (error) {
      const code = (error as NodeJS.ErrnoException | undefined)?.code;
      if (this.state.kind !== 'down') {
        terminalLog('warn', 'C2 connect failed', {
          pane_id: this.options.paneId,
          code,
          socket: this.options.socketPath,
        });
      }
      this.retry(
        code === 'ENOENT' || code === 'ECONNREFUSED' ? 'plyd is not running' : String(error),
      );
      return;
    }
    if (this.closed) {
      socket.end();
      return;
    }
    this.socket = socket;
    this.sentSize = this.size;
    this.write(
      encodeFrame({ kind: 'attach', v: C2_VERSION, paneId: this.options.paneId, ...this.size }),
    );
  }

  private retry(reason: string): void {
    if (this.closed) return;
    const delay = this.backoffMs;
    this.backoffMs = Math.min(this.backoffMs * 2, this.maxBackoffMs);
    this.setState({ kind: 'down', reason, retryInMs: delay });
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      void this.connect();
    }, delay);
  }

  private write(bytes: Uint8Array): void {
    if (this.outbox.length === 0) {
      this.outbox = bytes;
    } else {
      const merged = new Uint8Array(this.outbox.length + bytes.length);
      merged.set(this.outbox);
      merged.set(bytes, this.outbox.length);
      this.outbox = merged;
    }
    this.flush();
  }

  private flush(): void {
    if (!this.socket || this.outbox.length === 0) return;
    const written = this.socket.write(this.outbox);
    this.outbox = written >= this.outbox.length ? new Uint8Array(0) : this.outbox.subarray(written);
  }

  private onData(socket: Socket<undefined>, chunk: Uint8Array): void {
    if (socket !== this.socket) return;
    const started = performance.now();
    const frames: ServerFrame[] = [];
    let ackSeq = -1;
    let refused: ServerFrame | null = null;
    try {
      this.reader.push(chunk);
      for (let f = this.reader.next(); f; f = this.reader.next()) {
        if (f.kind === 'snapshot' || f.kind === 'delta') {
          ackSeq = f.seq;
        } else if (f.kind === 'attachRefused') {
          refused = f;
          break;
        } else if (
          f.kind !== 'history' &&
          f.kind !== 'title' &&
          f.kind !== 'bell' &&
          f.kind !== 'exit' &&
          f.kind !== 'pasteRejected' &&
          f.kind !== 'clipboardWrite'
        ) {
          throw new FrameError('unknownKind', 0, `plyd sent the client frame ${f.kind}`);
        }
        frames.push(f);
      }
    } catch (error) {
      terminalLog('error', 'C2 protocol error; reattaching', {
        pane_id: this.options.paneId,
        error: String(error),
      });
      socket.end();
      return;
    }
    const took = performance.now() - started;
    this.stats.batches++;
    this.stats.frames += frames.length;
    this.stats.lastMs = took;
    this.stats.totalMs += took;
    this.stats.maxMs = Math.max(this.stats.maxMs, took);
    if (frames.length > 0) this.deliver(frames);
    if (refused?.kind === 'attachRefused') {
      terminalLog('warn', 'plyd refused the attach', {
        pane_id: this.options.paneId,
        reason: refused.reason,
        message: refused.message,
      });
      this.closed = true;
      this.attached = false;
      this.socket = null;
      socket.end();
      this.setState({ kind: 'refused', reason: refused.reason, message: refused.message });
      return;
    }
    if (ackSeq >= 0 && socket === this.socket) {
      this.write(encodeFrame({ kind: 'ack', seq: ackSeq }));
      if (!this.attached) {
        this.attached = true;
        this.backoffMs = this.initialBackoffMs;
        if (this.sentSize && !sameSize(this.sentSize, this.size)) {
          this.send({ kind: 'resize', ...this.size });
          this.sentSize = this.size;
        }
        this.setState({ kind: 'attached' });
      }
    }
  }

  private deliver(frames: ServerFrame[]): void {
    try {
      this.handlers.onFrames(frames);
    } catch (error) {
      terminalLog('error', 'applying C2 frames failed; reattaching', {
        pane_id: this.options.paneId,
        error: String(error),
      });
      this.socket?.end();
    }
  }

  private onClose(socket: Socket<undefined>): void {
    if (socket !== this.socket) return;
    this.socket = null;
    const wasAttached = this.attached;
    this.attached = false;
    if (this.closed) return;
    if (wasAttached) {
      terminalLog('info', 'C2 connection closed; reattaching', { pane_id: this.options.paneId });
    }
    this.retry('plyd closed the connection');
  }
}

/** Opens one pane's C2 connection at once and keeps it attached (reattaching with backoff) until `close()`. */
export function connectPane(
  options: DataConnectionOptions,
  handlers: DataConnectionHandlers,
): DataConnection {
  return new UnixDataConnection(options, handlers);
}
