# ply — Claude Code Instructions

## What this repository is

**ply**: a local macOS app that runs the real `claude` (Claude Code) and `codex`
(Codex CLI) TUIs in terminal panes — tabs across the top, up to four panes per tab —
and adds what a terminal multiplexer lacks: status per pane ("needs you", "your
turn", running, exited), progress from the agent's own plan, the model the
session reports, stored session history and the CLIs' own worktrees.

It is **not** an LLM wrapper. It never calls a model API, holds a key, rewrites a
prompt, hands a transcript between CLIs or picks a model or effort level; the
user changes those inside the session. ply manages terminal panes and tracks and
stores sessions, and nothing else.

`ARCHITECTURE.md` has the processes, the three wire boundaries and the module
map. `docs/` has one document per subject (see the doc map below).

## Where the pieces live

| | |
|---|---|
| the app | `crates/app` (`ply-app`, binary `ply`) — Rust on GPUI 0.2.2; what `just dmg` bundles. `main.rs` (the window, tabs, grid and ⌘ chords), `conn.rs` (C2 per pane, decoded off the UI thread), `paint.rs` (the terminal element), `select.rs` (selection, copy, find, links), `keys.rs` (keystrokes → KEY frames), `chrome.rs` (palette, type scale and chrome), `forms.rs` (⌘N/⌘T, accounts), `drop.rs` (dropped files, pasted images), `control.rs` (the C1 client), `launcher.rs` (starting plyd); Geist, Geist Mono and the icon in `assets/` |
| the daemon | `crates/daemon` (`plyd`): ptys, one libghostty-vt terminal per pane, the C1 and C2 servers, SQLite, the LaunchAgent, `usage.get` (Claude panes' status line reports and the CLIs' own usage records, read-only), the Claude account switch (`accounts.rs`) and Codex's sign-in (`accounts/codex.rs`) |
| the terminal engine | `crates/term` (`ply-term`) over `crates/ghostty-sys`, whose `build.rs` downloads pristine ghostty 0fa3d57 (libghostty-vt 1.0.0, SHA-256-verified) and builds libghostty-vt from it with Zig; its `Replica` is the C2 decoder the app draws from |
| the agent adapters | `crates/agents` (`ply-agents`): launch specs, the Claude hooks file, hook/notify/OSC 9/rollout parsing, progress, the version check, the plan-usage parsers |
| the hook helper | `crates/hook` (`ply-hook`): one hook or notify payload → one C3 line to plyd; as `ply-hook statusline`, Claude Code's status line in a pane |
| the wire types | `crates/proto` (`ply-proto`): C1, C2, C3 |
| the bundle | `scripts/dmg.ts` (`just dmg`) → `dist/ply.app` and `dist/ply-<version>.dmg` with the GPUI app as `MacOS/ply` |
| the gates | `scripts/check-rules.ts` (the invariants and layers), `scripts/check-deps.ts` + `deps.allow.toml` (dependency admission), `deny.toml` |
| tests | `#[cfg(test)]` inline, `crates/*/tests/` (golden frames, fake CLIs, replays), `scripts/*.test.ts` for the gates |

## Rules the tests enforce

**Raw pty bytes never reach the app.** plyd emulates every pane; the app
receives decoded screen rows over C2 and paints them. C1 types carry no byte
fields (`checkInv2NoPtyBytes`), and ply-app may not depend on ghostty-sys or
any pty crate (the layer table below).

**No network.** No ply crate depends on an HTTP client (`cargo deny`), and each
crate's dependencies are limited to its row of the layer table.

**The UI is GPUI in Rust.** The owner reversed D4, INV-3 and INV-13 on
2026-11-11: GPUIX is no longer ply's UI framework and a ply crate may depend on
`gpui`. The React app on GPUIX was deleted the same day; ply has no JavaScript
UI. plyd, ply-term and ply-agents never link `gpui` (the layer table below).

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

**ply never manages worktrees.** The CLI creates and removes them; ply passes
`claude --worktree <name>` and shows what the CLI reports. The only worktree
column is `worktree_seen` (`checkInv7Worktrees`).

**The protocols are strict and pinned by golden files.** `deny_unknown_fields`
on every C1/C3 struct, 1 MiB caps, version checks at `hello` and ATTACH; one
golden file per message in `crates/proto/tests/golden/`, which the encoder must
write byte for byte and the decoder read back.

**Layers are checked, not agreed.** `checkCrateLayers` enforces the table below:
a crate's normal dependencies must all be in its row (dev- and
build-dependencies may go beyond it, never to a forbidden crate), and
`check-rules.test.ts` fails when the table and the check disagree;
`checkRustStandards` rejects `unsafe` and `anyhow` where they are not allowed;
`checkExactPins` wants `=x.y.z`.

**Every dependency is admitted.** A new direct dependency needs a row in
`deps.allow.toml` with its repository, stars, last commit and number of
contributors with ≥ 5 commits (≥ 1 000 stars, a commit in the last 6 months,
≥ 3 such contributors, not experimental). `bun scripts/check-deps.ts --add` fills
it in. The exceptions by decision are libghostty-vt, `thiserror` and `anyhow`.

**No personal data in the repository.** No real user names, `/Users/<name>`
paths, hostnames or machine values in code, docs, tests or fixtures; use
`/Users/example` (`checkInv11PersonalData`).

## The things that will bite you

**Always set `PLY_HOME` when testing.** Without it the app (`cargo run -p
ply-app`, or the bundle) starts plyd through `plyd install-agent`, which writes
a real `~/Library/LaunchAgents/dev.ply.app.plyd.plist` and kickstarts it, and
plyd uses the real `~/Library/Application Support/ply/`. With `PLY_HOME=<dir>`
everything lives under that directory and the app starts `plyd --foreground`.

**libghostty-vt needs Zig 0.17.0 and, once, the network.** Set `ZIG` or put
`zig` on `PATH`. The first build of `ghostty-sys` downloads the ghostty source
(about 40 MB) into `~/Library/Caches/ply/ghostty/<commit>/` and the release's
Zig packages (about 6 MB) beside it, and takes about a minute; later builds
reuse the cache and `zig build` itself runs offline under `OUT_DIR`. To build
offline set `PLY_GHOSTTY_SRC` to an extracted ghostty 0fa3d57 tree (and
`PLY_ZIG_PKG_DIR` to the packages). Never build inside the cached source by
hand: `zig build` there writes `zig-out/`, `.zig-cache/` and `zig-pkg/` into
the tree every later build uses.

**Never run `codex --version`, `codex --help` or `codex doctor` on this
machine.** They start Codex's self-updater (it updated an install once). The
adapters read versions from the install layout instead. Never pass
permission-skipping flags to `claude` or `codex`.

**The app starts plyd only when it starts.** `launcher::ensure_plyd` runs once,
before the window opens; when plyd stops later the app reconnects to whatever
answers but starts nothing, so after stopping plyd reopen the app or start plyd
yourself.

**The comment hook.** A machine-level hook rejects comment blocks longer than one
line (two with a tracking link). Doc comments state errors and safety inline
(`errors: …`, `safety: …`) instead of multi-line sections.

## Run it

```
cargo run -p ply-daemon -- --foreground        # plyd, with PLY_HOME=<dir> set
cargo run -p ply-app                           # the app, with the same PLY_HOME; starts plyd itself when nothing answers
cargo run -p ply-daemon --example ply-cli      # the command-line C1/C2 test client
```

## Build & test

```
just check      # fmt, clippy -D warnings, cargo doc -D warnings, check-rules, check-deps, biome, tsc on scripts/
just test       # cargo nextest, doctests, bun test on scripts/
just deny       # licences, advisories, the HTTP-client ban
just dmg        # dist/ply.app and dist/ply-<version>.dmg: the GPUI app, plyd, ply-hook at profile dist, ad-hoc signed
```

Toolchain: Rust 1.97.1 (`rust-toolchain.toml`), Zig 0.17.0, Bun 1.3.10 (for `scripts/`), just.
The GitHub repository is public; push only when asked. `docs/development.md` walks through setup, the
run directory and the logs.

## Personal use

ply is built for one person's machine. It runs from this checkout —
`cargo run --release -p ply-app` against a release `plyd` (`cargo build
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

**Documentation.** Every crate opens with a `//!` doc: what it is for, the
contract it implements and what it must never do. Every public Rust item has a
`///` doc stating its contract — guarantees, units, error cases, threading and
blocking — never a restatement of its name (`missing_docs` is on and warnings are
errors). Exported TypeScript symbols in `scripts/` carry TSDoc for what the types
do not say. A change that alters behaviour described in `docs/` or
`ARCHITECTURE.md` updates that document in the same commit.

**Comments.** Only a non-obvious why: a hidden invariant, a workaround with its
upstream link, a surprising performance choice. One line. No restating the code,
no dividers, no commented-out code, no TODO without an owner or trigger.

**Tests.** Behaviour, not mocks. Daemon behaviour is tested with fake CLIs
(`crates/daemon/tests/fake/`) and a sandboxed `HOME`/`CODEX_HOME`. Output stays
free of warnings.

**Commits.** Conventional Commits with the work package as scope
(`feat(wp5): …`). Use the repository's git identity; stage explicit paths, never
`git add -A`. End every message with
`Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`.

## Dependency layers

| Unit | May depend on | Must not depend on |
|---|---|---|
| ply-proto | serde, serde_json, thiserror | any ply crate |
| ghostty-sys | nothing at runtime; Zig, curl, shasum and tar at build time | any ply crate |
| ply-term | ply-proto, thiserror, tracing; ghostty-sys only with `engine` | gpui, tokio, any I/O crate |
| ply-agents | ply-proto, serde, serde_json, thiserror | ply-term, tokio, gpui |
| ply-daemon | ply-proto, ply-term (`engine`), ply-agents, anyhow (in `main.rs`), notify, rusqlite, rustix, serde, serde_json, thiserror, tokio, toml, tracing, tracing-appender, tracing-subscriber | gpui |
| ply-hook | serde_json | everything else, tokio included |
| ply-app | ply-proto, ply-term (no `engine`), futures, gpui, serde, serde_json, tracing, tracing-appender, tracing-subscriber | tokio, ghostty-sys |

## Doc map

- `ARCHITECTURE.md` — the processes, C1/C2/C3, the module map, startup, layering.
- `docs/control-channel.md` — C1 in full: framing, handshake, every method, event and error code.
- `docs/screen-protocol.md` — C2 in full: every frame's byte layout, flow control, history paging.
- `docs/agents.md` — Claude Code and Codex: launch, hooks, notify, OSC 9, rollouts, progress, the status state machine.
- `docs/terminal.md` — libghostty-vt, the engine options, input encoding, the GPUI terminal view, measured numbers.
- `docs/keybindings.md` — every key the app acts on and every key it leaves to the pane.
- `docs/configuration.md` — paths, `config.toml`, settings, environment variables, what a run writes to the machine.
- `docs/development.md` — setup, the gates, running plyd and the app, adding a dependency.
- `docs/perf.md` — the GPUI app against the React app it replaced, then the React app's commissioning evidence
  (journeys J1–J7, P1–P5 and F1–F4, the real-CLI runs and the soak) as history.

## What lives elsewhere

- GPUI — Zed's UI framework, crate `gpui` 0.2.2 from crates.io, source in zed-industries/zed.
- libghostty-vt — 1.0.0, ghostty-org/ghostty at 0fa3d57 (`vt-1.0.0`), downloaded pristine by `crates/ghostty-sys/build.rs` into
  `~/Library/Caches/ply/ghostty/`.
- `claude` and `codex` — the user's own installs, found on the login-shell `PATH`; ply never installs or
  updates them, and never signs them in itself: an account switch runs Claude Code's own `claude auth login`,
  and a Codex sign-in Codex's own `codex login`, in a pane for the user to complete (`crates/daemon/src/accounts.rs`).
- The design canvas (visual reference) and the published specification are claude.ai artifacts, not files here.
