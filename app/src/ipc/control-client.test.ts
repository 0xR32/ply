import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ConnectionState } from '../state/actions';
import { type ControlClient, createControlClient, RequestError } from './control-client';
import { recentLogLines } from './log';
import { MockServer } from './mock-server';
import type { Event } from './proto.gen';

const cleanups: (() => void)[] = [];

afterEach(() => {
  for (const c of cleanups.splice(0).reverse()) c();
});

function socketPath(): string {
  const dir = mkdtempSync(join(tmpdir(), 'ply-c1-'));
  cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
  return join(dir, 'run', 'plyd.sock');
}

function serve(path: string, options: Partial<Parameters<typeof MockServer.start>[0]> = {}) {
  const server = MockServer.start({ socketPath: path, home: '/Users/example', ...options });
  cleanups.push(() => server.stop());
  return server;
}

function client(path: string, startDaemon?: () => Promise<void>): ControlClient {
  const c = createControlClient({
    socketPath: path,
    appVersion: '0.1.0',
    initialBackoffMs: 10,
    maxBackoffMs: 40,
    requestTimeoutMs: 1_000,
    ...(startDaemon ? { startDaemon } : {}),
  });
  cleanups.push(() => c.stop());
  return c;
}

async function until(check: () => boolean, ms = 2_000): Promise<void> {
  const end = Date.now() + ms;
  while (!check()) {
    if (Date.now() > end) throw new Error('condition not met in time');
    await Bun.sleep(5);
  }
}

describe('ControlClient', () => {
  test('handshakes, answers requests and delivers events in order', async () => {
    const path = socketPath();
    const server = serve(path, { scenario: 'demo' });
    const c = client(path);
    const states: ConnectionState['kind'][] = [];
    const events: Event[] = [];
    c.onState((s) => states.push(s.kind));
    c.onEvent((e) => events.push(e));
    c.start();
    await until(() => c.state.kind === 'connected');
    expect(c.state).toEqual({ kind: 'connected', daemonVersion: '0.1.0-mock' });
    const panes = await c.request('pane.list', { workspace_id: 1 });
    expect(panes.map((p) => p.id)).toEqual([1, 2, 3, 4]);
    server.setStatus(1, 'idle');
    server.setProgress(1, { done: 5, total: 5 });
    await until(() => events.length === 2);
    expect(events.map((e) => e.e)).toEqual(['pane.status', 'pane.progress']);
    expect(states).toEqual(['connected']);
    expect(server.requestsOf('pane.list')).toEqual([{ workspace_id: 1 }]);
  });

  test('rejects an error response with its C1 code and a request while down with `disconnected`', async () => {
    const path = socketPath();
    serve(path, { scenario: 'demo' });
    const c = client(path);
    const early = c.request('settings.get', {});
    await expect(early).rejects.toMatchObject({ code: 'disconnected' });
    c.start();
    await until(() => c.state.kind === 'connected');
    const failed = c.request('pane.close', { pane_id: 1, kill: false });
    await expect(failed).rejects.toBeInstanceOf(RequestError);
    await expect(failed).rejects.toMatchObject({ code: 'pane_alive' });
  });

  test('reports "not running", starts the daemon once per outage and connects when it is up', async () => {
    const path = socketPath();
    let starts = 0;
    const c = client(path, async () => {
      starts++;
      serve(path);
    });
    const seen: ConnectionState[] = [];
    c.onState((s) => seen.push(s));
    c.start();
    await until(() => c.state.kind === 'connected');
    expect(starts).toBe(1);
    expect(seen[0]).toMatchObject({ kind: 'down', reason: 'plyd is not running', starting: true });
    expect(recentLogLines().some((l) => l.includes('WARN control connect failed'))).toBe(true);
  });

  test('a start whose plyd never comes up is run again once the retries reach the cap', async () => {
    const path = socketPath();
    let starts = 0;
    const c = client(path, async () => {
      starts++;
      // The first plyd found the old one still holding the lock and exited, as after a Restart.
      if (starts > 1) serve(path);
    });
    c.start();
    await until(() => c.state.kind === 'connected');
    expect(starts).toBe(2);
  });

  test('a failed start is reported and retried with backoff up to the cap', async () => {
    const path = socketPath();
    const c = client(path, async () => {
      throw new Error('plyd is not built');
    });
    const delays: number[] = [];
    c.onState((s) => {
      if (s.kind === 'down' && !s.starting) delays.push(s.retryInMs);
    });
    c.start();
    await until(() => delays.length >= 4);
    expect(delays.slice(0, 4)).toEqual([10, 20, 40, 40]);
    expect(c.state).toMatchObject({ kind: 'down' });
    expect((c.state as { reason: string }).reason).toContain('plyd is not built');
  });

  test('reconnects after plyd drops the connection and fails what was pending', async () => {
    const path = socketPath();
    const server = serve(path, { scenario: 'demo' });
    const c = client(path);
    c.start();
    await until(() => c.state.kind === 'connected');
    const kinds: ConnectionState['kind'][] = [];
    c.onState((s) => kinds.push(s.kind));
    server.dropClients();
    await until(() => kinds.includes('down'));
    await until(() => c.state.kind === 'connected');
    expect(kinds[0]).toBe('down');
    expect((await c.request('workspace.list', {}))[0]?.path).toBe('/Users/example');
  });

  test('a protocol version mismatch is reported as incompatible', async () => {
    const path = socketPath();
    serve(path, { protocolVersion: 2 });
    const c = client(path);
    c.start();
    await until(() => c.state.kind === 'incompatible');
    expect((c.state as { reason: string }).reason).toContain('protocol 2');
  });
});
