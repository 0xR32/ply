# ADR-0007: GPUIX is ply's UI framework (D4)

- Status: Accepted (owner's decision, 2026-09-25)
- Date: 2026-09-25
- Work package: WP0
- Spec version: 5.0.1 (records D4; no change)

## Context

ply wants React productivity for its chrome and native GPU rendering for busy
terminals. GPUIX renders a React tree with Zed's GPUI (Metal on macOS). It is
pre-1.0 with three active contributors (spec R1), so it fails INV-16's contributor
threshold. Alternatives weighed in spec 2: plain GPUI (UI in Rust, slower
iteration), Electron and Tauri (webview frame cost with several busy terminals),
Flutter.

## Decision

ply uses GPUIX, pinned exactly at remorses/gpuix 9fcd628 (`gpuix-native` 0.10.0,
`@gpuix/react` 0.10.0, zed fork at 81c99f8), as a git submodule in `vendor/gpuix`.
It is admitted outside INV-16 by this decision. GPUIX changes only through
`patches/gpuix/0001-element-registry.patch` and `0002-event-props.patch` (INV-13),
both offered upstream as a registration hook (not the separate-binary plugin
split that GPUIX's `docs/custom-elements-plan.md` calls Phase 2, which would also
need such a hook; ADR-0001 §1).

## Consequences

- Upgrades are ADR-gated and rerun P1–P4 (rule 0.1.5).
- ply depends on a small upstream for fixes; the two patches keep the fork surface
  minimal. If GPUIX ships its own background-repaint mechanism (R2), ply switches
  to it by ADR.
- `deps.allow.toml` records gpuix with `exception = "D4"`.

## Spec delta

None. Confirms spec 2 (UI framework), D4, R1 and INV-13.
