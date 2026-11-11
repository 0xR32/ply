# The terminal

How a pane's bytes become the rows on screen, and how keys get back. plyd owns
the pty and emulates it with libghostty-vt; the app receives decoded rows over
C2 and paints them with GPUI. Raw pty output never reaches the app (INV-2),
and the app never mirrors terminal modes: it sends what the user did, and plyd
encodes it against the pane's live state.

```
child ──pty──► reader thread ──► pane task: Engine.write ──► dirty rows ──► DeltaBuilder ──C2──►
                                   │ replies (DA, DSR, OSC 10/11 …) ─► pty writer thread
conn.rs reader thread ──► ply_term::Replica ──► paint.rs: quads and shaped lines ──► GPUI
keys, mouse, paste, focus ──C2──► encode_input (against the live modes) ──► pty writer thread
```

`docs/screen-protocol.md` is the wire between the two halves.

## libghostty-vt

The terminal engine is libghostty-vt, Ghostty's terminal library, release
1.0.0, built from its tagged ghostty commit 0fa3d57
(`0fa3d577a5614150ff8e2c152de5fb9d5c6d2dec`, `vt-1.0.0`) with no local patches. It is not vendored: `crates/ghostty-sys/build.rs` downloads that
commit's source, verifies it and builds it (Ruling R45). Only plyd links it:
`ghostty-sys` is reached only through ply-term's `engine` feature, and only
ply-daemon enables that (INV-17, `checkInv17Ghostty`).

### The build

`crates/ghostty-sys/build.rs` runs while cargo builds `ghostty-sys`:

- **The source.** The build fetches
  `https://codeload.github.com/ghostty-org/ghostty/tar.gz/<commit>` for the
  pinned commit, checks the archive's SHA-256 against the value pinned in
  `build.rs` (`2f38f10e…95ea`) and refuses a mismatch, and keeps the extracted
  tree in `~/Library/Caches/ply/ghostty/<commit>/`, so the download happens once
  per machine. `PLY_GHOSTTY_SRC=<dir>` builds from an existing ghostty checkout
  at that commit instead, which is how a build runs offline. The download is
  extracted beside the cache entry and renamed into place whole, so a concurrent
  or interrupted build never sees half a tree.
- **Zig** is `$ZIG`, else `zig` on `PATH`, and must be 0.17.0, the
  `minimum_zig_version` of the pin; anything else fails with the version found.
  The Zig packages the library needs come from the release's dependency
  tarball (`libghostty-vt-1.0.0-deps.tar.gz` from `release.files.ghostty.org`),
  downloaded, checked against the SHA-256 pinned in `build.rs` and cached the
  same way; `PLY_ZIG_PKG_DIR=<dir>` supplies them extracted by hash instead.
- **Options.** `zig build -Demit-lib-vt -Dlib-version-string=1.0.0`, with `-Doptimize=` from
  `LIBGHOSTTY_VT_OPTIMIZE` (`Debug`, `ReleaseSafe`, `ReleaseFast` or
  `ReleaseSmall`; default `ReleaseFast`) and `-Dtarget=` mapped from the Rust
  target (`aarch64-apple-darwin` → `aarch64-macos`, `x86_64-apple-darwin` →
  `x86_64-macos`). The build writes only under cargo's `OUT_DIR` and the cache
  directory above; never into the source tree.
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
| `clipboard_writes` (OSC 52) | the `text/plain` part (UTF-8, at most one frame, 1 MiB) goes at once to every attached client as CLIPBOARD_WRITE (which the app does not act on yet); a write while no client is attached is dropped. Clipboard reads get no callback, so they are refused |

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

The app side lives in `crates/app/src`: `conn.rs` (the connection), `paint.rs`
(the terminal element), `select.rs`, `keys.rs`, `drop.rs` and `PaneView` in
`main.rs`.

**Which panes attach.** Only the active tab's panes have a view, laid out by
count (Ruling R56): one fills the tab, two and three are equal full-height
columns, four are equal quadrants, in position order. While the tab is zoomed
only the focused pane is drawn; the others stay attached. Switching tabs
detaches the panes the new tab does not hold and attaches its own, each
answered with a Snapshot. Hidden panes keep running and emulating in plyd;
their status keeps arriving over C1.

**The connection** (`conn.rs`). A view attaches when it is first laid out: a
thread of its own (`c2-pane-<id>`) connects to `run/data.sock`, sends ATTACH
with the grid and cell size, and reads frames into the pane's
`ply_term::Replica` (`crates/term/src/replica.rs`), the strict decoder the
tests use too, behind a mutex, so decoding never runs on the UI thread. It acks
every Snapshot and Delta, keeps TITLE and EXIT, and wakes the view only when the
view has painted since the last wake, so a burst of Deltas costs one repaint
and an idle pane costs nothing. A dropped connection or a bad frame re-attaches
after 100 ms, doubling to 2 s; ATTACH_REFUSED ends it. FOCUS is sent when the
pane gains or loses focus, and again after each Snapshot. BELL, PASTE_REJECTED
and CLIPBOARD_WRITE are not acted on yet: a bell marks nothing, a paste plyd
refuses as unsafe is dropped without a word, and a program's OSC 52 write does
not reach the pasteboard.

**Size.** Every layout measures the pane's body and sends the whole cells that
fit: ATTACH the first time, RESIZE when the grid or the cell changes. The cell
is Geist Mono's: the advance of `m` at the text size wide, and round(1.3 × size)
high; ATTACH and RESIZE carry it rounded to whole pixels. The app embeds Geist
and Geist Mono (`crates/app/assets/fonts/`, SIL OFL) and registers them at
startup, so nothing has to be installed; should that fail, the system fonts
stand in.

**Painting** (`paint.rs`). Each frame of a pane paints its rows from the
replica: the live screen, or the view from an absolute line when scrolled back,
asking for a missing page of scrollback (one request in flight, at most
`MAX_HISTORY_ROWS` lines). Per row, runs of cells with a background other than
the default become one quad each; text becomes one shaped line per run of cells
of one style, every glyph snapped to the cell grid (GPUI's `shape_line` with the
cell width), so a fallback glyph cannot drift the columns after it; a wide
character or a grapheme cluster gets a run of its own, and trailing blank cells
are not drawn. Colours resolve through `ply_term::Palette`, built from the
theme the app derives from the accent (`theme_for`) and sends with `theme.set`.
Inverse swaps fg and bg, faint blends fg halfway into bg, invisible paints fg in
bg; bold is semibold; italic, underline (curly drawn wavy, the others straight,
in the underline colour) and strikethrough are drawn; overline and blink are
not. The selection is painted in the theme's selection colour behind the text,
and a ⌘-hovered link is underlined in the cursor colour.

**The cursor** is a block (85 % opaque), a bar or an underline in the cursor
colour; an unfocused pane draws a hollow block. It shows while it is visible and
the program has DECTCEM on, and blinks at 530 ms while the pane has focus and
the program asked for a blinking cursor.

**The header and frame** (`chrome.rs`) show the pane's position, project,
branch, plan progress, CLI and model, how far back the view is scrolled, and a
status chip (Starting, Running, Needs you, Done, Your turn, Exited n, Lost, or
the shell's name for a shell); the frame glows amber while the pane needs you.
A TITLE names the pane while plyd's record has no title.

## Selection, clipboard, scrollback and search

These are ply's own, done in the app against the replica (`select.rs`).

**Selection.** Dragging with the primary button selects by cell; a double click
by word (letters, digits and `_-./~:@%+#`), a triple click by line; ⇧-click
extends. A click without a drag clears the selection, and so does a key typed
into the pane. While the program reports the mouse, clicks go to the program and
⇧ bypasses that to select. The selection is held by absolute line, so it stays
on its text while output scrolls.

**Copy and paste.** ⌘C copies the selection, joining rows with newlines except
after a soft-wrapped row and trimming each line's trailing blanks; scrollback
lines not fetched yet are left out. With no selection it does nothing and never
sends ^C. ⌘V pastes the pasteboard's text as one PASTE frame, never with
`allow_unsafe`. An image on the pasteboard is saved as a file in `ply-drops`
(turned into PNG with `sips` when the CLIs cannot read its format) and its path
typed, which Claude Code turns into its own `[Image #N]`. A program can never
read the pasteboard.

**Dropped files.** Files dropped on a pane are typed the way Ghostty, iTerm2
and Terminal.app type them: each path with its shell metacharacters (space, tab
and `\()[]{}<>"'` `` ` `` `!#$&;|*?`) backslash-escaped, joined by one space
with none after, as one PASTE frame (`drop.rs`); the pane becomes the focused
one. Claude Code strips the escapes and turns a pasted image path (png, jpeg,
gif, webp) into its own `[Image #N]`; ply types the path and never reads the
file.

**The ⌘⇧4 thumbnail** offers a file AppKit staged in
`…/TemporaryItems/NSIRD_screencaptureui_*/`, which screencaptureui deletes as
soon as the drag ends; Claude Code reads a pasted image asynchronously, finds it
gone and drops it without a word ([anthropics/claude-code#80980], in every
terminal). So a dropped path under `/TemporaryItems/` is kept first:
`keep_dropped` hard-links it (or copies it, when a link fails) into a fresh
folder in `ply-drops` (`docs/configuration.md`) under its own name, in the drop
handler itself, and that path is typed instead; a real thumbnail drag logs
`kept a dropped file its source deletes when the drag ends`. A staged file that
cannot be kept is typed as dropped and logged at `warn`. Any other path is typed
as it is.

A kept copy goes once nothing will open it. Claude Code reads the image into
memory at the paste, but it also tells the model `[Image source: <path>]`, so a
tool in that turn may still open the file. A pane's copies and pasted images
(`ply-drops/pane-<id>-<ms>-*`) therefore live through the turn that sent them:
when the pane starts a turn (its status becomes `running`), the app deletes its
copies made before the turn it started last, so a screenshot goes when the turn
after the one that used it begins; all of them go when the pane is closed.

[anthropics/claude-code#80980]: https://github.com/anthropics/claude-code/issues/80980

**Scrollback** stays in plyd (`scrollback_lines`, default 10 000). The wheel
scrolls ply's own view of it and the view fetches the pages it shows
(`docs/screen-protocol.md`); new output keeps a scrolled-back view in place, and
a key press or a paste returns to the live screen. On the alternate screen
without mouse reporting the wheel sends arrow keys instead; with reporting on it
goes to the program as buttons 4 and 5, at most ten steps per event.

**Search.** ⌘F opens a find bar over the pane and fetches the whole scrollback,
a page at a time. Matching is a case-insensitive substring search over the
screen and the fetched lines, redone as lines and pages arrive; the bar shows
the current match and the count, starts at the newest match, ↑ and ⏎ step to
older ones and ↓ to newer ones, and each match is selected and scrolled into
view. Esc closes it. Keys typed into it never reach the pty. (The engine's own
`search` is not used by the app.)

**Links.** ⌘-click on an http or https URL opens it in the default browser
through `/usr/bin/open -u` (`link_at` in `select.rs`). A URL is found in the
text of the soft-wrapped line under the pointer, so one the terminal wrapped
opens whole; one the program broke with a newline opens only up to the break.
Sentence punctuation and a `)` or `]` with no opening partner at its end are
left out, so `(https://…)` and `[text](https://…)` work. No other scheme is a
link, because the text is the program's output and a custom scheme could launch
an app. The link opens when the button is released over the link it was pressed
on, and neither the press nor the release reaches the program or touches the
selection. While ⌘ is held over a link it is underlined and the pointer is a
hand; the underline follows the mouse moves. OSC 8 hyperlinks are not read:
panes run with `TERM=xterm-256color` and no `TERM_PROGRAM`, so neither CLI sends
them.

**IME** is not implemented: every KEY frame carries `composing` 0 and the text
GPUI reports for the key, so dead keys and CJK composition do not work in a
pane.

## Input in the view

`keys.rs` turns GPUI keystrokes into KEY frames:

- **⌘ chords never become KEY frames**; they belong to the app
  (`docs/keybindings.md`).
- The physical key comes from GPUI's key name through a table of libghostty-vt
  key codes (letters, digits, punctuation, Enter, Tab, Esc, Backspace, Delete,
  Home, End, Page Up and Down, the arrows, F1–F25). A layout key with no code
  (`ö`) is key 0 with its text.
- Text is the character GPUI reports, dropped when it is a control character or
  when Ctrl is held (plyd encodes Ctrl chords from the key). ⌥ and ⇧ are marked
  consumed when they changed the key's character, so ⌥'s layout character is
  typed as text.
- Presses and repeats are sent; releases are not.

Mouse events become MOUSE frames only while the program reports the mouse
(motion only when the cell under the pointer changes), positioned at the
centre of the cell in plyd's whole-pixel cells; focus changes become FOCUS
frames once the view is attached.

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

The app's own numbers — CPU, frame stalls, memory and tab switches, measured
against the React app it replaced — are in `docs/perf.md`. `ply --stats` logs a
`frame stats` line every second to the app log: frames drawn, pane wakes, the
time from a frame's render to its paint (p50, p99, max), the interval between
painted frames, and the main thread's stalls, measured as the gaps of a 1 ms
timer (p99, max, how many over 16.7 and 33.3 ms). A debug plyd streams the
recorded agent output into live panes for such runs (`plyd --replay`,
`crates/daemon/src/replay.rs`; `docs/development.md` shows the run).

**P2**, key to the pty. plyd measures the part it owns itself: with
`PLY_LOG=debug` every KEY frame logs `P2: key frame to pty write` with
`latency_us`, from the data server decoding the frame to the pty writer's
`write(2)` returning. A release plyd,
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
- `crates/app/src/select.rs`: copy joins rows except after a soft wrap and
  trims trailing blanks, a double click takes a word with path characters,
  find is case-insensitive and oldest first, a link is found under the pointer
  without its trailing punctuation and a soft-wrapped link opens whole;
  `crates/app/src/drop.rs`: dropped paths are escaped as terminals type them.
- `crates/daemon/tests/lifecycle.rs`:
  `an_osc_11_query_is_answered_with_the_palette_background` and
  `a_shell_survives_detach_and_reattach_with_the_same_screen`.
