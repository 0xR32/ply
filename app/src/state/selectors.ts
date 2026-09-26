import { basename, resolve } from 'node:path';
import { MAX_PANES_PER_TAB } from '../ipc/proto.gen';
import type { Cli, Pane, Split, Tab } from './actions';
import { type DirCandidate, type DirSuggestion, rankDirs } from './dir-match';
import type { AppState, DirSearch, PaneState } from './reducer';

/** How a pane's status is presented (spec 6.3): "done" is idle with every plan item complete. */
export type StatusTone =
  | 'starting'
  | 'running'
  | 'waiting'
  | 'done'
  | 'idle'
  | 'shell'
  | 'exited'
  | 'failed'
  | 'lost';

/** The header chip of one pane: its tone and label. */
export interface StatusView {
  tone: StatusTone;
  label: string;
}

/** A pane that needs the user: the CLI shows a permission dialog or asks a question. */
export function needsYou(pane: Pick<Pane, 'status'>): boolean {
  return pane.status === 'waiting_permission' || pane.status === 'waiting_input';
}

/** Whether `pane.resume` applies: the pane's process vanished across a plyd restart (spec 6.3 `lost`). */
export function isLost(pane: Pick<Pane, 'status'>): boolean {
  return pane.status === 'lost';
}

/** What resuming a lost pane runs: the CLI's own resume with the stored session id, else a fresh shell (spec 11.3). */
export function resumeHow(pane: Pick<Pane, 'cli' | 'session_ref'>): string {
  if (pane.session_ref === undefined || pane.cli === 'shell') return 'a fresh shell';
  return pane.cli === 'claude' ? 'claude --resume' : 'codex resume';
}

/** Whether the pane's process still runs, so closing it must confirm and kill (Ruling R7). */
export function isAlive(pane: Pick<Pane, 'status'>): boolean {
  return pane.status !== 'exited' && pane.status !== 'lost';
}

/** Idle with a plan whose items are all complete. */
export function isDone(pane: Pick<Pane, 'status' | 'progress'>): boolean {
  const p = pane.progress;
  return pane.status === 'idle' && p !== undefined && p.total > 0 && p.done >= p.total;
}

/** The chip for a pane; the running label carries no clock, so nothing re-renders the header every second. */
export function statusView(pane: PaneState, shellName: string): StatusView {
  if (pane.status === 'exited') {
    const code = pane.exit_code ?? 0;
    return { tone: code === 0 ? 'exited' : 'failed', label: `Exited ${code}` };
  }
  if (pane.status === 'lost') return { tone: 'lost', label: 'Lost' };
  if (pane.cli === 'shell') return { tone: 'shell', label: shellName };
  switch (pane.status) {
    case 'starting':
      return { tone: 'starting', label: 'Starting' };
    case 'running':
      return { tone: 'running', label: 'Running' };
    case 'waiting_permission':
    case 'waiting_input':
      return { tone: 'waiting', label: 'Needs you' };
    default:
      return isDone(pane) ? { tone: 'done', label: 'Done' } : { tone: 'idle', label: 'Your turn' };
  }
}

/** Whether `tab` holds the most panes a tab may (Ruling R56); plyd refuses another with `tab_full`. */
export function isTabFull(tab: Pick<Tab, 'pane_ids'> | undefined): boolean {
  return tab !== undefined && tab.pane_ids.length >= MAX_PANES_PER_TAB;
}

/** What ⌘N, ⌘D and the palette say instead of adding a pane to a full tab. */
export const FULL_TAB_NOTICE = `This tab has ${MAX_PANES_PER_TAB} panes — ⌘T opens a new tab`;

/** The tab shown in the grid, if any. */
export function selectActiveTab(state: AppState): Tab | undefined {
  return state.tabs.find((t) => t.id === state.activeTabId);
}

/** The focused pane of the active tab, if any. */
export function selectFocusedPane(state: AppState): PaneState | undefined {
  const id = selectActiveTab(state)?.focus_pane_id;
  return id === undefined ? undefined : state.panes[id];
}

/** Pane ids the grid mounts: the active tab's panes, or only its focused pane while zoomed (R-R20). */
export function visiblePaneIds(tab: Tab | undefined): readonly number[] {
  if (!tab) return [];
  if (tab.zoomed && tab.focus_pane_id !== undefined) return [tab.focus_pane_id];
  return tab.pane_ids;
}

/** Columns and rows of the grid for `count` panes (Ruling R56): side by side up to three, a 2 × 2 grid of quadrants at four. */
export function gridShape(count: number): { columns: number; rows: number } {
  if (count <= 3) return { columns: Math.max(count, 1), rows: 1 };
  return { columns: 2, rows: Math.ceil(count / 2) };
}

/** `count` equal shares summing to 1. */
export function equalShares(count: number): number[] {
  return Array.from({ length: count }, () => 1 / count);
}

/** The tab's dragged split when it fits `shape` (one share per column and per row), else equal shares. */
export function paneSplit(
  state: AppState,
  tabId: number | undefined,
  shape: { columns: number; rows: number },
): Split {
  const split = tabId === undefined ? undefined : state.splits[tabId];
  const fits = split?.columns.length === shape.columns && split.rows.length === shape.rows;
  return fits && split
    ? split
    : { columns: equalShares(shape.columns), rows: equalShares(shape.rows) };
}

/** A direction ⌘← ⌘→ ⌘↑ ⌘↓ moves focus in (Ruling R58). */
export type PaneDirection = 'left' | 'right' | 'up' | 'down';

/** The spatial neighbour of `paneId` in `direction` from `gridShape` over the tab's full pane list, so zoom follows to it; `undefined` at an edge or with one pane, never wrapping (R58). */
export function paneNeighbour(
  tab: Pick<Tab, 'pane_ids'> | undefined,
  paneId: number,
  direction: PaneDirection,
): number | undefined {
  if (!tab) return undefined;
  const ids = tab.pane_ids;
  const index = ids.indexOf(paneId);
  if (index < 0) return undefined;
  const { columns, rows } = gridShape(ids.length);
  const row = Math.floor(index / columns);
  const col = index % columns;
  const targetRow = direction === 'up' ? row - 1 : direction === 'down' ? row + 1 : row;
  const targetCol = direction === 'left' ? col - 1 : direction === 'right' ? col + 1 : col;
  if (targetRow < 0 || targetRow >= rows || targetCol < 0 || targetCol >= columns) {
    return undefined;
  }
  const targetIndex = targetRow * columns + targetCol;
  return targetIndex < ids.length ? ids[targetIndex] : undefined;
}

/** The tab-bar dot of a tab: amber if any pane needs you, accent if any runs, mint when all are done or shells. */
export type TabDot = 'waiting' | 'running' | 'done' | 'idle';

/** Dot colour class for a tab from its panes' states. */
export function tabDot(state: AppState, tab: Tab): TabDot {
  const panes = tab.pane_ids.map((id) => state.panes[id]).filter((p) => p !== undefined);
  if (panes.some(needsYou)) return 'waiting';
  if (panes.some((p) => p.status === 'running')) return 'running';
  if (panes.length > 0 && panes.every((p) => isDone(p) || p.cli === 'shell')) return 'done';
  return 'idle';
}

/** How many open panes need the user, across every tab. */
export function selectWaitingCount(state: AppState): number {
  let n = 0;
  for (const tab of state.tabs) {
    for (const id of tab.pane_ids) {
      const p = state.panes[id];
      if (p && needsYou(p)) n++;
    }
  }
  return n;
}

/** Whether the connected plyd was built from another commit than the app (C1 `welcome.daemon_version` against the app's build id). */
export function selectForeignDaemon(state: AppState): boolean {
  const { connection, env } = state;
  return (
    connection.kind === 'connected' &&
    env.buildId !== undefined &&
    connection.daemonVersion !== env.buildId
  );
}

/** Open panes per program, for the status bar's "n claude · n codex · n zsh". */
export function selectCliCounts(state: AppState): Record<Cli, number> {
  const counts: Record<Cli, number> = { claude: 0, codex: 0, shell: 0 };
  for (const tab of state.tabs) {
    for (const id of tab.pane_ids) {
      const p = state.panes[id];
      if (p) counts[p.cli]++;
    }
  }
  return counts;
}

/** The next pane that needs you after the focused one, in bar order across tabs, wrapping; `undefined` if none. */
export function nextWaitingPane(state: AppState): number | undefined {
  const order = state.tabs.flatMap((t) => t.pane_ids);
  if (order.length === 0) return undefined;
  const current = selectActiveTab(state)?.focus_pane_id;
  const start = current === undefined ? -1 : order.indexOf(current);
  for (let step = 1; step <= order.length; step++) {
    const id = order[(start + step + order.length) % order.length] as number;
    const p = state.panes[id];
    if (p && needsYou(p)) return id;
  }
  return undefined;
}

/** The 1-based tab number and pane place of a pane, e.g. for "tab 1 · pane 2". */
export function panePlace(state: AppState, paneId: number): { tab: number; pane: number } | null {
  const tabIndex = state.tabs.findIndex((t) => t.pane_ids.includes(paneId));
  if (tabIndex < 0) return null;
  const tab = state.tabs[tabIndex] as Tab;
  return { tab: tabIndex + 1, pane: tab.pane_ids.indexOf(paneId) + 1 };
}

/** `path` with the home directory written as `~`. */
export function abbreviateHome(path: string, home: string): string {
  if (path === home) return '~';
  return path.startsWith(`${home}/`) ? `~${path.slice(home.length)}` : path;
}

/** `path` with a leading `~` expanded to the home directory; other paths are returned trimmed. */
export function expandHome(path: string, home: string): string {
  const p = path.trim();
  if (p === '~') return home;
  return p.startsWith('~/') ? `${home}${p.slice(1)}` : p;
}

/** A tab's name from its first pane's directory (Ruling R3): the basename, or `~` for the home directory. */
export function tabName(cwd: string, home: string): string {
  if (cwd === home) return '~';
  return basename(cwd) || cwd;
}

/** The title a pane header shows: the terminal's own title when set, else plyd's title, else the CLI name. */
export function paneTitle(pane: PaneState): string {
  return pane.terminalTitle || pane.title || pane.cli;
}

/** A query that names a path (it starts with `/`, `~` or `.`) split into the folder it names (absolute; `.` is resolved against `base`) and the segment typed after it; `null` for any other query. */
export function pathQuery(
  query: string,
  home: string,
  base: string,
): { dir: string; segment: string } | null {
  const q = query.trim();
  if (q === '~') return { dir: home, segment: '' };
  if (!(q.startsWith('/') || q.startsWith('~/') || q.startsWith('.'))) return null;
  const cut = q.lastIndexOf('/');
  const head = cut < 0 ? '.' : q.slice(0, cut) || '/';
  return { dir: resolve(base, expandHome(head, home)), segment: q.slice(cut + 1) };
}

/** The completions of a path query: sub-folders of the listed folder matched by the first segment typed after it, hidden ones only once that segment starts with `.`. */
function completionCandidates(dirs: DirSearch, home: string): DirCandidate[] {
  const listed = dirs.completion;
  const pq = pathQuery(dirs.query, home, dirs.base);
  if (!listed || !pq) return [];
  let segment: string;
  if (pq.dir === listed.dir) segment = pq.segment;
  else {
    const prefix = listed.dir === '/' ? '/' : `${listed.dir}/`;
    if (!pq.dir.startsWith(prefix)) return [];
    segment = pq.dir.slice(prefix.length).split('/')[0] ?? '';
  }
  const hidden = segment.startsWith('.');
  return listed.children
    .filter((path) => hidden || !basename(path).startsWith('.'))
    .map(
      (path): DirCandidate => ({
        path,
        shown: abbreviateHome(path, home),
        source: 'completion',
        recency: 0,
        segment,
      }),
    );
}

let lastSuggestions: { dirs: DirSearch; home: string; rows: readonly DirSuggestion[] } | null =
  null;

/** The new-pane form's folder suggestions for its current query (Ruling R57): recents, then repositories, then completions, ranked by `rankDirs`; recomputed only when the search changes. */
export function selectDirSuggestions(state: AppState): readonly DirSuggestion[] {
  const { dirs } = state;
  const home = state.env.home;
  if (lastSuggestions?.dirs === dirs && lastSuggestions.home === home) return lastSuggestions.rows;
  const candidates: DirCandidate[] = [
    ...dirs.recent.map(
      (path, i): DirCandidate => ({
        path,
        shown: abbreviateHome(path, home),
        source: 'recent',
        recency: dirs.recent.length - i,
      }),
    ),
    ...dirs.repos.map(
      (path): DirCandidate => ({
        path,
        shown: abbreviateHome(path, home),
        source: 'repo',
        recency: 0,
      }),
    ),
    ...completionCandidates(dirs, home),
  ];
  const rows = rankDirs(dirs.query, candidates);
  lastSuggestions = { dirs, home, rows };
  return rows;
}
