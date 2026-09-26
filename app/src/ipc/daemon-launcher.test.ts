import { afterEach, describe, expect, test } from 'bun:test';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createDaemonStarter, planLaunch } from './daemon-launcher';
import { fontDirs, geistAvailable, shellName } from './os';

const dirs: string[] = [];

afterEach(() => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

function tree(files: string[]): string {
  const root = mkdtempSync(join(tmpdir(), 'ply-launch-'));
  dirs.push(root);
  for (const f of files) {
    mkdirSync(join(root, f, '..'), { recursive: true });
    writeFileSync(join(root, f), '');
  }
  return root;
}

function fakePlyd(path: string, exitCode: number): void {
  mkdirSync(join(path, '..'), { recursive: true });
  const record = join(path, '..', 'args.txt');
  writeFileSync(path, `#!/bin/sh\necho "$@" > '${record}'\necho boom >&2\nexit ${exitCode}\n`);
  chmodSync(path, 0o755);
}

describe('planLaunch', () => {
  test('a run without PLY_HOME installs the agent for the cargo-built plyd (Ruling R38)', () => {
    const repo = tree(['target/debug/plyd']);
    const plan = planLaunch({ env: {}, repoRoot: repo });
    expect(plan).toEqual({ kind: 'launch-agent', plyd: join(repo, 'target/debug/plyd') });
  });

  test('the release build wins over the debug build, and PLY_PLYD over both', () => {
    const repo = tree(['target/debug/plyd', 'target/release/plyd']);
    expect(planLaunch({ env: {}, repoRoot: repo })).toEqual({
      kind: 'launch-agent',
      plyd: join(repo, 'target/release/plyd'),
    });
    const own = tree(['bin/plyd']);
    expect(planLaunch({ env: { PLY_PLYD: join(own, 'bin/plyd') }, repoRoot: repo })).toEqual({
      kind: 'launch-agent',
      plyd: join(own, 'bin/plyd'),
    });
  });

  test("the bundle's plyd beside the executable wins over the checkout's builds, PLY_PLYD over it", () => {
    const repo = tree(['target/release/plyd']);
    const app = tree(['ply.app/Contents/MacOS/plyd']);
    const exeDir = join(app, 'ply.app/Contents/MacOS');
    expect(planLaunch({ env: {}, repoRoot: repo, exeDir })).toEqual({
      kind: 'launch-agent',
      plyd: join(exeDir, 'plyd'),
    });
    const own = tree(['bin/plyd']);
    expect(
      planLaunch({ env: { PLY_PLYD: join(own, 'bin/plyd') }, repoRoot: repo, exeDir }),
    ).toEqual({ kind: 'launch-agent', plyd: join(own, 'bin/plyd') });
  });

  test('PLY_HOME always spawns a build-dir plyd, never the LaunchAgent, and says when none is built', () => {
    const repo = tree(['target/debug/plyd']);
    expect(planLaunch({ env: { PLY_HOME: '/x' }, repoRoot: repo })).toEqual({
      kind: 'spawn',
      plyd: join(repo, 'target/debug/plyd'),
    });
    const empty = tree([]);
    expect(planLaunch({ env: { PLY_HOME: '/x' }, repoRoot: empty })).toEqual({
      kind: 'unavailable',
      reason: 'plyd is not built (cargo build --release -p ply-daemon -p ply-hook)',
    });
    expect(planLaunch({ env: {}, repoRoot: empty }).kind).toBe('unavailable');
  });
});

describe('createDaemonStarter', () => {
  test('rejects with the reason when plyd cannot be found', async () => {
    const empty = tree([]);
    const start = createDaemonStarter({ env: {}, repoRoot: empty });
    await expect(start()).rejects.toThrow('plyd is not built');
  });

  test('runs `plyd install-agent` instead of writing the plist itself', async () => {
    const repo = tree([]);
    fakePlyd(join(repo, 'target/release/plyd'), 0);
    const start = createDaemonStarter({ env: {}, repoRoot: repo });
    await start();
    expect(readFileSync(join(repo, 'target/release/args.txt'), 'utf8')).toBe('install-agent\n');
  });

  test('a failing install-agent rejects with its status and stderr', async () => {
    const repo = tree([]);
    fakePlyd(join(repo, 'target/release/plyd'), 3);
    const start = createDaemonStarter({ env: {}, repoRoot: repo });
    await expect(start()).rejects.toThrow('plyd install-agent failed (3): boom');
  });
});

describe('os facts', () => {
  test('the shell name comes from $SHELL; Geist counts as available only where macOS finds fonts', () => {
    expect(shellName({ SHELL: '/opt/homebrew/bin/fish' })).toBe('fish');
    expect(shellName({})).toBe('zsh');
    const home = tree(['Library/Fonts/Geist-Regular.ttf']);
    expect(fontDirs(home)[0]).toBe(join(home, 'Library', 'Fonts'));
    expect(geistAvailable(fontDirs(home).slice(0, 1))).toBe(true);
    const bare = tree(['Library/Fonts/Menlo.ttc']);
    expect(geistAvailable([join(bare, 'Library', 'Fonts')])).toBe(false);
    const app = tree(['ply.app/Contents/Resources/Fonts/GeistMono-Regular.ttf']);
    const bundled = fontDirs(bare, join(app, 'ply.app', 'Contents', 'MacOS'));
    expect(bundled[2]).toBe(join(app, 'ply.app', 'Contents', 'Resources', 'Fonts'));
    expect(geistAvailable(bundled)).toBe(true);
  });
});
