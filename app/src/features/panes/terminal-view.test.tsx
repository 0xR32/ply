import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { TestRenderer } from '@gpuix/react/testing';
import type { ReactNode } from 'react';
import { makePane, makeState, mountWithStore } from '../../state/test-support';
import { type ClientFrame, Modes, type Row, type ServerFrame } from '../../terminal/frames';
import { TerminalHostContext } from '../../terminal/host';
import { cellMetrics } from '../../terminal/metrics';
import { terminalStats } from '../../terminal/session';
import { type FakeHost, fakeTerminalHost, fixtureServerFrames } from '../../terminal/test-host';
import { terminalTheme } from '../../theme/tokens';
import { PaneGrid } from './pane-grid';
import { TerminalView } from './terminal-view';

const FONT = 'Menlo';
const SIZE = 12.5;
const cell = cellMetrics(FONT, SIZE);
const SCREENS = join(
  import.meta.dir,
  '..',
  '..',
  '..',
  '..',
  'crates',
  'term',
  'tests',
  'fixtures',
);

async function settle(renderer: TestRenderer, until: () => boolean = () => true, ms = 1_500) {
  const deadline = Date.now() + ms;
  for (let i = 0; ; i++) {
    renderer.flush();
    renderer.dispatchNativeEvents();
    if (until() && i >= 2) return;
    if (Date.now() > deadline) throw new Error('the view did not settle');
    await Bun.sleep(4);
  }
}

interface Mounted {
  renderer: TestRenderer;
  fake: FakeHost;
  unmount: () => void;
  rerender: (node: ReactNode) => void;
  seen: { titles: string[]; bells: number; exits: number[] };
  body: () => { id: number; x: number; y: number; width: number; height: number };
  sent: () => ClientFrame[];
  deliver: (frames: ServerFrame[]) => Promise<void>;
}

function view(width: number, height: number, seen: Mounted['seen'], focused = true) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', width, height }}>
      <TerminalView
        paneId={1}
        focused={focused}
        theme={terminalTheme}
        fontFamily={FONT}
        fontSize={SIZE}
        placeholder="zsh · ~/code"
        onTitle={(t) => seen.titles.push(t)}
        onBell={() => seen.bells++}
        onExit={(c) => seen.exits.push(c)}
      />
    </div>
  );
}

async function mount(width = 640, height = 320, clipboard = ''): Promise<Mounted> {
  const fake = fakeTerminalHost(clipboard);
  const seen = { titles: [] as string[], bells: 0, exits: [] as number[] };
  const wrap = (node: ReactNode) => (
    <TerminalHostContext.Provider value={fake.host}>{node}</TerminalHostContext.Provider>
  );
  const m = mountWithStore(
    wrap(view(width, height, seen)),
    makeState([makePane({ id: 1, cli: 'shell' })]),
    { width: 1400, height: 900 },
  );
  await settle(m.renderer, () => fake.connections.length === 1);
  const body = () => {
    const el = m.renderer.findByTestId('terminal-1');
    const b = el ? m.renderer.getElementBounds(el.id) : null;
    if (!el || !b) throw new Error('terminal did not paint');
    return { id: el.id, ...b };
  };
  const conn = () => {
    const c = fake.connections[0];
    if (!c) throw new Error('no connection');
    return c;
  };
  return {
    renderer: m.renderer,
    fake,
    unmount: m.unmount,
    rerender: (node) => m.rerender(wrap(node)),
    seen,
    body,
    sent: () => conn().sent,
    deliver: async (frames) => {
      conn().deliver(frames);
      await settle(m.renderer);
    },
  };
}

function snapshotOf(name: string): ServerFrame {
  const [f] = fixtureServerFrames(name);
  if (f?.kind !== 'snapshot') throw new Error(`${name} holds no Snapshot`);
  return f;
}

function textRow(index: number, text: string): Row {
  const cps = [...text].map((c) => c.codePointAt(0) ?? 0);
  return {
    index,
    wrapped: false,
    codepoints: Uint32Array.from(cps),
    styles: new Uint16Array(cps.length),
    flags: new Uint8Array(cps.length),
    graphemes: null,
  };
}

function screen(lines: string[], patch: Partial<Extract<ServerFrame, { kind: 'snapshot' }>> = {}) {
  const frame: ServerFrame = {
    kind: 'snapshot',
    seq: 1,
    cols: 40,
    rows: lines.length,
    cursor: { col: 0, row: 0, shape: 'block', visible: true, blinking: false },
    modes: Modes.cursorVisible,
    scrollbackRows: 0,
    styles: [],
    lines: lines.map((l, i) => textRow(i, l)),
    ...patch,
  };
  return frame;
}

const pressed = (m: Mounted, keys: string) => {
  const before = m.sent().length;
  m.renderer.nativeSimulateKeystrokes(m.body().id, keys);
  return m.sent().slice(before);
};

describe('TerminalView: data path and rendering', () => {
  test('attaches with the grid its measured size holds and the cell size in pixels', async () => {
    const m = await mount(640, 320);
    try {
      const conn = m.fake.connections[0];
      expect(conn?.options.paneId).toBe(1);
      expect(conn?.options.size).toEqual({
        cols: Math.floor(640 / cell.width),
        rows: Math.floor(320 / cell.height),
        cellWidthPx: Math.round(cell.width),
        cellHeightPx: cell.height,
      });
      expect(m.renderer.getPaintedText()).toContain('zsh · ~/code');
    } finally {
      m.unmount();
    }
  });

  test("paints a recorded Claude Code screen: the Engine's rows, in order, as style runs", async () => {
    const m = await mount(900, 500);
    try {
      await m.deliver([snapshotOf('claude-80x24.truth.bin')]);
      const expected = readFileSync(join(SCREENS, 'claude-session.screen'), 'utf8')
        .split('\n')
        .flatMap((l) => /^\d\d\|(.*)$/.exec(l)?.slice(1) ?? []);
      const painted = m.renderer.getPaintedText();
      const squash = (s: string) => s.replace(/\s+/g, '');
      expect(squash(painted.join(''))).toBe(squash(expected.join('')));
      expect(painted).toContain(' I created ');
      expect(painted).toContain('b.txt');
      expect(painted).not.toContain('zsh · ~/code');
    } finally {
      m.unmount();
    }
  });

  test('draws the cursor as a block in the accent with the character under it', async () => {
    const m = await mount();
    try {
      await m.deliver([
        screen(['$ ls', ''], {
          cursor: { col: 2, row: 0, shape: 'block', visible: true, blinking: false },
        }),
      ]);
      const painted = m.renderer.getPaintedText();
      expect(painted.at(-1)).toBe('l');
    } finally {
      m.unmount();
    }
  });

  test('title, bell and exit reach the pane header callbacks', async () => {
    const m = await mount();
    try {
      await m.deliver([screen(['x']), { kind: 'title', title: 'vim a.txt' }, { kind: 'bell' }]);
      await m.deliver([{ kind: 'exit', code: 130 }]);
      expect(m.seen).toMatchObject({ titles: ['vim a.txt'], bells: 1, exits: [130] });
    } finally {
      m.unmount();
    }
  });

  test('shows a reconnecting overlay while the data connection is down', async () => {
    const m = await mount();
    try {
      await m.deliver([screen(['x'])]);
      m.fake.connections[0]?.setState({
        kind: 'down',
        reason: 'plyd is not running',
        retryInMs: 100,
      });
      await settle(m.renderer);
      expect(m.renderer.findByTestId('terminal-1-overlay')).toBeDefined();
      expect(m.renderer.getAllText().join(' ')).toContain('Reconnecting to plyd');
      m.fake.connections[0]?.setState({ kind: 'attached' });
      await settle(m.renderer);
      expect(m.renderer.findByTestId('terminal-1-overlay')).toBeUndefined();
    } finally {
      m.unmount();
    }
  });

  test('sends RESIZE when its box changes and detaches on unmount', async () => {
    const m = await mount(640, 320);
    try {
      await m.deliver([screen(['x'])]);
      m.rerender(view(480, 240, m.seen));
      await settle(m.renderer, () => m.sent().some((f) => f.kind === 'resize'));
      expect(m.sent().find((f) => f.kind === 'resize')).toEqual({
        kind: 'resize',
        cols: Math.floor(480 / cell.width),
        rows: Math.floor(240 / cell.height),
        cellWidthPx: Math.round(cell.width),
        cellHeightPx: cell.height,
      });
    } finally {
      m.unmount();
    }
    expect(m.fake.connections[0]?.closed).toBe(true);
  });

  test('records decode and render time per pane (R-R17)', async () => {
    const m = await mount();
    try {
      await m.deliver([screen(['x'])]);
      const stats = terminalStats().find((s) => s.paneId === 1);
      expect(stats?.render.renders).toBeGreaterThan(0);
      expect(stats?.render.maxMs).toBeGreaterThan(0);
    } finally {
      m.unmount();
    }
  });
});

describe('TerminalView: input', () => {
  test('keys go to plyd as KEY frames; ⌘ chords never do', async () => {
    const m = await mount();
    try {
      await m.deliver([screen(['$ '])]);
      const kinds = (keys: string) =>
        pressed(m, keys).flatMap((f) => (f.kind === 'key' && f.action === 'press' ? [f] : []));
      expect(kinds('a')[0]).toMatchObject({ text: 'a', mods: 0 });
      expect(kinds('ctrl-c')[0]).toMatchObject({ key: 22, mods: 2, text: '' });
      expect(kinds('tab')[0]).toMatchObject({ key: 64, text: '' });
      expect(kinds('shift-tab')[0]).toMatchObject({ key: 64, mods: 1 });
      expect(kinds('escape')[0]).toMatchObject({ key: 120 });
      expect(kinds('up')[0]).toMatchObject({ key: 78 });
      expect(kinds('shift-enter')[0]).toMatchObject({ key: 58, mods: 1, text: '' });
      expect(pressed(m, 'cmd-t')).toEqual([]);
      expect(pressed(m, 'cmd-c')).toEqual([]);
      expect(m.fake.clipboard.writes).toEqual([]);
    } finally {
      m.unmount();
    }
  });

  test('the focused prop goes out as FOCUS frames once attached', async () => {
    const m = await mount();
    try {
      await m.deliver([screen(['x'])]);
      expect(m.sent().filter((f) => f.kind === 'focus')).toEqual([
        { kind: 'focus', focused: true },
      ]);
      m.rerender(view(640, 320, m.seen, false));
      await settle(m.renderer, () => m.sent().filter((f) => f.kind === 'focus').length === 2);
      expect(
        m
          .sent()
          .filter((f) => f.kind === 'focus')
          .at(-1),
      ).toEqual({
        kind: 'focus',
        focused: false,
      });
    } finally {
      m.unmount();
    }
  });

  test('⌘V pastes the clipboard; a refused paste asks, and ⏎ resends it as allowed', async () => {
    const m = await mount(640, 320, 'echo one\necho two');
    try {
      await m.deliver([screen(['$ '])]);
      pressed(m, 'cmd-v');
      await settle(m.renderer, () => m.sent().some((f) => f.kind === 'paste'));
      expect(m.sent().find((f) => f.kind === 'paste')).toEqual({
        kind: 'paste',
        allowUnsafe: false,
        text: 'echo one\necho two',
      });
      await m.deliver([{ kind: 'pasteRejected' }]);
      expect(m.renderer.findByTestId('terminal-paste-confirm')).toBeDefined();
      const sent = pressed(m, 'enter');
      expect(sent).toEqual([{ kind: 'paste', allowUnsafe: true, text: 'echo one\necho two' }]);
      await settle(m.renderer);
      expect(m.renderer.findByTestId('terminal-paste-confirm')).toBeUndefined();
    } finally {
      m.unmount();
    }
  });

  test('a drag selects cells and ⌘C copies them; the selection never sends ^C', async () => {
    const m = await mount();
    try {
      await m.deliver([screen(['first line here', 'second line', 'third'])]);
      const b = m.body();
      const at = (col: number, row: number) =>
        [b.x + (col + 0.5) * cell.width, b.y + (row + 0.5) * cell.height] as const;
      m.renderer.nativeSimulateMouseDown(...at(6, 0));
      m.renderer.nativeSimulateMouseMove(...at(5, 1), 0);
      m.renderer.nativeSimulateMouseUp(...at(5, 1));
      await settle(m.renderer);
      const sent = pressed(m, 'cmd-c');
      await settle(m.renderer, () => m.fake.clipboard.writes.length === 1);
      expect(m.fake.clipboard.writes).toEqual(['line here\nsecond']);
      expect(sent).toEqual([]);
    } finally {
      m.unmount();
    }
  });

  test('with mouse reporting on, clicks go to the program; ⇧ still selects', async () => {
    const m = await mount();
    try {
      await m.deliver([screen(['a', 'b'], { modes: Modes.mouseReporting | Modes.cursorVisible })]);
      const b = m.body();
      m.renderer.nativeSimulateClick(b.x + 3.5 * cell.width, b.y + 1.5 * cell.height);
      await settle(m.renderer);
      const mouse = m.sent().filter((f) => f.kind === 'mouse');
      expect(mouse.map((f) => f.kind === 'mouse' && [f.action, f.button, f.col, f.row])).toEqual([
        ['press', 1, 3, 1],
        ['release', 1, 3, 1],
      ]);
      m.renderer.nativeSimulateMouseDown(b.x + 2, b.y + 2, 0, 'shift');
      await settle(m.renderer);
      expect(m.sent().filter((f) => f.kind === 'mouse').length).toBe(2);
    } finally {
      m.unmount();
    }
  });

  test('the wheel scrolls ply scrollback, fetching the rows it shows', async () => {
    const m = await mount(640, 3 * 15 + 1);
    try {
      const history = Array.from({ length: 30 }, (_, i) => textRow(i - 30, `old ${i}`));
      await m.deliver([screen(['live 0', 'live 1', 'live 2'], { scrollbackRows: 30 })]);
      const b = m.body();
      m.renderer.nativeSimulateScrollWheel(b.x + 10, b.y + 10, 0, cell.height * 2);
      await settle(m.renderer, () => m.sent().some((f) => f.kind === 'fetchHistory'));
      const fetch = m.sent().find((f) => f.kind === 'fetchHistory');
      if (fetch?.kind !== 'fetchHistory') throw new Error('no FETCH_HISTORY');
      expect(fetch.start).toBeLessThan(0);
      const lines = history.filter(
        (r) => r.index >= fetch.start && r.index < fetch.start + fetch.count,
      );
      await m.deliver([{ kind: 'history', start: fetch.start, stylesAdded: [], lines }]);
      const painted = m.renderer.getPaintedText();
      expect(painted).toContain('old 28');
      expect(painted).toContain('old 29');
      expect(painted).toContain('live 0');
      expect(painted).not.toContain('live 2');
      pressed(m, 'x');
      await settle(m.renderer);
      expect(m.renderer.getPaintedText()).toContain('live 2');
    } finally {
      m.unmount();
    }
  });

  test('⌘A selects all scrollback and ⌘C copies it once plyd sent every page', async () => {
    const m = await mount();
    try {
      const history = Array.from({ length: 1500 }, (_, i) => textRow(i - 1500, `h${i}`));
      await m.deliver([screen(['live'], { scrollbackRows: 1500 })]);
      pressed(m, 'cmd-a');
      pressed(m, 'cmd-c');
      for (let served = 0; m.fake.clipboard.writes.length === 0 && served < 10; ) {
        await settle(m.renderer);
        const asks = m.sent().filter((f) => f.kind === 'fetchHistory');
        const ask = asks[served];
        if (ask?.kind !== 'fetchHistory') continue;
        served++;
        const n = served === 1 ? Math.ceil(ask.count / 2) : ask.count;
        const lines = history.filter((r) => r.index >= ask.start && r.index < ask.start + n);
        await m.deliver([{ kind: 'history', start: ask.start, stylesAdded: [], lines }]);
      }
      await settle(m.renderer, () => m.fake.clipboard.writes.length === 1);
      const text = m.fake.clipboard.writes[0] ?? '';
      const lines = text.split('\n');
      expect(lines.length).toBe(1501);
      expect(lines[0]).toBe('h0');
      expect(lines[1499]).toBe('h1499');
      expect(lines[1500]).toBe('live');
    } finally {
      m.unmount();
    }
  });
});

describe('R-R16: a 4-pane tab of 50 × 160 recorded agent screens', () => {
  test('stays under 2 000 host nodes', async () => {
    const fake = fakeTerminalHost();
    const base = makeState([
      makePane({ id: 1 }),
      makePane({ id: 2, position: 1, cli: 'codex' }),
      makePane({ id: 3, position: 2 }),
      makePane({ id: 4, position: 3, cli: 'codex' }),
    ]);
    const m = mountWithStore(
      <TerminalHostContext.Provider value={fake.host}>
        <div style={{ display: 'flex', flexDirection: 'column', width: '100%', height: '100%' }}>
          <PaneGrid />
        </div>
      </TerminalHostContext.Provider>,
      base,
    );
    try {
      await settle(m.renderer, () => fake.connections.length === 4);
      for (const c of fake.connections) {
        const name = c.options.paneId % 2 === 0 ? 'codex-160x50.bin' : 'claude-160x50.bin';
        c.deliver([snapshotOf(name)]);
      }
      await settle(m.renderer);
      const root = m.renderer.getRoot();
      let nodes = 0;
      const walk = (id: number) => {
        const el = m.renderer.getElement(id);
        if (!el) return;
        nodes++;
        for (const child of el.children) walk(child);
      };
      if (root) walk(root.id);
      expect(m.renderer.getPaintedText().join('').length).toBeGreaterThan(1000);
      expect(nodes).toBeLessThan(2000);
    } finally {
      m.unmount();
    }
  });
});
