import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { log } from './log';

/** Where the starter looks for plyd; everything defaults to the running process and its environment. */
export interface DaemonLauncherOptions {
  env?: NodeJS.ProcessEnv;
  /** The app executable; a `plyd` beside it means the app runs from the bundle's `Contents/MacOS`. */
  execPath?: string;
  /** Repository root for development builds (`target/{debug,release}/plyd`). */
  repoRoot?: string;
}

/** `launch-agent`: run `plyd install-agent` (plyd writes its own LaunchAgent, R38); `spawn`: `PLY_HOME` runs never touch it. */
export type LaunchPlan =
  | { kind: 'launch-agent'; plyd: string }
  | { kind: 'spawn'; plyd: string }
  | { kind: 'unavailable'; reason: string };

const REPO_ROOT = join(import.meta.dir, '..', '..', '..');

const NOT_BUILT = 'plyd is not built (cargo build -p ply-daemon -p ply-hook)';

/** Decides how plyd would be started; `PLY_PLYD` names a specific plyd binary for development. */
export function planLaunch(options: DaemonLauncherOptions = {}): LaunchPlan {
  const env = options.env ?? process.env;
  const bundled = join(dirname(options.execPath ?? process.execPath), 'plyd');
  const root = options.repoRoot ?? REPO_ROOT;
  const built = [
    env.PLY_PLYD,
    join(root, 'target', 'debug', 'plyd'),
    join(root, 'target', 'release', 'plyd'),
  ].find((c): c is string => c !== undefined && existsSync(c));
  if (env.PLY_HOME) {
    return built ? { kind: 'spawn', plyd: built } : { kind: 'unavailable', reason: NOT_BUILT };
  }
  if (existsSync(bundled)) return { kind: 'launch-agent', plyd: bundled };
  if (built) return { kind: 'launch-agent', plyd: built };
  return { kind: 'unavailable', reason: NOT_BUILT };
}

async function installAgent(plyd: string, env: NodeJS.ProcessEnv): Promise<void> {
  const child = Bun.spawn([plyd, 'install-agent'], { env, stdout: 'pipe', stderr: 'pipe' });
  const [code, stdout, stderr] = await Promise.all([
    child.exited,
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
  ]);
  if (code !== 0) throw new Error(`plyd install-agent failed (${code}): ${stderr.trim()}`);
  log('info', 'plyd install-agent', { plyd, report: stdout.trim() });
}

function spawnDetached(plyd: string, env: NodeJS.ProcessEnv): void {
  const child = Bun.spawn([plyd, '--foreground'], {
    env,
    stdin: 'ignore',
    stdout: 'ignore',
    stderr: 'ignore',
    detached: true,
  });
  child.unref();
  log('info', 'started plyd from the build directory', { plyd, pid: child.pid });
}

/** A `startDaemon` for the control client; it throws with a readable reason when plyd cannot be started. */
export function createDaemonStarter(options: DaemonLauncherOptions = {}): () => Promise<void> {
  return async () => {
    const env = options.env ?? process.env;
    const plan = planLaunch(options);
    switch (plan.kind) {
      case 'launch-agent':
        await installAgent(plan.plyd, env);
        return;
      case 'spawn':
        spawnDetached(plan.plyd, env);
        return;
      case 'unavailable':
        throw new Error(plan.reason);
    }
  };
}
