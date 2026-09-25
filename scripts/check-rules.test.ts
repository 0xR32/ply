import { afterEach, describe, expect, test } from 'bun:test';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import {
  checkAppLayers,
  checkInv1NoNetwork,
  checkInv2NoPtyBytes,
  checkInv4KeyHandlers,
  checkInv5Palette,
  checkInv7Worktrees,
  checkInv11PersonalData,
  ghosttyPinProblems,
} from './check-rules';
import { listRepoFiles } from './lib/repo';

let dirs: string[] = [];

function repo(files: Record<string, string>): { root: string; files: string[] } {
  const root = mkdtempSync(join(tmpdir(), 'ply-rules-'));
  dirs.push(root);
  for (const [path, body] of Object.entries(files)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), body);
  }
  return { root, files: listRepoFiles(root) };
}

afterEach(() => {
  for (const d of dirs) rmSync(d, { recursive: true, force: true });
  dirs = [];
});

describe('check-rules', () => {
  test('INV-5 rejects a colour literal in a feature and allows it in tokens.ts', () => {
    const { root, files } = repo({
      'app/src/features/x.tsx': "export const c = '#fff';\n",
      'app/src/theme/tokens.ts': "export const c = '#FFFFFF';\n",
      'app/src/features/panes/fixtures/screen.ts': "export const c = '#123456';\n",
    });
    const v = checkInv5Palette(root, files).violations;
    expect(v.map((x) => `${x.file}:${x.line}`)).toEqual(['app/src/features/x.tsx:1']);
  });

  test('INV-4 allows onKeyDown only in the listed files', () => {
    const { root, files } = repo({
      'app/src/keymap/dispatcher.ts': 'export const h = { onKeyDown: () => {} };\n',
      'app/src/features/tabs/tab-bar.tsx': 'const p = { onKeyDown: () => {} };\n',
    });
    const v = checkInv4KeyHandlers(root, files).violations;
    expect(v.map((x) => x.file)).toEqual(['app/src/features/tabs/tab-bar.tsx']);
  });

  test('INV-2 rejects byte fields in C1 types but not byte-typed function parameters', () => {
    const { root, files } = repo({
      'crates/proto/src/control.rs':
        'pub struct A {\n    pub ids: Vec<u64>,\n}\npub fn decode(line: &[u8]) {}\npub enum B {\n    Raw(Vec<u8>),\n}\n',
      'crates/proto/src/pane.rs': 'pub struct P {\n    pub bytes: [u8; 4],\n}\n',
    });
    const v = checkInv2NoPtyBytes(root, files).violations.map((x) => `${x.file}:${x.line}`);
    expect(v).toEqual(['crates/proto/src/control.rs:6', 'crates/proto/src/pane.rs:2']);
  });

  test('INV-1 rejects network APIs in app/', () => {
    const { root, files } = repo({
      'app/src/ipc/a.ts': "await fetch('x');\nnew WebSocket('y');\n",
      'app/src/ipc/b.ts': 'export function fetchHistory() {}\n',
    });
    expect(checkInv1NoNetwork(root, files).violations.map((x) => x.line)).toEqual([1, 2]);
  });

  test('INV-7 rejects git worktree calls and worktree columns other than worktree_seen', () => {
    const { root, files } = repo({
      'crates/daemon/src/db/migrations/0001_init.sql':
        'CREATE TABLE panes(id INTEGER, worktree_seen TEXT, worktree_path TEXT);\n',
      'crates/daemon/src/x.rs': 'fn f() { cmd("git worktree add"); }\n',
    });
    const v = checkInv7Worktrees(root, files).violations.map((x) => x.message);
    expect(v).toHaveLength(2);
    expect(v.some((m) => m.includes('worktree_path'))).toBe(true);
  });

  test('INV-11 rejects a real home path and the committer identity, not the example placeholder', () => {
    const real = ['', 'Users', 'someone', 'project'].join('/');
    const { root, files } = repo({
      'docs/a.md': `ok /Users/example/project\nbad ${real}\n`,
      'crates/x/tests/fixtures/b.json': '{"author":"Jane Example-Committer"}\n',
    });
    const v = checkInv11PersonalData(root, files, ['Jane Example-Committer']).violations;
    expect(v.map((x) => `${x.file}:${x.line}`)).toEqual([
      'crates/x/tests/fixtures/b.json:1',
      'docs/a.md:2',
    ]);
  });

  test('INV-17 accepts the committed ghostty pin in ghostty-sys/build.rs', () => {
    const buildRs = readFileSync(join(import.meta.dir, '../crates/ghostty-sys/build.rs'), 'utf8');
    expect(ghosttyPinProblems(buildRs)).toEqual([]);
  });

  test('INV-17 rejects a short commit, a URL for another commit and a hash that is not SHA-256', () => {
    const commit = 'a'.repeat(40);
    const pin = (c: string, url: string, sha: string) =>
      `const GHOSTTY_COMMIT: &str = "${c}";\nconst GHOSTTY_ARCHIVE_URL: &str =\n    "${url}";\nconst GHOSTTY_ARCHIVE_SHA256: &str = "${sha}";\n`;
    const url = `https://codeload.github.com/ghostty-org/ghostty/tar.gz/${commit}`;
    expect(ghosttyPinProblems(pin(commit, url, 'f'.repeat(64)))).toEqual([]);
    expect(ghosttyPinProblems(pin('aaaaaaa', url, 'f'.repeat(64)))).toEqual([
      'GHOSTTY_COMMIT is not a 40-hex commit',
    ]);
    expect(
      ghosttyPinProblems(pin(commit, url.replace(commit, 'b'.repeat(40)), 'f'.repeat(64))),
    ).toEqual(['GHOSTTY_ARCHIVE_URL does not name GHOSTTY_COMMIT']);
    expect(ghosttyPinProblems(pin(commit, url, 'f'.repeat(40)))).toEqual([
      'GHOSTTY_ARCHIVE_SHA256 is not a 64-hex SHA-256',
    ]);
    expect(ghosttyPinProblems('')).toHaveLength(3);
  });

  test('layer rules: features never import another feature or ipc; ui imports only theme', () => {
    const { root, files } = repo({
      'app/src/features/tabs/tab-bar.tsx':
        "import { a } from '../panes/pane-grid';\nimport { b } from '../../ipc/control-client';\nimport { t } from '../../theme/tokens';\n",
      'app/src/ui/text.tsx': "import { s } from '../state/store';\n",
      'app/src/state/reducer.ts': "import { c } from '../ipc/control-client';\n",
      'app/src/state/effects.ts': "import { c } from '../ipc/control-client';\n",
      'app/src/state/actions.ts': "import type { Pane } from '../ipc/proto.gen';\n",
    });
    const v = checkAppLayers(root, files).violations.map((x) => `${x.file}:${x.line}`);
    expect(v.sort()).toEqual([
      'app/src/features/tabs/tab-bar.tsx:1',
      'app/src/features/tabs/tab-bar.tsx:2',
      'app/src/state/reducer.ts:1',
      'app/src/ui/text.tsx:1',
    ]);
  });
});
