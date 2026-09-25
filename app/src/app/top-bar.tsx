import { TabBar } from '../features/tabs/tab-bar';
import { commandKeyLabel } from '../keymap/keymap';
import { selectWaitingCount } from '../state/selectors';
import { useAppSelector, useDispatch } from '../state/store';
import { useChrome } from '../theme/chrome';
import { tokens } from '../theme/tokens';
import { Dot } from '../ui/chip';
import { Icon } from '../ui/icons';
import { Kbd } from '../ui/kbd';
import { Text } from '../ui/text';

function Wordmark() {
  const { z, accent } = useChrome();
  return (
    <div
      testId="wordmark"
      style={{ flexShrink: 0, display: 'flex', alignItems: 'center', gap: z(8) }}
    >
      <div
        style={{
          width: z(8),
          height: z(8),
          borderRadius: z(2),
          backgroundColor: accent.base,
          boxShadow: {
            offsetX: 0,
            offsetY: 0,
            blurRadius: z(12),
            spreadRadius: 0,
            color: accent.a70,
          },
        }}
      />
      <Text color={tokens.text} variant="wordmark">
        ply
      </Text>
    </div>
  );
}

function NeedsYouPill() {
  const dispatch = useDispatch();
  const { z } = useChrome();
  const count = useAppSelector(selectWaitingCount);
  if (count === 0) return null;
  return (
    <div
      testId="needs-you"
      role="button"
      onClick={() => dispatch({ type: 'command', id: 'pane.nextWaiting' })}
      style={{
        height: z(30),
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: z(8),
        paddingLeft: z(10),
        paddingRight: z(12),
        borderRadius: z(15),
        backgroundColor: tokens.amberA[10],
        borderWidth: 1,
        borderColor: tokens.amberA[35],
        cursor: 'pointer',
      }}
    >
      <Dot color={tokens.amber} size={7} />
      <Text color={tokens.amber} weight={500}>
        {count === 1 ? '1 needs you' : `${count} need you`}
      </Text>
      <Text color={tokens.amberA[75]} variant="label" mono>
        {commandKeyLabel('pane.nextWaiting')}
      </Text>
    </div>
  );
}

function PaletteButton() {
  const dispatch = useDispatch();
  const { z } = useChrome();
  return (
    <div
      testId="palette-button"
      role="button"
      aria-label="Open the command palette"
      onClick={() => dispatch({ type: 'command', id: 'palette.open' })}
      style={{
        width: z(300),
        height: z(32),
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: z(10),
        paddingLeft: z(12),
        paddingRight: z(8),
        borderRadius: z(9),
        backgroundColor: tokens.white[4],
        borderWidth: 1,
        borderColor: tokens.white[8],
        cursor: 'pointer',
      }}
    >
      <Icon name="search" size={14} color={tokens.hint} />
      <div style={{ flexGrow: 1, minWidth: 0, display: 'flex' }}>
        <Text color={tokens.hint} ellipsis>
          Search or run a command
        </Text>
      </div>
      <Kbd
        label={commandKeyLabel('palette.open')}
        color={tokens.text2}
        background={tokens.white[2]}
        ring={tokens.white[10]}
      />
    </div>
  );
}

/** The 52 px title bar beside the traffic lights: wordmark, tabs, the needs-you pill and the palette button. */
export function TopBar() {
  const { z } = useChrome();
  return (
    <div
      testId="top-bar"
      style={{
        height: z(tokens.layout.headerHeight),
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: z(14),
        paddingLeft: tokens.layout.headerPaddingLeft,
        paddingRight: z(tokens.layout.headerPaddingRight),
      }}
    >
      <Wordmark />
      <TabBar />
      <div style={{ flexGrow: 1 }} />
      <NeedsYouPill />
      <PaletteButton />
    </div>
  );
}
