import { render } from '@gpuix/react';
import { App } from './app/App';
import { tokens } from './theme/tokens';

// PLY_WINDOW_FOCUS=0 opens the window without stealing focus, for agents and scripted runs.
const takeFocus = process.env.PLY_WINDOW_FOCUS !== '0';

render(<App />, {
  title: 'ply',
  appName: 'ply',
  width: 1280,
  height: 800,
  minWidth: 720,
  minHeight: 480,
  titlebarTransparent: true,
  trafficLightX: tokens.layout.trafficLightX,
  trafficLightY: tokens.layout.trafficLightY,
  focus: takeFocus,
});
