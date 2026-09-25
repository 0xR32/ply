import type { EventPayload } from '@gpuix/react';
import { abbreviateHome, panePlace, paneTitle } from '../../state/selectors';
import { useAppSelector, useDispatch } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Button } from '../../ui/button';
import { Kbd } from '../../ui/kbd';
import { Card } from '../../ui/overlay-card';
import { Text } from '../../ui/text';

/** Asks before ⌘⇧W stops a live pane (Ruling R7: close with kill sends SIGHUP, then SIGKILL after 2 s). */
export function CloseConfirm({ paneId }: { paneId: number }) {
  const dispatch = useDispatch();
  const { z } = useChrome();
  const pane = useAppSelector((s) => s.panes[paneId]);
  const place = useAppSelector((s) => panePlace(s, paneId));
  const home = useAppSelector((s) => s.env.home);
  if (!pane) return null;
  const cancel = () => dispatch({ type: 'overlay/close' });
  const confirm = () => dispatch({ type: 'pane/closeConfirmed', paneId });
  const onKeyDown = (event: EventPayload) => {
    if (event.modifiers?.cmd) return;
    if (event.key === 'enter') confirm();
    else if (event.key === 'escape') cancel();
  };
  return (
    <Card width={460} testId="close-confirm">
      <div
        tabIndex={0}
        autoFocus
        onKeyDown={onKeyDown}
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: z(8),
          paddingTop: z(18),
          paddingBottom: z(16),
          paddingLeft: z(20),
          paddingRight: z(16),
        }}
      >
        <Text color={tokens.text} variant="title">
          {`Close pane ${place?.pane ?? ''} · ${paneTitle(pane)}?`}
        </Text>
        <Text color={tokens.text3} variant="small" weight={400}>
          {`${pane.cli} is still running in ${abbreviateHome(pane.cwd, home)}. Closing stops it.`}
        </Text>
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: z(8), marginTop: z(10) }}>
          <Button testId="close-cancel" onClick={cancel} gap={10} paddingRight={8}>
            <Text color={tokens.text}>Cancel</Text>
            <Kbd label="esc" color={tokens.text2} />
          </Button>
          <Button testId="close-stop" variant="danger" onClick={confirm} gap={10} paddingRight={8}>
            <Text color={tokens.ground} weight={600}>
              Close and stop
            </Text>
            <Kbd label="⏎" color={tokens.ground} background={tokens.onAccentKey} ring={null} />
          </Button>
        </div>
      </div>
    </Card>
  );
}
