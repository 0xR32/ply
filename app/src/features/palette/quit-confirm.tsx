import type { EventPayload } from '@gpuix/react';
import { isAlive } from '../../state/selectors';
import { useAppSelector, useDispatch } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Button } from '../../ui/button';
import { Kbd } from '../../ui/kbd';
import { Card } from '../../ui/overlay-card';
import { Text } from '../../ui/text';

/** Asks before "Quit ply and stop sessions" stops plyd with every pane's process and quits the app (Ruling R53). */
export function QuitConfirm() {
  const dispatch = useDispatch();
  const { z } = useChrome();
  const live = useAppSelector((s) => Object.values(s.panes).filter(isAlive).length);
  const cancel = () => dispatch({ type: 'overlay/close' });
  const confirm = () => dispatch({ type: 'daemon/quit' });
  const onKeyDown = (event: EventPayload) => {
    if (event.modifiers?.cmd) return;
    if (event.key === 'enter') confirm();
    else if (event.key === 'escape') cancel();
  };
  const running =
    live === 0
      ? 'No pane runs a process.'
      : `${live} ${live === 1 ? 'pane runs a process' : 'panes run a process'}; each one is stopped.`;
  return (
    <Card width={460} testId="quit-confirm">
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
          Quit ply and stop every session?
        </Text>
        <Text color={tokens.text3} variant="small" weight={400}>
          {`plyd stops and ply quits. ${running} The next start of ply starts plyd again.`}
        </Text>
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: z(8), marginTop: z(10) }}>
          <Button testId="quit-cancel" onClick={cancel} gap={10} paddingRight={8}>
            <Text color={tokens.text}>Cancel</Text>
            <Kbd label="esc" color={tokens.text2} />
          </Button>
          <Button testId="quit-stop" variant="danger" onClick={confirm} gap={10} paddingRight={8}>
            <Text color={tokens.ground} weight={600}>
              Quit and stop
            </Text>
            <Kbd label="⏎" color={tokens.ground} background={tokens.onAccentKey} ring={null} />
          </Button>
        </div>
      </div>
    </Card>
  );
}
