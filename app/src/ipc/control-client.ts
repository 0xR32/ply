import type { Socket } from 'bun';
import type { ConnectionState } from '../state/actions';
import { log } from './log';
import {
  type ErrorCode,
  type Event,
  HANDSHAKE_ID,
  type Hello,
  MAX_LINE_BYTES,
  type Method,
  type Methods,
  PROTOCOL_VERSION,
  type ServerMsg,
} from './proto.gen';

/** Why a request failed: a C1 `ErrorCode` from plyd, or `disconnected` / `timeout` from the client itself. */
export type RequestErrorCode = ErrorCode | 'disconnected' | 'timeout';

/** A failed C1 request; match on `code`, never on `message` (which is shown to the user as is). */
export class RequestError extends Error {
  constructor(
    readonly code: RequestErrorCode,
    message: string,
  ) {
    super(message);
    this.name = 'RequestError';
  }
}

/** The C1 client the app uses (spec 4.1); one connection, requests only while `state.kind` is `connected`. */
export interface ControlClient {
  readonly state: ConnectionState;
  /** Connects, and keeps reconnecting with backoff until `stop()`. */
  start(): void;
  /** Closes the connection and rejects every pending request with `disconnected`. */
  stop(): void;
  /** Sends one request; rejects with `RequestError` on an error response, a disconnect or the request timeout. */
  request<M extends Method>(method: M, params: Methods[M]['params']): Promise<Methods[M]['result']>;
  /** Receives every daemon event after the handshake, in wire order. */
  onEvent(listener: (event: Event) => void): () => void;
  /** Receives every connection state change, starting with the next one. */
  onState(listener: (state: ConnectionState) => void): () => void;
}

/** Where to connect and how to recover; `startDaemon` runs at most once per outage, before the first retry. */
export interface ControlClientOptions {
  socketPath: string;
  appVersion: string;
  startDaemon?: () => Promise<void>;
  requestTimeoutMs?: number;
  handshakeTimeoutMs?: number;
  initialBackoffMs?: number;
  maxBackoffMs?: number;
}

interface Pending {
  resolve: (value: unknown) => void;
  reject: (error: RequestError) => void;
  timer: ReturnType<typeof setTimeout>;
  method: Method;
}

const NEWLINE = 10;

class UnixControlClient implements ControlClient {
  state: ConnectionState = { kind: 'connecting' };
  private socket: Socket<undefined> | null = null;
  private stopped = true;
  private welcomed = false;
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private partial: Uint8Array[] = [];
  private partialBytes = 0;
  private outbox: Uint8Array = new Uint8Array(0);
  private backoffMs: number;
  private startedThisOutage = false;
  private startError: string | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private handshakeTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly eventListeners = new Set<(event: Event) => void>();
  private readonly stateListeners = new Set<(state: ConnectionState) => void>();
  private readonly encoder = new TextEncoder();
  private readonly decoder = new TextDecoder();
  private readonly requestTimeoutMs: number;
  private readonly handshakeTimeoutMs: number;
  private readonly initialBackoffMs: number;
  private readonly maxBackoffMs: number;

  constructor(private readonly options: ControlClientOptions) {
    this.requestTimeoutMs = options.requestTimeoutMs ?? 10_000;
    this.handshakeTimeoutMs = options.handshakeTimeoutMs ?? 5_000;
    this.initialBackoffMs = options.initialBackoffMs ?? 100;
    this.maxBackoffMs = options.maxBackoffMs ?? 2_000;
    this.backoffMs = this.initialBackoffMs;
  }

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    void this.connect();
  }

  stop(): void {
    this.stopped = true;
    this.clearTimers();
    const socket = this.socket;
    this.socket = null;
    socket?.end();
    this.dropConnection('client stopped');
  }

  request<M extends Method>(
    method: M,
    params: Methods[M]['params'],
  ): Promise<Methods[M]['result']> {
    if (this.state.kind !== 'connected' || !this.socket) {
      return Promise.reject(new RequestError('disconnected', 'plyd is not connected'));
    }
    const id = this.nextId++;
    const line = `${JSON.stringify({ t: 'req', id, m: method, p: params })}\n`;
    const bytes = this.encoder.encode(line);
    if (bytes.length > MAX_LINE_BYTES) {
      return Promise.reject(new RequestError('bad_request', `${method} is larger than 1 MiB`));
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        log('warn', 'control request timed out', { method, id });
        reject(new RequestError('timeout', `${method} got no answer from plyd`));
      }, this.requestTimeoutMs);
      this.pending.set(id, {
        resolve: resolve as (value: unknown) => void,
        reject,
        timer,
        method,
      });
      this.send(bytes);
    });
  }

  onEvent(listener: (event: Event) => void): () => void {
    this.eventListeners.add(listener);
    return () => this.eventListeners.delete(listener);
  }

  onState(listener: (state: ConnectionState) => void): () => void {
    this.stateListeners.add(listener);
    return () => this.stateListeners.delete(listener);
  }

  private setState(state: ConnectionState): void {
    this.state = state;
    for (const listener of this.stateListeners) {
      try {
        listener(state);
      } catch (error) {
        log('error', 'connection state listener failed', { error: String(error) });
      }
    }
  }

  private async connect(): Promise<void> {
    if (this.stopped) return;
    let socket: Socket<undefined>;
    try {
      socket = await Bun.connect({
        unix: this.options.socketPath,
        socket: {
          data: (s, chunk) => this.onData(s, chunk),
          close: (s) => this.onClose(s),
          drain: () => this.flushOutbox(),
          error: (_s, error) => log('warn', 'control socket error', { error: String(error) }),
        },
      });
    } catch (error) {
      await this.onConnectFailed(error);
      return;
    }
    if (this.stopped) {
      socket.end();
      return;
    }
    this.socket = socket;
    this.welcomed = false;
    const hello: Hello & { t: 'hello' } = {
      t: 'hello',
      v: PROTOCOL_VERSION,
      client: 'ply-app',
      app_version: this.options.appVersion,
    };
    this.send(this.encoder.encode(`${JSON.stringify(hello)}\n`));
    this.handshakeTimer = setTimeout(() => {
      log('warn', 'plyd sent no welcome', { timeout_ms: this.handshakeTimeoutMs });
      socket.end();
    }, this.handshakeTimeoutMs);
  }

  private async onConnectFailed(error: unknown): Promise<void> {
    const code = (error as NodeJS.ErrnoException | undefined)?.code;
    const firstOfOutage = this.state.kind !== 'down';
    let reason =
      code === 'ENOENT' || code === 'ECONNREFUSED'
        ? 'plyd is not running'
        : `cannot reach plyd: ${String(error)}`;
    if (firstOfOutage) {
      log('warn', 'control connect failed', { code, socket: this.options.socketPath });
    }
    const start = this.options.startDaemon;
    const again = this.startedThisOutage;
    // A start can find the old plyd still holding its lock (a Restart), so a long outage starts plyd again at every capped retry.
    if (start && (!again || this.backoffMs >= this.maxBackoffMs)) {
      this.startedThisOutage = true;
      if (again) log('debug', 'plyd is still not up; starting it again');
      this.setState({ kind: 'down', reason, retryInMs: 0, starting: true });
      try {
        await start();
        this.startError = null;
      } catch (startError) {
        const why = startError instanceof Error ? startError.message : String(startError);
        log(why === this.startError ? 'debug' : 'error', 'starting plyd failed', { error: why });
        this.startError = why;
      }
    }
    if (this.startError) reason = `${reason} and could not be started: ${this.startError}`;
    this.scheduleRetry(reason);
  }

  private scheduleRetry(reason: string): void {
    if (this.stopped) return;
    const delay = this.backoffMs;
    this.backoffMs = Math.min(this.backoffMs * 2, this.maxBackoffMs);
    if (this.state.kind !== 'incompatible') {
      this.setState({ kind: 'down', reason, retryInMs: delay, starting: false });
    }
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      void this.connect();
    }, delay);
  }

  private send(bytes: Uint8Array): void {
    if (this.outbox.length === 0) {
      this.outbox = bytes;
    } else {
      const merged = new Uint8Array(this.outbox.length + bytes.length);
      merged.set(this.outbox);
      merged.set(bytes, this.outbox.length);
      this.outbox = merged;
    }
    this.flushOutbox();
  }

  private flushOutbox(): void {
    if (!this.socket || this.outbox.length === 0) return;
    const written = this.socket.write(this.outbox);
    this.outbox = written >= this.outbox.length ? new Uint8Array(0) : this.outbox.subarray(written);
  }

  private onData(socket: Socket<undefined>, chunk: Uint8Array): void {
    if (socket !== this.socket) return;
    let start = 0;
    for (let nl = chunk.indexOf(NEWLINE); nl >= 0; nl = chunk.indexOf(NEWLINE, start)) {
      const piece = chunk.subarray(start, nl);
      start = nl + 1;
      const line = this.partialBytes === 0 ? piece : this.joinPartial(piece);
      if (line.length + 1 > MAX_LINE_BYTES) {
        this.protocolError('a line from plyd exceeds 1 MiB');
        return;
      }
      this.onLine(this.decoder.decode(line));
      if (socket !== this.socket) return;
    }
    if (start < chunk.length) {
      const rest = chunk.slice(start);
      this.partial.push(rest);
      this.partialBytes += rest.length;
      if (this.partialBytes + 1 > MAX_LINE_BYTES)
        this.protocolError('a line from plyd exceeds 1 MiB');
    }
  }

  private joinPartial(tail: Uint8Array): Uint8Array {
    const line = new Uint8Array(this.partialBytes + tail.length);
    let at = 0;
    for (const part of this.partial) {
      line.set(part, at);
      at += part.length;
    }
    line.set(tail, at);
    this.partial = [];
    this.partialBytes = 0;
    return line;
  }

  private protocolError(why: string): void {
    log('error', 'control protocol error', { why });
    this.socket?.end();
  }

  private onLine(text: string): void {
    let msg: ServerMsg;
    try {
      msg = JSON.parse(text) as ServerMsg;
    } catch (error) {
      log('error', 'unparseable line from plyd', { error: String(error), bytes: text.length });
      return;
    }
    switch (msg.t) {
      case 'welcome':
        this.onWelcome(msg.v, msg.daemon_version);
        return;
      case 'res':
        this.onResponse(msg);
        return;
      case 'evt':
        if (this.welcomed) this.emit(msg);
        return;
      default:
        log('warn', 'unknown message from plyd', { t: String((msg as { t?: unknown }).t) });
    }
  }

  private onWelcome(version: number, daemonVersion: string): void {
    if (this.handshakeTimer) clearTimeout(this.handshakeTimer);
    this.handshakeTimer = null;
    if (version !== PROTOCOL_VERSION) {
      this.incompatible(`plyd speaks protocol ${version}, the app ${PROTOCOL_VERSION}`);
      return;
    }
    this.welcomed = true;
    this.backoffMs = this.initialBackoffMs;
    this.startedThisOutage = false;
    this.startError = null;
    log('info', 'connected to plyd', { daemon_version: daemonVersion });
    this.setState({ kind: 'connected', daemonVersion });
  }

  private onResponse(msg: Extract<ServerMsg, { t: 'res' }>): void {
    if (msg.id === HANDSHAKE_ID && !msg.ok) {
      this.incompatible(msg.err.msg);
      return;
    }
    const pending = this.pending.get(msg.id);
    if (!pending) {
      log('warn', 'response for an unknown request', { id: msg.id });
      return;
    }
    this.pending.delete(msg.id);
    clearTimeout(pending.timer);
    if (msg.ok) {
      pending.resolve(msg.r);
    } else {
      log('warn', 'control request failed', {
        method: pending.method,
        code: msg.err.code,
        msg: msg.err.msg,
      });
      pending.reject(new RequestError(msg.err.code, msg.err.msg));
    }
  }

  private emit(msg: Extract<ServerMsg, { t: 'evt' }>): void {
    const event = { e: msg.e, p: msg.p } as Event;
    for (const listener of this.eventListeners) {
      try {
        listener(event);
      } catch (error) {
        log('error', 'event listener failed', { event: event.e, error: String(error) });
      }
    }
  }

  private incompatible(reason: string): void {
    log('error', 'plyd is incompatible', { reason });
    this.setState({ kind: 'incompatible', reason });
    this.socket?.end();
  }

  private onClose(socket: Socket<undefined>): void {
    if (socket !== this.socket) return;
    this.socket = null;
    const wasConnected = this.state.kind === 'connected';
    this.dropConnection('connection to plyd closed');
    if (this.stopped) return;
    if (wasConnected) log('warn', 'connection to plyd lost');
    this.scheduleRetry(wasConnected ? 'connection to plyd lost' : 'plyd closed the connection');
  }

  private dropConnection(why: string): void {
    if (this.handshakeTimer) clearTimeout(this.handshakeTimer);
    this.handshakeTimer = null;
    this.welcomed = false;
    this.partial = [];
    this.partialBytes = 0;
    this.outbox = new Uint8Array(0);
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer);
      pending.reject(new RequestError('disconnected', why));
      this.pending.delete(id);
    }
  }

  private clearTimers(): void {
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    if (this.handshakeTimer) clearTimeout(this.handshakeTimer);
    this.handshakeTimer = null;
  }
}

/** A client for plyd's control socket; call `start()` to connect. */
export function createControlClient(options: ControlClientOptions): ControlClient {
  return new UnixControlClient(options);
}
