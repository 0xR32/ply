import { describe, expect, test } from 'bun:test';
import { flushSync } from '@gpuix/react';
import { hasNativeTestRenderer, type TestRenderer } from '@gpuix/react/testing';
import type { Action } from '../../state/actions';
import { type AppState, reduce } from '../../state/reducer';
import {
  makePane,
  makeState,
  makeTask,
  mountWithStore,
  testSkills,
} from '../../state/test-support';
import { DispatchForm } from './dispatch-form';

const CWD = '/Users/example/code/ply';

function loaded(): AppState {
  const base = makeState([
    makePane({ id: 1, cwd: CWD }),
    makePane({ id: 2, position: 1, cli: 'codex', cwd: CWD, status: 'running' }),
    makePane({ id: 3, position: 2, cli: 'shell', cwd: CWD }),
  ]);
  const withTasks = reduce(base, {
    type: 'tasks/loaded',
    list: { tasks: [makeTask({ id: 9, pane_id: 2 })], queues: [] },
  });
  const opened = reduce(withTasks, { type: 'command', id: 'task.dispatch' });
  const asked = reduce(opened, { type: 'skills/query', cli: 'claude', cwd: CWD });
  return reduce(asked, {
    type: 'skills/loaded',
    key: `claude ${CWD}`,
    list: { skills: testSkills },
  });
}

function mount(state: AppState = loaded()) {
  const overlay =
    state.overlay?.kind === 'dispatch' ? state.overlay : { kind: 'dispatch' as const };
  const m = mountWithStore(<DispatchForm paneId={overlay.paneId} />, state, {
    width: 900,
    height: 900,
  });
  const seen: Action[] = [];
  m.store.addEffect((a) => seen.push(a));
  const press = (...keys: string[]) => {
    for (const k of keys) {
      m.renderer.simulateKeystrokes(k);
      m.renderer.flush();
      m.renderer.dispatchNativeEvents();
    }
  };
  const type = (text: string) => press(...[...text].map((c) => (c === ' ' ? 'space' : c)));
  return { ...m, seen, press, type };
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
  return parts.join('');
}

function focusedTestId(renderer: TestRenderer): string | undefined {
  const id = renderer.getFocusedElementId();
  return id === null ? undefined : renderer.getElement(id)?.testId;
}

describe.if(hasNativeTestRenderer)('DispatchForm', () => {
  test('opens on the focused pane, lists its skills and ⌘⏎ queues the prompt there', () => {
    const { renderer, seen, press, type, unmount } = mount();
    try {
      const text = renderer.getAllText();
      for (const s of [
        'Dispatch a task',
        'This pane',
        'Next free pane',
        'New pane',
        '/review-pr',
        '/superpowers:brainstorming',
        'Plugins',
      ]) {
        expect(text).toContain(s);
      }
      expect(text).toContain('Typed into the pane exactly as written, when it is your turn.');
      expect(focusedTestId(renderer)).toBe('dispatch-search');
      press('tab');
      expect(focusedTestId(renderer)).toBe('dispatch-prompt');
      type('why is the test slow');
      press('cmd-enter');
      expect(seen.at(-1)).toEqual({
        type: 'task/add',
        target: { kind: 'pane', paneId: 1 },
        text: 'why is the test slow',
      });
    } finally {
      unmount();
    }
  });

  test('picking a skill starts the prompt with its invocation and sends the skill along', () => {
    const { renderer, seen, press, type, unmount } = mount();
    try {
      click(renderer, 'dispatch-skill-/review-pr');
      expect(renderer.getAllText()).toContain('arguments [PR number]');
      expect(focusedTestId(renderer)).toBe('dispatch-prompt');
      type('212');
      press('cmd-enter');
      expect(seen.at(-1)).toEqual({
        type: 'task/add',
        target: { kind: 'pane', paneId: 1 },
        text: '/review-pr 212',
        skill: '/review-pr',
      });
    } finally {
      unmount();
    }
  });

  test('the search filters the skills and ↑↓ ⏎ pick one', () => {
    const { renderer, seen, press, type, unmount } = mount();
    try {
      type('brain');
      expect(renderer.findByTestId('dispatch-skill-/review-pr')).toBeUndefined();
      expect(renderer.findByTestId('dispatch-skill-/superpowers:brainstorming')).toBeDefined();
      press('enter', 'cmd-enter');
      expect(seen.at(-1)).toEqual({
        type: 'task/add',
        target: { kind: 'pane', paneId: 1 },
        text: '/superpowers:brainstorming',
        skill: '/superpowers:brainstorming',
      });
    } finally {
      unmount();
    }
  });

  test('choosing another pane asks for its CLI’s skills and says where the task goes in its queue', () => {
    const { renderer, seen, unmount } = mount();
    try {
      click(renderer, 'dispatch-pane-2');
      expect(seen).toContainEqual({ type: 'skills/query', cli: 'codex', cwd: CWD });
      expect(textOf(renderer, 'dispatch-summary')).toContain('goes 2nd in its queue');
      expect(renderer.findByTestId('dispatch-pane-3')).toBeUndefined();
    } finally {
      unmount();
    }
  });

  test('the next free pane and a new pane take a CLI and a folder', () => {
    const { renderer, seen, press, type, store, unmount } = mount();
    try {
      click(renderer, 'dispatch-target-pool');
      click(renderer, 'dispatch-cli-codex');
      expect(seen).toContainEqual({ type: 'skills/query', cli: 'codex', cwd: CWD });
      click(renderer, 'dispatch-prompt');
      type('$audit');
      press('cmd-enter');
      expect(seen.at(-1)).toEqual({
        type: 'task/add',
        target: { kind: 'pool', cli: 'codex', cwd: CWD },
        text: '$audit',
      });
      flushSync(() => store.dispatch({ type: 'task/addFailed', message: 'no pane is free' }));
      renderer.flush();
      click(renderer, 'dispatch-target-new');
      expect(renderer.getAllText()).toContain('Open pane');
      press('cmd-enter');
      expect(seen.at(-1)).toEqual({
        type: 'task/add',
        target: { kind: 'new', cli: 'codex', cwd: CWD },
        text: '$audit',
      });
    } finally {
      unmount();
    }
  });

  test('an empty prompt is refused with a reason, and plyd’s refusal shows in the footer', () => {
    const { renderer, seen, press, store, unmount } = mount();
    try {
      press('cmd-enter');
      expect(seen).not.toContainEqual(expect.objectContaining({ type: 'task/add' }));
      expect(renderer.getAllText()).toContain('Write a prompt or pick a skill');
      flushSync(() =>
        store.dispatch({ type: 'task/addFailed', message: 'the queue already holds 32 tasks' }),
      );
      renderer.flush();
      expect(textOf(renderer, 'dispatch-summary')).toBe('the queue already holds 32 tasks');
      press('escape');
      expect(seen.at(-1)).toEqual({ type: 'overlay/close' });
    } finally {
      unmount();
    }
  });
});
