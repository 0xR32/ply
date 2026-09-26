import { afterEach, describe, expect, test } from 'bun:test';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createControlClient } from '../ipc/control-client';
import { MockServer } from '../ipc/mock-server';
import { accentAlternatives } from '../theme/tokens';
import type { Action } from './actions';
import { startEffects } from './effects';
import { type AppState, initialState } from './reducer';
import { selectDirSuggestions, selectForeignDaemon } from './selectors';
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
  const quits: number[] = [];
  cleanups.push(
    startEffects(store, {
      client,
      layoutSaveDelayMs: 5,
      settingsSaveDelayMs: 5,
      noticeMs: 30,
      reducedMotion: async () => true,
      buildId: async () => '0.1.0+0a1b2c3d4e5f',
      keyRepeat: async () => ({ delayMs: 225, intervalMs: 30 }),
      usageRefreshMs: 20,
      quit: () => quits.push(Date.now()),
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
  return { server, store, seen, until, state, quits };
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
      'task.list',
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
    const { state, seen } = await setup((server) =>
      server.refuseNext('theme.set', 'internal', 'cannot write config.toml'),
    );
    expect(state().tabs.map((t) => t.pane_ids)).toEqual([[1, 2, 3], [4]]);
    expect(seen).toContainEqual({
      type: 'notice/show',
      text: 'Setting the terminal colours failed: cannot write config.toml',
    });
  });

  test('Restart plyd stops it without the sessions and the app reconnects and reloads', async () => {
    const { server, store, until, seen, quits } = await setup();
    store.dispatch({ type: 'daemon/restart' });
    await until(() => seen.filter((a) => a.type === 'session/loaded').length === 2, 'a reload');
    expect(server.requestsOf('daemon.shutdown')).toEqual([{ kill_panes: false }]);
    expect(quits).toEqual([]);
  });

  test('Quit ply and stop sessions asks first, then stops every session and quits', async () => {
    const { server, store, until, state, quits } = await setup();
    store.dispatch({ type: 'overlay/open', overlay: { kind: 'quit-confirm' } });
    expect(server.requestsOf('daemon.shutdown')).toEqual([]);
    store.dispatch({ type: 'daemon/quit' });
    expect(state().overlay).toBeNull();
    await until(() => quits.length === 1, 'the quit');
    expect(server.requestsOf('daemon.shutdown')).toEqual([{ kill_panes: true }]);
  });

  test('the app knows its build id and whether plyd is from another build', async () => {
    const { state } = await setup();
    expect(state().env.buildId).toBe('0.1.0+0a1b2c3d4e5f');
    expect(selectForeignDaemon(state())).toBe(true);
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
    store.dispatch({ type: 'command', id: 'tab.new' });
    store.dispatch({
      type: 'pane/create',
      request: { target: 'tab', cli: 'claude', cwd: 'relative' },
    });
    await until(() => state().create.error !== null, 'the error');
    expect(state().create.error).toBe('cwd must be absolute');
    expect(state().overlay?.kind).toBe('new-pane');
  });

  test('a full tab takes no fifth pane: ⌘D, ⌘N and the form stop short with a notice', async () => {
    const { server, store, until, state, seen } = await setup();
    store.dispatch({ type: 'command', id: 'pane.terminalHere' });
    await until(() => state().tabs[0]?.pane_ids.length === 4, 'the fourth pane');
    const created = server.requestsOf('pane.create').length;
    store.dispatch({ type: 'command', id: 'pane.terminalHere' });
    store.dispatch({ type: 'command', id: 'pane.new' });
    expect(state().overlay).toBeNull();
    expect(state().notice?.text).toBe('This tab has 4 panes — ⌘T opens a new tab');
    store.dispatch({
      type: 'pane/create',
      request: { target: 'pane', cli: 'shell', cwd: '/Users/example' },
    });
    expect(state().create.error).toBe('This tab has 4 panes — ⌘T opens a new tab');
    store.dispatch({ type: 'command', id: 'tab.new' });
    expect(state().overlay).toEqual({ kind: 'new-pane', target: 'tab' });
    await Bun.sleep(30);
    expect(server.requestsOf('pane.create')).toHaveLength(created);
    expect(seen.filter((a) => a.type === 'pane/createFailed')).toHaveLength(1);
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
    store.dispatch({ type: 'pane/answer', paneId: 2, answer: 'no' });
    await until(() => state().panes[2]?.status === 'running', 'the answered pane');
    expect(server.requestsOf('pane.answer')).toEqual([{ pane_id: 2, answer: 'no' }]);
    store.dispatch({ type: 'notice/show', text: 'hello' });
    await until(() => state().notice === null, 'the notice to clear');
    server.addPane({ id: 9, cli: 'shell', tab_id: 2 });
    server.dropClients();
    await until(() => state().connection.kind !== 'connected', 'the drop');
    await until(() => seen.filter((a) => a.type === 'session/loaded').length === 2, 'a reload');
    expect(state().tabs[1]?.pane_ids).toContain(9);
  });
});

describe('the usage view’s reads (R59)', () => {
  test('holding ⌘U asks plyd at once and every refresh while held, and stops when it is released', async () => {
    const { server, store, state, until } = await setup((s) => {
      s.usage = {
        codex: {
          as_of: 1_790_331_000,
          windows: [{ label: 'Week', used_percent: 41, resets_at: 1_790_926_095, models: [] }],
        },
      };
    });
    const asked = () => server.requestsOf('usage.get').length;
    expect(state().env.keyRepeat).toEqual({ delayMs: 225, intervalMs: 30 });
    expect(asked()).toBe(0);
    store.dispatch({ type: 'command', id: 'usage.show' });
    await until(() => state().usage.usage !== null, 'the first answer');
    expect(state().usage.usage?.codex?.windows[0]?.used_percent).toBe(41);
    await until(() => asked() >= 3, 'the refreshes');
    store.dispatch({ type: 'usage/hide' });
    const after = asked();
    await Bun.sleep(80);
    expect(asked()).toBe(after);
    expect(state().usage.usage).not.toBeNull();
  });

  test('a plyd from before usage.get is named, and the next answer clears the error', async () => {
    const { server, store, state, until } = await setup((s) =>
      s.refuseNext('usage.get', 'unknown_method', 'unknown method "usage.get"'),
    );
    store.dispatch({ type: 'command', id: 'usage.show' });
    await until(() => state().usage.error !== null, 'the refusal');
    expect(state().usage.error).toBe('This plyd predates usage.get: rebuild it, then Restart plyd');
    await until(() => state().usage.usage !== null, 'the next answer');
    expect(state().usage.error).toBeNull();
    expect(server.requestsOf('usage.get').length).toBeGreaterThanOrEqual(2);
    store.dispatch({ type: 'usage/hide' });
  });
});

describe('the directory search’s sources', () => {
  /** Effects over a temporary home holding `code/ply` (a repository), `code/api` and `notes`, with one pane in `code/ply`. */
  async function searching(pick: () => Promise<string | null> = async () => null) {
    const home = mkdtempSync(join(tmpdir(), 'ply-home-'));
    cleanups.push(() => rmSync(home, { recursive: true, force: true }));
    for (const d of ['code/ply/.git', 'code/api', 'notes'])
      mkdirSync(join(home, d), { recursive: true });
    const socketPath = join(home, 'run', 'plyd.sock');
    const server = MockServer.start({ socketPath, home, scenario: 'empty', startDelayMs: null });
    cleanups.push(() => server.stop());
    server.addPane({ cli: 'claude', cwd: join(home, 'code/ply') });
    server.closedSessions.push(
      { ...closed(9, join(home, 'notes')), closed_at: 1_790_000_100 },
      { ...closed(8, join(home, 'gone')), closed_at: 1_790_000_200 },
    );
    const store = createStore(initialState({ home, shellName: 'zsh', geistAvailable: false }));
    const client = createControlClient({ socketPath, appVersion: '0.1.0', initialBackoffMs: 10 });
    cleanups.push(
      startEffects(store, {
        client,
        reducedMotion: async () => true,
        buildId: async () => null,
        promptForDirectory: pick,
      }),
    );
    const until = async (check: () => boolean, what: string) => {
      const end = Date.now() + 3_000;
      while (!check()) {
        if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
        await Bun.sleep(5);
      }
    };
    await until(() => store.getState().panes[1] !== undefined, 'the session');
    return { home, server, store, until, dirs: () => store.getState().dirs };
  }

  function closed(id: number, cwd: string) {
    return {
      pane_id: id,
      workspace_id: 1,
      cli: 'shell' as const,
      cwd,
      title: 'zsh',
      status: 'exited' as const,
      created_at: 1_790_000_000,
    };
  }

  test('opening the form reads the recent folders from session.list and scans home for repositories', async () => {
    const { home, server, store, until, dirs } = await searching();
    store.dispatch({ type: 'command', id: 'tab.new' });
    await until(() => dirs().recent.length > 0 && dirs().repos.length > 0, 'the sources');
    expect(server.requestsOf('session.list')).toEqual([{ workspace_id: 1, include_closed: true }]);
    expect(dirs().recent).toEqual([join(home, 'code/ply'), join(home, 'notes')]);
    expect(dirs().repos).toEqual([join(home, 'code/ply')]);
    store.dispatch({ type: 'dirs/query', query: '' });
    expect(selectDirSuggestions(store.getState()).map((r) => r.shown)).toEqual([
      '~/code/ply',
      '~/notes',
    ]);
  });

  test('a typed path lists the folder it names, and only the newest listing is kept', async () => {
    const { home, store, until, dirs } = await searching();
    store.dispatch({ type: 'command', id: 'pane.new' });
    store.dispatch({ type: 'dirs/query', query: '~/' });
    store.dispatch({ type: 'dirs/query', query: '~/code/' });
    await until(() => dirs().completion?.dir === join(home, 'code'), 'the listing');
    expect(dirs().completion?.children).toEqual([join(home, 'code/api'), join(home, 'code/ply')]);
    await Bun.sleep(20);
    expect(dirs().completion?.dir).toBe(join(home, 'code'));
    store.dispatch({ type: 'dirs/query', query: '~/code/a' });
    expect(selectDirSuggestions(store.getState()).map((r) => r.shown)).toEqual(['~/code/api']);
  });

  test('Browse fills the field with the picked folder; a cancelled picker leaves it', async () => {
    const answers: (string | null)[] = [null];
    const { home, store, until, dirs } = await searching(async () => answers.shift() ?? null);
    store.dispatch({ type: 'command', id: 'pane.new' });
    expect(dirs().query).toBe('~/code/ply');
    store.dispatch({ type: 'dirs/browse' });
    await Bun.sleep(20);
    expect(dirs().query).toBe('~/code/ply');
    answers.push(join(home, 'notes'));
    store.dispatch({ type: 'dirs/browse' });
    await until(() => dirs().query === '~/notes', 'the picked folder');
    expect(dirs().open).toBe(false);
  });
});

describe('the task queue', () => {
  test('a plyd from before the task queue still loads the session, with the queue unavailable', async () => {
    const { state } = await setup((server) =>
      server.refuseNext('task.list', 'unknown_method', 'unknown method "task.list"'),
    );
    expect(state().tabs.length).toBe(2);
    expect(state().tasks.available).toBe(false);
  });

  test('the queue loads with the session and task/add queues on a pane and closes the form', async () => {
    const { server, store, state, until } = await setup();
    await until(() => state().tasks.available, 'the queue');
    store.dispatch({ type: 'overlay/open', overlay: { kind: 'dispatch', paneId: 1 } });
    store.dispatch({
      type: 'task/add',
      target: { kind: 'pane', paneId: 1 },
      text: '/review-pr 212',
      skill: '/review-pr',
    });
    await until(() => state().overlay === null, 'the form to close');
    const add = server.requests.find((r) => r.m === 'task.add');
    expect(add?.p).toEqual({
      workspace_id: 1,
      target: { pane: 1 },
      text: '/review-pr 212',
      skill: '/review-pr',
    });
    const task = Object.values(state().tasks.tasks)[0];
    expect(task).toMatchObject({ pane_id: 1, text: '/review-pr 212', state: 'queued' });
  });

  test("a refused task keeps the form open with plyd's reason", async () => {
    const { server, store, state, until } = await setup();
    server.refuseNext('task.add', 'invalid_state', 'the queue already holds 32 tasks');
    store.dispatch({ type: 'overlay/open', overlay: { kind: 'dispatch', paneId: 1 } });
    store.dispatch({ type: 'task/add', target: { kind: 'pane', paneId: 1 }, text: 'x' });
    await until(() => state().taskForm.error !== null, 'the error');
    expect(state().taskForm.error).toContain('32 tasks');
    expect(state().overlay?.kind).toBe('dispatch');
  });

  test('a pool target goes to task.add; a new-pane target opens a pane with the text as its first prompt', async () => {
    const { server, store, state, until } = await setup();
    store.dispatch({
      type: 'task/add',
      target: { kind: 'pool', cli: 'codex', cwd: '/Users/example/code/ply' },
      text: '$audit',
    });
    await until(() => server.requests.some((r) => r.m === 'task.add'), 'task.add');
    expect(server.requests.find((r) => r.m === 'task.add')?.p).toMatchObject({
      target: { pool: { cli: 'codex', cwd: '/Users/example/code/ply' } },
    });
    store.dispatch({ type: 'overlay/open', overlay: { kind: 'dispatch' } });
    store.dispatch({
      type: 'task/add',
      target: { kind: 'new', cli: 'claude', cwd: '/Users/example/code/ply' },
      text: 'plan the queue',
    });
    await until(() => state().overlay === null, 'the form to close');
    expect(server.requests.find((r) => r.m === 'pane.create')?.p).toMatchObject({
      cli: 'claude',
      cwd: '/Users/example/code/ply',
      prompt: 'plan the queue',
    });
  });

  test('cancel, move, send and pause reach plyd; skills are asked for and kept', async () => {
    const { server, store, state, until } = await setup();
    server.skills = { skills: [{ name: 'review-pr', invocation: '/review-pr', source: 'user' }] };
    store.dispatch({ type: 'task/add', target: { kind: 'pane', paneId: 1 }, text: 'a' });
    store.dispatch({ type: 'task/add', target: { kind: 'pane', paneId: 1 }, text: 'b' });
    await until(() => Object.keys(state().tasks.tasks).length === 2, 'two tasks');
    const [a, b] = Object.values(state().tasks.tasks).sort((x, y) => x.id - y.id);
    store.dispatch({ type: 'task/move', taskId: b?.id ?? 0, position: 0 });
    store.dispatch({ type: 'queue/pause', paneId: 1, paused: true });
    store.dispatch({ type: 'task/send', taskId: a?.id ?? 0 });
    store.dispatch({ type: 'task/cancel', taskId: a?.id ?? 0 });
    store.dispatch({ type: 'skills/query', cli: 'claude', cwd: '/Users/example/code/ply' });
    await until(() => state().skills.list.length === 1, 'the skills');
    await until(() => state().tasks.queues[1]?.paused === 'user', 'the pause');
    const sent = server.requests
      .map((r) => r.m)
      .filter((m) => m.startsWith('task.') || m.startsWith('queue.') || m === 'skill.list');
    expect(sent).toEqual([
      'task.list',
      'task.add',
      'task.add',
      'task.move',
      'queue.pause',
      'task.send',
      'task.cancel',
      'skill.list',
    ]);
    expect(server.requests.find((r) => r.m === 'skill.list')?.p).toEqual({
      cli: 'claude',
      cwd: '/Users/example/code/ply',
    });
  });
});
