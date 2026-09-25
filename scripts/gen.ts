import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, normalize } from 'node:path';
import { run } from './lib/repo';

const ROOT = normalize(join(import.meta.dir, '..'));
const TARGET = 'app/src/ipc/proto.gen.ts';

/** The TypeScript ts-rs derives from ply-proto, formatted by Biome exactly as the committed file must be. */
export function generateProtoTypes(root: string): string {
  const dir = mkdtempSync(join(tmpdir(), 'ply-gen-'));
  try {
    const out = join(dir, 'proto.gen.ts');
    const test = run(
      ['cargo', 'test', '--quiet', '-p', 'ply-proto', '--test', 'export_bindings'],
      root,
      { PLY_GEN_OUT: out },
    );
    if (test.code !== 0) throw new Error(`ts-rs export failed:\n${test.stdout}${test.stderr}`);
    const raw = readFileSync(out, 'utf8');
    const fmt = Bun.spawnSync({
      cmd: ['bunx', 'biome', 'format', `--stdin-file-path=${TARGET}`],
      cwd: root,
      stdin: new TextEncoder().encode(raw),
      stdout: 'pipe',
      stderr: 'pipe',
    });
    if (fmt.exitCode !== 0) throw new Error(`biome format failed: ${fmt.stderr.toString()}`);
    return fmt.stdout.toString();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function main(): void {
  const check = process.argv.includes('--check');
  const generated = generateProtoTypes(ROOT);
  const path = join(ROOT, TARGET);
  if (check) {
    let committed = '';
    try {
      committed = readFileSync(path, 'utf8');
    } catch {
      committed = '';
    }
    if (committed !== generated) {
      console.log(`gen: ${TARGET} is stale; run \`bun run gen\``);
      process.exit(1);
    }
    console.log(`gen: ${TARGET} is fresh`);
    return;
  }
  writeFileSync(path, generated);
  console.log(`gen: wrote ${TARGET}`);
}

if (import.meta.main) main();
