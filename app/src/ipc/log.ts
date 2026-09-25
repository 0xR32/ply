import { mkdirSync, readdirSync, statSync, unlinkSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

/** Severity of one log line, lowest first. */
export type LogLevel = 'debug' | 'info' | 'warn' | 'error';

/** Structured context attached to a log line; values are JSON-encoded. */
export type LogFields = Record<string, string | number | boolean | null | undefined>;

type LogSink = (line: string) => void;

const RETAIN_DAYS = 14;
const KEPT_TEST_LINES = 2000;
const lines: string[] = [];
let sink: LogSink | null = null;

/** The app log directory: `$PLY_HOME/logs` under tests and development, else `~/Library/Logs/ply` (spec 11.1). */
export function logDir(env: NodeJS.ProcessEnv = process.env): string {
  return env.PLY_HOME ? join(env.PLY_HOME, 'logs') : join(homedir(), 'Library', 'Logs', 'ply');
}

function day(now: Date): string {
  return now.toISOString().slice(0, 10);
}

function pruneOld(dir: string, now: Date): void {
  const cutoff = now.getTime() - RETAIN_DAYS * 86_400_000;
  for (const name of readdirSync(dir)) {
    if (!/^app\.\d{4}-\d{2}-\d{2}\.log$/.test(name)) continue;
    const path = join(dir, name);
    if (statSync(path).mtimeMs < cutoff) unlinkSync(path);
  }
}

function fileSink(): LogSink {
  const dir = logDir();
  mkdirSync(dir, { recursive: true });
  pruneOld(dir, new Date());
  let openDay = '';
  let writer: ReturnType<ReturnType<typeof Bun.file>['writer']> | null = null;
  return (line) => {
    const today = day(new Date());
    if (today !== openDay || writer === null) {
      void writer?.end();
      writer = Bun.file(join(dir, `app.${today}.log`)).writer();
      openDay = today;
    }
    writer.write(`${line}\n`);
    void writer.flush();
  };
}

function defaultSink(): LogSink {
  // bun test sets NODE_ENV=test; tests read lines back through recentLogLines() instead of files.
  if (process.env.NODE_ENV === 'test') {
    return (line) => {
      if (lines.push(line) > KEPT_TEST_LINES) lines.shift();
    };
  }
  try {
    return fileSink();
  } catch (error) {
    console.error(`ply: cannot open the app log in ${logDir()}: ${String(error)}`);
    return (line) => console.error(line);
  }
}

/** The last 2 000 lines logged under `bun test`, oldest first (the file sink keeps none). */
export function recentLogLines(): readonly string[] {
  return lines;
}

/** Writes one line `<iso time> <LEVEL> <message> {fields}`; never throws, so it is safe inside catch blocks. */
export function log(level: LogLevel, message: string, fields: LogFields = {}): void {
  const extra = Object.entries(fields).filter(([, v]) => v !== undefined);
  const tail = extra.length > 0 ? ` ${JSON.stringify(Object.fromEntries(extra))}` : '';
  const line = `${new Date().toISOString()} ${level.toUpperCase()} ${message}${tail}`;
  try {
    sink ??= defaultSink();
    sink(line);
  } catch (error) {
    console.error(`${line} (log sink failed: ${String(error)})`);
  }
}
