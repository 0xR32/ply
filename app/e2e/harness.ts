import {
  appendFileSync,
  chmodSync,
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { type App, connectStdio, type TreeNode } from '@gpuix/react/automation';
import { type ControlClient, createControlClient } from '../src/ipc/control-client';
import type { Layout, Pane } from '../src/ipc/proto.gen';

/** The checkout: the app runs from here against the cargo-built plyd (Ruling R41, no bundle). */
export const REPO = join(import.meta.dir, '..', '..');
const MAIN = join(REPO, 'app', 'src', 'main.tsx');
const FAKES = join(REPO, 'crates', 'daemon', 'tests', 'fake');

/** Polls `check` every 20 ms until it returns a value other than `undefined`/`false`; throws naming `what` after `timeoutMs`. */
export async function until<T>(
  check: () => T | undefined | false | Promise<T | undefined | false>,
  what: string,
  timeoutMs = 10_000,
): Promise<T> {
  const end = Date.now() + timeoutMs;
  for (;;) {
    const got = await check();
    if (got !== undefined && got !== false) return got;
    if (Date.now() > end) throw new Error(`timed out after ${timeoutMs} ms waiting for ${what}`);
    await Bun.sleep(20);
  }
}

/** Whether process `pid` exists (signal 0). */
export function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/** A throwaway `PLY_HOME`, plus (unless `realHome`) a `HOME` whose login `PATH` puts the fake CLIs first as `claude` and `codex`. */
export class Sandbox {
  readonly root: string;
  readonly home: string;
  readonly plyHome: string;
  private readonly fakes: boolean;

  constructor(options: { realHome?: boolean } = {}) {
    this.root = mkdtempSync(join(tmpdir(), 'ply-e2e-'));
    this.plyHome = join(this.root, 'ply');
    this.fakes = !options.realHome;
    this.home = this.fakes ? join(this.root, 'home') : (process.env.HOME ?? '');
    if (join(this.plyHome, 'run', 'data.sock').length >= 104) {
      throw new Error(`sandbox socket path too long under ${this.root}`);
    }
    if (!this.fakes) return;
    mkdirSync(join(this.home, 'bin'), { recursive: true });
    for (const [name, script] of [
      ['claude', 'fake-claude.sh'],
      ['codex', 'fake-codex.sh'],
    ] as const) {
      copyFileSync(join(FAKES, script), join(this.home, 'bin', name));
      chmodSync(join(this.home, 'bin', name), 0o755);
    }
    writeFileSync(join(this.home, '.profile'), 'PATH="$HOME/bin:$PATH"\nexport PATH\n');
  }

  /** The environment the app (and so the plyd it starts) runs with; never `NODE_ENV=test`, which makes the app inert. */
  env(extra: Record<string, string> = {}): Record<string, string> {
    const env: Record<string, string> = {};
    for (const [k, v] of Object.entries(process.env)) if (v !== undefined) env[k] = v;
    delete env.NODE_ENV;
    env.PLY_HOME = this.plyHome;
    env.PLY_WINDOW_FOCUS = '0';
    if (this.fakes) {
      env.HOME = this.home;
      env.SHELL = '/bin/sh';
      env.USER = 'example';
      delete env.CODEX_HOME;
    }
    return { ...env, ...extra };
  }

  /** `run/plyd.sock`. */
  get controlSocket(): string {
    return join(this.plyHome, 'run', 'plyd.sock');
  }

  /** The running plyd's pid from its instance lock, if one is alive. */
  plydPid(): number | undefined {
    const lock = join(this.plyHome, 'plyd.lock');
    if (!existsSync(lock)) return undefined;
    const pid = Number(readFileSync(lock, 'utf8').trim());
    return Number.isInteger(pid) && pid > 0 && alive(pid) ? pid : undefined;
  }

  /** Stops plyd with SIGTERM (SIGKILL after 5 s) and waits for it to be gone. */
  async stopPlyd(): Promise<void> {
    const pid = this.plydPid();
    if (pid === undefined) return;
    process.kill(pid, 'SIGTERM');
    const end = Date.now() + 5_000;
    while (alive(pid) && Date.now() < end) await Bun.sleep(20);
    if (alive(pid)) process.kill(pid, 'SIGKILL');
    await until(() => !alive(pid), `plyd ${pid} to exit`);
  }

  /** The command FIFO of the fake CLI in pane `paneId` (`crates/daemon/tests/common/fake.rs` lists the commands). */
  fake(paneId: number): FakeCli {
    return new FakeCli(join(this.home, `fake-${paneId}.cmd`));
  }

  /** The launch lines a fake CLI logged (`fake-claude.log` or `fake-codex.log`). */
  launches(cli: 'claude' | 'codex'): string[] {
    const path = join(this.home, `fake-${cli}.log`);
    return existsSync(path) ? readFileSync(path, 'utf8').trim().split('\n') : [];
  }

  /** Stops plyd and deletes the sandbox (copied into `$PLY_E2E_KEEP` first when set); end every app first, or its launcher starts plyd again. */
  async remove(): Promise<void> {
    await this.stopPlyd();
    if (this.fakes) Bun.spawnSync(['pkill', '-f', this.root]);
    const keep = process.env.PLY_E2E_KEEP;
    if (keep) {
      cpSync(this.root, join(keep, basename(this.root)), {
        recursive: true,
        filter: (src) => !/\.(sock|cmd)$/.test(src),
      });
    }
    rmSync(this.root, { recursive: true, force: true });
  }
}

/** Drives one fake CLI through its FIFO; the fake keeps the FIFO open, so writes never block. */
export class FakeCli {
  constructor(readonly fifo: string) {}

  /** Sends one command line once the fake has created its FIFO. */
  async send(line: string): Promise<void> {
    await until(() => existsSync(this.fifo), `the fake CLI's FIFO ${this.fifo}`);
    appendFileSync(this.fifo, `${line}\n`);
  }

  /** `hook <event> <payload>`: the fake pipes the payload into the hook command its `--settings` file registers. */
  hook(event: string, payload: Record<string, unknown>): Promise<void> {
    return this.send(`hook ${event} ${JSON.stringify(payload)}`);
  }
}

/** The pane ids of the tree's pane frames (`pane-<id>`), in document order. */
function frames(node: TreeNode | null | undefined, out: TreeNode[] = []): TreeNode[] {
  if (!node) return out;
  if (node.testId && /^pane-\d+$/.test(node.testId)) out.push(node);
  for (const child of node.children ?? []) frames(child, out);
  return out;
}

function textOf(node: TreeNode): string {
  return (node.text ?? '') + (node.children ?? []).map(textOf).join('');
}

function findTestId(node: TreeNode | null | undefined, testId: string): TreeNode | undefined {
  if (!node) return undefined;
  if (node.testId === testId) return node;
  for (const child of node.children ?? []) {
    const hit = findTestId(child, testId);
    if (hit) return hit;
  }
  return undefined;
}

/** The full app (`bun app/src/main.tsx`) in its own process, driven through GPUIX's stdio automation. */
export class AppProcess {
  private constructor(
    readonly child: ReturnType<typeof Bun.spawn>,
    readonly automation: App,
    readonly startedAt: number,
  ) {}

  /** Starts the app with `env` (stdin not a TTY, so GPUIX serves automation on stdio) and waits for the handshake. */
  static async launch(env: Record<string, string>, stderrPath: string): Promise<AppProcess> {
    const startedAt = performance.now();
    const child = Bun.spawn([process.execPath, MAIN], {
      cwd: REPO,
      env,
      stdin: 'pipe',
      stdout: 'pipe',
      stderr: openSync(stderrPath, 'a'),
    });
    const listeners: ((chunk: string) => void)[] = [];
    void (async () => {
      const decoder = new TextDecoder();
      for await (const chunk of child.stdout as ReadableStream<Uint8Array>) {
        const text = decoder.decode(chunk, { stream: true });
        for (const l of listeners) l(text);
      }
    })();
    const automation = await connectStdio({
      write: (chunk) => {
        const sink = child.stdin as import('bun').FileSink;
        sink.write(chunk);
        void sink.flush();
      },
      feed: (listener) => listeners.push(listener),
    });
    return new AppProcess(child, automation, startedAt);
  }

  /** The app's pid. */
  get pid(): number {
    return this.child.pid;
  }

  /** Sends space-separated keystrokes one at a time to whatever holds GPUI focus. */
  async keys(keystrokes: string): Promise<void> {
    for (const k of keystrokes.split(' ').filter(Boolean)) {
      await this.automation.call('keystrokes', { keys: k });
      await Bun.sleep(30);
    }
  }

  /** Types `text` into whatever holds focus, one keystroke per character (space, newline and tab by name). */
  async type(text: string): Promise<void> {
    const keys = [...text].map((ch) =>
      ch === ' ' ? 'space' : ch === '\n' ? 'enter' : ch === '\t' ? 'tab' : ch,
    );
    await this.automation.call('keystrokes', { keys: keys.join(' ') });
  }

  /** Every `<text>` of the retained tree, depth first. */
  async text(): Promise<string[]> {
    return (await this.automation.call('getAllText', {})).text;
  }

  /** Every string GPUI painted in the last frame. */
  async painted(): Promise<string[]> {
    return (await this.automation.call('getPaintedText', {})).text;
  }

  /** The retained tree with bounds. */
  async tree(): Promise<TreeNode | null> {
    return (await this.automation.call('getTree', {})).tree;
  }

  /** Waits until some `<text>` contains `needle`. */
  async waitForText(needle: string, timeoutMs = 10_000): Promise<void> {
    await until(
      async () => (await this.text()).some((t) => t.includes(needle)),
      `"${needle}" on screen`,
      timeoutMs,
    );
  }

  /** The text under the node with `testId`, concatenated (empty when absent). */
  async textIn(testId: string): Promise<string> {
    const node = findTestId(await this.tree(), testId);
    return node ? textOf(node) : '';
  }

  /** Whether a node with `testId` is in the tree. */
  async has(testId: string): Promise<boolean> {
    return findTestId(await this.tree(), testId) !== undefined;
  }

  /** Clicks the centre of the node with `testId`. */
  async click(testId: string): Promise<void> {
    await this.automation.getByTestId(testId).click();
  }

  /** Types `text` into the field with `testId` (select all, then the characters). */
  async fill(testId: string, text: string): Promise<void> {
    await this.automation.getByTestId(testId).fill(text);
  }

  /** The mounted pane frames' ids, in grid order. */
  async visiblePanes(): Promise<number[]> {
    return frames(await this.tree()).map((n) => Number(n.testId?.slice('pane-'.length)));
  }

  /** Quits through the app menu's ⌘Q, as a user would, and waits for the process to end (SIGTERM after 5 s). */
  async quit(): Promise<'cmd-q' | 'sigterm'> {
    // The app exits before it answers, so the call never settles.
    void this.automation.call('keystrokes', { keys: 'cmd-q' }).catch(() => undefined);
    const exited = await Promise.race([this.child.exited, Bun.sleep(5_000).then(() => null)]);
    if (exited !== null) return 'cmd-q';
    this.child.kill('SIGTERM');
    await this.child.exited;
    return 'sigterm';
  }

  /** Ends the process if it still runs. */
  async kill(): Promise<void> {
    if (this.child.exitCode === null && this.child.signalCode === null) {
      this.child.kill('SIGKILL');
      await this.child.exited;
    }
  }
}

/** A C1 client of the test's own, beside the app: reads what plyd holds (panes, sessions) to check what the app shows. */
export class Observer {
  private constructor(
    readonly client: ControlClient,
    readonly workspaceId: number,
  ) {}

  /** Connects once plyd answers on `socketPath`, and picks plyd's home workspace. */
  static async connect(socketPath: string): Promise<Observer> {
    const client = createControlClient({ socketPath, appVersion: 'e2e', initialBackoffMs: 20 });
    client.start();
    await until(() => client.state.kind === 'connected', 'the observer to connect to plyd', 20_000);
    const workspaces = await client.request('workspace.list', {});
    const first = workspaces[0];
    if (!first) throw new Error('plyd has no workspace');
    return new Observer(client, first.id);
  }

  /** Every open pane of the workspace, by id. */
  async panes(): Promise<Pane[]> {
    const list = await this.client.request('pane.list', { workspace_id: this.workspaceId });
    return [...list].sort((a, b) => a.id - b.id);
  }

  /** Waits until pane `id` exists and satisfies `check`; returns it. */
  async waitPane(id: number, check: (p: Pane) => boolean, what: string, timeoutMs = 10_000) {
    return until(
      async () => {
        if (this.client.state.kind !== 'connected') return undefined;
        const p = (await this.panes().catch(() => [])).find((x) => x.id === id);
        return p && check(p) ? p : undefined;
      },
      what,
      timeoutMs,
    );
  }

  /** Waits until the workspace holds `count` panes; returns them by id. */
  async waitCount(count: number, what: string): Promise<Pane[]> {
    return until(async () => {
      const list = await this.panes().catch(() => []);
      return list.length === count ? list : undefined;
    }, what);
  }

  /** The layout the app last saved (`layout.save`, 250 ms after a tab or focus change). */
  layout(): Promise<Layout> {
    return this.client.request('layout.get', { workspace_id: this.workspaceId });
  }

  /** Waits until the app's saved layout shows pane `paneId` focused in the active tab. */
  async waitFocus(paneId: number, what: string): Promise<void> {
    await until(async () => {
      const layout = await this.layout().catch(() => undefined);
      const tab = layout?.tabs.find((t) => t.pane_ids.includes(paneId));
      return tab !== undefined && layout?.active_tab_id === tab.id && tab.focus_pane_id === paneId;
    }, what);
  }

  /** Closes the connection. */
  close(): void {
    this.client.stop();
  }
}
