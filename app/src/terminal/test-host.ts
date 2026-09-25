import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type {
  DataConnection,
  DataConnectionHandlers,
  DataConnectionOptions,
  DataConnectionState,
  GridSize,
} from './data-client';
import { type ClientFrame, type Frame, FrameReader, type ServerFrame } from './frames';
import type { TerminalHost } from './host';

/** One connection a `FakeHost` handed out: what the view sent, and hooks to play plyd's side. */
export interface FakeConnection {
  readonly options: DataConnectionOptions;
  readonly sent: ClientFrame[];
  readonly sizes: GridSize[];
  closed: boolean;
  /** Delivers frames as one socket read would (a Snapshot also attaches the connection). */
  deliver(frames: ServerFrame[]): void;
  setState(state: DataConnectionState): void;
}

/** A terminal host for tests: no socket, an in-memory clipboard, every connection recorded. */
export interface FakeHost {
  host: TerminalHost;
  connections: FakeConnection[];
  clipboard: { text: string; writes: string[] };
}

/** A fresh fake host; its clipboard starts with `clipboard`. */
export function fakeTerminalHost(clipboard = ''): FakeHost {
  const connections: FakeConnection[] = [];
  const board = { text: clipboard, writes: [] as string[] };
  const host: TerminalHost = {
    socketPath: '/tmp/example/run/data.sock',
    connect(options: DataConnectionOptions, handlers: DataConnectionHandlers): DataConnection {
      let state: DataConnectionState = { kind: 'connecting' };
      const fake: FakeConnection = {
        options,
        sent: [],
        sizes: [options.size],
        closed: false,
        deliver(frames) {
          if (state.kind !== 'attached' && frames.some((f) => f.kind === 'snapshot')) {
            handlers.onFrames(frames);
            fake.setState({ kind: 'attached' });
            return;
          }
          handlers.onFrames(frames);
        },
        setState(next) {
          state = next;
          handlers.onState(next);
        },
      };
      connections.push(fake);
      return {
        get state() {
          return state;
        },
        stats: { frames: 0, batches: 0, lastMs: 0, maxMs: 0, totalMs: 0 },
        send(frame) {
          if (state.kind !== 'attached') return false;
          fake.sent.push(frame);
          return true;
        },
        resize(size) {
          fake.sizes.push(size);
          if (state.kind === 'attached') fake.sent.push({ kind: 'resize', ...size });
        },
        close() {
          fake.closed = true;
        },
      };
    },
    readClipboard: async () => board.text,
    writeClipboard: async (text) => {
      board.text = text;
      board.writes.push(text);
    },
  };
  return { host, connections, clipboard: board };
}

/** Every frame of a recorded fixture in `app/src/terminal/fixtures/` (frames written back to back by ply-term). */
export function fixtureFrames(name: string): Frame[] {
  const reader = new FrameReader();
  reader.push(new Uint8Array(readFileSync(join(import.meta.dir, 'fixtures', name))));
  const out: Frame[] = [];
  for (let f = reader.next(); f; f = reader.next()) out.push(f);
  return out;
}

/** The server frames of a fixture, for feeding a view or a replica. */
export function fixtureServerFrames(name: string): ServerFrame[] {
  return fixtureFrames(name).filter((f): f is ServerFrame =>
    [
      'snapshot',
      'delta',
      'history',
      'title',
      'bell',
      'exit',
      'pasteRejected',
      'attachRefused',
    ].includes(f.kind),
  );
}
