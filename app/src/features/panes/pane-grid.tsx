import { type EventPayload, type PublicInstance, useGpuix, useWindowSize } from '@gpuix/react';
import { type ReactNode, useEffect, useRef, useState } from 'react';
import { commandKeyLabel } from '../../keymap/keymap';
import type { ConnectionState, Split } from '../../state/actions';
import {
  equalShares,
  gridShape,
  paneSplit,
  selectActiveTab,
  visiblePaneIds,
} from '../../state/selectors';
import { useAppSelector, useDispatch } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Button } from '../../ui/button';
import { Kbd } from '../../ui/kbd';
import { Text } from '../../ui/text';
import { PaneFrame } from './pane-frame';
import { moveBoundary, tracks } from './split';

/** The narrowest a pane gets when a gutter is dragged, in unscaled px. */
const MIN_PANE = 200;

/** How many frames the grid is measured again after a change of window or shape, until its layout has settled. */
const MEASURE_FRAMES = 6;

type Box = { x: number; y: number; width: number; height: number };

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

/** The main area: the active tab's panes in columns up to three and quadrants at four, in position order (only the focused pane while zoomed), with gutters that resize them. */
export function PaneGrid() {
  const { z, scale } = useChrome();
  const dispatch = useDispatch();
  const { renderer } = useGpuix();
  const win = useWindowSize();
  const connection = useAppSelector((s) => s.connection);
  const loaded = useAppSelector((s) => s.workspace !== null);
  const tab = useAppSelector(selectActiveTab);
  const ids = visiblePaneIds(tab);
  const shape = gridShape(ids.length);
  const split = useAppSelector((s) => paneSplit(s, tab?.id, shape));
  const ref = useRef<PublicInstance>(null);
  const [drag, setDrag] = useState<{ axis: 'column' | 'row'; index: number } | null>(null);
  const [box, setBox] = useState<Box | null>(null);
  const shown = connection.kind === 'connected' && loaded && ids.length > 0;
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new window size, text size or shape is what makes the grid worth measuring again
  useEffect(() => {
    if (!shown) return;
    let frames = 0;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const measure = () => {
      const id = ref.current?.id;
      const b = id === undefined ? null : (renderer?.getElementBounds?.(id) ?? null);
      if (b) {
        setBox((old) =>
          old && old.x === b.x && old.y === b.y && old.width === b.width && old.height === b.height
            ? old
            : b,
        );
      }
      frames++;
      if (frames < MEASURE_FRAMES || !b) timer = setTimeout(measure, 16);
    };
    measure();
    return () => {
      if (timer) clearTimeout(timer);
    };
  }, [renderer, shown, win.width, win.height, scale, ids.length]);

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
  if (!tab || ids.length === 0) return <EmptyTab />;
  const place = (id: number) => tab.pane_ids.indexOf(id) + 1;
  const gap = z(tokens.layout.gap);
  const pad = {
    x: z(tokens.layout.gridPaddingX),
    top: z(tokens.layout.gridPaddingTop),
    bottom: z(tokens.layout.gridPaddingBottom),
  };
  const inner = box
    ? { width: box.width - 2 * pad.x, height: box.height - pad.top - pad.bottom }
    : null;
  const columns = inner ? tracks(split.columns, pad.x, inner.width, gap) : null;
  const rows = inner ? tracks(split.rows, pad.top, inner.height, gap) : null;
  const cell = (i: number) => {
    const column = columns?.[shape.rows > 1 ? i % shape.columns : i];
    const row = rows?.[shape.rows > 1 ? Math.floor(i / shape.columns) : 0];
    return column && row
      ? {
          position: 'absolute' as const,
          left: column.start,
          top: row.start,
          width: column.size,
          height: row.size,
        }
      : {};
  };
  const setSplit = (next: Split) => dispatch({ type: 'grid/split', tabId: tab.id, split: next });
  const onGutterDown = (axis: 'column' | 'row', index: number) => (e: EventPayload) => {
    if (e.button !== 0) return;
    if ((e.clickCount ?? 1) >= 2) {
      setDrag(null);
      setSplit({ columns: equalShares(shape.columns), rows: equalShares(shape.rows) });
      return;
    }
    setDrag({ axis, index });
  };
  const onMove = (e: EventPayload) => {
    const d = drag;
    if (!d || !box || !inner) return;
    if (e.pressedButton !== 0) {
      setDrag(null);
      return;
    }
    const min = z(MIN_PANE);
    if (d.axis === 'column') {
      const offset = (e.x ?? 0) - box.x - pad.x;
      setSplit({
        ...split,
        columns: moveBoundary(split.columns, d.index, offset, inner.width, gap, min),
      });
    } else {
      const offset = (e.y ?? 0) - box.y - pad.top;
      setSplit({
        ...split,
        rows: moveBoundary(split.rows, d.index, offset, inner.height, gap, min),
      });
    }
  };
  const gutters: ReactNode[] = [];
  if (columns && rows && inner) {
    columns.slice(0, -1).forEach((c, i) => {
      gutters.push(
        <div
          key={`column-after-${ids[i]}`}
          testId={`pane-grid-gutter-column-${i}`}
          onMouseDown={onGutterDown('column', i)}
          style={{
            position: 'absolute',
            left: c.start + c.size,
            top: pad.top,
            width: gap,
            height: inner.height,
            cursor: 'col-resize',
          }}
        />,
      );
    });
    rows.slice(0, -1).forEach((r, i) => {
      gutters.push(
        <div
          key={`row-after-${ids[i * shape.columns]}`}
          testId={`pane-grid-gutter-row-${i}`}
          onMouseDown={onGutterDown('row', i)}
          style={{
            position: 'absolute',
            left: pad.x,
            top: r.start + r.size,
            width: inner.width,
            height: gap,
            cursor: 'row-resize',
          }}
        />,
      );
    });
  }
  // While a gutter is dragged the pointer is captured: the moves would otherwise go to the terminal under it.
  if (drag && box) {
    gutters.push(
      <div
        key="drag"
        testId="pane-grid-drag"
        onMouseMove={onMove}
        onMouseUp={() => setDrag(null)}
        style={{
          position: 'absolute',
          left: 0,
          top: 0,
          width: box.width,
          height: box.height,
          cursor: drag.axis === 'column' ? 'col-resize' : 'row-resize',
        }}
      />,
    );
  }
  // One parent and one wrapper per pane for every count, so a re-flow or a drag moves the panes without remounting (and re-attaching) their terminals.
  return (
    <div
      ref={ref}
      testId="pane-grid"
      style={{
        position: 'relative',
        flexGrow: 1,
        minHeight: 0,
        display: 'grid',
        gridTemplateColumns: shape.columns,
        gridTemplateRows: shape.rows,
        gridColumnMin: 'zero',
        gridRowMin: 'zero',
        gap,
        paddingTop: pad.top,
        paddingBottom: pad.bottom,
        paddingLeft: pad.x,
        paddingRight: pad.x,
      }}
    >
      {ids.map((id, i) => (
        <div key={id} style={{ display: 'flex', minWidth: 0, minHeight: 0, ...cell(i) }}>
          <PaneFrame paneId={id} position={place(id)} />
        </div>
      ))}
      {gutters}
    </div>
  );
}
