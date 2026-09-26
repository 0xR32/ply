import { describe, expect, test } from 'bun:test';
import { hasNativeTestRenderer, type TestRenderer } from '@gpuix/react/testing';
import type { Action } from '../../state/actions';
import { type AppState, reduce } from '../../state/reducer';
import { makePane, makeState, makeTask, mountWithStore } from '../../state/test-support';
import { QueueSheet } from './queue-sheet';

const CWD = '/Users/example/code/ply';

function loaded(): AppState {
  const base = makeState([
    makePane({ id: 1, cwd: CWD, status: 'running' }),
    makePane({ id: 2, position: 1, cli: 'codex', cwd: CWD }),
    makePane({ id: 3, position: 2, cwd: CWD, status: 'waiting_permission' }),
  ]);
  const withTasks = reduce(base, {
    type: 'tasks/loaded',
    list: {
      tasks: [
        makeTask({ id: 1, pane_id: 1, state: 'running', text: '/review-pr 212' }),
        makeTask({ id: 2, pane_id: 1, text: '/open-pr' }),
        makeTask({ id: 3, pane_id: 1, position: 1, text: '/triage' }),
        makeTask({ id: 4, pane_id: 2, text: '$audit' }),
        makeTask({ id: 5, pane_id: 3, state: 'running', text: '/superpowers:brainstorming' }),
        makeTask({ id: 6, pane_id: undefined, pool: { cli: 'claude', cwd: CWD }, text: '/perf' }),
        makeTask({
          id: 7,
          state: 'failed',
          detail: 'not submitted: no acknowledgement',
          ended_at: 5,
          text: '$e2e',
        }),
      ],
      queues: [{ pane_id: 2, paused: 'user' }],
    },
  });
  return reduce(withTasks, { type: 'command', id: 'task.queue' });
}

function mount(state: AppState = loaded()) {
  const m = mountWithStore(<QueueSheet />, state, { width: 900, height: 900 });
  const seen: Action[] = [];
  m.store.addEffect((a) => seen.push(a));
  const press = (...keys: string[]) => {
    for (const k of keys) {
      m.renderer.simulateKeystrokes(k);
      m.renderer.flush();
      m.renderer.dispatchNativeEvents();
    }
  };
  return { ...m, seen, press };
}

function textOf(renderer: TestRenderer, testId: string): string | undefined {
  const root = renderer.findByTestId(testId);
  if (!root) return undefined;
  const parts: string[] = [];
  const walk = (id: number) => {
    const el = renderer.getElement(id);
    if (!el) return;
    if (el.text) parts.push(el.text);
    for (const child of el.children) walk(child);
  };
  walk(root.id);
  return parts.join(' ');
}

function click(renderer: TestRenderer, testId: string): void {
  const el = renderer.findByTestId(testId);
  const b = el && renderer.getElementBounds(el.id);
  if (!b) throw new Error(`${testId} did not paint`);
  renderer.nativeSimulateClick(b.x + b.width / 2, b.y + b.height / 2);
  renderer.flush();
  renderer.dispatchNativeEvents();
  renderer.flush();
}

describe.if(hasNativeTestRenderer)('QueueSheet', () => {
  test('lists each pane’s queue, the pool and the history with what each task is doing', () => {
    const { renderer, unmount } = mount();
    try {
      expect(renderer.getAllText()).toContain('Task queue');
      expect(textOf(renderer, 'queue-subtitle')).toBe('4 queued · 2 running · 1 needs you');
      expect(textOf(renderer, 'queue-task-1')).toContain('running');
      expect(textOf(renderer, 'queue-task-2')).toContain('next');
      expect(textOf(renderer, 'queue-task-3')).toContain('2nd');
      expect(textOf(renderer, 'queue-task-4')).toContain('held');
      expect(textOf(renderer, 'queue-task-5')).toContain('needs you');
      expect(textOf(renderer, 'queue-task-6')).toContain('waits for a free pane');
      expect(textOf(renderer, 'queue-group-2')).toContain('Resume');
      expect(textOf(renderer, 'queue-finished')).toContain('1 not submitted');
      expect(renderer.findByTestId('queue-task-7')).toBeUndefined();
      click(renderer, 'queue-finished');
      expect(textOf(renderer, 'queue-task-7')).toContain('not submitted');
    } finally {
      unmount();
    }
  });

  test('the row and group buttons cancel, reorder, pause and resume', () => {
    const { renderer, seen, unmount } = mount();
    try {
      click(renderer, 'queue-task-3-up');
      expect(seen.at(-1)).toEqual({ type: 'task/move', taskId: 3, position: 0 });
      click(renderer, 'queue-task-2-cancel');
      expect(seen.at(-1)).toEqual({ type: 'task/cancel', taskId: 2 });
      click(renderer, 'queue-pause-1');
      expect(seen.at(-1)).toEqual({ type: 'queue/pause', paneId: 1, paused: true });
      click(renderer, 'queue-pause-2');
      expect(seen.at(-1)).toEqual({ type: 'queue/pause', paneId: 2, paused: false });
      expect(renderer.findByTestId('queue-task-1-cancel')).toBeUndefined();
    } finally {
      unmount();
    }
  });

  test('keys: ↑↓ select, ⌥↑↓ reorder, ⌫ cancel, p pauses the pane, ⏎ goes to it, esc closes', () => {
    const { seen, press, unmount } = mount();
    try {
      press('down', 'down', 'down');
      press('alt-up');
      expect(seen.at(-1)).toEqual({ type: 'task/move', taskId: 3, position: 0 });
      press('backspace');
      expect(seen.at(-1)).toEqual({ type: 'task/cancel', taskId: 3 });
      press('p');
      expect(seen.at(-1)).toEqual({ type: 'queue/pause', paneId: 1, paused: true });
      press('enter');
      expect(seen.slice(-2)).toEqual([
        { type: 'overlay/close' },
        { type: 'pane/focus', paneId: 1 },
      ]);
    } finally {
      unmount();
    }
  });

  test('an empty queue says how to add a task', () => {
    const { renderer, unmount } = mount(
      reduce(makeState([makePane({ id: 1 })]), { type: 'command', id: 'task.queue' }),
    );
    try {
      expect(renderer.getAllText()).toContain('Nothing queued');
    } finally {
      unmount();
    }
  });
});
