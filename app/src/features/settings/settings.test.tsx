import { describe, expect, test } from 'bun:test';
import { hasNativeTestRenderer } from '@gpuix/react/testing';
import type { Action } from '../../state/actions';
import { makeState, mountWithStore } from '../../state/test-support';
import { Settings } from './settings';

describe.if(hasNativeTestRenderer)('Settings', () => {
  test('←→ change the focused choice, Space toggles, Tab walks and esc closes', () => {
    const m = mountWithStore(<Settings />, makeState([], [], { overlay: { kind: 'settings' } }), {
      width: 900,
      height: 800,
    });
    const seen: Action[] = [];
    m.store.addEffect((a) => seen.push(a));
    const press = (...keys: string[]) => {
      for (const k of keys) {
        m.renderer.simulateKeystrokes(k);
        m.renderer.flush();
      }
    };
    try {
      for (const s of ['Accent', '⌥ as Meta', 'Keep the Mac awake while an agent runs']) {
        expect(m.renderer.getAllText()).toContain(s);
      }
      press(
        'right',
        'tab',
        'right',
        'right',
        'tab',
        'space',
        'tab',
        'enter',
        'shift-tab',
        'escape',
      );
      const s = m.store.getState().settings;
      expect(s).toMatchObject({
        accent: 'mint',
        option_as_meta: 'right',
        keep_awake_while_running: false,
        use_ply_colours_in_claude: false,
      });
      expect(seen.filter((a) => a.type === 'settings/change')).toHaveLength(5);
      expect(seen.at(-1)).toEqual({ type: 'overlay/close' });
    } finally {
      m.unmount();
    }
  });
});
