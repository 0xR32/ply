import { commandKeyLabel, TAB_DIGITS } from '../../keymap/keymap';
import type { Action, CommandId } from '../../state/actions';
import type { AppState } from '../../state/reducer';
import {
  abbreviateHome,
  isLost,
  nextWaitingPane,
  panePlace,
  paneTitle,
  resumeHow,
  selectActiveTab,
  selectFocusedPane,
  selectForeignDaemon,
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

function commands(state: AppState): PaletteItem[] {
  const home = state.env.home;
  const tab = selectActiveTab(state);
  const focused = selectFocusedPane(state);
  const place = focused ? panePlace(state, focused.id) : null;
  const items = [
    command('pane.new', 'New pane', 'Claude Code, Codex or a shell, in this tab', 'accent'),
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
    command('pane.terminalHere', 'Terminal here', hereHint, 'dim'),
    command('pane.zoom', tab?.zoomed ? 'Unzoom pane' : 'Zoom pane', 'fill the tab, toggle', 'dim'),
    command('pane.next', 'Next pane', 'in this tab', 'dim'),
    command('pane.prev', 'Previous pane', 'in this tab', 'dim'),
    command('tab.next', 'Next tab', '', 'dim'),
    command('tab.prev', 'Previous tab', '', 'dim'),
    command(
      'pane.close',
      'Close pane',
      place ? `pane ${place.pane} · asks while it runs` : '',
      'dim',
    ),
    command('settings.open', 'Settings', 'accent, ⌥ as Meta, keep awake', 'dim'),
    command('font.up', 'Bigger text', `${state.settings.font_size} pt now`, 'dim'),
    command('font.down', 'Smaller text', `${state.settings.font_size} pt now`, 'dim'),
    command('font.reset', 'Reset text size', '', 'dim'),
    ...daemonItems(state),
  );
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
