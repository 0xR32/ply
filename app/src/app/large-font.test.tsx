import { describe, expect, test } from 'bun:test';
import { createTestRoot, hasNativeTestRenderer, type TestRenderer } from '@gpuix/react/testing';
import { windowKeyListeners } from '../keymap/dispatcher';
import { createStore } from '../state/store';
import { makePane, makeState } from '../state/test-support';
import { fontSizeRange } from '../theme/tokens';
import { App } from './App';

interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

function box(renderer: TestRenderer, testId: string): Box | undefined {
  const node = renderer.findByTestId(testId);
  return (node && renderer.getElementBounds(node.id)) ?? undefined;
}

function largeFontState() {
  const now = Math.floor(Date.now() / 1000);
  const state = makeState(
    [
      makePane({
        id: 1,
        title: 'Polish the tab bar and the palette hints for narrow windows',
        status: 'running',
        statusSince: now - 252,
        progress: { done: 3, total: 5 },
        model_seen: 'claude-opus-5',
        branch: 'feat/tab-bar-polish-for-narrow-windows',
        worktree_seen: 'tab-bar',
      }),
      makePane({
        id: 2,
        position: 1,
        title: 'Review the page guides',
        status: 'waiting_permission',
        detail: 'claude wants to edit guide-dot.tsx',
        progress: { done: 2, total: 4 },
        model_seen: 'claude-sonnet-5',
        branch: 'feat/page-guides',
      }),
      makePane({
        id: 3,
        position: 2,
        cli: 'codex',
        title: "This week's merges",
        progress: { done: 1, total: 1 },
        model_seen: 'gpt-5-codex',
        branch: 'main',
      }),
    ],
    [{ id: 1, name: 'ply' }],
  );
  return { ...state, settings: { ...state.settings, font_size: fontSizeRange.max } };
}

const HEADER_ITEMS = ['focus', 'progress-bar', 'progress', 'cli', 'status'];

describe.if(hasNativeTestRenderer)('the chrome at the largest font size', () => {
  for (const width of [1440, 1100, 720]) {
    test(`at ${width} px the pane headers drop items instead of overlapping, and the footer drops hints`, async () => {
      const store = createStore(largeFontState());
      const root = createTestRoot({ width, height: 900, ...windowKeyListeners(store) });
      root.render(<App store={store} />);
      const { renderer } = root;
      try {
        renderer.flush();
        await Bun.sleep(60);
        renderer.flush();
        for (const id of [1, 2, 3]) {
          const header = box(renderer, `pane-${id}-header`);
          if (!header) throw new Error(`pane ${id} has no header`);
          const items = HEADER_ITEMS.map((name) => box(renderer, `pane-${id}-${name}`)).filter(
            (b): b is Box => b !== undefined && b.width > 0,
          );
          let right = header.x;
          for (const item of items) {
            expect(item.x).toBeGreaterThanOrEqual(right - 0.5);
            right = item.x + item.width;
          }
          expect(right).toBeLessThanOrEqual(header.x + header.width + 0.5);
        }
        const footer = box(renderer, 'statusbar');
        const hints = box(renderer, 'statusbar-hints');
        const counts = box(renderer, 'cli-counts');
        if (!footer || !hints || !counts) throw new Error('the footer did not paint');
        expect(counts.x + counts.width).toBeLessThanOrEqual(footer.x + footer.width + 0.5);
        expect(hints.x + hints.width).toBeLessThanOrEqual(counts.x + 0.5);
        for (const label of ['commands', 'new-tab', 'new-pane', 'terminal-here', 'next-waiting']) {
          const hint = box(renderer, `hint-${label}`);
          if (!hint || hint.y >= hints.y + hints.height) continue;
          expect(hint.x + hint.width).toBeLessThanOrEqual(hints.x + hints.width + 0.5);
        }
        expect(box(renderer, 'hint-commands')?.y).toBe(hints.y);
      } finally {
        root.unmount();
      }
    });
  }
});
