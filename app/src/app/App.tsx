import { useMemo } from 'react';
import { PaneGrid } from '../features/panes/pane-grid';
import { StatusBar } from '../features/statusbar/statusbar';
import { UsageView } from '../features/usage/usage-view';
import { type Store, StoreContext, useAppSelector } from '../state/store';
import { ChromeThemeContext, chromeFonts, createChromeTheme } from '../theme/chrome';
import { tokens } from '../theme/tokens';
import { Overlays } from './overlays';
import { TopBar } from './top-bar';

function Shell() {
  const accent = useAppSelector((s) => s.settings.accent);
  const fontSize = useAppSelector((s) => s.settings.font_size);
  const reducedMotion = useAppSelector((s) => s.reducedMotion);
  const geist = useAppSelector((s) => s.env.geistAvailable);
  const theme = useMemo(
    () => createChromeTheme(accent, fontSize, chromeFonts(geist), reducedMotion),
    [accent, fontSize, geist, reducedMotion],
  );
  return (
    <ChromeThemeContext.Provider value={theme}>
      <div
        testId="app-root"
        style={{
          position: 'relative',
          display: 'flex',
          flexDirection: 'column',
          width: '100%',
          height: '100%',
          backgroundColor: tokens.ground,
          fontFamily: theme.fonts.ui,
        }}
      >
        <div
          style={{
            position: 'absolute',
            top: 0,
            left: 0,
            right: 0,
            height: tokens.layout.groundGlowHeight,
            pointerEvents: 'none',
            background: {
              type: 'linear-gradient',
              angle: 180,
              stops: [
                { color: theme.accent.groundGlow, position: 0 },
                { color: theme.accent.groundGlowEnd, position: 1 },
              ],
            },
          }}
        />
        <TopBar />
        <PaneGrid />
        <StatusBar />
        <Overlays />
        <UsageView />
      </div>
    </ChromeThemeContext.Provider>
  );
}

/** Composition root: provides the store and the chrome theme, and lays out bar, grid, footer and overlays. */
export function App({ store }: { store: Store }) {
  return (
    <StoreContext.Provider value={store}>
      <Shell />
    </StoreContext.Provider>
  );
}
