import { createTestRoot, type TestRenderer } from '@gpuix/react/testing';
import type { ReactNode } from 'react';
import { windowKeyListeners } from '../keymap/dispatcher';
import { ChromeThemeContext, chromeFonts, createChromeTheme } from '../theme/chrome';
import type { Skill, Tab, Task, Workspace } from './actions';
import { type AppState, initialState, type PaneState } from './reducer';
import { createStore, type Store, StoreContext } from './store';

/** The workspace test states use; `/Users/example` keeps fixtures free of real paths (INV-11). */
export const testWorkspace: Workspace = {
  id: 1,
  path: '/Users/example',
  name: 'example',
  opened_at: 1_790_000_000,
};

/** A pane with test defaults (tab 1, idle claude pane in the example workspace) overridden by `fields`. */
export function makePane(fields: Partial<PaneState> & { id: number }): PaneState {
  return {
    workspace_id: testWorkspace.id,
    tab_id: 1,
    position: 0,
    cli: 'claude',
    cwd: `${testWorkspace.path}/code/ply`,
    title: 'claude',
    status: 'idle',
    created_at: 1_790_000_000,
    ...fields,
  };
}

/** A queued task on pane 1 of the example workspace, overridden by `fields`. */
export function makeTask(fields: Partial<Task> & { id: number }): Task {
  return {
    workspace_id: testWorkspace.id,
    pane_id: 1,
    text: `task ${fields.id}`,
    state: 'queued',
    position: 0,
    created_at: 1_790_000_000,
    ...fields,
  };
}

/** Skills as `skill.list` returns them for Claude Code: a user skill with a hint, a plugin skill, a command. */
export const testSkills: Skill[] = [
  {
    name: 'review-pr',
    invocation: '/review-pr',
    description: 'Full pre-merge review of a pull request',
    argument_hint: '[PR number]',
    source: 'user',
  },
  { name: 'open-pr', invocation: '/open-pr', description: 'Open a pull request', source: 'user' },
  {
    name: 'brainstorming',
    invocation: '/superpowers:brainstorming',
    description: 'Explore intent and design before building',
    source: 'plugin',
    plugin: 'superpowers',
  },
];

/** A connected, loaded state holding `panes` in `tabs` (tab 1 active unless `activeTabId` says otherwise). */
export function makeState(
  panes: PaneState[],
  tabs: Partial<Tab>[] = [{ id: 1 }],
  patch: Partial<AppState> = {},
): AppState {
  const base = initialState({
    home: testWorkspace.path,
    shellName: 'zsh',
    geistAvailable: false,
  });
  const full: Tab[] = tabs.map((t, i) => {
    const id = t.id ?? i + 1;
    const ids = t.pane_ids ?? panes.filter((p) => p.tab_id === id).map((p) => p.id);
    const focus = t.focus_pane_id ?? ids[0];
    return {
      name: t.name ?? `tab${id}`,
      position: i,
      zoomed: false,
      ...t,
      id,
      pane_ids: ids,
      ...(focus !== undefined ? { focus_pane_id: focus } : {}),
    };
  });
  return {
    ...base,
    connection: { kind: 'connected', daemonVersion: '0.1.0' },
    workspace: testWorkspace,
    panes: Object.fromEntries(panes.map((p) => [p.id, p])),
    tabs: full,
    activeTabId: full[0]?.id ?? null,
    reducedMotion: true,
    ...patch,
  };
}

/** A mounted test tree with the store and the chrome theme around `node`; call `unmount` in `finally`. */
export interface Mounted {
  store: Store;
  renderer: TestRenderer;
  unmount: () => void;
  rerender: (node: ReactNode) => void;
}

/** Renders `node` under a store holding `state` (or the given store); `withKeymap` also installs the real dispatcher, for tests that press ⌘ chords. */
export function mountWithStore(
  node: ReactNode,
  state: AppState | Store,
  size: { width: number; height: number } = { width: 1440, height: 900 },
  withKeymap = false,
): Mounted {
  const store = 'dispatch' in state ? state : createStore(state);
  const { render, renderer, unmount } = createTestRoot(
    withKeymap ? { ...size, ...windowKeyListeners(store) } : size,
  );
  const wrap = (child: ReactNode) => {
    const s = store.getState();
    const theme = createChromeTheme(s.settings.accent, s.settings.font_size, chromeFonts(false));
    return (
      <StoreContext.Provider value={store}>
        <ChromeThemeContext.Provider value={theme}>{child}</ChromeThemeContext.Provider>
      </StoreContext.Provider>
    );
  };
  render(wrap(node));
  return { store, renderer, unmount, rerender: (next) => render(wrap(next)) };
}
