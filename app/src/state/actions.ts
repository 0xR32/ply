import type {
  AccentName,
  Choice,
  Cli,
  Event,
  Layout,
  OptionAsMeta,
  Pane,
  PaneStatus,
  Progress,
  Settings,
  Tab,
  TerminalTheme,
  Workspace,
} from '../ipc/proto.gen';

/** ply-proto wire types, re-exported so features reach them through state (they never import ipc, spec 8.2). */
export type {
  AccentName,
  Choice,
  Cli,
  Event,
  Layout,
  OptionAsMeta,
  Pane,
  PaneStatus,
  Progress,
  Settings,
  Tab,
  TerminalTheme,
  Workspace,
};

/** Where the C1 connection stands; `down` retries by itself, `incompatible` waits for a restart of plyd. */
export type ConnectionState =
  | { kind: 'connecting' }
  | { kind: 'connected'; daemonVersion: string }
  | { kind: 'down'; reason: string; retryInMs: number; starting: boolean }
  | { kind: 'incompatible'; reason: string };

/** The digits ⌘1–⌘9 address; tab n is the n-th tab in the bar. */
export type TabDigit = 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9;

/** Every command of spec 7.2 by its id; the keymap binds keys to these, the palette and the footer run them. */
export type CommandId =
  | 'palette.open'
  | 'pane.new'
  | 'tab.new'
  | 'pane.nextWaiting'
  | `tab.go.${TabDigit}`
  | 'pane.prev'
  | 'pane.next'
  | 'tab.prev'
  | 'tab.next'
  | 'pane.zoom'
  | 'pane.terminalHere'
  | 'pane.close'
  | 'font.up'
  | 'font.down'
  | 'font.reset'
  | 'settings.open';

/** The one overlay that may be open; while any is open, global key bindings do nothing (spec 7.4). */
export type Overlay =
  | { kind: 'palette' }
  | { kind: 'new-pane'; target: 'pane' | 'tab' }
  | { kind: 'settings' }
  | { kind: 'close-confirm'; paneId: number };

/** What the new-pane form asks for; effects turn it into `pane.create` (`cwd` absolute, `worktree` Claude only). */
export interface NewPaneRequest {
  target: 'pane' | 'tab';
  cli: Cli;
  cwd: string;
  worktree?: string;
  prompt?: string;
}

/** Every state change; the reducer is pure over these and effects.ts performs the C1 side of the intents. */
export type Action =
  | { type: 'connection/changed'; state: ConnectionState }
  | {
      type: 'session/loaded';
      workspace: Workspace;
      panes: Pane[];
      layout: Layout;
      settings: Settings;
    }
  | { type: 'daemon/event'; event: Event }
  | { type: 'command'; id: CommandId }
  | { type: 'tab/select'; tabId: number }
  | { type: 'pane/focus'; paneId: number }
  | { type: 'pane/title'; paneId: number; title: string }
  | { type: 'pane/answer'; paneId: number; choice: Choice }
  | { type: 'pane/resume'; paneId: number }
  | { type: 'pane/resumed'; pane: Pane }
  | { type: 'pane/create'; request: NewPaneRequest }
  | { type: 'pane/created'; pane: Pane }
  | { type: 'pane/createFailed'; message: string }
  | { type: 'pane/closeConfirmed'; paneId: number }
  | { type: 'overlay/open'; overlay: Overlay }
  | { type: 'overlay/close' }
  | { type: 'settings/change'; settings: Settings }
  | { type: 'notice/show'; text: string }
  | { type: 'notice/clear'; id: number }
  | { type: 'env/reducedMotion'; value: boolean };
