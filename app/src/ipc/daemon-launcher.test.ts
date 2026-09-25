import { afterEach, describe, expect, test } from 'bun:test';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createDaemonStarter, planLaunch } from './daemon-launcher';
import { geistAvailable, shellName } from './os';

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
  test('a plyd beside the app binary means the bundle: plyd installs its LaunchAgent', () => {
    const root = tree(['Ply.app/Contents/MacOS/ply', 'Ply.app/Contents/MacOS/plyd']);
    const plan = planLaunch({ env: {}, execPath: join(root, 'Ply.app/Contents/MacOS/ply') });
    expect(plan).toEqual({ kind: 'launch-agent', plyd: join(root, 'Ply.app/Contents/MacOS/plyd') });
  });

  test('a development run installs the agent for the cargo-built plyd (Ruling R38)', () => {
    const repo = tree(['target/debug/plyd']);
    const empty = tree([]);
    const plan = planLaunch({ env: {}, execPath: join(empty, 'bun'), repoRoot: repo });
    expect(plan).toEqual({ kind: 'launch-agent', plyd: join(repo, 'target/debug/plyd') });
  });

  test('PLY_HOME always spawns a build-dir plyd, never the LaunchAgent, and says when none is built', () => {
    const bundle = tree(['MacOS/ply', 'MacOS/plyd']);
    const repo = tree(['target/debug/plyd']);
    const exec = join(bundle, 'MacOS/ply');
    expect(planLaunch({ env: { PLY_HOME: '/x' }, execPath: exec, repoRoot: repo })).toEqual({
      kind: 'spawn',
      plyd: join(repo, 'target/debug/plyd'),
    });
    const empty = tree([]);
    expect(planLaunch({ env: { PLY_HOME: '/x' }, execPath: exec, repoRoot: empty })).toEqual({
      kind: 'unavailable',
      reason: 'plyd is not built (cargo build -p ply-daemon -p ply-hook)',
    });
    expect(planLaunch({ env: {}, execPath: join(empty, 'bun'), repoRoot: empty }).kind).toBe(
      'unavailable',
    );
  });
});

describe('createDaemonStarter', () => {
  test('rejects with the reason when plyd cannot be found', async () => {
    const empty = tree([]);
    const start = createDaemonStarter({ env: {}, execPath: join(empty, 'bun'), repoRoot: empty });
    await expect(start()).rejects.toThrow('plyd is not built');
  });

  test('runs `plyd install-agent` instead of writing the plist itself', async () => {
    const root = tree(['Ply.app/Contents/MacOS/ply']);
    const plyd = join(root, 'Ply.app/Contents/MacOS/plyd');
    fakePlyd(plyd, 0);
    const start = createDaemonStarter({
      env: {},
      execPath: join(root, 'Ply.app/Contents/MacOS/ply'),
    });
    await start();
    expect(readFileSync(join(root, 'Ply.app/Contents/MacOS/args.txt'), 'utf8')).toBe(
      'install-agent\n',
    );
  });

  test('a failing install-agent rejects with its status and stderr', async () => {
    const root = tree(['Ply.app/Contents/MacOS/ply']);
    fakePlyd(join(root, 'Ply.app/Contents/MacOS/plyd'), 3);
    const start = createDaemonStarter({
      env: {},
      execPath: join(root, 'Ply.app/Contents/MacOS/ply'),
    });
    await expect(start()).rejects.toThrow('plyd install-agent failed (3): boom');
  });
});

describe('os facts', () => {
  test('the shell name comes from $SHELL; Geist counts as available when the bundle carries it', () => {
    expect(shellName({ SHELL: '/opt/homebrew/bin/fish' })).toBe('fish');
    expect(shellName({})).toBe('zsh');
    const bundle = tree([
      'Ply.app/Contents/MacOS/ply',
      'Ply.app/Contents/Resources/fonts/Geist-Regular.ttf',
    ]);
    expect(geistAvailable(join(bundle, 'Ply.app/Contents/MacOS/ply'))).toBe(true);
  });
});
