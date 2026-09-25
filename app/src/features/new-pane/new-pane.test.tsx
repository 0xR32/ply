import { describe, expect, test } from 'bun:test';
import { flushSync } from '@gpuix/react';
import { hasNativeTestRenderer, type TestRenderer } from '@gpuix/react/testing';
import type { Action } from '../../state/actions';
import { makePane, makeState, mountWithStore } from '../../state/test-support';
import { NewPane } from './new-pane';

function mount(target: 'pane' | 'tab' = 'pane') {
  const state = makeState([makePane({ id: 1, cwd: '/Users/example/code/ply' })], [{ id: 1 }], {
    overlay: { kind: 'new-pane', target },
  });
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
      expect(bad.seen).toEqual([]);
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
});
