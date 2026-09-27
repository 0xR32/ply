import { constants, copyFileSync, existsSync, linkSync, mkdirSync, mkdtempSync } from 'node:fs';
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

/** The path to type for a dropped file: one AppKit staged in `TemporaryItems` is hard-linked (else copied) into a fresh folder under `dir`, keeping its name, since its source deletes it before a CLI reads it; any other path, or a staged file already gone, comes back as dropped. Synchronous, so it runs before the drop's event turn ends; never throws. */
export function keepDroppedFile(path: string, dir: string = dropsDir()): string {
  if (!path.includes(STAGED) || !existsSync(path)) return path;
  try {
    mkdirSync(dir, { recursive: true });
    const kept = join(mkdtempSync(join(dir, 'drop-')), basename(path));
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
