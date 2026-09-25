import { existsSync, readdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, join } from 'node:path';
import { version } from '../../package.json';
import { log } from './log';

const REPO_ROOT = join(import.meta.dir, '..', '..', '..');

/** The login shell's name for shell panes' status chip, from `$SHELL`; `zsh` when unset. */
export function shellName(env: NodeJS.ProcessEnv = process.env): string {
  return env.SHELL ? basename(env.SHELL) : 'zsh';
}

function hasGeist(dir: string): boolean {
  try {
    return readdirSync(dir).some((name) => /^Geist(Mono)?-.*\.(ttf|otf)$/i.test(name));
  } catch (error) {
    log('warn', 'cannot list a font directory', { dir, error: String(error) });
    return false;
  }
}

/** Where macOS looks for user and system fonts; `just fonts` copies Geist into the first. */
export function fontDirs(home: string = homedir()): string[] {
  return [join(home, 'Library', 'Fonts'), '/Library/Fonts'];
}

/** Whether GPUI will resolve the Geist families: GPUIX loads no font files, so they must be installed in `dirs`. */
export function geistAvailable(dirs: readonly string[] = fontDirs()): boolean {
  return dirs.some((d) => existsSync(d) && hasGeist(d));
}

/** The app's build id, `<version>+<commit>` with `HEAD` of `root` abbreviated to 12 as plyd's `build.rs` does; `null` when git cannot tell. */
export async function readBuildId(root: string = REPO_ROOT): Promise<string | null> {
  try {
    const child = Bun.spawn(['git', '-C', root, 'rev-parse', '--short=12', 'HEAD'], {
      stdin: 'ignore',
      stdout: 'pipe',
      stderr: 'ignore',
    });
    const [code, out] = await Promise.all([child.exited, new Response(child.stdout).text()]);
    const commit = out.trim();
    return code === 0 && commit !== '' ? `${version}+${commit}` : null;
  } catch (error) {
    log('warn', 'cannot read the build id from git', { error: String(error) });
    return null;
  }
}

/** The macOS "Reduce motion" setting; `false` when it cannot be read (the key is absent until first set). */
export async function readReducedMotion(): Promise<boolean> {
  try {
    const child = Bun.spawn(['defaults', 'read', 'com.apple.universalaccess', 'reduceMotion'], {
      stdout: 'pipe',
      stderr: 'ignore',
    });
    const [code, out] = await Promise.all([child.exited, new Response(child.stdout).text()]);
    return code === 0 && out.trim() === '1';
  } catch (error) {
    log('warn', 'cannot read the reduce-motion setting', { error: String(error) });
    return false;
  }
}
