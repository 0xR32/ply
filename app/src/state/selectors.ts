import { basename } from 'node:path';
import type { Cli, Pane, Tab } from './actions';
import type { AppState, PaneState } from './reducer';

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

/** The header chip of one pane: tone, label and whether its dot pulses (running only). */
export interface StatusView {
  tone: StatusTone;
  label: string;
  pulse: boolean;
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

/** `mm:ss` below an hour, `h:mm:ss` from there; negative spans read as zero. */
export function formatElapsed(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const two = (n: number) => String(n).padStart(2, '0');
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h > 0 ? `${h}:${two(m)}:${two(s % 60)}` : `${two(m)}:${two(s % 60)}`;
}

/** The chip for a pane at `nowSeconds`; the running label carries the time since the status began when known. */
export function statusView(pane: PaneState, shellName: string, nowSeconds: number): StatusView {
  if (pane.status === 'exited') {
    const code = pane.exit_code ?? 0;
    return { tone: code === 0 ? 'exited' : 'failed', label: `Exited ${code}`, pulse: false };
  }
  if (pane.status === 'lost') return { tone: 'lost', label: 'Lost', pulse: false };
  if (pane.cli === 'shell') return { tone: 'shell', label: shellName, pulse: false };
  switch (pane.status) {
    case 'starting':
      return { tone: 'starting', label: 'Starting', pulse: false };
    case 'running': {
      const since = pane.statusSince;
      const label =
        since === undefined ? 'Running' : `Running ${formatElapsed(nowSeconds - since)}`;
      return { tone: 'running', label, pulse: true };
    }
    case 'waiting_permission':
    case 'waiting_input':
      return { tone: 'waiting', label: 'Needs you', pulse: false };
    default:
      return isDone(pane)
        ? { tone: 'done', label: 'Done', pulse: false }
        : { tone: 'idle', label: 'Your turn', pulse: false };
  }
}

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
