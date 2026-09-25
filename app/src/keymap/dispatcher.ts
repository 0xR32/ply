import type { EventPayload } from '@gpuix/react';
import type { Action, KeyRepeat } from '../state/actions';
import type { AppState } from '../state/reducer';
import { bindingFor, bindingForKeys, keysOfEvent, parseKeys } from './keymap';

/** What the dispatcher needs from the store. */
export interface KeyTarget {
  getState(): AppState;
  dispatch(action: Action): void;
}

/** macOS's key-repeat timing when `defaults` holds none: `NSEvent.keyRepeatDelay` and `keyRepeatInterval` of a fresh account. */
export const DEFAULT_KEY_REPEAT: KeyRepeat = { delayMs: 500, intervalMs: 83 };

/** Grace beyond the key-repeat timing before a ⌘U hold without a new repeat counts as released. */
export const HOLD_SLACK_MS = 150;

/** Longest a ⌘U hold is believed without a repeat, so the view hides even with key repeat turned off. */
export const HOLD_MAX_MS = 2_000;

/** How long after a ⌘U key-down (the first press or a repeat) the hold ends unless another repeat arrives. */
export function holdTimeoutMs(repeat: KeyRepeat, isRepeat: boolean): number {
  const wait = isRepeat ? repeat.intervalMs : repeat.delayMs;
  return Math.min(Math.max(wait, 0) + HOLD_SLACK_MS, HOLD_MAX_MS);
}

/** The timers the ⌘U hold runs on; tests pass a fake clock. */
export interface HoldClock {
  setTimeout(run: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

const realClock: HoldClock = {
  setTimeout: (run, ms) => setTimeout(run, ms),
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/** What the dispatcher does with a window key-down, a key-up, and any handled event (for blur). */
export interface KeyDispatcher {
  keyDown: (event: EventPayload) => void;
  keyUp: (event: EventPayload) => void;
  event: (event: EventPayload) => void;
}

/** The window-level listeners `render()` and `createTestRoot()` take. */
export interface WindowListeners {
  onKeyDown: (event: EventPayload) => void;
  onKeyUp: (event: EventPayload) => void;
  onEvent: (event: EventPayload) => void;
}

/** The window's keys: ⌘ chords only (K1), none while an overlay owns the keys (7.4), and the ⌘U hold (R59, docs/keybindings.md). */
export function createKeyDispatcher(
  target: KeyTarget,
  clock: HoldClock = realClock,
): KeyDispatcher {
  const usageKeys = bindingFor('usage.show')?.keys;
  const usageKey = usageKeys === undefined ? undefined : parseKeys(usageKeys).key;
  let hold: unknown = null;
  const stopHold = () => {
    if (hold === null) return;
    clock.clearTimeout(hold);
    hold = null;
  };
  const hide = () => {
    stopHold();
    if (target.getState().usage.shown) target.dispatch({ type: 'usage/hide' });
  };
  // AppKit sends no key-up for a key released while ⌘ is down, so the repeats stopping is the release it hides.
  const holdOpen = (isRepeat: boolean) => {
    stopHold();
    const repeat = { ...DEFAULT_KEY_REPEAT, ...target.getState().env.keyRepeat };
    hold = clock.setTimeout(
      () => {
        hold = null;
        hide();
      },
      holdTimeoutMs(repeat, isRepeat),
    );
  };
  return {
    keyDown: (event: EventPayload) => {
      const keys = keysOfEvent(event);
      if (keys !== usageKeys && target.getState().usage.shown) hide();
      if (!event.modifiers?.cmd) return;
      // JS listeners cannot stop propagation, so an overlay's own ⌘⏎ must never close it before this check runs.
      if (target.getState().overlay !== null) return;
      const binding = keys === null ? undefined : bindingForKeys(keys);
      if (!binding) return;
      if (binding.command === 'usage.show') {
        holdOpen(event.isHeld === true);
        if (target.getState().usage.shown) return;
      }
      target.dispatch({ type: 'command', id: binding.command });
    },
    keyUp: (event: EventPayload) => {
      if (event.key !== undefined && event.key.toLowerCase() === usageKey) hide();
    },
    event: (event: EventPayload) => {
      if (event.eventType === 'blur') hide();
    },
  };
}

/** The `render()` / `createTestRoot()` options that install the dispatcher as the window-level listeners. */
export function windowKeyListeners(target: KeyTarget): WindowListeners {
  const keys = createKeyDispatcher(target);
  return { onKeyDown: keys.keyDown, onKeyUp: keys.keyUp, onEvent: keys.event };
}
