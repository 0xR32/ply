import { commandKeyLabel } from '../../keymap/keymap';
import { selectCliCounts, selectForeignDaemon } from '../../state/selectors';
import { shallowEqual, useAppSelector } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Text } from '../../ui/text';

// A bundle (`just dmg`) compiles its build id in and ships its own plyd, so a restart is all it needs.
const FOREIGN_DAEMON = process.env.PLY_BUILD_ID
  ? `plyd is from another build — Restart plyd (${commandKeyLabel('palette.open')})`
  : 'plyd is from another build — cargo build --release -p ply-daemon, then Restart plyd';

/** The status at the right of the top bar: a notice or the other-build warning, then the per-CLI pane counts; the text gives way first. */
export function StatusBar() {
  const { z } = useChrome();
  const counts = useAppSelector(selectCliCounts, shallowEqual);
  const shellName = useAppSelector((s) => s.env.shellName);
  const notice = useAppSelector((s) => s.notice);
  const foreign = useAppSelector(selectForeignDaemon);
  return (
    <div
      testId="statusbar"
      style={{
        flexShrink: 1,
        minWidth: 0,
        overflow: 'hidden',
        display: 'flex',
        alignItems: 'center',
        gap: z(12),
      }}
    >
      {notice ? (
        <Text color={tokens.amber} variant="small" weight={400} ellipsis testId="notice">
          {notice.text}
        </Text>
      ) : foreign ? (
        <Text color={tokens.amber} variant="small" weight={400} ellipsis testId="foreign-daemon">
          {FOREIGN_DAEMON}
        </Text>
      ) : null}
      <Text color={tokens.hint} variant="label" mono testId="cli-counts">
        {`${counts.claude} claude · ${counts.codex} codex · ${counts.shell} ${shellName}`}
      </Text>
    </div>
  );
}
