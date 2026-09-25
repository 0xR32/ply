import { describe, expect, test } from 'bun:test';
import {
  accentAlternatives,
  accentPalette,
  ansi16,
  terminalTheme,
  terminalThemeFor,
  tokens,
} from './tokens';

const RGB = /^#[0-9A-F]{6}$/;

function luminance(hex: string): number {
  const n = Number.parseInt(hex.slice(1), 16);
  const lin = (c: number): number => {
    const v = c / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin((n >> 16) & 255) + 0.7152 * lin((n >> 8) & 255) + 0.0722 * lin(n & 255);
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return ((hi ?? 0) + 0.05) / ((lo ?? 0) + 0.05);
}

function toHsl(hex: string): [number, number, number] {
  const n = Number.parseInt(hex.slice(1), 16);
  const r = ((n >> 16) & 255) / 255;
  const g = ((n >> 8) & 255) / 255;
  const b = (n & 255) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, l];
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h: number;
  if (max === r) h = (g - b) / d + (g < b ? 6 : 0);
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  return [h / 6, s, l];
}

describe('tokens', () => {
  test('ansi16 has 16 opaque colours and slots 0-8 are the chrome tokens', () => {
    expect(ansi16).toHaveLength(16);
    for (const c of ansi16) expect(c).toMatch(RGB);
    expect(ansi16.slice(0, 9)).toEqual([
      tokens.ground,
      tokens.red,
      tokens.mint,
      tokens.amber,
      tokens.accent,
      tokens.violet,
      tokens.teal,
      tokens.text,
      tokens.brightBlack,
    ]);
  });

  test('white text stays readable on bright black, the Claude Code prompt background', () => {
    expect(contrast(tokens.text, ansi16[8] ?? '')).toBeGreaterThanOrEqual(4.5);
  });

  test('bright slots 9-15 keep the hue of 1-7 at +8 points lightness, clamped at white', () => {
    for (let slot = 1; slot <= 7; slot++) {
      const [h, s, l] = toHsl(ansi16[slot] ?? '');
      const [bh, bs, bl] = toHsl(ansi16[slot + 8] ?? '');
      expect(bl).toBeCloseTo(Math.min(1, l + 0.08), 2);
      if (bl < 1) {
        expect(bh).toBeCloseTo(h, 2);
        expect(bs).toBeCloseTo(s, 1);
      }
    }
  });

  test('terminalTheme uses the accent for the cursor and 25 % accent over term for the selection', () => {
    expect(terminalTheme.cursor).toBe(tokens.accent);
    expect(terminalTheme.bg).toBe(tokens.term);
    const accent = Number.parseInt(tokens.accent.slice(1), 16);
    const term = Number.parseInt(tokens.term.slice(1), 16);
    const sel = Number.parseInt(terminalTheme.selectionBg.slice(1), 16);
    for (const shift of [16, 8, 0]) {
      const blended = ((accent >> shift) & 255) * 0.25 + ((term >> shift) & 255) * 0.75;
      expect(Math.abs(((sel >> shift) & 255) - blended)).toBeLessThanOrEqual(0.5);
    }
    for (const c of Object.values(terminalTheme).flat()) expect(c).toMatch(RGB);
  });

  test('the default accent alternative is the accent token', () => {
    expect(accentAlternatives.blue).toBe(tokens.accent);
  });

  test('every accent choice moves the cursor and selection but never the ANSI slots', () => {
    for (const name of Object.keys(accentAlternatives) as (keyof typeof accentAlternatives)[]) {
      const theme = terminalThemeFor(name);
      expect(theme.cursor).toBe(accentAlternatives[name]);
      expect(theme.selectionBg).toMatch(RGB);
      expect(theme.ansi).toEqual([...ansi16]);
    }
    expect(terminalTheme).toEqual(terminalThemeFor('blue'));
  });

  test('accent tints are the accent at the named opacity', () => {
    const n = Number.parseInt(accentAlternatives.mint.slice(1), 16);
    const rgb = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map(String);
    expect([...(accentPalette('mint').a45.match(/[\d.]+/g) ?? [])]).toEqual([...rgb, '0.45']);
    expect(
      accentPalette('blue')
        .groundGlow.match(/[\d.]+/g)
        ?.at(-1),
    ).toBe('0.09');
  });
});
