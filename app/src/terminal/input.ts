import type { EventPayload } from '@gpuix/react';
import type { OptionAsMeta } from '../ipc/proto.gen';
import type { KeyAction, KeyFrame, MouseAction, MouseFrame } from './frames';
import { Mods } from './frames';

/** libghostty-vt's `GhosttyKey` names in enum order (`key/event.h` at ghostty 44f2a44); the index is the wire value. */
export const GHOSTTY_KEYS: readonly string[] = [
  'UNIDENTIFIED',
  'BACKQUOTE',
  'BACKSLASH',
  'BRACKET_LEFT',
  'BRACKET_RIGHT',
  'COMMA',
  ...Array.from({ length: 10 }, (_, d) => `DIGIT_${d}`),
  'EQUAL',
  'INTL_BACKSLASH',
  'INTL_RO',
  'INTL_YEN',
  ...Array.from({ length: 26 }, (_, i) => String.fromCharCode(65 + i)),
  'MINUS',
  'PERIOD',
  'QUOTE',
  'SEMICOLON',
  'SLASH',
  'ALT_LEFT',
  'ALT_RIGHT',
  'BACKSPACE',
  'CAPS_LOCK',
  'CONTEXT_MENU',
  'CONTROL_LEFT',
  'CONTROL_RIGHT',
  'ENTER',
  'META_LEFT',
  'META_RIGHT',
  'SHIFT_LEFT',
  'SHIFT_RIGHT',
  'SPACE',
  'TAB',
  'CONVERT',
  'KANA_MODE',
  'NON_CONVERT',
  'DELETE',
  'END',
  'HELP',
  'HOME',
  'INSERT',
  'PAGE_DOWN',
  'PAGE_UP',
  'ARROW_DOWN',
  'ARROW_LEFT',
  'ARROW_RIGHT',
  'ARROW_UP',
  'NUM_LOCK',
  ...Array.from({ length: 10 }, (_, d) => `NUMPAD_${d}`),
  'NUMPAD_ADD',
  'NUMPAD_BACKSPACE',
  'NUMPAD_CLEAR',
  'NUMPAD_CLEAR_ENTRY',
  'NUMPAD_COMMA',
  'NUMPAD_DECIMAL',
  'NUMPAD_DIVIDE',
  'NUMPAD_ENTER',
  'NUMPAD_EQUAL',
  'NUMPAD_MEMORY_ADD',
  'NUMPAD_MEMORY_CLEAR',
  'NUMPAD_MEMORY_RECALL',
  'NUMPAD_MEMORY_STORE',
  'NUMPAD_MEMORY_SUBTRACT',
  'NUMPAD_MULTIPLY',
  'NUMPAD_PAREN_LEFT',
  'NUMPAD_PAREN_RIGHT',
  'NUMPAD_SUBTRACT',
  'NUMPAD_SEPARATOR',
  'NUMPAD_UP',
  'NUMPAD_DOWN',
  'NUMPAD_RIGHT',
  'NUMPAD_LEFT',
  'NUMPAD_BEGIN',
  'NUMPAD_HOME',
  'NUMPAD_END',
  'NUMPAD_INSERT',
  'NUMPAD_DELETE',
  'NUMPAD_PAGE_UP',
  'NUMPAD_PAGE_DOWN',
  'ESCAPE',
  ...Array.from({ length: 25 }, (_, i) => `F${i + 1}`),
  'FN',
  'FN_LOCK',
  'PRINT_SCREEN',
  'SCROLL_LOCK',
  'PAUSE',
  'BROWSER_BACK',
  'BROWSER_FAVORITES',
  'BROWSER_FORWARD',
  'BROWSER_HOME',
  'BROWSER_REFRESH',
  'BROWSER_SEARCH',
  'BROWSER_STOP',
  'EJECT',
  'LAUNCH_APP_1',
  'LAUNCH_APP_2',
  'LAUNCH_MAIL',
  'MEDIA_PLAY_PAUSE',
  'MEDIA_SELECT',
  'MEDIA_STOP',
  'MEDIA_TRACK_NEXT',
  'MEDIA_TRACK_PREVIOUS',
  'POWER',
  'SLEEP',
  'AUDIO_VOLUME_DOWN',
  'AUDIO_VOLUME_MUTE',
  'AUDIO_VOLUME_UP',
  'WAKE_UP',
  'COPY',
  'CUT',
  'PASTE',
];

const CODE = new Map(GHOSTTY_KEYS.map((name, i) => [name, i]));

function code(name: string): number {
  return CODE.get(name) ?? 0;
}

// GPUI's key names (zed Keystroke::key) for keys that type no character.
const NAMED: Record<string, string> = {
  enter: 'ENTER',
  tab: 'TAB',
  escape: 'ESCAPE',
  backspace: 'BACKSPACE',
  delete: 'DELETE',
  insert: 'INSERT',
  home: 'HOME',
  end: 'END',
  pageup: 'PAGE_UP',
  pagedown: 'PAGE_DOWN',
  up: 'ARROW_UP',
  down: 'ARROW_DOWN',
  left: 'ARROW_LEFT',
  right: 'ARROW_RIGHT',
  space: 'SPACE',
  menu: 'CONTEXT_MENU',
  help: 'HELP',
};

const PUNCTUATION: Record<string, string> = {
  '`': 'BACKQUOTE',
  '\\': 'BACKSLASH',
  '[': 'BRACKET_LEFT',
  ']': 'BRACKET_RIGHT',
  ',': 'COMMA',
  '=': 'EQUAL',
  '-': 'MINUS',
  '.': 'PERIOD',
  "'": 'QUOTE',
  ';': 'SEMICOLON',
  '/': 'SLASH',
};

// AppKit reports some shifted US-layout characters as the key itself; fold them back to the physical key.
const SHIFTED: Record<string, string> = {
  '~': '`',
  '!': '1',
  '@': '2',
  '#': '3',
  $: '4',
  '%': '5',
  '^': '6',
  '&': '7',
  '*': '8',
  '(': '9',
  ')': '0',
  _: '-',
  '+': '=',
  '{': '[',
  '}': ']',
  '|': '\\',
  ':': ';',
  '"': "'",
  '<': ',',
  '>': '.',
  '?': '/',
};

/** The `GhosttyKey` and unshifted codepoint for a GPUIX key name; unknown layout keys (such as `ö`) are 0 with their own codepoint. */
export function keyCode(key: string): { key: number; unshifted: number } {
  const named = NAMED[key];
  if (named) return { key: code(named), unshifted: key === 'space' ? 0x20 : 0 };
  const fn = /^f([1-9]|1\d|2[0-5])$/.exec(key);
  if (fn) return { key: code(`F${fn[1]}`), unshifted: 0 };
  const chars = [...key];
  if (chars.length !== 1) return { key: 0, unshifted: 0 };
  const lower = (SHIFTED[key] ?? key).toLowerCase();
  const cp = lower.codePointAt(0) ?? 0;
  if (lower >= 'a' && lower <= 'z') return { key: code(lower.toUpperCase()), unshifted: cp };
  if (lower >= '0' && lower <= '9') return { key: code(`DIGIT_${lower}`), unshifted: cp };
  const punct = PUNCTUATION[lower];
  return { key: punct ? code(punct) : 0, unshifted: cp };
}

/** C2 modifier bits for GPUIX's modifier flags (GPUIX reports no left/right side). */
export function modsOf(m: EventPayload['modifiers']): number {
  if (!m) return 0;
  return (
    (m.shift ? Mods.shift : 0) |
    (m.ctrl ? Mods.ctrl : 0) |
    (m.alt ? Mods.alt : 0) |
    (m.cmd ? Mods.super : 0)
  );
}

function printable(text: string | undefined): string {
  if (!text) return '';
  for (const ch of text) {
    const cp = ch.codePointAt(0) ?? 0;
    if (cp < 0x20 || cp === 0x7f || (cp >= 0x80 && cp < 0xa0)) return '';
  }
  return text;
}

/**
 * The KEY frame for a GPUIX key event, or `null` for every ⌘ chord (K6: those belong to the app keymap). With ⌥ as Meta (K5) the key's own character is sent with ⌥ unconsumed so plyd prefixes ESC; GPUIX reports no key side, so `left` and `right` act on either ⌥.
 */
export function keyFrame(
  event: EventPayload,
  action: KeyAction,
  optionAsMeta: OptionAsMeta,
): KeyFrame | null {
  const m = event.modifiers;
  if (m?.cmd) return null;
  const name = event.key ?? '';
  const { key, unshifted } = keyCode(name);
  let mods = modsOf(m);
  let consumedMods = 0;
  let text = '';
  if (action !== 'release' && !m?.ctrl) {
    const base = unshifted > 0 ? String.fromCodePoint(unshifted) : '';
    const baseShifted = m?.shift ? base.toUpperCase() : base;
    if (m?.alt && optionAsMeta !== 'off') {
      text = printable(baseShifted || event.keyChar);
      if (optionAsMeta === 'right') mods |= Mods.altSide;
    } else {
      text = printable(event.keyChar);
      if (text && m?.alt && text !== baseShifted) consumedMods |= Mods.alt;
    }
    if (text && m?.shift && text !== base) consumedMods |= Mods.shift;
  }
  if (key === 0 && text === '') return null;
  return {
    kind: 'key',
    key,
    mods,
    consumedMods,
    action,
    composing: false,
    unshiftedCodepoint: unshifted,
    text,
  };
}

/** GPUIX mouse buttons (0 left, 1 middle, 2 right) as C2 buttons (1 left, 3 middle, 2 right). */
export function mouseButton(gpuixButton: number | undefined): number {
  switch (gpuixButton) {
    case 0:
      return 1;
    case 1:
      return 3;
    case 2:
      return 2;
    default:
      return 0;
  }
}

/** A MOUSE frame at pixel `(x, y)` from the grid's top-left; the cell is derived from the cell size and clamped to the grid. */
export function mouseFrame(
  action: MouseAction,
  button: number,
  mods: number,
  x: number,
  y: number,
  cell: { width: number; height: number; cols: number; rows: number },
): MouseFrame {
  const col = Math.min(Math.max(Math.floor(x / cell.width), 0), Math.max(cell.cols - 1, 0));
  const row = Math.min(Math.max(Math.floor(y / cell.height), 0), Math.max(cell.rows - 1, 0));
  return { kind: 'mouse', action, button, mods, col, row, x, y };
}
