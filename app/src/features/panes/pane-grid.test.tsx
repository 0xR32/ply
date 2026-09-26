import { describe, expect, test } from 'bun:test';
import { hasNativeTestRenderer, type TestRenderer } from '@gpuix/react/testing';
import type { ReactNode } from 'react';
import type { Action } from '../../state/actions';
import type { PaneState } from '../../state/reducer';
import { makePane, makeState, mountWithStore } from '../../state/test-support';
import { tokens } from '../../theme/tokens';
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
  // On the CI runner a 1440 × 812 test window came out 1024 × 653, its usable screen; these fit, at whole-pixel columns.
  const W = 998;
  const H = 600;
  const PAD = {
    x: tokens.layout.gridPaddingX,
    top: tokens.layout.gridPaddingTop,
    bottom: tokens.layout.gridPaddingBottom,
  };
  const GAP = tokens.layout.gap;
  const inner = { width: W - 2 * PAD.x, height: H - PAD.top - PAD.bottom };
  const panesIn = (n: number) =>
    makeState(
      Array.from({ length: n }, (_, i) =>
        makePane({ id: i + 1, position: i, cli: 'shell', title: `pane ${i + 1}` }),
      ),
    );

  for (const n of [1, 2, 3]) {
    test(`${n} ${n === 1 ? 'pane fills' : 'panes split'} the tab into equal full-height columns in position order`, () => {
      const { renderer, unmount } = mountWithStore(grid, panesIn(n), { width: W, height: H });
      try {
        const width = (inner.width - (n - 1) * GAP) / n;
        for (let i = 0; i < n; i++) {
          const f = frame(renderer, `pane-${i + 1}`);
          expect(f.x).toBeCloseTo(PAD.x + i * (width + GAP), 0);
          expect(f.y).toBe(PAD.top);
          expect(f.width).toBeCloseTo(width, 0);
          expect(f.height).toBe(inner.height);
        }
        expect(bounds(renderer, 'pane-1-focus').height).toBe(42);
      } finally {
        unmount();
      }
    });
  }

  test('⌘→ moves focus to the next column, exactly as clicking it does (R58)', () => {
    const { store, renderer, unmount } = mountWithStore(grid, panesIn(2), undefined, true);
    try {
      renderer.flush();
      expect(store.getState().tabs[0]?.focus_pane_id).toBe(1);
      renderer.simulateKeystrokes('cmd-right');
      renderer.flush();
      expect(store.getState().tabs[0]?.focus_pane_id).toBe(2);
      renderer.simulateKeystrokes('cmd-right');
      renderer.flush();
      expect(store.getState().tabs[0]?.focus_pane_id).toBe(2);
    } finally {
      unmount();
    }
  });

  test('⌘↓ moves focus from the top-left to the bottom-left quadrant, in four panes (R58)', () => {
    const { store, renderer, unmount } = mountWithStore(grid, panesIn(4), undefined, true);
    try {
      renderer.flush();
      renderer.simulateKeystrokes('cmd-down');
      renderer.flush();
      expect(store.getState().tabs[0]?.focus_pane_id).toBe(3);
    } finally {
      unmount();
    }
  });

  test('4 panes form equal quadrants: top-left, top-right, bottom-left, bottom-right', () => {
    const { renderer, unmount } = mountWithStore(grid, panesIn(4), { width: W, height: H });
    try {
      const width = (inner.width - GAP) / 2;
      const height = (inner.height - GAP) / 2;
      const at = [
        [PAD.x, PAD.top],
        [PAD.x + width + GAP, PAD.top],
        [PAD.x, PAD.top + height + GAP],
        [PAD.x + width + GAP, PAD.top + height + GAP],
      ];
      at.forEach(([x, y], i) => {
        const f = frame(renderer, `pane-${i + 1}`);
        expect(f.x).toBeCloseTo(x as number, 0);
        expect(f.y).toBeCloseTo(y as number, 0);
        expect(f.width).toBeCloseTo(width, 0);
        expect(f.height).toBeCloseTo(height, 0);
      });
    } finally {
      unmount();
    }
  });

  test('closing a quadrant re-flows the other three into columns', async () => {
    const { store, renderer, unmount } = mountWithStore(grid, panesIn(4), {
      width: W,
      height: H,
    });
    try {
      const kept = renderer.findByTestId('pane-3')?.id;
      store.dispatch({ type: 'daemon/event', event: { e: 'pane.removed', p: { pane_id: 2 } } });
      await Bun.sleep(10);
      renderer.flush();
      expect(renderer.findByTestId('pane-3')?.id).toBe(kept);
      expect(renderer.findByTestId('pane-2')).toBeUndefined();
      const width = (inner.width - 2 * GAP) / 3;
      [1, 3, 4].forEach((id, i) => {
        const f = frame(renderer, `pane-${id}`);
        expect(f.x).toBeCloseTo(PAD.x + i * (width + GAP), 0);
        expect(f.height).toBe(inner.height);
      });
    } finally {
      unmount();
    }
  });

  test('dragging the gutter resizes the columns and the minimum holds', async () => {
    const { renderer, unmount } = mountWithStore(grid, panesIn(2), { width: W, height: H });
    const settle = async () => {
      for (let i = 0; i < 10; i++) {
        renderer.flush();
        await Bun.sleep(20);
      }
      renderer.flush();
    };
    try {
      await settle();
      const gutter = renderer.findByTestId('pane-grid-gutter-column-0');
      const g = gutter ? renderer.getElementBounds(gutter.id) : null;
      if (!g) throw new Error('no gutter once the grid was measured');
      const width = (inner.width - GAP) / 2;
      expect(frame(renderer, 'pane-1').x).toBeCloseTo(PAD.x, 0);
      expect(frame(renderer, 'pane-1').width).toBeCloseTo(width, 0);
      const x = g.x + g.width / 2;
      const y = g.y + g.height / 2;
      const dragTo = async (to: number) => {
        renderer.nativeSimulateMouseDown(x, y, 0);
        renderer.nativeSimulateMouseMove(to, y, 0);
        renderer.nativeSimulateMouseUp(to, y, 0);
        await settle();
      };
      await dragTo(x + 150);
      expect(frame(renderer, 'pane-1').width).toBeCloseTo(width + 150, 0);
      expect(frame(renderer, 'pane-2').width).toBeCloseTo(width - 150, 0);
      expect(frame(renderer, 'pane-2').x + frame(renderer, 'pane-2').width).toBeCloseTo(
        W - PAD.x,
        0,
      );
      const moved = renderer.findByTestId('pane-grid-gutter-column-0');
      const m = moved ? renderer.getElementBounds(moved.id) : null;
      if (!m) throw new Error('the gutter went away');
      renderer.nativeSimulateMouseDown(m.x + m.width / 2, y, 0);
      renderer.nativeSimulateMouseMove(W - 2, y, 0);
      renderer.nativeSimulateMouseUp(W - 2, y, 0);
      await settle();
      expect(frame(renderer, 'pane-2').width).toBeCloseTo(200, 0);
      expect(renderer.findByTestId('pane-grid-drag'), 'the release ends the drag').toBeUndefined();
    } finally {
      unmount();
    }
  });

  test('in quadrants the row gutter resizes both rows of panes together', async () => {
    const { renderer, unmount } = mountWithStore(grid, panesIn(4), { width: W, height: H });
    try {
      for (let i = 0; i < 10; i++) {
        renderer.flush();
        await Bun.sleep(20);
      }
      const gutter = renderer.findByTestId('pane-grid-gutter-row-0');
      const g = gutter ? renderer.getElementBounds(gutter.id) : null;
      if (!g) throw new Error('no row gutter');
      const height = (inner.height - GAP) / 2;
      const y = g.y + g.height / 2;
      renderer.nativeSimulateMouseDown(g.x + 40, y, 0);
      renderer.nativeSimulateMouseMove(g.x + 40, y - 60, 0);
      renderer.nativeSimulateMouseUp(g.x + 40, y - 60, 0);
      for (let i = 0; i < 5; i++) {
        renderer.flush();
        await Bun.sleep(20);
      }
      for (const id of [1, 2])
        expect(frame(renderer, `pane-${id}`).height).toBeCloseTo(height - 60, 0);
      for (const id of [3, 4])
        expect(frame(renderer, `pane-${id}`).height).toBeCloseTo(height + 60, 0);
      expect(renderer.findByTestId('pane-grid-gutter-column-1')).toBeUndefined();
    } finally {
      unmount();
    }
  });

  test('the second tab is not painted', () => {
    const { renderer, unmount } = mountWithStore(grid, demo(), { width: W, height: H });
    try {
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

  test("headers name the project, git's when plyd sent it and the folder's before, and git's linked worktree", () => {
    const state = makeState([
      makePane({ id: 1, project: 'agentmon', git_worktree: 'agentmon-side', branch: 'side' }),
      makePane({ id: 2, position: 1, cwd: '/Users/example/notes' }),
    ]);
    const { renderer, unmount } = mountWithStore(grid, state);
    try {
      expect(textOf(renderer, 'pane-1-project')).toBe('agentmon');
      expect(textOf(renderer, 'pane-2-project')).toBe('notes');
      expect(renderer.getAllText()).toContain('side · worktree agentmon-side');
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
    const { renderer, unmount } = mountWithStore(grid, zoomed, { width: W, height: H });
    try {
      expect(renderer.findByTestId('terminal-2')).toBeDefined();
      for (const id of [1, 3, 4]) expect(renderer.findByTestId(`terminal-${id}`)).toBeUndefined();
      expect(frame(renderer, 'pane-2').width).toBe(W - 2 * PAD.x);
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

  test('the needs-you strip answers a dialog Yes or No by meaning, with no positional choice', () => {
    const { store, renderer, unmount } = mountWithStore(grid, demo());
    const seen: Action[] = [];
    store.addEffect((a) => seen.push(a));
    try {
      expect(renderer.getAllText()).toContain('claude wants to edit guide-dot.tsx');
      expect(renderer.getAllText()).toContain('other options in the pane');
      expect(renderer.getAllText()).not.toContain('Always');
      const yes = bounds(renderer, 'answer-2-yes');
      renderer.nativeSimulateClick(yes.x + yes.width / 2, yes.y + yes.height / 2);
      const no = bounds(renderer, 'answer-2-no');
      renderer.nativeSimulateClick(no.x + no.width / 2, no.y + no.height / 2);
      expect(seen).toEqual([
        { type: 'pane/answer', paneId: 2, answer: 'yes' },
        { type: 'pane/answer', paneId: 2, answer: 'no' },
      ]);
      expect(renderer.findByTestId('pane-1-waiting')).toBeUndefined();
    } finally {
      unmount();
    }
  });

  test('a lost pane shows the resume strip, whose button resumes it through pane.resume', () => {
    const state = demo();
    const panes = {
      ...state.panes,
      3: { ...(state.panes[3] as PaneState), status: 'lost' as const },
    };
    const { store, renderer, unmount } = mountWithStore(grid, { ...state, panes });
    const seen: Action[] = [];
    store.addEffect((a) => seen.push(a));
    try {
      expect(renderer.getAllText()).toContain('shell stopped when plyd did');
      expect(renderer.getAllText()).toContain('resumes with a fresh shell');
      expect(renderer.findByTestId('pane-1-lost')).toBeUndefined();
      const resume = bounds(renderer, 'resume-3');
      renderer.nativeSimulateClick(resume.x + resume.width / 2, resume.y + resume.height / 2);
      expect(seen).toEqual([{ type: 'pane/resume', paneId: 3 }]);
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
