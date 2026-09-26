import { commandKeyLabel, TAB_DIGITS } from '../../keymap/keymap';
import type { Action, CommandId } from '../../state/actions';
import type { AppState } from '../../state/reducer';
import {
  abbreviateHome,
  FULL_TAB_NOTICE,
  isLost,
  isTabFull,
  nextWaitingPane,
  type PaneDirection,
  paneNeighbour,
  panePlace,
  paneTitle,
  resumeHow,
  selectActiveTab,
  selectFocusedPane,
  selectForeignDaemon,
  selectPaneQueue,
  selectQueueCounts,
  selectWaitingCount,
  statusView,
  tabDot,
} from '../../state/selectors';

/** Colour class of an item's leading dot. */
export type ItemDot = 'accent' | 'amber' | 'mint' | 'dim';

/** One row of the palette: what it shows and the actions it dispatches when run. */
export interface PaletteItem {
  id: string;
  section: 'Commands' | 'Tabs' | 'Panes';
  label: string;
  hint: string;
  keys: string;
  dot: ItemDot;
  actions: Action[];
  /** Shown dimmed: running it only says why it cannot (a new pane in a full tab). */
  unavailable?: boolean;
}

function command(id: CommandId, label: string, hint: string, dot: ItemDot): PaletteItem {
  return {
    id,
    section: 'Commands',
    label,
    hint,
    keys: commandKeyLabel(id),
    dot,
    actions: [{ type: 'overlay/close' }, { type: 'command', id }],
  };
}

function resumeItems(state: AppState): PaletteItem[] {
  return state.tabs.flatMap((t, tabIndex) =>
    t.pane_ids.flatMap((id, paneIndex): PaletteItem[] => {
      const pane = state.panes[id];
      if (!pane || !isLost(pane)) return [];
      return [
        {
          id: `pane-resume-${id}`,
          section: 'Commands',
          label: `Resume ${paneTitle(pane)}`,
          hint: `tab ${tabIndex + 1} · pane ${paneIndex + 1} · ${resumeHow(pane)}`,
          keys: '',
          dot: 'accent',
          actions: [{ type: 'overlay/close' }, { type: 'pane/resume', paneId: id }],
        },
      ];
    }),
  );
}

const DIRECTIONS: readonly PaneDirection[] = ['left', 'right', 'up', 'down'];

/** "Pane left/right/up/down" (Ruling R58), only for directions with a spatial neighbour to move to. */
function directionItems(state: AppState, tab: ReturnType<typeof selectActiveTab>): PaletteItem[] {
  const focused = selectFocusedPane(state);
  if (!focused) return [];
  return DIRECTIONS.filter((dir) => paneNeighbour(tab, focused.id, dir) !== undefined).map((dir) =>
    command(`pane.${dir}`, `Pane ${dir}`, 'in this tab', 'dim'),
  );
}

function commands(state: AppState): PaletteItem[] {
  const home = state.env.home;
  const tab = selectActiveTab(state);
  const focused = selectFocusedPane(state);
  const place = focused ? panePlace(state, focused.id) : null;
  const full = isTabFull(tab);
  const unavailable = (item: PaletteItem): PaletteItem =>
    full
      ? { ...item, hint: `unavailable: ${FULL_TAB_NOTICE}`, dot: 'dim', unavailable: true }
      : item;
  const items = [
    unavailable(
      command('pane.new', 'New pane', 'Claude Code, Codex or a shell, in this tab', 'accent'),
    ),
    command('tab.new', 'New tab', 'starts with one pane', 'accent'),
  ];
  const waitingId = selectWaitingCount(state) > 0 ? nextWaitingPane(state) : undefined;
  const waiting = waitingId === undefined ? undefined : state.panes[waitingId];
  if (waiting) {
    const where = panePlace(state, waiting.id);
    const kind = waiting.status === 'waiting_permission' ? 'permission' : 'question';
    const hint = where ? `tab ${where.tab} · pane ${where.pane} · ${kind}` : kind;
    items.push(command('pane.nextWaiting', 'Go to what needs you', hint, 'amber'));
  }
  items.push(...resumeItems(state));
  const here = focused?.cwd ?? state.workspace?.path ?? home;
  const hereHint = place
    ? `pane ${place.pane} in ${abbreviateHome(here, home)}`
    : abbreviateHome(here, home);
  items.push(
    unavailable(command('pane.terminalHere', 'Terminal here', hereHint, 'dim')),
    command('pane.zoom', tab?.zoomed ? 'Unzoom pane' : 'Zoom pane', 'fill the tab, toggle', 'dim'),
    command('pane.next', 'Next pane', 'in this tab', 'dim'),
    command('pane.prev', 'Previous pane', 'in this tab', 'dim'),
    ...directionItems(state, tab),
    command('tab.next', 'Next tab', '', 'dim'),
    command('tab.prev', 'Previous tab', '', 'dim'),
    command(
      'pane.close',
      'Close pane',
      place ? `pane ${place.pane} · asks while it runs` : '',
      'dim',
    ),
    ...taskItems(state),
    command('settings.open', 'Settings', 'accent, ⌥ as Meta, keep awake', 'dim'),
    command('font.up', 'Bigger text', `${state.settings.font_size} pt now`, 'dim'),
    command('font.down', 'Smaller text', `${state.settings.font_size} pt now`, 'dim'),
    command('font.reset', 'Reset text size', '', 'dim'),
    ...daemonItems(state),
  );
  return items;
}

const PAUSE_REASON = {
  user: 'paused',
  restored: 'held after plyd restarted',
  failed: 'its last task failed',
} as const;

/** The task queue (Ruling R60): the form, the queue, and pausing or resuming each pane that holds queued tasks. */
function taskItems(state: AppState): PaletteItem[] {
  if (!state.tasks.available) return [];
  const counts = selectQueueCounts(state);
  const items: PaletteItem[] = [
    command(
      'task.dispatch',
      'Dispatch a task',
      'a skill or a prompt, to a pane or the next free one',
      'accent',
    ),
    command(
      'task.queue',
      'Show task queue',
      `${counts.queued} queued · ${counts.running} running${counts.needsYou ? ` · ${counts.needsYou} needs you` : ''}`,
      'accent',
    ),
  ];
  for (const tab of state.tabs) {
    for (const id of tab.pane_ids) {
      const queued = selectPaneQueue(state, id).length;
      const pane = state.panes[id];
      if (queued === 0 || !pane) continue;
      const paused = state.tasks.queues[id]?.paused;
      const place = panePlace(state, id);
      const name = place ? `pane ${place.pane}` : paneTitle(pane);
      items.push({
        id: `queue-pause-${id}`,
        section: 'Commands',
        label: `${paused ? 'Resume' : 'Pause'} queue of ${name}`,
        hint: `${pane.cli} · ${paused ? `${PAUSE_REASON[paused]} · ` : ''}${queued} queued`,
        keys: '',
        dot: 'dim',
        actions: [{ type: 'overlay/close' }, { type: 'queue/pause', paneId: id, paused: !paused }],
      });
    }
  }
  return items;
}

/** Replacing plyd (Ruling R53): a restart that the launcher follows with the build in target/, and a confirmed quit. */
function daemonItems(state: AppState): PaletteItem[] {
  const foreign = selectForeignDaemon(state);
  return [
    {
      id: 'daemon-restart',
      section: 'Commands',
      label: 'Restart plyd',
      hint: `${foreign ? 'plyd is from another build, rebuild it first; ' : ''}starts the build in target/, running sessions come back lost`,
      keys: '',
      dot: foreign ? 'amber' : 'dim',
      actions: [{ type: 'overlay/close' }, { type: 'daemon/restart' }],
    },
    {
      id: 'daemon-quit',
      section: 'Commands',
      label: 'Quit ply and stop sessions',
      hint: 'stops every process plyd runs, asks first',
      keys: '',
      dot: 'dim',
      actions: [{ type: 'overlay/open', overlay: { kind: 'quit-confirm' } }],
    },
  ];
}

function tabItems(state: AppState): PaletteItem[] {
  return state.tabs.map((t, i) => {
    const dot = tabDot(state, t);
    const digit = TAB_DIGITS[i];
    return {
      id: `tab-${t.id}`,
      section: 'Tabs',
      label: t.name,
      hint: `tab ${i + 1} · ${t.pane_ids.length} ${t.pane_ids.length === 1 ? 'pane' : 'panes'}`,
      keys: digit ? commandKeyLabel(`tab.go.${digit}`) : '',
      dot:
        dot === 'waiting'
          ? 'amber'
          : dot === 'running'
            ? 'accent'
            : dot === 'done'
              ? 'mint'
              : 'dim',
      actions: [{ type: 'overlay/close' }, { type: 'tab/select', tabId: t.id }],
    };
  });
}

function paneItems(state: AppState, nowSeconds: number): PaletteItem[] {
  return state.tabs.flatMap((t) =>
    t.pane_ids.flatMap((id): PaletteItem[] => {
      const pane = state.panes[id];
      if (!pane) return [];
      const view = statusView(pane, state.env.shellName, nowSeconds);
      const dot: ItemDot =
        view.tone === 'waiting'
          ? 'amber'
          : view.tone === 'running'
            ? 'accent'
            : view.tone === 'done'
              ? 'mint'
              : 'dim';
      return [
        {
          id: `pane-${id}`,
          section: 'Panes',
          label: paneTitle(pane),
          hint: `${t.name} · ${pane.cli} · ${view.label}`,
          keys: '',
          dot,
          actions: [{ type: 'overlay/close' }, { type: 'pane/focus', paneId: id }],
        },
      ];
    }),
  );
}

/** Whether every word of `query` occurs in the item's label or hint, ignoring case. */
export function matches(item: PaletteItem, query: string): boolean {
  const hay = `${item.label} ${item.hint}`.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => hay.includes(word));
}

/** The palette rows for `query` (spec 7.4): commands only while empty; commands, tabs and panes when filtering. */
export function paletteItems(state: AppState, query: string, nowSeconds: number): PaletteItem[] {
  if (query.trim() === '') return commands(state);
  return [...commands(state), ...tabItems(state), ...paneItems(state, nowSeconds)].filter((item) =>
    matches(item, query),
  );
}
