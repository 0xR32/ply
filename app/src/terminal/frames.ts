// Hand-written twin of crates/proto/src/data.rs (C2, spec 4.2); ply-proto's golden frames keep them byte-identical.

import { C2_VERSION } from '../ipc/proto.gen';

export { C2_VERSION };

/** Largest payload of one frame in bytes (1 MiB); a larger header is refused before anything is buffered. */
export const MAX_FRAME_LEN = 1 << 20;

/** Bytes before the payload: `len:u32 LE` and `kind:u8`. */
export const HEADER_LEN = 5;

/** Most rows one FETCH_HISTORY may ask for; plyd may answer with fewer when 1 000 rows exceed one frame. */
export const MAX_HISTORY_ROWS = 1000;

/** Highest libghostty-vt `GhosttyKey` value (176 keys at ghostty 44f2a44). */
export const MAX_KEY_CODE = 175;

/** Highest Unicode scalar value; a larger codepoint is refused. */
export const MAX_CODEPOINT = 0x10ffff;

/** Kind bytes: 0x10–0x1F travel client → plyd, 0x20–0x2F plyd → client. */
export const Kind = {
  attach: 0x10,
  inputRaw: 0x11,
  resize: 0x12,
  fetchHistory: 0x13,
  ack: 0x14,
  key: 0x15,
  mouse: 0x16,
  paste: 0x17,
  focus: 0x18,
  snapshot: 0x20,
  delta: 0x21,
  history: 0x22,
  title: 0x23,
  bell: 0x24,
  exit: 0x25,
  pasteRejected: 0x26,
  attachRefused: 0x27,
  clipboardWrite: 0x28,
} as const;

/** Name of a frame kind, the `kind` discriminant of `Frame`. */
export type KindName = keyof typeof Kind;

/** Modifier bits, identical to libghostty-vt's `GhosttyMods`; the `*Side` bits mean "the right-hand key". */
export const Mods = {
  shift: 1 << 0,
  ctrl: 1 << 1,
  alt: 1 << 2,
  super: 1 << 3,
  capsLock: 1 << 4,
  numLock: 1 << 5,
  shiftSide: 1 << 6,
  ctrlSide: 1 << 7,
  altSide: 1 << 8,
  superSide: 1 << 9,
} as const;
const MODS_ALL = (1 << 10) - 1;

/** Per-cell layout flags; a SPACER or SPACER_HEAD cell draws nothing, GRAPHEME carries extra codepoints. */
export const CellFlags = {
  wide: 1 << 0,
  spacer: 1 << 1,
  spacerHead: 1 << 2,
  grapheme: 1 << 3,
} as const;
const CELL_FLAGS_ALL = 0b1111;

/** Display modes the view reads; plyd never trusts them back (spec R-R5). */
export const Modes = {
  altScreen: 1 << 0,
  cursorVisible: 1 << 1,
  mouseReporting: 1 << 2,
  bracketedPaste: 1 << 3,
} as const;
const MODES_ALL = 0b1111;

/** Style attribute bits of the u16 `attrs`; bits 3–5 hold the underline kind (see `underlineKind`). */
export const Attrs = {
  bold: 1 << 0,
  faint: 1 << 1,
  italic: 1 << 2,
  blink: 1 << 6,
  inverse: 1 << 7,
  invisible: 1 << 8,
  strikethrough: 1 << 9,
  overline: 1 << 10,
} as const;
const ATTRS_DEFINED = (1 << 11) - 1;

/** Underline kind of an `attrs` value: 0 none, 1 single, 2 double, 3 curly, 4 dotted, 5 dashed. */
export function underlineKind(attrs: number): number {
  return (attrs >> 3) & 0b111;
}

/** A symbolic colour; the app resolves it against its theme, so a theme change needs no resend. */
export type Color =
  | { kind: 'default' }
  | { kind: 'indexed'; index: number }
  | { kind: 'rgb'; r: number; g: number; b: number };

/** The look of a run of cells; `underlineColor` `default` follows the foreground. */
export interface Style {
  fg: Color;
  bg: Color;
  underlineColor: Color;
  attrs: number;
}

/** One interned style: `id` is never 0 (0 is the implied default style) and unique per attachment. */
export interface StyleEntry {
  id: number;
  style: Style;
}

/** One row as parallel cell arrays from column 0 (trailing default blanks may be missing); `index` is the screen row in a Snapshot or Delta, the position in the page (line `start + index`) in a History. */
export interface Row {
  index: number;
  /** The text continues on the next row (a soft wrap), so copying joins the two without a newline. */
  wrapped: boolean;
  codepoints: Uint32Array;
  styles: Uint16Array;
  flags: Uint8Array;
  /** The codepoints after the base of each GRAPHEME cell, by column; `null` when the row has none. */
  graphemes: Map<number, readonly number[]> | null;
}

/** Cursor shape, numbered as libghostty-vt's render-state visual style. */
export type CursorShape = 'bar' | 'block' | 'underline' | 'blockHollow';

/** Cursor state in live-screen coordinates. */
export interface Cursor {
  col: number;
  row: number;
  shape: CursorShape;
  visible: boolean;
  blinking: boolean;
}

/** Key action, numbered as libghostty-vt's `GhosttyKeyAction`. */
export type KeyAction = 'release' | 'press' | 'repeat';

/** Mouse action, numbered as libghostty-vt's `GhosttyMouseAction`. */
export type MouseAction = 'press' | 'release' | 'motion';

/** Why plyd refused an ATTACH; plyd closes the connection after it. */
export type RefuseReason = 'versionMismatch' | 'unknownPane' | 'notAttached';

/** 0x10: attach to a pane; the cell pixel sizes are one cell's, not the view's. */
export interface AttachFrame {
  kind: 'attach';
  v: number;
  paneId: number;
  cols: number;
  rows: number;
  cellWidthPx: number;
  cellHeightPx: number;
}

/** 0x15: a key event; plyd encodes it against the pane's live modes (R-R5). */
export interface KeyFrame {
  kind: 'key';
  /** `GhosttyKey`, 0..=175; 0 (unidentified) sends `text` alone. */
  key: number;
  mods: number;
  /** Modifiers the keyboard layout used to produce `text` (⌥ for "@" on a German layout). */
  consumedMods: number;
  action: KeyAction;
  composing: boolean;
  /** The key's codepoint without Shift, for kitty alternate-key reports; 0 when unknown. */
  unshiftedCodepoint: number;
  text: string;
}

/** 0x16: a mouse event; `x`/`y` are pixels from the grid's top-left and must be finite. */
export interface MouseFrame {
  kind: 'mouse';
  action: MouseAction;
  /** 0 none, 1 left, 2 right, 3 middle, 4–7 wheel up/down/left/right, 8–11 extra buttons. */
  button: number;
  mods: number;
  col: number;
  row: number;
  x: number;
  y: number;
}

/** A frame the app sends to plyd. */
export type ClientFrame =
  | AttachFrame
  | { kind: 'inputRaw'; bytes: Uint8Array }
  | { kind: 'resize'; cols: number; rows: number; cellWidthPx: number; cellHeightPx: number }
  /** `start` is an absolute line (the scrollback runs from `scrollbackBase`). */
  | { kind: 'fetchHistory'; start: number; count: number }
  | { kind: 'ack'; seq: number }
  | KeyFrame
  | MouseFrame
  | { kind: 'paste'; allowUnsafe: boolean; text: string }
  | { kind: 'focus'; focused: boolean };

/** 0x20: the whole screen, answering every ATTACH and RESIZE; it replaces the client's style table. */
export interface SnapshotFrame {
  kind: 'snapshot';
  seq: number;
  cols: number;
  rows: number;
  cursor: Cursor;
  modes: number;
  scrollbackRows: number;
  /** Absolute line of the oldest scrollback row; it only grows while plyd drops old lines, so row 0 is line `scrollbackBase + scrollbackRows`. */
  scrollbackBase: number;
  styles: StyleEntry[];
  lines: Row[];
}

/** 0x21: rows changed since the previous frame (possibly none, when only cursor, modes or scrollback moved). */
export interface DeltaFrame {
  kind: 'delta';
  seq: number;
  cursor: Cursor;
  modes: number;
  scrollbackRows: number;
  scrollbackBase: number;
  stylesAdded: StyleEntry[];
  lines: Row[];
}

/** 0x22: scrollback rows answering FETCH_HISTORY, oldest first, clipped to what exists and to one frame. */
export interface HistoryFrame {
  kind: 'history';
  /** Absolute line of the first row (the request's `start` when there is none); row `i` has index `i`. */
  start: number;
  stylesAdded: StyleEntry[];
  lines: Row[];
}

/** A frame plyd sends to the app. */
export type ServerFrame =
  | SnapshotFrame
  | DeltaFrame
  | HistoryFrame
  | { kind: 'title'; title: string }
  | { kind: 'bell' }
  | { kind: 'exit'; code: number }
  | { kind: 'pasteRejected' }
  | { kind: 'attachRefused'; reason: RefuseReason; message: string }
  /** 0x28: the program set the clipboard through OSC 52 (empty text clears it); reads are never answered. */
  | { kind: 'clipboardWrite'; text: string };

/** Every C2 frame. */
export type Frame = ClientFrame | ServerFrame;

/** Why a frame was refused; decoding is strict like ply-proto's (INV-10). */
export type FrameErrorReason =
  | 'unknownKind'
  | 'tooLarge'
  | 'truncated'
  | 'trailingBytes'
  | 'invalidValue'
  | 'invalidUtf8';

/** A malformed frame (decode) or a value that does not fit its field (encode); the connection must be dropped. */
export class FrameError extends Error {
  constructor(
    readonly reason: FrameErrorReason,
    readonly kind: number,
    message: string,
  ) {
    super(`C2 frame 0x${kind.toString(16).padStart(2, '0')}: ${message}`);
    this.name = 'FrameError';
  }
}

const KEY_ACTIONS: readonly KeyAction[] = ['release', 'press', 'repeat'];
const MOUSE_ACTIONS: readonly MouseAction[] = ['press', 'release', 'motion'];
const SHAPES: readonly CursorShape[] = ['bar', 'block', 'underline', 'blockHollow'];
const REASONS: Record<number, RefuseReason> = {
  1: 'versionMismatch',
  2: 'unknownPane',
  3: 'notAttached',
};
const MAX_SAFE = BigInt(Number.MAX_SAFE_INTEGER);
const utf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
const utf8Out = new TextEncoder();

class Reader {
  private readonly view: DataView;
  pos = 0;

  constructor(
    readonly kind: number,
    readonly buf: Uint8Array,
  ) {
    this.view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  }

  private need(n: number): number {
    const at = this.pos;
    if (at + n > this.buf.length)
      throw new FrameError('truncated', this.kind, 'payload ends early');
    this.pos = at + n;
    return at;
  }
  invalid(field: string, value: number | bigint): FrameError {
    return new FrameError('invalidValue', this.kind, `${field} = ${value}`);
  }
  u8(): number {
    return this.buf[this.need(1)] as number;
  }
  u16(): number {
    return this.view.getUint16(this.need(2), true);
  }
  u32(): number {
    return this.view.getUint32(this.need(4), true);
  }
  i32(): number {
    return this.view.getInt32(this.need(4), true);
  }
  u64(field: string): number {
    const v = this.view.getBigUint64(this.need(8), true);
    if (v > MAX_SAFE) throw this.invalid(field, v);
    return Number(v);
  }
  f32(field: string): number {
    const v = this.view.getFloat32(this.need(4), true);
    if (!Number.isFinite(v)) throw this.invalid(field, v);
    return v;
  }
  bool(field: string): boolean {
    const v = this.u8();
    if (v > 1) throw this.invalid(field, v);
    return v === 1;
  }
  codepoint(field: string): number {
    const v = this.u32();
    if (v > MAX_CODEPOINT) throw this.invalid(field, v);
    return v;
  }
  bits(field: string, width: 16, all: number): number {
    const v = width === 16 ? this.u16() : this.u8();
    if ((v & ~all) !== 0) throw this.invalid(field, v);
    return v;
  }
  restText(): string {
    const bytes = this.buf.subarray(this.pos);
    this.pos = this.buf.length;
    try {
      return utf8.decode(bytes);
    } catch {
      throw new FrameError('invalidUtf8', this.kind, 'text is not UTF-8');
    }
  }
  color(field: string): Color {
    const at = this.need(4);
    const tag = this.buf[at] as number;
    const a = this.buf[at + 1] as number;
    const b = this.buf[at + 2] as number;
    const c = this.buf[at + 3] as number;
    if (tag === 0 && a === 0 && b === 0 && c === 0) return { kind: 'default' };
    if (tag === 1 && b === 0 && c === 0) return { kind: 'indexed', index: a };
    if (tag === 2) return { kind: 'rgb', r: a, g: b, b: c };
    throw this.invalid(field, this.view.getUint32(at, true));
  }
  styles(): StyleEntry[] {
    const n = this.u16();
    const out: StyleEntry[] = [];
    for (let i = 0; i < n; i++) {
      const id = this.u16();
      if (id === 0) throw this.invalid('style id', 0);
      const fg = this.color('fg');
      const bg = this.color('bg');
      const underlineColor = this.color('underline colour');
      const attrs = this.u16();
      if ((attrs & ~ATTRS_DEFINED) !== 0 || underlineKind(attrs) > 5) {
        throw this.invalid('attrs', attrs);
      }
      out.push({ id, style: { fg, bg, underlineColor, attrs } });
    }
    return out;
  }
  cursor(): Cursor {
    const col = this.u16();
    const row = this.u16();
    const shapeByte = this.u8();
    const shape = SHAPES[shapeByte];
    if (shape === undefined) throw this.invalid('cursor shape', shapeByte);
    const flags = this.u8();
    if ((flags & ~0b11) !== 0) throw this.invalid('cursor flags', flags);
    return { col, row, shape, visible: (flags & 1) !== 0, blinking: (flags & 2) !== 0 };
  }
  rows(): Row[] {
    const n = this.u16();
    const out: Row[] = [];
    for (let r = 0; r < n; r++) {
      const index = this.i32();
      const wrapped = this.bool('row flags');
      const count = this.u16();
      if (this.pos + count * 7 > this.buf.length) {
        throw new FrameError('truncated', this.kind, 'payload ends early');
      }
      const codepoints = new Uint32Array(count);
      const styles = new Uint16Array(count);
      const flags = new Uint8Array(count);
      let graphemes: Map<number, readonly number[]> | null = null;
      const view = this.view;
      const buf = this.buf;
      for (let c = 0; c < count; c++) {
        const at = this.pos;
        if (at + 7 > buf.length) throw new FrameError('truncated', this.kind, 'payload ends early');
        const cp = view.getUint32(at, true);
        if (cp > MAX_CODEPOINT) throw this.invalid('codepoint', cp);
        const f = buf[at + 6] as number;
        if ((f & ~CELL_FLAGS_ALL) !== 0) throw this.invalid('cell flags', f);
        codepoints[c] = cp;
        styles[c] = view.getUint16(at + 4, true);
        flags[c] = f;
        this.pos = at + 7;
        if ((f & CellFlags.grapheme) !== 0) {
          const len = this.u8();
          if (len === 0) throw this.invalid('grapheme length', 0);
          const extra: number[] = [];
          for (let i = 0; i < len; i++) extra.push(this.codepoint('grapheme codepoint'));
          graphemes ??= new Map();
          graphemes.set(c, extra);
        }
      }
      out.push({ index, wrapped, codepoints, styles, flags, graphemes });
    }
    return out;
  }
}

/** Decodes one payload exactly as ply-proto's `Frame::decode`; throws `FrameError` on any malformed input, including u64 values beyond safe integers. */
export function decodeFrame(kind: number, payload: Uint8Array): Frame {
  if (payload.length > MAX_FRAME_LEN) {
    throw new FrameError('tooLarge', kind, `payload of ${payload.length} bytes`);
  }
  const r = new Reader(kind, payload);
  const frame = decodePayload(kind, r);
  if (r.pos !== payload.length) {
    throw new FrameError(
      'trailingBytes',
      kind,
      `${payload.length - r.pos} bytes after the payload`,
    );
  }
  return frame;
}

function decodePayload(kind: number, r: Reader): Frame {
  switch (kind) {
    case Kind.attach:
      return {
        kind: 'attach',
        v: r.u16(),
        paneId: r.u64('pane_id'),
        cols: r.u16(),
        rows: r.u16(),
        cellWidthPx: r.u16(),
        cellHeightPx: r.u16(),
      };
    case Kind.inputRaw: {
      const bytes = r.buf.slice(r.pos);
      r.pos = r.buf.length;
      return { kind: 'inputRaw', bytes };
    }
    case Kind.resize:
      return {
        kind: 'resize',
        cols: r.u16(),
        rows: r.u16(),
        cellWidthPx: r.u16(),
        cellHeightPx: r.u16(),
      };
    case Kind.fetchHistory: {
      const start = r.u64('start');
      const count = r.u16();
      if (count === 0 || count > MAX_HISTORY_ROWS) throw r.invalid('history count', count);
      return { kind: 'fetchHistory', start, count };
    }
    case Kind.ack:
      return { kind: 'ack', seq: r.u64('seq') };
    case Kind.key: {
      const key = r.u16();
      if (key > MAX_KEY_CODE) throw r.invalid('key', key);
      const mods = r.bits('mods', 16, MODS_ALL);
      const consumedMods = r.bits('consumed mods', 16, MODS_ALL);
      const actionByte = r.u8();
      const action = KEY_ACTIONS[actionByte];
      if (action === undefined) throw r.invalid('key action', actionByte);
      const composing = r.bool('key flags');
      const unshiftedCodepoint = r.codepoint('unshifted codepoint');
      return {
        kind: 'key',
        key,
        mods,
        consumedMods,
        action,
        composing,
        unshiftedCodepoint,
        text: r.restText(),
      };
    }
    case Kind.mouse: {
      const actionByte = r.u8();
      const action = MOUSE_ACTIONS[actionByte];
      if (action === undefined) throw r.invalid('mouse action', actionByte);
      const button = r.u8();
      if (button > 11) throw r.invalid('mouse button', button);
      return {
        kind: 'mouse',
        action,
        button,
        mods: r.bits('mods', 16, MODS_ALL),
        col: r.u16(),
        row: r.u16(),
        x: r.f32('x'),
        y: r.f32('y'),
      };
    }
    case Kind.paste:
      return { kind: 'paste', allowUnsafe: r.bool('allow_unsafe'), text: r.restText() };
    case Kind.focus:
      return { kind: 'focus', focused: r.bool('in') };
    case Kind.snapshot:
      return {
        kind: 'snapshot',
        seq: r.u64('seq'),
        cols: r.u16(),
        rows: r.u16(),
        cursor: r.cursor(),
        modes: r.bits('modes', 16, MODES_ALL),
        scrollbackRows: r.u32(),
        scrollbackBase: r.u64('scrollback_base'),
        styles: r.styles(),
        lines: r.rows(),
      };
    case Kind.delta:
      return {
        kind: 'delta',
        seq: r.u64('seq'),
        cursor: r.cursor(),
        modes: r.bits('modes', 16, MODES_ALL),
        scrollbackRows: r.u32(),
        scrollbackBase: r.u64('scrollback_base'),
        stylesAdded: r.styles(),
        lines: r.rows(),
      };
    case Kind.history: {
      const start = r.u64('start');
      const stylesAdded = r.styles();
      const lines = r.rows();
      const misplaced = lines.find((row, i) => row.index !== i);
      if (misplaced) throw r.invalid('history row index', Math.abs(misplaced.index));
      return { kind: 'history', start, stylesAdded, lines };
    }
    case Kind.title:
      return { kind: 'title', title: r.restText() };
    case Kind.clipboardWrite:
      return { kind: 'clipboardWrite', text: r.restText() };
    case Kind.bell:
      return { kind: 'bell' };
    case Kind.exit:
      return { kind: 'exit', code: r.i32() };
    case Kind.pasteRejected:
      return { kind: 'pasteRejected' };
    case Kind.attachRefused: {
      const byte = r.u8();
      const reason = REASONS[byte];
      if (reason === undefined) throw r.invalid('refuse reason', byte);
      return { kind: 'attachRefused', reason, message: r.restText() };
    }
    default:
      throw new FrameError('unknownKind', kind, 'unknown kind');
  }
}

class Writer {
  buf = new Uint8Array(256);
  view = new DataView(this.buf.buffer);
  len = 0;

  constructor(readonly kind: number) {}

  // Callers must take the offset before touching `buf`/`view`: growing replaces both.
  private room(n: number): number {
    if (this.len + n > this.buf.length) {
      const next = new Uint8Array(Math.max(this.buf.length * 2, this.len + n));
      next.set(this.buf.subarray(0, this.len));
      this.buf = next;
      this.view = new DataView(next.buffer);
    }
    const at = this.len;
    this.len += n;
    return at;
  }
  private check(field: string, v: number, min: number, max: number): number {
    if (!Number.isInteger(v) || v < min || v > max) {
      throw new FrameError('invalidValue', this.kind, `${field} = ${v}`);
    }
    return v;
  }
  u8(field: string, v: number): void {
    const at = this.room(1);
    this.buf[at] = this.check(field, v, 0, 0xff);
  }
  u16(field: string, v: number): void {
    const at = this.room(2);
    this.view.setUint16(at, this.check(field, v, 0, 0xffff), true);
  }
  u32(field: string, v: number): void {
    const at = this.room(4);
    this.view.setUint32(at, this.check(field, v, 0, 0xffffffff), true);
  }
  i32(field: string, v: number): void {
    const at = this.room(4);
    this.view.setInt32(at, this.check(field, v, -0x80000000, 0x7fffffff), true);
  }
  u64(field: string, v: number): void {
    this.check(field, v, 0, Number.MAX_SAFE_INTEGER);
    const at = this.room(8);
    this.view.setBigUint64(at, BigInt(v), true);
  }
  f32(field: string, v: number): void {
    if (!Number.isFinite(v)) throw new FrameError('invalidValue', this.kind, `${field} = ${v}`);
    const at = this.room(4);
    this.view.setFloat32(at, v, true);
  }
  bool(v: boolean): void {
    const at = this.room(1);
    this.buf[at] = v ? 1 : 0;
  }
  bytes(v: Uint8Array): void {
    const at = this.room(v.length);
    this.buf.set(v, at);
  }
  text(v: string): void {
    this.bytes(utf8Out.encode(v));
  }
  color(field: string, c: Color): void {
    const at = this.room(4);
    switch (c.kind) {
      case 'default':
        this.buf.fill(0, at, at + 4);
        return;
      case 'indexed':
        this.buf[at] = 1;
        this.buf[at + 1] = this.check(field, c.index, 0, 255);
        this.buf[at + 2] = 0;
        this.buf[at + 3] = 0;
        return;
      case 'rgb':
        this.buf[at] = 2;
        this.buf[at + 1] = this.check(field, c.r, 0, 255);
        this.buf[at + 2] = this.check(field, c.g, 0, 255);
        this.buf[at + 3] = this.check(field, c.b, 0, 255);
        return;
    }
  }
  styles(entries: readonly StyleEntry[]): void {
    this.u16('style count', entries.length);
    for (const e of entries) {
      this.u16('style id', e.id);
      this.color('fg', e.style.fg);
      this.color('bg', e.style.bg);
      this.color('underline colour', e.style.underlineColor);
      this.u16('attrs', e.style.attrs);
    }
  }
  cursor(c: Cursor): void {
    this.u16('cursor col', c.col);
    this.u16('cursor row', c.row);
    this.u8('cursor shape', SHAPES.indexOf(c.shape));
    this.u8('cursor flags', (c.visible ? 1 : 0) | (c.blinking ? 2 : 0));
  }
  rows(lines: readonly Row[]): void {
    this.u16('row count', lines.length);
    for (const row of lines) {
      this.i32('row index', row.index);
      this.bool(row.wrapped);
      const n = row.codepoints.length;
      this.u16('cell count', n);
      for (let c = 0; c < n; c++) {
        this.u32('codepoint', row.codepoints[c] as number);
        this.u16('cell style', row.styles[c] as number);
        const flags = row.flags[c] as number;
        this.u8('cell flags', flags);
        const extra = row.graphemes?.get(c);
        if ((flags & CellFlags.grapheme) !== 0) {
          if (!extra || extra.length === 0 || extra.length > 255) {
            throw new FrameError('invalidValue', this.kind, `grapheme length at column ${c}`);
          }
          this.u8('grapheme length', extra.length);
          for (const cp of extra) this.u32('grapheme codepoint', cp);
        } else if (extra && extra.length > 0) {
          throw new FrameError(
            'invalidValue',
            this.kind,
            `grapheme without the flag at column ${c}`,
          );
        }
      }
    }
  }
}

/** The frame's wire bytes (`len · kind · payload`) as ply-proto writes them; throws `FrameError` for a value outside its field (never wraps) or over 1 MiB. */
export function encodeFrame(frame: Frame): Uint8Array {
  const kind = Kind[frame.kind];
  const w = new Writer(kind);
  w.u32('len', 0);
  w.u8('kind', kind);
  encodePayload(frame, w);
  const len = w.len - HEADER_LEN;
  if (len > MAX_FRAME_LEN) throw new FrameError('tooLarge', kind, `payload of ${len} bytes`);
  w.view.setUint32(0, len, true);
  return w.buf.slice(0, w.len);
}

function encodePayload(f: Frame, w: Writer): void {
  switch (f.kind) {
    case 'attach':
      w.u16('v', f.v);
      w.u64('pane_id', f.paneId);
      w.u16('cols', f.cols);
      w.u16('rows', f.rows);
      w.u16('cell_width_px', f.cellWidthPx);
      w.u16('cell_height_px', f.cellHeightPx);
      return;
    case 'inputRaw':
      w.bytes(f.bytes);
      return;
    case 'resize':
      w.u16('cols', f.cols);
      w.u16('rows', f.rows);
      w.u16('cell_width_px', f.cellWidthPx);
      w.u16('cell_height_px', f.cellHeightPx);
      return;
    case 'fetchHistory':
      w.u64('start', f.start);
      w.u16('count', f.count);
      return;
    case 'ack':
      w.u64('seq', f.seq);
      return;
    case 'key':
      w.u16('key', f.key);
      w.u16('mods', f.mods);
      w.u16('consumed mods', f.consumedMods);
      w.u8('key action', KEY_ACTIONS.indexOf(f.action));
      w.bool(f.composing);
      w.u32('unshifted codepoint', f.unshiftedCodepoint);
      w.text(f.text);
      return;
    case 'mouse':
      w.u8('mouse action', MOUSE_ACTIONS.indexOf(f.action));
      w.u8('mouse button', f.button);
      w.u16('mods', f.mods);
      w.u16('col', f.col);
      w.u16('row', f.row);
      w.f32('x', f.x);
      w.f32('y', f.y);
      return;
    case 'paste':
      w.bool(f.allowUnsafe);
      w.text(f.text);
      return;
    case 'focus':
      w.bool(f.focused);
      return;
    case 'snapshot':
      w.u64('seq', f.seq);
      w.u16('cols', f.cols);
      w.u16('rows', f.rows);
      w.cursor(f.cursor);
      w.u16('modes', f.modes);
      w.u32('scrollback_rows', f.scrollbackRows);
      w.u64('scrollback_base', f.scrollbackBase);
      w.styles(f.styles);
      w.rows(f.lines);
      return;
    case 'delta':
      w.u64('seq', f.seq);
      w.cursor(f.cursor);
      w.u16('modes', f.modes);
      w.u32('scrollback_rows', f.scrollbackRows);
      w.u64('scrollback_base', f.scrollbackBase);
      w.styles(f.stylesAdded);
      w.rows(f.lines);
      return;
    case 'history':
      if (f.lines.some((row, i) => row.index !== i)) {
        throw new FrameError('invalidValue', Kind.history, 'history row index');
      }
      w.u64('start', f.start);
      w.styles(f.stylesAdded);
      w.rows(f.lines);
      return;
    case 'title':
      w.text(f.title);
      return;
    case 'clipboardWrite':
      w.text(f.text);
      return;
    case 'bell':
    case 'pasteRejected':
      return;
    case 'exit':
      w.i32('code', f.code);
      return;
    case 'attachRefused':
      w.u8('refuse reason', Object.values(REASONS).indexOf(f.reason) + 1);
      w.text(f.message);
      return;
  }
}

/** Splits a byte stream into frames (`next` is `null` until a whole frame arrived); a header over 1 MiB or a bad payload throws and ends the stream. */
export class FrameReader {
  private buf = new Uint8Array(64 * 1024);
  private start = 0;
  private end = 0;

  /** Bytes received but not yet returned as frames. */
  get buffered(): number {
    return this.end - this.start;
  }

  push(chunk: Uint8Array): void {
    if (this.end + chunk.length > this.buf.length) {
      const live = this.end - this.start;
      if (live + chunk.length <= this.buf.length) {
        this.buf.copyWithin(0, this.start, this.end);
      } else {
        const next = new Uint8Array(Math.max(this.buf.length * 2, live + chunk.length));
        next.set(this.buf.subarray(this.start, this.end));
        this.buf = next;
      }
      this.start = 0;
      this.end = live;
    }
    this.buf.set(chunk, this.end);
    this.end += chunk.length;
  }

  next(): Frame | null {
    if (this.end - this.start < HEADER_LEN) return null;
    const b = this.buf;
    const s = this.start;
    const len =
      ((b[s] as number) |
        ((b[s + 1] as number) << 8) |
        ((b[s + 2] as number) << 16) |
        ((b[s + 3] as number) << 24)) >>>
      0;
    const kind = b[s + 4] as number;
    if (len > MAX_FRAME_LEN)
      throw new FrameError('tooLarge', kind, `header announces ${len} bytes`);
    if (this.end - s < HEADER_LEN + len) return null;
    const payload = b.subarray(s + HEADER_LEN, s + HEADER_LEN + len);
    this.start = s + HEADER_LEN + len;
    if (this.start === this.end) {
      this.start = 0;
      this.end = 0;
    }
    return decodeFrame(kind, payload);
  }
}
