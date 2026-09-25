import type { Dirent } from 'node:fs';
import { readdir, stat } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { log } from './log';
import type { Session } from './proto.gen';

/** The fields of a pane or a stored session that date its working directory. */
export type DirVisit = Pick<Session, 'cwd' | 'created_at' | 'closed_at' | 'last_activity_at'>;

/** What one repository scan found and what it cost; `truncated` when a limit ended it early. */
export interface RepoScan {
  repos: string[];
  visited: number;
  elapsedMs: number;
  truncated: boolean;
}

/** Bounds of the repository scan (Ruling R57): depth below the root, directories listed, wall time, parallel listings. */
export interface ScanLimits {
  maxDepth: number;
  maxVisits: number;
  maxMs: number;
  concurrency: number;
}

/** The folder sources of the new-pane form's directory search; effects.ts is their only caller, tests pass their own. */
export interface DirSources {
  recentDirs: typeof recentDirs;
  scanRepos: typeof scanRepos;
  completeDir: typeof completeDir;
}

/** The limits the app scans with. */
export const SCAN_LIMITS: Readonly<ScanLimits> = {
  maxDepth: 4,
  maxVisits: 5_000,
  maxMs: 2_000,
  concurrency: 8,
};

const MAX_RECENT = 50;
const MAX_CHILDREN = 1_000;
const SKIPPED = new Set(['node_modules', 'target']);
const SLICE = 1_000;

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

async function isDirectory(path: string): Promise<boolean> {
  try {
    return (await stat(path)).isDirectory();
  } catch {
    return false;
  }
}

// Under Bun, readdir holds the main thread far less than opendir's iteration (measured); slices keep a huge listing's walk inside R-R15's 2 ms.
async function entries(dir: string, visit: (entry: Dirent) => boolean): Promise<void> {
  const list = await readdir(dir, { withFileTypes: true });
  for (let at = 0; at < list.length; at += SLICE) {
    if (at > 0) await new Promise((resume) => setImmediate(resume));
    for (const entry of list.slice(at, at + SLICE)) if (!visit(entry)) return;
  }
}

/** The recent folders of `sessions` and `panes`, most recent first: open ones first, then by last activity; de-duplicated, at most 50, only folders that still exist. */
export async function recentDirs(
  sessions: readonly DirVisit[],
  panes: readonly DirVisit[],
  limit = MAX_RECENT,
): Promise<string[]> {
  const at = (v: DirVisit) => v.last_activity_at ?? v.closed_at ?? v.created_at;
  const ordered = [...panes, ...sessions]
    .filter((v) => v.cwd.startsWith('/'))
    .sort(
      (a, b) =>
        Number(a.closed_at !== undefined) - Number(b.closed_at !== undefined) || at(b) - at(a),
    );
  const unique = [...new Set(ordered.map((v) => v.cwd))];
  const present = await Promise.all(unique.map(isDirectory));
  return unique.filter((_, i) => present[i]).slice(0, limit);
}

/** Git repositories under `root` (a directory holding `.git`), shallowest first: breadth-first to `maxDepth`, never into hidden directories, `<root>/Library`, `node_modules`, `target`, a symlink or a repository's own tree; an unreadable directory is skipped; stops at `maxVisits` directories listed or after `maxMs`. */
export async function scanRepos(root: string, limits: Partial<ScanLimits> = {}): Promise<RepoScan> {
  const { maxDepth, maxVisits, maxMs, concurrency } = { ...SCAN_LIMITS, ...limits };
  const started = performance.now();
  const elapsed = () => performance.now() - started;
  const library = join(root, 'Library');
  const repos: string[] = [];
  let visited = 0;
  let truncated = false;
  let level = [root];
  for (let depth = 0; depth <= maxDepth && level.length > 0 && !truncated; depth++) {
    const next: string[] = [];
    const current = level;
    let index = 0;
    const worker = async () => {
      while (index < current.length) {
        if (visited >= maxVisits || elapsed() >= maxMs) {
          truncated = true;
          return;
        }
        const dir = current[index++] as string;
        visited++;
        const children: string[] = [];
        let repo = false;
        try {
          await entries(dir, (entry) => {
            if (entry.name === '.git') repo = true;
            else if (
              entry.isDirectory() &&
              !entry.name.startsWith('.') &&
              !SKIPPED.has(entry.name)
            ) {
              const child = join(dir, entry.name);
              if (child !== library) children.push(child);
            }
            return elapsed() < maxMs;
          });
        } catch {
          continue;
        }
        if (repo && depth > 0) repos.push(dir);
        else if (depth < maxDepth) next.push(...children);
      }
    };
    await Promise.all(Array.from({ length: Math.min(concurrency, current.length) }, worker));
    level = next;
    if (elapsed() >= maxMs && level.length > 0) truncated = true;
  }
  return { repos, visited, elapsedMs: Math.round(elapsed()), truncated };
}

/** The deepest existing directory of `dir` (itself or its nearest existing ancestor) and its sub-folders, symlinked ones included, sorted, at most 1 000. */
export async function completeDir(
  dir: string,
  limit = MAX_CHILDREN,
): Promise<{ dir: string; children: string[] }> {
  let at = dir;
  while (!(await isDirectory(at)) && dirname(at) !== at) at = dirname(at);
  const children: string[] = [];
  const links: string[] = [];
  try {
    await entries(at, (entry) => {
      if (entry.isDirectory()) children.push(join(at, entry.name));
      else if (entry.isSymbolicLink()) links.push(join(at, entry.name));
      return children.length + links.length < limit;
    });
  } catch (error) {
    log('debug', 'cannot list a folder for completion', { error: errorText(error) });
  }
  const linked = await Promise.all(links.map(isDirectory));
  children.push(...links.filter((_, i) => linked[i]));
  return { dir: at, children: children.sort() };
}
