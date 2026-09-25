import { describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { version } from '../../package.json';
import { readBuildId } from './os';

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
