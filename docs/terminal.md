# The terminal

How a pane's bytes become the rows on screen, and how keys get back. plyd owns
the pty and emulates it with libghostty-vt; the app receives decoded rows over
C2 and draws them with GPUIX. Raw pty output never enters JavaScript (INV-2),
and the app never mirrors terminal modes: it sends what the user did, and plyd
encodes it against the pane's live state.

```
child ──pty──► reader thread ──► pane task: Engine.write ──► dirty rows ──► DeltaBuilder ──C2──►
                                   │ replies (DA, DSR, OSC 10/11 …) ─► pty writer thread
data-client ──► Replica (a version per row) ──► TerminalRow: <text> runs ──► GPUIX
keys, mouse, paste, focus ──C2──► encode_input (against the live modes) ──► pty writer thread
```

`docs/screen-protocol.md` is the wire between the two halves.

## libghostty-vt

The terminal engine is libghostty-vt, Ghostty's terminal library, built from
ghostty commit 44f2a44 (`44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`) with no
local patches. It is not vendored: `crates/ghostty-sys/build.rs` downloads that
commit's source, verifies it and builds it (Ruling R45). Only plyd links it:
`ghostty-sys` is reached only through ply-term's `engine` feature, and only
ply-daemon enables that (INV-17, `checkInv17Ghostty`).

### The build

`crates/ghostty-sys/build.rs` runs while cargo builds `ghostty-sys`:

- **The source.** The build fetches
  `https://codeload.github.com/ghostty-org/ghostty/tar.gz/<commit>` for the
  pinned commit, checks the archive's SHA-256 against the value pinned in
  `build.rs` (`7bd1a8b6…a779`) and refuses a mismatch, and keeps the extracted
  tree in `~/Library/Caches/ply/ghostty/<commit>/`, so the download happens once
  per machine. `PLY_GHOSTTY_SRC=<dir>` builds from an existing ghostty checkout
  at that commit instead, which is how a build runs offline. The download is
  extracted beside the cache entry and renamed into place whole, so a concurrent
  or interrupted build never sees half a tree.
- **Zig** is `$ZIG`, else `zig` on `PATH`, and must be 0.16.0, the
  `minimum_zig_version` of the pin; anything else fails with the version found.
  The Zig packages the library needs are fetched by the build (`zig fetch`,
  whose package hash must match the one pinned in `build.rs`) when Zig's cache
  lacks them; `PLY_ZIG_PKG_DIR=<dir>` supplies them extracted by hash instead.
- **Options.** `zig build -Demit-lib-vt`, with `-Doptimize=` from
  `LIBGHOSTTY_VT_OPTIMIZE` (`Debug`, `ReleaseSafe`, `ReleaseFast` or
  `ReleaseSmall`; default `ReleaseFast`) and `-Dtarget=` mapped from the Rust
  target (`aarch64-apple-darwin` → `aarch64-macos`, `x86_64-apple-darwin` →
  `x86_64-macos`). The build writes only under cargo's `OUT_DIR`, the cache
  directory above and, for a missing package, Zig's global cache; never into the
  source tree.
- **Linking.** Zig installs a static archive and a dylib side by side; the
  build links `libghostty-vt.a` statically (`links = "ghostty-vt"`), so plyd
  carries the engine inside its own binary.
- A cold build of the library takes about a minute, once per profile per target
  directory.

`crates/ghostty-sys/build.rs` is the reference for the exact flags and the
package handling.

`crates/ghostty-sys/src/lib.rs` declares by hand the part of the C API ply uses
(`terminal.h`, `render.h`, `screen.h`, `style.h`, `modes.h`, `device.h`,
`size_report.h`, `snapshot.h`, the key and mouse headers, `focus.h`, `paste.h`,
`search.h`, `grid_ref.h`, `point.h`, `sys.h`, `types.h`, and `formatter.h` for
tests). `crates/ghostty-sys/tests/layout.rs` checks every declared struct's size
and field offsets and every enum constant against the library's own ABI
manifest (`ghostty_type_json()`), and that ply-proto's key, mouse, cursor and
underline numbering match the library's; an upgrade of the pin reruns it.

The ReleaseFast library does not range-check enum arguments or float-to-int
conversions, so an undefined key code or a non-finite pointer position would be
undefined behaviour inside it. ply-term therefore validates every such value
twice: in `crates/term/src/input.rs`, with a log line, and again at the unsafe
boundary in `crates/term/src/engine/encoders.rs`.

## The engine

`ply_term::Engine` (`crates/term/src/engine.rs`) is one pane's terminal behind a
safe API. It is `Send` and not `Sync`; each pane's task owns its engine, so
every call on it is serialized without a lock. Nothing in ply-term does I/O or
blocks.

**What plyd sets on every engine**, before the child's first byte:

| Option | Value | Why |
|---|---|---|
| palette | the `theme.set` palette: fg, bg, cursor and the 256-colour table (0–15 from the theme's ANSI colours, 16–231 the xterm cube, 232–255 the grey ramp) | the terminal answers colour queries itself; programs' OSC 4 overrides stay until OSC 104 |
| colour scheme | dark when the background's relative luminance is below one half | answers `CSI ? 996 n` |
| XTVERSION | `ply <version>` (`XTVERSION`, from ply-term's crate version) | answers `CSI > q` |
| mode 2027 (grapheme clustering) | on, as the reset default | wide emoji and ZWJ clusters take the cells Ghostty gives them |
| terminfo name | `xterm-256color` | answers XTGETTCAP `TN`; plyd sets `TERM` to the same |
| scrollback | no byte cap; a line cap of `scrollback_lines` + 300 (`SCROLLBACK_SLACK`) | the library prunes whole pages and keeps fewer rows than asked |
| continuation | 64 KiB of an unfinished escape sequence kept | a saved state restores mid-sequence |
| OSC 5522 clipboard writes | at most 4 MiB (`CLIPBOARD_WRITE_MAX_BYTES`) | the library's default is 64 MiB |
| Kitty graphics | storage limit 0 | C2 carries no images |
| Glyph Protocol | off | C2 cannot carry registered glyphs, so the terminal must not advertise them |
| ⌥ as Alt | from `option_as_meta` | the key encoder's macOS option-as-alt |
| log | the library's log goes to `tracing`, target `libghostty_vt` | |

A new engine is sized to the last grid and cell size any client attached or
resized with, or 80 × 24 cells of 8 × 16 px before any has. `Engine::new` alone
leaves the cell size at 0, and size queries stay unanswered until a resize sets
it (`size_reports_wait_for_a_cell_size`), which is why plyd resizes every engine
before its child starts.

**What the terminal answers itself** (the bytes go back to the pty through the
write queue; `crates/term/tests/engine.rs`,
`colour_and_device_queries_are_answered_from_the_palette`):

| Query | Answer |
|---|---|
| DA1 `CSI c` | `CSI ? 62 ; 22 c` |
| DSR `CSI 5 n` / cursor position `CSI 6 n` | `CSI 0 n` / `CSI <row> ; <col> R` |
| `CSI ? u` (kitty keyboard flags) | `CSI ? <flags> u` |
| OSC 10, 11, 12 `?` and OSC 4 `;<n>;?` | the palette's colour as `rgb:rrrr/gggg/bbbb` |
| `CSI 18 t`, `CSI 16 t`, `CSI 14 t` | the grid in cells, the cell in pixels, the grid in pixels (the pixel ones only once the cell size is known) |
| mode 2048 on | an in-band size report on every resize |
| `CSI ? 996 n` | `CSI ? 997 ; 1 n` for a dark palette, `; 2` for a light one |
| `CSI > q` (XTVERSION) | `DCS > \| ply <version> ST` |
| DECRQM `CSI ? <mode> $ p` | the mode's state (2027 set, 2026 reset when idle) |
| XTGETTCAP `TN` | `xterm-256color` |

It does not answer ENQ, title reports (`CSI 21 t`) or Kitty graphics queries.
This is what makes Codex's startup probe work before any view is attached
(`the_codex_startup_probe_is_answered_at_once`).

**What else comes out of a write** (`EngineOutput`) and what plyd does with it
(`crates/daemon/src/panes/pane.rs`, `on_effects`):

| Output | plyd |
|---|---|
| `reply` | queued for the pty |
| `bells` | a BELL frame at the next update |
| `title` (OSC 0, 2) | stored as the pane's title (an empty title falls back to the CLI or shell name) and sent as TITLE at the next update |
| `pwd` (OSC 7 `file://host/path`) | percent-decoded to the pane's `cwd` and announced with `pane.meta`; other schemes are ignored |
| `notifications` (OSC 9, OSC 777) | an OSC 9 body goes to the pane's agent session (C8: approval, question, plan prompt or turn complete; `docs/agents.md`); every notification is also logged |
| `progress` (OSC 9;4) | unused |
| `clipboard_writes` (OSC 52) | the `text/plain` part (UTF-8, at most one frame, 1 MiB) goes at once to every attached client as CLIPBOARD_WRITE, which the app writes to the pasteboard; a write while no client is attached is dropped. Clipboard reads get no callback, so they are refused |

**Dirty tracking.** libghostty-vt's render state consumes the terminal's dirty
flags, so each engine owns one render state and records, per viewport row, the
generation at which it last changed. Every attached client's `DeltaBuilder`
sends the rows newer than its own last frame, so several clients never steal
each other's changes and a blocked client catches up correctly.

**Absolute lines.** The library does not count the lines its line cap prunes,
so after every write and resize the engine reads how far a tracked grid
reference at the top of the live screen moved up, adds that to
`scrollback_base`, and moves the reference back to the top (on the primary
screen only). A write that adds more lines than the scrollback holds loses the
reference; the base then skips every line the terminal held. The bench shows no
cost (engine alone 134 MB/s on the agent corpus, 707 MB/s on ASCII).

**The rest of the engine**: `resize` reflows the primary screen and ends an open
DEC 2026 update; `compress_idle` compresses idle scrollback in bounded steps;
`search` finds matches in the screen and scrollback (in absolute lines); `save`
and `restore` serialize the whole state, unfinished sequences included (not used by plyd yet:
screens do not survive a plyd restart).

## The pane process and its task

`crates/daemon/src/pty.rs` opens a pty pair with rustix, starts the child with a
cleared environment (`docs/agents.md` lists it), the slave as its stdin, stdout
and stderr, and one audited `pre_exec` block that makes it a session leader
with the pty as its controlling terminal (`setsid`, `TIOCSCTTY`) and closes
every descriptor from 3 up that is not close-on-exec, so nothing plyd or a
library opened without `CLOEXEC` reaches the pane's program; the loop's bound,
the highest descriptor open in plyd (from `/dev/fd`, plus a margin of 64), is
computed before the fork. Three std threads per pane do the blocking work:

- a **reader** that reads up to 64 KiB at a time into a channel of 64 chunks;
- a **writer** that drains a channel of 64 writes into the master, so a child
  that stops reading its input never blocks plyd; a write that encodes a KEY
  frame carries the frame's arrival time and logs the P2 latency (`P2: key
  frame to pty write`, `latency_us`) at debug level;
- a **waiter** that reaps the child and reports its exit code (128 + signal for
  a signal death).

The pane task (`crates/daemon/src/panes/pane.rs`) is the only owner of the
engine, the pty channels and the attached clients. It feeds output into the
engine in batches of up to 512 KiB before it publishes and looks at other work;
queues replies and encoded input for the writer without waiting on it (at most
16 MiB pending; more is dropped and logged); publishes under the rules of
`docs/screen-protocol.md`; writes `last_activity_at` at most every 5 s; and
publishes the exit after the process's last output, waiting up to 250 ms for
the pty's end-of-file. A kill is SIGHUP to the process group and SIGKILL 2 s
later.

**Idle compression.** When the engine's compression-activity token has not moved
for 10 s, the task compresses scrollback in 5 ms slices until it is done; an
attach or a history read restarts the wait. Measured below: a full pane drops
from 14–17 MiB to 1.3–4.8 MiB.

## Input, encoded in plyd

The view sends KEY, MOUSE, PASTE and FOCUS frames; `ply_term::encode_input`
(`crates/term/src/input.rs`) turns each into pty bytes against the pane's live
modes, through libghostty-vt's encoders. Each encode first copies the
terminal's current modes into the encoder (kitty keyboard flags, cursor-key
mode, mouse mode and format), so the bytes are always what the program asked
for.

**Keys.**

- A key code above 175, an action out of range or a composing key encodes
  nothing. Modifier bits are masked to the ten defined ones; an unshifted
  codepoint that is not a Unicode scalar value is sent as 0.
- ⌥ acts as Alt only on the sides `option_as_meta` names; otherwise ⌥ types the
  layout's character and the view reports ⌥ as consumed.
- **⇧⏎ is LF.** Return with Shift as the only held chord modifier (Caps Lock and
  Num Lock ignored), pressed or repeated and not composing, sends `0x0A` — the
  byte both CLIs read as "insert newline" — while the pane has not enabled the
  kitty keyboard protocol. With kitty flags set the encoder's own report is
  sent (`CSI 13 ; 2 u`); ⇧⌃⏎ always goes to the encoder (`CSI 27 ; 6 ; 13 ~` in
  legacy mode).
- Releases encode nothing in legacy mode and are reported under kitty's
  release flag.

**Mouse.** Encoded from the pixel position with the engine's cell size (the
frame's `col` and `row` are not used), for whatever tracking mode and format
the program enabled; with no tracking mode on, nothing is sent. Nothing is
encoded before the first cell size is known or for a non-finite position; huge
positions are clamped to twice the surface. Pressed buttons are tracked per
pane so drags report correctly (wheel buttons 4–7 are momentary); the held
state is reset when a client detaches and when focus is lost.

**Paste.** libghostty-vt's paste: control bytes are replaced with spaces, the
text is wrapped in bracketed-paste sequences when the program enabled mode 2004,
and newlines become carriage returns when it did not. Text that could inject
commands — a newline without bracketed paste, or the bracketed-paste terminator
with it — is refused unless the frame's `allow_unsafe` is set, and plyd answers
PASTE_REJECTED. Kitty's paste events (mode 5522) are not offered, because ply
installs no clipboard-read callback.

**Focus.** `CSI I` and `CSI O`, only while the program enabled mode 1004.

**INPUT_RAW** bytes, and `pane.answer`'s `1` or ESC, go to the pty unencoded.

## The view

The app side lives in `app/src/terminal/` and
`app/src/features/panes/terminal-view.tsx`.

**Mounting.** A TerminalView exists only for a visible pane: the active tab's
panes, only the focused one while the tab is zoomed
(`app/src/features/panes/pane-grid.tsx`, `visiblePaneIds`). The grid lays them
out by count (Ruling R56, `gridShape`): one fills the tab, two and three are
equal full-height columns, four are equal quadrants, in position order. Every
count uses the same grid parent, so a re-flow (a pane added or closed) moves
the views without remounting them: each one measures its new box and sends a
RESIZE, not a new ATTACH. Switching tabs
unmounts the old views, which detach, and mounts the new ones, which attach and
receive a Snapshot each. Hidden panes keep running and emulating in plyd; their
status keeps arriving over C1.

**Title, bell and exit** reach the pane header through TerminalView's callbacks
(`pane-frame.tsx`): a TITLE replaces the pane's name, a BELL marks a pane that
is not the focused one with a bell until it is, and an EXIT marks the pane
`exited` with its code at once (C1 `pane.exit` says the same a moment later).

**A narrow header degrades.** The header measures its own width the way the
view does (every 250 ms) and, in unscaled pixels (its width over the font
scale, since everything grows with ⌘=), drops the plan's numbers below 480,
the model below 400, the plan bar below 330, the branch below 290 and the CLI
label below 240; the title and the branch ellipsize, and the header clips
rather than letting items overlap (`pane-header.tsx`, `headerFit`). The status
bar's key hints wrap onto a hidden second line, so hints that do not fit drop
whole from the right (`large-font.test.tsx` checks both at the largest font
size).

**Size.** The view measures its box every 250 ms (every 16 ms until the first
measurement) and sends the whole cells that fit (`gridFor` in
`app/src/terminal/metrics.ts`): ATTACH the first time, RESIZE when the grid or
cell size changes. The cell comes from the terminal font's own tables:

| Font | Cell width | Cell height |
|---|---|---|
| Geist Mono | 0.6 × size (the advance of `m`) | round(1.3 × size) |
| Menlo | 1233/2048 × size | round(2384/2048 × size) |

The height is the font's ascent plus descent, rounded to whole pixels, which is
the line height at which its box-drawing glyphs join. The width stays
fractional; ATTACH and RESIZE carry it rounded to a whole pixel. Geist Mono is
used only when the Geist fonts are installed (in `~/Library/Fonts` or
`/Library/Fonts`; the TTFs are in `app/assets/fonts/`, and `just fonts` installs
them), because GPUIX cannot
load a font file; otherwise the terminal uses Menlo and the chrome the system
font (`app/src/ipc/os.ts`, `geistAvailable`).

**The replica** (`app/src/terminal/replica.ts`) is the app's copy of the screen.
It applies Snapshot, Delta and History frames and is strict like ply-term's
reference `Replica` (`crates/term/src/replica.rs`): a frame with a sequence
number that does not increase, a style id that is 0 or already known, an unknown
style or a row outside the grid throws `ReplicaError` and changes nothing, and
the view drops the connection and re-attaches. Each row keeps its cells as typed
arrays, a version that changes whenever the row is replaced, and an FNV-1a hash
of its cells and the style epoch (bumped by every Snapshot, which may reuse
ids). Scrollback rows it fetched are kept by absolute line number
(`scrollback_base` onwards, `docs/screen-protocol.md`), so they stay right after
the scrollback cap starts dropping lines, and a Delta whose base moved on drops
the lines plyd no longer keeps.

**Rendering.** Every pane's changes are flushed by one shared timer, at most
once per 16 ms and at once for the first change after an idle frame, so all
panes re-render in one React batch (`app/src/terminal/session.ts`, `schedule`):
plyd publishes up to 120 Hz, and the view draws at most 60. A row is one `TerminalRow`
(`terminal-row.tsx`), memoized: it re-renders only when its cells, the selection
columns, the style epoch, the resolver or the cell size changed. Rows are keyed
by content hash, so a scroll moves the rows it kept instead of re-rendering every
position.

A row draws as the fewest `<text>` runs (`rowRuns` in `app/src/terminal/runs.ts`):

- cells of one resolved style merge into one run;
- a wide character, a grapheme cluster, and any glyph Geist Mono does not draw
  at exactly one cell (`isNarrowGlyph`, a table taken from the font) get a run
  of their own, one or two cells wide, so a fallback glyph cannot push the
  columns after it;
- spacer cells are skipped and trailing default blanks dropped (a blank that
  paints a background, or a selected one, stays).

Each run is positioned absolutely at `round(col × cell width)` with the width to
the next rounded edge, so layout rounding cannot drift columns across a row.
Every host node costs GPUIX about 0.01 ms per frame, which is why runs are
coalesced: a test holds a 4-pane tab of 160 × 50 recorded agent screens under
2 000 host nodes, and the chrome alone under 400.

**Colours and attributes** (`StyleResolver`): colours resolve against the
theme (`default` → the theme's fg or bg, indexed 0–15 → its ANSI colours,
16–255 → the xterm cube and greys, RGB as sent). Inverse swaps fg and bg; faint
blends fg halfway into the background; invisible paints fg in the background
colour; the selection overrides both with the theme's selection colours; bold
is weight 600; any underline kind draws as an underline, else strikethrough as
a line-through. GPUIX styles no italic, overline or underline colour, and blink
is not drawn. The default background is never painted, so the pane body's
colour shows through.

**The cursor** is a positioned `<div>`: a bar, an underline, or a block that
draws the character under it in the theme's cursor-text colour. It shows once a
Snapshot has arrived, when the cursor is visible and the program has DECTCEM on,
and while the live row it sits on is on screen. An unfocused pane draws a hollow
block. A blinking cursor blinks at 530 ms while the pane has focus and the
system's reduce-motion setting is off, and every screen change restarts the
blink.

## Selection, clipboard, scrollback and search

These are ply's own, done in the app against the replica.

**Selection.** Dragging with the primary button selects by cell; a double click
by word (letters, digits and `_-./~:@%+#`), a triple click by line; ⇧-click
extends. Dragging past the top or bottom scrolls. A click without a drag clears
the selection, and so does typing. While the program reports the mouse, clicks
go to the program and ⇧ bypasses that to select.

**Copy and paste.** ⌘C copies the selection — after fetching any scrollback it
covers, for up to 5 s — joining rows with newlines except after a soft-wrapped
row and trimming each line's trailing blanks; with no selection it does nothing
and never sends ^C. ⌘V pastes the pasteboard's text as a PASTE frame. The
pasteboard is reached through `pbcopy` and `pbpaste` (`app/src/terminal/host.ts`),
because GPUIX gives JavaScript no clipboard API; under `bun test` an inert host
never touches it. When plyd refuses a paste as unsafe the view asks, and ⏎
resends it with `allow_unsafe` while esc drops it. A program's OSC 52 write
arrives as CLIPBOARD_WRITE and goes to the pasteboard the same way (an empty
text clears it); a program can never read the pasteboard.

**Scrollback** stays in plyd (`scrollback_lines`, default 10 000). The wheel
scrolls ply's own view of it and the session fetches the pages it shows
(`docs/screen-protocol.md`); new output keeps a scrolled-back view in place,
and a key press returns to the live screen. On the alternate screen there is no
scrollback, and without mouse reporting the wheel sends arrow keys instead.
With reporting on, the wheel goes to the program as buttons 4 and 5, at most
ten steps per event. ⌘A selects every line plyd keeps and starts fetching them.

**Search.** ⌘F opens a find bar and fetches the whole scrollback. Matching is a
case-insensitive substring search over the screen and the fetched lines; the bar
starts at the newest match, ↑ and ⏎ step to older ones and ↓ to newer ones, and
each match is scrolled into view and selected. Esc closes it and returns focus
to the pane. Keys typed into the find field never reach the pty: the field
marks each key it handled, and the terminal skips that event when it bubbles up.
(The engine's own `search` is not used by the app.)

**IME** is not implemented: every KEY frame carries `composing` 0 and the text
GPUIX reports for the key, so dead keys and CJK composition do not work in a
pane.

## Input in the view

`app/src/terminal/input.ts` turns GPUIX key events into KEY frames:

- **⌘ chords never become KEY frames** (`keyFrame` returns `null`); they belong
  to the keymap, and the four the terminal handles itself are in
  `docs/keybindings.md`.
- The physical key comes from GPUIX's key name through `GHOSTTY_KEYS`, the
  libghostty-vt key table in enum order (`input.test.ts` checks it name for name
  against `key/event.h`); AppKit's shifted US characters fold back to their key.
  A layout key with no physical code (`ö`) is key 0 with its text.
- Text is the character GPUIX reports, dropped when it is a control character
  or when Ctrl is held (plyd encodes Ctrl chords from the key). With ⌥ as Meta
  the key's own character is sent with ⌥ unconsumed, so plyd prefixes ESC;
  otherwise ⌥'s layout character is sent with ⌥ consumed. GPUIX reports no key
  side, so `left` and `right` both act on either ⌥ (`right` also sets the
  right-Option bit).
- Presses, repeats and releases are all sent.

Mouse events become MOUSE frames only while the program reports the mouse
(motion only when the cell under the pointer changes); focus changes become
FOCUS frames once the view is attached.

## Measured

On a 168 × 50 pane with 10 000 lines of scrollback, written in 64 KiB chunks
with a Delta built at most every 1/120 s (`cargo bench -p ply-term --features
engine --bench throughput`, release, best and median of seven runs, on a loaded
machine):

| Stream | Engine alone | Engine + Deltas | Engine + Deltas + `Frame::encode` |
|---|---|---|---|
| recorded agent corpus (15 Claude Code and Codex captures, 188 223 bytes, repeated to 33.7 MB) | 135.7 / 134.8 MB/s | 133.1 / 132.7 MB/s | 98.9 / 95.8 MB/s |
| plain ASCII, 168-column lines (33.6 MB) | 703.7 / 686.0 MB/s | 708.8 / 669.3 MB/s | — |

The exit criterion is at least 100 MB/s on the agent corpus and 300 MB/s on
ASCII, engine plus Deltas. The corpus captures are not committed; without them
the bench uses the scrubbed `crates/term/tests/fixtures/*-session.bytes` and
says so (`PLY_AGENT_CORPUS_DIR` points it elsewhere).

Memory per pane, six 168 × 50 panes filled at once, each with a client after
one Snapshot (`crates/term/tests/memory.rs`, macOS `phys_footprint`):

| Content | Scrollback rows | Live | After idle compression |
|---|---|---|---|
| plain ASCII | 10 001 | 14.16 MiB | 1.32 MiB |
| SGR-dense text with CJK and emoji | 10 157 | 16.53 MiB | 4.79 MiB |

The test asserts at most 15 MB per pane after compression.

The view records its own cost per pane: decode time per socket read
(`DecodeStats` in `data-client.ts`) and render time per commit (`RenderStats` in
`session.ts`), read through `terminalStats()` and shown in the view's corner
with `PLY_TERMINAL_STATS=1`. The target (P1) is a p99 frame time within 16.6 ms
with six streaming panes. A debug plyd streams the recorded agent output into
live panes for that measurement (`plyd --replay`, `crates/daemon/src/replay.rs`;
`docs/development.md` shows the run). The numbers below were measured for the
terminal view (WP5) with the npm GPUIX build, Menlo, on a machine loaded by
parallel builds (load 3–40), single runs.

**Decode** stays on the main thread (no Worker): a worst-case 1 MiB Snapshot
(300 × 495 cells, 1 043 094 bytes) decodes in 0.82 ms p50, 1.52 ms p90, 6.6 ms
max (JIT warm-up); a 160 × 50 agent screen in 0.04 ms; live frames take
0.01–0.03 ms on average, 5.8 ms once.

**Test renderer**, 1440 × 900, every row of every pane changing every frame (the
160 × 50 recorded screens rotated by a row per frame):

| Panes | Host nodes (chrome) | GPUI draw p50 / p99 | React per pane, average |
|---|---|---|---|
| 1 | 266 (20) | 2.0 / 2.35 ms | 0.64 ms |
| 4 | 972 (72) | 7.1 / 8.2 ms | 0.76 ms |
| 6 | 1 456 (106) | 11.0 / 11.9 ms | 0.77 ms |

Before rows were keyed by content, React took 1.6–4.2 ms per pane for the same
load. The node-count test (a 4-pane tab of 160 × 50 recorded screens) asserts
fewer than 2 000; it is 972.

**Live** (GPUI draw time from GPUIX's frame overlay; "JS batch" is one React
render of every pane):

| Load | Deltas/s per pane | Draw p99 / max | JS batch, average |
|---|---|---|---|
| 1 shell, `yes \| head -c 50M` | about 120 | 1.4 / 9.0 ms | 1.4 ms |
| 4 shells, `yes \| head -c 50M` (60 Hz render cadence) | about 120 in, 60 drawn | 3.0 / 11.3 ms | about 2.1 ms |
| 6 replayed agent panes, `plyd --replay … --speed 10` (40 KiB/s each) | 35–85 | 5.95 / 55 ms | about 5.4 ms (max 63 ms: the six first Snapshots) |

P1 holds with six replayed panes (draw p99 5.95 ms plus about 5.4 ms of
JavaScript); the one 55 ms frame is six panes attaching at once, so "no frame
over 33 ms" is not met at attach.

**P2**, key to the pty: in the app, building and encoding a KEY frame takes
0.002 ms p50, 0.02 ms p99. KEY frame to the echoed Delta from plyd (shell echo
and plyd's 120 Hz publisher included, six replayed panes streaming) takes
0.48 ms p50, 1.93 ms p90, 8.97 ms p99, 15 ms max (n = 200), an upper bound of
P2. plyd measures the part it owns itself: with `PLY_LOG=debug` every KEY frame
logs `P2: key frame to pty write` with `latency_us`, from the data server
decoding the frame to the pty writer's `write(2)` returning. A release plyd,
300 keys 5 ms apart into a shell pane, measured 21 µs p50, 52 µs p90, 231 µs
p99 and 509 µs max, and 21 / 34 / 62 / 78 µs while another pane streamed
`seq` output to its own client.

## Tests

- `crates/term/tests/delta.rs`:
  `a_replica_fed_random_snapshots_and_deltas_equals_the_engine` (24 seeds × 300
  random bursts of text, SGR, cursor moves, erases, scrolls, wide and combining
  characters, the alternate screen and resizes, with two clients acking at
  different rates; both replicas must equal a fresh Snapshot every 25 steps),
  and the Delta, style, cell, resize and history-page rules.
- `crates/term/tests/replay.rs`: recorded Claude Code (80 × 24) and Codex
  (120 × 40) sessions replay through the engine to their stored screens, and the
  Delta-fed replica matches libghostty-vt's own formatter at every fourth chunk.
  `PLY_BLESS=1` rewrites the stored screens after an intended change.
- `crates/term/tests/engine.rs` (query answers, effects, DEC 2026, compression,
  search, save and restore), `input.rs` (the key, ⇧⏎, ⌥, mouse, focus and paste
  rules, and the guards against undefined values), `memory.rs`.
- `crates/ghostty-sys/tests/layout.rs`.
- `app/src/terminal/replica.test.ts` (recorded agent streams rebuild the
  engine's final screen; bad frames change nothing), `runs.test.ts`,
  `input.test.ts`, `frames.test.ts`, `data-client.test.ts`.
- `app/src/features/panes/terminal-view.test.tsx` with GPUIX's test renderer:
  attaching with the measured grid, painting a recorded Claude Code screen as
  style runs, the cursor, title, bell and exit callbacks, reconnecting,
  resizing, keys (⌘ chords never sent), focus, paste and its confirmation,
  selection and ⌘C, mouse reporting with ⇧ bypass, wheel scrollback, ⌘A, and
  the 2 000-node budget.
- `crates/daemon/tests/lifecycle.rs`:
  `an_osc_11_query_is_answered_with_the_palette_background` and
  `a_shell_survives_detach_and_reattach_with_the_same_screen`.
