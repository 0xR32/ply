/** One column or row of the pane grid, in px from the grid's own origin. */
export interface Track {
  start: number;
  size: number;
}

/** Where each track of `shares` starts and how long it is along `length` px from `start`, with `gap` px between tracks; whole pixels, no gap lost to rounding. */
export function tracks(
  shares: readonly number[],
  start: number,
  length: number,
  gap: number,
): Track[] {
  const usable = Math.max(0, length - gap * (shares.length - 1));
  let before = 0;
  return shares.map((share, i) => {
    const from = Math.round(start + before * usable + i * gap);
    before += share;
    const to = Math.round(start + before * usable + i * gap);
    return { start: from, size: to - from };
  });
}

/** `shares` with the boundary after track `index` moved to `offset` px into the `length` px it spans (gaps of `gap` px included), each of the two tracks kept at least `min` px; unchanged when they cannot both be. */
export function moveBoundary(
  shares: readonly number[],
  index: number,
  offset: number,
  length: number,
  gap: number,
  min: number,
): number[] {
  const out = [...shares];
  const usable = length - gap * (shares.length - 1);
  const a = shares[index];
  const b = shares[index + 1];
  if (a === undefined || b === undefined || usable <= 0) return out;
  const pair = a + b;
  const least = min / usable;
  if (pair < 2 * least) return out;
  const before = shares.slice(0, index).reduce((sum, s) => sum + s, 0);
  const at = (offset - index * gap - gap / 2) / usable - before;
  const next = Math.min(Math.max(at, least), pair - least);
  out[index] = next;
  out[index + 1] = pair - next;
  return out;
}
