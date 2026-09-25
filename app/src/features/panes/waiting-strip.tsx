import type { Choice } from '../../state/actions';
import type { PaneState } from '../../state/reducer';
import { useDispatch } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Button } from '../../ui/button';
import { Dot } from '../../ui/chip';
import { Kbd } from '../../ui/kbd';
import { Text } from '../../ui/text';

const ANSWERS: readonly { choice: Choice; label: string }[] = [
  { choice: 1, label: 'Yes' },
  { choice: 2, label: 'Always' },
  { choice: 3, label: 'No' },
];

function waitText(pane: PaneState): string {
  if (pane.detail) return pane.detail;
  return pane.status === 'waiting_permission'
    ? `${pane.cli} asks for permission`
    : `${pane.cli} is waiting for your answer`;
}

/** The needs-you strip under a waiting pane; its buttons send the dialog's digits through `pane.answer` (7.4). */
export function WaitingStrip({ pane }: { pane: PaneState }) {
  const dispatch = useDispatch();
  const { z } = useChrome();
  const dialog = pane.status === 'waiting_permission';
  return (
    <div
      testId={`pane-${pane.id}-waiting`}
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
        backgroundColor: tokens.amberA[8],
        borderWidth: 1,
        borderColor: tokens.amberA[25],
      }}
    >
      <Dot color={tokens.amber} size={7} />
      <Text color={tokens.text} ellipsis>
        {waitText(pane)}
      </Text>
      <div style={{ flexGrow: 1 }} />
      <Text color={tokens.text2} variant="small" weight={400}>
        {`answer in ${pane.cli}`}
      </Text>
      {dialog
        ? ANSWERS.map(({ choice, label }) => {
            const first = choice === 1;
            return (
              <Button
                key={choice}
                testId={`answer-${pane.id}-${choice}`}
                label={`Answer ${choice}, ${label}`}
                variant={first ? 'amber' : 'secondary'}
                height={28}
                paddingLeft={5}
                paddingRight={10}
                gap={7}
                radius={7}
                onClick={() => dispatch({ type: 'pane/answer', paneId: pane.id, choice })}
              >
                <Kbd
                  label={String(choice)}
                  height={18}
                  minWidth={18}
                  paddingX={0}
                  color={first ? tokens.onAmber : tokens.text}
                  background={first ? tokens.onAmberKey : tokens.white[8]}
                  ring={null}
                />
                <Text color={first ? tokens.onAmber : tokens.text} weight={500}>
                  {label}
                </Text>
              </Button>
            );
          })
        : null}
    </div>
  );
}
