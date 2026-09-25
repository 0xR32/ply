import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createControlClient } from '../ipc/control-client';
import { MockServer } from '../ipc/mock-server';
import { accentAlternatives } from '../theme/tokens';
import type { Action } from './actions';
import { startEffects } from './effects';
import { type AppState, initialState } from './reducer';
import { createStore } from './store';

const HOME = '/Users/example';
const cleanups: (() => void)[] = [];

afterEach(() => {
  for (const c of cleanups.splice(0).reverse()) c();
});

async function setup(before: (server: MockServer) => void = () => {}) {
  const dir = mkdtempSync(join(tmpdir(), 'ply-fx-'));
  cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
  const socketPath = join(dir, 'run', 'plyd.sock');
  const server = MockServer.start({ socketPath, home: HOME, scenario: 'demo', startDelayMs: null });
  cleanups.push(() => server.stop());
  before(server);
  const store = createStore(initialState({ home: HOME, shellName: 'zsh', geistAvailable: false }));
  const seen: Action[] = [];
  store.addEffect((a) => seen.push(a));
  const client = createControlClient({ socketPath, appVersion: '0.1.0', initialBackoffMs: 10 });
  cleanups.push(
    startEffects(store, {
      client,
      layoutSaveDelayMs: 5,
      settingsSaveDelayMs: 5,
      noticeMs: 30,
      reducedMotion: async () => true,
    }),
  );
  const until = async (check: () => boolean, what: string) => {
    const end = Date.now() + 2_000;
    while (!check()) {
      if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
      await Bun.sleep(5);
    }
  };
  const state = (): AppState => store.getState();
  await until(() => state().workspace !== null, 'the session');
  return { server, store, seen, until, state };
}

describe('effects', () => {
  test('on connect it sends the palette first, then loads settings, workspace, layout and panes', async () => {
    const { server, state } = await setup();
    expect(server.requests.map((r) => r.m)).toEqual([
      'settings.get',
      'theme.set',
      'workspace.list',
      'layout.get',
      'pane.list',
    ]);
    expect(server.palette?.cursor).toBe(accentAlternatives.blue);
    expect(state().tabs.map((t) => t.pane_ids)).toEqual([[1, 2, 3], [4]]);
    expect(state().reducedMotion).toBe(true);
  });

  test('an event read together with the pane.list answer is not lost to the load', async () => {
    const { state, until } = await setup((server) =>
      server.trailAnswer('pane.list', {
        e: 'pane.status',
        p: { pane_id: 1, status: 'waiting_permission', detail: 'Edit a.txt', at: 2 },
      }),
    );
    await until(() => state().panes[1] !== undefined, 'pane 1');
    expect(state().panes[1]).toMatchObject({ status: 'waiting_permission', detail: 'Edit a.txt' });
  });

  test('a theme.set that fails leaves the load going and says why', async () => {
    const { state } = await setup((server) =>
      server.refuseNext('theme.set', 'internal', 'cannot write config.toml'),
    );
    expect(state().tabs.map((t) => t.pane_ids)).toEqual([[1, 2, 3], [4]]);
    expect(state().notice?.text).toBe(
      'Setting the terminal colours failed: cannot write config.toml',
    );
  });

  test('a settings change is saved, and an accent change also re-sends the palette', async () => {
    const { server, store, until } = await setup();
    store.dispatch({
      type: 'settings/change',
      settings: { ...store.getState().settings, accent: 'mint' },
    });
    await until(() => server.requestsOf('settings.set').length === 1, 'settings.set');
    expect(server.settings.accent).toBe('mint');
    expect(server.palette?.cursor).toBe(accentAlternatives.mint);
    store.dispatch({ type: 'command', id: 'font.up' });
    await until(() => server.settings.font_size === 13.5, 'the font size');
  });

  test('focus, zoom and tab changes are saved through layout.save', async () => {
    const { server, store, until } = await setup();
    store.dispatch({ type: 'command', id: 'pane.next' });
    store.dispatch({ type: 'command', id: 'pane.zoom' });
    store.dispatch({ type: 'command', id: 'tab.next' });
    await until(() => server.layout?.active_tab_id === 2, 'the saved layout');
    expect(server.layout?.tabs[0]).toMatchObject({ focus_pane_id: 2, zoomed: true });
    expect(server.requestsOf('layout.save')).toHaveLength(1);
  });

  test('⌘D opens a shell in the focused pane’s directory in this tab', async () => {
    const { server, store, until, state } = await setup();
    store.dispatch({ type: 'command', id: 'tab.next' });
    store.dispatch({ type: 'command', id: 'pane.terminalHere' });
    await until(() => server.requestsOf('pane.create').length === 1, 'pane.create');
    expect(server.requestsOf('pane.create')[0]).toEqual({
      workspace_id: 1,
      cli: 'shell',
      cwd: `${HOME}/code/notes`,
      tab_id: 2,
    });
    await until(() => state().tabs[1]?.pane_ids.length === 2, 'the shell in tab 2');
    expect(state().tabs[1]?.focus_pane_id).toBe(5);
  });

  test('closing: an exited pane closes at once; a live one only after the confirmation, with kill', async () => {
    const { server, store, until, state } = await setup();
    server.setStatus(1, 'exited', undefined, 0);
    await until(() => state().panes[1]?.status === 'exited', 'pane 1 to exit');
    store.dispatch({ type: 'command', id: 'pane.close' });
    await until(() => state().panes[1] === undefined, 'pane 1 to close');
    expect(server.requestsOf('pane.close')).toEqual([{ pane_id: 1, kill: false }]);
    store.dispatch({ type: 'command', id: 'pane.close' });
    expect(state().overlay).toEqual({ kind: 'close-confirm', paneId: 2 });
    store.dispatch({ type: 'pane/closeConfirmed', paneId: 2 });
    await until(() => state().panes[2] === undefined, 'pane 2 to close');
    expect(server.requestsOf('pane.close').at(-1)).toEqual({ pane_id: 2, kill: true });
  });

  test('pane.create drops a worktree for Codex, and a refusal keeps the form open with plyd’s message', async () => {
    const { server, store, until, state } = await setup();
    store.dispatch({ type: 'command', id: 'pane.new' });
    store.dispatch({
      type: 'pane/create',
      request: {
        target: 'pane',
        cli: 'codex',
        cwd: '/Users/example',
        worktree: 'x',
        prompt: ' hi ',
      },
    });
    await until(() => state().overlay === null, 'the codex pane');
    expect(server.requestsOf('pane.create')[0]).toEqual({
      workspace_id: 1,
      tab_id: 1,
      cli: 'codex',
      cwd: '/Users/example',
      prompt: 'hi',
    });
    store.dispatch({ type: 'command', id: 'pane.new' });
    store.dispatch({
      type: 'pane/create',
      request: { target: 'pane', cli: 'claude', cwd: 'relative' },
    });
    await until(() => state().create.error !== null, 'the error');
    expect(state().create.error).toBe('cwd must be absolute');
    expect(state().overlay?.kind).toBe('new-pane');
  });

  test('resuming a lost pane calls pane.resume and takes the relaunched pane', async () => {
    const { server, store, until, state } = await setup();
    server.setStatus(4, 'lost');
    await until(() => state().panes[4]?.status === 'lost', 'the lost pane');
    store.dispatch({ type: 'pane/resume', paneId: 4 });
    await until(() => state().panes[4]?.status !== 'lost', 'the resumed pane');
    expect(server.requestsOf('pane.resume')).toEqual([{ pane_id: 4 }]);
    store.dispatch({ type: 'pane/resume', paneId: 4 });
    await until(() => state().notice !== null, 'the refusal notice');
    expect(state().notice?.text).toContain('Resuming the session failed');
  });

  test('answers go to pane.answer, notices clear themselves, and a reconnect reloads the session', async () => {
    const { server, store, until, state, seen } = await setup();
    store.dispatch({ type: 'pane/answer', paneId: 2, choice: 2 });
    await until(() => state().panes[2]?.status === 'running', 'the answered pane');
    expect(server.requestsOf('pane.answer')).toEqual([{ pane_id: 2, choice: 2 }]);
    store.dispatch({ type: 'notice/show', text: 'hello' });
    await until(() => state().notice === null, 'the notice to clear');
    server.addPane({ id: 9, cli: 'shell', tab_id: 2 });
    server.dropClients();
    await until(() => state().connection.kind !== 'connected', 'the drop');
    await until(() => seen.filter((a) => a.type === 'session/loaded').length === 2, 'a reload');
    expect(state().tabs[1]?.pane_ids).toContain(9);
  });
});
