# ADR-0002: ply draws its own terminal element; C2 is ply's own format

- Status: Accepted
- Date: 2026-09-25
- Work package: WP0
- Spec version: 5.0.1 (records the 4.0.0 decision; no change)

## Context

Spec 4.0.0 replaced a vendored third-party terminal stack with a terminal that ply
owns end to end. INV-16 admits only well-maintained dependencies, and no terminal
element for GPUI meets it. Zed's own `terminal` and `terminal_view` crates are
GPL-3.0 (INV-15) and cannot be copied. The daemon must stream screen state to the
app without pty bytes ever entering JavaScript (INV-2, R-R1).

## Decision

1. The `<terminal>` element is ply's own native GPUIX custom element in
   `crates/native`, painted with GPUI only (spec 2, "Terminal rendering"): background
   spans, glyph runs from a shaped-run cache, strokes, cursor, scrollbar, and sprite
   glyphs drawn as quads (R-R13).
2. The screen crosses the process boundary as C2 (spec 4.2): length-prefixed
   little-endian frames encoded by hand in `ply-proto` (`crates/proto/src/data.rs`),
   with no serialisation crate. Snapshots and Deltas carry rows of cells whose
   colours stay symbolic, so a theme change needs no resend.
3. Emulation happens only in plyd (libghostty-vt, ADR-0008); the app keeps a
   `Replica` (`ply-term`, no `engine` feature) that applies frames.

## Consequences

- Terminal quality is ply's responsibility (R9): the Replica property tests, the
  recorded-stream replays and `vttest` guard it.
- C2 is versioned by the ATTACH `v` field and changes only by ADR (rule 0.1.1).
- The element can be profiled and tuned without upstream coordination
  (`plyStats()`, R-R17).

## Spec delta

None. Confirms spec 2 (terminal rendering, protocol types), 4.2 and R-R1.
