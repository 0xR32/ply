import type { CommandId } from '../state/actions';

/** Who owns a reserved chord: GPUIX's app menu (never reaches the app), macOS convention, or the terminal view. */
export type ReservedOwner = 'gpuix-menu' | 'macos' | 'terminal';

/** A standard macOS shortcut ply must not shadow (K3); `standardCommand` is the one command allowed to bind it. */
export interface ReservedShortcut {
  keys: string;
  meaning: string;
  owner: ReservedOwner;
  standardCommand?: CommandId;
}

/** Spec 7.1 K3, in canonical keystroke form (see keymap.ts). */
export const RESERVED: readonly ReservedShortcut[] = [
  { keys: 'cmd-q', meaning: 'Quit', owner: 'gpuix-menu' },
  { keys: 'cmd-h', meaning: 'Hide', owner: 'gpuix-menu' },
  { keys: 'alt-cmd-h', meaning: 'Hide others', owner: 'gpuix-menu' },
  { keys: 'cmd-m', meaning: 'Minimize', owner: 'gpuix-menu' },
  { keys: 'cmd-w', meaning: 'Close window; sessions keep running', owner: 'gpuix-menu' },
  { keys: 'cmd-`', meaning: 'Cycle windows', owner: 'macos' },
  { keys: 'cmd-,', meaning: 'Settings', owner: 'macos', standardCommand: 'settings.open' },
  { keys: 'cmd-c', meaning: 'Copy', owner: 'terminal' },
  { keys: 'cmd-v', meaning: 'Paste', owner: 'terminal' },
  { keys: 'cmd-x', meaning: 'Cut', owner: 'terminal' },
  { keys: 'cmd-a', meaning: 'Select all', owner: 'terminal' },
  { keys: 'cmd-z', meaning: 'Undo', owner: 'terminal' },
  { keys: 'cmd-f', meaning: 'Find', owner: 'terminal' },
];

/** The reserved entry a keystroke would shadow when bound to `command`, or `undefined` if the binding is allowed. */
export function reservedConflict(keys: string, command: CommandId): ReservedShortcut | undefined {
  const hit = RESERVED.find((r) => r.keys === keys);
  return hit && hit.standardCommand !== command ? hit : undefined;
}
