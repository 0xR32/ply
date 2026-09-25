import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { log } from './log';

/** Where the starter looks for plyd; everything defaults to the running process and its environment. */
export interface DaemonLauncherOptions {
  env?: NodeJS.ProcessEnv;
  /** Repository root of the cargo builds (`target/{release,debug}/plyd`); ply runs from its checkout (no bundle). */
  repoRoot?: string;
}

/** `launch-agent`: run `plyd install-agent` (plyd writes its own LaunchAgent, R38); `spawn`: `PLY_HOME` runs never touch it. */
export type LaunchPlan =
  | { kind: 'launch-agent'; plyd: string }
  | { kind: 'spawn'; plyd: string }
  | { kind: 'unavailable'; reason: string };

const REPO_ROOT = join(import.meta.dir, '..', '..', '..');

const NOT_BUILT = 'plyd is not built (cargo build --release -p ply-daemon -p ply-hook)';

/** Decides how plyd would be started: `PLY_PLYD` if set, else the release build, else the debug build (the release one wins when both exist). */
export function planLaunch(options: DaemonLauncherOptions = {}): LaunchPlan {
  const env = options.env ?? process.env;
  const root = options.repoRoot ?? REPO_ROOT;
  const built = [
    env.PLY_PLYD,
    join(root, 'target', 'release', 'plyd'),
    join(root, 'target', 'debug', 'plyd'),
  ].find((c): c is string => c !== undefined && existsSync(c));
  if (!built) return { kind: 'unavailable', reason: NOT_BUILT };
  return env.PLY_HOME ? { kind: 'spawn', plyd: built } : { kind: 'launch-agent', plyd: built };
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
