import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { log } from './log';
import { LAUNCH_AGENT_LABEL, launchAgentPath } from './paths';

/** Where the starter looks for plyd; everything defaults to the running process and its environment. */
export interface DaemonLauncherOptions {
  env?: NodeJS.ProcessEnv;
  /** The app executable; a `plyd` beside it means the app runs from the bundle's `Contents/MacOS`. */
  execPath?: string;
  /** Repository root for development builds (`target/{debug,release}/plyd`). */
  repoRoot?: string;
}

/** How the starter will bring plyd up: the LaunchAgent for a bundle, a detached build-dir binary in development. */
export type LaunchPlan =
  | { kind: 'launch-agent'; plyd: string; plist: string }
  | { kind: 'spawn'; plyd: string }
  | { kind: 'unavailable'; reason: string };

const REPO_ROOT = join(import.meta.dir, '..', '..', '..');

/** The LaunchAgent plist for `plyd` (spec 11.3): started on demand, restarted only after a crash, not throttled. */
export function launchAgentPlist(plyd: string): string {
  const escaped = plyd.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LAUNCH_AGENT_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${escaped}</string>
  </array>
  <key>RunAtLoad</key>
  <false/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ProcessType</key>
  <string>Standard</string>
</dict>
</plist>
`;
}

/** Decides how plyd would be started; `PLY_HOME` forces a build-dir spawn so a test never touches the LaunchAgent. */
export function planLaunch(options: DaemonLauncherOptions = {}): LaunchPlan {
  const env = options.env ?? process.env;
  const bundled = join(dirname(options.execPath ?? process.execPath), 'plyd');
  if (!env.PLY_HOME && existsSync(bundled)) {
    return { kind: 'launch-agent', plyd: bundled, plist: launchAgentPath() };
  }
  const root = options.repoRoot ?? REPO_ROOT;
  const candidates = [
    env.PLY_PLYD,
    join(root, 'target', 'debug', 'plyd'),
    join(root, 'target', 'release', 'plyd'),
  ];
  const plyd = candidates.find((c): c is string => c !== undefined && existsSync(c));
  if (plyd) return { kind: 'spawn', plyd };
  return { kind: 'unavailable', reason: 'plyd is not built (cargo build -p ply-daemon)' };
}

async function launchctl(args: string[]): Promise<{ code: number; stderr: string }> {
  const child = Bun.spawn(['launchctl', ...args], { stdout: 'ignore', stderr: 'pipe' });
  const [code, stderr] = await Promise.all([child.exited, new Response(child.stderr).text()]);
  return { code, stderr: stderr.trim() };
}

async function startWithLaunchAgent(plyd: string, plist: string): Promise<void> {
  const wanted = launchAgentPlist(plyd);
  const current = existsSync(plist) ? readFileSync(plist, 'utf8') : null;
  const domain = `gui/${process.getuid?.() ?? 0}`;
  if (current !== wanted) {
    mkdirSync(dirname(plist), { recursive: true });
    if (current !== null) await launchctl(['bootout', `${domain}/${LAUNCH_AGENT_LABEL}`]);
    writeFileSync(plist, wanted);
    log('info', 'installed the plyd LaunchAgent', { plist });
  }
  const boot = await launchctl(['bootstrap', domain, plist]);
  // bootstrap fails with EIO (5), EEXIST (17) or EALREADY (37) when the agent is already loaded, the usual case.
  if (boot.code !== 0 && boot.code !== 5 && boot.code !== 17 && boot.code !== 37) {
    throw new Error(`launchctl bootstrap failed (${boot.code}): ${boot.stderr}`);
  }
  const kick = await launchctl(['kickstart', `${domain}/${LAUNCH_AGENT_LABEL}`]);
  if (kick.code !== 0) throw new Error(`launchctl kickstart failed (${kick.code}): ${kick.stderr}`);
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
    const plan = planLaunch(options);
    switch (plan.kind) {
      case 'launch-agent':
        await startWithLaunchAgent(plan.plyd, plan.plist);
        return;
      case 'spawn':
        spawnDetached(plan.plyd, options.env ?? process.env);
        return;
      case 'unavailable':
        throw new Error(plan.reason);
    }
  };
}
