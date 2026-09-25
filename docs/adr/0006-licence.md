# ADR-0006: ply's licence (D2)

- Status: Proposed — awaiting the owner's decision
- Date: 2026-09-25
- Work package: WP0 (blocks WP10 distribution, not development)
- Spec version: 5.0.1 (no change until decided)

## Context

`gpui` at zed 81c99f8 depends on `ztracing` (`vendor/gpuix/zed/crates/gpui/Cargo.toml:108`,
`ztracing.workspace = true`), and so does `sum_tree` (`crates/sum_tree/Cargo.toml:20`), which
gpui also depends on (`crates/gpui/Cargo.toml:93`). The zed workspace maps all three crate
names to local paths: `zlog`, `ztracing` and `ztracing_macro` are `{ path = "crates/…" }`
(`vendor/gpuix/zed/Cargo.toml:504-507`). `ztracing` depends on `zlog` and `ztracing_macro`
(`crates/ztracing/Cargo.toml:16,21`). All three declare `license = "GPL-3.0-or-later"`
(`crates/ztracing/Cargo.toml:6`, `crates/zlog/Cargo.toml:6`,
`crates/ztracing_macro/Cargo.toml:6`). `gpui` itself is Apache-2.0 (`crates/gpui/Cargo.toml:9`).
The directories of `ztracing` and `ztracing_macro` contain both a `LICENSE-APACHE` and a
`LICENSE-GPL` link, and `zlog` only `LICENSE-GPL`, but the manifests, which `cargo-deny`
reads, all say GPL-3.0-or-later.

S1b resolved the spike addon's normal-dependency graph for aarch64-apple-darwin
(`cargo metadata`, 480 packages): these three are its only GPL-licensed crates. Every binary
that links gpui — ply's addon and so the shipped app — therefore links GPL-3.0 code.

ply uses very little of it. The only uses in linked code are the attribute
`#[ztracing::instrument(skip_all)]` twice in `crates/gpui/src/svg_renderer.rs:190,196`, and
`use ztracing::instrument;` with `#[instrument(skip_all)]` in `crates/sum_tree/src/cursor.rs`
(lines 4, 215, 407, 422, 464) and `crates/sum_tree/src/sum_tree.rs` (lines 13, 399, 425, 510).
`zlog` is reached only through `ztracing` and as a sum_tree dev-dependency
(`crates/sum_tree/Cargo.toml:28`). Unless the build sets the `ZTRACING` environment variable
(`crates/ztracing/build.rs`), `instrument` is `ztracing_macro`'s attribute, which returns its
input unchanged (`crates/ztracing/src/lib.rs:8-9`, `crates/ztracing_macro/src/lib.rs:1-7`).
Removing or replacing it therefore changes no behaviour.

Spec D2 asks two questions, not one: **ply's licence, and whether ply is a company product
or a personal one.** The second decides whether an existing project's parsers could be reused
(spec D2), and it bears on which licences are acceptable at all.

## Options

1. **License ply GPL-3.0-or-later.** Compliant as is; ply's source must be offered with every
   binary; closes the door on proprietary distribution.
2. **Remove the GPL crates from ply's build.** There are two ways to carry it. S1b checked both
   with `cargo check -p ply-native` on the spike workspace (seeded lock, patches 0001–0002
   applied):

   - **2a. A patch inside the nested zed submodule** (recommended carrier).
     `patches/zed/0001-drop-ztracing.patch` deletes 13 lines in 5 Apache-2.0 files: the
     `ztracing.workspace = true` lines in `crates/gpui/Cargo.toml` and
     `crates/sum_tree/Cargo.toml`, the two attributes in `svg_renderer.rs`, and the `use` line
     plus the seven `#[instrument(skip_all)]` attributes in `cursor.rs` and `sum_tree.rs`. It
     touches no GPL file. `just vendor-patch` gains a second loop that applies
     `patches/zed/*.patch` with `git -C vendor/gpuix/zed apply`; the existing loop runs inside
     `vendor/gpuix` and never reaches the nested repository. **Measured:** the draft passes
     `git -C vendor/gpuix/zed apply --check`. With it applied, `cargo tree` shows no ztracing
     or zlog, the resolve drops 9 packages (`zlog`, `ztracing`, `ztracing_macro`,
     `tracing-subscriber`, `tracing-log`, `sharded-slab`, `thread_local`, `nu-ansi-term`,
     `valuable`), and `cargo check -p ply-native` passes. While it is applied,
     `git -C vendor/gpuix status` reports ` m zed`. Costs: INV-13 must be amended to cover the
     nested submodule (Spec delta). The committed `Cargo.lock` is the patched resolve, so every
     build runs `just vendor-patch` first, which patches 0001–0002 already require. The patch is
     re-applied, and possibly extended, on every GPUIX/zed upgrade. It is offered upstream to
     remorses/zed as "make ztracing optional".
   - **2b. A Cargo override of the path dependency, with no vendor change.** The Cargo book
     says each `[patch]` key "is a URL of the source that is being patched, or the name of a
     registry", and that `[patch]` can also be given as `--config`
     (doc.rust-lang.org/cargo/reference/overriding-dependencies.html); it says nothing about
     path sources. **Measured** with cargo 1.97.1:
     - A path dependency is overridden by `[patch."file://<absolute directory of the dependency>"]`.
     - Relative keys are ignored ("patch … was not used") or rejected ("should be a URL or
       registry name").
     - The key must be the path exactly as Cargo reached it. Reached through a symlinked vendor
       directory, the canonical path did not match.
     - On the spike workspace,
       `--config "patch.'file://<root>/vendor/gpuix/zed/crates/ztracing'.ztracing.path='<root>/crates/ztracing-shim'"`
       with a fresh Apache-2.0 proc-macro crate named `ztracing` exporting a no-op
       `#[instrument]` replaced ztracing and dropped 8 packages, zlog and ztracing_macro among
       them. `cargo check -p ply-native` passed.

     Costs:
     - The key is an absolute, machine-specific path, so it cannot be committed (INV-11).
       `just` must pass it from `justfile_directory()` on every cargo invocation.
     - Any tool that runs cargo directly (rust-analyzer, a bare `cargo build`) resolves the
       unpatched graph and rewrites `Cargo.lock`.
     - INV-13 stays as it is. The shim is ply's own code, written fresh and not copied
       (INV-15).

   Either way ply's licence stays open and runtime behaviour is unchanged.
3. **Keep ply unreleased** until upstream zed relicenses or removes the dependency.

## Recommendation

Option 2, carried as 2a: the change is 13 deleted lines in Apache-2.0 files, it works with
every tool that runs cargo, and the lockfile stays stable. 2b avoids touching vendor, but it
depends on Cargo behaviour the book does not state for path sources, and every cargo
invocation must carry a flag. Option 1 is the right answer only if ply is meant to be a GPL
project anyway. Which licence fits depends on the owner's answer to the second question
(company product or personal).

## Decision

Not taken. The owner decides both questions of D2: the licence, and whether ply is a
company product or a personal one. Until then WP10 builds and signs locally but nothing
is distributed.

## Consequences

- Development proceeds on the unmodified dependency tree.
- WP10's release workflow must not publish artefacts until this ADR is Accepted.
- If 2a is chosen: `just vendor-patch` applies `patches/gpuix/*` in `vendor/gpuix`, then
  `patches/zed/*` in `vendor/gpuix/zed`. `check-rules.ts` compares both working trees against
  their patch sets, and `cargo-deny` can then ban GPL-3.0 for every ply crate.

## Spec delta

None yet. Accepting option 2a changes INV-13 to "`vendor/gpuix` and its nested
`vendor/gpuix/zed` are clean at their pinned commits and change only through
`patches/gpuix/*` and `patches/zed/*`", and changes 8.1's `vendor-patch` step. That is a
changed invariant, so a major bump. Option 2b adds a build rule to 8.1 (minor bump). Option 1
or 3 adds a new rule in section 15 (minor bump). Either way, the answer to "company product
or personal" goes into D2's resolution column.
