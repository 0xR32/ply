// Dev demo, needs a running plyd: PLY_DEMO_PANES (1–6) shell panes, PLY_DEMO_INPUT typed into each, PLY_TERMINAL_STATS=1 logs timings.

import { homedir } from 'node:os';
import { render, useGpuix } from '@gpuix/react';
import { useEffect, useState } from 'react';
import { version } from '../../package.json';
import { TerminalView } from '../features/panes/terminal-view';
import { createControlClient } from '../ipc/control-client';
import { log } from '../ipc/log';
import { controlSocketPath } from '../ipc/paths';
import type { Pane } from '../ipc/proto.gen';
import { initialState } from '../state/reducer';
import { createStore, StoreContext } from '../state/store';
import { connectPane, dataSocketPath } from '../terminal/data-client';
import { setTerminalLogSink } from '../terminal/log';
import { terminalStats } from '../terminal/session';
import { terminalTheme, tokens } from '../theme/tokens';

declare global {
  var plyStopDemo: (() => void) | undefined;
}

globalThis.plyStopDemo?.();
setTerminalLogSink((level, message, fields) => log(level, message, fields));

const count = Math.min(6, Math.max(1, Number(process.env.PLY_DEMO_PANES ?? '1') || 1));
const input = process.env.PLY_DEMO_INPUT;
const home = homedir();
const store = createStore(initialState({ home, shellName: 'zsh', geistAvailable: false }));
const client = createControlClient({ socketPath: controlSocketPath(), appVersion: version });
const panes: Pane[] = [];
const listeners = new Set<() => void>();

client.onState(async (state) => {
  if (state.kind !== 'connected' || panes.length > 0) return;
  try {
    const workspace = await client.request('workspace.open', { path: home });
    for (let i = 0; i < count; i++) {
      panes.push(
        await client.request('pane.create', {
          workspace_id: workspace.id,
          cli: 'shell',
          cwd: home,
        }),
      );
    }
    for (const l of listeners) l();
    if (input) for (const p of panes) typeInto(p.id, input);
  } catch (error) {
    log('error', 'terminal demo could not create its panes', { error: String(error) });
  }
});
client.start();

function typeInto(paneId: number, text: string): void {
  const conn = connectPane(
    {
      socketPath: dataSocketPath(),
      paneId,
      size: { cols: 80, rows: 24, cellWidthPx: 8, cellHeightPx: 16 },
    },
    {
      onFrames: () => {},
      onState: (s) => {
        if (s.kind !== 'attached') return;
        conn.send({ kind: 'inputRaw', bytes: new TextEncoder().encode(`${text}\r`) });
        setTimeout(() => conn.close(), 100);
      },
    },
  );
}

function Demo() {
  const [list, setList] = useState(panes);
  const { renderer } = useGpuix();
  useEffect(() => {
    const update = () => setList([...panes]);
    listeners.add(update);
    return () => {
      listeners.delete(update);
    };
  }, []);
  useEffect(() => {
    if (process.env.PLY_TERMINAL_STATS !== '1') return;
    const t = setInterval(() => {
      const frame = renderer?.getDebugFrameOverlayStats?.();
      log('info', 'terminal demo timing', {
        frame_p99_ms: frame?.p99Ms,
        frame_max_ms: frame?.maxMs,
        frames: frame?.frames,
        panes: JSON.stringify(terminalStats()),
      });
    }, 2_000);
    return () => clearInterval(t);
  }, [renderer]);
  return (
    <div
      style={{
        display: 'flex',
        flexWrap: 'wrap',
        width: '100%',
        height: '100%',
        gap: 8,
        padding: 8,
        backgroundColor: tokens.ground,
      }}
    >
      {list.length === 0 ? (
        <text style={{ color: tokens.text3 }}>{`Waiting for plyd at ${controlSocketPath()}`}</text>
      ) : (
        list.map((p) => (
          <div
            key={p.id}
            style={{
              display: 'flex',
              flexDirection: 'column',
              flexGrow: 1,
              flexBasis: list.length > 1 ? 560 : 0,
              minWidth: 0,
              minHeight: 0,
              height: list.length > 2 ? '32%' : list.length > 1 ? '48%' : '100%',
              padding: 10,
              borderRadius: tokens.radius.term,
              backgroundColor: tokens.term,
            }}
          >
            <TerminalView
              paneId={p.id}
              focused={p.id === list[0]?.id}
              theme={terminalTheme}
              fontFamily="Menlo"
              fontSize={tokens.type.terminal.fontSize}
              placeholder={`shell · pane ${p.id}`}
            />
          </div>
        ))
      )}
    </div>
  );
}

globalThis.plyStopDemo = () => client.stop();

render(
  <StoreContext.Provider value={store}>
    <Demo />
  </StoreContext.Provider>,
  {
    title: 'ply terminal demo',
    width: 1280,
    height: 800,
    focus: process.env.PLY_WINDOW_FOCUS !== '0',
    debugFrameOverlay: process.env.PLY_TERMINAL_STATS === '1' ? 'full' : 'hidden',
  },
);
