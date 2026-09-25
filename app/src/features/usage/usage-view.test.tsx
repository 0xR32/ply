import { describe, expect, test } from 'bun:test';
import { hasNativeTestRenderer } from '@gpuix/react/testing';
import type { Usage } from '../../state/actions';
import type { AppState } from '../../state/reducer';
import { makeState, mountWithStore } from '../../state/test-support';
import { ageText, resetText, spanText, UsageView, usageTone } from './usage-view';

describe('usage text (R59)', () => {
  test('bars turn amber over 80 % and red at 100 %', () => {
    expect([0, 80, 80.5, 99.9, 100, 104].map(usageTone)).toEqual([
      'normal',
      'normal',
      'high',
      'high',
      'full',
      'full',
    ]);
  });

  test('spans read in days, hours and minutes', () => {
    expect(spanText(30)).toBe('under a minute');
    expect(spanText(13 * 60 + 59)).toBe('13 min');
    expect(spanText(2 * 3600 + 13 * 60)).toBe('2 h 13 min');
    expect(spanText(5 * 3600)).toBe('5 h');
    expect(spanText(3 * 86_400 + 4 * 3600 + 59)).toBe('3 d 4 h');
    expect(spanText(2 * 86_400)).toBe('2 d');
  });

  test('a reset time reads ahead while it is to come and behind once it has passed', () => {
    const now = 1_790_350_000;
    expect(resetText(now + 2 * 3600 + 13 * 60, now)).toBe('resets in 2 h 13 min');
    expect(resetText(now - 5 * 60, now)).toBe('reset 5 min ago');
    expect(resetText(undefined, now)).toBe('');
  });

  test('the age says how stale the numbers are', () => {
    const now = 1_790_350_000;
    expect(ageText(now - 20, now)).toBe('updated just now');
    expect(ageText(now - 12 * 60 - 30, now)).toBe('updated 12 min ago');
    const threeHours = new Date((now - 3 * 3600) * 1000);
    const hhmm = (d: Date) =>
      `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
    expect(ageText(now - 3 * 3600, now)).toBe(`as of ${hhmm(threeHours)}`);
    const twoDays = new Date((now - 2 * 86_400) * 1000);
    expect(ageText(now - 2 * 86_400, now)).toMatch(
      new RegExp(`^as of ${twoDays.getDate()} [A-Z][a-z]{2} ${hhmm(twoDays)}$`),
    );
  });
});

function viewState(usage: Usage | null, error: string | null = null): AppState {
  const base = makeState([]);
  return { ...base, usage: { shown: true, usage, error } };
}

describe.if(hasNativeTestRenderer)('UsageView', () => {
  const now = Math.floor(Date.now() / 1000);
  const usage: Usage = {
    claude: {
      as_of: now - 12 * 60,
      windows: [
        {
          label: 'Session · 5h',
          window_minutes: 300,
          used_percent: 81,
          resets_at: now + 2 * 3600 + 13 * 60 + 30,
          models: [],
        },
        {
          label: 'Week · all models',
          window_minutes: 10_080,
          used_percent: 30,
          resets_at: now - 5 * 60 - 10,
          models: [],
        },
        { label: 'Week · Sonnet', window_minutes: 10_080, used_percent: 104, models: [] },
      ],
    },
  };

  test('shows a row per window with its share, bar and reset, and says when a CLI has none', () => {
    const m = mountWithStore(<UsageView />, viewState(usage), { width: 1280, height: 800 });
    try {
      const text = m.renderer.getAllText();
      for (const s of [
        'Claude Code',
        'updated 12 min ago',
        'Session · 5h',
        '81 %',
        'resets in 2 h 13 min',
        'Week · all models',
        'reset 5 min ago',
        'Week · Sonnet',
        '104 %',
        'Codex',
        'No usage recorded yet',
      ]) {
        expect(text).toContain(s);
      }
      expect(m.renderer.findByTestId('usage-fill-high')).toBeDefined();
      expect(m.renderer.findByTestId('usage-fill-full')).toBeDefined();
      expect(m.renderer.findByTestId('usage-fill-past')).toBeDefined();
      expect(m.renderer.findByTestId('usage-fill-normal')).toBeUndefined();
    } finally {
      m.unmount();
    }
  });

  test('names the models behind a Codex limit, and is not there while ⌘U is up', () => {
    const codex: Usage = {
      codex: {
        as_of: now - 30,
        plan: 'plus',
        windows: [
          {
            label: 'Week',
            window_minutes: 10_080,
            used_percent: 14,
            resets_at: now + 86_400,
            models: ['gpt-6-astra', 'gpt-6-sol'],
          },
        ],
      },
    };
    const m = mountWithStore(<UsageView />, viewState(codex), { width: 1280, height: 800 });
    try {
      const text = m.renderer.getAllText();
      expect(text).toContain('gpt-6-astra, gpt-6-sol');
      expect(text).toContain('updated just now');
      expect(m.renderer.findByTestId('usage-fill-normal')).toBeDefined();
    } finally {
      m.unmount();
    }
    const up = mountWithStore(
      <UsageView />,
      { ...viewState(codex), usage: { shown: false, usage: codex, error: null } },
      {
        width: 1280,
        height: 800,
      },
    );
    try {
      expect(up.renderer.findByTestId('usage-view')).toBeUndefined();
    } finally {
      up.unmount();
    }
  });

  test('before the first answer it is reading, and after a failure it says why', () => {
    const reading = mountWithStore(<UsageView />, viewState(null), { width: 1280, height: 800 });
    try {
      expect(reading.renderer.getAllText()).toContain('Reading…');
    } finally {
      reading.unmount();
    }
    const failed = mountWithStore(<UsageView />, viewState(null, 'timeout'), {
      width: 1280,
      height: 800,
    });
    try {
      expect(failed.renderer.getAllText()).toContain('Unavailable');
      expect(failed.renderer.getAllText()).toContain('timeout');
    } finally {
      failed.unmount();
    }
  });
});

describe.if(hasNativeTestRenderer)('⌘U in the window', () => {
  test('the key-down shows the view, repeats keep it, and the key-up hides it', () => {
    const m = mountWithStore(
      // A focusable element for the renderer to send the keys through; the window listeners are the real dispatcher.
      <div testId="focus" tabIndex={0} onFocus={() => {}}>
        <UsageView />
      </div>,
      makeState([]),
      { width: 1280, height: 800 },
      true,
    );
    const focus = m.renderer.findByTestId('focus')?.id ?? -1;
    const shown = () => m.renderer.findByTestId('usage-view') !== undefined;
    try {
      expect(shown()).toBe(false);
      m.renderer.nativeSimulateKeyDown(focus, 'cmd-u', false);
      m.renderer.flush();
      expect(shown()).toBe(true);
      m.renderer.nativeSimulateKeyDown(focus, 'cmd-u', true);
      m.renderer.flush();
      expect(shown()).toBe(true);
      m.renderer.nativeSimulateKeyUp(focus, 'cmd-u');
      m.renderer.flush();
      expect(shown()).toBe(false);
      m.renderer.nativeSimulateKeyDown(focus, 'cmd-u', false);
      m.renderer.nativeSimulateKeyDown(focus, 'cmd-k', false);
      m.renderer.flush();
      expect(shown()).toBe(false);
      expect(m.store.getState().overlay).toEqual({ kind: 'palette' });
    } finally {
      m.unmount();
    }
  });
});
