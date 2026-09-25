import { writeFileSync } from 'node:fs';
import { join, normalize } from 'node:path';
import { readText, run } from './lib/repo';

const ROOT = normalize(join(import.meta.dir, '..'));
const ALLOW_FILE = 'deps.allow.toml';
const MIN_STARS = 1000;
const MIN_CONTRIBUTORS = 3;
const MAX_COMMIT_AGE_DAYS = 183;
const ECOSYSTEMS = ['cargo', 'npm', 'vendor'] as const;

/** Package ecosystem an admission entry belongs to. */
export type Ecosystem = (typeof ECOSYSTEMS)[number];

/** One admitted dependency and the INV-16 numbers recorded for its upstream repository. */
export interface Admission {
  repo: string;
  stars: number;
  last_commit: string;
  contributors_5plus: number;
  checked: string;
  /** Decision id (D4, D5) that admits the dependency outside INV-16; the numbers are still recorded. */
  exception?: string;
  note?: string;
}

/** Parsed `deps.allow.toml`: ecosystem -> package name -> admission. */
export type AllowList = Record<Ecosystem, Record<string, Admission>>;

/** A direct dependency found in a manifest. */
export interface DirectDep {
  ecosystem: Ecosystem;
  name: string;
  manifest: string;
}

function daysBetween(a: string, b: string): number {
  return (Date.parse(b) - Date.parse(a)) / 86_400_000;
}

/** INV-16 problems with one admission entry; an `exception` skips the thresholds but not the fields. */
export function admissionProblems(name: string, a: Partial<Admission>): string[] {
  const out: string[] = [];
  for (const key of ['repo', 'stars', 'last_commit', 'contributors_5plus', 'checked'] as const) {
    if (a[key] === undefined || a[key] === '') out.push(`${name}: missing \`${key}\``);
  }
  if (out.length > 0) return out;
  const adm = a as Admission;
  for (const key of ['last_commit', 'checked'] as const) {
    if (Number.isNaN(Date.parse(adm[key])))
      out.push(`${name}: \`${key}\` is not a date (${adm[key]})`);
  }
  if (adm.exception) return out;
  if (adm.stars < MIN_STARS) out.push(`${name}: ${adm.stars} stars, INV-16 needs ${MIN_STARS}`);
  if (adm.contributors_5plus < MIN_CONTRIBUTORS) {
    out.push(
      `${name}: ${adm.contributors_5plus} contributors with >= 5 commits, INV-16 needs ${MIN_CONTRIBUTORS}`,
    );
  }
  if (daysBetween(adm.last_commit, adm.checked) > MAX_COMMIT_AGE_DAYS) {
    out.push(`${name}: last commit ${adm.last_commit} is more than 6 months before ${adm.checked}`);
  }
  return out;
}

interface CargoMeta {
  packages: {
    name: string;
    manifest_path: string;
    dependencies: { name: string; source: string | null }[];
  }[];
}

/** Every third-party dependency named directly by a workspace Cargo.toml or package.json. */
export function directDependencies(root: string): DirectDep[] {
  const out: DirectDep[] = [];
  const meta = run(['cargo', 'metadata', '--format-version', '1', '--no-deps'], root);
  if (meta.code !== 0) throw new Error(`cargo metadata failed: ${meta.stderr.trim()}`);
  for (const pkg of (JSON.parse(meta.stdout) as CargoMeta).packages) {
    for (const dep of pkg.dependencies) {
      if (dep.source !== null) {
        out.push({
          ecosystem: 'cargo',
          name: dep.name,
          manifest: pkg.manifest_path.slice(root.length + 1),
        });
      }
    }
  }
  const cargoToml = Bun.TOML.parse(readText(root, 'Cargo.toml') ?? '') as {
    workspace?: { dependencies?: Record<string, string | { path?: string }> };
  };
  for (const [name, spec] of Object.entries(cargoToml.workspace?.dependencies ?? {})) {
    if (typeof spec === 'object' && spec.path) continue;
    out.push({ ecosystem: 'cargo', name, manifest: 'Cargo.toml' });
  }
  for (const manifest of ['package.json', 'app/package.json']) {
    const text = readText(root, manifest);
    if (text === null) continue;
    const pkg = JSON.parse(text) as Record<string, Record<string, string> | undefined>;
    for (const section of [
      'dependencies',
      'devDependencies',
      'optionalDependencies',
      'peerDependencies',
    ]) {
      for (const [name, spec] of Object.entries(pkg[section] ?? {})) {
        if (spec.startsWith('workspace:')) continue;
        out.push({ ecosystem: 'npm', name, manifest });
      }
    }
  }
  if (readText(root, 'crates/ghostty-sys/build.rs')?.includes('GHOSTTY_ARCHIVE_URL')) {
    out.push({
      ecosystem: 'vendor',
      name: 'libghostty-vt',
      manifest: 'crates/ghostty-sys/build.rs',
    });
  }
  return out;
}

/** Reads `deps.allow.toml`; missing ecosystems come back empty. */
export function readAllowList(root: string): AllowList {
  const parsed = Bun.TOML.parse(readText(root, ALLOW_FILE) ?? '') as Partial<AllowList>;
  return { cargo: parsed.cargo ?? {}, npm: parsed.npm ?? {}, vendor: parsed.vendor ?? {} };
}

function tomlKey(name: string): string {
  return /^[A-Za-z0-9_-]+$/.test(name) ? name : JSON.stringify(name);
}

/** Serialises the allow list in a stable order: ecosystems, then names, then fields. */
export function renderAllowList(list: AllowList): string {
  const lines = [
    '# INV-16 admission list. Numbers come from `bun scripts/check-deps.ts --refresh`; see docs/development.md.',
  ];
  const fields = [
    'repo',
    'stars',
    'last_commit',
    'contributors_5plus',
    'checked',
    'exception',
    'note',
  ] as const;
  for (const eco of ECOSYSTEMS) {
    for (const name of Object.keys(list[eco]).sort()) {
      const a = list[eco][name] as Admission;
      lines.push('', `[${eco}.${tomlKey(name)}]`);
      for (const f of fields) {
        const v = a[f];
        if (v === undefined) continue;
        lines.push(`${f} = ${typeof v === 'number' ? String(v) : JSON.stringify(v)}`);
      }
    }
  }
  return `${lines.join('\n')}\n`;
}

/** Every problem: unlisted direct dependencies and entries that fail INV-16. */
export function checkDeps(root: string): { problems: string[]; notices: string[] } {
  const list = readAllowList(root);
  const deps = directDependencies(root);
  const problems: string[] = [];
  const seen = new Set<string>();
  for (const dep of deps) {
    const key = `${dep.ecosystem}:${dep.name}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const entry = list[dep.ecosystem][dep.name];
    if (!entry) {
      problems.push(`${key} (${dep.manifest}) is not admitted in ${ALLOW_FILE}`);
      continue;
    }
    problems.push(...admissionProblems(key, entry));
  }
  const notices: string[] = [];
  for (const eco of ECOSYSTEMS) {
    for (const name of Object.keys(list[eco])) {
      if (!seen.has(`${eco}:${name}`))
        notices.push(`${eco}:${name} is admitted but no manifest uses it`);
    }
  }
  return { problems, notices };
}

function gh(path: string): unknown {
  const r = run(['gh', 'api', path], ROOT);
  if (r.code !== 0) throw new Error(`gh api ${path}: ${r.stderr.trim()}`);
  return JSON.parse(r.stdout);
}

/** Fetches stars, last default-branch commit and contributors with >= 5 commits for `owner/repo`. */
export function fetchNumbers(repo: string, today: string): Omit<Admission, 'exception' | 'note'> {
  const info = gh(`repos/${repo}`) as { stargazers_count: number; default_branch: string };
  const commits = gh(`repos/${repo}/commits?per_page=1&sha=${info.default_branch}`) as {
    commit: { committer: { date: string } };
  }[];
  let contributors = 0;
  for (let page = 1; ; page++) {
    const batch = gh(`repos/${repo}/contributors?per_page=100&page=${page}`) as {
      contributions: number;
    }[];
    const qualified = batch.filter((c) => c.contributions >= 5).length;
    contributors += qualified;
    if (batch.length < 100 || qualified < batch.length) break;
  }
  return {
    repo,
    stars: info.stargazers_count,
    last_commit: (commits[0]?.commit.committer.date ?? '').slice(0, 10),
    contributors_5plus: contributors,
    checked: today,
  };
}

function refresh(adds: string[], only: string[]): void {
  const list = readAllowList(ROOT);
  const today = new Date().toISOString().slice(0, 10);
  for (const add of adds) {
    const m = /^(cargo|npm|vendor):(.+)=([\w.-]+\/[\w.-]+)$/.exec(add);
    if (!m) throw new Error(`--add expects <cargo|npm|vendor>:<name>=<owner/repo>, got ${add}`);
    const [, eco, name, repo] = m as unknown as [string, Ecosystem, string, string];
    list[eco][name] = { ...(list[eco][name] ?? {}), repo } as Admission;
    only.push(`${eco}:${name}`);
  }
  for (const eco of ECOSYSTEMS) {
    for (const [name, entry] of Object.entries(list[eco])) {
      if (only.length > 0 && !only.includes(`${eco}:${name}`)) continue;
      const numbers = fetchNumbers(entry.repo, today);
      list[eco][name] = { ...entry, ...numbers };
      console.log(`${eco}:${name} ${JSON.stringify(numbers)}`);
    }
  }
  writeFileSync(join(ROOT, ALLOW_FILE), renderAllowList(list));
}

function main(): void {
  const args = process.argv.slice(2);
  if (args.includes('--refresh')) {
    const adds: string[] = [];
    const only: string[] = [];
    for (let i = 0; i < args.length; i++) {
      if (args[i] === '--add') adds.push(args[++i] ?? '');
      else if (args[i] !== '--refresh') only.push(args[i] ?? '');
    }
    refresh(adds, only);
  }
  const { problems, notices } = checkDeps(ROOT);
  for (const n of notices) console.log(`notice: ${n}`);
  for (const p of problems) console.log(`INV-16 ${p}`);
  if (problems.length > 0) {
    console.log(`check-deps: ${problems.length} problem(s)`);
    process.exit(1);
  }
  console.log('check-deps: ok');
}

if (import.meta.main) main();
