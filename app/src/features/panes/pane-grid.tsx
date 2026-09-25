import type { ReactNode } from 'react';
import { commandKeyLabel } from '../../keymap/keymap';
import type { ConnectionState } from '../../state/actions';
import { selectActiveTab, visiblePaneIds } from '../../state/selectors';
import { useAppSelector, useDispatch } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Button } from '../../ui/button';
import { Kbd } from '../../ui/kbd';
import { Text } from '../../ui/text';
import { PaneFrame } from './pane-frame';

function connectionText(c: ConnectionState): { title: string; detail: string } {
  switch (c.kind) {
    case 'connecting':
      return {
        title: 'Connecting to plyd',
        detail: 'the background service that runs every session',
      };
    case 'down':
      return {
        title: 'Daemon not running',
        detail: c.starting
          ? `${c.reason} · starting it`
          : `${c.reason} · retrying in ${(c.retryInMs / 1000).toFixed(1)} s`,
      };
    case 'incompatible':
      return { title: 'plyd is a different version', detail: c.reason };
    case 'connected':
      return { title: 'Connected', detail: '' };
  }
}

function Centered({ testId, children }: { testId: string; children: ReactNode }) {
  const { z } = useChrome();
  return (
    <div
      testId={testId}
      style={{
        flexGrow: 1,
        minHeight: 0,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: z(10),
      }}
    >
      {children}
    </div>
  );
}

function EmptyTab() {
  const dispatch = useDispatch();
  return (
    <Centered testId="pane-grid-empty">
      <Text color={tokens.text} variant="title">
        No panes yet
      </Text>
      <div style={{ display: 'flex', gap: 8 }}>
        {(['pane.new', 'tab.new'] as const).map((id) => (
          <Button
            key={id}
            variant="quiet"
            height={28}
            paddingLeft={8}
            paddingRight={8}
            gap={7}
            radius={7}
            onClick={() => dispatch({ type: 'command', id })}
          >
            <Kbd label={commandKeyLabel(id)} />
            <Text color={tokens.text3} variant="small" weight={400}>
              {id === 'pane.new' ? 'New pane' : 'New tab'}
            </Text>
          </Button>
        ))}
      </div>
    </Centered>
  );
}

/** Columns and rows of the grid for `count` panes (Ruling R56): side by side up to three, a 2 × 2 grid of quadrants at four. */
export function gridShape(count: number): { columns: number; rows: number } {
  if (count <= 3) return { columns: Math.max(count, 1), rows: 1 };
  return { columns: 2, rows: Math.ceil(count / 2) };
}

/** The main area: the active tab's panes as equal columns up to three and quadrants at four, in position order (only the focused pane while zoomed). */
export function PaneGrid() {
  const { z } = useChrome();
  const connection = useAppSelector((s) => s.connection);
  const loaded = useAppSelector((s) => s.workspace !== null);
  const tab = useAppSelector(selectActiveTab);
  if (connection.kind !== 'connected' || !loaded) {
    const text = connectionText(connection);
    return (
      <Centered testId="pane-grid-daemon">
        <Text color={tokens.text} variant="title">
          {connection.kind === 'connected' ? 'Loading the workspace' : text.title}
        </Text>
        <Text color={tokens.text3} variant="small" weight={400}>
          {connection.kind === 'connected' ? '' : text.detail}
        </Text>
      </Centered>
    );
  }
  const ids = visiblePaneIds(tab);
  if (!tab || ids.length === 0) return <EmptyTab />;
  const place = (id: number) => tab.pane_ids.indexOf(id) + 1;
  const { columns, rows } = gridShape(ids.length);
  // One grid parent for every count, so a re-flow moves the panes without remounting (and re-attaching) their terminals.
  return (
    <div
      testId="pane-grid"
      style={{
        flexGrow: 1,
        minHeight: 0,
        display: 'grid',
        gridTemplateColumns: columns,
        gridTemplateRows: rows,
        gridColumnMin: 'zero',
        gridRowMin: 'zero',
        gap: z(tokens.layout.gap),
        paddingTop: z(tokens.layout.gridPaddingTop),
        paddingBottom: z(tokens.layout.gridPaddingBottom),
        paddingLeft: z(tokens.layout.gridPaddingX),
        paddingRight: z(tokens.layout.gridPaddingX),
      }}
    >
      {ids.map((id) => (
        <PaneFrame key={id} paneId={id} position={place(id)} />
      ))}
    </div>
  );
}
