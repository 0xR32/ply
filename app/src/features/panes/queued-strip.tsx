import type { AppState } from '../../state/reducer';
import { type QueueStrip, queueStripView } from '../../state/selectors';
import { shallowEqual, useAppSelector, useDispatch } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Button } from '../../ui/button';
import { Dot } from '../../ui/chip';
import { Icon } from '../../ui/icons';
import { Text } from '../../ui/text';

function plural(n: number): string {
  return n === 1 ? '1 task' : `${n} tasks`;
}

/** The strip under a pane whose next queued task cannot go by itself (Ruling R60): the user's typing, a pause, a restart or a failure. */
export function QueuedStrip({ paneId }: { paneId: number }) {
  const strip = useAppSelector((s: AppState) => queueStripView(s, paneId), shallowEqual);
  if (!strip) return null;
  return <StripBody paneId={paneId} strip={strip} />;
}

function StripBody({ paneId, strip }: { paneId: number; strip: QueueStrip }) {
  const dispatch = useDispatch();
  const { z, accent } = useChrome();
  const resume = () => dispatch({ type: 'queue/pause', paneId, paused: false });
  const tone =
    strip.kind === 'typing'
      ? { background: accent.a10, ring: accent.a28 }
      : strip.kind === 'failed'
        ? { background: tokens.redA12, ring: tokens.redA12 }
        : { background: tokens.white[3], ring: tokens.white[8] };
  const text = (value: string) => (
    <Text color={tokens.text} variant="small" mono ellipsis>
      {value}
    </Text>
  );
  const note = (value: string) => (
    <Text color={tokens.text2} variant="small" weight={400}>
      {value}
    </Text>
  );
  const button = (testId: string, label: string, onClick: () => void, primary = false) => (
    <Button
      testId={testId}
      variant={primary ? 'primary' : 'secondary'}
      height={28}
      paddingLeft={10}
      paddingRight={10}
      radius={7}
      onClick={onClick}
    >
      <Text color={primary ? tokens.onAccent : tokens.text} weight={primary ? 600 : 500}>
        {label}
      </Text>
    </Button>
  );
  return (
    <div
      testId={`pane-${paneId}-queued`}
      style={{
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: z(8),
        marginLeft: z(6),
        marginRight: z(6),
        marginBottom: z(6),
        paddingTop: z(6),
        paddingBottom: z(6),
        paddingLeft: z(12),
        paddingRight: z(6),
        borderRadius: z(8),
        backgroundColor: tone.background,
        borderWidth: 1,
        borderColor: tone.ring,
      }}
    >
      {strip.kind === 'typing' ? (
        <>
          <Icon name="queue" size={13} color={accent.base} />
          {note('Next')}
          {text(strip.next.text)}
          <div style={{ flexGrow: 1 }} />
          {note('you typed here')}
          {button(`queue-hold-${paneId}`, 'Hold', () =>
            dispatch({ type: 'queue/pause', paneId, paused: true }),
          )}
          {button(
            `queue-send-${paneId}`,
            'Send now',
            () => dispatch({ type: 'task/send', taskId: strip.next.id }),
            true,
          )}
        </>
      ) : strip.kind === 'failed' ? (
        <>
          <Dot color={tokens.red} size={7} />
          {text(strip.failed?.text ?? strip.next.text)}
          {note(
            strip.failed?.detail?.startsWith('not submitted')
              ? 'was not submitted · queue paused'
              : 'failed · queue paused',
          )}
          <div style={{ flexGrow: 1 }} />
          {button(`queue-show-${paneId}`, 'Show queue', () =>
            dispatch({ type: 'command', id: 'task.queue' }),
          )}
          {button(`queue-resume-${paneId}`, 'Resume', resume)}
        </>
      ) : (
        <>
          <Icon name="pause" size={12} color={tokens.text2} />
          <Text color={tokens.textSoft} variant="small" weight={500}>
            {strip.kind === 'restored' ? 'Held after plyd restarted' : 'Queue paused'}
          </Text>
          {note(`· ${plural(strip.count)} · next`)}
          {text(strip.next.text)}
          <div style={{ flexGrow: 1 }} />
          {button(
            `queue-resume-${paneId}`,
            strip.kind === 'restored' ? 'Resume queue' : 'Resume',
            resume,
            strip.kind === 'restored',
          )}
        </>
      )}
    </div>
  );
}
