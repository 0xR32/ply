// Ghostty's set (macOS `Shell.escape`): the CLIs recognise a dropped image path in this form, as iTerm2 and Terminal.app send it.
const SHELL_META = /[\\ ()[\]{}<>"'`!#$&;|*?\t]/g;

/** A dropped file's absolute path with its shell metacharacters backslash-escaped, as a terminal types it. */
export function escapedPath(path: string): string {
  return path.replace(SHELL_META, '\\$&');
}

/** The text a terminal types for files dropped on it: each path escaped, joined by one space, with no trailing space; Claude Code and Codex turn an image path in a paste into their own `[Image #1]`. */
export function droppedPathsText(paths: readonly string[]): string {
  return paths.map(escapedPath).join(' ');
}
