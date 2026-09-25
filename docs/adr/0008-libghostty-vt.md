# ADR-0008: libghostty-vt is ply's terminal engine (D5)

- Status: Accepted (owner's decision, 2026-09-25)
- Date: 2026-09-25
- Work package: WP0
- Spec version: 5.0.1 → 6.0.0 (applied by the spec-sync task)

## Context

plyd runs one terminal emulator per pane for every pane, including hidden ones (R-R20),
and streams its grid to the app (C2). The engine decides memory per pane (P4: ≤ 15 MB with
10,000 lines of 168-column scrollback), how cheaply dirty rows reach a DeltaBuilder, and
whether plyd can answer the CLIs' startup probes and encode keys, mouse, focus and paste
against the pane's live modes (R-R4 to R-R10).

Two engines were weighed in spec 2:

- **alacritty_terminal**, the engine of spec 4.0.0. Its cell is 24 bytes:
  `size_of::<alacritty_terminal::term::cell::Cell>()` printed 24 for release 0.26.0 on
  2026-09-25. At 10,000 × 168 cells that is 40.3 MB of cells alone per pane, 2.7 times
  P4. It has no incremental render-state API and no snapshot encode/restore (spec 2,
  VERIFIED on 2026-09-24).
- **libghostty-vt**, Ghostty's engine as a C library with a Zig build. Its cell is a
  packed 8-byte value (`GhosttyCell`, `include/ghostty/vt/screen.h:42`; 8 bytes in the
  library's ABI manifest; `src/terminal/page.zig:2143`). ADR-0005 measured 13.5–14.0 MiB
  per live 10k × 168 pane for plain and diff output, 0.7–4.8 MiB after idle compression,
  and confirmed the render-state dirty rows, snapshots across processes, search, palette
  answers to the CLIs' probes, the OSC 7/9 callbacks and the key, mouse, focus and paste
  encoders.

libghostty-vt does not meet INV-16's last clause: its maintainers declare the API
experimental ("an incomplete, work-in-progress API. It is not yet stable and is definitely
going to change", `include/ghostty/vt.h:10-11,25-26`), and the snapshot format carries no
binary-compatibility guarantee (`include/ghostty/vt/snapshot.h:112-113`). Its other INV-16
numbers pass (`gh api`, 2026-09-25): ghostty-org/ghostty has 61,523 stars, a push on
2026-09-25, and 138 contributors with 5 or more commits; licence MIT
(`vendor/libghostty-vt/LICENSE`).

herdr (MIT), a Rust terminal multiplexer, ships libghostty-vt from the same commit
(`vendor/libghostty-vt.vendor.json`: `source_commit`
44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51) and drives it from Rust with a `build.rs`, which
is the model for `ghostty-sys`.

## Decision

1. **Engine.** ply's terminal engine is libghostty-vt, by the owner's decision of
   2026-09-25 (D5). It is admitted outside INV-16 by this decision, as GPUIX is by D4
   (ADR-0007).
2. **Pin.** `vendor/libghostty-vt` holds the pristine ghostty source at commit
   44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51 (2026-09-10, "i18n: update `vi` translation
   for 1.4 (#13783)") in herdr's dist layout, with `VERSION` = `1.3.2-HEAD-+44f2a44df`.
   The library itself reports version `0.1.0-dev` (`build.zig:10`). `vendor.json`
   (ledger, WP3) records the commit, archive name and extracted directory the way herdr's
   `libghostty-vt.vendor.json` does, and `patches.md` lists no patches: ADR-0005 needed
   none, and herdr's five local patches are not taken.
3. **Fence (INV-17).** Only `ghostty-sys` has FFI to it and only `ply-term`'s `engine`
   feature uses `ghostty-sys`; only `ply-daemon` enables that feature. `ply-native` is
   never built together with the rest of the workspace (Ruling R11), so Cargo's feature
   unification can never switch `engine` on inside the addon, and the check runs per
   crate: `cargo tree -p ply-native -i ghostty-sys` must be empty, and
   `cargo tree -p ply-daemon -i ghostty-sys` must show only the path
   ghostty-sys → ply-term → ply-daemon. The app never links it and never
   mirrors terminal modes: KEY, MOUSE, FOCUS and PASTE are encoded in plyd with the pane's
   own encoders. `check-rules.ts` compares `vendor/libghostty-vt` with `vendor.json` plus
   `patches.md`, **including ignored files**: Zig 0.16 writes fetched packages into
   `<build root>/zig-pkg/`, which the vendored `.gitignore` hides from git (ADR-0005,
   Decision 1).
4. **Build.** Zig is pinned to 0.16.0, the version `build.zig.zon:6` requires, and
   installed by CI. `crates/ghostty-sys/build.rs` runs the command in ADR-0005 Decision 1:
   `-Demit-lib-vt -Doptimize=ReleaseFast -Dsimd=true -Dtarget=aarch64-macos`, `--system`
   with a ply-owned directory of the eight packages the build needs, `-fno-sys=` for every
   integration, and prefix and caches under `OUT_DIR`. The build never touches the network
   and never writes into the vendor tree. It links `libghostty-vt.a` statically from a
   directory that holds only the archive.
5. **Upgrades.** A new pin is its own PR with an ADR, the WP3 suite, P1–P5, the FFI layout
   check against `ghostty_type_json()`, and a re-check of every behaviour ADR-0005 records
   as a default ply overrides (scrollback byte cap, mode 2027, Kitty image storage,
   option-as-alt reset). Persisted snapshots from the previous pin are discarded.

## Consequences

- A second toolchain (Zig) and 31 MB of extracted build packages in ply's tree or CI cache;
  a clean build adds about 65–85 s to a cold `cargo build`.
- The C API is work in progress, so upgrades can break `ghostty-sys` and `ply-term`'s
  `engine` module. R10 contains that to those two files; nothing else in ply names a
  `ghostty_*` symbol.
- plyd, not libghostty-vt, owns everything the library leaves to its embedder: the
  idle-compression timer, the DEC 2026 hold and cap, the pty write-back queue, re-applying
  option-as-alt, the scrollback limits, disabling Kitty graphics, and serializing all calls
  per pane (the library creates no threads and its callbacks run inside `vt_write`).
- Memory per pane stays near 8 bytes a cell (P4), and idle panes shrink further through
  the library's compression.
- `deps.allow.toml` records libghostty-vt with `exception = "D5"`, the pinned commit and
  the stars and contributor numbers above.

## Spec delta

Confirms spec 2 (terminal emulation, build toolchain), D5, R10 and INV-17; the spike's
corrections are in ADR-0005. Two changes to INV-17's "Enforced by" column, applied with
6.0.0 by the spec-sync task: the `cargo tree` check is per crate, as in Decision 3
(Ruling R11), instead of one `cargo tree -i ghostty-sys` over the workspace; and the
vendor directory comparison includes git-ignored files, because Zig 0.16 hides its
`zig-pkg/` there (Decision 3).
