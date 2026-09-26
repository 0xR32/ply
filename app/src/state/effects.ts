import { type ControlClient, RequestError } from '../ipc/control-client';
import { completeDir, type DirSources, type DirVisit, recentDirs, scanRepos } from '../ipc/dirs';
import { log } from '../ipc/log';
import { readBuildId, readKeyRepeat, readReducedMotion } from '../ipc/os';
import type { Layout, PaneCreateParams, Workspace } from '../ipc/proto.gen';
import { terminalThemeFor } from '../theme/tokens';
import type { Action, Event, KeyRepeat, NewPaneRequest } from './actions';
import type { AppState } from './reducer';
import {
  FULL_TAB_NOTICE,
  isAlive,
  isTabFull,
  pathQuery,
  selectActiveTab,
  selectFocusedPane,
} from './selectors';
import type { Store } from './store';

/** Timing and OS hooks of the effects; tests shorten the delays and stub the OS reads and the quit. */
export interface EffectsOptions {
  client: ControlClient;
  layoutSaveDelayMs?: number;
  settingsSaveDelayMs?: number;
  noticeMs?: number;
  reducedMotion?: () => Promise<boolean>;
  /** The app's build id (`readBuildId`); `null` when unknown. */
  buildId?: () => Promise<string | null>;
  /** macOS's key-repeat timing (`readKeyRepeat`) for the ⌘U hold. */
  keyRepeat?: () => Promise<Partial<KeyRepeat>>;
  /** How often `usage.get` is asked again while the usage view is shown; 5 s, plyd's own cache time. */
  usageRefreshMs?: number;
  /** Ends the app after "Quit ply and stop sessions"; defaults to `process.exit(0)`, as GPUIX does when the window closes. */
  quit?: () => void;
  /** Where the new-pane form's folder suggestions come from; defaults to `ipc/dirs.ts`. */
  dirs?: DirSources;
  /** The native folder picker for "Browse…": the chosen folder, or `null` when cancelled; without it Browse does nothing. */
  promptForDirectory?: () => Promise<string | null>;
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** What the task form says for a failed request; a plyd from before the queue answers `unknown_method`. */
function queueMessage(error: unknown): string {
  if (error instanceof RequestError && error.code === 'unknown_method') {
    return 'This plyd predates the task queue: rebuild it, then Restart plyd';
  }
  return message(error);
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
  const quit = options.quit ?? (() => process.exit(0));
  const layoutDelay = options.layoutSaveDelayMs ?? 250;
  const settingsDelay = options.settingsSaveDelayMs ?? 300;
  const noticeMs = options.noticeMs ?? 5_000;
  const usageRefreshMs = options.usageRefreshMs ?? 5_000;
  const timers = new Set<ReturnType<typeof setTimeout>>();
  let loadGeneration = 0;
  let loadEvents: Event[] | null = null;
  let layoutTimer: ReturnType<typeof setTimeout> | null = null;
  let settingsTimer: ReturnType<typeof setTimeout> | null = null;
  let stopped = false;
  const dirs = options.dirs ?? { recentDirs, scanRepos, completeDir };
  let scanning = false;
  let listing: string | null = null;
  let listGeneration = 0;
  let usageTimer: ReturnType<typeof setTimeout> | null = null;
  let usageAsking = false;

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
      const [layout, panes, tasks] = await Promise.all([
        client.request('layout.get', ref),
        client.request('pane.list', ref),
        client.request('task.list', ref).catch((error) => {
          if (error instanceof RequestError && error.code === 'unknown_method') {
            log('info', 'this plyd predates the task queue');
            return null;
          }
          if (error instanceof RequestError && error.code === 'disconnected') throw error;
          failed('Loading the task queue', error);
          return null;
        }),
      ]);
      if (generation !== loadGeneration) return;
      dispatch({ type: 'session/loaded', workspace, panes, layout, settings });
      dispatch({ type: 'tasks/loaded', list: tasks });
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

  /** The form opened: recent folders from its panes and stored sessions, and a background rescan of the home directory unless one runs. */
  function refreshDirs(state: AppState): void {
    listing = null;
    const panes = Object.values(state.panes);
    const workspace = state.workspace;
    const sessions: Promise<DirVisit[]> = workspace
      ? client
          .request('session.list', { workspace_id: workspace.id, include_closed: true })
          .catch((error) => {
            log('warn', 'session.list for the recent folders failed', { error: message(error) });
            return [];
          })
      : Promise.resolve([]);
    sessions
      .then((list) => dirs.recentDirs(list, panes))
      .then((paths) => dispatch({ type: 'dirs/recent', paths }))
      .catch((error) =>
        log('warn', 'reading the recent folders failed', { error: message(error) }),
      );
    if (scanning) return;
    scanning = true;
    dirs
      .scanRepos(state.env.home)
      .then((scan) => {
        log('info', 'scanned the home directory for repositories', {
          repos: scan.repos.length,
          visited: scan.visited,
          ms: scan.elapsedMs,
          truncated: scan.truncated,
        });
        dispatch({ type: 'dirs/repos', paths: scan.repos });
      })
      .catch((error) => log('warn', 'the repository scan failed', { error: message(error) }))
      .finally(() => {
        scanning = false;
      });
  }

  /** Lists the folder a path query names, unless it is the one listed last; only the newest listing is kept. */
  function completeQuery(state: AppState): void {
    const query = pathQuery(state.dirs.query, state.env.home, state.dirs.base);
    if (!query || query.dir === listing) return;
    listing = query.dir;
    const generation = ++listGeneration;
    dirs
      .completeDir(query.dir)
      .then((listed) => {
        if (generation === listGeneration) dispatch({ type: 'dirs/completion', ...listed });
      })
      .catch((error) => log('warn', 'listing a folder failed', { error: message(error) }));
  }

  function browse(): void {
    const prompt = options.promptForDirectory;
    if (!prompt) return;
    prompt()
      .then((path) => {
        if (path) dispatch({ type: 'dirs/accept', path });
      })
      .catch((error) => failed('Opening the folder picker', error));
  }

  /** Asks plyd for the CLIs' usage now and again every `usageRefreshMs` while the view is shown; one request at a time. */
  function refreshUsage(): void {
    usageTimer = null;
    if (!store.getState().usage.shown) return;
    usageTimer = later(usageRefreshMs, refreshUsage);
    if (usageAsking) return;
    usageAsking = true;
    client
      .request('usage.get', {})
      .then((usage) => dispatch({ type: 'usage/loaded', usage }))
      .catch((error) => {
        log('warn', 'usage.get failed', { error: message(error) });
        const text =
          error instanceof RequestError && error.code === 'unknown_method'
            ? 'This plyd predates usage.get: rebuild it, then Restart plyd'
            : message(error);
        dispatch({ type: 'usage/failed', message: text });
      })
      .finally(() => {
        usageAsking = false;
      });
  }

  function stopUsage(): void {
    if (!usageTimer) return;
    clearTimeout(usageTimer);
    timers.delete(usageTimer);
    usageTimer = null;
  }

  /** The dispatch form's submit: `task.add` for a pane or a pool, `pane.create` with the text as first prompt for a new pane. */
  function addTask(state: AppState, action: Extract<Action, { type: 'task/add' }>): void {
    const workspace = state.workspace;
    if (!workspace) {
      dispatch({ type: 'task/addFailed', message: 'plyd has not loaded a workspace yet' });
      return;
    }
    const target = action.target;
    if (target.kind === 'new') {
      const tab = selectActiveTab(state);
      const params = createParams(state, {
        target: tab && !isTabFull(tab) ? 'pane' : 'tab',
        cli: target.cli,
        cwd: target.cwd,
        prompt: action.text,
      });
      if (!params) return;
      client
        .request('pane.create', params)
        .then((pane) => {
          dispatch({ type: 'pane/created', pane });
          dispatch({ type: 'task/opened' });
        })
        .catch((error) => {
          log('warn', 'pane.create for a task failed', { error: message(error) });
          dispatch({ type: 'task/addFailed', message: message(error) });
        });
      return;
    }
    client
      .request('task.add', {
        workspace_id: workspace.id,
        target:
          target.kind === 'pane'
            ? { pane: target.paneId }
            : { pool: { cli: target.cli, cwd: target.cwd } },
        text: action.text,
        ...(action.skill ? { skill: action.skill } : {}),
      })
      .then((task) => dispatch({ type: 'task/added', task }))
      .catch((error) => {
        log('warn', 'task.add failed', { error: message(error) });
        dispatch({ type: 'task/addFailed', message: queueMessage(error) });
      });
  }

  function askSkills(action: Extract<Action, { type: 'skills/query' }>): void {
    const key = `${action.cli} ${action.cwd}`;
    client
      .request('skill.list', { cli: action.cli, cwd: action.cwd })
      .then((list) => dispatch({ type: 'skills/loaded', key, list }))
      .catch((error) => {
        log('warn', 'skill.list failed', { error: message(error) });
        dispatch({ type: 'skills/failed', key, message: queueMessage(error) });
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
          .request('pane.answer', { pane_id: action.paneId, answer: action.answer })
          .catch((error) => failed('Answering the pane', error));
        break;
      case 'pane/resume':
        client
          .request('pane.resume', { pane_id: action.paneId })
          .then((pane) => dispatch({ type: 'pane/resumed', pane }))
          .catch((error) => failed('Resuming the session', error));
        break;
      case 'pane/create':
        if (action.request.target === 'pane' && isTabFull(selectActiveTab(next))) {
          dispatch({ type: 'pane/createFailed', message: FULL_TAB_NOTICE });
        } else {
          createPane(createParams(next, action.request), true);
        }
        break;
      case 'dirs/browse':
        browse();
        break;
      case 'task/add':
        addTask(next, action);
        break;
      case 'task/cancel':
        client
          .request('task.cancel', { task_id: action.taskId })
          .catch((error) => failed('Cancelling the task', error));
        break;
      case 'task/move':
        client
          .request('task.move', { task_id: action.taskId, position: action.position })
          .catch((error) => failed('Moving the task', error));
        break;
      case 'task/send':
        client
          .request('task.send', { task_id: action.taskId })
          .catch((error) => failed('Sending the task', error));
        break;
      case 'queue/pause':
        client
          .request('queue.pause', { pane_id: action.paneId, paused: action.paused })
          .catch((error) =>
            failed(action.paused ? 'Pausing the queue' : 'Resuming the queue', error),
          );
        break;
      case 'skills/query':
        askSkills(action);
        break;
      case 'pane/closeConfirmed':
        client
          .request('pane.close', { pane_id: action.paneId, kill: true })
          .catch((error) => failed('Closing the pane', error));
        break;
      case 'daemon/restart':
        client
          .request('daemon.shutdown', { kill_panes: false })
          .then(() => log('info', 'plyd stops; the next connect starts it again'))
          .catch((error) => failed('Restarting plyd', error));
        break;
      case 'daemon/quit':
        client
          .request('daemon.shutdown', { kill_panes: true })
          .then(() => {
            log('info', 'plyd stops every session; quitting');
            quit();
          })
          .catch((error) => {
            // With plyd gone there is no session left to stop.
            if (error instanceof RequestError && error.code === 'disconnected') quit();
            else failed('Stopping the sessions', error);
          });
        break;
      case 'command':
        if (action.id === 'pane.terminalHere') {
          if (isTabFull(selectActiveTab(next))) break;
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
    if (next.usage.shown && !prev.usage.shown) refreshUsage();
    else if (!next.usage.shown && prev.usage.shown) stopUsage();
    if (next.overlay?.kind === 'new-pane') {
      const opened = prev.overlay?.kind !== 'new-pane';
      if (opened) refreshDirs(next);
      if (opened || next.dirs.query !== prev.dirs.query) completeQuery(next);
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
  (options.buildId ?? readBuildId)()
    .then((value) => dispatch({ type: 'env/buildId', value }))
    .catch((error) => log('warn', 'reading the build id failed', { error: message(error) }));
  (options.keyRepeat ?? readKeyRepeat)()
    .then((value) => {
      log('info', 'key repeat', value);
      dispatch({ type: 'env/keyRepeat', value });
    })
    .catch((error) => log('warn', 'reading the key repeat failed', { error: message(error) }));

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
