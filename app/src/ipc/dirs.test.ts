import { afterEach, describe, expect, test } from 'bun:test';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { completeDir, type DirVisit, recentDirs, scanRepos } from './dirs';

const cleanups: (() => void)[] = [];

afterEach(() => {
  for (const c of cleanups.splice(0).reverse()) c();
});

/** A temporary directory standing in for the home directory; `make` creates folders and `.git` markers in it. */
function tree(): {
  root: string;
  make: (...paths: string[]) => void;
  repo: (path: string) => void;
} {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'ply-dirs-')));
  cleanups.push(() => rmSync(root, { recursive: true, force: true }));
  const make = (...paths: string[]) => {
    for (const p of paths) mkdirSync(join(root, p), { recursive: true });
  };
  return { root, make, repo: (path) => make(join(path, '.git')) };
}

describe('the repository scan', () => {
  test('finds repositories breadth-first down to depth 4, and a .git file counts', async () => {
    const t = tree();
    t.repo('code/ply');
    t.repo('a/b/c/four');
    t.repo('a/b/c/d/five');
    t.make('code/wt');
    writeFileSync(
      join(t.root, 'code/wt/.git'),
      'gitdir: /Users/example/code/ply/.git/worktrees/wt\n',
    );
    const scan = await scanRepos(t.root);
    expect(scan.repos.sort()).toEqual(
      [join(t.root, 'a/b/c/four'), join(t.root, 'code/ply'), join(t.root, 'code/wt')].sort(),
    );
    expect(scan.truncated).toBe(false);
  });

  test('skips hidden folders, ~/Library, node_modules, target and a repository’s own tree', async () => {
    const t = tree();
    t.repo('.config/dotfiles');
    t.repo('Library/Mobile/notes');
    t.repo('code/web/node_modules/pkg');
    t.repo('code/rust/target/debug');
    t.repo('code/mono');
    t.repo('code/mono/vendor/inner');
    t.repo('code/Library/kept');
    const scan = await scanRepos(t.root);
    expect(scan.repos.sort()).toEqual([
      join(t.root, 'code/Library/kept'),
      join(t.root, 'code/mono'),
    ]);
  });

  test('never follows a symlinked folder', async () => {
    const t = tree();
    const outside = tree();
    outside.repo('elsewhere');
    symlinkSync(join(outside.root, 'elsewhere'), join(t.root, 'linked-repo'));
    symlinkSync(outside.root, join(t.root, 'linked-parent'));
    symlinkSync(t.root, join(t.root, 'loop'));
    t.repo('code/real');
    const scan = await scanRepos(t.root);
    expect(scan.repos).toEqual([join(t.root, 'code/real')]);
  });

  test('stops after the visit limit or the time limit and says it was cut short', async () => {
    const t = tree();
    for (const name of ['a', 'b', 'c', 'd', 'e']) t.repo(`x/${name}/repo`);
    const few = await scanRepos(t.root, { maxVisits: 3 });
    expect(few.visited).toBe(3);
    expect(few.truncated).toBe(true);
    const none = await scanRepos(t.root, { maxMs: 0 });
    expect(none).toMatchObject({ repos: [], visited: 0, truncated: true });
    const all = await scanRepos(t.root);
    expect(all.repos).toHaveLength(5);
    expect(all.visited).toBe(12);
  });

  test('a home directory that is itself a repository is still searched, and a missing one finds nothing', async () => {
    const t = tree();
    t.repo('');
    t.repo('code/ply');
    expect((await scanRepos(t.root)).repos).toEqual([join(t.root, 'code/ply')]);
    expect(await scanRepos(join(t.root, 'missing'))).toMatchObject({ repos: [], visited: 1 });
  });

  test('a listing longer than one slice is read to its end', async () => {
    const t = tree();
    for (let i = 0; i < 2_500; i++) t.make(`many/d${String(i).padStart(4, '0')}`);
    t.repo('many/d2499/repo');
    const scan = await scanRepos(t.root);
    expect(scan.repos).toEqual([join(t.root, 'many/d2499/repo')]);
    expect(scan.visited).toBe(2 + 2_500 + 1);
  });
});

describe('path completion', () => {
  test('lists the sub-folders of the folder, symlinked ones included, files not', async () => {
    const t = tree();
    t.make('code/ply', 'code/api', 'code/.hidden', 'other');
    writeFileSync(join(t.root, 'code/README.md'), 'x');
    symlinkSync(join(t.root, 'other'), join(t.root, 'code/linked'));
    symlinkSync(join(t.root, 'code/README.md'), join(t.root, 'code/file-link'));
    expect(await completeDir(join(t.root, 'code'))).toEqual({
      dir: join(t.root, 'code'),
      children: ['.hidden', 'api', 'linked', 'ply'].map((n) => join(t.root, 'code', n)),
    });
  });

  test('a folder that does not exist completes from its deepest existing ancestor', async () => {
    const t = tree();
    t.make('code/ply');
    const listed = await completeDir(join(t.root, 'code/nope/deeper'));
    expect(listed).toEqual({ dir: join(t.root, 'code'), children: [join(t.root, 'code/ply')] });
  });
});

describe('recent folders', () => {
  test('open panes first, then stored sessions by last activity, each folder once, missing ones dropped', async () => {
    const t = tree();
    t.make('ply', 'api', 'notes', 'old');
    const at = (name: string) => join(t.root, name);
    const panes: DirVisit[] = [
      { cwd: at('ply'), created_at: 10, last_activity_at: 50 },
      { cwd: at('api'), created_at: 10, last_activity_at: 90 },
    ];
    const sessions: DirVisit[] = [
      { cwd: at('api'), created_at: 10, last_activity_at: 90 },
      { cwd: at('old'), created_at: 1, closed_at: 5 },
      { cwd: at('notes'), created_at: 2, closed_at: 40, last_activity_at: 30 },
      { cwd: at('gone'), created_at: 3, closed_at: 60 },
      { cwd: at('ply'), created_at: 4, closed_at: 70 },
    ];
    expect(await recentDirs(sessions, panes)).toEqual([
      at('api'),
      at('ply'),
      at('notes'),
      at('old'),
    ]);
    expect(await recentDirs(sessions, [], 2)).toEqual([at('api'), at('ply')]);
  });
});
