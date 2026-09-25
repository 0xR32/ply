import { createContext, useContext } from 'react';
import {
  type AccentName,
  type AccentPalette,
  accentPalette,
  baseFontSize,
  fontFamily,
  tokens,
} from './tokens';

/** Font families the chrome paints with, after deciding whether the bundled Geist families resolve. */
export interface ChromeFonts {
  ui: string;
  mono: string;
}

/** One entry of the chrome type scale, already multiplied by the font scale. */
export interface TypeStyle {
  fontSize: number;
  fontWeight: number;
  lineHeight: number;
}

/** Accent tints, resolved fonts and the type ramp at one scale; `z` scales a length and rounds it to 0.5 px. */
export interface ChromeTheme {
  accent: AccentPalette;
  fonts: ChromeFonts;
  scale: number;
  reducedMotion: boolean;
  type: Record<keyof typeof tokens.type, TypeStyle>;
  z: (length: number) => number;
}

/** The families to use when Geist resolves (the .app bundle, or installed) and when it does not. */
export function chromeFonts(geistAvailable: boolean): ChromeFonts {
  return geistAvailable
    ? { ui: fontFamily.ui, mono: fontFamily.mono }
    : { ui: fontFamily.uiFallbacks[0], mono: fontFamily.monoFallbacks[0] };
}

/** Theme for an accent and a Settings font size in points (`baseFontSize` is scale 1; a bad size is scale 1). */
export function createChromeTheme(
  accent: AccentName,
  fontSize: number,
  fonts: ChromeFonts,
  reducedMotion: boolean,
): ChromeTheme {
  const scale = Number.isFinite(fontSize) && fontSize > 0 ? fontSize / baseFontSize : 1;
  const z = (length: number) => Math.round(length * scale * 2) / 2;
  const type = {} as Record<keyof typeof tokens.type, TypeStyle>;
  for (const [name, t] of Object.entries(tokens.type) as [keyof typeof tokens.type, TypeStyle][]) {
    type[name] = { fontSize: z(t.fontSize), fontWeight: t.fontWeight, lineHeight: z(t.lineHeight) };
  }
  return { accent: accentPalette(accent), fonts, scale, reducedMotion, type, z };
}

/** Provides the chrome theme to every component below it; the default is blue at scale 1 with Geist. */
export const ChromeThemeContext = createContext<ChromeTheme>(
  createChromeTheme('blue', baseFontSize, chromeFonts(true), false),
);

/** The chrome theme of the nearest provider. */
export function useChrome(): ChromeTheme {
  return useContext(ChromeThemeContext);
}
