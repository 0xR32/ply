import { describe, expect, test } from 'bun:test';
import { admissionProblems, readAllowList, renderAllowList } from './check-deps';

const passing = {
  repo: 'owner/repo',
  stars: 1000,
  last_commit: '2026-06-01',
  contributors_5plus: 3,
  checked: '2026-09-25',
};

describe('check-deps', () => {
  test('an entry at the INV-16 thresholds passes', () => {
    expect(admissionProblems('cargo:x', passing)).toEqual([]);
  });

  test('each threshold fails on its own', () => {
    expect(admissionProblems('cargo:x', { ...passing, stars: 999 })).toHaveLength(1);
    expect(admissionProblems('cargo:x', { ...passing, contributors_5plus: 1 })).toHaveLength(1);
    expect(admissionProblems('cargo:x', { ...passing, last_commit: '2026-01-01' })).toHaveLength(1);
    expect(admissionProblems('cargo:x', { ...passing, repo: undefined })).toHaveLength(1);
  });

  test('an exception skips the thresholds but not the recorded fields', () => {
    expect(admissionProblems('npm:x', { ...passing, stars: 1, exception: 'D4' })).toEqual([]);
    expect(admissionProblems('npm:x', { exception: 'D4' })).toHaveLength(5);
  });

  test('the committed allow list round-trips through the renderer', async () => {
    const root = `${import.meta.dir}/..`;
    const text = await Bun.file(`${root}/deps.allow.toml`).text();
    expect(renderAllowList(readAllowList(root))).toBe(text);
  });
});
