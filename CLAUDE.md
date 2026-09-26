# ply — Claude Code Instructions

## What this repository is

**ply**: a local macOS app that runs the real `claude` (Claude Code) and `codex`
(Codex CLI) TUIs in terminal panes — tabs across the top, up to four panes per tab —
and adds what a terminal multiplexer lacks: status per pane ("needs you", "your
turn", running, exited), progress from the agent's own plan, the model the
session reports, stored session history, the CLIs' own worktrees, and a queue of
tasks the user writes for a pane, typed when that pane is idle.

It is **not** an LLM wrapper. It never calls a model API, holds a key, rewrites a
prompt, hands a transcript between CLIs or picks a model or effort level; the
user changes those inside the session. ply manages terminal panes and tracks and
stores sessions, and nothing else.

`ARCHITECTURE.md` has the processes, the three wire boundaries and the module
map. `docs/` has one document per subject (see the doc map below).

## Where the pieces live

| | |
|---|---|
| the app | `app/` — Bun + React on GPUIX (`@gpuix/react` and `@gpuix/native` 0.10.0 from npm, used as released). `app/src/main.tsx` is the entry, `app/src/theme/tokens.ts` the only palette |
| the terminal view | `app/src/terminal/` (C2 codec, replica, runs, input, selection) and `app/src/features/panes/terminal-view.tsx` |
| the daemon | `crates/daemon` (`plyd`): ptys, one libghostty-vt terminal per pane, the C1 and C2 servers, SQLite, the LaunchAgent, the task queue and its dispatch (`panes/queue.rs`, `panes/dispatch.rs`), `skill.list`, `usage.get` (Claude panes' status line reports and the CLIs' own usage records, read-only) |
| the terminal engine | `crates/term` (`ply-term`) over `crates/ghostty-sys`, whose `build.rs` downloads pristine ghostty 44f2a44 (SHA-256-verified) and builds libghostty-vt from it with Zig |
| the agent adapters | `crates/agents` (`ply-agents`): launch specs, the Claude hooks file, hook/notify/OSC 9/rollout parsing, progress, the version check, the plan-usage parsers, skill discovery (`skills.rs`, read-only) |
| the hook helper | `crates/hook` (`ply-hook`): one hook or notify payload → one C3 line to plyd; as `ply-hook statusline`, Claude Code's status line in a pane |
| the wire types | `crates/proto` (`ply-proto`): C1, C2, C3; `app/src/ipc/proto.gen.ts` is generated from it |
| the bundle | `scripts/dmg.ts` (`just dmg`) → `dist/ply.app` and `dist/ply-<version>.dmg`; `app/src/bundle.ts` is the bundled app's entry |
| the gates | `scripts/check-rules.ts` (the invariants and layers), `scripts/check-deps.ts` + `deps.allow.toml` (dependency admission), `deny.toml` |
| tests | `#[cfg(test)]` inline, `crates/*/tests/` (golden frames, fake CLIs, replays), `*.test.ts(x)` beside the app code |

## Rules the tests enforce

**Raw pty bytes never enter JavaScript.** plyd emulates every pane; the app
receives decoded screen rows over C2 and draws them. C1 types carry no byte
fields and the app never opens a pty (`checkInv2NoPtyBytes`).

**No network, no GPUI, GPUIX as released.** No `fetch`, `WebSocket`,
`XMLHttpRequest` or remote `<img>` in `app/` (`checkInv1NoNetwork`); no ply crate
depends on `gpui` or an HTTP client (`checkInv3NoGpui`, `cargo deny`);
`@gpuix/*` are pinned exactly at 0.10.0 with no patches, overrides or local
builds (`checkInv13Gpuix`). Solve UI problems with GPUIX's public API; never
patch or fork a dependency or drop to GPUI in Rust.

**libghostty-vt is plyd's alone and stays pristine.** Only ply-daemon enables
ply-term's `engine` feature; `crates/ghostty-sys/build.rs` pins the ghostty
commit, its archive URL and the archive's SHA-256, rejects a download that
differs, and applies no local patches (`checkInv17Ghostty`).

**ply never decides for a CLI and never writes its config.** `ply-hook` prints
nothing and exits 0 on every path within 200 ms, with plyd up or down; as the
status line it prints only what the user's own status line prints
(`crates/hook/tests/hook.rs`). Claude Code gets a per-pane `--settings` file
holding only `hooks`, the `statusLine` (`ply-hook statusline` in front of the
user's own) and the optional theme, never WorktreeCreate or WorktreeRemove; Codex gets only `notify`, the two OSC 9 options and
`tools.update_plan.enabled` (`crates/agents/tests/launch.rs`). The user's
`~/.claude/settings.json`, `~/.claude.json` and `~/.codex/config.toml` are never
written (INV-8).

**ply types only a task the user wrote, and never answers for the CLI.** A queued
task is pasted, text unchanged, into an idle Claude or Codex pane whose CLI has
started a session, never into a shell, a pane the user is typing in or one showing
a dialog; failure pauses the pane's queue (`crates/daemon/src/panes/dispatch.rs`,
`crates/daemon/tests/dispatch.rs`). `skill.list` only reads the CLIs' skill folders.

**ply never manages worktrees.** The CLI creates and removes them; ply passes
`claude --worktree <name>` and shows what the CLI reports. The only worktree
column is `worktree_seen` (`checkInv7Worktrees`).

**One keymap, one palette.** Every binding is declared once in
`app/src/keymap/keymap.ts`; `keymap.test.ts` fails on duplicates, reserved chords
and chords without ⌘. `onKeyDown` appears only in `keymap/dispatcher.ts`, the
overlay forms and `terminal-view.tsx` (`checkInv4KeyHandlers`). No hex colour
outside `theme/tokens.ts` (`checkInv5Palette`).

**The protocols are strict and pinned by golden files.** `deny_unknown_fields`
on every C1/C3 struct, 1 MiB caps, version checks at `hello` and ATTACH; one
golden file per message in `crates/proto/tests/golden/`, and the TypeScript C2
codec must decode and re-encode `golden/c2/*.bin` byte for byte. `bun run gen`
must leave `proto.gen.ts` unchanged (`checkGeneratedTypes`).

**Layers are checked, not agreed.** `checkCrateLayers` and `checkAppLayers`
enforce the tables below: a crate's normal dependencies must all be in its row
(dev- and build-dependencies may go beyond it, never to a forbidden crate), and
`check-rules.test.ts` fails when the table and the check disagree;
`checkRustStandards` rejects `unwrap`/`expect` and
`unsafe` where they are not allowed; `checkExactPins` wants `=x.y.z`.

**Every dependency is admitted.** A new direct dependency needs a row in
`deps.allow.toml` with its repository, stars, last commit and number of
contributors with ≥ 5 commits (≥ 1 000 stars, a commit in the last 6 months,
≥ 3 such contributors, not experimental). `bun scripts/check-deps.ts --add` fills
it in. The exceptions by decision are GPUIX, libghostty-vt, `thiserror` and
`anyhow`.

**No personal data in the repository.** No real user names, `/Users/<name>`
paths, hostnames or machine values in code, docs, tests or fixtures; use
`/Users/example` (`checkInv11PersonalData`).

## The things that will bite you

**Always set `PLY_HOME` when testing.** Without it `bun run dev` (and the app)
starts plyd through `plyd install-agent`, which writes a real
`~/Library/LaunchAgents/dev.ply.app.plyd.plist` and kickstarts it, and plyd uses
the real `~/Library/Application Support/ply/`. With `PLY_HOME=<dir>` everything
lives under that directory and plyd runs in the foreground.

**libghostty-vt needs Zig 0.16.0 and, once, the network.** Set `ZIG` or put
`zig` on `PATH`. The first build of `ghostty-sys` downloads the ghostty source
(about 40 MB) into `~/Library/Caches/ply/ghostty/<commit>/`, plus any Zig
package Zig's cache lacks, and takes about a minute; later builds reuse the
caches and `zig build` itself runs offline under `OUT_DIR`. To build offline
set `PLY_GHOSTTY_SRC` to an extracted ghostty 44f2a44 tree (and
`PLY_ZIG_PKG_DIR` to the packages). Never build inside the cached source by
hand: `zig build` there writes `zig-out/`, `.zig-cache/` and `zig-pkg/` into
the tree every later build uses.

**Never run `codex --version`, `codex --help` or `codex doctor` on this
machine.** They start Codex's self-updater (it updated an install once). The
adapters read versions from the install layout instead. Never pass
permission-skipping flags to `claude` or `codex`.

**GPUIX owns ⌘Q, ⌘H, ⌥⌘H, ⌘M and ⌘W.** Its app menu binds them and they never
reach the app: ⌘W closes the window (sessions keep running). Close pane is
⌘⇧W. GPUIX binds neither Tab nor ⇧Tab, so both reach the terminal; JS key
handlers cannot stop propagation.

**AppKit sends no key-up for a key released while ⌘ is down**, and GPUIX
reports no modifier change. A binding that must notice a release (the ⌘U hold)
ends on the key's repeats stopping, any other key, or a blur, as well as the
key-up (`keymap/dispatcher.ts`, `docs/keybindings.md`).

**GPUIX is not the DOM.** Every `<text>` needs an explicit `color` (text does not
inherit it); `div` is block until `display: "flex"`; a shrinking flex child needs
`minWidth: 0`; style values are numbers, not CSS shorthand, and `boxShadow` is an
object; there is no `<button>` (use `<div onClick>`); never nest `<text>` inside
`<text>`. Each `<text>` is its own box and Taffy rounds every box, so a flex row
of fractional-width runs drifts: a terminal row is a `<div>` whose `<text>` runs
are each placed absolutely at `round(col × cell width)` (`terminal-row.tsx`,
since 22e57db), and rows are memoized and keyed by content. Every host node
costs about 0.01 ms per frame.

**Fonts.** GPUIX has no font-loading API, so Geist and Geist Mono
(`app/assets/fonts/`) are used only when installed in `~/Library/Fonts`
(`just fonts` copies them there; the owner runs it); otherwise the app falls
back to Menlo and the system font.

**The comment hook.** A machine-level hook rejects comment blocks longer than one
line (two with a tracking link). Doc comments state errors and safety inline
(`errors: …`, `safety: …`) instead of multi-line sections.

## Run it

```
cargo run -p ply-daemon -- --foreground        # plyd, with PLY_HOME=<dir> set
bun run dev                                    # the app, hot-reloaded, attaching to that plyd
cargo run -p ply-daemon --example ply-cli      # the command-line C1/C2 test client
```

## Build & test

```
just check      # fmt, clippy -D warnings, cargo doc -D warnings, check-rules, check-deps, biome, tsc
just test       # cargo nextest, doctests, bun test
just e2e        # journeys J1–J7 on the full app and the release plyd (opens windows, unfocused)
just deny       # licences, advisories, the HTTP-client and gpui bans
just gen        # regenerate app/src/ipc/proto.gen.ts
just fonts      # copy the Geist TTFs into ~/Library/Fonts (writes outside the checkout)
just dmg        # dist/ply.app and dist/ply-<version>.dmg: profile dist, Bun bytecode, ad-hoc signed
```

Toolchain: Rust 1.97.1 (`rust-toolchain.toml`), Zig 0.16.0, Bun 1.3.10, just.
The GitHub repository is public; push only when asked. `docs/development.md` walks through setup, the
run directory and the logs.

## Personal use

ply is built for one person's machine. It runs from this checkout — `bun run
dev` (or `bun app/src/main.tsx`) against a release `plyd` (`cargo build
--release -p ply-daemon -p ply-hook`) — or as the `.app` that `just dmg` builds.
The bundle is ad-hoc signed and not notarised, and there is no release workflow
and no distribution, so notarisation and licensing questions do not arise.
`docs/development.md`, **The app bundle**, has its layout and optimisations.

## Coding rules

**Rust.** `lib.rs` holds only `mod` declarations and re-exports. One `thiserror`
error enum per crate; `anyhow` only in a binary's `main.rs`. No `unwrap`/`expect`
outside tests. `#![forbid(unsafe_code)]` everywhere except `ghostty-sys`, ply-term's
`engine` module and the one `pre_exec` block in `crates/daemon/src/pty.rs`; every
`unsafe` block has a `// SAFETY:` line. Every error-handling site (`match Err`,
`if let Err`, block-bodied `map_err`, `unwrap_or_else`) logs through `tracing`
with `pane_id` when there is one; `?` needs no log. Async only in plyd; each pty
is read on its own std thread into a bounded channel; every channel is bounded
and every socket read is capped. Workspace dependencies are pinned exactly.

**TypeScript.** Strict, no `any`, named exports, kebab-case files, PascalCase
components. One store (`state/store.ts`) read through selectors; state changes
through typed actions and a pure reducer; `effects.ts` is the only ipc caller.
Features select state and dispatch actions; `ui/` is presentational. Colours,
fonts and sizes come from `tokens.ts`.

**Documentation.** Every crate opens with a `//!` doc: what it is for, the
contract it implements and what it must never do. Every public Rust item has a
`///` doc stating its contract — guarantees, units, error cases, threading and
blocking — never a restatement of its name (`missing_docs` is on and warnings are
errors). Exported TypeScript symbols carry TSDoc for what the types do not say;
each component gets a one-line role. A change that alters behaviour described in
`docs/` or `ARCHITECTURE.md` updates that document in the same commit.

**Comments.** Only a non-obvious why: a hidden invariant, a workaround with its
upstream link, a surprising performance choice. One line. No restating the code,
no dividers, no commented-out code, no TODO without an owner or trigger.

**Tests.** Behaviour, not mocks. Daemon behaviour is tested with fake CLIs
(`crates/daemon/tests/fake/`) and a sandboxed `HOME`/`CODEX_HOME`; UI with
GPUIX's test renderer (`@gpuix/react/testing`) and the mock daemon
(`app/src/ipc/mock-server.ts`, tests only). Output stays free of warnings.

**Commits.** Conventional Commits with the work package as scope
(`feat(wp5): …`). Use the repository's git identity; stage explicit paths, never
`git add -A`. End every message with
`Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`.

## Dependency layers

| Unit | May depend on | Must not depend on |
|---|---|---|
| ply-proto | serde, serde_json, ts-rs, thiserror | any ply crate |
| ghostty-sys | nothing at runtime; Zig, curl, shasum and tar at build time | any ply crate |
| ply-term | ply-proto, thiserror, tracing; ghostty-sys only with `engine` | gpui, tokio, any I/O crate |
| ply-agents | ply-proto, serde, serde_json, thiserror | ply-term, tokio, gpui |
| ply-daemon | ply-proto, ply-term (`engine`), ply-agents, anyhow (in `main.rs`), notify, rusqlite, rustix, serde, serde_json, thiserror, tokio, toml, tracing, tracing-appender, tracing-subscriber | gpui |
| ply-hook | serde_json | everything else, tokio included |
| app/src/features | state, ui, keymap types, theme, terminal | another feature, ipc |
| app/src/ui | theme | state, ipc, features |
| app/src/ipc, app/src/terminal | proto.gen, theme, state/actions | features, ui |

## Doc map

- `ARCHITECTURE.md` — the processes, C1/C2/C3, the module map, startup, layering.
- `docs/control-channel.md` — C1 in full: framing, handshake, every method, event and error code.
- `docs/screen-protocol.md` — C2 in full: every frame's byte layout, flow control, history paging.
- `docs/agents.md` — Claude Code and Codex: launch, hooks, notify, OSC 9, rollouts, progress, the status state machine.
- `docs/terminal.md` — libghostty-vt, the engine options, input encoding, the React terminal view, measured numbers.
- `docs/keybindings.md` — every binding, the reserved chords, how to add one.
- `docs/configuration.md` — paths, `config.toml`, settings, environment variables, what a run writes to the machine.
- `docs/development.md` — setup, the gates, running plyd and the app, adding a dependency.
- `docs/perf.md` — the commissioning evidence: journeys J1–J7, P1–P5 and F1–F4 against their targets, the real-CLI
  runs and the soak, with the commands that measured them.

## What lives elsewhere

- GPUIX (React on Zed's GPUI) — npm `@gpuix/react` / `@gpuix/native`, source at remorses/gpuix.
- libghostty-vt — ghostty-org/ghostty at 44f2a44, downloaded pristine by `crates/ghostty-sys/build.rs` into
  `~/Library/Caches/ply/ghostty/`.
- `claude` and `codex` — the user's own installs, found on the login-shell `PATH`; ply never installs,
  updates or signs them in.
- The design canvas (visual reference) and the published specification are claude.ai artifacts, not files here.
