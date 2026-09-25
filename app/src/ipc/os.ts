import { existsSync, readdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { log } from './log';

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

/** Whether GPUI will resolve the Geist families: bundled with the .app (Resources/fonts) or installed by the user. */
export function geistAvailable(execPath: string = process.execPath): boolean {
  const bundleFonts = join(dirname(execPath), '..', 'Resources', 'fonts');
  const dirs = [bundleFonts, join(homedir(), 'Library', 'Fonts'), '/Library/Fonts'];
  return dirs.some((d) => existsSync(d) && hasGeist(d));
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
