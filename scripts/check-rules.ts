import { existsSync } from 'node:fs';
import { dirname, join, normalize, relative } from 'node:path';
import {
  type CheckResult,
  formatViolation,
  isBinary,
  listRepoFiles,
  matchLines,
  readText,
  run,
  type Violation,
} from './lib/repo';

const ROOT = normalize(join(import.meta.dir, '..'));
const GPUIX_VERSION = '0.10.0';

const PLY_CRATES = ['ply-proto', 'ghostty-sys', 'ply-term', 'ply-agents', 'ply-daemon', 'ply-hook'];
const GPUI_CRATES = ['gpui', 'gpuix-native'];
const IO_CRATES = ['tokio', 'async-std', 'mio', 'rusqlite', 'notify', 'rustix', 'libc', 'nix'];
const HTTP_CRATES = [
  'reqwest',
  'hyper',
  'ureq',
  'isahc',
  'surf',
  'attohttpc',
  'curl',
  'http-client',
];

const TS_EXT = /\.(ts|tsx|js|jsx|mjs|cjs)$/;

function ok(): CheckResult {
  return { violations: [], notices: [] };
}

function appSources(files: string[]): string[] {
  return files.filter((f) => f.startsWith('app/') && TS_EXT.test(f));
}

/** INV-1: no network API in app/ (fetch, WebSocket, XMLHttpRequest, http imports, remote <img>). */
export function checkInv1NoNetwork(root: string, files: string[]): CheckResult {
  const res = ok();
  const patterns: [RegExp, string][] = [
    [/\bfetch\s*\(|\.fetch\b/, 'fetch'],
    [/\bWebSocket\b/, 'WebSocket'],
    [/\bXMLHttpRequest\b/, 'XMLHttpRequest'],
    [/\bEventSource\b/, 'EventSource'],
    [/from\s+['"](node:)?(http|https|http2|dgram|undici)['"]/, 'a network module import'],
    [/<img\b[^>]*\bsrc\s*=\s*\{?\s*['"`]https?:/, 'a remote <img src>'],
  ];
  for (const file of appSources(files)) {
    const text = readText(root, file) ?? '';
    for (const [re, what] of patterns) {
      for (const hit of matchLines(text, re)) {
        res.violations.push({
          check: 'INV-1',
          file,
          line: hit.line,
          message: `${what} in app/ (ply makes no network requests)`,
        });
      }
    }
  }
  return res;
}

/** The `{ … }` bodies of every `struct` and `enum` in a Rust source, with their byte offsets. */
export function typeBodies(text: string): { offset: number; text: string }[] {
  const out: { offset: number; text: string }[] = [];
  for (const m of text.matchAll(/\b(?:struct|enum)\s+\w+[^{;]*\{/g)) {
    const open = m.index + m[0].length - 1;
    let depth = 0;
    for (let i = open; i < text.length; i++) {
      if (text[i] === '{') depth++;
      else if (text[i] === '}' && --depth === 0) {
        out.push({ offset: open, text: text.slice(open, i + 1) });
        break;
      }
    }
  }
  return out;
}

/** INV-2: C1 wire types carry no byte fields, and the app never opens a pty. */
export function checkInv2NoPtyBytes(root: string, files: string[]): CheckResult {
  const res = ok();
  const c1 = ['crates/proto/src/control.rs', 'crates/proto/src/pane.rs'];
  const present = c1.filter((f) => existsSync(join(root, f)));
  if (present.length === 0) {
    res.notices.push(
      'INV-2: crates/proto/src/{control,pane}.rs not written yet (WP2); C1 part skipped',
    );
  }
  const bytes = /Vec<u8>|&\[u8\]|\[u8;|\bBytes\b|\bByteBuf\b|serde_bytes/g;
  for (const file of present) {
    const text = readText(root, file) ?? '';
    for (const body of typeBodies(text)) {
      for (const m of body.text.matchAll(bytes)) {
        res.violations.push({
          check: 'INV-2',
          file,
          line: text.slice(0, body.offset + m.index).split('\n').length,
          message: `byte field \`${m[0]}\` in a C1 type (pty output never enters JavaScript)`,
        });
      }
    }
  }
  const pty = /\bBun\.Terminal\b|\bnode-pty\b|\bopenpty\b|\bforkpty\b|\bposix_openpt\b|\/dev\/ptmx/;
  for (const file of appSources(files)) {
    for (const hit of matchLines(readText(root, file) ?? '', pty)) {
      res.violations.push({
        check: 'INV-2',
        file,
        line: hit.line,
        message: `pty API \`${hit.match}\` in app/ (only plyd owns ptys)`,
      });
    }
  }
  return res;
}

interface CargoDep {
  name: string;
  source: string | null;
  kind: string | null;
  optional: boolean;
}
interface CargoPackage {
  id: string;
  name: string;
  dependencies: CargoDep[];
  features: Record<string, string[]>;
  manifest_path: string;
}
interface CargoMetadata {
  packages: CargoPackage[];
  workspace_members: string[];
  resolve: { nodes: { id: string; deps: { pkg: string }[] }[] } | null;
}

let metadataCache: CargoMetadata | null = null;

function cargoMetadata(root: string): CargoMetadata | { error: string } {
  if (metadataCache) return metadataCache;
  const r = run(['cargo', 'metadata', '--format-version', '1'], root);
  if (r.code !== 0)
    return { error: r.stderr.trim().split('\n').slice(-1)[0] ?? 'cargo metadata failed' };
  metadataCache = JSON.parse(r.stdout) as CargoMetadata;
  return metadataCache;
}

/** INV-3: no ply crate depends on gpui or gpuix-native, directly or transitively. */
export function checkInv3NoGpui(root: string): CheckResult {
  const res = ok();
  const meta = cargoMetadata(root);
  if ('error' in meta) {
    res.violations.push({ check: 'INV-3', message: `cannot read cargo metadata: ${meta.error}` });
    return res;
  }
  const byId = new Map(meta.packages.map((p) => [p.id, p]));
  const graph = new Map((meta.resolve?.nodes ?? []).map((n) => [n.id, n.deps.map((d) => d.pkg)]));
  for (const member of meta.workspace_members) {
    const start = byId.get(member);
    if (!start) continue;
    const seen = new Map<string, string | null>([[member, null]]);
    const queue = [member];
    while (queue.length > 0) {
      const id = queue.shift() as string;
      const pkg = byId.get(id);
      if (pkg && GPUI_CRATES.includes(pkg.name)) {
        const path: string[] = [];
        for (let at: string | null = id; at; at = seen.get(at) ?? null) {
          path.unshift(byId.get(at)?.name ?? at);
        }
        res.violations.push({
          check: 'INV-3',
          file: relative(root, start.manifest_path),
          message: `${start.name} depends on ${pkg.name} (${path.join(' -> ')})`,
        });
        break;
      }
      for (const next of graph.get(id) ?? []) {
        if (!seen.has(next)) {
          seen.set(next, id);
          queue.push(next);
        }
      }
    }
  }
  return res;
}

const KEY_HANDLER_FILES = new Set([
  'app/src/keymap/dispatcher.ts',
  'app/src/features/panes/terminal-view.tsx',
  'app/src/features/palette/palette.tsx',
  'app/src/features/new-pane/new-pane.tsx',
  'app/src/features/settings/settings.tsx',
  'app/src/features/panes/close-confirm.tsx',
  'app/src/features/palette/quit-confirm.tsx',
]);

/** INV-4: `onKeyDown` only in the keymap dispatcher, the overlay forms and the terminal view. */
export function checkInv4KeyHandlers(root: string, files: string[]): CheckResult {
  const res = ok();
  for (const file of appSources(files)) {
    if (KEY_HANDLER_FILES.has(file)) continue;
    for (const hit of matchLines(readText(root, file) ?? '', /\bonKeyDown\b/)) {
      res.violations.push({
        check: 'INV-4',
        file,
        line: hit.line,
        message: 'onKeyDown outside keymap/dispatcher.ts, the overlay forms and terminal-view.tsx',
      });
    }
  }
  return res;
}

function isFixture(file: string): boolean {
  return /(^|\/)fixtures\//.test(file) || /\.fixture\.(ts|tsx)$/.test(file);
}

/** INV-5: colour literals (hex, rgb(), hsl(), oklch()) only in app/src/theme/tokens.ts and fixtures. */
export function checkInv5Palette(root: string, files: string[]): CheckResult {
  const res = ok();
  const colour =
    /#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})\b|\b(?:rgba?|hsla?|oklch|oklab)\(/;
  for (const file of appSources(files)) {
    if (file === 'app/src/theme/tokens.ts' || isFixture(file)) continue;
    for (const hit of matchLines(readText(root, file) ?? '', colour)) {
      res.violations.push({
        check: 'INV-5',
        file,
        line: hit.line,
        message: `colour literal \`${hit.match}\` outside app/src/theme/tokens.ts`,
      });
    }
  }
  return res;
}

const PLY_CODE = /^(crates|app|scripts)\/.*\.(rs|ts|tsx|sql|sh)$/;

/** INV-7 (Ruling R5): no `git worktree` call and no worktree table or column except `worktree_seen`. */
export function checkInv7Worktrees(root: string, files: string[]): CheckResult {
  const res = ok();
  const self = new Set(['scripts/check-rules.ts', 'scripts/check-rules.test.ts']);
  const gitWorktree = new RegExp(
    [
      'git',
      '\\s+',
      'worktree',
      '|',
      '["\']',
      'worktree',
      '["\']\\s*,\\s*["\'](add|remove|prune|move|repair|lock)',
    ].join(''),
  );
  const ident = /\b\w*worktree\w*\b/gi;
  for (const file of files) {
    if (!PLY_CODE.test(file) || self.has(file) || file.includes('/fixtures/')) continue;
    const text = readText(root, file) ?? '';
    for (const hit of matchLines(text, gitWorktree)) {
      res.violations.push({
        check: 'INV-7',
        file,
        line: hit.line,
        message: 'git worktree invocation (worktrees belong to the CLI)',
      });
    }
    const sqlChunks: { offset: number; body: string }[] = [];
    if (file.endsWith('.sql')) {
      sqlChunks.push({ offset: 0, body: text.replace(/--[^\n]*/g, '') });
    } else if (file.endsWith('.rs')) {
      for (const m of text.matchAll(/\b(CREATE|ALTER)\s+TABLE\b[\s\S]*?(;|"#|"\s*[,)])/gi)) {
        sqlChunks.push({ offset: m.index, body: m[0] });
      }
    }
    for (const chunk of sqlChunks) {
      for (const m of chunk.body.matchAll(ident)) {
        if (m[0].toLowerCase() === 'worktree_seen') continue;
        const line = text.slice(0, chunk.offset + m.index).split('\n').length;
        res.violations.push({
          check: 'INV-7',
          file,
          line,
          message: `worktree table or column \`${m[0]}\` (only worktree_seen is allowed)`,
        });
      }
    }
  }
  return res;
}

function gitIdentity(root: string): string[] {
  const out: string[] = [];
  for (const key of ['user.name', 'user.email']) {
    const v = run(['git', 'config', '--get', key], root).stdout.trim();
    if (v.length >= 4) out.push(v);
  }
  return out;
}

/** INV-11: no home path of a real user and no committer identity in any repo file. */
export function checkInv11PersonalData(
  root: string,
  files: string[],
  identities = gitIdentity(root),
): CheckResult {
  const res = ok();
  const home = /\/(?:Users|home)\/(?!example\/|Shared\/)[A-Za-z0-9._-]+\//;
  const skipExt = /\.(ttf|otf|png|jpe?g|gif|icns|ico|lock)$/;
  const lowered = identities.map((s) => s.toLowerCase());
  for (const file of files) {
    if (skipExt.test(file) || isBinary(root, file)) continue;
    const text = readText(root, file) ?? '';
    for (const hit of matchLines(text, home)) {
      res.violations.push({
        check: 'INV-11',
        file,
        line: hit.line,
        message: `home path \`${hit.match}\` (use /Users/example/)`,
      });
    }
    const lower = text.toLowerCase();
    for (const id of lowered) {
      const at = lower.indexOf(id);
      if (at >= 0) {
        res.violations.push({
          check: 'INV-11',
          file,
          line: text.slice(0, at).split('\n').length,
          message: 'the committer identity appears in the file (use an `example` placeholder)',
        });
      }
    }
  }
  return res;
}

function stripJsonTrailingCommas(text: string): string {
  return text.replace(/,(\s*[}\]])/g, '$1');
}

/** INV-13 (ADR-0011): @gpuix/react and @gpuix/native pinned exactly at 0.10.0 from npm, unpatched. */
export function checkInv13Gpuix(root: string): CheckResult {
  const res = ok();
  const manifests = ['package.json', 'app/package.json'];
  for (const file of manifests) {
    const text = readText(root, file);
    if (text === null) continue;
    const pkg = JSON.parse(text) as Record<string, unknown>;
    for (const key of ['patchedDependencies', 'overrides', 'resolutions']) {
      const section = pkg[key] as Record<string, unknown> | undefined;
      const hits = Object.keys(section ?? {}).filter((k) => k.startsWith('@gpuix/'));
      for (const hit of hits) {
        res.violations.push({ check: 'INV-13', file, message: `${key} entry for ${hit}` });
      }
    }
  }
  const app = JSON.parse(readText(root, 'app/package.json') ?? '{}') as {
    dependencies?: Record<string, string>;
  };
  for (const name of ['@gpuix/react', '@gpuix/native']) {
    const spec = app.dependencies?.[name];
    if (spec !== GPUIX_VERSION) {
      res.violations.push({
        check: 'INV-13',
        file: 'app/package.json',
        message: `${name} must be pinned exactly at "${GPUIX_VERSION}" (found ${JSON.stringify(spec ?? null)})`,
      });
    }
  }
  const lockText = readText(root, 'bun.lock');
  if (lockText === null) {
    res.violations.push({ check: 'INV-13', file: 'bun.lock', message: 'bun.lock is missing' });
    return res;
  }
  const lock = JSON.parse(stripJsonTrailingCommas(lockText)) as {
    packages?: Record<string, unknown[]>;
    patchedDependencies?: Record<string, string>;
    overrides?: Record<string, string>;
  };
  for (const key of ['patchedDependencies', 'overrides'] as const) {
    for (const name of Object.keys(lock[key] ?? {}).filter((k) => k.startsWith('@gpuix/'))) {
      res.violations.push({
        check: 'INV-13',
        file: 'bun.lock',
        message: `${key} entry for ${name}`,
      });
    }
  }
  for (const [name, entry] of Object.entries(lock.packages ?? {})) {
    if (!name.startsWith('@gpuix/')) continue;
    const resolved = String(entry[0] ?? '');
    const registry = entry[1];
    const want = `${name}@${GPUIX_VERSION}`;
    if (resolved !== want || registry !== '') {
      res.violations.push({
        check: 'INV-13',
        file: 'bun.lock',
        message: `${name} resolves to ${JSON.stringify(resolved)}, expected ${want} from the npm registry`,
      });
    }
  }
  for (const name of ['@gpuix/react', '@gpuix/native']) {
    if (!lock.packages?.[name]) {
      res.violations.push({ check: 'INV-13', file: 'bun.lock', message: `${name} is not locked` });
    }
  }
  return res;
}

function cargoTreeInverse(root: string, pkg: string, target: string): string[] | null {
  const r = run(
    ['cargo', 'tree', '-p', pkg, '-i', target, '--prefix', 'depth', '-e', 'normal,build'],
    root,
  );
  if (r.code !== 0) return null;
  return r.stdout
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
}

const GHOSTTY_BUILD_RS = 'crates/ghostty-sys/build.rs';

/** INV-17 problems with the pin in `ghostty-sys/build.rs`: a full commit, an archive URL naming it, a SHA-256. */
export function ghosttyPinProblems(buildRs: string): string[] {
  const value = (name: string) =>
    new RegExp(`\\bconst ${name}: &str =\\s*"([^"]*)";`).exec(buildRs)?.[1] ?? '';
  const commit = value('GHOSTTY_COMMIT');
  const url = value('GHOSTTY_ARCHIVE_URL');
  const sha256 = value('GHOSTTY_ARCHIVE_SHA256');
  const problems: string[] = [];
  if (!/^[0-9a-f]{40}$/.test(commit)) problems.push('GHOSTTY_COMMIT is not a 40-hex commit');
  if (commit === '' || !url.includes(commit))
    problems.push('GHOSTTY_ARCHIVE_URL does not name GHOSTTY_COMMIT');
  if (!/^[0-9a-f]{64}$/.test(sha256))
    problems.push('GHOSTTY_ARCHIVE_SHA256 is not a 64-hex SHA-256');
  return problems;
}

/** INV-17: libghostty-vt is linked only by plyd, built from the ghostty commit build.rs pins, downloads and SHA-256-verifies. */
export function checkInv17Ghostty(root: string): CheckResult {
  const res = ok();
  const meta = cargoMetadata(root);
  if ('error' in meta) {
    res.violations.push({ check: 'INV-17', message: `cannot read cargo metadata: ${meta.error}` });
    return res;
  }
  const members = meta.packages.filter((p) => meta.workspace_members.includes(p.id));
  for (const pkg of members) {
    const dep = pkg.dependencies.find((d) => d.name === 'ghostty-sys' && d.kind !== 'dev');
    if (!dep) continue;
    if (pkg.name !== 'ply-term') {
      res.violations.push({
        check: 'INV-17',
        file: relative(root, pkg.manifest_path),
        message: `${pkg.name} depends on ghostty-sys directly (only ply-term's engine feature may)`,
      });
    } else if (!dep.optional || !(pkg.features.engine ?? []).includes('dep:ghostty-sys')) {
      res.violations.push({
        check: 'INV-17',
        file: relative(root, pkg.manifest_path),
        message: 'ghostty-sys must be an optional dependency enabled only by the `engine` feature',
      });
    }
  }
  const daemonTree = cargoTreeInverse(root, 'ply-daemon', 'ghostty-sys');
  const expected = ['0ghostty-sys', '1ply-term', '2ply-daemon'];
  const got = (daemonTree ?? []).map((l) => l.split(' ')[0] ?? '');
  if (got.join(',') !== expected.join(',')) {
    res.violations.push({
      check: 'INV-17',
      message: `cargo tree -p ply-daemon -i ghostty-sys must be ghostty-sys <- ply-term <- ply-daemon, got [${got.join(', ')}]`,
    });
  }
  for (const pkg of members) {
    if (['ply-daemon', 'ghostty-sys'].includes(pkg.name)) continue;
    const tree = cargoTreeInverse(root, pkg.name, 'ghostty-sys');
    if (tree !== null && tree.length > 0) {
      res.violations.push({
        check: 'INV-17',
        message: `${pkg.name} links ghostty-sys when built alone (${tree.join(' | ')})`,
      });
    }
  }
  const buildRs = readText(root, GHOSTTY_BUILD_RS);
  const problems =
    buildRs === null ? [`${GHOSTTY_BUILD_RS} is missing`] : ghosttyPinProblems(buildRs);
  for (const message of problems) {
    res.violations.push({ check: 'INV-17', file: GHOSTTY_BUILD_RS, message });
  }
  return res;
}

/** One crate's row of CLAUDE.md's "Dependency layers" table: the only crates it may use, and the ones it must not. */
export interface LayerRule {
  /** The third-party crates it may depend on; any other normal dependency is a violation. */
  onlyAllowed: string[];
  /** The ply crates it may depend on. */
  allowedPly: string[];
  /** Crates it must not depend on in any section, dev and build included. */
  forbidden: string[];
}

/** CLAUDE.md's "Dependency layers" table for the crates, row for row. */
export const LAYERS: Record<string, LayerRule> = {
  'ply-proto': {
    onlyAllowed: ['serde', 'serde_json', 'ts-rs', 'thiserror'],
    allowedPly: [],
    forbidden: [],
  },
  'ghostty-sys': { onlyAllowed: [], allowedPly: [], forbidden: [] },
  'ply-term': {
    onlyAllowed: ['thiserror', 'tracing'],
    allowedPly: ['ply-proto', 'ghostty-sys'],
    forbidden: [...GPUI_CRATES, ...IO_CRATES, ...HTTP_CRATES],
  },
  'ply-agents': {
    onlyAllowed: ['serde', 'serde_json', 'thiserror'],
    allowedPly: ['ply-proto'],
    forbidden: [...GPUI_CRATES, 'tokio'],
  },
  'ply-daemon': {
    onlyAllowed: [
      'anyhow',
      'notify',
      'rusqlite',
      'rustix',
      'serde',
      'serde_json',
      'thiserror',
      'tokio',
      'toml',
      'tracing',
      'tracing-appender',
      'tracing-subscriber',
    ],
    allowedPly: ['ply-proto', 'ply-term', 'ply-agents'],
    forbidden: [...GPUI_CRATES],
  },
  'ply-hook': { onlyAllowed: ['serde_json'], allowedPly: [], forbidden: ['tokio'] },
};

/** A workspace crate as `cargo metadata` describes it, reduced to what the layer check reads. */
export interface LayerPackage {
  name: string;
  manifest_path: string;
  dependencies: Pick<CargoDep, 'name' | 'kind'>[];
}

/** The layer violations of one crate: a ply crate or third-party crate outside its row, or a forbidden one; dev- and build-dependencies may use crates outside the row. */
export function crateLayerViolations(
  root: string,
  pkg: LayerPackage,
  layers: Record<string, LayerRule> = LAYERS,
): Violation[] {
  const file = relative(root, pkg.manifest_path);
  const rule = layers[pkg.name];
  if (!rule) {
    return [{ check: 'LAYER', file, message: `${pkg.name} has no row in the layer table` }];
  }
  const out: Violation[] = [];
  for (const dep of pkg.dependencies) {
    const isPly = PLY_CRATES.includes(dep.name);
    const tooling = dep.kind === 'dev' || dep.kind === 'build';
    const where =
      dep.kind === 'dev'
        ? 'dev-dependency'
        : dep.kind === 'build'
          ? 'build-dependency'
          : 'dependency';
    if (rule.forbidden.includes(dep.name)) {
      out.push({
        check: 'LAYER',
        file,
        message: `${pkg.name} must not have ${dep.name} as a ${where}`,
      });
    } else if (tooling) {
    } else if (isPly && !rule.allowedPly.includes(dep.name)) {
      out.push({ check: 'LAYER', file, message: `${pkg.name} must not depend on ${dep.name}` });
    } else if (!isPly && !rule.onlyAllowed.includes(dep.name)) {
      out.push({
        check: 'LAYER',
        file,
        message: `${pkg.name} may depend only on ${rule.onlyAllowed.join(', ') || 'nothing'}; found ${dep.name}`,
      });
    }
  }
  return out;
}

/** Spec 8.2 for crates: each crate's direct dependencies against its row of the layer table (`LAYERS`). */
export function checkCrateLayers(root: string): CheckResult {
  const res = ok();
  const meta = cargoMetadata(root);
  if ('error' in meta) {
    res.violations.push({ check: 'LAYER', message: `cannot read cargo metadata: ${meta.error}` });
    return res;
  }
  for (const pkg of meta.packages.filter((p) => meta.workspace_members.includes(p.id))) {
    res.violations.push(...crateLayerViolations(root, pkg));
  }
  return res;
}

const IMPORT_RE =
  /(?:import|export)\s[^'"]*?from\s*['"]([^'"]+)['"]|import\s*\(\s*['"]([^'"]+)['"]\s*\)|import\s+['"]([^'"]+)['"]/g;

function appArea(file: string): string | null {
  const m = /^app\/src\/([^/]+)(?:\/([^/]+))?/.exec(file);
  if (!m) return null;
  if (m[1] === 'features' && m[2]) return `features/${m[2]}`;
  return m[1] ?? null;
}

/** Spec 8.2 and 9.2 for app/src: features, ui, ipc, state and terminal import only what their row allows. */
export function checkAppLayers(root: string, files: string[]): CheckResult {
  const res = ok();
  for (const file of appSources(files).filter((f) => f.startsWith('app/src/'))) {
    const from = appArea(file);
    if (!from) continue;
    const text = readText(root, file) ?? '';
    for (const m of text.matchAll(IMPORT_RE)) {
      const spec = m[1] ?? m[2] ?? m[3] ?? '';
      if (!spec.startsWith('.')) continue;
      const target = normalize(join(dirname(file), spec))
        .split('\\')
        .join('/');
      const to = appArea(target);
      if (!to || to === from) continue;
      const line = text.slice(0, m.index).split('\n').length;
      const deny = (why: string) =>
        res.violations.push({
          check: 'LAYER',
          file,
          line,
          message: `${from} imports ${to}: ${why}`,
        });
      if (from.startsWith('features/')) {
        if (to.startsWith('features/')) deny('a feature never imports another feature');
        else if (to === 'ipc') deny('features reach ipc only through state actions');
        else if (!['state', 'ui', 'keymap', 'theme', 'terminal'].includes(to))
          deny('not in the features row of 8.2');
      } else if (from === 'ui') {
        if (to !== 'theme') deny('ui is presentational and imports only theme');
      } else if (from === 'ipc' || from === 'terminal') {
        const actions = /^app\/src\/state\/actions(\.ts)?$/.test(target);
        const protoGen = /^app\/src\/ipc\/proto\.gen(\.ts)?$/.test(target);
        if (to === 'state' && !actions) deny(`${from} imports only state/actions from state`);
        else if (to === 'ipc' && from === 'terminal' && !protoGen)
          deny('terminal imports only proto.gen from ipc');
        else if (!['state', 'ipc', 'theme'].includes(to))
          deny(`${from} imports only proto.gen, theme and state/actions`);
      } else if (from === 'state') {
        const protoGen = /^app\/src\/ipc\/proto\.gen(\.ts)?$/.test(target);
        if (to === 'ipc' && !protoGen && !file.startsWith('app/src/state/effects'))
          deny('only state/effects.ts calls ipc');
        else if (to.startsWith('features/') || to === 'ui')
          deny('state never imports features or ui');
      }
    }
  }
  return res;
}

/** Spec 9.1: every ply crate forbids unsafe except ghostty-sys, ply-term's engine and daemon/pty.rs; anyhow only in main.rs. */
export function checkRustStandards(root: string, files: string[]): CheckResult {
  const res = ok();
  const unsafeAllowed = (f: string) =>
    f.startsWith('crates/ghostty-sys/') ||
    /^crates\/term\/src\/engine(\.rs|\/)/.test(f) ||
    f === 'crates/daemon/src/pty.rs';
  for (const file of files.filter((f) => /^crates\/[^/]+\/src\/(lib|main)\.rs$/.test(f))) {
    if (file.startsWith('crates/ghostty-sys/')) continue;
    const text = readText(root, file) ?? '';
    if (!/#!\[(forbid|deny)\(unsafe_code\)\]/.test(text)) {
      res.violations.push({
        check: 'RUST',
        file,
        message:
          'crate root lacks #![forbid(unsafe_code)] (or #![deny(unsafe_code)] with a documented exception)',
      });
    }
  }
  for (const file of files.filter((f) => /^crates\/.*\.rs$/.test(f))) {
    const text = readText(root, file) ?? '';
    if (!unsafeAllowed(file)) {
      for (const hit of matchLines(
        text,
        /\bunsafe\s*(\{|fn\b|impl\b|extern\b)|allow\(unsafe_code\)/,
      )) {
        res.violations.push({
          check: 'RUST',
          file,
          line: hit.line,
          message: 'unsafe outside the audited modules',
        });
      }
    }
    if (!/\/src\/main\.rs$/.test(file)) {
      for (const hit of matchLines(text, /\banyhow\b/)) {
        res.violations.push({
          check: 'RUST',
          file,
          line: hit.line,
          message: 'anyhow outside a binary main.rs',
        });
      }
    }
  }
  return res;
}

type TomlDeps = Record<string, string | { workspace?: boolean; path?: string; version?: string }>;

/** Spec 2: third-party versions are exact — `=x.y.z` in [workspace.dependencies], inherited by crates; exact npm pins. */
export function checkExactPins(root: string, files: string[]): CheckResult {
  const res = ok();
  const exactCargo = /^=\d+\.\d+\.\d+([-+][\w.]+)?$/;
  const rootToml = Bun.TOML.parse(readText(root, 'Cargo.toml') ?? '') as {
    workspace?: { dependencies?: TomlDeps };
  };
  for (const [name, spec] of Object.entries(rootToml.workspace?.dependencies ?? {})) {
    if (typeof spec === 'object' && spec.path) continue;
    const version = typeof spec === 'string' ? spec : spec.version;
    if (!version || !exactCargo.test(version)) {
      res.violations.push({
        check: 'PIN',
        file: 'Cargo.toml',
        message: `workspace dependency ${name} must be pinned as "=x.y.z" (found ${JSON.stringify(version ?? null)})`,
      });
    }
  }
  for (const file of files.filter((f) => /^crates\/[^/]+\/Cargo\.toml$/.test(f))) {
    const toml = Bun.TOML.parse(readText(root, file) ?? '') as Record<string, TomlDeps | undefined>;
    for (const section of ['dependencies', 'dev-dependencies', 'build-dependencies']) {
      for (const [name, spec] of Object.entries(toml[section] ?? {})) {
        if (typeof spec === 'object' && (spec.workspace || spec.path)) continue;
        res.violations.push({
          check: 'PIN',
          file,
          message: `${name} must come from [workspace.dependencies] (\`${name}.workspace = true\`)`,
        });
      }
    }
  }
  const exactNpm = /^\d+\.\d+\.\d+(-[\w.]+)?$/;
  for (const file of ['package.json', 'app/package.json']) {
    const text = readText(root, file);
    if (text === null) continue;
    const pkg = JSON.parse(text) as Record<string, Record<string, string> | undefined>;
    for (const section of ['dependencies', 'devDependencies', 'optionalDependencies']) {
      for (const [name, spec] of Object.entries(pkg[section] ?? {})) {
        if (spec.startsWith('workspace:') || exactNpm.test(spec)) continue;
        res.violations.push({
          check: 'PIN',
          file,
          message: `${name} must be pinned to an exact version (found ${JSON.stringify(spec)})`,
        });
      }
    }
  }
  return res;
}

/** WP2: app/src/ipc/proto.gen.ts must equal what `bun run gen` produces from ply-proto. */
export function checkGeneratedTypes(root: string): CheckResult {
  const res = ok();
  if (!existsSync(join(root, 'crates/proto/tests/export_bindings.rs'))) {
    res.notices.push(
      'GEN: ply-proto has no ts-rs export yet (WP2); proto.gen.ts freshness skipped',
    );
    return res;
  }
  const r = run(['bun', 'scripts/gen.ts', '--check'], root);
  if (r.code !== 0) {
    const why = (r.stdout + r.stderr).trim().split('\n').slice(-1)[0] ?? '';
    res.violations.push({
      check: 'GEN',
      file: 'app/src/ipc/proto.gen.ts',
      message: `stale or missing: run \`bun run gen\` (${why})`,
    });
  }
  return res;
}

/** Runs every check against the repo at `root` and returns the combined result. */
export function checkAll(root: string): CheckResult {
  const files = listRepoFiles(root);
  const results = [
    checkInv1NoNetwork(root, files),
    checkInv2NoPtyBytes(root, files),
    checkInv3NoGpui(root),
    checkInv4KeyHandlers(root, files),
    checkInv5Palette(root, files),
    checkInv7Worktrees(root, files),
    checkInv11PersonalData(root, files),
    checkInv13Gpuix(root),
    checkInv17Ghostty(root),
    checkCrateLayers(root),
    checkAppLayers(root, files),
    checkRustStandards(root, files),
    checkExactPins(root, files),
    checkGeneratedTypes(root),
  ];
  return {
    violations: results.flatMap((r) => r.violations),
    notices: results.flatMap((r) => r.notices),
  };
}

function main(): void {
  const { violations, notices } = checkAll(ROOT);
  for (const n of notices) console.log(`notice: ${n}`);
  for (const v of violations) console.log(formatViolation(v));
  if (violations.length > 0) {
    console.log(`check-rules: ${violations.length} violation(s)`);
    process.exit(1);
  }
  console.log('check-rules: ok');
}

if (import.meta.main) main();
