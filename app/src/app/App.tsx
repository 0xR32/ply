import { fontFamily, tokens } from '../theme/tokens';

/** Composition root: the window's ground, its accent light and the top bar with the wordmark. */
export function App() {
  return (
    <div
      testId="app-root"
      style={{
        position: 'relative',
        display: 'flex',
        flexDirection: 'column',
        width: '100%',
        height: '100%',
        backgroundColor: tokens.ground,
        fontFamily: fontFamily.ui,
      }}
    >
      <div
        style={{
          position: 'absolute',
          top: 0,
          left: 0,
          right: 0,
          height: tokens.layout.groundGlowHeight,
          background: {
            type: 'linear-gradient',
            angle: 180,
            stops: [
              { color: tokens.groundGlow, position: 0 },
              { color: tokens.groundGlowEnd, position: 1 },
            ],
          },
        }}
      />
      <TopBar />
    </div>
  );
}

function TopBar() {
  return (
    <div
      style={{
        height: tokens.layout.headerHeight,
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        paddingLeft: tokens.layout.headerPaddingLeft,
        paddingRight: tokens.layout.headerPaddingRight,
      }}
    >
      <Wordmark />
    </div>
  );
}

function Wordmark() {
  return (
    <div testId="wordmark" style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
      <div
        style={{
          width: 8,
          height: 8,
          borderRadius: 2,
          backgroundColor: tokens.accent,
          boxShadow: {
            offsetX: 0,
            offsetY: 0,
            blurRadius: 12,
            spreadRadius: 0,
            color: tokens.accentGlow,
          },
        }}
      />
      <text
        style={{
          color: tokens.text,
          fontFamily: fontFamily.ui,
          fontSize: tokens.type.wordmark.fontSize,
          fontWeight: tokens.type.wordmark.fontWeight,
          lineHeight: tokens.type.wordmark.lineHeight,
        }}
      >
        ply
      </text>
    </div>
  );
}
