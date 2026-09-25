import { afterEach, describe, expect, test } from 'bun:test';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createDaemonStarter, launchAgentPlist, planLaunch } from './daemon-launcher';
import { geistAvailable, shellName } from './os';
import { LAUNCH_AGENT_LABEL } from './paths';

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

describe('planLaunch', () => {
  test('a plyd beside the app binary means the bundle: start it through the LaunchAgent', () => {
    const root = tree(['Ply.app/Contents/MacOS/ply', 'Ply.app/Contents/MacOS/plyd']);
    const plan = planLaunch({ env: {}, execPath: join(root, 'Ply.app/Contents/MacOS/ply') });
    expect(plan).toMatchObject({
      kind: 'launch-agent',
      plyd: join(root, 'Ply.app/Contents/MacOS/plyd'),
    });
  });

  test('PLY_HOME or a development run spawns the build-dir plyd, and says so when none is built', () => {
    const bundle = tree(['MacOS/ply', 'MacOS/plyd']);
    const repo = tree(['target/debug/plyd']);
    const exec = join(bundle, 'MacOS/ply');
    expect(planLaunch({ env: { PLY_HOME: '/x' }, execPath: exec, repoRoot: repo })).toEqual({
      kind: 'spawn',
      plyd: join(repo, 'target/debug/plyd'),
    });
    const empty = tree([]);
    const plan = planLaunch({ env: {}, execPath: join(empty, 'bun'), repoRoot: empty });
    expect(plan).toEqual({
      kind: 'unavailable',
      reason: 'plyd is not built (cargo build -p ply-daemon)',
    });
  });

  test('the starter rejects with that reason', async () => {
    const empty = tree([]);
    const start = createDaemonStarter({ env: {}, execPath: join(empty, 'bun'), repoRoot: empty });
    await expect(start()).rejects.toThrow('plyd is not built');
  });

  test('the LaunchAgent restarts plyd only after a crash and never throttles it (spec 11.3)', () => {
    const plist = launchAgentPlist('/Applications/Ply.app/Contents/MacOS/plyd');
    expect(plist).toContain(`<string>${LAUNCH_AGENT_LABEL}</string>`);
    expect(plist).toContain('<key>SuccessfulExit</key>\n    <false/>');
    expect(plist).toContain('<key>ProcessType</key>\n  <string>Standard</string>');
    expect(plist).toContain('<key>RunAtLoad</key>\n  <false/>');
    expect(launchAgentPlist('/a&b/plyd')).toContain('<string>/a&amp;b/plyd</string>');
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
