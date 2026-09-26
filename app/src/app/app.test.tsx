import { describe, expect, test } from 'bun:test';
import { join } from 'node:path';
import { createTestRoot, hasNativeTestRenderer, type TestRenderer } from '@gpuix/react/testing';
import { windowKeyListeners } from '../keymap/dispatcher';
import { reduce } from '../state/reducer';
import { createStore, type Store } from '../state/store';
import { makePane, makeState, makeTask } from '../state/test-support';
import { App } from './App';

const shots = process.env.PLY_SCREENSHOT_DIR;

function shot(renderer: TestRenderer, name: string): void {
  if (!shots) return;
  renderer.flush();
  renderer.captureScreenshot(join(shots, `${name}.png`));
}

function canvasState() {
  const now = Math.floor(Date.now() / 1000);
  return makeState(
    [
      makePane({
        id: 1,
        title: 'Tab bar polish',
        status: 'running',
        statusSince: now - 252,
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
        model_seen: 'claude-sonnet-5',
        branch: 'feat/page-guides',
      }),
      makePane({ id: 3, position: 2, cli: 'shell', title: 'zsh', branch: 'feat/tab-bar' }),
      makePane({
        id: 4,
        tab_id: 2,
        cli: 'codex',
        cwd: '/Users/example/code/notes',
        title: "This week's merges",
        progress: { done: 1, total: 1 },
        model_seen: 'gpt-5-codex',
        branch: 'main',
      }),
    ],
    [
      { id: 1, name: 'ply' },
      { id: 2, name: 'notes' },
    ],
  );
}

function mount(store: Store) {
  const root = createTestRoot({ width: 1440, height: 900, ...windowKeyListeners(store) });
  root.render(<App store={store} />);
  return root;
}

describe.if(hasNativeTestRenderer)('App', () => {
  test('paints the main screen of the canvas: bar, tabs, status, needs-you pill and panes', () => {
    const store = createStore(canvasState());
    const { renderer, unmount } = mount(store);
    try {
      const text = renderer.getAllText();
      for (const s of ['ply', 'notes', '1 needs you', 'Search or run a command']) {
        expect(text).toContain(s);
      }
      expect(text).toContain('2 claude · 1 codex · 1 zsh');
      const bar = renderer.findByTestId('top-bar');
      const barBox = bar && renderer.getElementBounds(bar.id);
      expect(barBox?.height).toBe(40);
      const status = renderer.findByTestId('statusbar');
      const statusBox = status && renderer.getElementBounds(status.id);
      expect(
        statusBox &&
          barBox &&
          statusBox.y >= barBox.y &&
          statusBox.y + statusBox.height <= barBox.y + barBox.height,
      ).toBe(true);
      expect(renderer.findByTestId('hint-commands')).toBeUndefined();
      shot(renderer, 'main');
    } finally {
      unmount();
    }
  });

  test('⌘K opens the palette and ⌘N the new-pane form through the window key listener', () => {
    const store = createStore(canvasState());
    const { renderer, unmount } = mount(store);
    try {
      renderer.simulateKeystrokes('cmd-k');
      renderer.flush();
      expect(store.getState().overlay).toEqual({ kind: 'palette' });
      expect(renderer.findByTestId('palette')).toBeDefined();
      shot(renderer, 'palette');
      renderer.simulateKeystrokes('escape');
      renderer.flush();
      expect(store.getState().overlay).toBeNull();
      renderer.simulateKeystrokes('cmd-n');
      renderer.flush();
      expect(store.getState().overlay).toEqual({ kind: 'new-pane', target: 'pane' });
      shot(renderer, 'new-pane');
    } finally {
      unmount();
    }
  });

  test('clicking the needs-you pill focuses the waiting pane', () => {
    const store = createStore(canvasState());
    const { renderer, unmount } = mount(store);
    try {
      const pill = renderer.findByTestId('needs-you');
      const b = pill && renderer.getElementBounds(pill.id);
      if (!b) throw new Error('pill did not paint');
      renderer.nativeSimulateClick(b.x + b.width / 2, b.y + b.height / 2);
      expect(store.getState().tabs[0]?.focus_pane_id).toBe(2);
    } finally {
      unmount();
    }
  });

  test('the queue pill counts queued tasks, opens the queue on a click, and ⌘E ⌘⇧E open the form and the queue', () => {
    const loaded = reduce(canvasState(), {
      type: 'tasks/loaded',
      list: {
        tasks: [makeTask({ id: 1, pane_id: 1 }), makeTask({ id: 2, pane_id: 4 })],
        queues: [{ pane_id: 4, paused: 'restored' }],
      },
    });
    const store = createStore(loaded);
    const { renderer, unmount } = mount(store);
    try {
      const pill = renderer.findByTestId('queue-pill');
      const b = pill && renderer.getElementBounds(pill.id);
      if (!b) throw new Error('queue pill did not paint');
      expect(renderer.getAllText()).toContain('2 queued');
      renderer.nativeSimulateClick(b.x + b.width / 2, b.y + b.height / 2);
      renderer.flush();
      expect(store.getState().overlay).toEqual({ kind: 'queue' });
      expect(renderer.findByTestId('queue')).toBeDefined();
      renderer.simulateKeystrokes('escape');
      renderer.flush();
      expect(store.getState().overlay).toBeNull();
      renderer.simulateKeystrokes('cmd-e');
      renderer.flush();
      expect(store.getState().overlay?.kind).toBe('dispatch');
      expect(renderer.findByTestId('dispatch')).toBeDefined();
    } finally {
      unmount();
    }
  });

  test('no queue pill while nothing is queued', () => {
    const { renderer, unmount } = mount(createStore(canvasState()));
    try {
      expect(renderer.findByTestId('queue-pill')).toBeUndefined();
    } finally {
      unmount();
    }
  });

  test('a plyd of another build is named in the footer, and the palette quits only after ⏎ confirms', async () => {
    const state = canvasState();
    const store = createStore({ ...state, env: { ...state.env, buildId: '0.1.0+0a1b2c3d4e5f' } });
    const { renderer, unmount } = mount(store);
    const seen: string[] = [];
    store.addEffect((a) => seen.push(a.type));
    try {
      expect(renderer.getAllText()).toContain(
        'plyd is from another build — cargo build --release -p ply-daemon, then Restart plyd',
      );
      renderer.simulateKeystrokes('cmd-k');
      for (const k of ['q', 'u', 'i', 't']) renderer.simulateKeystrokes(k);
      renderer.simulateKeystrokes('enter');
      renderer.flush();
      expect(store.getState().overlay).toEqual({ kind: 'quit-confirm' });
      expect(seen).not.toContain('daemon/quit');
      expect(renderer.getAllText()).toContain('Quit ply and stop every session?');
      expect(renderer.getAllText().join(' ')).toContain(
        '4 panes run a process; each one is stopped.',
      );
      renderer.simulateKeystrokes('enter');
      renderer.flush();
      expect(seen).toContain('daemon/quit');
      expect(store.getState().overlay).toBeNull();
      store.dispatch({
        type: 'connection/changed',
        state: { kind: 'connected', daemonVersion: '0.1.0+0a1b2c3d4e5f' },
      });
      await Bun.sleep(10);
      renderer.flush();
      expect(renderer.getAllText()).not.toContain(
        'plyd is from another build — cargo build --release -p ply-daemon, then Restart plyd',
      );
    } finally {
      unmount();
    }
  });
});
