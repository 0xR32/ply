import { describe, expect, test } from 'bun:test';
import { flushSync } from '@gpuix/react';
import { hasNativeTestRenderer, type TestRenderer } from '@gpuix/react/testing';
import type { Action } from '../../state/actions';
import { type DirSearch, reduce } from '../../state/reducer';
import { makePane, makeState, mountWithStore } from '../../state/test-support';
import { NewPane } from './new-pane';

function mount(target: 'pane' | 'tab' = 'pane', dirs: Partial<DirSearch> = {}) {
  const loaded = makeState([makePane({ id: 1, cwd: '/Users/example/code/ply' })], [{ id: 1 }]);
  const opened = reduce(loaded, { type: 'command', id: target === 'tab' ? 'tab.new' : 'pane.new' });
  const state = { ...opened, dirs: { ...opened.dirs, ...dirs } };
  const m = mountWithStore(<NewPane target={target} />, state, { width: 900, height: 800 });
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
  return parts.join('');
}

/** The rows of the Directory field's suggestion list, top to bottom, as they read. */
function listed(renderer: TestRenderer): string[] {
  const rows: string[] = [];
  for (let i = 0; ; i++) {
    const row = textOf(renderer, `new-pane-dir-option-${i}`);
    if (row === undefined) return rows;
    rows.push(row);
  }
}

function clickOn(renderer: TestRenderer, testId: string): void {
  const el = renderer.findByTestId(testId);
  const b = el && renderer.getElementBounds(el.id);
  if (!b) throw new Error(`${testId} did not paint`);
  renderer.nativeSimulateClick(b.x + b.width / 2, b.y + b.height / 2);
  renderer.flush();
  renderer.dispatchNativeEvents();
  renderer.flush();
}

const RECENT = ['/Users/example/code/ply', '/Users/example/code/api', '/Users/example/notes'];
const REPOS = ['/Users/example/src/deploy', '/Users/example/code/ply-web'];

function focusedTestId(renderer: TestRenderer): string | undefined {
  const id = renderer.getFocusedElementId();
  return id === null ? undefined : renderer.getElement(id)?.testId;
}

describe.if(hasNativeTestRenderer)('NewPane', () => {
  test('opens on Claude Code in the focused pane’s directory and ⌘⏎ creates it', () => {
    const { renderer, seen, press, unmount } = mount();
    try {
      expect(focusedTestId(renderer)).toBe('new-pane-cli');
      const text = renderer.getAllText();
      for (const s of [
        'New pane',
        'Claude Code',
        'Codex',
        'Shell',
        'Run in a Claude Code worktree',
      ]) {
        expect(text).toContain(s);
      }
      expect(text).toContain('Model and effort are chosen inside the session.');
      press('cmd-enter');
      expect(seen).toEqual([
        {
          type: 'pane/create',
          request: { target: 'pane', cli: 'claude', cwd: '/Users/example/code/ply' },
        },
      ]);
    } finally {
      unmount();
    }
  });

  test('←→ choose the CLI: Codex hides the worktree, Shell also the prompt', () => {
    const { renderer, seen, press, unmount } = mount();
    try {
      press('right');
      expect(renderer.getAllText()).not.toContain('Run in a Claude Code worktree');
      expect(renderer.findByTestId('new-pane-prompt')).toBeDefined();
      press('right');
      expect(renderer.findByTestId('new-pane-prompt')).toBeUndefined();
      expect(renderer.getAllText()).toContain('Shell in this tab');
      press('right', 'left', 'left', '3', 'cmd-enter');
      expect(seen.at(-1)).toEqual({
        type: 'pane/create',
        request: { target: 'pane', cli: 'shell', cwd: '/Users/example/code/ply' },
      });
    } finally {
      unmount();
    }
  });

  test('Tab walks the fields; the worktree needs a name and is sent with the first prompt', () => {
    const { renderer, seen, press, unmount } = mount('tab');
    try {
      expect(renderer.getAllText()).toContain('New tab');
      press('tab');
      expect(focusedTestId(renderer)).toBe('new-pane-dir');
      press('tab');
      expect(focusedTestId(renderer)).toBe('new-pane-browse');
      press('tab');
      expect(focusedTestId(renderer)).toBe('new-pane-worktree');
      press('space');
      expect(renderer.findByTestId('new-pane-worktree-name')).toBeDefined();
      press('cmd-enter');
      expect(seen).toEqual([]);
      expect(renderer.getAllText()).toContain('Name the worktree, or turn it off');
      press('tab', 'f', 'i', 'x', 'tab', 'g', 'o');
      expect(renderer.getAllText()).toContain('claude --worktree fix');
      press('shift-tab');
      expect(focusedTestId(renderer)).toBe('new-pane-worktree-name');
      press('cmd-enter');
      expect(seen).toEqual([
        {
          type: 'pane/create',
          request: {
            target: 'tab',
            cli: 'claude',
            cwd: '/Users/example/code/ply',
            worktree: 'fix',
            prompt: 'go',
          },
        },
      ]);
    } finally {
      unmount();
    }
  });

  test('esc cancels, and a relative directory is refused before any request', () => {
    const esc = mount();
    try {
      esc.press('escape');
      expect(esc.seen).toEqual([{ type: 'overlay/close' }]);
    } finally {
      esc.unmount();
    }
    const bad = mount();
    try {
      bad.press('tab', 'cmd-a', 'r', 'e', 'l', 'cmd-enter');
      expect(bad.seen.filter((a) => a.type === 'pane/create')).toEqual([]);
      expect(bad.renderer.getAllText()).toContain(
        'The directory must be an absolute path or start with ~',
      );
    } finally {
      bad.unmount();
    }
  });

  test('a failed pane.create shows plyd’s message in the form', () => {
    const { store, renderer, unmount } = mount();
    try {
      flushSync(() =>
        store.dispatch({ type: 'pane/createFailed', message: 'claude is not on PATH' }),
      );
      renderer.flush();
      expect(renderer.getAllText()).toContain('claude is not on PATH');
    } finally {
      unmount();
    }
  });

  test('the directory field searches: typing filters recents and repositories, a basename match first', () => {
    const { renderer, press, unmount } = mount('pane', { recent: RECENT, repos: REPOS });
    try {
      expect(renderer.findByTestId('new-pane-dir-list')).toBeUndefined();
      press('tab', 'cmd-a', 'p', 'l', 'y');
      expect(listed(renderer)).toEqual([
        '~/code/plyrecent',
        '~/code/ply-webrepo',
        '~/src/deployrepo',
      ]);
      press('backspace', 'backspace', 'backspace');
      expect(listed(renderer)).toEqual(['~/code/plyrecent', '~/code/apirecent', '~/notesrecent']);
    } finally {
      unmount();
    }
  });

  test('↑↓ move the highlight and ⏎ takes that folder into the field and closes the list', () => {
    const { renderer, store, seen, press, unmount } = mount('pane', {
      recent: RECENT,
      repos: REPOS,
    });
    try {
      press('tab', 'cmd-a', 'p', 'down', 'down', 'up', 'down');
      expect(store.getState().dirs.open).toBe(true);
      press('enter');
      expect(store.getState().dirs.query).toBe('~/code/ply-web');
      expect(renderer.findByTestId('new-pane-dir-list')).toBeUndefined();
      expect(seen.filter((a) => a.type === 'pane/create')).toEqual([]);
      press('cmd-enter');
      expect(seen.at(-1)).toEqual({
        type: 'pane/create',
        request: { target: 'pane', cli: 'claude', cwd: '/Users/example/code/ply-web' },
      });
    } finally {
      unmount();
    }
  });

  test('a click takes a suggestion too, and the field keeps focus', () => {
    const { renderer, store, press, unmount } = mount('pane', { recent: RECENT, repos: REPOS });
    try {
      press('tab', 'cmd-a', 'n');
      clickOn(renderer, 'new-pane-dir-option-0');
      expect(store.getState().dirs.query).toBe('~/notes');
      expect(renderer.findByTestId('new-pane-dir-list')).toBeUndefined();
      expect(focusedTestId(renderer)).toBe('new-pane-dir');
    } finally {
      unmount();
    }
  });

  test('esc closes the list first and the form only when no list is open', () => {
    const { renderer, seen, press, unmount } = mount('pane', { recent: RECENT, repos: REPOS });
    try {
      press('tab', 'cmd-a', 'a');
      expect(renderer.findByTestId('new-pane-dir-list')).toBeDefined();
      press('escape');
      expect(renderer.findByTestId('new-pane-dir-list')).toBeUndefined();
      expect(seen.filter((a) => a.type === 'overlay/close')).toEqual([]);
      press('escape');
      expect(seen.at(-1)).toEqual({ type: 'overlay/close' });
    } finally {
      unmount();
    }
  });

  test('⌘⏎ opens with what the field says, not the highlighted suggestion; Tab leaves and closes the list', () => {
    const { renderer, seen, press, unmount } = mount('tab', { recent: RECENT, repos: REPOS });
    try {
      press('tab', 'cmd-a', '~', '/', 'c', 'o');
      expect(listed(renderer)[0]).toBe('~/code/plyrecent');
      press('tab');
      expect(renderer.findByTestId('new-pane-dir-list')).toBeUndefined();
      expect(focusedTestId(renderer)).toBe('new-pane-browse');
      press('cmd-enter');
      expect(seen.at(-1)).toEqual({
        type: 'pane/create',
        request: { target: 'tab', cli: 'claude', cwd: '/Users/example/co' },
      });
    } finally {
      unmount();
    }
  });

  test('Browse asks for a folder and the picked one fills the field', () => {
    const { renderer, store, seen, press, unmount } = mount('pane', { recent: RECENT });
    const picked = '/Users/example/picked';
    store.addEffect((a) => {
      if (a.type === 'dirs/browse') store.dispatch({ type: 'dirs/accept', path: picked });
    });
    try {
      press('tab', 'tab');
      expect(focusedTestId(renderer)).toBe('new-pane-browse');
      press('space');
      expect(seen.filter((a) => a.type === 'dirs/browse')).toHaveLength(1);
      expect(store.getState().dirs.query).toBe('~/picked');
      store.dispatch({ type: 'dirs/query', query: '~' });
      clickOn(renderer, 'new-pane-browse');
      expect(seen.filter((a) => a.type === 'dirs/browse')).toHaveLength(2);
      expect(store.getState().dirs.query).toBe('~/picked');
      press('cmd-enter');
      expect(seen.at(-1)).toEqual({
        type: 'pane/create',
        request: { target: 'pane', cli: 'claude', cwd: picked },
      });
    } finally {
      unmount();
    }
  });
});
