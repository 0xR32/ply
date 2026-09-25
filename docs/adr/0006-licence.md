# ADR-0006: ply's licence (D2)

- Status: Proposed — awaiting the owner's decision
- Date: 2026-09-25
- Work package: WP0 (blocks WP10 distribution, not development)
- Spec version: 5.0.1 (no change until decided)

## Context

`gpui` at zed 81c99f8 depends on `ztracing` (`vendor/gpuix/zed/crates/gpui/Cargo.toml:108`,
`ztracing.workspace = true`), which depends on `zlog` and `ztracing_macro`
(`vendor/gpuix/zed/crates/ztracing/Cargo.toml:16,21`). All three declare
`license = "GPL-3.0-or-later"` (`crates/ztracing/Cargo.toml:6`, `crates/zlog/Cargo.toml`,
`crates/ztracing_macro/Cargo.toml`). `gpui` itself is Apache-2.0
(`crates/gpui/Cargo.toml:9`). Every binary that links gpui — ply's addon and so the
shipped app — therefore links GPL-3.0 code. `ztracing` is small: 117 lines in
`src/lib.rs` plus a 441-line wasm-only `src/web.rs`; `ztracing_macro` is 7 lines.
The decision also settles whether an existing project's parsers could be reused (spec D2).

## Options

1. **License ply GPL-3.0-or-later.** Compliant as is; ply's source must be offered
   with every binary; closes the door on proprietary distribution.
2. **Stub `ztracing` in GPUIX's zed fork.** Replace the three crates with an
   Apache-2.0 shim exposing the same macros as no-ops (or as plain `tracing` calls),
   carried as a third gpuix patch (needs its own ADR under INV-13) and offered
   upstream. Keeps ply's licence open; costs one small patch to maintain on every
   GPUIX upgrade.
3. **Keep ply unreleased** until upstream zed relicenses or removes the dependency.

## Recommendation

Option 2: it is mechanical (the public surface is a handful of macros), keeps every
licence choice open, and the patch is small enough to offer upstream. Option 1 is the
right answer only if ply is meant to be a GPL project anyway.

## Decision

Not taken. The owner decides; until then WP10 builds and signs locally but nothing
is distributed.

## Consequences

- Development proceeds on the unmodified dependency tree.
- WP10's release workflow must not publish artefacts until this ADR is Accepted.

## Spec delta

None yet. Accepting option 2 adds patch 0003 to INV-13 (a changed invariant: major
bump); option 1 or 3 is a new rule in section 15 (minor bump).
