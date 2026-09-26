import { describe, expect, test } from 'bun:test';
import type { EventPayload } from '@gpuix/react';
import type { Action, CommandId } from '../state/actions';
import { makePane, makeState } from '../state/test-support';
import {
  createKeyDispatcher,
  DEFAULT_KEY_REPEAT,
  HOLD_MAX_MS,
  HOLD_SLACK_MS,
  type HoldClock,
  holdTimeoutMs,
} from './dispatcher';
import {
  bindingFor,
  bindings,
  keyLabel,
  keysOfEvent,
  normalizeKeys,
  parseKeys,
  TAB_DIGITS,
  terminalBindings,
  terminalCommandForKeys,
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
  'pane.left',
  'pane.right',
  'pane.up',
  'pane.down',
  'tab.prev',
  'tab.next',
  'pane.zoom',
  'pane.terminalHere',
  'pane.close',
  'font.up',
  'font.down',
  'font.reset',
  'settings.open',
  'usage.show',
  'task.dispatch',
  'task.queue',
];

function key(
  k: string,
  mods: Partial<NonNullable<EventPayload['modifiers']>> = {},
  isHeld = false,
): EventPayload {
  return {
    elementId: 0,
    eventType: 'keyDown',
    key: k,
    isHeld,
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

describe('terminal chords (spec 7.3, K7)', () => {
  test('are the terminal-owned reserved shortcuts with their standard meaning, declared once', () => {
    const owned = RESERVED.filter((r) => r.owner === 'terminal');
    for (const b of terminalBindings) {
      const hit = owned.find((r) => r.keys === b.keys);
      expect(hit?.meaning).toBe(b.label);
      expect(normalizeKeys(b.keys)).toBe(b.keys);
      expect(bindings.some((g) => g.keys === b.keys)).toBe(false);
    }
    expect(terminalBindings.map((b) => b.keys)).toEqual(['cmd-c', 'cmd-v', 'cmd-a', 'cmd-f']);
    expect(terminalCommandForKeys(keysOfEvent(key('c', { cmd: true })) ?? '')).toBe(
      'terminal.copy',
    );
    expect(terminalCommandForKeys('cmd-x')).toBeUndefined();
    expect(terminalCommandForKeys('ctrl-c')).toBeUndefined();
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
    }).keyDown;
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

/** A clock whose timers run only when the test advances it. */
function fakeClock(): HoldClock & { advance(ms: number): void; pending(): number } {
  let now = 0;
  let next = 1;
  const timers = new Map<number, { at: number; run: () => void }>();
  return {
    setTimeout: (run, ms) => {
      const id = next++;
      timers.set(id, { at: now + ms, run });
      return id;
    },
    clearTimeout: (id) => {
      timers.delete(id as number);
    },
    advance(ms) {
      now += ms;
      for (const [id, t] of [...timers].sort((a, b) => a[1].at - b[1].at)) {
        if (t.at > now) continue;
        timers.delete(id);
        t.run();
      }
    },
    pending: () => timers.size,
  };
}

describe('the ⌘U hold (R59)', () => {
  function setup(keyRepeat?: { delayMs?: number; intervalMs?: number }) {
    const actions: Action[] = [];
    let state = makeState([makePane({ id: 1 })], [{ id: 1 }]);
    if (keyRepeat) state = { ...state, env: { ...state.env, keyRepeat } };
    const clock = fakeClock();
    const keys = createKeyDispatcher(
      {
        getState: () => state,
        dispatch: (a) => {
          actions.push(a);
          if (a.type === 'command' && a.id === 'usage.show') {
            state = { ...state, usage: { ...state.usage, shown: true } };
          } else if (a.type === 'usage/hide') {
            state = { ...state, usage: { ...state.usage, shown: false } };
          } else if (a.type === 'overlay/open') {
            state = { ...state, overlay: a.overlay };
          }
        },
      },
      clock,
    );
    const up = (k: string, mods: Partial<NonNullable<EventPayload['modifiers']>> = {}) =>
      keys.keyUp({ ...key(k, mods), eventType: 'keyUp' });
    return { actions, keys, clock, up, shown: () => state.usage.shown };
  }
  const cmdU = (held = false) => key('u', { cmd: true }, held);

  test('⌘U shows the view and the key-up of u hides it', () => {
    const h = setup();
    h.keys.keyDown(cmdU());
    expect(h.shown()).toBe(true);
    expect(h.actions).toEqual([{ type: 'command', id: 'usage.show' }]);
    h.up('u');
    expect(h.shown()).toBe(false);
    expect(h.actions.at(-1)).toEqual({ type: 'usage/hide' });
    expect(h.clock.pending()).toBe(0);
  });

  test('key repeats keep it open without running the command again; their stopping hides it', () => {
    const h = setup();
    h.keys.keyDown(cmdU());
    h.clock.advance(DEFAULT_KEY_REPEAT.delayMs);
    for (let i = 0; i < 20; i++) {
      h.keys.keyDown(cmdU(true));
      h.clock.advance(DEFAULT_KEY_REPEAT.intervalMs);
    }
    expect(h.shown()).toBe(true);
    expect(h.actions).toEqual([{ type: 'command', id: 'usage.show' }]);
    h.clock.advance(HOLD_SLACK_MS);
    expect(h.shown()).toBe(false);
    expect(h.actions.at(-1)).toEqual({ type: 'usage/hide' });
  });

  test('a press released before the first repeat hides after the repeat delay', () => {
    const h = setup({ delayMs: 225, intervalMs: 30 });
    h.keys.keyDown(cmdU());
    h.clock.advance(225 + HOLD_SLACK_MS - 1);
    expect(h.shown()).toBe(true);
    h.clock.advance(1);
    expect(h.shown()).toBe(false);
  });

  test('another key, a u without ⌘ and a blur each hide it, and the other key still does its job', () => {
    const other = setup();
    other.keys.keyDown(cmdU());
    other.keys.keyDown(key('k', { cmd: true }));
    expect(other.shown()).toBe(false);
    expect(other.actions).toEqual([
      { type: 'command', id: 'usage.show' },
      { type: 'usage/hide' },
      { type: 'command', id: 'palette.open' },
    ]);
    const released = setup();
    released.keys.keyDown(cmdU());
    released.keys.keyDown(key('u', {}, true));
    expect(released.shown()).toBe(false);
    const blurred = setup();
    blurred.keys.keyDown(cmdU());
    blurred.keys.event({ elementId: 3, eventType: 'blur' });
    expect(blurred.shown()).toBe(false);
    expect(blurred.clock.pending()).toBe(0);
  });

  test('key-ups of other keys and events other than blur leave it open', () => {
    const h = setup();
    h.keys.keyDown(cmdU());
    h.up('shift');
    h.keys.event({ elementId: 3, eventType: 'focus' });
    expect(h.shown()).toBe(true);
  });

  test('the wait is the repeat timing plus a grace, capped so key repeat off still hides it', () => {
    expect(holdTimeoutMs(DEFAULT_KEY_REPEAT, false)).toBe(500 + HOLD_SLACK_MS);
    expect(holdTimeoutMs(DEFAULT_KEY_REPEAT, true)).toBe(83 + HOLD_SLACK_MS);
    expect(holdTimeoutMs({ delayMs: 300_000 * 15, intervalMs: 30 }, false)).toBe(HOLD_MAX_MS);
    const off = setup({ delayMs: 300_000 * 15 });
    off.keys.keyDown(cmdU());
    off.clock.advance(HOLD_MAX_MS);
    expect(off.shown()).toBe(false);
  });
});
