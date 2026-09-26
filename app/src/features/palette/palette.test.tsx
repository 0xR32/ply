import { describe, expect, test } from 'bun:test';
import { hasNativeTestRenderer } from '@gpuix/react/testing';
import type { Action } from '../../state/actions';
import { type PaneState, reduce } from '../../state/reducer';
import { makePane, makeState, makeTask, mountWithStore } from '../../state/test-support';
import { matches, paletteItems } from './commands';
import { Palette } from './palette';

const state = (waiting = true) =>
  makeState(
    [
      makePane({ id: 1, title: 'Tab bar polish', status: 'running' }),
      makePane({
        id: 2,
        position: 1,
        title: 'Review the guides',
        status: waiting ? 'waiting_permission' : 'idle',
      }),
      makePane({
        id: 3,
        tab_id: 2,
        cli: 'codex',
        title: 'Merges',
        cwd: '/Users/example/code/notes',
      }),
    ],
    [
      { id: 1, name: 'ply' },
      { id: 2, name: 'notes' },
    ],
    { overlay: { kind: 'palette' } },
  );

describe('palette commands', () => {
  test('with no query it lists the commands of the canvas first, with their keys and hints', () => {
    const items = paletteItems(state(), '', 0);
    expect(items.slice(0, 6).map((i) => [i.label, i.keys])).toEqual([
      ['New pane', '⌘N'],
      ['New tab', '⌘T'],
      ['Go to what needs you', '⌘J'],
      ['Terminal here', '⌘D'],
      ['Zoom pane', '⌘⏎'],
      ['Next pane', '⌘]'],
    ]);
    expect(items[2]?.hint).toBe('tab 1 · pane 2 · permission');
    expect(items[3]?.hint).toBe('pane 1 in ~/code/ply');
    expect(items.every((i) => i.section === 'Commands')).toBe(true);
    expect(paletteItems(state(false), '', 0).some((i) => i.id === 'pane.nextWaiting')).toBe(false);
  });

  test('the task queue has its commands, and a pane with queued tasks can be paused or resumed', () => {
    const withTasks = reduce(state(), {
      type: 'tasks/loaded',
      list: {
        tasks: [makeTask({ id: 1, pane_id: 1 }), makeTask({ id: 2, pane_id: 3 })],
        queues: [{ pane_id: 3, paused: 'restored' }],
      },
    });
    const items = paletteItems(withTasks, '', 0);
    const find = (id: string) => items.find((i) => i.id === id);
    expect([find('task.dispatch')?.label, find('task.dispatch')?.keys]).toEqual([
      'Dispatch a task',
      '⌘E',
    ]);
    expect([find('task.queue')?.label, find('task.queue')?.keys]).toEqual([
      'Show task queue',
      '⌘⇧E',
    ]);
    expect(find('queue-pause-1')?.label).toBe('Pause queue of pane 1');
    expect(find('queue-pause-1')?.actions).toEqual([
      { type: 'overlay/close' },
      { type: 'queue/pause', paneId: 1, paused: true },
    ]);
    expect(find('queue-pause-3')?.label).toBe('Resume queue of pane 1');
    expect(find('queue-pause-3')?.hint).toContain('held after plyd restarted');
    expect(find('queue-pause-2')).toBeUndefined();
  });

  test('every lost pane gets a resume command saying how it resumes', () => {
    const lost = state();
    const panes = {
      ...lost.panes,
      3: { ...(lost.panes[3] as PaneState), status: 'lost' as const, session_ref: 'example' },
    };
    const items = paletteItems({ ...lost, panes }, '', 0).filter((i) =>
      i.id.startsWith('pane-resume'),
    );
    expect(items.map((i) => [i.label, i.hint])).toEqual([
      ['Resume Merges', 'tab 2 · pane 1 · codex resume'],
    ]);
    expect(items[0]?.actions).toEqual([
      { type: 'overlay/close' },
      { type: 'pane/resume', paneId: 3 },
    ]);
    expect(paletteItems(lost, 'resume', 0)).toEqual([]);
  });

  test('plyd can be restarted, or stopped with every session after a confirmation (R53)', () => {
    const find = (s: ReturnType<typeof state>, id: string) =>
      paletteItems(s, 'plyd', 0).find((i) => i.id === id);
    const restart = find(state(), 'daemon-restart');
    expect(restart?.label).toBe('Restart plyd');
    expect(restart?.actions).toEqual([{ type: 'overlay/close' }, { type: 'daemon/restart' }]);
    expect(restart?.hint).not.toContain('another build');
    const quit = paletteItems(state(), 'quit', 0);
    expect(quit.map((i) => i.label)).toEqual(['Quit ply and stop sessions']);
    expect(quit[0]?.actions).toEqual([{ type: 'overlay/open', overlay: { kind: 'quit-confirm' } }]);
    const base = state();
    const foreign = { ...base, env: { ...base.env, buildId: '0.1.0+0a1b2c3d4e5f' } };
    expect(find(foreign, 'daemon-restart')).toMatchObject({
      dot: 'amber',
      hint: 'plyd is from another build, rebuild it first; starts the build in target/, running sessions come back lost',
    });
  });

  test('in a full tab New pane and Terminal here show as unavailable and only explain why', () => {
    const base = state();
    const four = makeState(
      [0, 1, 2, 3].map((i) => makePane({ id: i + 1, position: i })),
      [{ id: 1, name: 'ply' }],
      { overlay: { kind: 'palette' } },
    );
    const items = paletteItems(four, '', 0);
    for (const id of ['pane.new', 'pane.terminalHere'] as const) {
      const item = items.find((i) => i.id === id);
      expect(item).toMatchObject({ unavailable: true, dot: 'dim' });
      expect(item?.hint).toContain('This tab has 4 panes');
      expect(item?.actions).toEqual([{ type: 'overlay/close' }, { type: 'command', id }]);
    }
    expect(items.find((i) => i.id === 'tab.new')?.unavailable).toBeUndefined();
    expect(paletteItems(base, '', 0).find((i) => i.id === 'pane.new')?.unavailable).toBeUndefined();
  });

  test('a query filters commands, tabs and panes by every word', () => {
    const items = paletteItems(state(), 'notes', 0);
    expect(items.map((i) => [i.section, i.label])).toEqual([
      ['Tabs', 'notes'],
      ['Panes', 'Merges'],
    ]);
    expect(paletteItems(state(), 'next pane', 0).map((i) => i.id)).toEqual(['pane.next']);
    const tab = items[0];
    expect(tab && matches(tab, 'NOTES tab 2')).toBe(true);
    expect(tab && matches(tab, 'notes tab 3')).toBe(false);
  });
});

describe.if(hasNativeTestRenderer)('Palette', () => {
  function mount(waiting = true) {
    const m = mountWithStore(<Palette />, state(waiting), { width: 1000, height: 700 });
    const seen: Action[] = [];
    m.store.addEffect((a) => seen.push(a));
    return { ...m, seen };
  }

  test('focuses its input, and ↓ ↓ ⏎ runs the third row', () => {
    const { renderer, seen, unmount } = mount();
    try {
      const input = renderer.findByTestId('palette-input');
      expect(renderer.getFocusedElementId()).toBe(input?.id ?? -1);
      renderer.simulateKeystrokes('down');
      renderer.simulateKeystrokes('down');
      renderer.simulateKeystrokes('enter');
      expect(seen).toEqual([
        { type: 'overlay/close' },
        { type: 'command', id: 'pane.nextWaiting' },
      ]);
    } finally {
      unmount();
    }
  });

  test('↑ from the first row wraps to the last, and typing filters to what matches', () => {
    const { renderer, seen, unmount } = mount();
    try {
      for (const k of ['m', 'e', 'r', 'g']) renderer.simulateKeystrokes(k);
      renderer.flush();
      expect(renderer.getAllText()).toContain('Panes');
      expect(renderer.getAllText()).toContain('Merges');
      renderer.simulateKeystrokes('up');
      renderer.simulateKeystrokes('enter');
      expect(seen).toEqual([{ type: 'overlay/close' }, { type: 'pane/focus', paneId: 3 }]);
    } finally {
      unmount();
    }
  });

  test('a digit on an empty query jumps to that tab; esc closes', () => {
    const jump = mount();
    try {
      jump.renderer.simulateKeystrokes('2');
      expect(jump.seen).toEqual([{ type: 'overlay/close' }, { type: 'tab/select', tabId: 2 }]);
    } finally {
      jump.unmount();
    }
    const esc = mount();
    try {
      esc.renderer.simulateKeystrokes('escape');
      expect(esc.seen).toEqual([{ type: 'overlay/close' }]);
    } finally {
      esc.unmount();
    }
  });

  test('↑↓ keep the highlighted row inside the list, scrolling it when it would leave', () => {
    const { renderer, unmount } = mount();
    try {
      const list = renderer.findByTestId('palette-list');
      const view = list ? renderer.getElementBounds(list.id) : null;
      if (!view) throw new Error('no palette list');
      const highlighted = () => {
        const on = renderer
          .findByType('div')
          .find((el) => el.testId?.startsWith('palette-item-') && el.style.borderWidth === 1);
        return on ? renderer.getElementBounds(on.id) : null;
      };
      const rows = paletteItems(state(), '', 0).length;
      for (let step = 0; step < rows + 2; step++) {
        renderer.simulateKeystrokes(step < rows ? 'down' : 'up');
        renderer.flush();
        renderer.flush();
        const b = highlighted();
        expect(b).not.toBeNull();
        if (!b) return;
        expect(b.y).toBeGreaterThanOrEqual(view.y - 0.5);
        expect(b.y + b.height).toBeLessThanOrEqual(view.y + view.height + 0.5);
      }
    } finally {
      unmount();
    }
  });

  test('a click runs a row', () => {
    const { renderer, seen, unmount } = mount();
    try {
      const row = renderer.findByTestId('palette-item-tab.new');
      const b = row && renderer.getElementBounds(row.id);
      if (!b) throw new Error('row did not paint');
      renderer.nativeSimulateClick(b.x + 40, b.y + b.height / 2);
      expect(seen).toEqual([{ type: 'overlay/close' }, { type: 'command', id: 'tab.new' }]);
    } finally {
      unmount();
    }
  });
});
