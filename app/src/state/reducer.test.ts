import { describe, expect, test } from 'bun:test';
import type { Action, Event, Skill, Task } from './actions';
import { type AppState, type PaneState, reduce } from './reducer';
import {
  FULL_TAB_NOTICE,
  gridShape,
  isLost,
  isTabFull,
  matchSkills,
  nextWaitingPane,
  paneNeighbour,
  queueStripView,
  resumeHow,
  selectActiveTask,
  selectCliCounts,
  selectPaneQueue,
  selectQueueCounts,
  selectWaitingCount,
  statusView,
  tabDot,
  taskView,
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

  test("pane.meta carries git's project and linked worktree, kept in the same directory until git answers", () => {
    const cwd = '/Users/example/code/ply/crates';
    const a = run(
      threePanes(),
      evt({
        e: 'pane.meta',
        p: { pane_id: 1, cwd, branch: 'main', project: 'ply', git_worktree: 'ply-side' },
      }),
    );
    expect(a.panes[1]).toMatchObject({ project: 'ply', git_worktree: 'ply-side' });
    const b = run(a, evt({ e: 'pane.meta', p: { pane_id: 1, cwd, model: 'claude-opus-5' } }));
    expect(b.panes[1]).toMatchObject({ project: 'ply', git_worktree: 'ply-side', branch: 'main' });
    const c = run(b, evt({ e: 'pane.meta', p: { pane_id: 1, cwd: '/tmp' } }));
    expect(c.panes[1]?.project).toBeUndefined();
    expect(c.panes[1]?.git_worktree).toBeUndefined();
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
    expect(statusView(makePane({ id: 1, status: 'running', statusSince: 748 }), 'zsh')).toEqual({
      tone: 'running',
      label: 'Running',
    });
    expect(statusView(makePane({ id: 1, status: 'running' }), 'zsh').label).toBe('Running');
    expect(statusView(makePane({ id: 1, status: 'waiting_input' }), 'zsh').label).toBe('Needs you');
    expect(statusView(makePane({ id: 1, progress: { done: 2, total: 2 } }), 'zsh').label).toBe(
      'Done',
    );
    expect(statusView(makePane({ id: 1, progress: { done: 1, total: 2 } }), 'zsh').label).toBe(
      'Your turn',
    );
    expect(statusView(makePane({ id: 1, cli: 'shell' }), 'zsh').label).toBe('zsh');
    expect(statusView(makePane({ id: 1, status: 'exited', exit_code: 1 }), 'zsh').tone).toBe(
      'failed',
    );
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

  test('gridShape lays out ≤3 panes as one row and 4 as quadrants (R56)', () => {
    expect(gridShape(1)).toEqual({ columns: 1, rows: 1 });
    expect(gridShape(2)).toEqual({ columns: 2, rows: 1 });
    expect(gridShape(3)).toEqual({ columns: 3, rows: 1 });
    expect(gridShape(4)).toEqual({ columns: 2, rows: 2 });
  });

  test('paneNeighbour moves spatially with no wrap-around, in every layout (R58)', () => {
    const tab = (pane_ids: number[]) => ({ pane_ids });
    for (const dir of ['left', 'right', 'up', 'down'] as const) {
      expect(paneNeighbour(tab([1]), 1, dir)).toBeUndefined();
    }
    expect(paneNeighbour(tab([1, 2]), 1, 'right')).toBe(2);
    expect(paneNeighbour(tab([1, 2]), 2, 'left')).toBe(1);
    expect(paneNeighbour(tab([1, 2]), 1, 'left')).toBeUndefined();
    expect(paneNeighbour(tab([1, 2]), 2, 'right')).toBeUndefined();
    expect(paneNeighbour(tab([1, 2]), 1, 'up')).toBeUndefined();
    expect(paneNeighbour(tab([1, 2]), 1, 'down')).toBeUndefined();
    expect(paneNeighbour(tab([1, 2, 3]), 2, 'left')).toBe(1);
    expect(paneNeighbour(tab([1, 2, 3]), 2, 'right')).toBe(3);
    expect(paneNeighbour(tab([1, 2, 3]), 3, 'right')).toBeUndefined();
    expect(paneNeighbour(tab([1, 2, 3]), 1, 'up')).toBeUndefined();
    // 1 top-left, 2 top-right, 3 bottom-left, 4 bottom-right
    const q = tab([1, 2, 3, 4]);
    expect(paneNeighbour(q, 1, 'right')).toBe(2);
    expect(paneNeighbour(q, 1, 'down')).toBe(3);
    expect(paneNeighbour(q, 2, 'left')).toBe(1);
    expect(paneNeighbour(q, 2, 'right')).toBeUndefined();
    expect(paneNeighbour(q, 2, 'down')).toBe(4);
    expect(paneNeighbour(q, 3, 'up')).toBe(1);
    expect(paneNeighbour(q, 3, 'right')).toBe(4);
    expect(paneNeighbour(q, 4, 'left')).toBe(3);
    expect(paneNeighbour(q, 4, 'up')).toBe(2);
    expect(paneNeighbour(q, 4, 'right')).toBeUndefined();
    expect(paneNeighbour(q, 4, 'down')).toBeUndefined();
    expect(paneNeighbour(q, 99, 'left')).toBeUndefined();
    expect(paneNeighbour(undefined, 1, 'left')).toBeUndefined();
  });
});

describe('⌘← ⌘→ ⌘↑ ⌘↓ (R58)', () => {
  test('move focus to the spatial neighbour, do nothing at an edge, and the zoom follows', () => {
    const four = () =>
      makeState([0, 1, 2, 3].map((i) => makePane({ id: i + 1, position: i, cli: 'shell' })));
    expect(run(four(), { type: 'command', id: 'pane.left' }).tabs[0]?.focus_pane_id).toBe(1);
    const right = run(four(), { type: 'command', id: 'pane.right' });
    expect(right.tabs[0]?.focus_pane_id).toBe(2);
    const down = run(right, { type: 'command', id: 'pane.down' });
    expect(down.tabs[0]?.focus_pane_id).toBe(4);
    const zoomed = run(four(), { type: 'command', id: 'pane.zoom' });
    expect(zoomed.tabs[0]?.zoomed).toBe(true);
    const zoomedMoved = run(zoomed, { type: 'command', id: 'pane.right' });
    expect(zoomedMoved.tabs[0]).toMatchObject({ zoomed: true, focus_pane_id: 2 });
    expect(visiblePaneIds(zoomedMoved.tabs[0])).toEqual([2]);
  });

  test('one pane or no spatial neighbour: the key does nothing', () => {
    const one = makeState([makePane({ id: 1 })]);
    expect(run(one, { type: 'command', id: 'pane.right' })).toBe(one);
    const two = makeState([makePane({ id: 1 }), makePane({ id: 2, position: 1 })]);
    expect(run(two, { type: 'command', id: 'pane.up' })).toBe(two);
    expect(run(two, { type: 'command', id: 'pane.right' }).tabs[0]?.focus_pane_id).toBe(2);
  });
});

describe('the usage view (R59)', () => {
  const usage = {
    claude: {
      as_of: 1_790_350_000,
      windows: [{ label: 'Session · 5h', window_minutes: 300, used_percent: 81, models: [] }],
    },
  };

  test('⌘U shows it, the release hides it, and the last answer stays for the next hold', () => {
    const shown = run(makeState([]), { type: 'command', id: 'usage.show' });
    expect(shown.usage).toEqual({ shown: true, usage: null, error: null });
    expect(run(shown, { type: 'command', id: 'usage.show' })).toBe(shown);
    const loaded = run(shown, { type: 'usage/loaded', usage }, { type: 'usage/hide' });
    expect(loaded.usage).toEqual({ shown: false, usage, error: null });
    expect(run(loaded, { type: 'usage/hide' })).toBe(loaded);
  });

  test('a failure keeps the last answer until a new one clears it', () => {
    const failed = run(
      makeState([]),
      { type: 'usage/loaded', usage },
      { type: 'usage/failed', message: 'timeout' },
    );
    expect(failed.usage).toMatchObject({ usage, error: 'timeout' });
    expect(run(failed, { type: 'usage/loaded', usage: {} }).usage).toMatchObject({
      usage: {},
      error: null,
    });
  });

  test('the key-repeat timing is kept as far as macOS sets it', () => {
    const next = run(makeState([]), { type: 'env/keyRepeat', value: { intervalMs: 30 } });
    expect(next.env.keyRepeat).toEqual({ intervalMs: 30 });
  });
});

describe('task queue (R60)', () => {
  const task = (fields: Partial<Task> & { id: number }): Task => ({
    workspace_id: testWorkspace.id,
    pane_id: 1,
    text: `task ${fields.id}`,
    state: 'queued',
    position: 0,
    created_at: 1_790_000_000,
    ...fields,
  });

  test('task.changed adds and replaces tasks of this workspace; queue.changed sets and clears flags', () => {
    let state = run(
      threePanes(),
      evt({ e: 'task.changed', p: task({ id: 7 }) }),
      evt({ e: 'task.changed', p: task({ id: 8, workspace_id: 99 }) }),
    );
    expect(Object.keys(state.tasks.tasks)).toEqual(['7']);
    state = run(state, evt({ e: 'task.changed', p: task({ id: 7, state: 'running' }) }));
    expect(state.tasks.tasks[7]?.state).toBe('running');
    state = run(state, evt({ e: 'queue.changed', p: { pane_id: 1, paused: 'user' } }));
    expect(state.tasks.queues[1]).toEqual({ pane_id: 1, paused: 'user' });
    state = run(state, evt({ e: 'queue.changed', p: { pane_id: 1 } }));
    expect(state.tasks.queues[1]).toBeUndefined();
  });

  test('tasks/loaded replaces the queue; a plyd without task.list leaves it unavailable', () => {
    const loaded = run(threePanes(), {
      type: 'tasks/loaded',
      list: {
        tasks: [task({ id: 1 }), task({ id: 2, position: 1 })],
        queues: [{ pane_id: 1, blocked: 'typing' }],
      },
    });
    expect(loaded.tasks.available).toBe(true);
    expect(Object.keys(loaded.tasks.tasks)).toEqual(['1', '2']);
    expect(loaded.tasks.queues[1]?.blocked).toBe('typing');
    const old = run(threePanes(), { type: 'tasks/loaded', list: null });
    expect(old.tasks.available).toBe(false);
  });

  test('⌘E opens the dispatch form on the focused agent pane and ⌘⇧E the queue', () => {
    const state = run(
      threePanes(),
      { type: 'pane/focus', paneId: 2 },
      { type: 'command', id: 'task.dispatch' },
    );
    expect(state.overlay).toEqual({ kind: 'dispatch', paneId: 2 });
    const onShell = run(
      threePanes(),
      { type: 'pane/focus', paneId: 3 },
      { type: 'command', id: 'task.dispatch' },
    );
    expect(onShell.overlay).toEqual({ kind: 'dispatch' });
    expect(run(threePanes(), { type: 'command', id: 'task.queue' }).overlay).toEqual({
      kind: 'queue',
    });
  });

  test('task/add waits for the answer; task/added closes the form; task/addFailed keeps it with the error', () => {
    let state = run(threePanes(), { type: 'command', id: 'task.dispatch' });
    state = run(state, {
      type: 'task/add',
      target: { kind: 'pane', paneId: 1 },
      text: '/review-pr',
    });
    expect(state.taskForm).toEqual({ pending: true, error: null });
    const failed = run(state, {
      type: 'task/addFailed',
      message: 'the queue already holds 32 tasks',
    });
    expect(failed.overlay?.kind).toBe('dispatch');
    expect(failed.taskForm).toEqual({ pending: false, error: 'the queue already holds 32 tasks' });
    const added = run(state, { type: 'task/added', task: task({ id: 9 }) });
    expect(added.overlay).toBeNull();
    expect(added.tasks.tasks[9]?.id).toBe(9);
  });

  test('skills are kept for the CLI and directory they were asked for', () => {
    let state = run(threePanes(), {
      type: 'skills/query',
      cli: 'claude',
      cwd: '/Users/example/code/ply',
    });
    expect(state.skills).toMatchObject({ key: 'claude /Users/example/code/ply', loading: true });
    const skill = { name: 'review-pr', invocation: '/review-pr', source: 'user' as const };
    const stale = run(state, {
      type: 'skills/loaded',
      key: 'codex /Users/example',
      list: { skills: [skill] },
    });
    expect(stale.skills.list).toEqual([]);
    state = run(state, {
      type: 'skills/loaded',
      key: 'claude /Users/example/code/ply',
      list: { skills: [skill] },
    });
    expect(state.skills).toMatchObject({ loading: false, list: [skill], error: null });
    state = run(state, {
      type: 'skills/failed',
      key: 'claude /Users/example/code/ply',
      message: 'no plyd',
    });
    expect(state.skills.error).toBe('no plyd');
  });

  test('selectors: a pane queue in order, counts, and what a task and a strip show', () => {
    let state = run(
      threePanes(),
      {
        type: 'tasks/loaded',
        list: {
          tasks: [
            task({ id: 1, state: 'running', position: 0 }),
            task({ id: 3, position: 1 }),
            task({ id: 2, position: 0 }),
            task({ id: 4, pane_id: 2 }),
            task({ id: 5, pane_id: undefined, pool: { cli: 'claude', cwd: '/Users/example' } }),
            task({ id: 6, state: 'ended', ended_at: 1_790_000_100 }),
          ],
          queues: [{ pane_id: 2, paused: 'restored' }],
        },
      },
      evt({ e: 'pane.status', p: { pane_id: 1, status: 'waiting_permission', at: 5 } }),
    );
    expect(selectPaneQueue(state, 1).map((t) => t.id)).toEqual([2, 3]);
    expect(selectActiveTask(state, 1)?.id).toBe(1);
    expect(selectQueueCounts(state)).toEqual({ queued: 4, held: 1, running: 1, needsYou: 1 });
    const pane1 = state.panes[1] as PaneState;
    expect(taskView(state.tasks.tasks[1] as Task, pane1, undefined).tone).toBe('waiting');
    expect(taskView(state.tasks.tasks[2] as Task, pane1, undefined).label).toBe('next');
    expect(taskView(state.tasks.tasks[3] as Task, pane1, undefined).label).toBe('2nd');
    expect(
      taskView(state.tasks.tasks[4] as Task, state.panes[2], { pane_id: 2, paused: 'restored' })
        .label,
    ).toBe('held');
    expect(taskView(state.tasks.tasks[5] as Task, undefined, undefined).label).toBe(
      'waits for a free pane',
    );
    expect(queueStripView(state, 2)).toMatchObject({ kind: 'restored', count: 1 });
    expect(queueStripView(state, 1)).toBeNull();
    state = run(state, evt({ e: 'queue.changed', p: { pane_id: 1, blocked: 'typing' } }));
    expect(queueStripView(state, 1)).toMatchObject({ kind: 'typing', next: { id: 2 } });
    state = run(state, evt({ e: 'queue.changed', p: { pane_id: 1, blocked: 'startup' } }));
    expect(queueStripView(state, 1)).toMatchObject({ kind: 'startup', next: { id: 2 } });
    expect(
      taskView(state.tasks.tasks[2] as Task, pane1, { pane_id: 1, blocked: 'startup' }).label,
    ).toBe("waits for the CLI's prompt");
  });

  test('matchSkills finds a query in the name or the invocation, ignoring case, never in the description', () => {
    const skills: Skill[] = [
      { name: 'review-pr', invocation: '/review-pr', description: 'Full review', source: 'user' },
      {
        name: 'brainstorming',
        invocation: '/superpowers:brainstorming',
        source: 'plugin',
        plugin: 'superpowers',
      },
      {
        name: 'audit',
        invocation: '$audit',
        description: 'Security AUDIT of a diff',
        source: 'user',
      },
    ];
    expect(matchSkills(skills, 'REVIEW').map((s) => s.name)).toEqual(['review-pr']);
    expect(matchSkills(skills, 'audit').map((s) => s.name)).toEqual(['audit']);
    expect(matchSkills(skills, '$aud').map((s) => s.name)).toEqual(['audit']);
    expect(matchSkills(skills, 'superpowers').map((s) => s.name)).toEqual(['brainstorming']);
    expect(matchSkills(skills, 'diff')).toEqual([]);
    expect(matchSkills(skills, 'full')).toEqual([]);
    expect(matchSkills(skills, '  ')).toHaveLength(3);
  });
});
