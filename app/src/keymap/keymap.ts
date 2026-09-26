import type { CommandId, TabDigit } from '../state/actions';

/** One global binding (spec 7.2): canonical keystroke `[ctrl-][alt-]cmd-[shift-]<key>` and the command it runs. */
export interface Binding {
  keys: string;
  command: CommandId;
  label: string;
}

/** The digits ⌘1–⌘9 bind, in order. */
export const TAB_DIGITS: readonly TabDigit[] = [1, 2, 3, 4, 5, 6, 7, 8, 9];

/** Every app binding, each declared once (K4); nothing here may lack ⌘ or hit `RESERVED` (K1, K3). */
export const bindings: readonly Binding[] = [
  { keys: 'cmd-k', command: 'palette.open', label: 'Command palette' },
  { keys: 'cmd-n', command: 'pane.new', label: 'New pane' },
  { keys: 'cmd-t', command: 'tab.new', label: 'New tab' },
  { keys: 'cmd-j', command: 'pane.nextWaiting', label: 'Go to what needs you' },
  ...TAB_DIGITS.map(
    (d): Binding => ({ keys: `cmd-${d}`, command: `tab.go.${d}`, label: `Go to tab ${d}` }),
  ),
  { keys: 'cmd-[', command: 'pane.prev', label: 'Previous pane' },
  { keys: 'cmd-]', command: 'pane.next', label: 'Next pane' },
  { keys: 'cmd-left', command: 'pane.left', label: 'Pane left' },
  { keys: 'cmd-right', command: 'pane.right', label: 'Pane right' },
  { keys: 'cmd-up', command: 'pane.up', label: 'Pane up' },
  { keys: 'cmd-down', command: 'pane.down', label: 'Pane down' },
  { keys: 'cmd-shift-[', command: 'tab.prev', label: 'Previous tab' },
  { keys: 'cmd-shift-]', command: 'tab.next', label: 'Next tab' },
  { keys: 'cmd-enter', command: 'pane.zoom', label: 'Zoom pane' },
  { keys: 'cmd-d', command: 'pane.terminalHere', label: 'Terminal here' },
  { keys: 'cmd-shift-w', command: 'pane.close', label: 'Close pane' },
  { keys: 'cmd-=', command: 'font.up', label: 'Bigger text' },
  { keys: 'cmd--', command: 'font.down', label: 'Smaller text' },
  { keys: 'cmd-0', command: 'font.reset', label: 'Reset text size' },
  { keys: 'cmd-,', command: 'settings.open', label: 'Settings' },
  { keys: 'cmd-u', command: 'usage.show', label: 'Plan usage (hold)' },
  { keys: 'cmd-e', command: 'task.dispatch', label: 'Dispatch a task' },
  { keys: 'cmd-shift-e', command: 'task.queue', label: 'Task queue' },
];

/** What a focused terminal does with the ⌘ chords spec 7.3 gives it; never sent to the pty (K6, K7). */
export type TerminalCommand =
  | 'terminal.copy'
  | 'terminal.paste'
  | 'terminal.selectAll'
  | 'terminal.find';

/** A chord a focused terminal handles itself; each is a reserved macOS shortcut used with its standard meaning (K3). */
export interface TerminalBinding {
  keys: string;
  command: TerminalCommand;
  label: string;
}

/** Spec 7.3's pane chords, declared here once like every other binding (K4); the dispatcher ignores them. */
export const terminalBindings: readonly TerminalBinding[] = [
  { keys: 'cmd-c', command: 'terminal.copy', label: 'Copy' },
  { keys: 'cmd-v', command: 'terminal.paste', label: 'Paste' },
  { keys: 'cmd-a', command: 'terminal.selectAll', label: 'Select all' },
  { keys: 'cmd-f', command: 'terminal.find', label: 'Find' },
];

const terminalByKeys = new Map(terminalBindings.map((b) => [b.keys, b.command]));

/** The terminal command a canonical keystroke runs while a pane has focus, if any. */
export function terminalCommandForKeys(keys: string): TerminalCommand | undefined {
  return terminalByKeys.get(keys);
}

const byKeys = new Map(bindings.map((b) => [b.keys, b]));
const byCommand = new Map(bindings.map((b) => [b.command, b]));

/** The binding a canonical keystroke runs, if any. */
export function bindingForKeys(keys: string): Binding | undefined {
  return byKeys.get(keys);
}

/** The binding of a command; every `CommandId` has exactly one (checked by keymap.test.ts). */
export function bindingFor(command: CommandId): Binding | undefined {
  return byCommand.get(command);
}

const MODIFIERS = ['ctrl', 'alt', 'cmd', 'shift'] as const;
type Modifier = (typeof MODIFIERS)[number];

/** Splits a keystroke into its modifiers and key; the key itself may be `-` (as in `cmd--`). */
export function parseKeys(keys: string): { modifiers: Set<Modifier>; key: string } {
  const modifiers = new Set<Modifier>();
  let rest = keys;
  for (let again = true; again; ) {
    again = false;
    for (const m of MODIFIERS) {
      if (rest.startsWith(`${m}-`) && rest.length > m.length + 1) {
        modifiers.add(m);
        rest = rest.slice(m.length + 1);
        again = true;
      }
    }
  }
  return { modifiers, key: rest };
}

function canonical(modifiers: Set<Modifier>, key: string): string {
  return `${MODIFIERS.filter((m) => modifiers.has(m))
    .map((m) => `${m}-`)
    .join('')}${key}`;
}

/** Keystrokes in any modifier order in canonical form, e.g. `shift-cmd-]` → `cmd-shift-]`. */
export function normalizeKeys(keys: string): string {
  const { modifiers, key } = parseKeys(keys);
  return canonical(modifiers, key);
}

const SHIFTED: Record<string, string> = { '{': '[', '}': ']', _: '-' };

/** The canonical keystroke of a GPUIX key event, folding AppKit's shifted characters back to their keys (`}` → `]`). */
export function keysOfEvent(event: {
  key?: string;
  modifiers?: { shift: boolean; ctrl: boolean; alt: boolean; cmd: boolean };
}): string | null {
  const raw = event.key;
  if (!raw) return null;
  const m = event.modifiers;
  const modifiers = new Set<Modifier>();
  if (m?.ctrl) modifiers.add('ctrl');
  if (m?.alt) modifiers.add('alt');
  if (m?.cmd) modifiers.add('cmd');
  if (m?.shift) modifiers.add('shift');
  let key = raw.length === 1 ? raw.toLowerCase() : raw;
  if (key === 'return') key = 'enter';
  const unshifted = SHIFTED[key];
  if (unshifted) {
    key = unshifted;
    if (unshifted !== '-') modifiers.add('shift');
    else modifiers.delete('shift');
  }
  if (key === '+' || (key === '=' && modifiers.has('shift'))) {
    key = '=';
    modifiers.delete('shift');
  }
  return canonical(modifiers, key);
}

const GLYPHS: Record<Modifier, string> = { ctrl: '⌃', alt: '⌥', shift: '⇧', cmd: '⌘' };
const KEY_GLYPHS: Record<string, string> = {
  enter: '⏎',
  escape: 'esc',
  tab: '⇥',
  up: '↑',
  down: '↓',
  left: '←',
  right: '→',
};

/** The label of a keystroke in the spec's glyph order, e.g. `cmd-shift-]` → `⌘⇧]`, `cmd-enter` → `⌘⏎`. */
export function keyLabel(keys: string): string {
  const { modifiers, key } = parseKeys(keys);
  const glyphs = (['cmd', 'shift', 'alt', 'ctrl'] as const)
    .filter((m) => modifiers.has(m))
    .map((m) => GLYPHS[m])
    .join('');
  const shown = KEY_GLYPHS[key] ?? (key.length === 1 ? key.toUpperCase() : key);
  return `${glyphs}${shown}`;
}

/** The key label of a command's binding, or an empty string for an unbound command. */
export function commandKeyLabel(command: CommandId): string {
  const b = bindingFor(command);
  return b ? keyLabel(b.keys) : '';
}
