import {
  constants,
  copyFileSync,
  existsSync,
  linkSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  rmSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { terminalLog } from './log';

// Ghostty's set (macOS `Shell.escape`): the CLIs recognise a dropped image path in this form, as iTerm2 and Terminal.app send it.
const SHELL_META = /[\\ ()[\]{}<>"'`!#$&;|*?\t]/g;

// AppKit's item-replacement staging (the ⌘⇧4 thumbnail's `NSIRD_screencaptureui_*`), which the source deletes when the drag ends.
const STAGED = '/TemporaryItems/';

/** A dropped file's absolute path with its shell metacharacters backslash-escaped, as a terminal types it. */
export function escapedPath(path: string): string {
  return path.replace(SHELL_META, '\\$&');
}

/** The text a terminal types for files dropped on it: each path escaped, joined by one space, with no trailing space; Claude Code and Codex turn an image path in a paste into their own `[Image #1]`. */
export function droppedPathsText(paths: readonly string[]): string {
  return paths.map(escapedPath).join(' ');
}

/** Where kept drops go: `$PLY_HOME/ply-drops` when set, else `ply-drops` in the temporary directory, which macOS clears of files unused for days. */
export function dropsDir(env: NodeJS.ProcessEnv = process.env): string {
  return join(env.PLY_HOME ?? tmpdir(), 'ply-drops');
}

/** Whose input a kept drop went into: a pane's, where it is typed at once, or a task's, written in the ⌘E form and typed later. */
export type DropOwner = { pane: number } | 'task';

// A kept folder's name says whose it is and when it was made: `pane-<id>-<ms>-XXXXXX` or `task-<ms>-XXXXXX`.
const TASK_FOLDER = /^task-\d+-[A-Za-z0-9]+(?=\/)/;

/** The path to type for a dropped file: one AppKit staged in `TemporaryItems` is hard-linked (else copied) into a fresh folder under `dir` named for `owner` and the time, keeping its name, since its source deletes it before a CLI reads it; any other path, or a staged file already gone, comes back as dropped. Synchronous, so it runs before the drop's event turn ends; never throws. */
export function keepDroppedFile(path: string, owner: DropOwner, dir: string = dropsDir()): string {
  if (!path.includes(STAGED) || !existsSync(path)) return path;
  try {
    mkdirSync(dir, { recursive: true });
    const prefix = owner === 'task' ? `task-${Date.now()}-` : `pane-${owner.pane}-${Date.now()}-`;
    const kept = join(mkdtempSync(join(dir, prefix)), basename(path));
    try {
      linkSync(path, kept);
    } catch (error) {
      terminalLog('debug', 'a staged drop could not be linked; copying it', {
        path,
        error: String(error),
      });
      copyFileSync(path, kept, constants.COPYFILE_FICLONE);
    }
    terminalLog('info', 'kept a dropped file its source deletes when the drag ends', {
      from: path,
      to: kept,
    });
    return kept;
  } catch (error) {
    terminalLog('warn', 'a staged drop could not be kept; typing its own path', {
      path,
      error: String(error),
    });
    return path;
  }
}

function remove(folder: string, why: string): void {
  try {
    rmSync(folder, { recursive: true, force: true });
    terminalLog('debug', 'deleted a kept drop', { folder, why });
  } catch (error) {
    terminalLog('warn', 'a kept drop could not be deleted', { folder, error: String(error) });
  }
}

/** Deletes the drops kept for pane `paneId` before `before` (ms since the epoch; `Infinity` for every one); never throws. */
export function releasePaneDrops(paneId: number, before: number, dir: string = dropsDir()): void {
  let names: string[];
  try {
    names = existsSync(dir) ? readdirSync(dir) : [];
  } catch (error) {
    terminalLog('warn', 'the kept drops could not be listed', { dir, error: String(error) });
    return;
  }
  const mine = new RegExp(`^pane-${paneId}-(\\d+)-`);
  for (const name of names) {
    const made = mine.exec(name)?.[1];
    if (made !== undefined && Number(made) < before) remove(join(dir, name), `pane ${paneId}`);
  }
}

/** Deletes the task drops whose kept paths `text` names, escaped as the ⌘E form typed them into a task; never throws. */
export function releaseTaskDrops(text: string, dir: string = dropsDir()): void {
  const at = `${escapedPath(dir)}/`;
  for (let i = text.indexOf(at); i >= 0; i = text.indexOf(at, i + at.length)) {
    const name = TASK_FOLDER.exec(text.slice(i + at.length))?.[0];
    if (name !== undefined && existsSync(join(dir, name))) remove(join(dir, name), 'task');
  }
}
