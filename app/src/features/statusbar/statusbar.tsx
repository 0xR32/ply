import { commandKeyLabel } from '../../keymap/keymap';
import type { CommandId } from '../../state/actions';
import { selectActiveTab, selectCliCounts, selectForeignDaemon } from '../../state/selectors';
import { shallowEqual, useAppSelector, useDispatch } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Button } from '../../ui/button';
import { Kbd } from '../../ui/kbd';
import { Text } from '../../ui/text';

const HINT_HEIGHT = 28;

interface Hint {
  keys: string;
  label: string;
  command: CommandId | null;
}

function hints(zoomed: boolean): Hint[] {
  const k = commandKeyLabel;
  return [
    { keys: k('palette.open'), label: 'Commands', command: 'palette.open' },
    { keys: k('tab.new'), label: 'New tab', command: 'tab.new' },
    { keys: k('pane.new'), label: 'New pane', command: 'pane.new' },
    { keys: k('pane.terminalHere'), label: 'Terminal here', command: 'pane.terminalHere' },
    { keys: k('pane.nextWaiting'), label: 'Next waiting', command: 'pane.nextWaiting' },
    { keys: k('pane.zoom'), label: zoomed ? 'Unzoom' : 'Zoom', command: 'pane.zoom' },
    { keys: `${k('tab.go.1')}–9`, label: 'Tabs', command: null },
    { keys: `${k('pane.left')} ${k('pane.right')}`, label: 'Panes', command: 'pane.next' },
  ];
}

/** The 36 px footer: clickable key hints on the left, a notice and the per-CLI pane counts on the right; hints that do not fit drop from the right. */
export function StatusBar() {
  const dispatch = useDispatch();
  const { z } = useChrome();
  const zoomed = useAppSelector((s) => selectActiveTab(s)?.zoomed ?? false);
  const counts = useAppSelector(selectCliCounts, shallowEqual);
  const shellName = useAppSelector((s) => s.env.shellName);
  const notice = useAppSelector((s) => s.notice);
  const foreign = useAppSelector(selectForeignDaemon);
  return (
    <div
      testId="statusbar"
      style={{
        height: z(tokens.layout.footerHeight),
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: z(4),
        paddingLeft: z(12),
        paddingRight: z(12),
      }}
    >
      <div
        testId="statusbar-hints"
        style={{
          flexGrow: 1,
          flexShrink: 1,
          minWidth: 0,
          height: z(HINT_HEIGHT),
          overflow: 'hidden',
          display: 'flex',
          flexWrap: 'wrap',
          alignItems: 'center',
          gap: z(4),
        }}
      >
        {/* A hint that does not fit wraps onto a second line, which the fixed height hides: whole hints drop. */}
        {hints(zoomed).map((h) => (
          <Button
            key={h.label}
            testId={`hint-${h.label.toLowerCase().replaceAll(' ', '-')}`}
            variant="quiet"
            height={HINT_HEIGHT}
            paddingLeft={8}
            paddingRight={8}
            gap={7}
            radius={7}
            onClick={() => {
              if (h.command) dispatch({ type: 'command', id: h.command });
            }}
          >
            <Kbd label={h.keys} />
            <Text color={tokens.text3} variant="small" weight={400}>
              {h.label}
            </Text>
          </Button>
        ))}
      </div>
      {notice ? (
        <Text color={tokens.amber} variant="small" weight={400} ellipsis testId="notice">
          {notice.text}
        </Text>
      ) : foreign ? (
        <Text color={tokens.amber} variant="small" weight={400} ellipsis testId="foreign-daemon">
          plyd is from another build — cargo build --release -p ply-daemon, then Restart plyd
        </Text>
      ) : null}
      <Text color={tokens.hint} variant="label" mono testId="cli-counts">
        {`${counts.claude} claude · ${counts.codex} codex · ${counts.shell} ${shellName}`}
      </Text>
    </div>
  );
}
