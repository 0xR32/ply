# The screen protocol

How a terminal view gets a pane's screen and sends it input: C2, binary frames
over a Unix socket, one connection per attached pane. This document is every
byte of it. plyd emulates each pane with libghostty-vt and sends decoded rows;
raw pty output never reaches the view (INV-2). What the terminal does with the
input, and how the app draws the rows, is `docs/terminal.md`.

There is no serialisation crate. `crates/proto/src/data.rs` (`Frame::encode`,
`Frame::decode`, `FrameReader`) is the layout on the Rust side and
`app/src/terminal/frames.ts` (`encodeFrame`, `decodeFrame`, `FrameReader`) on
the TypeScript side. The golden frames keep the two byte-identical (see the end
of this document).

## Where it is

| Run | Socket |
|---|---|
| default | `~/Library/Application Support/ply/run/data.sock` |
| `PLY_HOME=<dir>` | `<dir>/run/data.sock` |
| `plyd --run-dir <dir>` | `<dir>/data.sock` |

It sits beside the control socket in the same `0700` run directory
(`docs/control-channel.md`). A connection attaches to exactly one pane; a pane
may have any number of attached connections, each with its own style table,
sequence numbers and acknowledgement window. The app opens one only for each
visible pane (the active tab's panes, only the focused one while zoomed) and
closes it when the pane leaves the screen.

## Frames

```
frame = len:u32 · kind:u8 · payload[len]
```

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `len` | u32 | Payload length in bytes, not counting these five; at most 1 048 576 (`MAX_FRAME_LEN`). |
| 4 | `kind` | u8 | The frame kind. |
| 5 | payload | `len` bytes | Laid out per kind below. |

Every integer is little-endian. A bool is one byte that must be 0 or 1. Text is
UTF-8 filling the rest of the payload. Both readers refuse a header whose `len`
is above the cap before they buffer the payload.

Decoding is strict (INV-10): an unknown kind, a payload cut short, bytes left
over after the last field, a value outside its range, an unknown flag bit and
invalid UTF-8 are all errors, and the receiver drops the connection. The
TypeScript codec also refuses a u64 above JavaScript's safe-integer range
(2^53 − 1), so every value plyd sends stays inside it.

| Kind | Name | Direction | Payload |
|---|---|---|---|
| `0x10` | ATTACH | client → plyd | 18 bytes |
| `0x11` | INPUT_RAW | client → plyd | raw bytes |
| `0x12` | RESIZE | client → plyd | 8 bytes |
| `0x13` | FETCH_HISTORY | client → plyd | 10 bytes |
| `0x14` | ACK | client → plyd | 8 bytes |
| `0x15` | KEY | client → plyd | 12 bytes + text |
| `0x16` | MOUSE | client → plyd | 16 bytes |
| `0x17` | PASTE | client → plyd | 1 byte + text |
| `0x18` | FOCUS | client → plyd | 1 byte |
| `0x20` | SNAPSHOT | plyd → client | variable |
| `0x21` | DELTA | plyd → client | variable |
| `0x22` | HISTORY | plyd → client | variable |
| `0x23` | TITLE | plyd → client | text |
| `0x24` | BELL | plyd → client | empty |
| `0x25` | EXIT | plyd → client | 4 bytes |
| `0x26` | PASTE_REJECTED | plyd → client | empty |
| `0x27` | ATTACH_REFUSED | plyd → client | 1 byte + text |
| `0x28` | CLIPBOARD_WRITE | plyd → client | text |

`0x10`–`0x1F` travel from the client and `0x20`–`0x2F` from plyd. A client that
sends a plyd kind, or a second ATTACH, is disconnected.

## Shared encodings

**Colour** — 4 bytes, `tag · a · b · c`. Colours stay symbolic: the app resolves
them against its theme, so a theme change needs no resend.

| Bytes | Colour |
|---|---|
| `00 00 00 00` | the theme's default for the slot (fg, bg, or for underline: the foreground) |
| `01 i 00 00` | index `i` of the 256-colour table; 0–15 are the theme's ANSI colours |
| `02 r g b` | true colour |

Any other combination is invalid.

**Attrs** — u16.

| Bits | Meaning |
|---|---|
| 0 | bold (SGR 1) |
| 1 | faint (SGR 2) |
| 2 | italic (SGR 3) |
| 3–5 | underline: 0 none, 1 single, 2 double, 3 curly, 4 dotted, 5 dashed; 6 and 7 are invalid |
| 6 | blink (SGR 5) |
| 7 | inverse (SGR 7) |
| 8 | invisible (SGR 8) |
| 9 | strikethrough (SGR 9) |
| 10 | overline (SGR 53) |
| 11–15 | always 0 |

**Style entry** — 16 bytes.

| Offset | Field | Type |
|---|---|---|
| 0 | `id` | u16, never 0 |
| 2 | `fg` | Colour |
| 6 | `bg` | Colour |
| 10 | `underline_color` | Colour |
| 14 | `attrs` | Attrs |

**Style table** — `n:u16` followed by `n` style entries.

Style ids belong to one attachment. Id 0 is the default style (all colours
default, no attrs); it is implied and never sent. plyd interns every other style
by value the first time a row it sends uses it, and names it in that frame's
table. A Snapshot carries the complete table and replaces the client's; a Delta
or History carries only the styles new to the client, and an id is never
reused within one attachment until the next Snapshot. When a client's table
reaches 65 535 styles its next update is a Snapshot, which starts the table
over.

**Cell** — 7 bytes, plus the cluster when `GRAPHEME` is set.

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `codepoint` | u32 | The base codepoint, at most `0x10FFFF`; 0 is an empty cell. |
| 4 | `style` | u16 | A style id, 0 for the default. |
| 6 | `flags` | u8 | bit 0 `WIDE` (first half of a wide character), bit 1 `SPACER` (its second half: draw nothing), bit 2 `SPACER_HEAD` (padding at a row end where a wide character wrapped: draw nothing), bit 3 `GRAPHEME`; bits 4–7 are 0. |
| 7 | `n` | u8 | Only with `GRAPHEME`: 1–255 further codepoints. |
| 8 | extra | `n` × u32 | The cluster's codepoints after the base. |

A cluster longer than 255 extra codepoints is truncated by plyd (and logged).

**Row** — 7 bytes of header, then its cells.

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `index` | i32 | In SNAPSHOT and DELTA the screen row, 0.. from the top of the live screen; in HISTORY the row's position in the page, 0.., so row `i` is absolute line `start + i` (any other value is invalid). |
| 4 | `flags` | u8 | 0, or 1: the row soft-wraps onto the next, so copying joins them without a newline. |
| 5 | `n` | u16 | Cell count. |
| 7 | cells | `n` cells | From column 0. |

plyd leaves out a row's trailing default blanks (codepoint 0, style 0, no
flags), so a row may carry fewer cells than the grid has columns; the rest are
blank. It never carries more.

**Row list** — `n:u16` followed by `n` rows.

**Cursor** — 6 bytes.

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `col` | u16 | Column in the live screen. |
| 2 | `row` | u16 | Row in the live screen. |
| 4 | `shape` | u8 | 0 bar, 1 block, 2 underline, 3 hollow block. |
| 5 | `flags` | u8 | bit 0 visible, bit 1 blinking; the rest 0. |

**Modes** — u16, the display facts the view needs; plyd never trusts them back.

| Bit | Mode |
|---|---|
| 0 | the alternate screen is active (it has no scrollback) |
| 1 | DECTCEM: the program wants the cursor shown |
| 2 | a mouse-reporting mode is on, so mouse events go to the program |
| 3 | bracketed paste (mode 2004) is on |

**Mods** — u16, bit-identical to libghostty-vt's `GhosttyMods`.

| Bit | Modifier |
|---|---|
| 0–3 | Shift, Control, Option, Command |
| 4, 5 | Caps Lock on, Num Lock on |
| 6–9 | the right-hand Shift, Control, Option, Command (meaningful only with the matching bit 0–3) |
| 10–15 | always 0 |

## Client → plyd

**ATTACH `0x10`** — must be the first frame on a connection.

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `v` | u16 | `C2_VERSION`, 1. |
| 2 | `pane_id` | u64 | The C1 `Pane.id`. |
| 10 | `cols` | u16 | Grid columns the view shows. |
| 12 | `rows` | u16 | Grid rows. |
| 14 | `cell_width_px` | u16 | Width of one cell in pixels, not of the view. |
| 16 | `cell_height_px` | u16 | Height of one cell. |

**INPUT_RAW `0x11`** — bytes written to the pty as they are, not encoded.

**RESIZE `0x12`** — `cols:u16 · rows:u16 · cell_width_px:u16 ·
cell_height_px:u16`, 8 bytes, the same fields as ATTACH. plyd reflows the pane,
sets the pty's size and answers this client with a Snapshot. A grid too large
for one Snapshot frame (see **Attaching**) is ignored, and the Snapshot shows
the size the pane kept.

**FETCH_HISTORY `0x13`** — `start:u64 · count:u16`, 10 bytes: scrollback lines
`start .. start + count`. `start` is an absolute line (see **Absolute lines**);
`count` is 1 to 1 000 (`MAX_HISTORY_ROWS`).

**ACK `0x14`** — `seq:u64`, 8 bytes: the newest Snapshot or Delta the client has
applied.

**KEY `0x15`** — one key event, which plyd encodes against the pane's modes.

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `key` | u16 | The physical key as a `GhosttyKey` value, 0–175 (`MAX_KEY_CODE`); 0 (unidentified) sends `text` alone. |
| 2 | `mods` | u16 | Mods held. |
| 4 | `consumed_mods` | u16 | Mods the layout used to produce `text` (⌥ for `@` on a German layout). |
| 6 | `action` | u8 | 0 release, 1 press, 2 repeat. |
| 7 | `flags` | u8 | 0, or 1: an IME composition is in progress (plyd encodes nothing). |
| 8 | `unshifted_codepoint` | u32 | The key's codepoint without Shift, for kitty alternate-key reports; 0 when unknown. |
| 12 | `text` | UTF-8 | What the key typed after the layout, possibly empty. |

**MOUSE `0x16`** — 16 bytes.

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `action` | u8 | 0 press, 1 release, 2 motion. |
| 1 | `button` | u8 | 0 none, 1 left, 2 right, 3 middle, 4–7 wheel up, down, left, right, 8–11 extra buttons. |
| 2 | `mods` | u16 | Mods held. |
| 4 | `col` | u16 | Cell column under the pointer. |
| 6 | `row` | u16 | Cell row under the pointer. |
| 8 | `x` | f32 | Pointer x in pixels from the grid's top-left; finite. |
| 12 | `y` | f32 | Pointer y; finite. |

plyd encodes from `x` and `y` with the pane's cell size (SGR-pixels, mode 1016,
needs the pixels); `col` and `row` are informational.

**PASTE `0x17`** — `allow_unsafe:u8 · text`. Without `allow_unsafe`, text that
could inject commands is refused with PASTE_REJECTED and nothing is written.

**FOCUS `0x18`** — `in:u8`, 1 gained, 0 lost. plyd reports it to the program
only while the program enabled mode 1004.

## plyd → client

**SNAPSHOT `0x20`** — the whole screen.

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `seq` | u64 | Sequence number, per attachment, strictly increasing across Snapshots and Deltas. |
| 8 | `cols` | u16 | Grid columns. |
| 10 | `rows` | u16 | Grid rows. |
| 12 | `cursor` | Cursor | |
| 18 | `modes` | Modes | |
| 20 | `scrollback_rows` | u32 | Scrollback rows above row 0; 0 on the alternate screen. |
| 24 | `scrollback_base` | u64 | Absolute line of the oldest scrollback row (see **Absolute lines**). |
| 32 | `styles` | style table | The complete table; it replaces the client's. |
| … | `lines` | row list | Every screen row, `0 .. rows`. |

**DELTA `0x21`** — what changed since the client's previous Snapshot or Delta.

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `seq` | u64 | |
| 8 | `cursor` | Cursor | After the change. |
| 14 | `modes` | Modes | After the change. |
| 16 | `scrollback_rows` | u32 | After the change. |
| 20 | `scrollback_base` | u64 | After the change; never below the previous frame's. |
| 28 | `styles_added` | style table | Styles new to this client. |
| … | `lines` | row list | Each changed row, replacing the row with the same index. |

**HISTORY `0x22`** — the answer to FETCH_HISTORY.

| Offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | `start` | u64 | Absolute line of the first row returned (the request's `start` when no row is returned). |
| 8 | `styles_added` | style table | Styles new to this client. |
| … | `lines` | row list | Lines `start .. start + n`, oldest first, row `i` with index `i`. |

**TITLE `0x23`** — the new terminal title (OSC 0 or 2), UTF-8, possibly empty.

**BELL `0x24`** — the program rang the bell; empty payload.

**EXIT `0x25`** — `code:i32`: the process ended. 128 + signal for a signal
death, -1 when plyd could not wait for it.

**PASTE_REJECTED `0x26`** — the last PASTE was refused as unsafe; empty payload.

**ATTACH_REFUSED `0x27`** — `reason:u8 · message`; plyd closes the connection
after it.

| `reason` | Why |
|---|---|
| 1 | `v` differs from plyd's `C2_VERSION`. |
| 2 | No open pane has that id (or it closed while attaching). |
| 3 | The first frame was not ATTACH, did not decode, or asked for a grid too large for one Snapshot frame. |

`message` is a sentence for the user.

**CLIPBOARD_WRITE `0x28`** — the text a program set the clipboard to with OSC
52, UTF-8 filling the payload; empty clears the clipboard. plyd sends it at once
to every attached client (a write while none is attached is dropped) and only
the `text/plain` part of the write; a write that does not fit one frame is
dropped and logged. Clipboard reads are never answered. This kind was added to
version 1 (nothing had been released), so `C2_VERSION` is still 1.

## Attaching

1. The client connects and sends ATTACH.
2. plyd reads `v` from the payload's first two bytes before decoding the rest,
   so a client of another version is told reason 1 whatever its ATTACH looks
   like. It refuses a bad first frame, another version, an unknown pane or a grid
   whose Snapshot could not fit one frame (7 bytes a cell plus 7 a row, with
   64 KiB kept for styles, within `MAX_FRAME_LEN`; `Geometry::fits_one_frame`)
   with ATTACH_REFUSED and closes. A refused size is never recorded.
3. Otherwise plyd applies the client's size to the pane (the terminal reflows
   and the pty gets `TIOCSWINSZ`; the size also becomes the size of the next
   pane created) and answers with a SNAPSHOT, then a TITLE when the program has
   set one, then an EXIT when the process has already ended.
4. From then on the client sends input, RESIZE, FETCH_HISTORY and ACK, and plyd
   sends updates as the screen changes.

Every attach is answered with a Snapshot, which is how reopening the app shows
each pane's current screen. A pane restored after a plyd restart (`lost` or
`exited`) attaches too; its screen is empty, because plyd does not keep screens
across its own restarts.

**Several clients on one pane.** The pane has one size, and each ATTACH or
RESIZE sets it: the client that sized it last wins, and the others receive a
Snapshot at the new size with their next update.

**Re-attaching** is a new connection, so sequence numbers and the style table
start over. The app's replica forgets its last sequence number when its
connection drops (`Replica.resetSequence`), keeps the old screen until the new
Snapshot replaces it, and then draws the new one.

## Updates

plyd builds each client's frames with that client's `DeltaBuilder`
(`crates/term/src/delta.rs`):

- a **Delta** carries the rows whose content changed since the client's last
  frame, plus the cursor, the modes, the scrollback count and its base;
- a Delta with **no rows** is sent when only the cursor, the modes or the
  scrollback count or base changed (a cursor hide dirties no row);
- **nothing** is sent for a pane with no change, so an idle pane sends no frame
  at all;
- a **Snapshot** is sent instead of a Delta when the client has had none yet,
  when the grid size changed, and when its style table is full.

Rows are whole: a row in a Delta replaces the client's row with that index.

## Flow control

The rules live in `crates/daemon/src/publisher.rs` as small clocked state
machines and are applied per pane by its task (`crates/daemon/src/panes/pane.rs`).

| Rule | Value |
|---|---|
| Unacknowledged Deltas a client may have | 4 (`MAX_UNACKED_DELTAS`) |
| Shortest interval between two updates of one pane | 1/120 s (`MIN_FRAME_INTERVAL`); the first change after a quiet spell goes out at once |
| A client blocked this long (changes waiting, window full) gets a Snapshot | 3 s (`FORCED_SNAPSHOT_AFTER`) |
| A client with anything unacknowledged and no Ack progress this long is disconnected | 30 s (`ACK_TIMEOUT`) |
| Longest a DEC 2026 synchronized update holds updates back | 150 ms (`SYNC_CAP`) |
| Frames queued towards one client before it is disconnected | 32 (`CLIENT_QUEUE`) |

- **Acknowledging.** A client acks the `seq` of the newest Snapshot or Delta it
  has applied; one ack covers every earlier frame. plyd ignores an ack for a
  frame it never sent or one older than the last ack. Only Deltas count against
  the window.
- **A forced Snapshot** supersedes the client's unacknowledged Deltas, so it
  reopens the window.
- **TITLE and BELL** are coalesced to the update cadence (many title changes in
  one interval arrive as one TITLE) and are not held by the window.
- **HISTORY, PASTE_REJECTED and CLIPBOARD_WRITE** go out as soon as they are
  built.
- **A client that stops reading** fills its queue of 32 frames and is
  disconnected. So is one whose frame cannot be encoded (a Snapshot of a grid
  larger than about 149 000 cells does not fit one frame).

**Synchronized output.** While the program holds DEC 2026 open, plyd builds no
update; the pane's next update waits until the program closes it or 150 ms have
passed since it opened, whichever is first. At the cap plyd ends the update
itself and publishes. A resize also ends it.

## Absolute lines

Scrollback is addressed by absolute line, so a line keeps its number while
output scrolls and while the line cap drops the oldest lines. Every SNAPSHOT
and DELTA carries `scrollback_base`, the absolute line of the oldest scrollback
row plyd keeps: the scrollback is lines `scrollback_base .. scrollback_base +
scrollback_rows`, and live screen row `y` is line `scrollback_base +
scrollback_rows + y`. The base counts every line dropped from the top of the
scrollback since plyd created the pane's terminal (pruned by the line cap,
erased with `CSI 3 J`), so it never decreases within an attachment.

plyd counts with a tracked libghostty-vt grid reference at the top of the live
screen, re-read after every write and resize (`Engine::scrollback_base`). The
count is exact while one pty read adds fewer lines than the scrollback holds;
past that the reference itself is dropped and the base jumps past every line the
terminal held before, so numbers are never reused for other content. On the
alternate screen `scrollback_rows` is 0 and the base stays where it was.

## History paging

Scrollback lives in plyd; the client fetches the rows it wants to show.

- FETCH_HISTORY asks for `count` lines from `start`. plyd clips the range to the
  scrollback it keeps (`scrollback_base .. scrollback_base + scrollback_rows`),
  so a page may hold fewer rows than asked, or none. A page reflects plyd's
  scrollback when it is built, which may be ahead of the client's last Delta;
  the absolute lines stay right either way.
- A page never exceeds one frame. 1 000 rows of a wide pane do not fit 1 MiB,
  so plyd stops at the last row that fits; the client asks again from the first
  row it is still missing.
- `styles_added` must be applied even when the client discards the rows: a page
  cut to fit can name styles of rows it left out, and later frames use those
  ids without sending them again.
- A client keeps fetched rows by absolute line. A Delta whose
  `scrollback_base` moved on drops every fetched line below it; a Snapshot
  (which also answers every resize, where the primary screen reflows) drops
  them all.

The app (`app/src/terminal/session.ts`) keeps at most one request in flight,
asks for the first run of missing lines around what it shows (at most 1 000),
and marks a start that came back empty as exhausted until the next Snapshot.

## The app's data client

`app/src/terminal/data-client.ts`, one connection per mounted TerminalView:

- it connects to `$PLY_HOME/run/data.sock` (or the default path) and sends
  ATTACH with the view's grid and cell size;
- after each socket read it hands that read's frames to the view in wire order
  and then sends one ACK for the newest Snapshot or Delta among them;
- a frame sent while not attached is dropped, not queued; a RESIZE made before
  the first Snapshot is sent once it arrives;
- a dropped connection or a protocol error re-attaches after 100 ms, doubling
  to at most 2 s, and the delay resets on the next Snapshot;
- ATTACH_REFUSED ends the connection for good and the view shows its message;
- it records decode time per socket read (`DecodeStats`).

## A worked example

The golden ATTACH (`crates/proto/tests/golden/c2/attach.bin`), 23 bytes:

```
12 00 00 00                len 18
10                         ATTACH
01 00                      v 1
ff ff ff ff ff ff 1f 00    pane_id 2^53 − 1
a8 00                      cols 168
32 00                      rows 50
08 00                      cell_width_px 8
13 00                      cell_height_px 19
```

The golden KEY (`key.bin`), which exercises the highest key code and every
field, 20 bytes:

```
0f 00 00 00                len 15
15                         KEY
af 00                      key 175
05 03                      mods: Shift, Option, right Option, right Command
04 00                      consumed_mods: Option
02                         action repeat
00                         flags: not composing
78 00 00 00                unshifted_codepoint 'x'
e2 89 88                   text "≈"
```

The golden ACK (`ack.bin`), acknowledging sequence number 2^53 − 1:
`08 00 00 00 · 14 · ff ff ff ff ff ff 1f 00`.

## Golden frames and the TypeScript codec

`crates/proto/tests/golden/c2/` holds one `.bin` per frame kind (KEY twice), 19 files, each
a whole frame with its header. `crates/proto/tests/golden_c2.rs` writes them
with ply-proto's encoder (`goldens()` lists the values; they exercise every
colour tag, every attr, wide, spacer and grapheme cells, and u64 values at
2^53 − 1):

- `c2_goldens_match_the_encoder_and_decode_back` fails when the encoder no
  longer writes a golden's bytes, or they do not decode back to the value;
- `c2_goldens_cover_every_kind_and_nothing_else` fails when a kind has no golden
  or a file has no value.

`app/src/terminal/frames.test.ts` mirrors `goldens()` value for value and, for
every file, decodes it to that value and re-encodes it byte for byte; it also
fails when a golden is added or removed on one side only. After an intended
layout change, regenerate the files with
`PLY_BLESS=1 cargo test -p ply-proto --test golden_c2` and update the
TypeScript expectations to match.

`crates/proto/tests/golden.rs` adds `c2_layouts_are_the_documented_byte_counts`
(ATTACH 18, RESIZE 8, FETCH_HISTORY 10, ACK 8, FOCUS 1, EXIT 4, BELL 0, KEY 12
+ text, MOUSE 16, a plain cell 7), `c2_every_kind_round_trips_through_encode_and_the_reader`,
`c2_max_size_frame_passes_and_oversized_frames_are_rejected`,
`c2_malformed_payloads_are_rejected`, `c2_encoding_refuses_inconsistent_graphemes`,
`c2_version_mismatch_is_detectable` and `c2_reader_reports_a_frame_cut_short`.
The TypeScript side has the same strictness tests in `frames.test.ts` and the
client's in `app/src/terminal/data-client.test.ts`.

End to end, `crates/daemon/tests/lifecycle.rs` checks that a shell survives
detach and re-attach with the same screen
(`a_shell_survives_detach_and_reattach_with_the_same_screen`), that a client
that stops acking gets exactly four Deltas and then a Snapshot
(`a_client_that_stops_acking_gets_a_forced_snapshot_after_3_s`), that one that
never acks is disconnected after 30 s
(`a_client_that_never_acks_is_disconnected_after_30_s`), that a DEC 2026 update
shows no intermediate state and is ended by plyd after 150 ms
(`a_synchronized_update_holds_frames_until_it_ends_or_150_ms_pass`), that an
OSC 52 write reaches every attached client
(`an_osc_52_write_reaches_every_attached_client`), and that the refusals carry
their reasons, an ATTACH of another version and layout included
(`handshakes_check_versions_and_panes`);
`crates/daemon/tests/idle.rs` checks that an idle pane sends no frame. The
publisher's rules have unit tests in `crates/daemon/src/publisher.rs`, and the
Delta rules in `crates/term/tests/delta.rs`.
