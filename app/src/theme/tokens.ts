// The only place a colour literal may appear in app/src (INV-5); values match the design canvas.

import type { TerminalTheme } from '../ipc/proto.gen';

const ground = '#0A0B10';
const pane = '#11131A';
const paneFocus = '#141722';
const term = '#0C0E14';
const text = '#E6E8EF';
const text2 = '#A4A9B8';
const text3 = '#8D93A4';
const hint = '#7C8294';
const accent = '#8AB4FF';
const amber = '#F2B35B';
const mint = '#7FD4B0';
const red = '#F07A6A';
const violet = '#B4A5FF';
const teal = '#86CDBB';

/** Accent colours selectable in Settings (spec 5.3); `blue` is the default and equals `tokens.accent`. */
export const accentAlternatives = {
  blue: accent,
  mint,
  violet,
  sand: '#E6D3A3',
} as const;

/** Name of one selectable accent colour, as stored in Settings. */
export type AccentName = keyof typeof accentAlternatives;

/** SGR slots 0–15: 0–7 are chrome tokens, 8 is `text3`, 9–15 are 1–7 at +8 points HSL lightness.
 *  `tokens.test.ts` enforces the bright-slot rule, so edit a base colour and its bright twin together. */
export const ansi16: readonly string[] = [
  ground,
  red,
  mint,
  amber,
  accent,
  violet,
  teal,
  text,
  text3,
  '#F49B8F',
  '#9EDEC3',
  '#F5C581',
  '#B3CEFF',
  '#D6CEFF',
  '#A3D9CB',
  '#FFFFFF',
];

/** The palette plyd and every terminal view use: cursor = accent, selection = accent at 25 % over `term`. */
export const terminalTheme: TerminalTheme = {
  ansi: [...ansi16],
  fg: text,
  bg: term,
  cursor: accent,
  cursorText: term,
  selectionBg: '#2C384F',
  selectionFg: text,
};

/** Families bundled with the .app; in development they resolve only when installed, else the fallbacks apply. */
export const fontFamily = {
  ui: 'Geist',
  mono: 'Geist Mono',
  uiFallbacks: ['.SystemUIFont', 'Helvetica Neue'],
  monoFallbacks: ['Menlo'],
} as const;

/** Chrome colours, radii, layout metrics and the type scale; lengths are logical pixels, weights CSS weights. */
export const tokens = {
  ground,
  groundGlow: 'rgba(138, 180, 255, 0.09)',
  groundGlowEnd: 'rgba(138, 180, 255, 0)',
  pane,
  paneFocus,
  term,
  hairline: 'rgba(255, 255, 255, 0.06)',
  hairlineStrong: 'rgba(255, 255, 255, 0.10)',
  text,
  text2,
  text3,
  hint,
  accent,
  accentGlow: 'rgba(138, 180, 255, 0.70)',
  amber,
  mint,
  red,
  violet,
  teal,
  radius: {
    pane: 12,
    term: 8,
    control: 8,
    key: 5,
  },
  layout: {
    headerHeight: 52,
    headerPaddingLeft: 84,
    headerPaddingRight: 16,
    trafficLightX: 16,
    trafficLightY: 18,
    groundGlowHeight: 420,
  },
  type: {
    wordmark: { fontSize: 15, fontWeight: 600, lineHeight: 20 },
    body: { fontSize: 13, fontWeight: 400, lineHeight: 20 },
    small: { fontSize: 12, fontWeight: 500, lineHeight: 16 },
    label: { fontSize: 11, fontWeight: 400, lineHeight: 14 },
    key: { fontSize: 10.5, fontWeight: 400, lineHeight: 14 },
    terminal: { fontSize: 12.5, fontWeight: 400, lineHeight: 19 },
  },
} as const;
