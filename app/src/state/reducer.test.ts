import { describe, expect, test } from 'bun:test';
import type { Action, Event } from './actions';
import { type AppState, reduce } from './reducer';
import {
  FULL_TAB_NOTICE,
  formatElapsed,
  isLost,
  isTabFull,
  nextWaitingPane,
  resumeHow,
  selectCliCounts,
  selectWaitingCount,
  statusView,
  tabDot,
  visiblePaneIds,
} from './selectors';
import { makePane, makeState, testWorkspace } from './test-support';

function run(state: AppState, ...actions: Action[]): AppState {
  return actions.reduce(reduce, state);
}

function evt(event: Event): Action {
  return { type: 'daemon/event', event };
}

const threePanes = () =>
  makeState([
    makePane({ id: 1, position: 0 }),
    makePane({ id: 2, position: 1, cli: 'codex' }),
    makePane({ id: 3, position: 2, cli: 'shell' }),
  ]);

describe('session/loaded', () => {
  test('orders tabs by the layout, keeps its focus, and appends panes the layout does not list', () => {
    const state = run(makeState([], []), {
      type: 'session/loaded',
      workspace: testWorkspace,
      panes: [
        makePane({ id: 1, tab_id: 7, position: 0 }),
        makePane({ id: 2, tab_id: 7, position: 1 }),
        makePane({ id: 3, tab_id: 9, position: 0, cwd: '/Users/example/code/notes' }),
        makePane({ id: 4, tab_id: 7, position: 2 }),
        makePane({ id: 5, tab_id: 7, position: 3, closed_at: 1_790_000_100 }),
      ],
      layout: {
        tabs: [
          { id: 7, name: 'ply', position: 0, pane_ids: [2, 1], focus_pane_id: 1, zoomed: true },
        ],
        active_tab_id: 7,
      },
      settings: { ...makeState([]).settings, accent: 'mint' },
    });
    expect(state.tabs.map((t) => [t.id, t.name, t.position, t.pane_ids])).toEqual([
      [7, 'ply', 0, [2, 1, 4]],
      [9, 'notes', 1, [3]],
    ]);
    expect(state.tabs[0]?.focus_pane_id).toBe(1);
    expect(state.tabs[0]?.zoomed).toBe(true);
    expect(state.activeTabId).toBe(7);
    expect(Object.keys(state.panes)).toEqual(['1', '2', '3', '4']);
    expect(state.settings.accent).toBe('mint');
  });

  test('falls back to the first tab when the saved active tab is gone', () => {
    const state = run(makeState([], []), {
      type: 'session/loaded',
      workspace: testWorkspace,
      panes: [makePane({ id: 1, tab_id: 3 })],
      layout: { tabs: [], active_tab_id: 99 },
      settings: makeState([]).settings,
    });
    expect(state.activeTabId).toBe(3);
    expect(state.tabs[0]?.name).toBe('ply');
  });
});

describe('daemon events', () => {
  test('pane.added inserts at its position in an existing tab without moving focus', () => {
    const state = run(
      threePanes(),
      evt({ e: 'pane.added', p: makePane({ id: 9, position: 1, cli: 'shell' }) }),
    );
    expect(state.tabs[0]?.pane_ids).toEqual([1, 9, 2, 3]);
    expect(state.tabs[0]?.focus_pane_id).toBe(1);
  });

  test('pane.added for an unknown tab opens a tab named after the directory, not activated', () => {
    const state = run(
      threePanes(),
      evt({ e: 'pane.added', p: makePane({ id: 9, tab_id: 4, cwd: '/Users/example' }) }),
    );
    expect(state.tabs.map((t) => [t.id, t.name])).toEqual([
      [1, 'tab1'],
      [4, '~'],
    ]);
    expect(state.activeTabId).toBe(1);
  });

  test('pane.added from another workspace or of a closed pane is ignored', () => {
    const before = threePanes();
    const after = run(
      before,
      evt({ e: 'pane.added', p: makePane({ id: 9, workspace_id: 2 }) }),
      evt({ e: 'pane.added', p: makePane({ id: 10, closed_at: 1 }) }),
    );
    expect(after).toBe(before);
  });

  test('pane.removed moves focus to the neighbour, drops an empty tab and activates the next', () => {
    const state = makeState(
      [makePane({ id: 1 }), makePane({ id: 2, position: 1 }), makePane({ id: 3, tab_id: 2 })],
      [{ id: 1, focus_pane_id: 2 }, { id: 2 }],
    );
    const a = run(state, evt({ e: 'pane.removed', p: { pane_id: 2 } }));
    expect(a.tabs[0]?.pane_ids).toEqual([1]);
    expect(a.tabs[0]?.focus_pane_id).toBe(1);
    const b = run(a, evt({ e: 'pane.removed', p: { pane_id: 1 } }));
    expect(b.tabs.map((t) => [t.id, t.position])).toEqual([[2, 0]]);
    expect(b.activeTabId).toBe(2);
    expect(b.panes[1]).toBeUndefined();
  });

  test('pane.removed closes a confirmation that was asking about that pane', () => {
    const state = run(threePanes(), { type: 'command', id: 'pane.close' });
    expect(state.overlay).toEqual({ kind: 'close-confirm', paneId: 1 });
    expect(run(state, evt({ e: 'pane.removed', p: { pane_id: 1 } })).overlay).toBeNull();
  });

  test('pane.status replaces status and detail and records when it began', () => {
    const a = run(
      threePanes(),
      evt({
        e: 'pane.status',
        p: { pane_id: 1, status: 'waiting_permission', detail: 'Edit a.ts', at: 100 },
      }),
    );
    expect(a.panes[1]).toMatchObject({
      status: 'waiting_permission',
      detail: 'Edit a.ts',
      statusSince: 100,
    });
    const b = run(a, evt({ e: 'pane.status', p: { pane_id: 1, status: 'running', at: 105 } }));
    expect(b.panes[1]?.detail).toBeUndefined();
    const c = run(b, evt({ e: 'pane.exit', p: { pane_id: 1, code: 2, at: 110 } }));
    expect(c.panes[1]).toMatchObject({ status: 'exited', exit_code: 2, statusSince: 110 });
  });

  test('pane.progress sets and clears the plan; pane.meta keeps unknown fields', () => {
    const a = run(
      threePanes(),
      evt({ e: 'pane.progress', p: { pane_id: 1, progress: { done: 1, total: 3 } } }),
      evt({
        e: 'pane.meta',
        p: { pane_id: 1, cwd: '/Users/example/code/ply', model: 'claude-opus-5', branch: 'main' },
      }),
    );
    expect(a.panes[1]).toMatchObject({
      progress: { done: 1, total: 3 },
      model_seen: 'claude-opus-5',
      branch: 'main',
    });
    const b = run(
      a,
      evt({ e: 'pane.progress', p: { pane_id: 1 } }),
      evt({ e: 'pane.meta', p: { pane_id: 1, cwd: '/Users/example/code/ply' } }),
    );
    expect(b.panes[1]?.progress).toBeUndefined();
    expect(b.panes[1]).toMatchObject({ model_seen: 'claude-opus-5', branch: 'main' });
    const c = run(b, evt({ e: 'pane.meta', p: { pane_id: 1, cwd: '/tmp', worktree: 'wt' } }));
    expect(c.panes[1]).toMatchObject({ cwd: '/tmp', worktree_seen: 'wt' });
    expect(c.panes[1]?.branch).toBeUndefined();
    const d = run(c, evt({ e: 'pane.meta', p: { pane_id: 1, cwd: '/Users/example/code/ply' } }));
    expect(d.panes[1]?.worktree_seen).toBeUndefined();
  });
});

describe('commands', () => {
  test('⌘[ ⌘] cycle focus within the tab, wrapping', () => {
    const next = run(threePanes(), { type: 'command', id: 'pane.next' });
    expect(next.tabs[0]?.focus_pane_id).toBe(2);
    const wrapped = run(threePanes(), { type: 'command', id: 'pane.prev' });
    expect(wrapped.tabs[0]?.focus_pane_id).toBe(3);
  });

  test('⌘⇧[ ⌘⇧] cycle tabs and ⌘digit jumps to a tab that exists', () => {
    const state = makeState(
      [makePane({ id: 1 }), makePane({ id: 2, tab_id: 2 })],
      [{ id: 1 }, { id: 2 }],
    );
    expect(run(state, { type: 'command', id: 'tab.next' }).activeTabId).toBe(2);
    expect(run(state, { type: 'command', id: 'tab.prev' }).activeTabId).toBe(2);
    expect(run(state, { type: 'command', id: 'tab.go.2' }).activeTabId).toBe(2);
    expect(run(state, { type: 'command', id: 'tab.go.5' })).toBe(state);
  });

  test('⌘⏎ zooms the active tab so only the focused pane is visible', () => {
    const state = run(
      threePanes(),
      { type: 'command', id: 'pane.next' },
      { type: 'command', id: 'pane.zoom' },
    );
    expect(visiblePaneIds(state.tabs[0])).toEqual([2]);
    const back = run(state, { type: 'command', id: 'pane.zoom' });
    expect(visiblePaneIds(back.tabs[0])).toEqual([1, 2, 3]);
  });

  test('⌘J goes to the next pane that needs you across tabs, wrapping, and says so when none does', () => {
    const state = makeState(
      [
        makePane({ id: 1, status: 'waiting_input' }),
        makePane({ id: 2, position: 1 }),
        makePane({ id: 3, tab_id: 2, status: 'waiting_permission' }),
      ],
      [{ id: 1 }, { id: 2 }],
    );
    const a = run(state, { type: 'command', id: 'pane.nextWaiting' });
    expect([a.activeTabId, a.tabs[1]?.focus_pane_id]).toEqual([2, 3]);
    const b = run(a, { type: 'command', id: 'pane.nextWaiting' });
    expect([b.activeTabId, b.tabs[0]?.focus_pane_id]).toEqual([1, 1]);
    const none = run(threePanes(), { type: 'command', id: 'pane.nextWaiting' });
    expect(none.notice?.text).toBe('Nothing needs you');
  });

  test('⌘J skips every status but waiting_permission and waiting_input', () => {
    const others = ['starting', 'idle', 'running', 'exited', 'lost'] as const;
    const state = makeState([
      ...others.map((status, i) => makePane({ id: i + 1, position: i, status })),
      makePane({ id: 9, position: 5, status: 'waiting_input' }),
    ]);
    const a = run(state, { type: 'command', id: 'pane.nextWaiting' });
    expect(a.tabs[0]?.focus_pane_id).toBe(9);
    const alone = run(a, { type: 'command', id: 'pane.nextWaiting' });
    expect(alone.tabs[0]?.focus_pane_id).toBe(9);
  });

  test('a bell marks a pane until it is the one looked at; one on the focused pane marks nothing', () => {
    const state = threePanes();
    expect(run(state, { type: 'pane/bell', paneId: 1 })).toBe(state);
    const rung = run(state, { type: 'pane/bell', paneId: 2 });
    expect(rung.panes[2]?.bell).toBe(true);
    const seen = run(rung, { type: 'pane/focus', paneId: 2 });
    expect(seen.panes[2]?.bell).toBeUndefined();
  });

  test('an EXIT from the terminal marks the pane exited once, as pane.exit does', () => {
    const state = run(
      threePanes(),
      evt({
        e: 'pane.status',
        p: { pane_id: 2, status: 'waiting_input', detail: 'Question', at: 5 },
      }),
      { type: 'pane/exited', paneId: 2, code: 3, at: 9 },
    );
    expect(state.panes[2]).toMatchObject({ status: 'exited', exit_code: 3, statusSince: 9 });
    expect(state.panes[2]?.detail).toBeUndefined();
    const again = run(state, { type: 'pane/exited', paneId: 2, code: 3, at: 12 });
    expect(again.panes[2]?.statusSince).toBe(9);
  });

  test('⌘⇧W asks before closing a live pane and leaves an exited one to effects', () => {
    expect(run(threePanes(), { type: 'command', id: 'pane.close' }).overlay).toEqual({
      kind: 'close-confirm',
      paneId: 1,
    });
    const exited = makeState([makePane({ id: 1, status: 'exited', exit_code: 0 })]);
    expect(run(exited, { type: 'command', id: 'pane.close' }).overlay).toBeNull();
    const confirmed = run(
      threePanes(),
      { type: 'command', id: 'pane.close' },
      { type: 'pane/closeConfirmed', paneId: 1 },
    );
    expect(confirmed.overlay).toBeNull();
  });

  test('⌘= ⌘- ⌘0 step the font size within its range', () => {
    let state = threePanes();
    state = run(state, { type: 'command', id: 'font.up' }, { type: 'command', id: 'font.up' });
    expect(state.settings.font_size).toBe(14.5);
    state = run(state, { type: 'command', id: 'font.reset' });
    expect(state.settings.font_size).toBe(12.5);
    for (let i = 0; i < 20; i++) state = run(state, { type: 'command', id: 'font.down' });
    expect(state.settings.font_size).toBe(9.5);
  });

  test('⌘N opens the form for this tab, or for a new tab when there is none; ⌘T always for a tab', () => {
    expect(run(threePanes(), { type: 'command', id: 'pane.new' }).overlay).toEqual({
      kind: 'new-pane',
      target: 'pane',
    });
    expect(run(makeState([], []), { type: 'command', id: 'pane.new' }).overlay).toEqual({
      kind: 'new-pane',
      target: 'tab',
    });
    expect(run(threePanes(), { type: 'command', id: 'tab.new' }).overlay).toEqual({
      kind: 'new-pane',
      target: 'tab',
    });
  });
});

describe('pane creation', () => {
  test('pane/created focuses the new pane, activates its tab and closes the form', () => {
    const open = run(
      threePanes(),
      { type: 'command', id: 'tab.new' },
      { type: 'pane/create', request: { target: 'tab', cli: 'codex', cwd: '/Users/example' } },
    );
    expect(open.create.pending).toBe(true);
    const done = run(open, {
      type: 'pane/created',
      pane: makePane({ id: 8, tab_id: 5, cli: 'codex' }),
    });
    expect(done.activeTabId).toBe(5);
    expect(done.tabs.find((t) => t.id === 5)?.focus_pane_id).toBe(8);
    expect(done.overlay).toBeNull();
    expect(done.create).toEqual({ pending: false, error: null });
  });

  test('pane/createFailed keeps the form open with the error', () => {
    const state = run(
      threePanes(),
      { type: 'command', id: 'pane.new' },
      { type: 'pane/create', request: { target: 'pane', cli: 'claude', cwd: '/x' } },
      { type: 'pane/createFailed', message: 'claude is not on PATH' },
    );
    expect(state.overlay?.kind).toBe('new-pane');
    expect(state.create).toEqual({ pending: false, error: 'claude is not on PATH' });
  });

  test('a pane.create answer never undoes the events that overtook it', () => {
    const added = run(
      threePanes(),
      evt({
        e: 'pane.added',
        p: makePane({ id: 8, tab_id: 5, cli: 'claude', status: 'starting' }),
      }),
      evt({
        e: 'pane.status',
        p: { pane_id: 8, status: 'waiting_permission', detail: 'Write a.txt', at: 20 },
      }),
      evt({ e: 'pane.progress', p: { pane_id: 8, progress: { done: 1, total: 3 } } }),
    );
    const done = run(added, {
      type: 'pane/created',
      pane: makePane({ id: 8, tab_id: 5, cli: 'claude', status: 'starting', title: 'fix it' }),
    });
    expect(done.panes[8]).toMatchObject({
      status: 'waiting_permission',
      detail: 'Write a.txt',
      progress: { done: 1, total: 3 },
      title: 'fix it',
    });
    expect(done.tabs.find((t) => t.id === 5)?.pane_ids).toEqual([8]);
    expect(done.activeTabId).toBe(5);
  });
});

describe('four panes per tab (R56)', () => {
  const full = () =>
    makeState([0, 1, 2, 3].map((i) => makePane({ id: i + 1, position: i, cli: 'shell' })));

  test('a tab of four is full; ⌘N and ⌘D say so and open nothing, ⌘T still opens a tab', () => {
    const state = full();
    expect(isTabFull(state.tabs[0])).toBe(true);
    expect(isTabFull({ pane_ids: [1, 2, 3] })).toBe(false);
    expect(isTabFull(undefined)).toBe(false);
    for (const id of ['pane.new', 'pane.terminalHere'] as const) {
      const next = run(state, { type: 'command', id });
      expect(next.overlay).toBeNull();
      expect(next.notice?.text).toBe(FULL_TAB_NOTICE);
    }
    expect(run(state, { type: 'command', id: 'tab.new' }).overlay).toEqual({
      kind: 'new-pane',
      target: 'tab',
    });
    expect(FULL_TAB_NOTICE).toBe('This tab has 4 panes — ⌘T opens a new tab');
  });
});

describe('session resume', () => {
  test('pane/resumed keeps the pane in place and its status from the events, taking cli and title', () => {
    const lost = makeState([
      makePane({ id: 1, status: 'lost', terminalTitle: 'fix it' }),
      makePane({ id: 2, position: 1 }),
    ]);
    const next = run(
      lost,
      { type: 'pane/resume', paneId: 1 },
      evt({ e: 'pane.status', p: { pane_id: 1, status: 'starting', at: 20 } }),
      evt({ e: 'pane.status', p: { pane_id: 1, status: 'idle', at: 21 } }),
      {
        type: 'pane/resumed',
        pane: makePane({ id: 1, status: 'starting', cli: 'shell', title: 'zsh' }),
      },
    );
    expect(next.panes[1]).toMatchObject({ status: 'idle', cli: 'shell', title: 'zsh' });
    expect(next.panes[1]?.terminalTitle).toBe('fix it');
    expect(next.tabs[0]?.pane_ids).toEqual([1, 2]);
    expect(next.tabs[0]?.focus_pane_id).toBe(1);
  });

  test('a lost pane resumes with the CLI when it has a session id, else as a fresh shell', () => {
    expect(isLost(makePane({ id: 1, status: 'lost' }))).toBe(true);
    expect(isLost(makePane({ id: 1, status: 'exited' }))).toBe(false);
    expect(resumeHow(makePane({ id: 1, session_ref: 'x' }))).toBe('claude --resume');
    expect(resumeHow(makePane({ id: 1, cli: 'codex', session_ref: 'x' }))).toBe('codex resume');
    expect(resumeHow(makePane({ id: 1 }))).toBe('a fresh shell');
    expect(resumeHow(makePane({ id: 1, cli: 'shell' }))).toBe('a fresh shell');
  });
});

describe('notices and settings', () => {
  test('a delayed clear only removes the notice it was scheduled for', () => {
    const a = run(
      threePanes(),
      { type: 'notice/show', text: 'one' },
      { type: 'notice/show', text: 'two' },
    );
    expect(a.notice).toEqual({ id: 2, text: 'two' });
    expect(run(a, { type: 'notice/clear', id: 1 }).notice?.text).toBe('two');
    expect(run(a, { type: 'notice/clear', id: 2 }).notice).toBeNull();
  });
});

describe('selectors', () => {
  test('status presentation follows spec 6.3', () => {
    const now = 1_000;
    expect(
      statusView(makePane({ id: 1, status: 'running', statusSince: 748 }), 'zsh', now),
    ).toEqual({
      tone: 'running',
      label: 'Running 04:12',
      pulse: true,
    });
    expect(statusView(makePane({ id: 1, status: 'running' }), 'zsh', now).label).toBe('Running');
    expect(statusView(makePane({ id: 1, status: 'waiting_input' }), 'zsh', now).label).toBe(
      'Needs you',
    );
    expect(statusView(makePane({ id: 1, progress: { done: 2, total: 2 } }), 'zsh', now).label).toBe(
      'Done',
    );
    expect(statusView(makePane({ id: 1, progress: { done: 1, total: 2 } }), 'zsh', now).label).toBe(
      'Your turn',
    );
    expect(statusView(makePane({ id: 1, cli: 'shell' }), 'zsh', now).label).toBe('zsh');
    expect(statusView(makePane({ id: 1, status: 'exited', exit_code: 1 }), 'zsh', now).tone).toBe(
      'failed',
    );
    expect(formatElapsed(3_725)).toBe('1:02:05');
  });

  test('tab dots, counts and the waiting order', () => {
    const state = makeState(
      [
        makePane({ id: 1, status: 'running' }),
        makePane({ id: 2, position: 1, cli: 'codex', status: 'waiting_permission' }),
        makePane({ id: 3, tab_id: 2, cli: 'shell' }),
      ],
      [{ id: 1 }, { id: 2 }],
    );
    const [t1, t2] = state.tabs;
    expect(t1 && tabDot(state, t1)).toBe('waiting');
    expect(t2 && tabDot(state, t2)).toBe('done');
    expect(selectWaitingCount(state)).toBe(1);
    expect(selectCliCounts(state)).toEqual({ claude: 1, codex: 1, shell: 1 });
    expect(nextWaitingPane(state)).toBe(2);
  });
});
