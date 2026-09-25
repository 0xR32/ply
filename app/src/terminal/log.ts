/** Severity of a terminal log line, lowest first (the same levels as the app log). */
export type TerminalLogLevel = 'debug' | 'info' | 'warn' | 'error';

/** Structured context of one line; `pane_id` is set whenever a pane is involved (spec 9.1). */
export type TerminalLogFields = Record<string, string | number | boolean | null | undefined>;

/** Where terminal log lines go; the composition root points it at the app log, which terminal/ may not import (spec 8.2). */
export type TerminalLogSink = (
  level: TerminalLogLevel,
  message: string,
  fields: TerminalLogFields,
) => void;

const kept: string[] = [];
const KEPT_LINES = 500;

function defaultSink(level: TerminalLogLevel, message: string, fields: TerminalLogFields): void {
  const line = `${level.toUpperCase()} ${message} ${JSON.stringify(fields)}`;
  // bun test sets NODE_ENV=test; tests read the lines back instead of printing them.
  if (process.env.NODE_ENV === 'test') {
    if (kept.push(line) > KEPT_LINES) kept.shift();
    return;
  }
  console.error(`ply terminal: ${line}`);
}

let sink: TerminalLogSink = defaultSink;

/** Routes every later terminal log line to `next`; returns the previous sink so a test can restore it. */
export function setTerminalLogSink(next: TerminalLogSink): TerminalLogSink {
  const previous = sink;
  sink = next;
  return previous;
}

/** Logs one line; never throws, so it is safe inside catch blocks and socket callbacks. */
export function terminalLog(
  level: TerminalLogLevel,
  message: string,
  fields: TerminalLogFields = {},
): void {
  try {
    sink(level, message, fields);
  } catch (error) {
    console.error(`ply terminal: ${message} (log sink failed: ${String(error)})`);
  }
}

/** Lines the default sink kept under `bun test`, oldest first. */
export function keptTerminalLogLines(): readonly string[] {
  return kept;
}
