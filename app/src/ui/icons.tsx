import { useChrome } from '../theme/chrome';

const SEARCH =
  '<svg viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/></svg>';
const PLUS =
  '<svg viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>';
const BRANCH =
  '<svg viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 7v10M18 10c0 5-7 3-11 7"/></svg>';

const BELL =
  '<svg viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15z"/><path d="M10 20.5a2 2 0 0 0 4 0"/></svg>';

const QUEUE =
  '<svg viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6h11M4 12h11M4 18h7"/><path d="M17 15l4 3-4 3z"/></svg>';
const PAUSE =
  '<svg viewBox="0 0 24 24" fill="black" stroke="none"><rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/></svg>';
const PLAY =
  '<svg viewBox="0 0 24 24" fill="black" stroke="none"><path d="M7 4l13 8-13 8z"/></svg>';
const UP =
  '<svg viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 15l6-6 6 6"/></svg>';
const DOWN =
  '<svg viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6"/></svg>';
const CLOSE =
  '<svg viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>';

const SOURCES = {
  search: SEARCH,
  plus: PLUS,
  branch: BRANCH,
  bell: BELL,
  queue: QUEUE,
  pause: PAUSE,
  play: PLAY,
  up: UP,
  down: DOWN,
  close: CLOSE,
} as const;

/** Name of one of the canvas's line icons. */
export type IconName = keyof typeof SOURCES;

/** A monochrome line icon tinted with `color` (GPUIX paints an `<svg>` only when `style.color` is set). */
export function Icon({ name, size, color }: { name: IconName; size: number; color: string }) {
  const { z } = useChrome();
  return (
    <svg source={SOURCES[name]} style={{ width: z(size), height: z(size), flexShrink: 0, color }} />
  );
}
