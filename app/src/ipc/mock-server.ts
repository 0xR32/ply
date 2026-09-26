import { mkdirSync, rmSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, dirname, isAbsolute, join } from 'node:path';
import type { Socket, UnixSocketListener } from 'bun';
import { log } from './log';
import { controlSocketPath } from './paths';
import {
  type Answer,
  type Cli,
  type ClientMsg,
  type ErrorCode,
  type Event,
  HANDSHAKE_ID,
  type JsonValue,
  type Layout,
  type Method,
  type Methods,
  type Pane,
  type PaneStatus,
  PROTOCOL_VERSION,
  type Progress,
  type Session,
  type Settings,
  type TerminalTheme,
  type Usage,
  type Workspace,
} from './proto.gen';

/** A request the mock received, in arrival order. */
export interface MockRequest {
  m: Method;
  p: unknown;
}

/** How the mock behaves; development runs use `live`, tests keep it off for deterministic event order. */
export interface MockServerOptions {
  socketPath: string;
  /** Workspace path; defaults to the user's home directory. */
  home?: string;
  /** `demo` seeds two tabs with claude, codex and shell panes; `empty` starts with no panes. */
  scenario?: 'demo' | 'empty';
  /** Change statuses on timers (a running agent finishes, answered prompts resume) like real CLIs. */
  live?: boolean;
  /** Milliseconds before a created agent pane goes from `starting` to `idle`; `null` keeps it starting. */
  startDelayMs?: number | null;
  /** Protocol version the mock answers `hello` with, to exercise the mismatch path. */
  protocolVersion?: number;
}

class MethodError extends Error {
  constructor(
    readonly code: ErrorCode,
    message: string,
  ) {
    super(message);
  }
}

const DEFAULT_SETTINGS: Settings = {
  accent: 'blue',
  option_as_meta: 'off',
  keep_awake_while_running: true,
  resume_sessions_on_start: true,
  use_ply_colours_in_claude: true,
  codex_plan_tool: true,
  scrollback_lines: 10_000,
  font_size: 12.5,
};

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

/** A C1 server over a Unix socket that answers every method from in-memory state (dev and tests only, never shipped). */
export class MockServer {
  readonly requests: MockRequest[] = [];
  readonly panes = new Map<number, Pane>();
  /** Stored records of closed panes, which `session.list {include_closed:true}` returns after the open ones. */
  readonly closedSessions: Session[] = [];
  palette: TerminalTheme | null = null;
  layout: Layout | null = null;
  settings: Settings = { ...DEFAULT_SETTINGS };
  /** What `usage.get` answers; tests set it, and `{}` means neither CLI recorded any usage. */
  usage: Usage = {};
  readonly workspace: Workspace;
  private readonly clients = new Set<Socket<{ buffer: string; welcomed: boolean }>>();
  private readonly timers = new Set<ReturnType<typeof setTimeout>>();
  private readonly trailers = new Map<Method, Event[]>();
  private readonly refusals = new Map<Method, { code: ErrorCode; msg: string }>();
  private listener: UnixSocketListener<{ buffer: string; welcomed: boolean }> | null = null;
  private nextPaneId = 1;
  private nextTabId = 1;

  private constructor(private readonly options: MockServerOptions) {
    const home = options.home ?? homedir();
    this.workspace = { id: 1, path: home, name: basename(home) || home, opened_at: nowSeconds() };
    if (options.scenario === 'demo') this.seedDemo(home);
  }

  /** Binds `socketPath` (creating its directory, replacing a stale socket) and starts accepting clients. */
  static start(options: MockServerOptions): MockServer {
    const server = new MockServer(options);
    mkdirSync(dirname(options.socketPath), { recursive: true, mode: 0o700 });
    rmSync(options.socketPath, { force: true });
    server.listener = Bun.listen<{ buffer: string; welcomed: boolean }>({
      unix: options.socketPath,
      socket: {
        open: (s) => {
          s.data = { buffer: '', welcomed: false };
          server.clients.add(s);
        },
        data: (s, chunk) => server.onData(s, chunk.toString()),
        close: (s) => {
          server.clients.delete(s);
        },
        error: (_s, error) => log('warn', 'mock client error', { error: String(error) }),
      },
    });
    return server;
  }

  /** Closes every client and the socket and cancels scripted timers. */
  stop(): void {
    for (const t of this.timers) clearTimeout(t);
    this.timers.clear();
    for (const c of this.clients) c.end();
    this.clients.clear();
    this.listener?.stop(true);
    this.listener = null;
    rmSync(this.options.socketPath, { force: true });
  }

  /** Disconnects every client without stopping, like a plyd restart the app must recover from. */
  dropClients(): void {
    for (const c of this.clients) c.end();
    this.clients.clear();
  }

  /** Broadcasts one event to every client that finished the handshake. */
  emit(event: Event): void {
    const line = `${JSON.stringify({ t: 'evt', ...event })}\n`;
    for (const c of this.clients) if (c.data.welcomed) c.write(line);
  }

  /** Adds an open pane (defaults: tab 1, idle, the workspace path) and announces it with `pane.added`. */
  addPane(fields: Partial<Pane> & { cli: Cli }): Pane {
    const id = fields.id ?? this.nextPaneId;
    this.nextPaneId = Math.max(this.nextPaneId, id + 1);
    const tabId = fields.tab_id ?? 1;
    this.nextTabId = Math.max(this.nextTabId, tabId + 1);
    const position = [...this.panes.values()].filter((p) => p.tab_id === tabId).length;
    const pane: Pane = {
      workspace_id: this.workspace.id,
      position,
      cwd: this.workspace.path,
      title: fields.cli,
      status: 'idle',
      created_at: nowSeconds(),
      ...fields,
      id,
      tab_id: tabId,
    };
    this.panes.set(id, pane);
    this.emit({ e: 'pane.added', p: pane });
    return pane;
  }

  /** Sets a pane's status and broadcasts `pane.status` (with `exit_code` for `exited`). */
  setStatus(paneId: number, status: PaneStatus, detail?: string, exitCode?: number): void {
    const pane = this.panes.get(paneId);
    if (!pane) return;
    const { detail: _d, exit_code: _e, ...rest } = pane;
    const next: Pane = {
      ...rest,
      status,
      ...(detail !== undefined ? { detail } : {}),
      ...(status === 'exited' ? { exit_code: exitCode ?? 0 } : {}),
    };
    this.panes.set(paneId, next);
    this.emit({
      e: 'pane.status',
      p: {
        pane_id: paneId,
        status,
        at: nowSeconds(),
        ...(detail !== undefined ? { detail } : {}),
        ...(status === 'exited' ? { exit_code: exitCode ?? 0 } : {}),
      },
    });
  }

  /** Sets or clears a pane's plan progress and broadcasts `pane.progress`. */
  setProgress(paneId: number, progress: Progress | undefined): void {
    const pane = this.panes.get(paneId);
    if (!pane) return;
    const { progress: _old, ...rest } = pane;
    this.panes.set(paneId, progress ? { ...rest, progress } : rest);
    this.emit({ e: 'pane.progress', p: { pane_id: paneId, ...(progress ? { progress } : {}) } });
  }

  /** Writes `event` in the same socket write as the answer to the next `method` request, as plyd's writer can; the mock's state is left as it is. */
  trailAnswer(method: Method, event: Event): void {
    this.trailers.set(method, [...(this.trailers.get(method) ?? []), event]);
  }

  /** Answers the next `method` request with the error `code` instead of handling it. */
  refuseNext(method: Method, code: ErrorCode, msg: string): void {
    this.refusals.set(method, { code, msg });
  }

  /** Requests of one method, in arrival order. */
  requestsOf<M extends Method>(method: M): Methods[M]['params'][] {
    return this.requests.filter((r) => r.m === method).map((r) => r.p as Methods[M]['params']);
  }

  private later(ms: number, run: () => void): void {
    const t = setTimeout(() => {
      this.timers.delete(t);
      run();
    }, ms);
    this.timers.add(t);
  }

  private seedDemo(home: string): void {
    const ply = join(home, 'code', 'ply');
    const notes = join(home, 'code', 'notes');
    const seed = (p: Partial<Pane> & { cli: Cli; id: number; tab_id: number }) => {
      const position = [...this.panes.values()].filter((x) => x.tab_id === p.tab_id).length;
      this.panes.set(p.id, {
        workspace_id: this.workspace.id,
        position,
        cwd: ply,
        title: p.cli,
        status: 'idle',
        created_at: nowSeconds() - 600,
        ...p,
      });
    };
    seed({
      id: 1,
      tab_id: 1,
      cli: 'claude',
      title: 'Tab bar polish',
      status: 'running',
      progress: { done: 3, total: 5, current: 'add a cross-screen test' },
      model_seen: 'claude-opus-5',
      branch: 'feat/tab-bar',
    });
    seed({
      id: 2,
      tab_id: 1,
      cli: 'claude',
      title: 'Review the page guides',
      status: 'waiting_permission',
      detail: 'claude wants to edit guide-dot.tsx',
      progress: { done: 2, total: 4 },
      model_seen: 'claude-sonnet-5',
      branch: 'feat/page-guides',
    });
    seed({ id: 3, tab_id: 1, cli: 'shell', title: 'zsh', branch: 'feat/tab-bar' });
    seed({
      id: 4,
      tab_id: 2,
      cli: 'codex',
      cwd: notes,
      title: "This week's merges",
      progress: { done: 1, total: 1 },
      model_seen: 'gpt-5-codex',
      branch: 'main',
    });
    this.nextPaneId = 5;
    this.nextTabId = 3;
    this.layout = {
      tabs: [
        {
          id: 1,
          name: 'ply',
          position: 0,
          pane_ids: [1, 2, 3],
          focus_pane_id: 1,
          zoomed: false,
        },
        { id: 2, name: 'notes', position: 1, pane_ids: [4], focus_pane_id: 4, zoomed: false },
      ],
      active_tab_id: 1,
    };
  }

  private onData(s: Socket<{ buffer: string; welcomed: boolean }>, text: string): void {
    s.data.buffer += text;
    for (let nl = s.data.buffer.indexOf('\n'); nl >= 0; nl = s.data.buffer.indexOf('\n')) {
      const line = s.data.buffer.slice(0, nl);
      s.data.buffer = s.data.buffer.slice(nl + 1);
      this.onLine(s, line);
    }
  }

  private onLine(s: Socket<{ buffer: string; welcomed: boolean }>, line: string): void {
    let msg: ClientMsg;
    try {
      msg = JSON.parse(line) as ClientMsg;
    } catch (error) {
      log('warn', 'mock got an unparseable line', { error: String(error) });
      s.end();
      return;
    }
    if (msg.t === 'hello') {
      const v = this.options.protocolVersion ?? PROTOCOL_VERSION;
      if (msg.v !== v) {
        const err = { code: 'version_mismatch', msg: `plyd speaks protocol ${v}` };
        s.write(`${JSON.stringify({ t: 'res', id: HANDSHAKE_ID, ok: false, err })}\n`);
        s.end();
        return;
      }
      s.write(`${JSON.stringify({ t: 'welcome', v, daemon_version: '0.1.0-mock' })}\n`);
      s.data.welcomed = true;
      if (this.options.live) this.later(400, () => this.announceDemoTimes());
      return;
    }
    this.requests.push({ m: msg.m, p: msg.p });
    let reply: string;
    try {
      const refusal = this.refusals.get(msg.m);
      if (refusal) {
        this.refusals.delete(msg.m);
        throw new MethodError(refusal.code, refusal.msg);
      }
      const r = this.handle(msg.m, msg.p as never);
      reply = JSON.stringify({ t: 'res', id: msg.id, ok: true, r });
    } catch (error) {
      const err =
        error instanceof MethodError
          ? { code: error.code, msg: error.message }
          : { code: 'internal', msg: String(error) };
      reply = JSON.stringify({ t: 'res', id: msg.id, ok: false, err });
    }
    const trailer = this.trailers.get(msg.m) ?? [];
    this.trailers.delete(msg.m);
    const lines = [reply, ...trailer.map((e) => JSON.stringify({ t: 'evt', ...e }))];
    s.write(`${lines.join('\n')}\n`);
  }

  private announceDemoTimes(): void {
    for (const p of this.panes.values()) {
      if (p.status === 'running') {
        this.emit({
          e: 'pane.status',
          p: { pane_id: p.id, status: p.status, at: nowSeconds() - 252 },
        });
      }
    }
  }

  private requirePane(id: number): Pane {
    const pane = this.panes.get(id);
    if (!pane) throw new MethodError('not_found', `no pane ${id}`);
    return pane;
  }

  private handle<M extends Method>(m: M, p: Methods[M]['params']): JsonValue {
    switch (m) {
      case 'workspace.list':
        return [this.workspace];
      case 'workspace.open':
        return this.workspace;
      case 'pane.list':
        return [...this.panes.values()];
      case 'pane.create':
        return this.create(p as Methods['pane.create']['params']);
      case 'pane.close':
        return this.close(p as Methods['pane.close']['params']);
      case 'pane.answer':
        return this.answer(p as Methods['pane.answer']['params']);
      case 'pane.resume': {
        const pane = this.requirePane((p as Methods['pane.resume']['params']).pane_id);
        if (pane.status !== 'lost') throw new MethodError('invalid_state', 'the pane is not lost');
        const resumable = pane.cli !== 'shell' && pane.session_ref !== undefined;
        if (!resumable) this.panes.set(pane.id, { ...pane, cli: 'shell', title: 'zsh' });
        this.setStatus(pane.id, resumable ? 'starting' : 'idle');
        return this.requirePane(pane.id);
      }
      case 'session.list': {
        const open = [...this.panes.values()].map(
          (x): Session => ({
            pane_id: x.id,
            workspace_id: x.workspace_id,
            cli: x.cli,
            cwd: x.cwd,
            title: x.title,
            status: x.status,
            created_at: x.created_at,
          }),
        );
        const params = p as Methods['session.list']['params'];
        return params.include_closed ? [...open, ...this.closedSessions] : open;
      }
      case 'theme.set':
        this.palette = (p as Methods['theme.set']['params']).palette;
        return {};
      case 'layout.get':
        return this.layout ?? { tabs: [] };
      case 'layout.save':
        this.layout = (p as Methods['layout.save']['params']).layout;
        return {};
      case 'settings.get':
        return this.settings;
      case 'settings.set':
        this.settings = (p as Methods['settings.set']['params']).settings;
        return {};
      case 'usage.get':
        return this.usage;
      case 'daemon.shutdown':
        this.later(0, () => {
          this.emit({ e: 'daemon.stopping', p: p as Methods['daemon.shutdown']['params'] });
          this.dropClients();
        });
        return {};
      default:
        throw new MethodError('unknown_method', `unknown method ${String(m)}`);
    }
  }

  private create(params: Methods['pane.create']['params']): Pane {
    if (!isAbsolute(params.cwd)) throw new MethodError('bad_request', 'cwd must be absolute');
    if (params.worktree && params.cli !== 'claude') {
      throw new MethodError('bad_request', 'worktree is for claude panes only');
    }
    const tabId = params.tab_id ?? this.nextTabId++;
    const cwd = params.worktree
      ? join(params.cwd, '.claude', 'worktrees', params.worktree.name)
      : params.cwd;
    const pane = this.addPane({
      id: this.nextPaneId,
      tab_id: tabId,
      cli: params.cli,
      cwd,
      title: params.cli === 'shell' ? 'zsh' : params.cli,
      status: params.cli === 'shell' ? 'idle' : 'starting',
      ...(params.worktree ? { worktree_seen: params.worktree.name } : {}),
    });
    const delay = this.options.startDelayMs === undefined ? 50 : this.options.startDelayMs;
    if (params.cli !== 'shell' && delay !== null) {
      this.later(delay, () => {
        this.setStatus(pane.id, params.prompt ? 'running' : 'idle');
      });
    }
    return pane;
  }

  private close(params: Methods['pane.close']['params']): Record<string, never> {
    const pane = this.requirePane(params.pane_id);
    const alive = pane.status !== 'exited' && pane.status !== 'lost';
    if (alive && !params.kill) throw new MethodError('pane_alive', 'the pane is still running');
    this.later(0, () => {
      if (alive) {
        this.setStatus(pane.id, 'exited', undefined, 129);
        this.emit({ e: 'pane.exit', p: { pane_id: pane.id, code: 129, at: nowSeconds() } });
      }
      this.panes.delete(pane.id);
      this.emit({ e: 'pane.removed', p: { pane_id: pane.id } });
    });
    return {};
  }

  private answer(params: Methods['pane.answer']['params']): Record<string, never> {
    const pane = this.requirePane(params.pane_id);
    if (pane.status !== 'waiting_permission' && pane.status !== 'waiting_input') {
      throw new MethodError('invalid_state', 'the pane shows no dialog');
    }
    const answer: Answer = params.answer;
    this.later(0, () => {
      this.setStatus(pane.id, 'running', answer === 'no' ? 'told no' : undefined);
      if (this.options.live) this.later(1500, () => this.setStatus(pane.id, 'idle'));
    });
    return {};
  }
}

if (import.meta.main) {
  if (!process.env.PLY_HOME) {
    console.error('mock-server: set PLY_HOME so the mock never binds the real run directory');
    process.exit(2);
  }
  const socketPath = controlSocketPath();
  MockServer.start({ socketPath, scenario: 'demo', live: true });
  console.log(`mock plyd listening on ${socketPath}`);
}
