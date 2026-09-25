import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { connectTest } from '@gpuix/react/automation';
import { createTestRoot, hasNativeTestRenderer, type TestRenderer } from '@gpuix/react/testing';
import { createControlClient } from '../ipc/control-client';
import { MockServer } from '../ipc/mock-server';
import { windowKeyListeners } from '../keymap/dispatcher';
import { startEffects } from '../state/effects';
import type { AppState } from '../state/reducer';
import { initialState } from '../state/reducer';
import { selectActiveTab } from '../state/selectors';
import { createStore, type Store } from '../state/store';
import { accentAlternatives } from '../theme/tokens';
import { App } from './App';

const HOME = '/Users/example';
const cleanups: (() => void)[] = [];

afterEach(() => {
  for (const c of cleanups.splice(0).reverse()) c();
});

interface Journey {
  server: MockServer;
  store: Store;
  renderer: TestRenderer;
  keys: (keystrokes: string) => Promise<void>;
  until: (check: (s: AppState) => boolean, what: string) => Promise<void>;
}

async function start(scenario: 'demo' | 'empty' = 'empty'): Promise<Journey> {
  const dir = mkdtempSync(join(tmpdir(), 'ply-j-'));
  cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
  const socketPath = join(dir, 'run', 'plyd.sock');
  const server = MockServer.start({ socketPath, home: HOME, scenario, startDelayMs: 20 });
  cleanups.push(() => server.stop());
  const store = createStore(initialState({ home: HOME, shellName: 'zsh', geistAvailable: false }));
  const client = createControlClient({ socketPath, appVersion: '0.1.0', initialBackoffMs: 10 });
  cleanups.push(
    startEffects(store, {
      client,
      layoutSaveDelayMs: 5,
      settingsSaveDelayMs: 5,
      reducedMotion: async () => true,
      buildId: async () => null,
    }),
  );
  const root = createTestRoot({ width: 1440, height: 900, ...windowKeyListeners(store) });
  root.render(<App store={store} />);
  cleanups.push(root.unmount);
  const { renderer } = root;
  const until = async (check: (s: AppState) => boolean, what: string) => {
    const end = Date.now() + 3_000;
    while (!check(store.getState())) {
      if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
      await Bun.sleep(5);
      renderer.flush();
    }
    await Bun.sleep(5);
    renderer.flush();
  };
  const keys = async (keystrokes: string) => {
    for (const k of keystrokes.split(' ')) {
      renderer.simulateKeystrokes(k);
      await Bun.sleep(2);
      renderer.flush();
    }
  };
  await until((s) => s.workspace !== null, 'the session to load');
  return { server, store, renderer, keys, until };
}

const focusedId = (s: AppState) => selectActiveTab(s)?.focus_pane_id;
const paneCount = (s: AppState) => Object.keys(s.panes).length;

describe.if(hasNativeTestRenderer)('journeys, keyboard only', () => {
  test('J1 first run: open a repo, create a claude pane, get a prompt', async () => {
    const j = await start('empty');
    expect(j.renderer.getAllText()).toContain('No panes yet');
    expect(j.server.palette?.cursor).toBe(accentAlternatives.blue);
    await j.keys('cmd-t');
    await j.until((s) => s.overlay?.kind === 'new-pane', 'the new-tab form');
    expect(j.renderer.getAllText()).toContain('New tab');
    await j.keys('tab');
    const automation = await connectTest(j.renderer);
    await automation.getByTestId('new-pane-dir').fill('~/code/ply');
    await j.keys('cmd-enter');
    await j.until((s) => paneCount(s) === 1, 'the claude pane');
    await j.until((s) => s.panes[1]?.status === 'idle', 'the claude prompt');
    expect(j.server.requestsOf('pane.create')).toEqual([
      { workspace_id: 1, cli: 'claude', cwd: `${HOME}/code/ply` },
    ]);
    expect(j.store.getState().overlay).toBeNull();
    expect(j.renderer.getAllText()).toContain('Your turn');
    await j.until(() => j.server.layout?.tabs.length === 1, 'the layout to be saved');
    expect(j.server.layout?.tabs[0]).toMatchObject({
      name: 'ply',
      pane_ids: [1],
      focus_pane_id: 1,
    });
  });

  test('J2 parallel work: three agents plus a shell; ⌘J cycles through what needs you', async () => {
    const j = await start('empty');
    await j.keys('cmd-t');
    await j.until((s) => s.overlay?.kind === 'new-pane', 'the form');
    await j.keys('cmd-enter');
    await j.until((s) => paneCount(s) === 1 && s.overlay === null, 'pane 1');
    await j.keys('cmd-n');
    await j.until((s) => s.overlay?.kind === 'new-pane', 'the form');
    await j.keys('right cmd-enter');
    await j.until((s) => paneCount(s) === 2 && s.overlay === null, 'the codex pane');
    await j.keys('cmd-n');
    await j.until((s) => s.overlay?.kind === 'new-pane', 'the form');
    await j.keys('cmd-enter');
    await j.until((s) => paneCount(s) === 3 && s.overlay === null, 'pane 3');
    await j.keys('cmd-d');
    await j.until((s) => paneCount(s) === 4, 'the shell');
    const state = j.store.getState();
    expect(Object.values(state.panes).map((p) => p.cli)).toEqual([
      'claude',
      'codex',
      'claude',
      'shell',
    ]);
    expect(state.tabs).toHaveLength(1);
    expect(j.renderer.getAllText()).toContain('2 claude · 1 codex · 1 zsh');

    j.server.setStatus(2, 'waiting_permission', 'codex wants to run cargo test');
    j.server.setStatus(3, 'waiting_input', 'claude asks which branch to use');
    await j.until((s) => s.panes[3]?.status === 'waiting_input', 'two panes to need you');
    expect(j.renderer.getAllText()).toContain('2 need you');
    await j.keys('cmd-j');
    await j.until((s) => focusedId(s) === 2, '⌘J to reach pane 2');
    await j.keys('cmd-j');
    await j.until((s) => focusedId(s) === 3, '⌘J to reach pane 3');
    await j.keys('cmd-j');
    await j.until((s) => focusedId(s) === 2, '⌘J to wrap to pane 2');
    await j.keys('cmd-[ cmd-[');
    await j.until((s) => focusedId(s) === 4, '⌘[ to step back to the shell');
  });

  test('J3 many agents: two tabs with three agents each; ⌘J jumps across tabs', async () => {
    const j = await start('empty');
    for (const [tab, target] of [
      [1, 'cmd-t'],
      [1, 'cmd-n'],
      [1, 'cmd-n'],
      [2, 'cmd-t'],
      [2, 'cmd-n'],
      [2, 'cmd-n'],
    ] as const) {
      const before = paneCount(j.store.getState());
      await j.keys(target);
      await j.until((s) => s.overlay?.kind === 'new-pane', 'the form');
      await j.keys('cmd-enter');
      await j.until(
        (s) => paneCount(s) === before + 1 && s.overlay === null,
        `a pane in tab ${tab}`,
      );
    }
    const tabs = j.store.getState().tabs;
    expect(tabs.map((t) => t.pane_ids.length)).toEqual([3, 3]);
    const [first, second] = tabs;
    if (!first || !second) throw new Error('expected two tabs');
    await j.keys('cmd-1');
    await j.until((s) => s.activeTabId === first.id, '⌘1');
    j.server.setStatus(second.pane_ids[1] as number, 'waiting_permission', 'Edit a.ts');
    await j.until(
      (s) => s.panes[second.pane_ids[1] as number]?.status === 'waiting_permission',
      'the prompt',
    );
    await j.keys('cmd-j');
    await j.until(
      (s) => s.activeTabId === second.id && focusedId(s) === second.pane_ids[1],
      '⌘J across tabs',
    );
    await j.keys('cmd-shift-[');
    await j.until((s) => s.activeTabId === first.id, '⌘⇧[ back to tab 1');
  });

  test('the needs-you strip answers by mouse and the pane runs again', async () => {
    const j = await start('demo');
    const strip = j.renderer.findByTestId('answer-2-1');
    const b = strip && j.renderer.getElementBounds(strip.id);
    if (!b) throw new Error('the answer button did not paint');
    j.renderer.nativeSimulateClick(b.x + b.width / 2, b.y + b.height / 2);
    await j.until((s) => s.panes[2]?.status === 'running', 'the answered pane to run');
    expect(j.server.requestsOf('pane.answer')).toEqual([{ pane_id: 2, choice: 1 }]);
    expect(j.renderer.findByTestId('pane-2-waiting')).toBeUndefined();
  });

  test('⌘⇧W on a live pane asks first, ⏎ closes it and ⌘W stays the window’s', async () => {
    const j = await start('demo');
    await j.keys('cmd-w');
    expect(j.store.getState().overlay).toBeNull();
    await j.keys('cmd-shift-w');
    await j.until((s) => s.overlay?.kind === 'close-confirm', 'the confirmation');
    await j.keys('enter');
    await j.until((s) => s.panes[1] === undefined, 'pane 1 to close');
    expect(j.server.requestsOf('pane.close')).toEqual([{ pane_id: 1, kill: true }]);
    expect(focusedId(j.store.getState())).toBe(2);
  });
});
