# ADR-0001: GPUIX integration — ply's own addon and the native `<terminal>` element

- Status: Proposed (spike S1b result; Accepted when the WP0 review merges it)
- Date: 2026-09-25
- Work package: WP0 (spike S1b)
- Spec version: 5.0.1 → 5.1.0 (corrections of VERIFY items and new build rules; see Spec delta.
  One item, ⌘W, is left as an open decision.)

## Context

Spec 2, 3.2, 3.3 (C6), 5.1, 5.2, 7.1, 8.1 and WP0 S1b assume that ply builds its own `.node`
addon on GPUIX 0.10.0, links `gpuix-native` as an rlib, registers a native `<terminal>`
element from outside the GPUIX crate, and repaints it from a background thread without
JavaScript. S1b had to prove that with running code and settle the VERIFY items: the
`codegen-units` question, R-R2, R-R15 at 120 Hz, font registration, the key path, props and
events, the test renderer, the lockfile, the JS package arrangement and the build cost.

Sources, pinned: `vendor/gpuix` at 9fcd628 (`gpuix-native` 0.10.0) and its zed submodule at
81c99f8. Every `file:line` below is against those commits **before** the patches, with paths
relative to `vendor/gpuix` (zed files start with `zed/`). Crate sources are the registry
copies of napi 3.12.7, napi-derive 3.6.8, napi-build 2.4.4 and napi-sys 3.3.2.

The experiment lived outside the repo in a scratch directory that mirrors ply's layout
(`<scratch>/s1b`: a Cargo workspace with `crates/native`, `vendor → ply/vendor`, and a JS
root with `vendor/gpuix/packages/{native,react}` as bun workspaces). It is thrown away; the
patch files and the `justfile` are kept. Toolchain: Rust 1.97.1, Bun 1.3.10, macOS 26.6.2
on an 8-core Apple Silicon (M1 Pro class) machine with 16 GB, with three other spikes
compiling at the same time (load average 4–30 during the runs). No Xcode is installed, only
the Command Line Tools.

Two machine facts limit what could be measured:

- Both connected displays report `NSScreen.maximumFramesPerSecond = 60` (measured through
  AppKit from the spike; the built-in display's ProMotion is not available at the current
  setting). **No 120 Hz frame interval could be observed.** The 120 Hz answer below is a
  simulation over measured pump timestamps, not an observation.
- There is no Metal compiler (`xcrun -f metal` fails), so GPUIX's precompiled-shader build
  cannot run here (Decision 4).

## Decision

### 1. Patch 0001 — `patches/gpuix/0001-element-registry.patch`

GPUIX keeps its element system crate-private: `mod custom_elements;` (packages/native/src/lib.rs:19),
`pub(crate) struct GpuixView` (packages/native/src/renderer.rs:3387), `pub(crate) type EventCallback`
(renderer.rs:95,97), `pub(crate) fn emit_event_full` (renderer.rs:5924), `pub(crate) fn custom_surface`
(packages/native/src/custom_elements/mod.rs:110), and the registry only knows its built-ins
(`with_defaults`, custom_elements/mod.rs:314-325), which every `GpuixView` creates for
itself (renderer.rs:3671). An outside crate therefore cannot implement `CustomElement`,
whose `render` takes `&mut gpui::Context<GpuixView>` (custom_elements/mod.rs:205-213).

The patch (149 lines, 4 files under `packages/native/src`) does exactly this:

| Change | Why an outside element needs it |
|---|---|
| `pub use custom_elements::{custom_surface, register_global_factory, CustomElement, CustomElementFactory, CustomRenderContext}` in lib.rs; the module stays private | implement the traits; `CustomRenderContext` is what `render` receives |
| `pub struct GpuixView` (fields stay `pub(crate)`) | name `Context<GpuixView>` in the trait impl; `cx.spawn` on it (R-R2) |
| `pub type EventCallback` | store `ctx.event_callback` in element state |
| `pub fn emit_event_full` | emit `terminalEvent` the way built-ins emit theirs |
| `pub fn custom_surface` | styles incl. hover/active, the automation bounds tracker (custom_elements/mod.rs:128) that `getByTestId` needs, accessibility, standard mouse events |
| `pub use text::log_painted_text` | make the terminal's rows visible to `getPaintedText()` in tests (the canvas paint bypasses GPUIX's text funnel) |
| `register_global_factory(fn() -> Box<dyn CustomElementFactory>)`, a `static Mutex<Vec<_>>` | register before any renderer exists, i.e. from `#[napi_derive::module_init]` |
| `with_defaults()` registers the global factories after the built-ins | every renderer (live and test) gets `<terminal>`; a global type replaces a built-in of the same name |
| `CustomElementFactory::init(&self, cx: &mut gpui::App) {}` (default no-op), called by `init_global_factories(cx)` next to `input::init` at all four app-creation sites (renderer.rs:1159, 1292, 2702; packages/native/src/test_renderer.rs:237) | the element's one-time app init — fonts (Decision 11) and any key bindings. It runs before the window opens and, on macOS, before `app_menu::init` reads the keymap |

A function pointer, not a boxed factory, is stored because the static must be `Sync` and
`CustomElementFactory` is not required to be `Send`.

GPUIX's `docs/custom-elements-plan.md` Phase 2 (lines 56-68, 504-505) is a different
mechanism — separate `.node` files sharing a GPUI dylib and a runtime `plugin.attach`. Patch
0001 is the static-link variant: one binary, registration before the renderer exists. It is
offered upstream as a registration hook that Phase 2 would also need, not as Phase 2.

### 2. Patch 0002 — `patches/gpuix/0002-event-props.patch`

Two lines in `packages/native/js`:

- `["onTerminalEvent", "terminalEvent"]` appended to `EVENT_PROPS` (mutations.ts:73-97).
  Without it the prop is not an event: `isReservedProp` (mutations.ts:129-131) is false, so
  React's host config forwards it as a custom prop (packages/react/src/reconciler/host-config.ts:130-142)
  and `serializeCustomProp` turns the function into `null` (mutations.ts:133-137). With it,
  `syncEventListeners` (host-config.ts:86-94) registers the handler and sends
  `setEventListener(id, "terminalEvent", true)`.
- `onTerminalEvent?: (event: EventPayload) => void` in `HostProps` next to
  `onMotionComplete` (packages/native/js/host.ts:426). **Measured:** with only the first
  line, `tsc` of `@gpuix/react` fails (TS7053 at host-config.ts:103-104, indexing `Props`
  with the widened `EVENT_PROPS` names), so the published JS cannot even be rebuilt.

Nothing else is needed on the event path: dispatch is by `(elementId, eventType)`
(packages/native/js/renderer-state.ts:116-117), and `EventPayload.value: Option<String>`
already exists (packages/native/src/element_tree.rs:96).

### 3. `just vendor-patch`

`justfile` recipe: for each `patches/gpuix/*.patch` in order, skip it if
`git apply --reverse --check` succeeds (already applied), otherwise `git apply --check`
then `git apply`, run inside `vendor/gpuix`. **Measured:** first run applies both; second run
skips both; with 0002 reverted by hand it skips 0001 and re-applies 0002; a patch that neither
applies nor reverses fails the recipe. Reverting is `git -C vendor/gpuix checkout .`.

### 4. The Cargo workspace (controller item 1)

- **`vendor` must be excluded from the workspace.** Cargo makes every path dependency inside
  the workspace root a member. **Measured:** with `vendor/` under the root and no exclude,
  loading fails at the first zed crate: `error inheriting edition from workspace root
  manifest's workspace.package.edition — workspace.package.edition was not defined`
  (zed/crates/gpui/Cargo.toml inherits from zed/Cargo.toml:267-269). With
  `[workspace] exclude = ["vendor"]` each zed crate inherits from zed/Cargo.toml as in GPUIX's
  own build.
- **Seeding the lock works and changes nothing.** `packages/native/Cargo.lock` copied to the
  root and resolved (`cargo metadata`, not `--locked`): all 817 shared packages keep their
  versions; the resolve adds `ply-native` and drops `rmp`/`rmp-serde` (gpuix-native's
  dev-dependencies, packages/native/Cargo.toml:104-105, unused when it is not the root).
  `--locked` against the unmodified seed fails, as it must; WP1 commits the resolved lock.
- **A fresh resolve does not break compilation today, but drifts.** `cargo generate-lockfile`
  without a seed: 829 packages, 206 of them at other versions than GPUIX's lock (e.g. `read-fonts`,
  `skrifa`, `toml` 0.9→1.1, `tiff` 0.10→0.11, `zune-jpeg`, `tokio` 1.50→1.53). With it,
  `cargo check -p ply-native --locked` passes (79.8 s). ply seeds anyway (Ruling R13) so it
  builds exactly what GPUIX builds and tests.
- **No `[patch]` from zed is needed.** zed/Cargo.toml:964-977 patches `async-task`,
  `async-process`, `notify`, `calloop` and others to git revisions, but that section only
  applies when zed is the root. GPUIX's own build does not apply it either: its lock has
  registry `async-task` 4.7.1 and `async-process` 2.5.0. ply matches GPUIX by not adding it.
- **Versions in the lock (controller item 3):** `napi` 3.12.7, `napi-derive` 3.6.8,
  `napi-build` 2.4.4, `napi-sys` 3.3.2, `napi-derive-backend` 6.1.4, `ctor` 1.0.13,
  `futures` 0.3.32. The npm side pins `@napi-rs/cli` 3.10.4 (packages/native/package.json);
  ply does not need it: the addon is built with plain `cargo build` and the `.dylib` is copied
  to `.node` (measured, loads).
- **Release profile of gpuix-native to copy (controller item 3):** `[profile.release] lto =
  true` (packages/native/Cargo.toml:115-116) and `opt-level = 3` for `png`, `image`,
  `fdeflate`, `flate2`, `miniz_oxide` in dev (packages/native/Cargo.toml:121-134). No
  `codegen-units`, `panic` or `strip` setting. **Measured:** Cargo rejects `lto` in a
  per-package profile (`lto may not be specified in a package profile`), so spec 8.1's
  `[profile.release.package.gpuix-native]` cannot hold it; `lto = true` must sit on a whole
  profile. The dev-only `opt-level` overrides copy as `[profile.dev.package.<crate>]`.
- **Metal shaders.** `gpui_apple`'s build script runs `xcrun metal` unless feature
  `runtime_shaders` is on (zed/crates/gpui_apple/build.rs:20-24, 124-172); GPUIX documents
  the Metal toolchain as a prerequisite (AGENTS.md:1053-1061). Without Xcode, ply-native
  enables `gpui_macos/runtime_shaders` through a direct dependency (feature unification
  reaches gpuix-native's copy). This spike built and ran every variant that way; the shader
  compile then happens at window creation and was not timed separately (`createTestRoot()`,
  which includes it and the font registration, took 347–702 ms).

The `crates/native` manifest that worked (copy for WP1/WP5):

```toml
[package]
name = "ply-native"
edition = "2024"            # napi-derive 3.6.8 macros compile under 2024

[lib]
crate-type = ["cdylib"]

[features]
default = ["test-support"]
test-support = ["gpuix-native/test-support"]

[dependencies]
gpuix-native = { path = "../../vendor/gpuix/packages/native", default-features = false }
gpui = { path = "../../vendor/gpuix/zed/crates/gpui", default-features = false }
napi = { version = "=3.12.7", features = ["napi8"] }
napi-derive = "=3.6.8"
futures = "0.3"             # bounded mpsc for the waker
serde_json = "1"

[target.'cfg(target_os = "macos")'.dependencies]
gpui_macos = { path = "../../vendor/gpuix/zed/crates/gpui_macos", default-features = false, features = ["runtime_shaders"] }

[build-dependencies]
napi-build = "=2.4.4"       # build.rs: napi_build::setup(), as GPUIX's own build.rs
```

`napi-build` is kept for parity with packages/native/build.rs, but it is not required on
macOS: napi's default feature `dyn-symbols` (napi 3.12.7 Cargo.toml:57-62) resolves N-API at
runtime through `libloading`. **Measured:** without `build.rs` the addon links with 0
undefined `napi_*` symbols and loads.

### 5. napi registrations survive linking; `codegen-units = 1` is not needed

`#[napi_derive::module_init]` expands to a `ctor` (napi-derive 3.6.8 src/lib.rs:183-192),
the same mechanism as every `#[napi]` registration. **Measured** on ply's cdylib, loaded
through `NAPI_RS_NATIVE_LIBRARY_PATH`:

| Build | Exports present |
|---|---|
| release, fat LTO, default codegen-units | `AvailableUpdate`, `CheckUpdateTask`, `GpuixRenderer`, `InstallUpdateTask`, `PromptForPathsTask`, `TestGpuixRenderer`, `checkUpdate`, `hasTestGpuixRenderer`, plus ply's own functions; `module_init` ran |
| dev (no LTO, dev-profile codegen units, incremental) | the same set; `module_init` ran |

That is every export GPUIX's `index.d.ts` declares, plus the three napi task classes. No
`codegen-units` override and no `-force_load` is needed.

### 6. Loading ply's addon, and the JS arrangement (controller item 2)

The generated loader honours `NAPI_RS_NATIVE_LIBRARY_PATH` first (packages/native/index.js:70-83)
and `require`s it through `createRequire(import.meta.url)` (index.js:5-6), so the value must
be an absolute path. It re-exports only five names (index.js:719); ply's own exports
(`plyStats`) are reached by `require`-ing the same path directly. **Measured:** the directly
required module and `@gpuix/native` share one `GpuixRenderer` class (one `dlopen`). Without
the variable, the vendored package has no prebuilt `.node`, so the import throws `Cannot
find native binding` — there is no silent fallback to a stock binary.

Because patch 0002 edits GPUIX's JS, ply never uses the npm-published `@gpuix/*`. The shape
that ran (Bun 1.3.10):

```jsonc
// package.json (repo root)
{ "private": true, "type": "module", "packageManager": "bun@1.3.10",
  "workspaces": ["app", "vendor/gpuix/packages/native", "vendor/gpuix/packages/react"] }
// app/package.json
{ "dependencies": { "@gpuix/native": "workspace:*", "@gpuix/react": "workspace:*", "react": "19.2.4" } }
```

```toml
# bunfig.toml (repo root) — both keys are needed: `bun test` reads only [test].preload
preload = ["./app/src/native/preload.ts"]
[test]
preload = ["./app/src/native/preload.ts"]
```

`preload.ts` sets `process.env.NAPI_RS_NATIVE_LIBRARY_PATH` to the absolute path of
`app/native/ply-native.darwin-arm64.node` before any module imports `@gpuix/native`. After
`just vendor-patch`, the vendored JS is compiled: `bun run --cwd vendor/gpuix/packages/native
build:js`, then `bun run --cwd vendor/gpuix/packages/react build` (native first: React's
`tsc` reads native's `dist/*.d.ts`). The resolve gives `react` 19.2.4 and `react-reconciler`
0.31.0, the same as GPUIX's own bun.lock:1088. Bun's isolated linker writes `node_modules/`
inside `vendor/gpuix/packages/{native,react}`, and `tsc` writes `dist/`; both are in
vendor/gpuix's `.gitignore`, so the submodule stays clean.

`<terminal>` is typed by augmenting `@gpuix/react/jsx-runtime`'s `JSX.IntrinsicElements`
with a `TerminalProps extends Props` (`Props` is exported by `@gpuix/react`). **Measured**
with `tsc`: a valid element passes, `paneId="one"` fails with TS2322. Augmenting
`@gpuix/react/jsx-dev-runtime` as well fails to resolve (TS2664) and is not needed.

### 7. Props, events and the element id (brief item 7)

- **Props:** host-config sends every prop that is not `style/className/children/key/ref` or
  an event (host-config.ts:130-142); `autoFocus` and `testId` are taken by the retained tree
  (packages/native/src/retained_tree.rs:389-396); the rest reaches `CustomElement::set_prop`,
  supported props first, then any other key (custom_elements/mod.rs:267-296), removed props
  as `null`. **Measured:** `tabIndex` and an unknown `unknownProp` both arrived in `set_prop`.
  So the element must ignore GPUIX's universal props (`tabIndex`, `motion`, `highlight`,
  `role`, `aria-*`, mutations.ts:113-127) silently and log only keys outside that list (C6).
- **Events:** the element emits `emit_event_full(ctx.event_callback, ctx.id, "terminalEvent",
  |p| p.value = Some(json))`. `ctx.events` holds `terminalEvent` only when React registered a
  handler **and** `supported_events()` lists it (custom_elements/mod.rs:374-379), which is how
  the element knows a listener exists. **Measured:** JSON `{"kind":"title",…}`,
  `{"kind":"focus",…}`, `{"kind":"key",…}` reached the React `onTerminalEvent` handler, live
  and in the test renderer. The callback type is `Arc<dyn Fn(EventPayload) + Send + Sync>`
  (renderer.rs:95) calling a non-blocking threadsafe function (renderer.rs:917-923).
- **Element id:** GPUIX's rule is `ElementId::Name("__gpuix_<kind>_<host id>")` (AGENTS.md:389-400;
  e.g. packages/native/src/custom_elements/code.rs:241), the host id being `ctx.id`. ply uses
  `__gpuix_terminal_<host id>`, not `<pane>` (Spec delta). Custom-element state is keyed by host
  id anyway (custom_elements/mod.rs:334-354): a React remount creates a new element instance
  whatever the GPUI id is. Bounds are recorded in paint by `custom_surface`'s tracker.

### 8. Repaint without JS (R-R2, brief item 4)

On first `render`, the element spawns one data-plane std thread and one GPUI task with
`cx.spawn(async move |view, cx| …)` on the `Context<GpuixView>` it receives. The thread
sets an `AtomicBool` and does `try_send(())` on a `futures::channel::mpsc::channel(1)` only
when the flag was clear (a burst gives one wake); the task clears the flag and calls
`view.update(cx, |_, cx| cx.notify())`. `destroy()` stops the thread and drops the `Task`.
GPUI runs the task on the main queue (zed/crates/gpui_macos/src/dispatcher.rs:55-60), which
the JS pump drains in `tick()` (zed/crates/gpui_macos/src/platform.rs:277-303, 624-645).

**Measured, live window, production build (no `test-support`), 60 Hz display, 240 Hz
producer, zero React renders during every phase:**

| Pump | Presented | signal → paint p50 / p95 | Pump ticks | Process CPU |
|---|---|---|---|---|
| 8 ms (GPUIX default) | 59.8–60.0 Hz, 1 GpuixView render per frame | 5.0–5.3 / 7.7 ms | 121.7–126.8 Hz | 5.7–6.2 % |
| 4 ms | 60.0 Hz | 2.0 / 4.9 ms | 286.8 Hz | 6.6 % |
| 1 ms | 60.0 Hz | 2.4 / 4.8 ms | 841.3 Hz | 8.2 % |
| 8 ms, 60 Hz producer | 49.9 Hz (producer/vsync beat) | 10.4 / 18.8 ms | 124.4 Hz | 5.3 % |
| 8 ms, 1 Hz producer | 1 frame per wake | — | 126.8 Hz | 1.6 % |

Three GPUI behaviours decide what these numbers mean:

- **`test-support` changes pacing.** With the feature (GPUIX's default, so that published
  binaries carry the test renderer, packages/native/Cargo.toml:94-96; also its `build` script), GPUI draws every dirty window at the end of each effect flush
  (zed/crates/gpui/src/app.rs:1711-1723). **Measured:** 128–130 scene builds per second on a
  60 Hz display (one per wake), presentation still vsync-paced. Without it, drawing happens
  only in the display-link frame callback (zed/crates/gpui/src/window.rs:1636-1661).
- **A window that macOS does not report visible draws nothing** in the production build: the
  display link is not started (zed/crates/gpui_macos/src/window.rs:759-768). **Measured:** a
  window opened with `focus: false` behind other apps rendered 0 frames during 25 s of measurement; floated
  above them (spike-only `setLevel` + `orderFrontRegardless`, no activation) it ran at 60 Hz.
- **Every `notify` rebuilds the whole React tree.** `GpuixView::render` rebuilds every
  retained node each frame (renderer.rs:4663-4815), so a terminal repaint costs a full chrome
  rebuild. **Measured** (live, production build, GPUI's own draw timer via
  `getDebugFrameOverlayStats`): ~24 host nodes p90 0.75 ms; ~504 nodes p90 5.0 ms / p99
  6.4 ms; ~2004 nodes p90 20.3 ms, presenting only 48 Hz on a 60 Hz display.

### 9. The 8 ms pump at 120 Hz (R-R15, brief item 4)

The pump is `setTimeout(loop, max(0, frameMs - elapsed))` with `frameMs = 8`
(packages/native/js/runtime.ts:13, 49-84), started by `render()` (packages/react/src/reconciler/renderer.ts:411-416).
Measured tick rate: 121.7–127.1 Hz, interval p50 7.9–8.0 ms, p95 8.0–9.1 ms. The display link
posts to the main queue at vsync and coalesces (`merge_data(1)`,
zed/crates/gpui_macos/src/display_link.rs:119-135), so a vsync with no tick before the next
vsync is a lost frame. Replaying the measured tick timestamps against a 120 Hz vsync train
(20 phases each) loses **0–0.27 %** of vsyncs under normal load and **1.9 %** while a
fat-LTO link ran next to it; a 4 ms pump loses 0 %. Every served vsync waits up to one tick
(0–8 ms). So the 8 ms pump does not cap 120 Hz on average (ticks ≥ 121 Hz), but it has no
margin: an 8.33 ms period against an 8 ms nominal tick. This was **not observed** on a
120 Hz display (Context). If WP5's P1 run on a 120 Hz display shows drops, ply runs its own
loop through public API: `createRenderer()`, `render(node, { renderer })` (no loop is
started for an injected renderer, renderer.ts:411) and `startFrameLoop(renderer, { frameMs:
4 })`.

### 10. Keys, IME, mouse, focus, clipboard (brief item 5)

GPUI dispatch order (zed/crates/gpui/src/window.rs:5457-5700): keystroke interceptors, then
key **bindings/actions** on the focus path (5608-5620, which stop dispatch when handled), then
key listeners — root capture, path capture, path bubble from the focused element up, root
bubble (5653-5702). GPUIX's window-level `render({onKeyDown})` is a root **bubble** listener
(renderer.rs:3522-3572). On macOS, `handle_key_event` (zed/crates/gpui_macos/src/window.rs:2407-2535)
sends a key to the IME first when text is being composed, when the key has no printable
character (dead keys), or when a CJK IME is active and the input handler prefers IME
(`ElementInputHandler` answers `accepts_text_input`, zed/crates/gpui/src/input.rs:243-246);
otherwise GPUI dispatch runs first and the IME (`insertText:`) only gets keys that were
**not** handled.

- **Focus:** GPUIX creates a `FocusHandle` for an element only if it has `tabIndex` or a
  key/focus listener (renderer.rs:4586-4592); `ctx.focus_handle` is then set
  (renderer.rs:4899). ply puts `tabIndex={0}` on `<terminal>`; the element calls
  `.track_focus(handle)`, which also focuses it on mouse down (zed/crates/gpui/src/elements/div.rs:2701-2715).
  **Measured:** `focusElement(id)`, `getFocusedElementId()` and an automation `click()` all
  focus the terminal. Focus changes re-render, so the element sees them in `render`; GPUIX's
  own `onFocus`/`onBlur` also work through the same handle (renderer.rs:4641-4655).
- **Keys:** `on_key_down`/`on_key_up` on the element's div; `cx.stop_propagation()` there
  keeps the key from its ancestors and from `render({onKeyDown})`. **Measured** (live
  production build and test renderer):

  | Keystroke | Element | Reaches `render({onKeyDown})` |
  |---|---|---|
  | `a` (not consumed by the spike) | `EntityInputHandler::replace_text_in_range("a")` | yes |
  | `enter`, `tab`, `shift-tab`, `escape`, `ctrl-c`, `alt-b` | `on_key_down`, propagation stopped | no |
  | `cmd-t` (in `passthroughKeys`), `cmd-k` | seen, not stopped | yes |
  | `cmd-c`, `cmd-v` | `on_key_down`, stopped (K7) | no |
  | `cmd-w` | live: the window closed and the process exited (measured). GPUIX binds ⌘W to `CloseWindow` (packages/native/src/app_menu.rs:44-50) and bindings run before key listeners (window.rs:5608-5620), so the listener cannot see it. Test renderer: bubbles, because it installs no app menu | live no / test yes (measured) |

  Rule for WP5: a key counts as text only if `key_char` is non-control (the same test GPUI
  uses, zed/crates/gpui_macos/src/window.rs:2468); `enter`/`tab` carry `"\n"`/`"\t"` in `key_char`
  and must be encoded as keys. ply should consume plain text in `on_key_down` too (KEY with
  the text), because anything left to propagate becomes a JS `windowKeyDown` event per
  keystroke; IME composition, dead keys, the emoji picker and dictation still arrive through
  the `EntityInputHandler`, registered in paint with `window.handle_input(&focus_handle,
  ElementInputHandler::new(bounds, entity), cx)` (zed/crates/gpui/src/window.rs:5023-5037;
  GPUIX's input does the same at packages/native/src/custom_elements/input.rs:1866).
- **`passthroughKeys`:** a list of GPUI keystroke strings in `Keystroke::unparse` form,
  `fn-ctrl-alt-cmd-shift-<key>` (zed/crates/gpui/src/platform/keystroke.rs:750-776). The
  element returns without handling or stopping them, so they reach `render({onKeyDown})`.
  Since the element never stops ⌘ chords except ⌘C/⌘V/⌘A, the list only matters for chords
  the element would otherwise own.
- **Mouse:** `on_mouse_down/up/move` and `on_scroll_wheel` on the element's div, with
  `cx.stop_propagation()` for wheel events it consumes (as input.rs:1336-1342). **Measured:**
  an automation click delivered `{"kind":"mouse","x":230,"y":96}` and focused the pane.
- **Clipboard (K7):** GPUIX installs no Edit menu (app_menu.rs:11-15), so ⌘C/⌘V reach the
  element (measured). The element uses `cx.write_to_clipboard(ClipboardItem::new_string(…))`
  and `cx.read_from_clipboard()` as GPUIX's input does (input.rs:1078, 1097). Not exercised:
  the test renderer runs the real `MacPlatform` (test_renderer.rs:234,
  zed/crates/gpui_platform/src/gpui_platform.rs:57-61), whose pasteboard is the system one
  (zed/crates/gpui_macos/src/platform.rs:241, 1352-1355), so a copy test would overwrite the
  user's clipboard.
- **IME composition itself was not driven** (no API from this session reaches
  `NSTextInputContext`); only the `insertText` path via `dispatch_keystroke` →
  `dispatch_input` (zed/crates/gpui/src/window.rs:5185-5207) was exercised. WP5 checks dead keys and a CJK IME by
  hand.

### 11. Fonts (brief item 6)

- **Registration:** in `CustomElementFactory::init` (patch 0001), `cx.text_system().add_fonts(
  vec![Cow::…(bytes)])` (zed/crates/gpui/src/text_system.rs:102-104). **Measured:** the six
  files Geist and Geist Mono 400/500/600 (829 KB) register in 0.8–1.2 ms (reading them:
  0.4–2.6 ms); `Geist Mono` is absent from `all_font_names()` before and present after (it
  is not installed on this machine); the terminal's run resolves to family `Geist Mono`, and a
  GPUIX `<text style={{fontFamily: "Geist Mono"}}>` measures 121 px for ten `m` at 20 px.
  `Cow::Borrowed(&'static [u8])` (from `include_bytes!`) avoids a copy
  (zed/crates/gpui_macos/src/text_system.rs:256-268).
- **Registration must precede the first lookup.** GPUI caches a failed lookup per `Font`
  value (text_system.rs:107-128). **Measured:** with registration moved to the element's first
  render and one earlier frame that showed `<text fontFamily="Geist Mono">`, that text stays
  on the fallback (167 px, Helvetica) after the fonts are added, while the terminal (a
  different `Font` key) resolves to Geist Mono. Registering in the first render works only if
  no earlier frame asked for the family — hence the factory `init` hook.
- **Fallbacks:** inside the element, `gpui::Font { fallbacks: Some(FontFallbacks::from_fonts(
  vec!["Menlo".into(), "Apple Color Emoji".into()])), ..gpui::font("Geist Mono") }`
  (zed/crates/gpui/src/text_system/font_fallbacks.rs:18, text_system.rs:1077). GPUIX's
  `fontFamily` style maps to `font_family(family)` with no fallback list (renderer.rs:5754),
  so React chrome cannot declare `FontFallbacks`; a family that fails to load falls back to
  GPUI's fixed stack, Helvetica first on macOS (text_system.rs:71-83).

### 12. The test renderer and automation client (brief item 8)

`createTestRoot()` (packages/react/src/testing.ts:20-38) constructs `TestGpuixRenderer`
through the same loader, so it loads ply's addon via `NAPI_RS_NATIVE_LIBRARY_PATH`.
**Measured with ply's addon:** `render` + `flush`, `getPaintedText()` (`["mmmmmmmmmm",
"pane 7 · counter 32 · renders 9"]`), `nativeSimulateKeystrokes(id, …)`, `captureScreenshot`
(PNG written and inspected), and `connectTest(renderer)` → `getByTestId("term").bounds()`
(`{x:0,y:36,width:460,height:120}`), `.click()` and `.press("escape")`. `bun test` runs it
(1 pass) although GPUIX's own suites use vitest (AGENTS.md:1302). Rules WP5 must know:

- The class exists only in a `test-support` build (packages/native/src/lib.rs:36-40); a
  production build's constructor throws (lib.rs:69-92).
- `flush()` is `notify` plus `run_until_parked` (test_renderer.rs:310-324). Events emitted
  while rendering stay queued until `dispatchNativeEvents()`, `advanceTime()` or a
  `nativeSimulate*` call (packages/native/js/testing.ts:223-236, 498-501). Measured.
- The waker's GPUI task runs inside the test dispatcher: after 150 ms, `advanceTime(0)` ran it
  once (wakes 1→2, counter 4→32).
- **A producer that wakes faster than a frame renders stalls `flush()`**: `run_until_parked`
  keeps finding the wake task runnable. **Measured** with a 240 Hz producer (4.2 ms period):
  a ~24-node tree (0.24 ms per flush) ran 400 flushes normally; a ~504-node tree (5.6 ms per
  flush) completed 4 flushes in 40 s; the three-size script ran 14 min at 100 % CPU without
  finishing. With the producer at 1 Hz all sizes finished. Tests must drive the data plane
  deterministically (paused, or fed by the test).
- The test renderer builds the accessibility tree on every flush (test_renderer.rs:268), has
  no app menu bindings (⌘W bubbles there), and uses the real pasteboard.

### 13. Build cost and size (brief item 9)

| Build | Wall | Notes |
|---|---|---|
| Cold release, `test-support`, fat LTO, fresh target dir, registry warm | **5 min 53 s** (354 s; 979 s CPU; peak RSS 2.9 GB) | load average 15.6 at start. An earlier run split it as 164 s of dependencies plus 143 s for ply-native and the LTO link (≈ 5 min 7 s) |
| Release relink after an edit in `crates/native` | 2 min 9 s – 2 min 45 s | fat LTO, single-threaded, 2.6 GB RSS |
| Release, switching `test-support` off | 3 min 6 s | features change for gpui, gpui_macos, gpui_platform |
| Cold dev build | 1 min 45 s | no LTO |
| Dev rebuild after an edit / a `touch` | 4.6 s / 2.2 s | GPUIX's own loop uses debug builds (scripts/dev.ts, `build:debug`) |

| Artifact | Size | `strip -x` |
|---|---|---|
| release, `test-support` | 18,348,016 B | 15,591,376 B |
| release, no `test-support` | 16,870,016 B | 14,366,976 B |
| dev | 86 MB | — |

The linker signs the dylib ad hoc; stripping invalidates that signature (WP10 re-signs).

## Consequences

- ply builds two addons: a `test-support` build for development and `bun test` (it contains
  `TestGpuixRenderer`), and a production build without it for shipping and for every
  performance measurement (P1–P5). GPUIX's default build and its npm binaries include
  `test-support` and draw on every notify, so they are not representative for frame numbers.
- ⌘W, ⌘Q, ⌘H, ⌥⌘H and ⌘M are bound by GPUIX's app menu (app_menu.rs:44-50) and never reach
  `render({onKeyDown})`. ⌘W closes the window, which quits the app (QuitMode::LastWindowClosed,
  renderer.rs:1157). K3's "⌘W closes the pane" is not reachable with patches 0001–0002
  (open decision, Spec delta).
- The per-frame cost of a terminal repaint includes a full rebuild of the React tree: ~0.01 ms
  per host node measured (0.75 ms at 24 nodes, 5.0 ms at 504, 20.3 ms at 2004, p90). At
  R-R16's 2 000-node cap the window cannot hold 60 Hz while a pane streams. From these points
  (an estimate, not a measurement of ply's chrome), P1's 8.3 ms p99 leaves room for a few
  hundred chrome nodes plus the panes' own painting.
- Idle cost: the whole process with one idle pane used **1.72 %** of one core with the 8 ms
  pump and **1.37 %** with 16 ms (10 s samples, production build). GPUIX's own comment gives
  1.5 % for the paced loop (packages/react/src/reconciler/renderer.ts:81-84). P3's 0.5 % is not
  met by the unmodified runtime; WP11 decides.
- `createRenderer()` starts GPUIX's stdio automation server whenever stdin is not a TTY
  (packages/native/js/runtime.ts:39-45). A packaged app launched with a pipe on stdin exposes
  click/type/screenshot to its parent; with `/dev/null` it reads EOF. WP10 keeps stdin
  `/dev/null`.
- Every Rust edit to the addon costs a 2–3 minute fat-LTO relink in release; the dev loop uses
  the dev profile (4.6 s) and release only for perf.
- Cargo prints gpuix-native's two pre-existing warnings (dead code at
  custom_elements/input.rs:264 and markdown/parser.rs:81) because path dependencies are not
  lint-capped, so `RUSTFLAGS=-Dwarnings` would turn them into errors (inferred, not run). CI
  should pass `-D warnings` to clippy (`cargo clippy -p <crate> -- -D warnings`), which lints
  the selected packages rather than their path dependencies (not run in this spike).
- The fonts are registered once per app from the factory `init`; the chrome can only use
  families registered there (no fallback list in GPUIX styles).
- Element state lives in GPUIX's registry, keyed by host id (custom_elements/mod.rs:332-354,
  400-404): a React remount of a pane destroys and re-creates the element, so the pane
  re-attaches C2 and receives a Snapshot (R-R20). Pane components need stable keys.

## Spec delta

Confirms:

- 2 "Native bindings": `napi` 3.12.7, `napi-derive` 3.6.8 (lock); ply-native links
  `gpuix-native` (`crate-type` includes `rlib`, packages/native/Cargo.toml:10) and registers in
  `#[napi_derive::module_init]` (a ctor). **VERIFY S1b resolved: `codegen-units = 1` is not
  needed** — all registrations survive fat-LTO release and no-LTO dev builds (Decision 5).
- 2 "Terminal rendering" and 3.2: an outside crate registers and repaints a native element
  through GPUIX's custom-element API, once patch 0001 is applied.
- 2 "JS runtime" / 8.1: the napi loader honours `NAPI_RS_NATIVE_LIBRARY_PATH` (index.js:70-83);
  the value must be absolute.
- 3.3 C6: props arrive through `setCustomProp` / `set_prop`; `onTerminalEvent` carries JSON in
  `EventPayload.value`; typing by augmenting `@gpuix/react/jsx-runtime` works.
- 5.2 R-R2 (**VERIFY S1b resolved**): `cx.spawn` on the render `Context` plus `notify`,
  woken by an `AtomicBool` and a capacity-1 channel, repaints with no JS work (Decision 8).
- 5.2 R-R15: the pump is an 8 ms `setTimeout`, precisely `max(0, 8 - elapsed)` (runtime.ts:13, 80).
- 7.1 K6: GPUI actions, then the focused element, then ancestors, then the window listener;
  only Rust stops propagation. K7: no Edit menu; ⌘C/⌘V reach the element.
- 8.1: `crate-type` includes `rlib`; custom elements are crate-private without the patch.

Corrects (5.1.0):

- **2 "Native bindings"** — add `napi-build` 2.4.4 (optional on macOS, kept for parity) and
  note that `@napi-rs/cli` is not needed to build ply's addon (`cargo build` + copy).
- **2 "Fonts"** — "from the element's init" becomes "from `CustomElementFactory::init`, a hook
  patch 0001 adds, called once per app before the window opens"; registration must precede
  any lookup of the family (failed lookups are cached). `gpui::FontFallbacks` applies inside
  the terminal element only; chrome text falls back to GPUI's fixed stack.
- **2 "Tests"** — `bun test` works with `createTestRoot()` (GPUIX itself uses vitest);
  `bunfig.toml` needs `[test] preload` as well; tests need the `test-support` build and a
  deterministic data plane (Decision 12).
- **2 "Build toolchain"** — add: ply-native enables `gpui_macos/runtime_shaders` unless the
  Metal toolchain (Xcode) is installed.
- **5.2 R-R15 / WP0 S1b "confirm the 8 ms tick does not cap a 120 Hz display"** — not
  observable on this machine (both displays 60 Hz). Measured ticks are 121.7–127.1 Hz; a
  simulation over them loses 0–1.9 % of 120 Hz vsyncs; a 4 ms pump loses none. Recheck in
  the P1 bench on a 120 Hz display; the fallback is ply's own `startFrameLoop(renderer,
  { frameMs: 4 })`.
- **5.2 R-R16** — the 2 000-node cap is inconsistent with P1 once every terminal frame rebuilds
  the whole tree (Consequences); to be set from the P1 bench (measured points above).
- **5.2 R-R19** — the element id is `ElementId::Name("__gpuix_terminal_<host id>")` (GPUIX's
  rule, AGENTS.md:398-400), not `<pane>`.
- **7.1 K3/K6** — ⌘W, ⌘Q, ⌘H, ⌥⌘H and ⌘M never reach the app keymap; they are GPUIX menu
  bindings. **Open decision for the owner:** keep ⌘W as "close window" (the app closes,
  sessions keep running per 11.3) and give "close pane" another chord; or a third patch that
  makes GPUIX's Window-menu binding configurable (INV-13 needs an ADR and an upstream offer);
  or a ply keybinding in the terminal's key context that wins only while a pane has focus.
- **8.1** — (a) the root `Cargo.toml` needs `[workspace] exclude = ["vendor"]`; (b) `lto` cannot
  go in `[profile.release.package.gpuix-native]`: it goes on a whole profile (`[profile.release]`
  or a dedicated profile used by `just build-native`), and the dev-only `opt-level` overrides
  copy as `[profile.dev.package.*]`; (c) `just build-native` builds two variants (with and
  without `test-support`); (d) the JS side is the bun workspace of Decision 6, with the vendored
  `dist/` built by `tsc` after `just vendor-patch`; (e) patch 0002 also adds the `HostProps`
  field; (f) the patches are a registration hook, not GPUIX's "Phase 2" (which is separate
  binaries sharing a GPUI dylib; the same wording is in ADR-0007).
- **8.2** — ply-native's allowed dependencies add `gpui_macos` (same zed path, for
  `runtime_shaders`), `napi`, `napi-derive`, `napi-build`, `futures` and `serde_json`; each
  third-party one needs its `deps.allow.toml` entry (INV-16).
- **1.2 P3** — the unmodified GPUIX runtime idles at 1.4–1.7 % of one core (Consequences).
