import type { EventPayload, WindowKeyEventHandlers } from '@gpuix/react';
import type { Action } from '../state/actions';
import type { AppState } from '../state/reducer';
import { bindingForKeys, keysOfEvent } from './keymap';

/** What the dispatcher needs from the store. */
export interface KeyTarget {
  getState(): AppState;
  dispatch(action: Action): void;
}

/** The window key listener: runs ⌘ chords only (K1), and nothing while an overlay owns the keys (7.4). */
export function createKeyDispatcher(target: KeyTarget): (event: EventPayload) => void {
  return (event) => {
    if (!event.modifiers?.cmd) return;
    // JS listeners cannot stop propagation, so an overlay's own ⌘⏎ must never close it before this check runs.
    if (target.getState().overlay !== null) return;
    const keys = keysOfEvent(event);
    const binding = keys === null ? undefined : bindingForKeys(keys);
    if (binding) target.dispatch({ type: 'command', id: binding.command });
  };
}

/** The `render()` / `createTestRoot()` options that install the dispatcher as the window-level listener. */
export function windowKeyListeners(target: KeyTarget): WindowKeyEventHandlers {
  return { onKeyDown: createKeyDispatcher(target) };
}
