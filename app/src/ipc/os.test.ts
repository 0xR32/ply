import { describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { version } from '../../package.json';
import { parseDefaultsNumber, readBuildId, readKeyRepeat } from './os';

describe('the build id', () => {
  test('is the version and the checkout commit abbreviated to 12, as plyd writes it', async () => {
    const id = await readBuildId();
    expect(id?.startsWith(`${version}+`)).toBe(true);
    expect(id?.slice(version.length + 1)).toMatch(/^[0-9a-f]{12}$/);
  });

  test('is unknown outside a git checkout', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'ply-nogit-'));
    try {
      expect(await readBuildId(dir)).toBeNull();
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe('the key-repeat defaults', () => {
  test('are positive numbers of 15 ms units, and an unset key is none', () => {
    expect(parseDefaultsNumber(0, '2\n')).toBe(2);
    expect(parseDefaultsNumber(0, '1.5\n')).toBe(1.5);
    expect(parseDefaultsNumber(1, '')).toBeNull();
    expect(parseDefaultsNumber(0, 'yes\n')).toBeNull();
    expect(parseDefaultsNumber(0, '0\n')).toBeNull();
  });

  test('read from this machine are positive milliseconds, or left out when never set', async () => {
    const timing = await readKeyRepeat();
    for (const ms of Object.values(timing)) expect(ms).toBeGreaterThan(0);
  });
});
