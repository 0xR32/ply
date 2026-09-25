import { describe, expect, test } from 'bun:test';
import type { EventPayload } from '@gpuix/react';
import type { Action, CommandId } from '../state/actions';
import { makePane, makeState } from '../state/test-support';
import { createKeyDispatcher } from './dispatcher';
import {
  bindingFor,
  bindings,
  keyLabel,
  keysOfEvent,
  normalizeKeys,
  parseKeys,
  TAB_DIGITS,
} from './keymap';
import { RESERVED, reservedConflict } from './reserved';

const SPEC_7_2: CommandId[] = [
  'palette.open',
  'pane.new',
  'tab.new',
  'pane.nextWaiting',
  ...TAB_DIGITS.map((d): CommandId => `tab.go.${d}`),
  'pane.prev',
  'pane.next',
  'tab.prev',
  'tab.next',
  'pane.zoom',
  'pane.terminalHere',
  'pane.close',
  'font.up',
  'font.down',
  'font.reset',
  'settings.open',
];

function key(k: string, mods: Partial<NonNullable<EventPayload['modifiers']>> = {}): EventPayload {
  return {
    elementId: 0,
    eventType: 'keyDown',
    key: k,
    modifiers: { shift: false, ctrl: false, alt: false, cmd: false, ...mods },
  };
}

describe('keymap (K4, INV-6)', () => {
  test('declares every binding of spec 7.2 exactly once', () => {
    expect(bindings.map((b) => b.command).sort()).toEqual([...SPEC_7_2].sort());
    for (const id of SPEC_7_2) expect(bindingFor(id)?.command).toBe(id);
  });

  test('has no duplicate keystroke and writes every keystroke canonically', () => {
    const keys = bindings.map((b) => b.keys);
    expect(new Set(keys).size).toBe(keys.length);
    for (const k of keys) expect(normalizeKeys(k)).toBe(k);
  });

  test('every chord carries ⌘ (K1)', () => {
    for (const b of bindings) expect(parseKeys(b.keys).modifiers.has('cmd')).toBe(true);
  });

  test('no binding shadows a reserved macOS shortcut except with its standard meaning (K3)', () => {
    for (const b of bindings) expect(reservedConflict(b.keys, b.command)).toBeUndefined();
    expect(RESERVED.map((r) => r.keys)).toContain('cmd-w');
    expect(reservedConflict('cmd-w', 'pane.close')?.meaning).toContain('Close window');
    expect(bindingFor('pane.close')?.keys).toBe('cmd-shift-w');
  });

  test('never binds keys a terminal needs (K2): Esc, Tab, Enter, Ctrl or ⌥ chords without ⌘', () => {
    for (const b of bindings) {
      const { modifiers } = parseKeys(b.keys);
      expect(modifiers.has('ctrl')).toBe(false);
      expect(modifiers.has('alt')).toBe(false);
    }
  });

  test('labels keystrokes as the spec writes them', () => {
    expect(keyLabel('cmd-shift-]')).toBe('⌘⇧]');
    expect(keyLabel('cmd-enter')).toBe('⌘⏎');
    expect(keyLabel('cmd--')).toBe('⌘-');
    expect(keyLabel('alt-cmd-h')).toBe('⌘⌥H');
    expect(parseKeys('cmd--')).toEqual({ modifiers: new Set(['cmd']), key: '-' });
  });

  test('folds AppKit shifted characters back to their key', () => {
    expect(keysOfEvent(key('}', { cmd: true, shift: true }))).toBe('cmd-shift-]');
    expect(keysOfEvent(key(']', { cmd: true, shift: true }))).toBe('cmd-shift-]');
    expect(keysOfEvent(key('+', { cmd: true, shift: true }))).toBe('cmd-=');
    expect(keysOfEvent(key('=', { cmd: true, shift: true }))).toBe('cmd-=');
    expect(keysOfEvent(key('K', { cmd: true }))).toBe('cmd-k');
  });
});

describe('dispatcher', () => {
  function setup(overlay = false) {
    const actions: Action[] = [];
    const state = makeState([makePane({ id: 1 })], [{ id: 1 }], {
      overlay: overlay ? { kind: 'palette' } : null,
    });
    const press = createKeyDispatcher({
      getState: () => state,
      dispatch: (a) => actions.push(a),
    });
    return { actions, press };
  }

  test('runs the command of a bound ⌘ chord', () => {
    const { actions, press } = setup();
    press(key('k', { cmd: true }));
    press(key('}', { cmd: true, shift: true }));
    press(key('w', { cmd: true, shift: true }));
    expect(actions).toEqual([
      { type: 'command', id: 'palette.open' },
      { type: 'command', id: 'tab.next' },
      { type: 'command', id: 'pane.close' },
    ]);
  });

  test('ignores keys without ⌘, unbound and reserved chords, and everything while an overlay is open', () => {
    const plain = setup();
    for (const k of ['escape', 'tab', 'enter', 'k', '1']) plain.press(key(k));
    plain.press(key('c', { ctrl: true }));
    plain.press(key('w', { cmd: true }));
    plain.press(key('c', { cmd: true }));
    expect(plain.actions).toEqual([]);
    const withOverlay = setup(true);
    withOverlay.press(key('enter', { cmd: true }));
    expect(withOverlay.actions).toEqual([]);
  });
});
