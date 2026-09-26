import { describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { infoPlist, minimumSystemVersion } from './dmg';

describe('infoPlist', () => {
  test('is a valid property list naming the executable, the bundle id and the font folder', () => {
    const xml = infoPlist({
      version: '0.1.0',
      buildId: '0.1.0+0123456789ab',
      minimumSystemVersion: '11.0',
    });
    const dir = mkdtempSync(join(tmpdir(), 'ply-plist-'));
    try {
      const path = join(dir, 'Info.plist');
      writeFileSync(path, xml);
      const lint = Bun.spawnSync({
        cmd: ['plutil', '-lint', path],
        stdout: 'pipe',
        stderr: 'pipe',
      });
      expect(lint.exitCode).toBe(0);
      const json = Bun.spawnSync({
        cmd: ['plutil', '-convert', 'json', '-o', '-', path],
        stdout: 'pipe',
      });
      expect(JSON.parse(json.stdout.toString())).toMatchObject({
        CFBundleExecutable: 'ply',
        CFBundleIdentifier: 'dev.ply.app',
        CFBundleVersion: '0.1.0',
        LSMinimumSystemVersion: '11.0',
        NSHighResolutionCapable: true,
        ATSApplicationFontsPath: 'Fonts',
        CFBundleIconFile: 'ply',
        PlyBuildId: '0.1.0+0123456789ab',
      });
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe('minimumSystemVersion', () => {
  const otool = (minos: string) =>
    `Load command 9\n      cmd LC_BUILD_VERSION\n  cmdsize 32\n platform 1\n    minos ${minos}\n      sdk 15.0\n`;

  test('is the highest minos of the binaries, compared as numbers', () => {
    expect(minimumSystemVersion([otool('11.0'), otool('13.3'), otool('13.10')])).toBe('13.10');
    expect(minimumSystemVersion([otool('11.0')])).toBe('11.0');
  });

  test('refuses binaries without a build version', () => {
    expect(() => minimumSystemVersion(['no load commands'])).toThrow();
  });
});
