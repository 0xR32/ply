import { homedir } from 'node:os';
import { join } from 'node:path';

/** ply's data directory: `$PLY_HOME` when set (tests, development), else `~/Library/Application Support/ply`. */
export function dataDir(env: NodeJS.ProcessEnv = process.env): string {
  return env.PLY_HOME ?? join(homedir(), 'Library', 'Application Support', 'ply');
}

/** The run directory holding plyd's sockets (mode 0700, created by plyd). */
export function runDir(env: NodeJS.ProcessEnv = process.env): string {
  return join(dataDir(env), 'run');
}

/** The C1 control socket; macOS caps a socket path at 104 bytes, which `PLY_HOME` must respect. */
export function controlSocketPath(env: NodeJS.ProcessEnv = process.env): string {
  return join(runDir(env), 'plyd.sock');
}
