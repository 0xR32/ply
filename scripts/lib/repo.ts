import { existsSync, lstatSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, sep } from 'node:path';

/** One rule violation; `file` is repo-relative with forward slashes, `line` is 1-based. */
export interface Violation {
  check: string;
  file?: string;
  line?: number;
  message: string;
}

/** What one check found: violations fail the run, notices (e.g. "skipped until WP3") never do. */
export interface CheckResult {
  violations: Violation[];
  notices: string[];
}

/** Captured output of a finished child process; `code` is 1 when the process was killed by a signal. */
export interface RunResult {
  code: number;
  stdout: string;
  stderr: string;
}

/** Runs `cmd` in `cwd` synchronously; never throws for a non-zero exit. */
export function run(cmd: string[], cwd: string, env?: Record<string, string>): RunResult {
  const p = Bun.spawnSync({
    cmd,
    cwd,
    stdout: 'pipe',
    stderr: 'pipe',
    env: env ? { ...process.env, ...env } : process.env,
  });
  return { code: p.exitCode ?? 1, stdout: p.stdout.toString(), stderr: p.stderr.toString() };
}

const WALK_SKIP = new Set([
  '.git',
  'node_modules',
  'target',
  '.superpowers',
  'zig-out',
  '.zig-cache',
  'zig-pkg',
]);

function walk(root: string, dir: string, out: string[]): void {
  for (const name of readdirSync(dir)) {
    if (WALK_SKIP.has(name) || name === '.DS_Store') continue;
    const abs = join(dir, name);
    const st = lstatSync(abs);
    if (st.isDirectory()) walk(root, abs, out);
    else if (st.isFile()) out.push(relative(root, abs).split(sep).join('/'));
  }
}

/** Repo files that exist on disk: tracked plus untracked-but-not-ignored (a plain walk outside git). */
export function listRepoFiles(root: string): string[] {
  const git = run(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z'], root);
  if (git.code !== 0) {
    const out: string[] = [];
    walk(root, root, out);
    return out.sort();
  }
  const files = git.stdout.split('\0').filter((f) => f.length > 0);
  return [...new Set(files)]
    .filter((f) => existsSync(join(root, f)) && statSync(join(root, f)).isFile())
    .sort();
}

/** Reads a repo-relative text file, or returns null when it does not exist. */
export function readText(root: string, file: string): string | null {
  const abs = join(root, file);
  if (!existsSync(abs)) return null;
  return readFileSync(abs, 'utf8');
}

/** True when the first 8 KiB contain a NUL byte, the usual binary-file heuristic. */
export function isBinary(root: string, file: string): boolean {
  const buf = readFileSync(join(root, file));
  return buf.subarray(0, 8192).includes(0);
}

/** Every match of a global regex with its 1-based line number. */
export function matchLines(text: string, re: RegExp): { line: number; match: string }[] {
  const flags = re.flags.includes('g') ? re.flags : `${re.flags}g`;
  const global = new RegExp(re.source, flags);
  const out: { line: number; match: string }[] = [];
  for (const m of text.matchAll(global)) {
    const line = text.slice(0, m.index).split('\n').length;
    out.push({ line, match: m[0] });
  }
  return out;
}

/** Formats a violation as the single output line check scripts print. */
export function formatViolation(v: Violation): string {
  const where = v.file ? `${v.file}${v.line ? `:${v.line}` : ''}: ` : '';
  return `${v.check} ${where}${v.message}`;
}
