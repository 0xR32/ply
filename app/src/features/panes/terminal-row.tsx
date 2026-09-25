import { memo } from 'react';
import type { Style } from '../../terminal/frames';
import { type ReplicaRow, sameCells } from '../../terminal/replica';
import { type RowSelection, rowRuns, type StyleResolver } from '../../terminal/runs';

/** Props of one terminal row; its cells (compared by hash, then content) and selection columns decide whether it re-renders. */
export interface TerminalRowProps {
  row: ReplicaRow;
  cols: number;
  styleOf: (id: number) => Style;
  /** Bumped when the replica's style table is replaced, so ids are looked up again. */
  styleEpoch: number;
  resolver: StyleResolver;
  selection: RowSelection;
  cellWidth: number;
  cellHeight: number;
}

function same(a: TerminalRowProps, b: TerminalRowProps): boolean {
  return (
    (a.row === b.row || (a.row.hash === b.row.hash && sameCells(a.row.row, b.row.row))) &&
    a.cols === b.cols &&
    a.styleEpoch === b.styleEpoch &&
    a.resolver === b.resolver &&
    a.cellWidth === b.cellWidth &&
    a.cellHeight === b.cellHeight &&
    a.selection?.[0] === b.selection?.[0] &&
    a.selection?.[1] === b.selection?.[1]
  );
}

/** One screen row: each `<text>` run placed at its own column, rounded to a whole pixel, so neither a fallback glyph nor layout rounding can shift the columns after it. */
export const TerminalRow = memo(function TerminalRow({
  row,
  cols,
  styleOf,
  resolver,
  selection,
  cellWidth,
  cellHeight,
}: TerminalRowProps) {
  const runs = rowRuns(row.row, cols, styleOf, resolver, selection);
  return (
    <div style={{ position: 'relative', height: cellHeight, flexShrink: 0 }}>
      {runs.map((run) => {
        const left = Math.round(run.col * cellWidth);
        return (
          <text
            key={run.col}
            style={{
              position: 'absolute',
              left,
              top: 0,
              width: Math.round((run.col + run.cells) * cellWidth) - left,
              height: cellHeight,
              pointerEvents: 'none',
              color: run.style.color,
              ...(run.style.backgroundColor ? { backgroundColor: run.style.backgroundColor } : {}),
              ...(run.style.bold ? { fontWeight: 600 } : {}),
              ...(run.style.decoration ? { textDecoration: run.style.decoration } : {}),
            }}
          >
            {run.text}
          </text>
        );
      })}
    </div>
  );
}, same);
