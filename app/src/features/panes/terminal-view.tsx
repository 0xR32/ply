import { type EventPayload, type PublicInstance, useGpuix } from '@gpuix/react';
import {
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react';
import { keysOfEvent, terminalCommandForKeys } from '../../keymap/keymap';
import type { TerminalTheme } from '../../state/actions';
import { useAppSelector } from '../../state/store';
import type { GridSize } from '../../terminal/data-client';
import { droppedPathsText, keepDroppedFile } from '../../terminal/drop';
import { type Cursor, type KeyFrame, Modes, type Style } from '../../terminal/frames';
import { defaultTerminalHost, TerminalHostContext } from '../../terminal/host';
import { keyCode, keyFrame, modsOf, mouseButton, mouseFrame } from '../../terminal/input';
import { linkAt, type TerminalLink } from '../../terminal/links';
import { terminalLog } from '../../terminal/log';
import { cellMetrics, gridFor } from '../../terminal/metrics';
import { blankRow, type ReplicaRow } from '../../terminal/replica';
import { rowText, StyleResolver } from '../../terminal/runs';
import {
  type CellPoint,
  type SelectionUnit,
  selectionBounds,
  selectionOnLine,
} from '../../terminal/selection';
import { type FindMatch, TerminalSession } from '../../terminal/session';
import { tokens } from '../../theme/tokens';
import { TerminalRow } from './terminal-row';

/** The pane body's contract (C6): the props TerminalSlot takes, passed through unchanged. */
export interface TerminalViewProps {
  paneId: number;
  /** The store's focused pane with no overlay open: the view takes GPUI focus so keys reach its pty. */
  focused: boolean;
  theme: TerminalTheme;
  fontFamily: string;
  fontSize: number;
  onTitle?: (title: string) => void;
  onBell?: () => void;
  onExit?: (code: number) => void;
  /** GPUI focus arrived here (a click), so the store's focus must follow. */
  onFocus?: () => void;
  /** Shown until the first screen arrives. */
  placeholder: string;
}

const DEFAULT_STYLE: Style = {
  fg: { kind: 'default' },
  bg: { kind: 'default' },
  underlineColor: { kind: 'default' },
  attrs: 0,
};
const BLINK_MS = 530;
const MEASURE_MS = 250;
const MAX_WHEEL_STEPS = 10;
const SHOW_STATS = process.env.PLY_TERMINAL_STATS === '1';

function useBlink(active: boolean, restart: number): boolean {
  const [on, setOn] = useState(true);
  // biome-ignore lint/correctness/useExhaustiveDependencies: every screen change restarts the blink phase, as terminals do
  useEffect(() => {
    setOn(true);
    if (!active) return;
    const t = setInterval(() => setOn((v) => !v), BLINK_MS);
    return () => clearInterval(t);
  }, [active, restart]);
  return on || !active;
}

function CursorBlock({
  cursor,
  top,
  under,
  wide,
  focused,
  theme,
  cellWidth,
  cellHeight,
}: {
  cursor: Cursor;
  top: number;
  under: string;
  wide: boolean;
  focused: boolean;
  theme: TerminalTheme;
  cellWidth: number;
  cellHeight: number;
}) {
  const left = Math.round(cursor.col * cellWidth);
  const width = Math.round((cursor.col + (wide ? 2 : 1)) * cellWidth) - left;
  const shape = focused ? cursor.shape : 'blockHollow';
  const box = {
    position: 'absolute',
    left,
    top: top * cellHeight,
    pointerEvents: 'none',
  } as const;
  switch (shape) {
    case 'bar':
      return (
        <div style={{ ...box, width: 2, height: cellHeight, backgroundColor: theme.cursor }} />
      );
    case 'underline':
      return (
        <div
          style={{
            ...box,
            top: box.top + cellHeight - 2,
            width,
            height: 2,
            backgroundColor: theme.cursor,
          }}
        />
      );
    case 'blockHollow':
      return (
        <div
          style={{ ...box, width, height: cellHeight, borderWidth: 1, borderColor: theme.cursor }}
        />
      );
    case 'block':
      return (
        <div style={{ ...box, width, height: cellHeight, backgroundColor: theme.cursor }}>
          <text style={{ color: theme.cursorText }}>{under}</text>
        </div>
      );
  }
}

function FindBar({
  matches,
  active,
  onQuery,
  onStep,
  onClose,
  onOwnKey,
  theme,
}: {
  matches: number;
  active: number;
  onQuery: (q: string) => void;
  onStep: (delta: number) => void;
  onClose: () => void;
  /** Marks a key the find field handled, so the terminal skips the same event when it bubbles up. */
  onOwnKey: () => void;
  theme: TerminalTheme;
}) {
  const [query, setQuery] = useState('');
  return (
    <div
      testId="terminal-find"
      style={{
        position: 'absolute',
        top: 4,
        right: 4,
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        paddingLeft: 8,
        paddingRight: 8,
        height: 26,
        borderRadius: tokens.radius.control,
        backgroundColor: tokens.overlay,
        borderWidth: 1,
        borderColor: tokens.hairlineStrong,
      }}
    >
      <input
        autoFocus
        value={query}
        placeholder="Find in scrollback"
        onChange={(e) => {
          const q = e.value ?? '';
          setQuery(q);
          onQuery(q);
        }}
        onSubmit={() => onStep(-1)}
        onKeyDown={(e) => {
          onOwnKey();
          if (e.key === 'escape') onClose();
          else if (e.key === 'up') onStep(-1);
          else if (e.key === 'down') onStep(1);
        }}
        style={{ width: 180, fontSize: tokens.type.small.fontSize, color: theme.fg }}
      />
      <text style={{ color: tokens.text3, fontSize: tokens.type.caption.fontSize }}>
        {query === '' ? '' : matches === 0 ? 'No results' : `${active + 1}/${matches}`}
      </text>
    </div>
  );
}

/** A live pane: rows of `<text>` runs drawn from plyd's C2 frames, with keys, mouse, paste, dropped files, selection, scrollback and ⌘-click links. */
export function TerminalView({
  paneId,
  focused,
  theme,
  fontFamily,
  fontSize,
  onTitle,
  onBell,
  onExit,
  onFocus,
  placeholder,
}: TerminalViewProps) {
  const renderStart = performance.now();
  const { renderer } = useGpuix();
  const host = useContext(TerminalHostContext) ?? defaultTerminalHost();
  const optionAsMeta = useAppSelector((s) => s.settings.option_as_meta);
  const reducedMotion = useAppSelector((s) => s.reducedMotion);
  const ref = useRef<PublicInstance>(null);
  const session = useMemo(
    () => new TerminalSession(host, paneId, {}),
    // A new pane or host is a new attachment; the header callbacks are refreshed below.
    [host, paneId],
  );
  session.setCallbacks({
    ...(onTitle ? { onTitle } : {}),
    ...(onBell ? { onBell } : {}),
    ...(onExit ? { onExit } : {}),
  });
  useEffect(() => () => session.dispose(), [session]);
  useSyncExternalStore(session.subscribe, session.getVersion, session.getVersion);
  useLayoutEffect(() => session.recordRender(performance.now() - renderStart));

  const cell = useMemo(() => cellMetrics(fontFamily, fontSize), [fontFamily, fontSize]);
  const resolver = useMemo(() => new StyleResolver(theme), [theme]);
  const [box, setBox] = useState<{ width: number; height: number } | null>(null);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const measure = () => {
      const id = ref.current?.id;
      const b = id === undefined ? null : (renderer?.getElementBounds?.(id) ?? null);
      if (b) setBox((old) => (old && old.width === b.width && old.height === b.height ? old : b));
      timer = setTimeout(measure, b ? MEASURE_MS : 16);
    };
    measure();
    return () => {
      if (timer) clearTimeout(timer);
    };
  }, [renderer]);

  useEffect(() => {
    if (!box) return;
    const { cols, rows } = gridFor(box.width, box.height, cell);
    const size: GridSize = {
      cols,
      rows,
      cellWidthPx: Math.max(1, Math.round(cell.width)),
      cellHeightPx: cell.height,
    };
    session.resize(size);
  }, [box, cell, session]);

  useEffect(() => {
    const id = ref.current?.id;
    if (focused && id !== undefined) renderer?.focusElement?.(id);
    session.focus(focused);
  }, [focused, renderer, session]);

  const replica = session.replica;
  const styleOf = useCallback((id: number) => replica.style(id) ?? DEFAULT_STYLE, [replica]);
  const [find, setFind] = useState<{ matches: FindMatch[]; active: number } | null>(null);
  const drag = useRef<{ unit: SelectionUnit } | null>(null);
  const wheel = useRef(0);
  const findKey = useRef(false);
  const lastMouse = useRef<{ col: number; row: number } | null>(null);
  // Only a mouse move reports ⌘ (GPUIX sends no modifier change), so the hover is the last ⌘-move's cell.
  const [linkHover, setLinkHover] = useState<CellPoint | null>(null);
  const linkPress = useRef<TerminalLink | null>(null);

  const grid = { width: cell.width, height: cell.height, cols: replica.cols, rows: replica.rows };
  const reporting = (replica.modes & Modes.mouseReporting) !== 0;

  const local = (e: EventPayload) => {
    const id = ref.current?.id;
    const b = id === undefined ? null : (renderer?.getElementBounds?.(id) ?? null);
    return { x: (e.x ?? 0) - (b?.x ?? 0), y: (e.y ?? 0) - (b?.y ?? 0), height: b?.height ?? 0 };
  };
  const pointAt = (x: number, y: number) => ({
    line: session.viewTop + Math.min(Math.max(Math.floor(y / cell.height), 0), replica.rows - 1),
    col: Math.min(Math.max(Math.floor(x / cell.width), 0), Math.max(replica.cols - 1, 0)),
  });
  const linkUnder = (e: EventPayload) => {
    if (replica.rows === 0) return null;
    const { x, y } = local(e);
    return linkAt(replica, pointAt(x, y));
  };
  const hoverLinks = (e: EventPayload) => {
    const { x, y } = local(e);
    const at = e.modifiers?.cmd && replica.rows > 0 ? pointAt(x, y) : null;
    setLinkHover((old) => (old?.line === at?.line && old?.col === at?.col ? old : at));
  };
  const openLink = (url: string) => {
    host.openUrl(url).catch((error: unknown) => {
      terminalLog('warn', 'cannot open a link', { pane_id: paneId, error: String(error) });
    });
  };
  const ownsKeys = () => {
    const id = ref.current?.id;
    const focusedId = renderer?.getFocusedElementId?.();
    return focusedId === undefined || focusedId === null || focusedId === id;
  };

  const runCommand = (keys: string) => {
    switch (terminalCommandForKeys(keys)) {
      case 'terminal.copy':
        void session.copySelection();
        return;
      case 'terminal.paste':
        void session.pasteClipboard();
        return;
      case 'terminal.selectAll':
        session.selectAll();
        return;
      case 'terminal.find':
        setFind({ matches: [], active: -1 });
        void session.loadHistory(replica.scrollbackBase, replica.screenTop);
        return;
      case undefined:
        return;
    }
  };

  const onKeyDown = (e: EventPayload) => {
    // JS handlers cannot stop propagation, so a key the find field already handled arrives here next.
    if (findKey.current) {
      findKey.current = false;
      return;
    }
    setLinkHover(null);
    if (!ownsKeys()) return;
    if (e.modifiers?.cmd) {
      const keys = keysOfEvent(e);
      if (keys) runCommand(keys);
      return;
    }
    if (session.pendingPaste !== null) {
      if (e.key === 'enter') return session.confirmPaste();
      if (e.key === 'escape') return session.cancelPaste();
      session.cancelPaste();
    }
    const frame = keyFrame(e, e.isHeld ? 'repeat' : 'press', optionAsMeta);
    if (frame) session.sendKey(frame);
  };
  const onKeyUp = (e: EventPayload) => {
    if (!ownsKeys() || e.modifiers?.cmd || find) return;
    const frame = keyFrame(e, 'release', optionAsMeta);
    if (frame) session.sendKey(frame);
  };

  const onMouseDown = (e: EventPayload) => {
    if (e.button === 0 && e.modifiers?.cmd) {
      const link = linkUnder(e);
      if (link) {
        linkPress.current = link;
        return;
      }
    }
    const { x, y } = local(e);
    if (reporting && !e.modifiers?.shift) {
      session.sendMouse(
        mouseFrame('press', mouseButton(e.button), modsOf(e.modifiers), x, y, grid),
      );
      return;
    }
    if (e.button !== 0 || replica.rows === 0) return;
    renderer?.clearSelection?.();
    const clicks = e.clickCount ?? 1;
    const unit: SelectionUnit = clicks >= 3 ? 'line' : clicks === 2 ? 'word' : 'cell';
    const at = pointAt(x, y);
    const extend = e.modifiers?.shift && session.selection && unit === 'cell';
    drag.current = { unit };
    session.select(
      extend && session.selection
        ? { ...session.selection, head: at }
        : { anchor: at, head: at, unit },
    );
  };
  const onMouseMove = (e: EventPayload) => {
    if (e.pressedButton === undefined) hoverLinks(e);
    if (linkPress.current) return;
    const { x, y, height } = local(e);
    if (reporting && !e.modifiers?.shift && !drag.current) {
      const f = mouseFrame(
        'motion',
        e.pressedButton === undefined ? 0 : mouseButton(e.pressedButton),
        modsOf(e.modifiers),
        x,
        y,
        grid,
      );
      const last = lastMouse.current;
      if (last && last.col === f.col && last.row === f.row) return;
      lastMouse.current = { col: f.col, row: f.row };
      session.sendMouse(f);
      return;
    }
    const sel = session.selection;
    if (!drag.current || !sel || e.pressedButton !== 0) return;
    if (y < 0) session.scrollBy(1);
    else if (y > height) session.scrollBy(-1);
    session.select({ ...sel, head: pointAt(x, y) });
  };
  const onMouseUp = (e: EventPayload) => {
    const pressed = linkPress.current;
    if (pressed) {
      linkPress.current = null;
      if (linkUnder(e)?.url === pressed.url) openLink(pressed.url);
      return;
    }
    const { x, y } = local(e);
    if (reporting && !e.modifiers?.shift && !drag.current) {
      session.sendMouse(
        mouseFrame('release', mouseButton(e.button), modsOf(e.modifiers), x, y, grid),
      );
      return;
    }
    const sel = session.selection;
    const d = drag.current;
    drag.current = null;
    if (
      d?.unit === 'cell' &&
      sel &&
      sel.anchor.line === sel.head.line &&
      sel.anchor.col === sel.head.col
    ) {
      session.select(null);
    }
  };
  const onScroll = (e: EventPayload) => {
    wheel.current += e.deltaY ?? 0;
    const steps = Math.trunc(wheel.current / cell.height);
    if (steps === 0) return;
    wheel.current -= steps * cell.height;
    const n = Math.min(Math.abs(steps), MAX_WHEEL_STEPS);
    if (reporting && !e.modifiers?.shift) {
      const { x, y } = local(e);
      for (let i = 0; i < n; i++) {
        session.sendMouse(mouseFrame('press', steps > 0 ? 4 : 5, modsOf(e.modifiers), x, y, grid));
      }
    } else if ((replica.modes & Modes.altScreen) !== 0) {
      const arrow = keyCode(steps > 0 ? 'up' : 'down').key;
      const press: KeyFrame = {
        kind: 'key',
        key: arrow,
        mods: 0,
        consumedMods: 0,
        action: 'press',
        composing: false,
        unshiftedCodepoint: 0,
        text: '',
      };
      for (let i = 0; i < n; i++) session.sendKey(press);
    } else {
      session.scrollBy(steps);
    }
  };
  const onFileDrop = (e: EventPayload) => {
    const id = ref.current?.id;
    if (id !== undefined) renderer?.focusElement?.(id);
    // focusElement fires no onFocus (seen under GPUIX's test renderer), so the store is told directly.
    onFocus?.();
    session.paste(
      droppedPathsText((e.paths ?? []).map((p) => keepDroppedFile(p, { pane: paneId }))),
    );
  };

  const bounds = session.selection ? selectionBounds(session.selection, replica) : null;
  const top = session.viewTop;
  const cursor = replica.cursor;
  const cursorRow = cursor.row + session.offset;
  const cursorLine = replica.screenRow(cursor.row)?.row;
  const cursorWide = ((cursorLine?.flags[cursor.col] ?? 0) & 1) !== 0;
  const cursorShown =
    session.attachedOnce &&
    cursor.visible &&
    (replica.modes & Modes.cursorVisible) !== 0 &&
    cursorRow < replica.rows;
  const blinkOn = useBlink(
    cursorShown && cursor.blinking && focused && !reducedMotion,
    session.version,
  );

  // Keyed by content, so a scroll moves the rows it kept instead of re-rendering every position (R-R16).
  const rows: { key: string; row: ReplicaRow; line: number }[] = [];
  const occurrences = new Map<number, number>();
  for (let y = 0; y < replica.rows; y++) {
    const line = top + y;
    const row = replica.line(line) ?? {
      row: blankRow(line - replica.screenTop),
      version: 0,
      hash: 0,
    };
    const n = occurrences.get(row.hash) ?? 0;
    occurrences.set(row.hash, n + 1);
    rows.push({ key: `${row.hash}.${n}`, row, line });
  }
  const overlay =
    session.state.kind === 'down'
      ? `Reconnecting to plyd · ${session.state.reason}`
      : session.state.kind === 'refused'
        ? session.state.message
        : null;
  const stats = SHOW_STATS ? session.stats() : null;
  const link = linkHover ? linkAt(replica, linkHover) : null;

  return (
    <div
      ref={ref}
      testId={`terminal-${paneId}`}
      tabIndex={-1}
      onKeyDown={onKeyDown}
      onKeyUp={onKeyUp}
      onFocus={() => {
        session.focus(true);
        onFocus?.();
      }}
      onBlur={() => {
        session.focus(false);
        setLinkHover(null);
      }}
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
      onScroll={onScroll}
      onFileDrop={onFileDrop}
      style={{
        flexGrow: 1,
        minHeight: 0,
        minWidth: 0,
        position: 'relative',
        overflow: 'hidden',
        userSelect: 'none',
        cursor: link ? 'pointer' : 'text',
      }}
    >
      <div
        testId={`terminal-${paneId}-grid`}
        style={{
          position: 'absolute',
          top: 0,
          left: 0,
          width: replica.cols * cell.width,
          height: replica.rows * cell.height,
          display: 'flex',
          flexDirection: 'column',
          pointerEvents: 'none',
          fontFamily,
          fontSize,
          lineHeight: cell.height,
          whiteSpace: 'nowrap',
          color: theme.fg,
        }}
      >
        {rows.map(({ key, row, line }) => (
          <TerminalRow
            key={key}
            row={row}
            cols={replica.cols}
            styleOf={styleOf}
            styleEpoch={replica.styleEpoch}
            resolver={resolver}
            selection={selectionOnLine(bounds, line, replica.cols)}
            cellWidth={cell.width}
            cellHeight={cell.height}
          />
        ))}
        {link?.segments.map((seg) =>
          seg.line >= top && seg.line < top + replica.rows ? (
            <div
              key={`link-${seg.line}`}
              testId="terminal-link"
              style={{
                position: 'absolute',
                left: Math.round(seg.from * cell.width),
                top: (seg.line - top + 1) * cell.height - 1,
                width: Math.round(seg.to * cell.width) - Math.round(seg.from * cell.width),
                height: 1,
                backgroundColor: theme.fg,
                pointerEvents: 'none',
              }}
            />
          ) : null,
        )}
        {cursorShown && blinkOn ? (
          <CursorBlock
            cursor={cursor}
            top={cursorRow}
            under={cursorLine ? rowText(cursorLine, cursor.col, cursor.col + 1, false) || ' ' : ' '}
            wide={cursorWide}
            focused={session.isFocused}
            theme={theme}
            cellWidth={cell.width}
            cellHeight={cell.height}
          />
        ) : null}
      </div>
      {!session.attachedOnce ? (
        <div
          style={{
            position: 'absolute',
            left: 0,
            right: 0,
            bottom: 0,
            display: 'flex',
            flexDirection: 'column',
            pointerEvents: 'none',
          }}
        >
          <text
            style={{
              color: tokens.hint,
              fontFamily,
              fontSize,
              lineHeight: cell.height,
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
            }}
          >
            {placeholder}
          </text>
        </div>
      ) : null}
      {overlay !== null ? (
        <div
          testId={`terminal-${paneId}-overlay`}
          style={{
            position: 'absolute',
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            backgroundColor: tokens.backdrop,
          }}
        >
          <text style={{ color: tokens.text2, fontSize: tokens.type.small.fontSize }}>
            {overlay}
          </text>
        </div>
      ) : null}
      {session.pendingPaste !== null ? (
        <PasteConfirm
          onConfirm={() => session.confirmPaste()}
          onCancel={() => session.cancelPaste()}
          theme={theme}
        />
      ) : null}
      {find ? (
        <FindBar
          matches={find.matches.length}
          active={find.active}
          theme={theme}
          onQuery={(q) => {
            const matches = session.findMatches(q);
            const active = matches.length - 1;
            setFind({ matches, active });
            revealMatch(session, matches[active]);
          }}
          onStep={(delta) => {
            if (find.matches.length === 0) return;
            const n = find.matches.length;
            const active = (find.active + delta + n) % n;
            setFind({ ...find, active });
            revealMatch(session, find.matches[active]);
          }}
          onOwnKey={() => {
            findKey.current = true;
          }}
          onClose={() => {
            setFind(null);
            const id = ref.current?.id;
            if (id !== undefined) renderer?.focusElement?.(id);
          }}
        />
      ) : null}
      {stats ? (
        <text
          style={{
            position: 'absolute',
            right: 4,
            bottom: 2,
            color: tokens.hint,
            fontSize: tokens.type.key.fontSize,
          }}
        >
          {`decode ${stats.decode.lastMs.toFixed(2)} ms · render ${stats.render.lastMs.toFixed(2)} ms`}
        </text>
      ) : null}
    </div>
  );
}

function revealMatch(session: TerminalSession, match: FindMatch | undefined): void {
  if (!match) return;
  session.reveal(match.line);
  session.select({
    anchor: { line: match.line, col: match.from },
    head: { line: match.line, col: match.to - 1 },
    unit: 'cell',
  });
}

function PasteConfirm({
  onConfirm,
  onCancel,
  theme,
}: {
  onConfirm: () => void;
  onCancel: () => void;
  theme: TerminalTheme;
}) {
  return (
    <div
      testId="terminal-paste-confirm"
      style={{
        position: 'absolute',
        left: 8,
        right: 8,
        bottom: 8,
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        paddingLeft: 10,
        paddingRight: 10,
        height: 34,
        borderRadius: tokens.radius.control,
        backgroundColor: tokens.overlay,
        borderWidth: 1,
        borderColor: tokens.amberA[35],
      }}
    >
      <text style={{ color: theme.fg, fontSize: tokens.type.small.fontSize, flexGrow: 1 }}>
        This paste has line breaks and the program did not ask for bracketed paste. Paste anyway?
      </text>
      <div testId="terminal-paste-yes" onClick={onConfirm} style={{ cursor: 'pointer' }}>
        <text style={{ color: tokens.amber, fontSize: tokens.type.small.fontSize }}>Paste ⏎</text>
      </div>
      <div testId="terminal-paste-no" onClick={onCancel} style={{ cursor: 'pointer' }}>
        <text style={{ color: tokens.text3, fontSize: tokens.type.small.fontSize }}>
          Cancel esc
        </text>
      </div>
    </div>
  );
}
