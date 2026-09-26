import { baseFontSize, fontSizeRange } from '../theme/tokens';
import type {
  Action,
  CommandId,
  ConnectionState,
  Event,
  KeyRepeat,
  Layout,
  Overlay,
  Pane,
  QueueState,
  Settings,
  Skill,
  Split,
  Tab,
  Task,
  TaskList,
  Usage,
  Workspace,
} from './actions';
import {
  abbreviateHome,
  FULL_TAB_NOTICE,
  isAlive,
  isTabFull,
  nextWaitingPane,
  type PaneDirection,
  paneNeighbour,
  selectActiveTab,
  selectFocusedPane,
  tabName,
} from './selectors';

/** A pane as the app holds it: the C1 record plus what only the app observes. */
export interface PaneState extends Pane {
  /** Unix seconds when the current status began (`pane.status.at`); absent for panes loaded after a reconnect. */
  statusSince?: number;
  /** Title the terminal set (C2 TITLE through TerminalView); shown in place of `title` when present. */
  terminalTitle?: string;
  /** The program rang the bell while the pane was not the focused one; cleared once it is (the header shows a bell). */
  bell?: boolean;
}

/** A transient message shown in the status bar; `id` lets a delayed clear skip a newer notice. */
export interface Notice {
  id: number;
  text: string;
}

/** Progress of the new-pane form's `pane.create`: pending while the request runs, `error` after a failure. */
export interface CreateStatus {
  pending: boolean;
  error: string | null;
}

/** The new-pane form's directory search (Ruling R57): the field, its list and the folders it suggests. */
export interface DirSearch {
  /** The Directory field's text as typed; opening the form sets it to the start folder, ~-abbreviated. */
  query: string;
  /** The folder the form opened in (absolute): a query starting with `.` is resolved against it. */
  base: string;
  /** Whether the suggestion list is open: typing opens it; accepting, esc and Tab close it. */
  open: boolean;
  /** Recent folders, most recent first (absolute, each existed when read). */
  recent: readonly string[];
  /** Git repositories under the home directory from the last scan (absolute); kept for the app's lifetime. */
  repos: readonly string[];
  /** The deepest existing folder of the last path query and its sub-folders (absolute). */
  completion: { dir: string; children: readonly string[] } | null;
}

/** Facts about the machine the app reads once at start. */
export interface Environment {
  home: string;
  shellName: string;
  geistAvailable: boolean;
  /** The app's build id, `<version>+<commit>` of the checkout it runs from; absent until read, or when git cannot tell. */
  buildId?: string;
  /** macOS's key-repeat timing as far as `defaults` sets it; the ⌘U hold assumes macOS's defaults for the rest. */
  keyRepeat?: Partial<KeyRepeat>;
}

/** The hold-⌘U usage view (Ruling R59): on screen while ⌘U is held, showing plyd's last `usage.get` answer. */
export interface UsageView {
  shown: boolean;
  /** The last answer, kept between holds so the next one shows it at once; `null` before the first. */
  usage: Usage | null;
  /** Why the last `usage.get` failed, until an answer replaces it. */
  error: string | null;
}

/** The task queue as plyd reports it (Ruling R60): this workspace's tasks by id and the queues that are paused or blocked. */
export interface TaskQueueView {
  /** False until `task.list` answered, and for a plyd that predates the queue. */
  available: boolean;
  tasks: Readonly<Record<number, Task>>;
  queues: Readonly<Record<number, QueueState>>;
}

/** The skills the dispatch form lists: the last `skill.list` answer for `key` (`cli cwd`). */
export interface SkillsView {
  key: string | null;
  list: readonly Skill[];
  loading: boolean;
  error: string | null;
}

/** Progress of the dispatch form's submit: pending while `task.add` runs, `error` after a failure. */
export interface DispatchStatus {
  pending: boolean;
  error: string | null;
}

/** Finished tasks the app keeps per workspace, as plyd does. */
export const KEEP_FINISHED_TASKS = 200;

/** The whole app state; tabs are in bar order with `position` equal to their index. */
export interface AppState {
  connection: ConnectionState;
  workspace: Workspace | null;
  panes: Readonly<Record<number, PaneState>>;
  tabs: readonly Tab[];
  activeTabId: number | null;
  overlay: Overlay | null;
  create: CreateStatus;
  dirs: DirSearch;
  settings: Settings;
  notice: Notice | null;
  /** Pane sizes dragged per tab, kept in memory; one that no longer fits the tab's shape is ignored (`paneSplit`). */
  splits: Readonly<Record<number, Split>>;
  usage: UsageView;
  tasks: TaskQueueView;
  skills: SkillsView;
  taskForm: DispatchStatus;
  reducedMotion: boolean;
  env: Environment;
}

/** plyd's defaults (ply-proto `Settings::default`), shown until `settings.get` answers. */
export const defaultSettings: Settings = {
  accent: 'blue',
  option_as_meta: 'off',
  keep_awake_while_running: true,
  resume_sessions_on_start: true,
  use_ply_colours_in_claude: true,
  codex_plan_tool: true,
  scrollback_lines: 10_000,
  font_size: baseFontSize,
};

/** The state before the first connection: no workspace, no panes, default settings. */
export function initialState(env: Environment): AppState {
  return {
    connection: { kind: 'connecting' },
    workspace: null,
    panes: {},
    tabs: [],
    activeTabId: null,
    overlay: null,
    create: { pending: false, error: null },
    dirs: { query: '', base: env.home, open: false, recent: [], repos: [], completion: null },
    settings: defaultSettings,
    notice: null,
    splits: {},
    usage: { shown: false, usage: null, error: null },
    tasks: { available: false, tasks: {}, queues: {} },
    skills: { key: null, list: [], loading: false, error: null },
    taskForm: { pending: false, error: null },
    reducedMotion: false,
    env,
  };
}

function renumber(tabs: Tab[]): Tab[] {
  return tabs.map((t, i) => (t.position === i ? t : { ...t, position: i }));
}

function fixFocus(tab: Tab): Tab {
  if (tab.focus_pane_id !== undefined && tab.pane_ids.includes(tab.focus_pane_id)) return tab;
  const first = tab.pane_ids[0];
  if (first === undefined) {
    const { focus_pane_id: _drop, ...rest } = tab;
    return rest;
  }
  return { ...tab, focus_pane_id: first };
}

function placePane(tabs: readonly Tab[], pane: Pane, home: string): readonly Tab[] {
  const at = tabs.findIndex((t) => t.id === pane.tab_id);
  if (at < 0) {
    const tab: Tab = {
      id: pane.tab_id,
      name: tabName(pane.cwd, home),
      position: tabs.length,
      pane_ids: [pane.id],
      focus_pane_id: pane.id,
      zoomed: false,
    };
    return [...tabs, tab];
  }
  const tab = tabs[at] as Tab;
  if (tab.pane_ids.includes(pane.id)) return tabs;
  const ids = [...tab.pane_ids];
  ids.splice(Math.min(Math.max(pane.position, 0), ids.length), 0, pane.id);
  const next = [...tabs];
  next[at] = fixFocus({ ...tab, pane_ids: ids });
  return next;
}

function neighbour<T>(list: readonly T[], index: number): T | undefined {
  return list[Math.min(index, list.length - 1)];
}

function removePane(state: AppState, paneId: number): AppState {
  if (!(paneId in state.panes)) return state;
  const { [paneId]: _removed, ...panes } = state.panes;
  const oldIndex = state.tabs.findIndex((t) => t.pane_ids.includes(paneId));
  let tabs = state.tabs.map((t) => {
    const at = t.pane_ids.indexOf(paneId);
    if (at < 0) return t;
    const ids = t.pane_ids.filter((id) => id !== paneId);
    const focus = t.focus_pane_id === paneId ? neighbour(ids, at) : t.focus_pane_id;
    const zoomed = t.focus_pane_id === paneId ? false : t.zoomed;
    return fixFocus({ ...t, pane_ids: ids, focus_pane_id: focus, zoomed });
  });
  tabs = renumber(tabs.filter((t) => t.pane_ids.length > 0));
  let activeTabId = state.activeTabId;
  if (activeTabId !== null && !tabs.some((t) => t.id === activeTabId)) {
    activeTabId = neighbour(tabs, Math.max(oldIndex, 0))?.id ?? null;
  }
  const overlay =
    state.overlay?.kind === 'close-confirm' && state.overlay.paneId === paneId
      ? null
      : state.overlay;
  return { ...state, panes, tabs, activeTabId, overlay };
}

function updatePane(state: AppState, paneId: number, patch: (p: PaneState) => PaneState): AppState {
  const pane = state.panes[paneId];
  if (!pane) return state;
  return { ...state, panes: { ...state.panes, [paneId]: patch(pane) } };
}

function focusPane(state: AppState, paneId: number): AppState {
  const tab = state.tabs.find((t) => t.pane_ids.includes(paneId));
  if (!tab) return state;
  if (tab.focus_pane_id === paneId && state.activeTabId === tab.id) return state;
  const tabs =
    tab.focus_pane_id === paneId
      ? state.tabs
      : state.tabs.map((t) => (t.id === tab.id ? { ...t, focus_pane_id: paneId } : t));
  return { ...state, tabs, activeTabId: tab.id };
}

function upsertPane(state: AppState, pane: Pane): AppState {
  if (pane.closed_at !== undefined) return state;
  if (state.workspace && pane.workspace_id !== state.workspace.id) return state;
  const previous = state.panes[pane.id];
  const merged: PaneState = previous?.terminalTitle
    ? { ...pane, terminalTitle: previous.terminalTitle }
    : { ...pane };
  const tabs = placePane(state.tabs, pane, state.env.home);
  const activeTabId = state.activeTabId ?? tabs[0]?.id ?? null;
  return { ...state, panes: { ...state.panes, [pane.id]: merged }, tabs, activeTabId };
}

/** A pane a response carries (`pane.create`, `pane.resume`): events sent after plyd built it may already be applied, so a known pane takes only its `cli` and `title`. */
function takeResponsePane(state: AppState, pane: Pane): AppState {
  const known = state.panes[pane.id];
  if (!known) return upsertPane(state, pane);
  if (known.cli === pane.cli && known.title === pane.title) return state;
  const next: PaneState = { ...known, cli: pane.cli, title: pane.title };
  return { ...state, panes: { ...state.panes, [pane.id]: next } };
}

function applyEvent(state: AppState, event: Event): AppState {
  switch (event.e) {
    case 'pane.added':
      return upsertPane(state, event.p);
    case 'pane.removed':
      return removePane(state, event.p.pane_id);
    case 'pane.status': {
      const { pane_id, status, detail, exit_code, at } = event.p;
      return updatePane(state, pane_id, (p) => {
        // Both belong to the previous status: an event without them must not keep a stale detail or exit code.
        const { detail: _d, exit_code: _e, ...rest } = p;
        return {
          ...rest,
          status,
          statusSince: at,
          ...(detail !== undefined ? { detail } : {}),
          ...(exit_code !== undefined ? { exit_code } : {}),
        };
      });
    }
    case 'pane.progress': {
      const { pane_id, progress } = event.p;
      return updatePane(state, pane_id, (p) => {
        const { progress: _old, ...rest } = p;
        return progress ? { ...rest, progress } : rest;
      });
    }
    case 'pane.meta': {
      const { pane_id, model, worktree, cwd, branch, project, git_worktree } = event.p;
      return updatePane(state, pane_id, (p) => {
        // plyd sends the live worktree with every pane.meta, so its absence means the pane left the worktree.
        const {
          branch: oldBranch,
          project: oldProject,
          git_worktree: oldGitWorktree,
          worktree_seen: _left,
          ...rest
        } = p;
        // git's labels arrive a moment after a directory change, so in the same directory the old ones stand.
        const same = cwd === p.cwd;
        const keptBranch = branch ?? (same ? oldBranch : undefined);
        const keptProject = project ?? (same ? oldProject : undefined);
        const keptGitWorktree = git_worktree ?? (same ? oldGitWorktree : undefined);
        return {
          ...rest,
          cwd,
          ...(model !== undefined ? { model_seen: model } : {}),
          ...(worktree !== undefined ? { worktree_seen: worktree } : {}),
          ...(keptBranch !== undefined ? { branch: keptBranch } : {}),
          ...(keptProject !== undefined ? { project: keptProject } : {}),
          ...(keptGitWorktree !== undefined ? { git_worktree: keptGitWorktree } : {}),
        };
      });
    }
    case 'pane.exit': {
      const { pane_id, code, at } = event.p;
      return updatePane(state, pane_id, (p) => ({
        ...p,
        status: 'exited',
        exit_code: code,
        statusSince: at,
      }));
    }
    case 'daemon.stopping':
      return showNotice(
        state,
        event.p.kill_panes ? 'plyd is stopping its sessions' : 'plyd is restarting',
      );
    case 'task.changed':
      return upsertTask(state, event.p);
    case 'queue.changed':
      return setQueue(state, event.p);
  }
}

function isFinished(task: Task): boolean {
  return task.state === 'ended' || task.state === 'failed' || task.state === 'cancelled';
}

/** Drops finished tasks beyond the newest `KEEP_FINISHED_TASKS`, as plyd's history does. */
function pruneTasks(tasks: Record<number, Task>): Record<number, Task> {
  const finished = Object.values(tasks).filter(isFinished);
  if (finished.length <= KEEP_FINISHED_TASKS) return tasks;
  finished.sort((a, b) => (a.ended_at ?? 0) - (b.ended_at ?? 0) || a.id - b.id);
  const next = { ...tasks };
  for (const t of finished.slice(0, finished.length - KEEP_FINISHED_TASKS)) delete next[t.id];
  return next;
}

function upsertTask(state: AppState, task: Task): AppState {
  if (state.workspace && task.workspace_id !== state.workspace.id) return state;
  const tasks = pruneTasks({ ...state.tasks.tasks, [task.id]: task });
  return { ...state, tasks: { ...state.tasks, tasks } };
}

function setQueue(state: AppState, queue: QueueState): AppState {
  const { [queue.pane_id]: _old, ...rest } = state.tasks.queues;
  const queues = queue.paused || queue.blocked ? { ...rest, [queue.pane_id]: queue } : rest;
  return { ...state, tasks: { ...state.tasks, queues } };
}

function loadTasks(state: AppState, list: TaskList | null): AppState {
  if (!list) return { ...state, tasks: { available: false, tasks: {}, queues: {} } };
  const tasks: Record<number, Task> = {};
  for (const t of list.tasks) {
    if (!state.workspace || t.workspace_id === state.workspace.id) tasks[t.id] = t;
  }
  const queues: Record<number, QueueState> = {};
  for (const q of list.queues) queues[q.pane_id] = q;
  return { ...state, tasks: { available: true, tasks, queues } };
}

/** The dispatch form, on the focused pane when it runs an agent. */
function openDispatch(state: AppState): AppState {
  const pane = selectFocusedPane(state);
  const overlay: Overlay =
    pane && pane.cli !== 'shell' ? { kind: 'dispatch', paneId: pane.id } : { kind: 'dispatch' };
  return { ...state, overlay, taskForm: { pending: false, error: null } };
}

function loadSession(
  state: AppState,
  workspace: Workspace,
  list: Pane[],
  layout: Layout,
  settings: Settings,
): AppState {
  const open = list.filter((p) => p.closed_at === undefined && p.workspace_id === workspace.id);
  const panes: Record<number, PaneState> = {};
  for (const p of open) {
    const known = state.panes[p.id];
    panes[p.id] = {
      ...p,
      ...(known?.terminalTitle ? { terminalTitle: known.terminalTitle } : {}),
      ...(known?.statusSince !== undefined && known.status === p.status
        ? { statusSince: known.statusSince }
        : {}),
    };
  }
  const placed = new Set<number>();
  const fromLayout: Tab[] = [];
  for (const t of [...layout.tabs].sort((a, b) => a.position - b.position)) {
    const ids = t.pane_ids.filter((id) => panes[id]?.tab_id === t.id && !placed.has(id));
    for (const id of ids) placed.add(id);
    if (ids.length > 0) fromLayout.push(fixFocus({ ...t, pane_ids: ids }));
  }
  const rest = open
    .filter((p) => !placed.has(p.id))
    .sort((a, b) => a.tab_id - b.tab_id || a.position - b.position || a.id - b.id);
  let withRest: readonly Tab[] = fromLayout;
  for (const p of rest) withRest = placePane(withRest, p, state.env.home);
  const tabs = renumber([...withRest]);
  const wanted = layout.active_tab_id;
  const activeTabId =
    wanted !== undefined && tabs.some((t) => t.id === wanted) ? wanted : (tabs[0]?.id ?? null);
  return { ...state, workspace, panes, tabs, activeTabId, settings };
}

function showNotice(state: AppState, text: string): AppState {
  return { ...state, notice: { id: (state.notice?.id ?? 0) + 1, text } };
}

function cycle<T>(list: readonly T[], current: T | undefined, delta: number): T | undefined {
  if (list.length === 0) return undefined;
  const at = current === undefined ? -1 : list.indexOf(current);
  const from = at < 0 ? (delta > 0 ? -1 : 0) : at;
  return list[(from + delta + list.length) % list.length];
}

function withFontSize(state: AppState, size: number): AppState {
  const clamped = Math.min(fontSizeRange.max, Math.max(fontSizeRange.min, size));
  if (clamped === state.settings.font_size) return state;
  return { ...state, settings: { ...state.settings, font_size: clamped } };
}

/** Opens the new-pane form with its Directory field on the focused pane's folder, else the workspace's, else home. */
function openNewPane(state: AppState, target: 'pane' | 'tab'): AppState {
  const start = selectFocusedPane(state)?.cwd ?? state.workspace?.path ?? state.env.home;
  return {
    ...state,
    overlay: { kind: 'new-pane', target },
    create: { pending: false, error: null },
    dirs: {
      ...state.dirs,
      query: abbreviateHome(start, state.env.home),
      base: start,
      open: false,
      completion: null,
    },
  };
}

function reduceDirs(state: AppState, dirs: Partial<DirSearch>): AppState {
  return { ...state, dirs: { ...state.dirs, ...dirs } };
}

function activeTab(state: AppState): Tab | undefined {
  return state.tabs.find((t) => t.id === state.activeTabId);
}

function runCommand(state: AppState, id: CommandId): AppState {
  const tab = activeTab(state);
  switch (id) {
    case 'palette.open':
      return { ...state, overlay: { kind: 'palette' } };
    case 'pane.new':
      if (isTabFull(tab)) return showNotice(state, FULL_TAB_NOTICE);
      return openNewPane(state, tab ? 'pane' : 'tab');
    case 'tab.new':
      return openNewPane(state, 'tab');
    case 'settings.open':
      return { ...state, overlay: { kind: 'settings' } };
    case 'pane.nextWaiting': {
      const next = nextWaitingPane(state);
      return next ? focusPane(state, next) : showNotice(state, 'Nothing needs you');
    }
    case 'pane.prev':
    case 'pane.next': {
      if (!tab) return state;
      const next = cycle(tab.pane_ids, tab.focus_pane_id, id === 'pane.next' ? 1 : -1);
      return next === undefined ? state : focusPane(state, next);
    }
    case 'pane.left':
    case 'pane.right':
    case 'pane.up':
    case 'pane.down': {
      const focusId = tab?.focus_pane_id;
      if (!tab || focusId === undefined) return state;
      const next = paneNeighbour(tab, focusId, id.slice('pane.'.length) as PaneDirection);
      return next === undefined ? state : focusPane(state, next);
    }
    case 'tab.prev':
    case 'tab.next': {
      const next = cycle(state.tabs, tab, id === 'tab.next' ? 1 : -1);
      return next ? { ...state, activeTabId: next.id } : state;
    }
    case 'pane.zoom':
      if (!tab || tab.pane_ids.length === 0) return state;
      return {
        ...state,
        tabs: state.tabs.map((t) => (t.id === tab.id ? { ...t, zoomed: !t.zoomed } : t)),
      };
    case 'pane.close': {
      const paneId = tab?.focus_pane_id;
      const pane = paneId === undefined ? undefined : state.panes[paneId];
      if (!pane || !isAlive(pane)) return state;
      return { ...state, overlay: { kind: 'close-confirm', paneId: pane.id } };
    }
    case 'font.up':
      return withFontSize(state, state.settings.font_size + fontSizeRange.step);
    case 'font.down':
      return withFontSize(state, state.settings.font_size - fontSizeRange.step);
    case 'font.reset':
      return withFontSize(state, baseFontSize);
    case 'pane.terminalHere':
      return isTabFull(tab) ? showNotice(state, FULL_TAB_NOTICE) : state;
    case 'usage.show':
      return state.usage.shown ? state : { ...state, usage: { ...state.usage, shown: true } };
    case 'task.dispatch':
      return openDispatch(state);
    case 'task.queue':
      return { ...state, overlay: { kind: 'queue' } };
    default: {
      const digit = Number(id.slice('tab.go.'.length));
      const target = state.tabs[digit - 1];
      return target ? { ...state, activeTabId: target.id } : state;
    }
  }
}

/** The pure state transition; returns `state` itself when an action changes nothing. */
export function reduce(state: AppState, action: Action): AppState {
  return clearSeenBell(reduceAction(state, action));
}

/** Drops the bell of the pane the user now looks at: the active tab's focused pane. */
function clearSeenBell(state: AppState): AppState {
  const id = selectActiveTab(state)?.focus_pane_id;
  const pane = id === undefined ? undefined : state.panes[id];
  if (id === undefined || !pane?.bell) return state;
  const { bell: _seen, ...rest } = pane;
  return { ...state, panes: { ...state.panes, [id]: rest } };
}

function reduceAction(state: AppState, action: Action): AppState {
  switch (action.type) {
    case 'connection/changed':
      return { ...state, connection: action.state };
    case 'session/loaded':
      return loadSession(state, action.workspace, action.panes, action.layout, action.settings);
    case 'daemon/event':
      return applyEvent(state, action.event);
    case 'command':
      return runCommand(state, action.id);
    case 'tab/select':
      return state.tabs.some((t) => t.id === action.tabId)
        ? { ...state, activeTabId: action.tabId }
        : state;
    case 'pane/focus':
      return focusPane(state, action.paneId);
    case 'pane/title':
      return updatePane(state, action.paneId, (p) => ({ ...p, terminalTitle: action.title }));
    case 'pane/bell':
      return selectActiveTab(state)?.focus_pane_id === action.paneId
        ? state
        : updatePane(state, action.paneId, (p) => (p.bell ? p : { ...p, bell: true }));
    case 'pane/exited':
      return updatePane(state, action.paneId, (p) => {
        if (p.status === 'exited') return p;
        const { detail: _d, ...rest } = p;
        return { ...rest, status: 'exited', exit_code: action.code, statusSince: action.at };
      });
    case 'pane/answer':
    case 'pane/resume':
      return state;
    case 'pane/resumed':
      return takeResponsePane(state, action.pane);
    case 'pane/create':
      return { ...state, create: { pending: true, error: null } };
    case 'pane/created': {
      const next = focusPane(takeResponsePane(state, action.pane), action.pane.id);
      const overlay = next.overlay?.kind === 'new-pane' ? null : next.overlay;
      return { ...next, overlay, create: { pending: false, error: null } };
    }
    case 'pane/createFailed':
      return { ...state, create: { pending: false, error: action.message } };
    case 'pane/closeConfirmed':
      return state.overlay?.kind === 'close-confirm' ? { ...state, overlay: null } : state;
    case 'dirs/query':
      return state.dirs.query === action.query && state.dirs.open
        ? state
        : reduceDirs(state, { query: action.query, open: true });
    case 'dirs/accept':
      return reduceDirs(state, { query: abbreviateHome(action.path, state.env.home), open: false });
    case 'dirs/close':
      return state.dirs.open ? reduceDirs(state, { open: false }) : state;
    case 'dirs/browse':
      return state;
    case 'dirs/recent':
      return reduceDirs(state, { recent: action.paths });
    case 'dirs/repos':
      return reduceDirs(state, { repos: action.paths });
    case 'dirs/completion':
      return reduceDirs(state, { completion: { dir: action.dir, children: action.children } });
    case 'daemon/restart':
      return state;
    case 'daemon/quit':
      return state.overlay?.kind === 'quit-confirm' ? { ...state, overlay: null } : state;
    case 'overlay/open':
      return action.overlay.kind === 'new-pane'
        ? openNewPane(state, action.overlay.target)
        : { ...state, overlay: action.overlay };
    case 'overlay/close':
      return state.overlay === null ? state : { ...state, overlay: null };
    case 'grid/split':
      return { ...state, splits: { ...state.splits, [action.tabId]: action.split } };
    case 'settings/change':
      return { ...state, settings: action.settings };
    case 'notice/show':
      return showNotice(state, action.text);
    case 'notice/clear':
      return state.notice?.id === action.id ? { ...state, notice: null } : state;
    case 'usage/hide':
      return state.usage.shown ? { ...state, usage: { ...state.usage, shown: false } } : state;
    case 'usage/loaded':
      return { ...state, usage: { ...state.usage, usage: action.usage, error: null } };
    case 'usage/failed':
      return { ...state, usage: { ...state.usage, error: action.message } };
    case 'env/keyRepeat':
      return { ...state, env: { ...state.env, keyRepeat: action.value } };
    case 'env/reducedMotion':
      return state.reducedMotion === action.value
        ? state
        : { ...state, reducedMotion: action.value };
    case 'tasks/loaded':
      return loadTasks(state, action.list);
    case 'task/add':
      return { ...state, taskForm: { pending: true, error: null } };
    case 'task/added': {
      const next = upsertTask(state, action.task);
      const overlay = next.overlay?.kind === 'dispatch' ? null : next.overlay;
      return { ...next, overlay, taskForm: { pending: false, error: null } };
    }
    case 'task/opened': {
      const overlay = state.overlay?.kind === 'dispatch' ? null : state.overlay;
      return { ...state, overlay, taskForm: { pending: false, error: null } };
    }
    case 'task/addFailed':
      return { ...state, taskForm: { pending: false, error: action.message } };
    case 'task/cancel':
    case 'task/move':
    case 'task/send':
    case 'queue/pause':
      return state;
    case 'skills/query': {
      const key = `${action.cli} ${action.cwd}`;
      if (state.skills.key === key && !state.skills.error) return state;
      return { ...state, skills: { key, list: [], loading: true, error: null } };
    }
    case 'skills/loaded':
      return state.skills.key === action.key
        ? {
            ...state,
            skills: { key: action.key, list: action.list.skills, loading: false, error: null },
          }
        : state;
    case 'skills/failed':
      return state.skills.key === action.key
        ? { ...state, skills: { ...state.skills, loading: false, error: action.message } }
        : state;
    case 'env/buildId': {
      if ((state.env.buildId ?? null) === action.value) return state;
      const { buildId: _old, ...env } = state.env;
      return { ...state, env: action.value === null ? env : { ...env, buildId: action.value } };
    }
  }
}
