import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Socket, UnixSocketListener } from 'bun';
import {
  connectPane,
  type DataConnection,
  type DataConnectionState,
  dataSocketPath,
} from './data-client';
import { type ClientFrame, encodeFrame, FrameReader, type ServerFrame } from './frames';

const size = { cols: 80, rows: 24, cellWidthPx: 8, cellHeightPx: 16 };

const snapshot = (seq: number): ServerFrame => ({
  kind: 'snapshot',
  seq,
  cols: 80,
  rows: 24,
  cursor: { col: 0, row: 0, shape: 'block', visible: true, blinking: false },
  modes: 0,
  scrollbackRows: 0,
  scrollbackBase: 0,
  styles: [],
  lines: [],
});

interface FakePlyd {
  path: string;
  received: ClientFrame[];
  sockets: Socket<undefined>[];
  listener: UnixSocketListener<undefined>;
  onFrame: (frame: ClientFrame, socket: Socket<undefined>) => void;
}

const cleanup: (() => void)[] = [];

afterEach(() => {
  for (const c of cleanup.splice(0)) c();
});

function tempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), 'ply-c2-'));
  cleanup.push(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function plyd(path = join(tempDir(), 'data.sock')): FakePlyd {
  const fake: FakePlyd = {
    path,
    received: [],
    sockets: [],
    listener: undefined as unknown as UnixSocketListener<undefined>,
    onFrame: (frame, socket) => {
      if (frame.kind === 'attach') socket.write(encodeFrame(snapshot(1)));
    },
  };
  const readers = new Map<Socket<undefined>, FrameReader>();
  fake.listener = Bun.listen<undefined>({
    unix: path,
    socket: {
      open: (s) => {
        fake.sockets.push(s);
        readers.set(s, new FrameReader());
      },
      data: (s, chunk) => {
        const r = readers.get(s);
        if (!r) return;
        r.push(chunk);
        for (let f = r.next(); f; f = r.next()) {
          fake.received.push(f as ClientFrame);
          fake.onFrame(f as ClientFrame, s);
        }
      },
      close: (s) => {
        readers.delete(s);
      },
    },
  });
  cleanup.push(() => fake.listener.stop(true));
  return fake;
}

async function until(what: string, check: () => boolean, ms = 2_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!check()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await Bun.sleep(5);
  }
}

function open(path: string, paneId = 7) {
  const states: DataConnectionState[] = [];
  const frames: ServerFrame[] = [];
  const conn: DataConnection = connectPane(
    { socketPath: path, paneId, size, initialBackoffMs: 20, maxBackoffMs: 80 },
    { onFrames: (f) => frames.push(...f), onState: (s) => states.push(s) },
  );
  cleanup.push(() => conn.close());
  return { conn, states, frames };
}

describe('C2 data client', () => {
  test('attaches with the C2 version, grid and cell size, acks the Snapshot and sends input', async () => {
    const server = plyd();
    const { conn, states, frames } = open(server.path);
    await until('attach', () => conn.state.kind === 'attached');
    expect(server.received[0]).toEqual({ kind: 'attach', v: 1, paneId: 7, ...size });
    expect(frames.map((f) => f.kind)).toEqual(['snapshot']);
    await until('ack', () => server.received.some((f) => f.kind === 'ack'));
    expect(server.received[1]).toEqual({ kind: 'ack', seq: 1 });
    expect(states.map((s) => s.kind)).toEqual(['attached']);

    expect(conn.send({ kind: 'focus', focused: true })).toBe(true);
    conn.resize(size);
    conn.resize({ ...size, cols: 100 });
    await until('resize', () => server.received.some((f) => f.kind === 'resize'));
    expect(server.received.slice(2)).toEqual([
      { kind: 'focus', focused: true },
      { kind: 'resize', ...size, cols: 100 },
    ]);
    expect(conn.stats.frames).toBe(1);
  });

  test('acks only the newest of several frames in one read and passes the rest through', async () => {
    const server = plyd();
    server.onFrame = (frame, socket) => {
      if (frame.kind !== 'attach') return;
      const delta: ServerFrame = {
        kind: 'delta',
        seq: 2,
        cursor: { col: 1, row: 0, shape: 'bar', visible: true, blinking: false },
        modes: 0,
        scrollbackRows: 0,
        scrollbackBase: 0,
        stylesAdded: [],
        lines: [],
      };
      socket.write(
        Buffer.concat([
          encodeFrame(snapshot(1)),
          encodeFrame({ kind: 'title', title: 'zsh' }),
          encodeFrame(delta),
          encodeFrame({ kind: 'bell' }),
        ]),
      );
    };
    const { frames } = open(server.path);
    await until('ack', () => server.received.some((f) => f.kind === 'ack'));
    expect(frames.map((f) => f.kind)).toEqual(['snapshot', 'title', 'delta', 'bell']);
    expect(server.received.filter((f) => f.kind === 'ack')).toEqual([{ kind: 'ack', seq: 2 }]);
  });

  test('a resize made before the Snapshot arrives is sent once attached', async () => {
    const server = plyd();
    let reply: (() => void) | null = null;
    server.onFrame = (frame, socket) => {
      if (frame.kind === 'attach') reply = () => socket.write(encodeFrame(snapshot(1)));
    };
    const { conn } = open(server.path);
    await until('attach frame', () => reply !== null);
    conn.resize({ ...size, rows: 30 });
    (reply as unknown as () => void)();
    await until('resize', () => server.received.some((f) => f.kind === 'resize'));
    expect(server.received.find((f) => f.kind === 'resize')).toEqual({
      kind: 'resize',
      ...size,
      rows: 30,
    });
  });

  test('reconnects with growing backoff while plyd is away and reattaches when it returns', async () => {
    const path = join(tempDir(), 'data.sock');
    const { conn, states } = open(path);
    await until('three retries', () => states.filter((s) => s.kind === 'down').length >= 3);
    const delays = states.flatMap((s) => (s.kind === 'down' ? [s.retryInMs] : []));
    expect(delays.slice(0, 4)).toEqual([20, 40, 80, 80].slice(0, delays.length));
    expect(conn.send({ kind: 'focus', focused: true })).toBe(false);
    const server = plyd(path);
    await until('attach', () => conn.state.kind === 'attached');
    expect(server.received[0]?.kind).toBe('attach');

    for (const s of server.sockets) s.end();
    await until('down again', () => conn.state.kind === 'down');
    await until('reattach', () => server.received.filter((f) => f.kind === 'attach').length === 2);
    await until('attached again', () => conn.state.kind === 'attached');
  });

  test('an ATTACH_REFUSED ends the connection for good', async () => {
    const server = plyd();
    server.onFrame = (frame, socket) => {
      if (frame.kind === 'attach') {
        socket.write(
          encodeFrame({ kind: 'attachRefused', reason: 'unknownPane', message: 'no pane 7' }),
        );
      }
    };
    const { conn } = open(server.path);
    await until('refused', () => conn.state.kind === 'refused');
    expect(conn.state).toEqual({ kind: 'refused', reason: 'unknownPane', message: 'no pane 7' });
    await Bun.sleep(60);
    expect(server.received.filter((f) => f.kind === 'attach').length).toBe(1);
  });

  test('a frame header over 1 MiB drops the connection and reattaches', async () => {
    const server = plyd();
    let attaches = 0;
    server.onFrame = (frame, socket) => {
      if (frame.kind !== 'attach') return;
      attaches++;
      socket.write(
        attaches === 1 ? Uint8Array.from([0x01, 0x00, 0x10, 0x00, 0x20]) : encodeFrame(snapshot(1)),
      );
    };
    const { conn } = open(server.path);
    await until('attached after the bad frame', () => conn.state.kind === 'attached');
    expect(attaches).toBe(2);
  });

  test('close() detaches and stops retrying', async () => {
    const server = plyd();
    const { conn, states } = open(server.path);
    await until('attach', () => conn.state.kind === 'attached');
    conn.close();
    expect(states.at(-1)).toEqual({ kind: 'closed' });
    await Bun.sleep(60);
    expect(server.received.filter((f) => f.kind === 'attach').length).toBe(1);
  });

  test('the socket path follows PLY_HOME', () => {
    expect(dataSocketPath({ PLY_HOME: '/tmp/example' })).toBe('/tmp/example/run/data.sock');
    expect(dataSocketPath({})).toMatch(/Library\/Application Support\/ply\/run\/data\.sock$/);
  });
});
