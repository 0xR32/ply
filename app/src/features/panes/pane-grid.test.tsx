import { describe, expect, test } from 'bun:test';
import { hasNativeTestRenderer, type TestRenderer } from '@gpuix/react/testing';
import type { ReactNode } from 'react';
import type { Action } from '../../state/actions';
import { makePane, makeState, mountWithStore } from '../../state/test-support';
import { PaneGrid } from './pane-grid';

function bounds(renderer: TestRenderer, testId: string) {
  const el = renderer.findByTestId(testId);
  if (!el) throw new Error(`no element ${testId}`);
  const b = renderer.getElementBounds(el.id);
  if (!b) throw new Error(`${testId} did not paint`);
  return b;
}

// Painted bounds are the box inside the border; pane frames draw a 1 px ring.
function frame(renderer: TestRenderer, testId: string) {
  const b = bounds(renderer, testId);
  return { x: b.x - 1, y: b.y - 1, width: b.width + 2, height: b.height + 2 };
}

function textOf(renderer: TestRenderer, testId: string): string | null {
  const el = renderer.findByTestId(testId);
  if (!el) return null;
  const parts: string[] = [];
  const walk = (id: number) => {
    const node = renderer.getElement(id);
    if (!node) return;
    if (node.text) parts.push(node.text);
    for (const child of node.children) walk(child);
  };
  walk(el.id);
  return parts.join('');
}

function Full({ children }: { children: ReactNode }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', width: '100%', height: '100%' }}>
      {children}
    </div>
  );
}

const grid = (
  <Full>
    <PaneGrid />
  </Full>
);

function countNodes(renderer: TestRenderer, skip: (testId: string | undefined) => boolean): number {
  const root = renderer.getRoot();
  if (!root) return 0;
  let n = 0;
  const walk = (id: number) => {
    const el = renderer.getElement(id);
    if (!el || skip(el.testId)) return;
    n++;
    for (const child of el.children) walk(child);
  };
  walk(root.id);
  return n;
}

const demo = () =>
  makeState(
    [
      makePane({
        id: 1,
        title: 'Tab bar polish',
        status: 'running',
        progress: { done: 3, total: 5 },
        model_seen: 'claude-opus-5',
        branch: 'feat/tab-bar',
      }),
      makePane({
        id: 2,
        position: 1,
        title: 'Review the page guides',
        status: 'waiting_permission',
        detail: 'claude wants to edit guide-dot.tsx',
        progress: { done: 2, total: 4 },
      }),
      makePane({ id: 3, position: 2, cli: 'shell', title: 'zsh', branch: 'feat/tab-bar' }),
      makePane({ id: 4, tab_id: 2, cli: 'codex', title: 'Merges', model_seen: 'gpt-5-codex' }),
    ],
    [{ id: 1 }, { id: 2 }],
  );

describe.if(hasNativeTestRenderer)('PaneGrid', () => {
  test('lays out one main pane and a stack with the canvas metrics', () => {
    const { renderer, unmount } = mountWithStore(grid, demo(), { width: 1440, height: 812 });
    try {
      const main = frame(renderer, 'pane-1');
      const second = frame(renderer, 'pane-2');
      const third = frame(renderer, 'pane-3');
      expect([main.x, main.y, main.height]).toEqual([14, 2, 812 - 2 - 10]);
      expect(1440 - (second.x + second.width)).toBe(14);
      expect(second.x - (main.x + main.width)).toBe(10);
      expect(third.y - (second.y + second.height)).toBe(10);
      expect(second.height).toBeCloseTo(third.height, 0);
      expect(main.width / second.width).toBeCloseTo(1.32, 1);
      const header = bounds(renderer, 'pane-1-focus');
      expect(header.height).toBe(42);
      expect(renderer.findByTestId('pane-4')).toBeUndefined();
    } finally {
      unmount();
    }
  });

  test('headers show position, title, branch, progress, CLI with the tracked model, and status', () => {
    const { renderer, unmount } = mountWithStore(grid, demo());
    try {
      const text = renderer.getAllText();
      expect(text).toEqual(
        expect.arrayContaining([
          '1',
          'Tab bar polish',
          'feat/tab-bar',
          '3/5',
          'claude',
          'claude-opus-5',
          'Running',
        ]),
      );
      expect(textOf(renderer, 'pane-2-progress')).toBe('2/4');
      expect(renderer.findByTestId('pane-2-model')).toBeUndefined();
      expect(renderer.findByTestId('pane-3-progress')).toBeUndefined();
      expect(textOf(renderer, 'pane-3-status')).toBe('zsh');
      expect(textOf(renderer, 'pane-1-status')).toBe('Running');
      expect(text).toContain('Needs you');
      expect(text).toContain('zsh');
    } finally {
      unmount();
    }
  });

  test('zoom shows only the focused pane and mounts only its terminal', () => {
    const state = demo();
    const zoomed = {
      ...state,
      tabs: state.tabs.map((t) => (t.id === 1 ? { ...t, zoomed: true, focus_pane_id: 2 } : t)),
    };
    const { renderer, unmount } = mountWithStore(grid, zoomed, { width: 1200, height: 800 });
    try {
      expect(renderer.findByTestId('terminal-2')).toBeDefined();
      for (const id of [1, 3, 4]) expect(renderer.findByTestId(`terminal-${id}`)).toBeUndefined();
      expect(frame(renderer, 'pane-2').width).toBe(1200 - 28);
    } finally {
      unmount();
    }
  });

  test('the focused pane holds GPUI focus so keys reach its terminal', () => {
    const { renderer, unmount } = mountWithStore(grid, demo());
    try {
      renderer.flush();
      expect(renderer.getFocusedElementId()).toBe(renderer.findByTestId('terminal-1')?.id ?? -1);
    } finally {
      unmount();
    }
  });

  test('the needs-you strip answers the CLI dialog by mouse through pane.answer', () => {
    const { store, renderer, unmount } = mountWithStore(grid, demo());
    const seen: Action[] = [];
    store.addEffect((a) => seen.push(a));
    try {
      expect(renderer.getAllText()).toContain('claude wants to edit guide-dot.tsx');
      expect(renderer.getAllText()).toContain('answer in claude');
      const yes = bounds(renderer, 'answer-2-1');
      renderer.nativeSimulateClick(yes.x + yes.width / 2, yes.y + yes.height / 2);
      const no = bounds(renderer, 'answer-2-3');
      renderer.nativeSimulateClick(no.x + no.width / 2, no.y + no.height / 2);
      expect(seen).toEqual([
        { type: 'pane/answer', paneId: 2, choice: 1 },
        { type: 'pane/answer', paneId: 2, choice: 3 },
      ]);
      expect(renderer.findByTestId('pane-1-waiting')).toBeUndefined();
    } finally {
      unmount();
    }
  });

  test('clicking a pane header focuses that pane', () => {
    const { store, renderer, unmount } = mountWithStore(grid, demo());
    try {
      const header = bounds(renderer, 'pane-3-focus');
      renderer.nativeSimulateClick(header.x + 200, header.y + header.height / 2);
      expect(store.getState().tabs[0]?.focus_pane_id).toBe(3);
    } finally {
      unmount();
    }
  });

  test('a 4-pane tab stays far under the 2 000 host-node budget (R-R16)', () => {
    const state = demo();
    const four = {
      ...state,
      panes: {
        ...state.panes,
        4: { ...state.panes[4], tab_id: 1, position: 3 },
      } as typeof state.panes,
      tabs: [{ ...state.tabs[0], pane_ids: [1, 2, 3, 4] } as (typeof state.tabs)[number]],
    };
    const { renderer, unmount } = mountWithStore(grid, four);
    try {
      const total = countNodes(renderer, () => false);
      const chrome = countNodes(renderer, (id) => id?.startsWith('terminal-') ?? false);
      expect(total).toBeLessThan(2000);
      expect(chrome).toBeLessThan(400);
    } finally {
      unmount();
    }
  });

  test('without a daemon it says so instead of showing panes', () => {
    const state = {
      ...demo(),
      connection: {
        kind: 'down',
        reason: 'plyd is not running',
        retryInMs: 400,
        starting: false,
      } as const,
    };
    const { renderer, unmount } = mountWithStore(grid, state);
    try {
      expect(renderer.getAllText()).toEqual(
        expect.arrayContaining(['Daemon not running', 'plyd is not running · retrying in 0.4 s']),
      );
      expect(renderer.findByTestId('pane-grid')).toBeUndefined();
    } finally {
      unmount();
    }
  });
});
