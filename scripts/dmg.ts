import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { join, normalize } from 'node:path';
import { run } from './lib/repo';

const ROOT = normalize(join(import.meta.dir, '..'));
const DIST = join(ROOT, 'dist');

/** The values the bundle's Info.plist carries. */
export interface BundleInfo {
  /** The root `package.json`'s version, a dotted number as `CFBundleVersion` requires. */
  version: string;
  /** `<version>+<commit>`, the id plyd reports in `welcome.daemon_version`. */
  buildId: string;
  /** From the LC_BUILD_VERSION of the binaries the bundle holds. */
  minimumSystemVersion: string;
}

const escapeXml = (s: string) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/** `ply.app/Contents/Info.plist`: bundle id `dev.ply.app` (the LaunchAgent is `dev.ply.app.plyd`), icon `Resources/ply.icns`. */
export function infoPlist(info: BundleInfo): string {
  const entries: [string, string | boolean][] = [
    ['CFBundleDevelopmentRegion', 'en'],
    ['CFBundleDisplayName', 'ply'],
    ['CFBundleExecutable', 'ply'],
    ['CFBundleIdentifier', 'dev.ply.app'],
    ['CFBundleInfoDictionaryVersion', '6.0'],
    ['CFBundleName', 'ply'],
    ['CFBundlePackageType', 'APPL'],
    ['CFBundleShortVersionString', info.version],
    ['CFBundleVersion', info.version],
    ['LSApplicationCategoryType', 'public.app-category.developer-tools'],
    ['LSMinimumSystemVersion', info.minimumSystemVersion],
    ['NSHighResolutionCapable', true],
    ['CFBundleIconFile', 'ply'],
    ['PlyBuildId', info.buildId],
  ];
  const body = entries
    .map(([key, value]) =>
      typeof value === 'boolean'
        ? `  <key>${key}</key>\n  <${value}/>`
        : `  <key>${key}</key>\n  <string>${escapeXml(value)}</string>`,
    )
    .join('\n');
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
${body}
</dict>
</plist>
`;
}

/** The highest `minos` of the Mach-O files, so the bundle claims no macOS its binaries cannot run on. */
export function minimumSystemVersion(otoolOutputs: string[]): string {
  const versions = otoolOutputs.flatMap((out) =>
    [...out.matchAll(/^\s*minos\s+(\d+(?:\.\d+)*)/gm)].map((m) => m[1] as string),
  );
  if (versions.length === 0) throw new Error('no LC_BUILD_VERSION minos in the binaries');
  const key = (v: string) => v.split('.').map(Number);
  return versions.reduce((a, b) => {
    const [x, y] = [key(a), key(b)];
    for (let i = 0; i < Math.max(x.length, y.length); i++) {
      if ((x[i] ?? 0) !== (y[i] ?? 0)) return (x[i] ?? 0) > (y[i] ?? 0) ? a : b;
    }
    return a;
  });
}

function step(title: string, cmd: string[], env?: Record<string, string>): void {
  console.log(`\n▸ ${title}`);
  const p = Bun.spawnSync({
    cmd,
    cwd: ROOT,
    stdout: 'inherit',
    stderr: 'inherit',
    env: env ? { ...process.env, ...env } : process.env,
  });
  if (p.exitCode !== 0) throw new Error(`${title} failed: ${cmd.join(' ')} exited ${p.exitCode}`);
}

function output(cmd: string[]): string {
  const r = run(cmd, ROOT);
  if (r.code !== 0) throw new Error(`${cmd.join(' ')} exited ${r.code}: ${r.stderr.trim()}`);
  return r.stdout;
}

const megabytes = (path: string) => `${(statSync(path).size / 1e6).toFixed(1)} MB`;

function main(): void {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') {
    throw new Error('just dmg builds an Apple-silicon macOS bundle; run it on an arm64 Mac');
  }
  const pkg = JSON.parse(readFileSync(join(ROOT, 'package.json'), 'utf8')) as {
    version: string;
  };
  const commit = output(['git', 'rev-parse', '--short=12', 'HEAD']).trim();
  const buildId = `${pkg.version}+${commit}`;
  if (output(['git', 'status', '--porcelain']).trim() !== '') {
    console.warn(`warning: uncommitted changes are built in, but the bundle still says ${buildId}`);
  }

  const app = join(DIST, 'ply.app');
  const contents = join(app, 'Contents');
  const macos = join(contents, 'MacOS');
  rmSync(app, { recursive: true, force: true });
  for (const dir of [macos, join(contents, 'Resources')]) mkdirSync(dir, { recursive: true });

  step(
    'the app, plyd and ply-hook (profile dist: fat LTO, one codegen unit; libghostty-vt ReleaseFast)',
    [
      'cargo',
      'build',
      '--profile',
      'dist',
      '--locked',
      '-p',
      'ply-app',
      '-p',
      'ply-daemon',
      '-p',
      'ply-hook',
    ],
    { PLY_BUILD_ID: buildId },
  );
  const daemonVersion = output([join(ROOT, 'target', 'dist', 'plyd'), '--version']).trim();
  if (daemonVersion !== `plyd ${buildId}`) {
    throw new Error(`plyd says "${daemonVersion}", the app would say ${buildId}`);
  }
  for (const bin of ['ply', 'plyd', 'ply-hook']) {
    copyFileSync(join(ROOT, 'target', 'dist', bin), join(macos, bin));
  }

  copyFileSync(
    join(ROOT, 'crates', 'app', 'assets', 'icon', 'ply.icns'),
    join(contents, 'Resources', 'ply.icns'),
  );

  const machO = [join(macos, 'ply'), join(macos, 'plyd'), join(macos, 'ply-hook')];
  const minos = minimumSystemVersion(machO.map((f) => output(['otool', '-l', f])));
  writeFileSync(
    join(contents, 'Info.plist'),
    infoPlist({ version: pkg.version, buildId, minimumSystemVersion: minos }),
  );
  output(['plutil', '-lint', join(contents, 'Info.plist')]);

  // Nested code is signed before the bundle that seals it; ad-hoc, since nothing here is notarised.
  for (const f of [join(macos, 'plyd'), join(macos, 'ply-hook')]) {
    output(['codesign', '--force', '--sign', '-', '--timestamp=none', f]);
  }
  output(['codesign', '--force', '--sign', '-', '--timestamp=none', app]);
  step('verify the signature', ['codesign', '--verify', '--strict', '--verbose=2', app]);

  const stage = join(DIST, 'dmg');
  const dmg = join(DIST, `ply-${pkg.version}.dmg`);
  rmSync(stage, { recursive: true, force: true });
  mkdirSync(stage, { recursive: true });
  output(['ditto', app, join(stage, 'ply.app')]);
  symlinkSync('/Applications', join(stage, 'Applications'));
  rmSync(dmg, { force: true });
  step('the disk image (LZFSE)', [
    'hdiutil',
    'create',
    '-quiet',
    '-volname',
    'ply',
    '-srcfolder',
    stage,
    '-format',
    'ULFO',
    '-ov',
    dmg,
  ]);
  rmSync(stage, { recursive: true, force: true });
  step('verify the disk image', ['hdiutil', 'verify', '-quiet', dmg]);

  if (!existsSync(dmg)) throw new Error(`${dmg} was not written`);
  console.log(`\n${buildId} · macOS ${minos}+`);
  console.log(
    `  ${app}  (ply ${megabytes(join(macos, 'ply'))}, plyd ${megabytes(join(macos, 'plyd'))})`,
  );
  console.log(`  ${dmg}  ${megabytes(dmg)}`);
}

if (import.meta.main) main();
