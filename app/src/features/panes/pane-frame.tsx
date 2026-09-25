import { useCallback, useMemo } from 'react';
import { abbreviateHome, isLost, needsYou, selectActiveTab } from '../../state/selectors';
import { useAppSelector, useDispatch } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { terminalThemeFor, tokens } from '../../theme/tokens';
import { LostStrip } from './lost-strip';
import { PaneHeader } from './pane-header';
import { TerminalSlot } from './terminal-slot';
import { WaitingStrip } from './waiting-strip';

/** One pane card: header, terminal body, and the waiting strip (needs you) or the resume strip (lost). */
export function PaneFrame({ paneId, position }: { paneId: number; position: number }) {
  const dispatch = useDispatch();
  const { z, accent, fonts, type } = useChrome();
  const pane = useAppSelector((s) => s.panes[paneId]);
  const focused = useAppSelector((s) => selectActiveTab(s)?.focus_pane_id === paneId);
  const overlayOpen = useAppSelector((s) => s.overlay !== null);
  const accentName = useAppSelector((s) => s.settings.accent);
  const home = useAppSelector((s) => s.env.home);
  const theme = useMemo(() => terminalThemeFor(accentName), [accentName]);
  const activate = useCallback(() => dispatch({ type: 'pane/focus', paneId }), [dispatch, paneId]);
  const onTitle = useCallback(
    (title: string) => dispatch({ type: 'pane/title', paneId, title }),
    [dispatch, paneId],
  );
  if (!pane) return null;
  const waiting = needsYou(pane);
  const ring = focused ? accent.a55 : waiting ? tokens.amberA[35] : tokens.hairline;
  const glow = focused ? accent.a35 : waiting ? tokens.amberA[25] : null;
  return (
    <div
      testId={`pane-${paneId}`}
      style={{
        flexGrow: 1,
        flexBasis: 0,
        minWidth: 0,
        minHeight: 0,
        display: 'flex',
        flexDirection: 'column',
        overflow: 'hidden',
        borderRadius: z(tokens.radius.pane),
        backgroundColor: focused ? tokens.paneFocus : tokens.pane,
        borderWidth: 1,
        borderColor: ring,
        ...(glow
          ? {
              boxShadow: {
                offsetX: 0,
                offsetY: z(18),
                blurRadius: z(48),
                spreadRadius: focused ? -z(18) : -z(22),
                color: glow,
              },
            }
          : {}),
      }}
    >
      <PaneHeader pane={pane} position={position} focused={focused} onActivate={activate} />
      <div
        style={{
          flexGrow: 1,
          minHeight: 0,
          display: 'flex',
          flexDirection: 'column',
          overflow: 'hidden',
          marginLeft: z(6),
          marginRight: z(6),
          marginBottom: z(6),
          paddingTop: z(12),
          paddingBottom: z(12),
          paddingLeft: z(14),
          paddingRight: z(14),
          borderRadius: z(tokens.radius.term),
          backgroundColor: tokens.term,
          borderWidth: 1,
          borderColor: tokens.white[4],
        }}
      >
        <TerminalSlot
          paneId={paneId}
          focused={focused && !overlayOpen}
          theme={theme}
          fontFamily={fonts.mono}
          fontSize={type.terminal.fontSize}
          onTitle={onTitle}
          onFocus={activate}
          placeholder={`${pane.cli} · ${abbreviateHome(pane.cwd, home)}`}
        />
      </div>
      {waiting ? <WaitingStrip pane={pane} /> : null}
      {isLost(pane) ? <LostStrip pane={pane} /> : null}
    </div>
  );
}
