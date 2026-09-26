import { homedir } from 'node:os';
import { render } from '@gpuix/react';
import { version } from '../package.json';
import { App } from './app/App';
import { BindFolderPicker, createFolderPicker } from './app/folder-picker';
import { FrameStats } from './app/frame-stats';
import { StartZoomed } from './app/start-zoomed';
import { createControlClient } from './ipc/control-client';
import { createDaemonStarter } from './ipc/daemon-launcher';
import { log } from './ipc/log';
import { geistAvailable, shellName } from './ipc/os';
import { controlSocketPath } from './ipc/paths';
import { windowKeyListeners } from './keymap/dispatcher';
import { startEffects } from './state/effects';
import { initialState } from './state/reducer';
import { createStore } from './state/store';
import { setTerminalLogSink } from './terminal/log';
import { tokens } from './theme/tokens';

declare global {
  var plyStopEffects: (() => void) | undefined;
}

// bun --hot re-runs this file on save; the previous run's socket and timers must stop first.
globalThis.plyStopEffects?.();
setTerminalLogSink(log);

const store = createStore(
  initialState({ home: homedir(), shellName: shellName(), geistAvailable: geistAvailable() }),
);
const client = createControlClient({
  socketPath: controlSocketPath(),
  appVersion: version,
  startDaemon: createDaemonStarter(),
});
const folderPicker = createFolderPicker();
globalThis.plyStopEffects = startEffects(store, {
  client,
  promptForDirectory: () => folderPicker.prompt(),
});

// PLY_WINDOW_FOCUS=0 opens the window without stealing focus, for agents and scripted runs.
const takeFocus = process.env.PLY_WINDOW_FOCUS !== '0';
const frameStats = process.env.PLY_TERMINAL_STATS === '1';

render(
  <>
    <App store={store} />
    <StartZoomed />
    <BindFolderPicker picker={folderPicker} />
    {frameStats ? <FrameStats /> : null}
  </>,
  {
    title: 'ply',
    appName: 'ply',
    width: 1280,
    height: 800,
    minWidth: 720,
    minHeight: 480,
    titlebarTransparent: true,
    // Liquid glass: macOS blurs what is behind the window, and the translucent ground (tokens.glass) lets it show.
    windowBackground: 'blurred',
    trafficLightX: tokens.layout.trafficLightX,
    trafficLightY: tokens.layout.trafficLightY,
    focus: takeFocus,
    debugFrameOverlay: frameStats ? 'full' : 'hidden',
    ...windowKeyListeners(store),
  },
);
