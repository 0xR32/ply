/** One cell in logical pixels: `width` may be fractional (the font's advance), `height` is whole (so rows land on device pixels). */
export interface CellMetrics {
  width: number;
  height: number;
}

// From each font's own tables: hmtx advance of `m` and hhea ascent + descent, in em (spec R-R3).
const FONTS: Record<string, { advance: number; extent: number }> = {
  'Geist Mono': { advance: 600 / 1000, extent: (1005 + 295) / 1000 },
  Menlo: { advance: 1233 / 2048, extent: (1901 + 483) / 2048 },
};
const FALLBACK = { advance: 0.6, extent: 1.3 };

/**
 * The cell of `fontFamily` at `fontSize` points: width is the advance of `m`, height the font's ascent + descent rounded to whole pixels, the line height at which its box-drawing glyphs join (V1); other families use Geist Mono's proportions.
 */
export function cellMetrics(fontFamily: string, fontSize: number): CellMetrics {
  const f = FONTS[fontFamily] ?? FALLBACK;
  return { width: fontSize * f.advance, height: Math.max(1, Math.round(fontSize * f.extent)) };
}

/** Whole cells that fit in a box of `width` × `height` logical pixels; at least one of each. */
export function gridFor(
  width: number,
  height: number,
  cell: CellMetrics,
): { cols: number; rows: number } {
  return {
    cols: Math.max(1, Math.floor((width + 0.01) / cell.width)),
    rows: Math.max(1, Math.floor((height + 0.01) / cell.height)),
  };
}
