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
export const ansi16: Readonly<TerminalTheme['ansi']> = [
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

function channels(hex: string): [number, number, number] {
  const n = Number.parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function withAlpha(hex: string, alpha: number): string {
  const [r, g, b] = channels(hex);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

function blendOver(top: string, alpha: number, bottom: string): string {
  const t = channels(top);
  const b = channels(bottom);
  const mixed = t.map((c, i) => Math.round(c * alpha + (b[i] ?? 0) * (1 - alpha)));
  return `#${mixed.map((c) => c.toString(16).padStart(2, '0').toUpperCase()).join('')}`;
}

/** Terminal palette for an accent: cursor = accent, selection = accent at 25 % blended over `term`; ANSI is fixed. */
export function terminalThemeFor(name: AccentName): TerminalTheme {
  const cursor = accentAlternatives[name];
  return {
    ansi: [...ansi16],
    fg: text,
    bg: term,
    cursor,
    cursorText: term,
    selectionBg: blendOver(cursor, 0.25, term),
    selectionFg: text,
  };
}

/** The default (`blue`) terminal palette. */
export const terminalTheme: TerminalTheme = terminalThemeFor('blue');

/** The accent and every translucent tint of it the chrome paints; `aNN` is the accent at NN % opacity. */
export interface AccentPalette {
  base: string;
  a10: string;
  a12: string;
  a14: string;
  a28: string;
  a35: string;
  a40: string;
  a45: string;
  a55: string;
  a60: string;
  a70: string;
  a80: string;
  groundGlow: string;
  groundGlowEnd: string;
}

/** The tints of one accent choice; the canvas derives every accent wash from the accent this way. */
export function accentPalette(name: AccentName): AccentPalette {
  const base = accentAlternatives[name];
  return {
    base,
    a10: withAlpha(base, 0.1),
    a12: withAlpha(base, 0.12),
    a14: withAlpha(base, 0.14),
    a28: withAlpha(base, 0.28),
    a35: withAlpha(base, 0.35),
    a40: withAlpha(base, 0.4),
    a45: withAlpha(base, 0.45),
    a55: withAlpha(base, 0.55),
    a60: withAlpha(base, 0.6),
    a70: withAlpha(base, 0.7),
    a80: withAlpha(base, 0.8),
    groundGlow: withAlpha(base, 0.09),
    groundGlowEnd: withAlpha(base, 0),
  };
}

/** Families bundled with the .app; in development they resolve only when installed, else the fallbacks apply. */
export const fontFamily = {
  ui: 'Geist',
  mono: 'Geist Mono',
  uiFallbacks: ['.SystemUIFont', 'Helvetica Neue'],
  monoFallbacks: ['Menlo'],
} as const;

/** Default terminal font size in points (Settings `font_size`); the chrome scales by `font_size / baseFontSize`. */
export const baseFontSize = 12.5;

/** Font sizes ⌘= / ⌘- step through, in points; ⌘0 returns to `baseFontSize`. */
export const fontSizeRange = { min: 9.5, max: 24.5, step: 1 } as const;

/** Chrome colours, radii, layout metrics and the type scale; lengths are logical pixels, weights CSS weights. */
export const tokens = {
  ground,
  pane,
  paneFocus,
  term,
  hairline: 'rgba(255, 255, 255, 0.06)',
  hairlineStrong: 'rgba(255, 255, 255, 0.10)',
  text,
  textSoft: '#C3C7D2',
  text2,
  text3,
  hint,
  accent,
  amber,
  mint,
  red,
  violet,
  teal,
  tabActive: '#1A1E2A',
  segmentActive: '#1E2330',
  dotIdle: '#5C6273',
  onAmber: '#1A1406',
  onAmberKey: 'rgba(26, 20, 6, 0.16)',
  onAccent: ground,
  onAccentKey: 'rgba(10, 11, 16, 0.14)',
  knob: text,
  overlay: 'rgba(20, 23, 33, 0.95)',
  backdrop: 'rgba(6, 7, 11, 0.55)',
  overlayShadow: 'rgba(0, 0, 0, 0.8)',
  tabShadow: 'rgba(0, 0, 0, 0.6)',
  /** White washes and rings by percent opacity (6 and 10 are `hairline` / `hairlineStrong`). */
  white: {
    2: 'rgba(255, 255, 255, 0.02)',
    3: 'rgba(255, 255, 255, 0.03)',
    4: 'rgba(255, 255, 255, 0.04)',
    5: 'rgba(255, 255, 255, 0.05)',
    6: 'rgba(255, 255, 255, 0.06)',
    7: 'rgba(255, 255, 255, 0.07)',
    8: 'rgba(255, 255, 255, 0.08)',
    9: 'rgba(255, 255, 255, 0.09)',
    10: 'rgba(255, 255, 255, 0.10)',
    14: 'rgba(255, 255, 255, 0.14)',
  },
  /** Amber washes and rings by percent opacity, for everything that needs the user. */
  amberA: {
    4: 'rgba(242, 179, 91, 0.04)',
    8: 'rgba(242, 179, 91, 0.08)',
    10: 'rgba(242, 179, 91, 0.10)',
    12: 'rgba(242, 179, 91, 0.12)',
    25: 'rgba(242, 179, 91, 0.25)',
    35: 'rgba(242, 179, 91, 0.35)',
    75: 'rgba(242, 179, 91, 0.75)',
  },
  mintA10: 'rgba(127, 212, 176, 0.10)',
  redA12: 'rgba(240, 122, 106, 0.12)',
  radius: {
    pane: 12,
    term: 8,
    control: 8,
    key: 5,
    overlay: 14,
  },
  layout: {
    headerHeight: 52,
    headerPaddingLeft: 84,
    headerPaddingRight: 16,
    trafficLightX: 16,
    trafficLightY: 18,
    groundGlowHeight: 420,
    paneHeaderHeight: 42,
    footerHeight: 36,
    gap: 10,
    gridPaddingX: 14,
    gridPaddingTop: 2,
    gridPaddingBottom: 10,
    overlayTop: 110,
    paletteWidth: 640,
    paletteHeight: 420,
    newPaneWidth: 560,
    newPaneHeight: 580,
  },
  type: {
    wordmark: { fontSize: 15, fontWeight: 600, lineHeight: 20 },
    title: { fontSize: 15, fontWeight: 600, lineHeight: 20 },
    body: { fontSize: 13, fontWeight: 400, lineHeight: 20 },
    small: { fontSize: 12, fontWeight: 500, lineHeight: 16 },
    caption: { fontSize: 11.5, fontWeight: 500, lineHeight: 16 },
    label: { fontSize: 11, fontWeight: 400, lineHeight: 14 },
    key: { fontSize: 10.5, fontWeight: 400, lineHeight: 14 },
    terminal: { fontSize: 12.5, fontWeight: 400, lineHeight: 19 },
  },
} as const;
