import { type ControlClient, RequestError } from '../ipc/control-client';
import { log } from '../ipc/log';
import { readReducedMotion } from '../ipc/os';
import type { Layout, PaneCreateParams, Workspace } from '../ipc/proto.gen';
import { terminalThemeFor } from '../theme/tokens';
import type { Action, Event, NewPaneRequest } from './actions';
import type { AppState } from './reducer';
import { isAlive, selectActiveTab, selectFocusedPane } from './selectors';
import type { Store } from './store';

/** Timing and OS hooks of the effects; tests shorten the delays and stub the reduce-motion read. */
export interface EffectsOptions {
  client: ControlClient;
  layoutSaveDelayMs?: number;
  settingsSaveDelayMs?: number;
  noticeMs?: number;
  reducedMotion?: () => Promise<boolean>;
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function pickWorkspace(list: Workspace[], home: string): Workspace | undefined {
  return (
    list.find((w) => w.path === home) ?? [...list].sort((a, b) => b.opened_at - a.opened_at)[0]
  );
}

function createParams(state: AppState, request: NewPaneRequest): PaneCreateParams | null {
  if (!state.workspace) return null;
  const tab = request.target === 'pane' ? selectActiveTab(state) : undefined;
  const worktree = request.cli === 'claude' ? request.worktree?.trim() : undefined;
  const prompt = request.cli === 'shell' ? undefined : request.prompt?.trim();
  return {
    workspace_id: state.workspace.id,
    cli: request.cli,
    cwd: request.cwd,
    ...(tab ? { tab_id: tab.id } : {}),
    ...(worktree ? { worktree: { name: worktree } } : {}),
    ...(prompt ? { prompt } : {}),
  };
}

function layoutOf(state: AppState): Layout {
  return {
    tabs: [...state.tabs],
    ...(state.activeTabId !== null ? { active_tab_id: state.activeTabId } : {}),
  };
}

/** Connects the store to plyd: the only place that calls ipc (spec 9.2). Returns a function that stops it all. */
export function startEffects(store: Store, options: EffectsOptions): () => void {
  const { client } = options;
  const layoutDelay = options.layoutSaveDelayMs ?? 250;
  const settingsDelay = options.settingsSaveDelayMs ?? 300;
  const noticeMs = options.noticeMs ?? 5_000;
  const timers = new Set<ReturnType<typeof setTimeout>>();
  let loadGeneration = 0;
  let loadEvents: Event[] | null = null;
  let layoutTimer: ReturnType<typeof setTimeout> | null = null;
  let settingsTimer: ReturnType<typeof setTimeout> | null = null;
  let stopped = false;

  const dispatch = (action: Action) => {
    if (!stopped) store.dispatch(action);
  };
  const notice = (text: string) => dispatch({ type: 'notice/show', text });
  const later = (ms: number, run: () => void) => {
    const t = setTimeout(() => {
      timers.delete(t);
      run();
    }, ms);
    timers.add(t);
    return t;
  };
  const failed = (what: string, error: unknown) => {
    log('warn', `${what} failed`, { error: message(error) });
    if (!(error instanceof RequestError && error.code === 'disconnected')) {
      notice(`${what} failed: ${message(error)}`);
    }
  };

  async function load(): Promise<void> {
    const generation = ++loadGeneration;
    let events: Event[] | null = null;
    try {
      const settings = await client.request('settings.get', {});
      try {
        await client.request('theme.set', { palette: terminalThemeFor(settings.accent) });
      } catch (error) {
        if (error instanceof RequestError && error.code === 'disconnected') throw error;
        failed('Setting the terminal colours', error);
      }
      const home = store.getState().env.home;
      const workspace =
        pickWorkspace(await client.request('workspace.list', {}), home) ??
        (await client.request('workspace.open', { path: home }));
      const ref = { workspace_id: workspace.id };
      // An event read with the pane.list answer is applied before the await resumes, so it is replayed after the load.
      events = [];
      loadEvents = events;
      const [layout, panes] = await Promise.all([
        client.request('layout.get', ref),
        client.request('pane.list', ref),
      ]);
      if (generation !== loadGeneration) return;
      dispatch({ type: 'session/loaded', workspace, panes, layout, settings });
      for (const event of events) dispatch({ type: 'daemon/event', event });
    } catch (error) {
      failed('Loading the session', error);
    } finally {
      if (loadEvents === events) loadEvents = null;
    }
  }

  function saveLayoutSoon(): void {
    if (layoutTimer) clearTimeout(layoutTimer);
    layoutTimer = later(layoutDelay, () => {
      layoutTimer = null;
      const state = store.getState();
      if (!state.workspace || state.connection.kind !== 'connected') return;
      client
        .request('layout.save', { workspace_id: state.workspace.id, layout: layoutOf(state) })
        .catch((error) => failed('Saving the layout', error));
    });
  }

  function saveSettingsSoon(): void {
    if (settingsTimer) clearTimeout(settingsTimer);
    settingsTimer = later(settingsDelay, () => {
      settingsTimer = null;
      const { settings, connection } = store.getState();
      if (connection.kind !== 'connected') return;
      client
        .request('settings.set', { settings })
        .catch((error) => failed('Saving the settings', error));
    });
  }

  function createPane(params: PaneCreateParams | null, fromForm: boolean): void {
    if (!params) {
      const text = 'plyd has not loaded a workspace yet';
      dispatch(
        fromForm ? { type: 'pane/createFailed', message: text } : { type: 'notice/show', text },
      );
      return;
    }
    client
      .request('pane.create', params)
      .then((pane) => dispatch({ type: 'pane/created', pane }))
      .catch((error) => {
        log('warn', 'pane.create failed', { cli: params.cli, error: message(error) });
        if (fromForm) dispatch({ type: 'pane/createFailed', message: message(error) });
        else failed('Opening a terminal', error);
      });
  }

  function closeFocused(prev: AppState): void {
    const pane = selectFocusedPane(prev);
    if (!pane || isAlive(pane)) return;
    client.request('pane.close', { pane_id: pane.id, kill: false }).catch((error) => {
      if (error instanceof RequestError && error.code === 'pane_alive') {
        dispatch({ type: 'overlay/open', overlay: { kind: 'close-confirm', paneId: pane.id } });
      } else {
        failed('Closing the pane', error);
      }
    });
  }

  function onAction(action: Action, next: AppState, prev: AppState): void {
    switch (action.type) {
      case 'pane/answer':
        client
          .request('pane.answer', { pane_id: action.paneId, choice: action.choice })
          .catch((error) => failed('Answering the pane', error));
        break;
      case 'pane/resume':
        client
          .request('pane.resume', { pane_id: action.paneId })
          .then((pane) => dispatch({ type: 'pane/resumed', pane }))
          .catch((error) => failed('Resuming the session', error));
        break;
      case 'pane/create':
        createPane(createParams(next, action.request), true);
        break;
      case 'pane/closeConfirmed':
        client
          .request('pane.close', { pane_id: action.paneId, kill: true })
          .catch((error) => failed('Closing the pane', error));
        break;
      case 'command':
        if (action.id === 'pane.terminalHere') {
          const cwd = selectFocusedPane(prev)?.cwd ?? next.workspace?.path;
          const params =
            cwd === undefined
              ? null
              : createParams(next, {
                  target: next.tabs.length > 0 ? 'pane' : 'tab',
                  cli: 'shell',
                  cwd,
                });
          createPane(params, false);
        } else if (action.id === 'pane.close') {
          closeFocused(prev);
        }
        break;
      default:
        break;
    }
    if (action.type === 'session/loaded') return;
    if (next.settings !== prev.settings) {
      saveSettingsSoon();
      if (next.settings.accent !== prev.settings.accent && next.connection.kind === 'connected') {
        client
          .request('theme.set', { palette: terminalThemeFor(next.settings.accent) })
          .catch((error) => failed('Setting the terminal colours', error));
      }
    }
    if (next.tabs !== prev.tabs || next.activeTabId !== prev.activeTabId) saveLayoutSoon();
    if (next.notice && next.notice !== prev.notice) {
      const id = next.notice.id;
      later(noticeMs, () => dispatch({ type: 'notice/clear', id }));
    }
  }

  const offState = client.onState((state) => {
    dispatch({ type: 'connection/changed', state });
    if (state.kind === 'connected') void load();
    else loadGeneration++;
  });
  const offEvent = client.onEvent((event) => {
    dispatch({ type: 'daemon/event', event });
    if (event.e !== 'daemon.stopping') loadEvents?.push(event);
  });
  const offEffect = store.addEffect((action, next, prev) => {
    try {
      onAction(action, next, prev);
    } catch (error) {
      log('error', 'effect failed', { action: action.type, error: message(error) });
    }
  });
  client.start();
  (options.reducedMotion ?? readReducedMotion)()
    .then((value) => dispatch({ type: 'env/reducedMotion', value }))
    .catch((error) => log('warn', 'reading reduce motion failed', { error: message(error) }));

  return () => {
    stopped = true;
    for (const t of timers) clearTimeout(t);
    timers.clear();
    offState();
    offEvent();
    offEffect();
    client.stop();
  };
}
