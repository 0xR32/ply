import { createContext } from 'react';
import {
  connectPane,
  type DataConnection,
  type DataConnectionHandlers,
  type DataConnectionOptions,
  type DataConnectionState,
  type DecodeStats,
  dataSocketPath,
} from './data-client';
import { terminalLog } from './log';

/** The side effects a terminal view needs: its C2 connection, the system clipboard and opening links; tests pass fakes. */
export interface TerminalHost {
  /** C2 socket path each connection opens. */
  readonly socketPath: string;
  connect(options: DataConnectionOptions, handlers: DataConnectionHandlers): DataConnection;
  /** The clipboard's text, empty when it holds none. */
  readClipboard(): Promise<string>;
  writeClipboard(text: string): Promise<void>;
  /** Opens an http(s) URL in the default browser; rejects any other scheme. */
  openUrl(url: string): Promise<void>;
}

async function run(argv: string[], input?: string): Promise<string> {
  const child = Bun.spawn(argv, {
    stdin: input === undefined ? 'ignore' : new TextEncoder().encode(input),
    stdout: 'pipe',
    stderr: 'ignore',
  });
  const [code, out] = await Promise.all([child.exited, new Response(child.stdout).text()]);
  if (code !== 0) throw new Error(`${argv[0]} exited with ${code}`);
  return out;
}

/** The real host: plyd's data socket, the macOS pasteboard through `pbpaste`/`pbcopy` and links through `open -u` (GPUIX 0.10.0 gives JavaScript neither a clipboard nor a URL API). */
export function systemTerminalHost(socketPath: string = dataSocketPath()): TerminalHost {
  return {
    socketPath,
    connect: connectPane,
    readClipboard: () => run(['pbpaste']),
    writeClipboard: async (text) => {
      await run(['pbcopy'], text);
    },
    openUrl: async (url) => {
      // The URL is program output; any other scheme could launch an app.
      if (!/^https?:\/\//.test(url)) throw new Error('only http and https links open');
      await run(['open', '-u', url]);
    },
  };
}

const INERT_STATS: DecodeStats = { frames: 0, batches: 0, lastMs: 0, maxMs: 0, totalMs: 0 };

/** A host that never connects and has an empty clipboard: the default under `bun test`, so no test reaches the user's plyd or pasteboard. */
export const inertTerminalHost: TerminalHost = {
  socketPath: '',
  connect(): DataConnection {
    const state: DataConnectionState = { kind: 'connecting' };
    return { state, stats: INERT_STATS, send: () => false, resize: () => {}, close: () => {} };
  },
  readClipboard: async () => '',
  writeClipboard: async () => {},
  openUrl: async () => {},
};

let fallback: TerminalHost | null = null;

/** The host a view uses when no `TerminalHostContext` provider is above it. */
export function defaultTerminalHost(): TerminalHost {
  if (!fallback) {
    fallback = process.env.NODE_ENV === 'test' ? inertTerminalHost : systemTerminalHost();
    terminalLog('debug', 'terminal host chosen', { socket: fallback.socketPath });
  }
  return fallback;
}

/** Overrides the terminal host for a subtree (tests, the dev demo). */
export const TerminalHostContext = createContext<TerminalHost | null>(null);
