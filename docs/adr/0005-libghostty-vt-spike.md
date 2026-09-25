# ADR-0005: libghostty-vt spike (S5b)

- Status: Accepted
- Date: 2026-09-25
- Work package: WP0 (spike S5b)
- Spec version: 5.0.1 → 6.0.0 proposed (C2 KEY and MOUSE field layouts change, a contract
  change under rule 0.1.2; the controller reconciles the number with ADR-0003/0004)

## Context

Spec 2 (terminal emulation, build toolchain, PTY), 3.3 (C2, C5, C8), 4.2, 5.2 (R-R4, R-R5,
R-R8 to R-R12, R-R14, R-R18, R-R22), 6.2, WP0 S5b, INV-17, D5, R10 and P4 mark the
libghostty-vt facts that WP3 (headless terminal core) and WP4 (daemon) build on as
VERIFY S5b. This spike proves them with running code.

Evidence, all recorded on 2026-09-25 on the reference machine (Apple M1 Pro, 6 P + 2 E
cores, macOS 26.6.2, Zig 0.16.0, Rust 1.97.1):

1. **Headers** of `vendor/libghostty-vt` (pristine ghostty 44f2a44, `VERSION` =
   `1.3.2-HEAD-+44f2a44df`). Every API fact below cites `include/ghostty/vt/<file>:<line>`
   (abbreviated `terminal.h:1972` etc.). Where a header was silent, the Zig source is cited
   as `src/<path>:<line>`.
2. **A spike crate** (`s5b`, scratch, not committed): a `build.rs` modelled on herdr's,
   hand-written FFI (89 functions and the types and constants they need), a DeltaBuilder,
   and 26 tests plus benchmark subcommands. The FFI was checked at run time against the
   library's own ABI manifest (`ghostty_type_json()`, `types.h:355-410`): 22 struct
   layouts (size and field offsets), 2 type sizes and 47 enum constants match.
3. **herdr** (MIT, `build.rs`, `src/ghostty/mod.rs`), which ships this commit — read as a
   reference only; herdr's copy carries five local patches (its `libghostty-vt.patches.md`)
   that ply's pristine copy does not have.
4. **Recorded real streams**: the eleven Claude Code pty captures of S2b and the four Codex
   0.156.1 captures of S3b, replayed through the engine; the Codex startup-probe fixture of
   ADR-0004.
5. **Load note.** Three sibling spikes ran at the same time (one compiling GPUI); the load
   average was 3–12 on 8 cores during measurements. Each number is the best of 5–7 runs,
   with the median shown where it differs; unloaded runs are expected to be equal or better.

## Decision

### 1. Build (ghostty-sys `build.rs`)

The exact command, run with `current_dir(vendor/libghostty-vt)`:

```
zig build -Demit-lib-vt -Doptimize=ReleaseFast -Dsimd=true -Dtarget=aarch64-macos \
  -Dversion-string=1.3.2-HEAD-+44f2a44df -Demit-xcframework=false \
  --system <pkgdir> -fno-sys=freetype -fno-sys=harfbuzz -fno-sys=fontconfig \
  -fno-sys=libpng -fno-sys=zlib -fno-sys=oniguruma -fno-sys=glslang -fno-sys=spirv-cross \
  -fno-sys=simdutf -fno-sys=gtk4-layer-shell -fno-sys=highway \
  --prefix $OUT_DIR/zig-out --cache-dir $OUT_DIR/zig-cache --global-cache-dir $OUT_DIR/zig-global-cache
```

- **Zig version.** `build.zig.zon:6` requires `minimum_zig_version = "0.16.0"`; 0.16.0 builds it.
- **Zig 0.16 writes fetched packages into `<build root>/zig-pkg/`**, i.e. into
  `vendor/libghostty-vt/zig-pkg/`, and nothing on the command line moves it
  (`--prefix`/`--cache-dir` do not; the vendored `.gitignore:13,16` hides it from git).
  `--system <pkgdir>` disables fetching and makes Zig read packages from `<pkgdir>`
  instead: a build of a fresh extraction with `--system` left the source tree
  byte-identical (`diff -r` against a second fresh extraction). Because `--system` also
  turns every system integration on, each of the 11 integrations listed by
  `zig build -Demit-lib-vt --help` needs `-fno-sys=`.
- **Lazy packages fetched** (network, first build): 8 packages, 7.5 MB compressed —
  `translate_c` (codeberg, `build.zig.zon:12`), `aro` (GitHub, a dependency of
  translate_c), `uucode` (`build.zig.zon:49`, not lazy), `zlib`
  (`pkg/zlib/build.zig.zon:8`), `highway` (`pkg/highway/build.zig.zon:8`), `wuffs` and
  `pixels` (`pkg/wuffs/build.zig.zon:13,20`), `iterm2_themes` (`build.zig.zon:127`).
  All but translate_c and aro come from `deps.files.ghostty.org`. **Offline
  reproducibility:** ply keeps the eight extracted packages (31 MB, keyed by their Zig
  hash) in a ply-owned directory outside `vendor/libghostty-vt` and passes it as
  `<pkgdir>`; with every proxy pointed at a closed port, the `--system` build succeeded
  (65 s) and a `--fetch` without it failed with `ConnectionRefused`. The alternative
  (pre-seeding `--global-cache-dir` with the tarballs) also builds offline (61 s) but
  extracts into `vendor/…/zig-pkg/`, so it is rejected.
- **Do not run any `zig build` in the vendor tree without these flags**: even
  `zig build --help` there fetches 18 more packages (the app's build graph) into
  `vendor/libghostty-vt/zig-pkg/` and writes `.zig-cache/`.
- **Times** (loaded machine): cold build with an empty cache and network fetch 83.9 s
  (102 s user); offline `--system` build 65 s; inside `cargo build --release` 67.8 s;
  no-op rebuild 0.70 s.
- **Output**: `lib/libghostty-vt.a` 10,982,392 bytes (plus a 1.9 MB dylib zig also
  installs). The archive has 204 exported `ghostty_*` symbols and needs only
  libSystem (no libc++). The final spike binary is 501,200 bytes and links only
  `/usr/lib/libSystem.B.dylib` (`otool -L`).
- **Linking.** Copy the `.a` alone into `$OUT_DIR/static/` and emit
  `cargo:rustc-link-search=native=$OUT_DIR/static` +
  `cargo:rustc-link-lib=static=ghostty-vt`. herdr uses `cargo:rustc-link-arg=<path>.a`
  (`build.rs:112`), which works for herdr's single binary but not from a `-sys` library
  crate (Cargo applies `rustc-link-arg` only to the package's own targets). Copying the
  archive alone also keeps `-lghostty-vt` from ever resolving to the dylib zig installs
  next to it.
- The archive's object files embed absolute build paths (49 strings); none reach the
  linked binary (only the spike's own `env!` string did).
- `-Dversion-string` sets the Ghostty app version; the library reports its own
  `0.1.0-dev` (`build.zig:10`, `ghostty_build_info(VERSION_STRING)`, manifest
  `library_version`). The pin identifier stays `44f2a44` / `1.3.2-HEAD-+44f2a44df`.
- Build info at run time: SIMD on, Kitty graphics compiled in, tmux control mode off,
  ReleaseFast (`build_info.h:53-142`).

### 2. API map (WP3 implements against this)

All enums are C `int` (`types.h:36-83`); sized structs start with `size_t size`
(`types.h:326-353`). `GhosttyMode` is a packed `u16` built by a `static inline` helper
(`modes.h:17-19,118-120`) that Rust must reimplement.

| Spec concept | libghostty-vt function / type | Header |
|---|---|---|
| Engine create / free / reset | `ghostty_terminal_new(NULL,&t,cols,rows)`, `_free`, `_reset` | `terminal.h:1972,1987,2000` |
| `write` (pty bytes) | `ghostty_terminal_vt_write` (never fails) | `terminal.h:2055-2078` |
| `resize` (R-R12) | `ghostty_terminal_resize(t,cols,rows,cell_w_px,cell_h_px)`: primary reflows, alternate does not, clears 2026, sends the 2048 report | `terminal.h:2002-2027` |
| Options / queries | `ghostty_terminal_set`, `_get`, `_get_multi` | `terminal.h:2051,2293,2324` |
| Effects (callbacks) | `GHOSTTY_TERMINAL_OPT_*` table; synchronous inside `vt_write`, no re-entry, must not block | `terminal.h:78-102` |
| Pty write-back | `OPT_WRITE_PTY` → `GhosttyTerminalWritePtyFn` | `terminal.h:1045,1109` |
| Palette (R-R4) | `OPT_COLOR_FOREGROUND/BACKGROUND/CURSOR/PALETTE` (`GhosttyColorRgb`, `[256]`) | `terminal.h:134-150,1199-1227` |
| Render state (DeltaBuilder) | `ghostty_render_state_new/update/clean/get`; two-phase `begin_update`/`end_update` for a lock-holding caller | `render.h:42-52,394-512` |
| Global dirty | `RENDER_STATE_DATA_DIRTY` → FALSE / PARTIAL / FULL | `render.h:104-114,152` |
| Dirty rows | `DATA_ROW_ITERATOR` + `ghostty_render_state_row_iterator_next_dirty(it,&y)` (FULL returns every row) | `render.h:159,577,605-626` |
| Row flags | `ROW_DATA_RAW` + `ghostty_row_get(WRAP / WRAP_CONTINUATION / GRAPHEME / DIRTY)` | `render.h:240`, `screen.h:258-318,383` |
| Cells of a row | `ROW_DATA_CELLS` + `row_cells_next/select/get`; bulk `ROW_DATA_CELLS_RAW` (`GhosttyCellsView`) | `render.h:246,265,718-842`, `screen.h:65-71` |
| cell.codepoint | `ghostty_cell_get(raw, CELL_DATA_CODEPOINT)` (u21; 0 = empty) | `screen.h:154,336` |
| cell.flags wide / spacer | `CELL_DATA_WIDE`: NARROW / WIDE / SPACER_TAIL / SPACER_HEAD | `screen.h:102-115,168` |
| cell.flags grapheme + extra codepoints | `CELL_DATA_CONTENT_TAG == CODEPOINT_GRAPHEME`; `CELLS_DATA_GRAPHEMES_LEN`, `_BUF` (base first) | `screen.h:80-93`, `render.h:739-744` |
| style (fg, underline colour, attrs) | `CELLS_DATA_STYLE` → `GhosttyStyle`: `fg_color`, `bg_color`, `underline_color` as `GhosttyStyleColor` {NONE, PALETTE u8, RGB}; 8 bools; `underline` = `GhosttySgrUnderline` 0–5 | `render.h:735`, `style.h:49-108`, `sgr.h:102-107` |
| style bg of an erased cell | content tag `BG_COLOR_PALETTE` / `BG_COLOR_RGB` + `CELL_DATA_COLOR_PALETTE` / `_RGB`; such cells report `HAS_STYLING=false` and a default style | `screen.h:87-91,218-226` |
| Cursor | `DATA_CURSOR` → `GhosttyRenderStateCursor` (viewport x/y, wide_tail, visible, blinking, password_input, bar/block/underline/hollow) | `render.h:121-134,208,315-342` |
| Modes (SNAPSHOT.modes) | `DATA_MODE` + `GhosttyTerminalModeConfig`; `DATA_KITTY_KEYBOARD_FLAGS`, `DATA_MOUSE_TRACKING`, `DATA_ACTIVE_SCREEN`, `DATA_CURSOR_VISIBLE` | `terminal.h:1077-1083,1603-1654,1918`, `modes.h:59-97` |
| Title / pwd | `OPT_TITLE_CHANGED` + `DATA_TITLE`; `OPT_PWD_CHANGED` + `DATA_PWD` (raw bytes, URI not decoded) | `terminal.h:1002,1005-1028,1142,1344,1665,1677` |
| OSC 9 / 777 (C8) | `OPT_DESKTOP_NOTIFICATION` → `{title, body}`; OSC 9;4 → `OPT_PROGRESS_REPORT` | `terminal.h:829-912,1411,1419` |
| BEL | `OPT_BELL` | `terminal.h:340,1117` |
| OSC 52 (R-R11) | `OPT_CLIPBOARD_WRITE` (decoded MIME parts, answer with `reply`); leave `OPT_CLIPBOARD_READ` unset | `terminal.h:556-624,1356,1524` |
| Size reports | `OPT_SIZE` → `GhosttySizeReportSize` (CSI 14/16/18 t, mode 2048) | `terminal.h:986,1150`, `size_report.h:58-67` |
| DA1/2/3 | `OPT_DEVICE_ATTRIBUTES` (optional: without it DA1 = `ESC[?62;22c`) | `terminal.h:949,1170`, `device.h:91-147` |
| CSI ?996n | `OPT_COLOR_SCHEME` → `GhosttyColorScheme` | `terminal.h:928,1160`, `device.h:76-80` |
| XTVERSION / XTGETTCAP TN | `OPT_XTVERSION` (default "libghostty"), `OPT_TERMINFO_NAME` | `terminal.h:1064,1133,1513` |
| Scrollback cap (R-R22) | `OPT_SCROLLBACK_MAX_LINES`; `OPT_SCROLLBACK_MAX_BYTES` = NULL | `terminal.h:1358-1402` |
| Scrollback size / scrollbar | `DATA_TOTAL_ROWS`, `DATA_SCROLLBACK_ROWS`, `DATA_SCROLLBAR` (poll; no notification) | `terminal.h:1619-1635,1684,1691` |
| FETCH_HISTORY | `ghostty_terminal_grid_ref(t,{HISTORY,x,y})` + `ghostty_grid_ref_cell/row/graphemes/style` | `terminal.h:2356`, `grid_ref.h:124-203`, `point.h:32-60` |
| Idle compression (R-R22) | `ghostty_terminal_compression_activity`, `ghostty_terminal_compress(INCREMENTAL)` → PENDING / COMPLETE / UNSUPPORTED | `terminal.h:44-53,192-216,2241,2272` |
| DEC 2026 (R-R18) | `DATA_MODE` with `GHOSTTY_MODE_SYNC_OUTPUT`; cap with `OPT_MODE` value false | `modes.h:92`, `terminal.h:1476,1918` |
| Grapheme clustering (R-R14) | `OPT_MODE_DEFAULT` with mode 2027 | `terminal.h:1466`, `modes.h:93` |
| `snapshot` / `restore` | `ghostty_snapshot_encode_alloc`; `ghostty_snapshot_decoder_new_buf` + `_ready` (renderable) + `_next` (history pages) or `_decode`; `ghostty_free` | `snapshot.h:336,385,458,485,515`, `allocator.h:257` |
| Parser continuation (snapshots mid-sequence) | `OPT_CONTINUATION_MAX_BYTES` | `terminal.h:1421-1439`, `snapshot.h:262-269` |
| `search` | `ghostty_search_new`, `_set(NEEDLE)`, `_run` or `_feed`+`_tick`, `_get(TOTAL_MATCHES / SELECTED_MATCH)` | `search.h:37-51,175-299,325-457` |
| KEY → bytes (R-R5) | `ghostty_key_encoder_new`, `_setopt_from_terminal`, `_setopt(MACOS_OPTION_AS_ALT)`, `_encode`; event: `set_key` (`GhosttyKey`), `set_mods` (`GhosttyMods` u16), `set_consumed_mods`, `set_action`, `set_composing`, `set_utf8`, `set_unshifted_codepoint` | `key/encoder.h:36-117,132-253`, `key/event.h:31-470` |
| MOUSE → bytes (R-R9) | `ghostty_mouse_encoder_new`, `_setopt(SIZE / ANY_BUTTON_PRESSED / TRACK_LAST_CELL)`, `_setopt_from_terminal`, `_encode`; event: action, button (or `clear_button`), mods, position (f32 surface px) | `mouse/encoder.h:33-212`, `mouse/event.h:30-193` |
| FOCUS → bytes (R-R8) | `ghostty_focus_encode(GAINED/LOST)`; plyd gates on mode 1004 | `focus.h:38-68`, `modes.h:78` |
| PASTE → bytes (R-R10) | `ghostty_terminal_paste(t,&GhosttyPaste,&written)` → bytes via WRITE_PTY; `GHOSTTY_REJECTED` for unsafe text | `paste.h:24-53,111-190`, `types.h:103-108` |
| Library log → tracing | `ghostty_sys_set(GHOSTTY_SYS_OPT_LOG, fn)` | `sys.h:98-104,185,217` |
| ABI self-check | `ghostty_type_json()` | `types.h:355-410` |

### 3. Findings, by spec item

**Dirty rows drive a DeltaBuilder — CONFIRMED.** A fresh render state reports FULL
(24 rows); after `ghostty_render_state_clean` (`render.h:495`) and no write, FALSE with no
rows. Writing three lines reported PARTIAL rows [0,1,2]; a CUP to row 10 plus text reported
[2,10] — **the row the cursor left is dirty too**; a cursor move alone reported [4,10]; SGR
text on row 23 reported [23]. Global state went FULL for a scroll (LF on the bottom row),
for entering the alternate screen and for a resize. Per-row flags can be cleared individually with
`ghostty_render_state_row_set(ROW_OPTION_DIRTY)` (`render.h:696`) for partial consumers.
Cost on a 168 × 50 pane (8,400 cells): `render_state_update` plus the dirty-row walk
3.4 µs; a full-screen extraction into ply cells 134 µs with the bulk `CELLS_RAW` view and a
last-style cache (16 ns/cell), 371 µs with the per-cell iterator and a hash lookup per cell.

**Cells, styles and symbolic colours — CONFIRMED, with three rules for the DeltaBuilder.**
Run output: `A中B` gives cp 0x4E2D + WIDE, then cp 0 + SPACER_TAIL; `e` + U+0301 gives
cp 0x65 with extra [0x301]; `ESC[1;3;4;9;53m ESC[31;48;5;200;58;2;1;2;3m` gives fg
PALETTE 1, bg PALETTE 200, underline RGB(1,2,3), bold+italic+strike+overline, underline 1;
`ESC[4:3m` gives underline 3 (curly); truecolor fg stays RGB. (1) `GhosttyStyleId`
(`style.h:39`) is **page-local** (`src/terminal/page.zig:210-211`, one `StyleSet` per page),
so C2's `style:u16` must be ply's own intern table keyed by the `GhosttyStyle` value.
(2) An erased cell with a background (`ESC[44m ESC[K`) keeps its colour in the content tag
(tag 2 = BG_COLOR_PALETTE, `has_styling=false`, style_id 0, `CELLS_DATA_STYLE` default);
`CELLS_DATA_BG_COLOR` would return it resolved to RGB. (3) Never use
`CELLS_DATA_FG_COLOR`/`BG_COLOR` (`render.h:746-760`) for C2: they resolve palette indices
to RGB, which breaks "colours stay symbolic".

**Grapheme clusters (R-R14) — CONFIRMED; the default is off.** With mode 2027 off (the
library default; DECRQM `?2027$p` answers `2` = reset) the family emoji `👨‍👩‍👧` becomes three
wide cells, the first two carrying a trailing ZWJ; with 2027 on it is one wide cell `1F468+[200D,1F469,200D,1F467]`, and a flag pair is
one cell. herdr turns it on through `OPT_MODE_DEFAULT` (`src/ghostty/mod.rs:876-899`).

**Colour and device queries from ply's palette (R-R4) — CONFIRMED; the header names in
the spec are wrong.** Every answer comes out of the `WRITE_PTY` callback. With
`OPT_COLOR_*` set: `OSC 10;?` → `ESC]10;rgb:d4d4/d4d4/d8d8 ESC\`, `OSC 11;?` → the bg,
`OSC 4;1;?` → ply's index 1, `OSC 12;?` → cursor; an OSC 4 set followed by OSC 104 restores
ply's value. **Without a default fg/bg, OSC 10/11 get no answer at all**
(`src/terminal/stream_terminal.zig:1717-1720`), so plyd must set the palette before the
first byte from the child. DSR 5n → `ESC[0n`, DSR 6n at 5;7 → `ESC[5;7R`
(`stream_terminal.zig:1337-1360`); `CSI ?u` → `ESC[?0u`, after `CSI >1u` → `ESC[?1u`
(`stream_terminal.zig:1538-1545`); DA1 without a callback → `ESC[?62;22c`, with one → the
callback's features; `CSI 14/16/18 t` → from the SIZE callback; `CSI ?996n` → `ESC[?997;1n`
only with a COLOR_SCHEME callback; XTVERSION → the callback's string; DECRQM `?2026$p` →
`ESC[?2026;2$y`; `CSI ?2048h` and a later resize → in-band reports; ENQ, XTGETTCAP TN
(until `OPT_TERMINFO_NAME` is set) and `CSI 21t` (`OPT_TITLE_REPORT`, off by default,
`terminal.h:1441-1451`) → nothing. The Codex 0.156.1 startup probe of ADR-0004 was answered
in 2.9 µs: `ESC[1;1R`, OSC 10, OSC 11, `ESC[?5u`, `ESC[?62;22c`. Replaying all four
recorded Codex streams and all eleven Claude Code streams answered every probe they
contain (Claude Code sends XTVERSION, `CSI ?u` and DA1). `vt/color_scheme.h` only encodes the mode
2031 dark/light report (`color_scheme.h:38-65`); `vt/size_report.h` only encodes size
reports by hand (`size_report.h:43-93`); `vt/device.h` holds the DA types. None of them
takes a palette.

**OSC 7 and OSC 9 (C8) — CONFIRMED through the terminal's own callbacks; the standalone
parser cannot do it.** OSC 7 `file://example-host/Users/example/project` reached
`PWD_CHANGED` as the raw URI, `%20` not decoded; `OSC 9;9;path` and
`OSC 1337;CurrentDir=` also land there. OSC 9 bodies reached `DESKTOP_NOTIFICATION` with an
empty title, including when the sequence was split across two `vt_write` calls; replaying
the Codex captures delivered "Codex wants to edit a.txt" and "Created [a.txt](…) containing
`hi`." **Bodies that start with a ConEmu sub-command are not notifications**: `4;1;50`
became a progress report and `9;<path>` a pwd (`src/terminal/osc/parsers/osc9.zig:14-20`);
"10 files changed" and "" were notifications. OSC 777 delivers title and body. The
standalone `vt/osc.h` parser exposes only the command type and a title string
(`osc.h:81-97`), not the OSC 7 URI or the OSC 9 text, so it cannot serve C8. OSC 0/2
reached `TITLE_CHANGED`; a 32 kB Codex capture changed the title about 100 times (its
spinner is in the title).
BEL counted. OSC 52 `c;aGVsbG8gcGx5` arrived decoded as `text/plain "hello ply"`;
`OSC 52;c;?` wrote nothing to the pty with no read callback.

**Encoders cover R-R5 to R-R10 — CONFIRMED; C2's KEY and MOUSE are too narrow.**
Key (legacy): `a` → `a`, ⌃C → `0x03`, ⏎ → `\r`, **⇧⏎ → `ESC[27;2;13~`** (so R-R6's LF
mapping must be done by plyd, the encoder never produces it), ⌃J → `\n`, ⇥ / ⇧⇥ →
`\t` / `ESC[Z`, ↑ → `ESC[A` and after `CSI ?1h` `ESCOA`, F1 → `ESCOP`, release → nothing,
committed IME text with key 0 → the text. Kitty after `CSI >1u`: ⇧⏎ → `ESC[13;2u`, ⌃C →
`ESC[99;5u`, Esc → `ESC[27u`; after `CSI =31u`: `a` press / release → `ESC[97;;97u` /
`ESC[97;1:3u`. Option handling depends on three inputs: `macos_option_as_alt`
(`key/encoder.h:67-77`, **reset to FALSE by every `setopt_from_terminal`**,
`key/encoder.h:177-179`), the ALT side bit (`MODS_ALT_SIDE`, `key/event.h:86`), and
`consumed_mods`. ⌥X with text "≈": option-as-alt FALSE → "≈"; TRUE and ALT not consumed →
`ESC x`; TRUE and ALT consumed → "≈"; RIGHT with the left ⌥ → "≈", with the right ⌥ →
`ESC x`; LEFT with the left ⌥ → `ESC x`; under kitty 31 → `ESC[120;3;8776u`
(`src/input/key_encode.zig:555-572`). Mods are a `u16`
with 10 bits in use (`key/event.h:57-91`); `GhosttyKey` has 176 values, 0–175
(`key/event.h:107-301`). Mouse: nothing is encoded without a tracking mode; `?1000h` →
X10 bytes; `?1006h` press/release at pixel (45,50) with 10 × 20 px cells → `ESC[<0;5;3M`
/ `…m`; motion without a button under 1000 → nothing; wheel (button 4) → `ESC[<64;5;3M`;
⌃-click → `ESC[<16;5;3M`; 1002 drag → `ESC[<32;7;3M`; 1003 hover → `ESC[<35;8;3M`; 1016
SGR-pixels → `ESC[<0;48;51M` from (47.5, 51.25). The encoder takes positions in surface
pixels plus a geometry struct (`mouse/encoder.h:73-100`) and needs the caller to track
"any button pressed" (`mouse/encoder.h:121`). Focus: `ESC[I` / `ESC[O`; the encoder is
terminal-free, so plyd checks mode 1004 (true after `CSI ?1004h`). Paste: needs
`WRITE_PTY` (`GHOSTTY_INVALID_VALUE` otherwise, `paste.h:180-181`); 2004 off: `echo hi` →
as is, `a\nb` → `GHOSTTY_REJECTED` (-7), with `allow_unsafe` → `a\rb`, ESC → space; 2004
on: `a\nb` → `ESC[200~a\nbESC[201~`, a pasted `ESC[201~` → REJECTED.

**DEC 2026 (R-R18) — CONFIRMED as a polled mode; there is no callback.** `CSI ?2026h`
sets `GHOSTTY_MODE_SYNC_OUTPUT`, readable through `DATA_MODE`; the render state still
reports dirty rows while it is set, so the hold is plyd's job (Ghostty's own renderer skips
frames, `src/renderer/generic.zig:1325`, and resets the mode after 1,000 ms,
`src/termio/Thread.zig:36-38`). A resize clears it (`terminal.h:2009-2011`, measured), and
`OPT_MODE` with value false clears it for the 150 ms cap (measured). Codex enables 2026
(seen in all four replays); the Claude Code captures never did (ADR-0003 explains why that
harness could not show it).

**Idle-scrollback compression (R-R22) — CONFIRMED on macOS.** Caller-driven: poll the
activity token, restart an idle timer when it changes, then call
`ghostty_terminal_compress(INCREMENTAL)` until it stops returning PENDING
(`terminal.h:44-53`; the `c-vt-compression` example). On macOS it returns COMPLETE, not
UNSUPPORTED: 12,000 styled 168-column lines took 40 steps / 3.8 ms; six 10k-line panes
took 240 steps / 20–175 ms. The token does not change when compressing. Reading history
decompresses what it touches (`terminal.h:2257-2258`; formatting a whole compressed pane
raised the footprint again).

**Memory per pane (P4) — MEASURED; met when idle, not while a styled stream is live.**
`proc_pid_rusage` phys_footprint (what `vmmap` reports as footprint), six 168 × 50
terminals filled at once, delta divided by six:

| content | scrollback rows kept | filled | after idle compression |
|---|---|---|---|
| plain ASCII, cap 10 000 | 9,718 | 13.00 MiB | 0.52 MiB |
| plain ASCII, cap 10 300 | 10,118 | 13.51 MiB (14.2 MB) | 0.67 MiB |
| recorded colour `git log -p` | 10,149 (cap 10 300) | 13.96 MiB (14.6 MB) | 1.93 MiB |
| SGR on every word, truecolor, CJK, emoji | 10,031 (cap 10 300) | 16.22 MiB (17.0 MB) | 4.75 MiB |

An empty terminal costs 0.03 MiB; a render state plus DeltaBuilder another 0.79 MiB. The
cost follows the 8-byte cell: 10,149 rows × 168 cells × 8 B = 13.0 MiB. The P4 target of
15 MB holds for idle panes (all ≤ 4.75 MiB) and for plain and diff output while live; a
pane that has just received 10,000 densely styled lines is at 17.0 MB until its idle timer
compresses it.

**Scrollback limits — two defaults must be overridden.** A new terminal has
`SCROLLBACK_MAX_BYTES = 10,000` (bytes, `src/terminal/Terminal.zig:272`) and no line
limit; with the default only 424 history rows survived 20,000 lines. Set MAX_BYTES to
NULL (`terminal.h:1372-1374`) and MAX_LINES to the cap. Pruning is page-granular and, contrary to the
header's "almost always higher than configured" (`terminal.h:1383-1386`), keeps **fewer**
rows than the cap: 9,721–9,995 while streaming 11k–20k lines into a 10,000 cap (a deficit
of up to 279 rows). A cap of 10,300 kept 10,031–10,149 rows in the three memory runs. FETCH_HISTORY of 1,000 × 168 cells (cell + style per cell via
`grid_ref`) took 4.3 ms at the top of a 10k scrollback, 6.2 ms in the middle, 9.0 ms at the
bottom; the viewport-scroll + render-state alternative took 8.5 ms per 1,000 rows but moves
the live viewport.

**Snapshot encode/restore across a plyd restart — CONFIRMED for terminal state.** One
process encoded a 168 × 50 pane with 9,747 history rows to 1,747,119 bytes in 1.5 ms and
wrote it to a file; a second process decoded it: READY (renderable) after 0.18 ms, 34
history pages, FINISH after 4.3 ms, formatted text identical. Title, pwd, 2004, DECCKM,
kitty flags, cursor position, the scrollback line cap, the default fg and an OSC 4 override
all survived. Not in the snapshot: callbacks and userdata (re-install them). An unfinished
sequence at encode time needs `OPT_CONTINUATION_MAX_BYTES` set beforehand, otherwise encode
returns `GHOSTTY_INVALID_VALUE`; with it, an OSC 9 split by the snapshot completed after the
restore. The format is "version 1 … work in progress … no binary-compatibility
guarantee" (`snapshot.h:112-113`).

**Search — CONFIRMED.** Needle "needle" over 5,000 lines: 5 case-insensitive matches in
2.1 ms with `ghostty_search_run`; `SELECT_NEXT` with scroll policy NONE selects index 0.

**Throughput (WP3 exit ≥ 300 MB/s) — NOT MET on recorded agent output.** 168 × 50
terminal, 10k scrollback, 64 KiB writes (pty-read size), best of 5–7:

| stream | engine only | engine + Delta at ≤ 120 Hz | engine + Delta after every 64 KiB |
|---|---|---|---|
| recorded Claude Code + Codex captures (188 kB, repeated to 33.7 MB) | 134 MB/s | 128 MB/s | 73 MB/s |
| colour `git log -p` (33.6 MB) | 123 MB/s | 116 MB/s | 67 MB/s |
| synthetic Claude-style TUI frames with 2026 (33.6 MB) | 348 MB/s | 321 MB/s | 107 MB/s |
| CJK / emoji / combining (8.4 MB) | 120 MB/s | 114 MB/s | 68 MB/s |
| plain ASCII 168-column lines (34 MB) | 712–724 MB/s | — | — |

Mode 2027 and write size (4 KiB–1 MiB) stay within run-to-run noise; plain ASCII reaches 994 MB/s with
no scrollback. `sample` puts the time inside the engine: for the recorded agent streams in
`vt_write` parsing, `Screen.clearCells` and `printSliceFill`; for SGR-dense streams in the
SGR parser, the per-page style set and, once scrollback is full, page recycling
(`PageList.grow` → `madvise`, 436 of 2,427 samples). Delta extraction at the 120 Hz cap costs
4–8 %.

**PTY (C5) — CONFIRMED with `rustix` 1.1.5** (`default-features = false`, features `std`,
`pty`, `termios`, `process`, `fs`, `stdio`, plus `event` for the spike's poll loop).
`openpt(RDWR|NOCTTY)`, `grantpt`, `unlockpt`, `ptsname` → `/dev/ttys013`; open the slave
`RDWR|NOCTTY|CLOEXEC`; `tcsetwinsize` on the master; `Command` with `env_clear()` + six
explicit vars, the slave as stdio, and `pre_exec { setsid()?; ioctl_tiocsctty(stdin())?; }`.
`/usr/bin/env` printed exactly the six vars. `/bin/sh` printed the pts path for `tty`,
`33 111` for `stty size`, pid = pgid = tpgid for `ps`, wrote to `/dev/tty`, and read
`ESC[3;4R` for its own `CSI 6n`, answered by libghostty-vt through `WRITE_PTY` → master.
Three macOS facts: `OpenptFlags::CLOEXEC` does not exist on macOS (rustix
`src/pty.rs:40-46`), so set `FD_CLOEXEC` with `fcntl_setfd`; `TIOCSWINSZ` on the master
fails with ENOTTY until the slave has been opened; and once the session leader exits, the
parent's slave fd returns EIO, so each child needs a new pty.

**rustix admission (INV-16) — PASSES**, checked with `gh api` on 2026-09-25:
bytecodealliance/rustix, 2,111 stars, last commit 2026-09-16, 16 contributors with ≥ 5
commits (125 in total), not archived, release 1.1.5.

### 4. What plyd sets on every pane terminal

Before the child starts (R-R4): `OPT_USERDATA`; `WRITE_PTY`; `BELL`; `TITLE_CHANGED`;
`PWD_CHANGED`; `DESKTOP_NOTIFICATION`; `PROGRESS_REPORT`; `CLIPBOARD_WRITE` (never
`CLIPBOARD_READ`); `SIZE`; `COLOR_SCHEME`; `COLOR_FOREGROUND/BACKGROUND/CURSOR/PALETTE` from
`theme.set`; `TERMINFO_NAME` = `xterm-256color` (C5); `SCROLLBACK_MAX_BYTES` = NULL and
`SCROLLBACK_MAX_LINES` = `scrollback_lines` + 300 (the deficit measured at 168 columns);
`CONTINUATION_MAX_BYTES` > 0; `KITTY_IMAGE_STORAGE_LIMIT` = 0 (by default the terminal
answers a Kitty graphics query `OK` and stores up to 10,000,000 bytes of images, while C2
carries no images); and a `ghostty_sys_set(LOG)` hook into `tracing`. herdr also turns off
the glyph protocol (`OPT_GLYPH_PROTOCOL`, `src/ghostty/mod.rs:871`); the spike did not test
it. `setopt_from_terminal` before every key and mouse encode, then re-apply
`MACOS_OPTION_AS_ALT`.

## Consequences

- WP3 can implement the Engine, DeltaBuilder, `encode_input`, snapshot/restore,
  `scroll_history` and `search` directly against the table in Decision 2. Nothing in the
  S5b list needs a local patch, so `patches.md` stays empty (INV-17).
- `ghostty-sys/build.rs` gains a hard rule: every `zig build` it runs uses `--system` with
  a ply-owned package directory and `OUT_DIR` caches; CI needs the eight packages
  (7.5 MB compressed) staged before the first build and no network.
- The terminal is not thread-safe and its callbacks run inside `vt_write`. plyd serializes
  every call per pane (reader thread, input encoders, publisher, compression, snapshot,
  history reads). The render state's two-phase update keeps the lock short
  (`render.h:42-52`). The WRITE_PTY callback must not block (`terminal.h:82-83`): a
  synchronous write to a master whose child is itself blocked writing output can deadlock
  the pane, so answers go to a bounded per-pane write queue.
- A scroll produces a FULL Delta (all 50 rows): 8,400 cells × 7 bytes ≈ 59 KB per frame
  per streaming pane at the 120 Hz cap. Bounded, but the reason a SCROLL op may pay off
  later.
- The ≥ 300 MB/s WP3 exit criterion cannot be met as written on this machine: recorded agent
  output runs at 134 MB/s (engine alone, loaded machine), and the time is inside
  libghostty-vt (`sample` over all three modes: 3,032 of 3,214 samples inside
  `ghostty_terminal_vt_write`, 126 inside ply's DeltaBuilder). For scale,
  the S2b Claude Code captures average 0.3–1.3 kB/s between their first and last hook;
  P1 itself (six panes at ten times real speed) is measured by WP11's replay bench. The
  owner decides the new threshold (spec delta 12).
- P4 holds for idle panes only because of compression, so plyd must run the idle
  compressor; a pane that has just scrolled 10,000 densely styled lines stays over 15 MB
  until then.
- Snapshots work across a plyd restart; across a libghostty-vt upgrade they are not
  guaranteed to decode, so persisted snapshots record the pin and are discarded (start
  blank) when it changes or decoding fails.
- macOS pty rules (CLOEXEC by `fcntl`, winsize after the slave is open, one pty per child)
  go into `crates/daemon/src/pty.rs`.

## Spec delta

VERIFY S5b items (WP0 list and the rules that name S5b):

| Spec item | Result |
|---|---|
| WP0: ghostty-sys builds it with the pinned Zig for aarch64-apple-darwin | CONFIRMED (Zig 0.16.0; command and times in Decision 1) |
| 2 Build toolchain: build.rs "the way herdr's build.rs does" | CORRECTED: herdr writes `zig-out/` and `.zig-cache/` into the vendor tree and Zig 0.16 adds `zig-pkg/`; ply adds `--system <pkgdir>`, `-fno-sys=` ×11 and `OUT_DIR` prefix/caches, and links a static-only directory |
| WP0: render state reports dirty rows a DeltaBuilder turns into Deltas | CONFIRMED (dirty set = touched rows plus the row the cursor left; scroll, alternate screen and resize are FULL) |
| WP0: snapshot encode/restore works across a plyd restart | CONFIRMED for terminal state (cross-process); callbacks are re-installed; format has no compatibility guarantee across library versions |
| R-R4 and WP0: colour and device queries answered from ply's palette | CONFIRMED; CORRECTED: the palette is `OPT_COLOR_*` (`terminal.h:1199-1227`), not `vt/color_scheme.h`; answers leave through `OPT_WRITE_PTY`; OSC 10/11 are unanswered unless fg/bg are set |
| C8 and WP0: OSC 7 and OSC 9 reachable | CONFIRMED through `OPT_PWD_CHANGED` and `OPT_DESKTOP_NOTIFICATION`; CORRECTED: the standalone `vt/osc.h` path cannot deliver the payloads; OSC 9 bodies starting `1;`–`12;` are ConEmu commands |
| WP0: key, mouse, focus and paste encoders cover R-R5 to R-R10 | CONFIRMED; C2 fields CORRECTED below |
| R-R18 and WP0: DEC 2026 handling | CONFIRMED: polled `DATA_MODE` 2026, no callback; plyd holds Deltas and caps with `OPT_MODE` false |
| R-R22 and WP0: idle-scrollback compression | CONFIRMED (`compression_activity` + `compress(INCREMENTAL)`, COMPLETE on macOS) |
| P4 and WP0: bytes per pane at 10 000 × 168 | MEASURED: 13.5–14.0 MiB live for plain/diff output, 16.2 MiB for SGR-dense output, 0.7–4.8 MiB idle-compressed |
| C5 and WP0: `rustix::pty` + `pre_exec` gives a controlling tty and exactly ply's env | CONFIRMED (rustix 1.1.5; macOS rules in Decision 3) |
| R-R12: reflow on resize | CONFIRMED: primary reflows 20→40→10 columns, alternate does not |
| R-R14: wide, spacer, grapheme from the grid | CONFIRMED; grapheme clusters need mode 2027 on |
| R-R6: ⇧⏎ → LF without kitty | CONFIRMED as a plyd rule: the legacy encoder emits `ESC[27;2;13~`, kitty `ESC[13;2u` |
| R-R11: OSC 52 writes allowed, reads refused | CONFIRMED by installing only `OPT_CLIPBOARD_WRITE` |
| 2 Terminal emulation: 8-byte cells, SIMD, snapshots, search, encoders | CONFIRMED (`GhosttyCell` = 8 bytes in the manifest, `page.zig:2143`; SIMD on at run time) |
| 6.2: Codex probes answered by plyd's terminal within 250 ms | CONFIRMED on the recorded probe (2.9 µs) |

Changes this ADR proposes (spec 6.0.0, pending the controller's merge with ADR-0003/0004):

1. **4.2 KEY** becomes `{key:u16 (GhosttyKey 0–175), mods:u16, consumed_mods:u16,
   action:u8 (0 release, 1 press, 2 repeat), flags:u8 (bit 0 composing),
   unshifted_codepoint:u32, text:utf8}`. `mods` needs 10 bits (`key/event.h:57-91`), the
   option-as-alt rule needs the side bits and `consumed_mods`, and kitty's
   alternate-key reporting needs `unshifted_codepoint`. (Contract change.)
2. **4.2 MOUSE** becomes `{action:u8 (press, release, motion), button:u8 (0 = none,
   1–11), x:f32, y:f32 (pixels from the terminal's top-left), mods:u16}`; plyd keeps the
   pressed-button state and the geometry. `col/row` cannot feed SGR-pixels (1016).
   ATTACH/RESIZE `px_w/px_h` must be defined as the **cell** size in pixels, the value
   `ghostty_terminal_resize` takes (`terminal.h:2026`). (Contract change.)
3. **4.2 cell flags** gain `spacer_head` (a wide character wrapped to the next line,
   `screen.h:112-113`); **style `attrs`** is a `u16` (eight booleans plus a 3-bit underline
   kind: none, single, double, curly, dotted, dashed).
4. **4.2 PASTE** needs a rule for `GHOSTTY_REJECTED` (a multi-line paste into a pane without
   2004): either an `allow_unsafe` flag on PASTE plus a plyd → app rejection frame, or a
   fixed plyd policy. Open for WP2.
5. **R-R4**: "The exact callbacks (`vt/color_scheme.h`, `vt/device.h`,
   `vt/size_report.h`)" is replaced by the list in Decision 4; it adds "the palette is set
   before the child starts, or OSC 10/11 stay unanswered".
6. **C8**: "from the terminal's own callbacks if it reports these sequences, otherwise with
   its standalone OSC parser" becomes "from `OPT_PWD_CHANGED` and
   `OPT_DESKTOP_NOTIFICATION`"; ADR-0004's classifier must treat a body that starts with a
   ConEmu sub-command as not a notification.
7. **R-R18**: "Its exact reporting is VERIFY S5b" becomes "plyd reads mode 2026 after each
   write batch, holds Deltas while it is set, and clears it with `OPT_MODE` after 150 ms".
8. **R-R22**: add "`SCROLLBACK_MAX_BYTES` is cleared and `SCROLLBACK_MAX_LINES` is
   `scrollback_lines` + 300, because page-granular pruning keeps fewer rows than the cap".
9. **R-R14**: add "plyd sets mode 2027 as the reset default". Decision for the owner:
   Ghostty and herdr cluster by default, but a CLI that measures with wcwidth then
   disagrees on the width of ZWJ sequences.
10. **2 Terminal emulation** pin: "libghostty-vt 1.3.2-HEAD" is Ghostty's source version;
    the library reports `0.1.0-dev`. Wording.
11. **8 ledger, `crates/ghostty-sys`**: the header list becomes `terminal.h`, `render.h`,
    `screen.h`, `style.h`, `modes.h`, `device.h`, `size_report.h`, `snapshot.h`, `key.h`,
    `mouse.h`, `focus.h`, `paste.h`, `search.h`, `grid_ref.h`, `point.h`, `sys.h`,
    `types.h`; `osc.h` and `color_scheme.h` are not needed.
12. **WP3 exit criterion** "at least 300 MB/s of recorded output": measured 128 MB/s
    (engine + Deltas at 120 Hz) on the recorded Claude Code and Codex corpus. Proposed:
    "≥ 100 MB/s on the recorded agent corpus on the reference machine", with P1 as the
    binding end-to-end requirement. Owner decides.
13. **P4** is read as the idle-compressed footprint, and plyd's idle compressor becomes
    part of R-R22's "compressed when idle" rather than optional.
14. **C2 TITLE**: Codex puts its spinner in the title (about 100 changes in a 32 kB
    capture); TITLE frames are coalesced to the Delta cadence.
