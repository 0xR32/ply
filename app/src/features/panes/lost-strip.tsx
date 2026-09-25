import type { PaneState } from '../../state/reducer';
import { resumeHow } from '../../state/selectors';
import { useDispatch } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Button } from '../../ui/button';
import { Dot } from '../../ui/chip';
import { Text } from '../../ui/text';

/** The strip under a lost pane (its process ended with plyd); its button resumes the session through `pane.resume`. */
export function LostStrip({ pane }: { pane: PaneState }) {
  const dispatch = useDispatch();
  const { z } = useChrome();
  return (
    <div
      testId={`pane-${pane.id}-lost`}
      style={{
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: z(8),
        marginLeft: z(6),
        marginRight: z(6),
        marginBottom: z(6),
        paddingTop: z(8),
        paddingBottom: z(8),
        paddingLeft: z(12),
        paddingRight: z(8),
        borderRadius: z(8),
        backgroundColor: tokens.white[4],
        borderWidth: 1,
        borderColor: tokens.white[10],
      }}
    >
      <Dot color={tokens.text3} size={7} />
      <Text color={tokens.text} ellipsis>
        {`${pane.cli} stopped when plyd did`}
      </Text>
      <div style={{ flexGrow: 1 }} />
      <Text color={tokens.text2} variant="small" weight={400}>
        {`resumes with ${resumeHow(pane)}`}
      </Text>
      <Button
        testId={`resume-${pane.id}`}
        label="Resume the session"
        variant="primary"
        height={28}
        paddingLeft={12}
        paddingRight={12}
        radius={7}
        onClick={() => dispatch({ type: 'pane/resume', paneId: pane.id })}
      >
        <Text color={tokens.onAccent} weight={500}>
          Resume
        </Text>
      </Button>
    </div>
  );
}
