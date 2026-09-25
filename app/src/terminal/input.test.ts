import { describe, expect, test } from 'bun:test';
import { existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import type { EventPayload } from '@gpuix/react';
import { MAX_KEY_CODE, Mods } from './frames';
import { GHOSTTY_KEYS, keyCode, keyFrame, modsOf, mouseButton, mouseFrame } from './input';

const BUILD_RS = join(import.meta.dir, '..', '..', '..', 'crates', 'ghostty-sys', 'build.rs');

/** The ghostty source ghostty-sys/build.rs builds from: `PLY_GHOSTTY_SRC`, else the pinned commit in the cache. */
function ghosttySource(): string {
  const override = process.env.PLY_GHOSTTY_SRC;
  if (override) return override;
  const commit = /const GHOSTTY_COMMIT: &str = "([0-9a-f]{40})";/.exec(
    readFileSync(BUILD_RS, 'utf8'),
  )?.[1];
  if (!commit) throw new Error(`${BUILD_RS} pins no GHOSTTY_COMMIT`);
  const xdg = process.env.XDG_CACHE_HOME;
  const cache =
    process.platform === 'darwin'
      ? join(homedir(), 'Library', 'Caches')
      : xdg?.startsWith('/')
        ? xdg
        : join(homedir(), '.cache');
  return join(cache, 'ply', 'ghostty', commit);
}

const HEADER = join(ghosttySource(), 'include', 'ghostty', 'vt', 'key', 'event.h');

function key(
  k: string,
  keyChar?: string,
  mods: Partial<NonNullable<EventPayload['modifiers']>> = {},
  isHeld = false,
): EventPayload {
  return {
    elementId: 1,
    eventType: 'keyDown',
    key: k,
    ...(keyChar !== undefined ? { keyChar } : {}),
    isHeld,
    modifiers: { shift: false, ctrl: false, alt: false, cmd: false, ...mods },
  };
}

const code = (name: string) => GHOSTTY_KEYS.indexOf(name);

describe('GhosttyKey table', () => {
  test('matches libghostty-vt key/event.h name for name and in order', () => {
    if (!existsSync(HEADER)) {
      throw new Error(`${HEADER} is missing: build plyd once to download the ghostty source`);
    }
    const header = readFileSync(HEADER, 'utf8');
    const block = header.slice(header.indexOf('GHOSTTY_KEY_UNIDENTIFIED'));
    const names = [...block.matchAll(/GHOSTTY_KEY_([A-Z0-9_]+)\s*[,=]/g)]
      .map((m) => m[1] as string)
      .filter((n) => n !== 'MAX_VALUE');
    expect(GHOSTTY_KEYS).toEqual(names);
    expect(GHOSTTY_KEYS.length).toBe(MAX_KEY_CODE + 1);
  });

  test('maps GPUIX key names to physical keys and unshifted codepoints', () => {
    expect(keyCode('a')).toEqual({ key: code('A'), unshifted: 0x61 });
    expect(keyCode('A')).toEqual({ key: code('A'), unshifted: 0x61 });
    expect(keyCode('7')).toEqual({ key: code('DIGIT_7'), unshifted: 0x37 });
    expect(keyCode('[')).toEqual({ key: code('BRACKET_LEFT'), unshifted: 0x5b });
    expect(keyCode('}')).toEqual({ key: code('BRACKET_RIGHT'), unshifted: 0x5d });
    expect(keyCode('!')).toEqual({ key: code('DIGIT_1'), unshifted: 0x31 });
    expect(keyCode('enter')).toEqual({ key: code('ENTER'), unshifted: 0 });
    expect(keyCode('space')).toEqual({ key: code('SPACE'), unshifted: 0x20 });
    expect(keyCode('pagedown').key).toBe(code('PAGE_DOWN'));
    expect(keyCode('left').key).toBe(code('ARROW_LEFT'));
    expect(keyCode('f12').key).toBe(code('F12'));
    expect(keyCode('ö')).toEqual({ key: 0, unshifted: 0xf6 });
    expect(keyCode('shift')).toEqual({ key: 0, unshifted: 0 });
  });
});

describe('keyFrame (R-R5, K2, K5, K6)', () => {
  test('plain, shifted and control keys', () => {
    expect(keyFrame(key('a', 'a'), 'press', 'off')).toEqual({
      kind: 'key',
      key: code('A'),
      mods: 0,
      consumedMods: 0,
      action: 'press',
      composing: false,
      unshiftedCodepoint: 0x61,
      text: 'a',
    });
    expect(keyFrame(key('a', 'A', { shift: true }), 'press', 'off')).toMatchObject({
      mods: Mods.shift,
      consumedMods: Mods.shift,
      text: 'A',
    });
    expect(keyFrame(key('1', '!', { shift: true }), 'press', 'off')).toMatchObject({
      key: code('DIGIT_1'),
      consumedMods: Mods.shift,
      text: '!',
    });
    expect(keyFrame(key('c', undefined, { ctrl: true }), 'press', 'off')).toMatchObject({
      key: code('C'),
      mods: Mods.ctrl,
      text: '',
    });
  });

  test('Tab, ⇧Tab, Esc, Enter and ⇧⏎ go out as keys without text (plyd applies R-R6)', () => {
    expect(keyFrame(key('tab', '\t'), 'press', 'off')).toMatchObject({
      key: code('TAB'),
      text: '',
    });
    expect(keyFrame(key('tab', '\t', { shift: true }), 'press', 'off')).toMatchObject({
      key: code('TAB'),
      mods: Mods.shift,
      text: '',
    });
    expect(keyFrame(key('escape'), 'press', 'off')).toMatchObject({ key: code('ESCAPE') });
    expect(keyFrame(key('enter', '\n', { shift: true }), 'press', 'off')).toMatchObject({
      key: code('ENTER'),
      mods: Mods.shift,
      consumedMods: 0,
      text: '',
    });
    expect(keyFrame(key('up'), 'press', 'off')).toMatchObject({ key: code('ARROW_UP') });
  });

  test('every ⌘ chord is left to the app keymap', () => {
    expect(keyFrame(key('c', undefined, { cmd: true }), 'press', 'off')).toBeNull();
    expect(keyFrame(key('k', undefined, { cmd: true, shift: true }), 'press', 'both')).toBeNull();
  });

  test('⌥ types the layout character by default (German ⌥L is @, ⌥5 is [)', () => {
    expect(keyFrame(key('l', '@', { alt: true }), 'press', 'off')).toMatchObject({
      key: code('L'),
      mods: Mods.alt,
      consumedMods: Mods.alt,
      text: '@',
    });
    expect(keyFrame(key('5', '[', { alt: true }), 'press', 'off')).toMatchObject({
      key: code('DIGIT_5'),
      consumedMods: Mods.alt,
      text: '[',
    });
  });

  test('with ⌥ as Meta the key character goes out with ⌥ unconsumed, so plyd sends ESC + key', () => {
    expect(keyFrame(key('b', '∫', { alt: true }), 'press', 'both')).toMatchObject({
      key: code('B'),
      mods: Mods.alt,
      consumedMods: 0,
      text: 'b',
    });
    expect(keyFrame(key('b', '∫', { alt: true }), 'press', 'right')).toMatchObject({
      mods: Mods.alt | Mods.altSide,
      text: 'b',
    });
    expect(keyFrame(key('b', 'ı', { alt: true, shift: true }), 'press', 'left')).toMatchObject({
      mods: Mods.alt | Mods.shift,
      consumedMods: Mods.shift,
      text: 'B',
    });
  });

  test('layout keys without a physical code send their text; repeats and releases', () => {
    expect(keyFrame(key('ö', 'ö'), 'press', 'off')).toMatchObject({ key: 0, text: 'ö' });
    expect(keyFrame(key('shift'), 'press', 'off')).toBeNull();
    expect(keyFrame(key('a', 'a', {}, true), 'repeat', 'off')).toMatchObject({ action: 'repeat' });
    expect(keyFrame(key('a', 'a'), 'release', 'off')).toMatchObject({
      action: 'release',
      text: '',
    });
  });
});

describe('mouse', () => {
  test('buttons, modifiers and cells', () => {
    expect([0, 1, 2, undefined].map(mouseButton)).toEqual([1, 3, 2, 0]);
    expect(modsOf({ shift: true, ctrl: true, alt: false, cmd: true })).toBe(
      Mods.shift | Mods.ctrl | Mods.super,
    );
    const cell = { width: 7.5, height: 16, cols: 80, rows: 24 };
    expect(mouseFrame('press', 1, 0, 47.5, 51.25, cell)).toEqual({
      kind: 'mouse',
      action: 'press',
      button: 1,
      mods: 0,
      col: 6,
      row: 3,
      x: 47.5,
      y: 51.25,
    });
    expect(mouseFrame('motion', 0, 0, -3, 9_999, cell)).toMatchObject({ col: 0, row: 23 });
  });
});
