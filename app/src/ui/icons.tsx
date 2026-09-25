import { useChrome } from '../theme/chrome';

const SEARCH =
  '<svg viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/></svg>';
const PLUS =
  '<svg viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>';
const BRANCH =
  '<svg viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 7v10M18 10c0 5-7 3-11 7"/></svg>';

const SOURCES = { search: SEARCH, plus: PLUS, branch: BRANCH } as const;

/** Name of one of the canvas's line icons. */
export type IconName = keyof typeof SOURCES;

/** A monochrome line icon tinted with `color` (GPUIX paints an `<svg>` only when `style.color` is set). */
export function Icon({ name, size, color }: { name: IconName; size: number; color: string }) {
  const { z } = useChrome();
  return (
    <svg source={SOURCES[name]} style={{ width: z(size), height: z(size), flexShrink: 0, color }} />
  );
}
