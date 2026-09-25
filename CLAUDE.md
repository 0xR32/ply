# ply

ply is a local macOS app that runs the real `claude` (Claude Code) and `codex` (Codex CLI) TUIs in terminal panes. It
adds what a terminal multiplexer lacks: status per pane, progress from the agent's own plan, stored session history,
the CLIs' own worktrees, and a keyboard-first layout of tabs with several panes each.

ply is **not** an LLM wrapper. It never calls a model API, holds keys, rewrites prompts, hands transcripts between
CLIs, or picks a model or effort level. Model and effort are changed inside the CLI session (`/model`); ply only shows
what the session reports. All ply does is manage terminal panes, and track and store sessions.

## Architecture

### Process model

```
┌──────────────────────── Ply.app (Bun process) ─────────────────────────┐
│  React on GPUIX (@gpuix/react + prebuilt @gpuix/native from npm)       │
│  chrome: tabs · panes · overlays · keymap · store (app/src)            │
│  TerminalView: <div> rows of <text> runs ◄ TS replica ◄ C2 data client │
│  C1 control client (JSON lines)                                        │
└─────────────┬──────────────────────────────────┬───────────────────────┘
      run/data.sock (C2, binary)          run/plyd.sock (C1, JSON lines)
┌─────────────▼──────────────────────────────────▼───────────────────────┐
│  plyd (crates/daemon) — per-user LaunchAgent, not a child of the app   │
│  control server · pane registry · status state machine · SQLite        │
│  per pane: pty (rustix) → libghostty-vt terminal → dirty rows          │
│            → Publisher: Snapshot/Delta, ack window, history pages      │
│  hook server (C3, run/hook.sock) · Codex rollout tailer (C4)           │
└──────┬───────────────┬───────────────┬─────────────────────────────────┘
       ▼               ▼               ▼
  claude (pty)    codex (pty)      $SHELL -l (pty)      ply-hook ──C3──► plyd
```

- **plyd owns every process.** Quitting, closing or crashing the app stops nothing; agents keep running and reopening
  the app shows each pane's current screen (a Snapshot on attach). plyd has no idle exit; only "Quit ply and stop
  sessions" (`daemon.shutdown {kill_panes:true}`) or closing a pane stops a process.
- **Emulation happens only in plyd.** Each pane has one libghostty-vt terminal. plyd also encodes keys, mouse, focus and
  paste against that pane's live modes; the app never mirrors terminal modes.
- **Raw pty bytes never enter JavaScript.** The app receives decoded screen rows over C2 and draws them.

### Components and what each must never do

| Unit | Owns | Never |
|---|---|---|
| `app/` (TypeScript, React on GPUIX) | tabs, layout, focus, overlays, keymap, store, C1 requests, TerminalView (C2 client, replica, rows, input events, selection, clipboard) | opens a pty, spawns processes, reads transcripts, emulates a terminal |
| `crates/proto` (`ply-proto`) | wire types for C1, C2, C3; protocol versions; ts-rs export to `app/src/ipc/proto.gen.ts` | logic; depends on any other ply crate |
| `crates/ghostty-sys` | building libghostty-vt with Zig; its C declarations — the only FFI crate | logic |
| `crates/term` (`ply-term`) | feature `engine` (plyd only): safe libghostty-vt wrapper, DeltaBuilder, input encoding; always: reference `Replica` (tests, CLI test client), `Palette` | I/O; tokio; enables `engine` outside plyd |
| `crates/agents` (`ply-agents`) | per-CLI launch specs, Claude settings generation, hook/notify/OSC 9/rollout parsing, progress, session metadata | I/O beyond reading files it is given |
| `crates/daemon` (`ply-daemon`, bin `plyd`) | ptys, engines, publisher, C1/C2/C3 servers, registry, state machine, SQLite, LaunchAgent, power assertion | render; depend on gpui |
| `crates/hook` (`ply-hook`) | forwarding one hook or notify payload to plyd | block or fail a CLI; print to stdout; depend on anything but std + serde_json |

### Protocols

- **C1 control** — `run/plyd.sock`, UTF-8 JSON, one object per line, ≤ 1 MiB, `#[serde(deny_unknown_fields)]` on every
  struct, `PROTOCOL_VERSION = 1`. Handshake `hello`/`welcome`; `req {id, m, p}` → `res {id, ok, r | err{code,msg}}`;
  daemon events `evt {e, p}`. Methods: `workspace.list/open`, `pane.list/create/close/answer/resume`, `session.list`,
  `theme.set`, `layout.get/save`, `settings.get/set`, `daemon.shutdown`. Events: `pane.added`, `pane.removed`,
  `pane.status`, `pane.progress` (≤ 4/s per pane), `pane.meta` (model, worktree, cwd, branch as the CLI reports them),
  `pane.exit`, `daemon.stopping`. `ErrorCode` is a closed snake_case enum. `pane.close {kill:false}` on a live pane is
  `pane_alive`; `{kill:true}` sends SIGHUP to the process group, SIGKILL after 2 s.
- **C2 screen data** — `run/data.sock`, one connection per visible pane, `len:u32 LE · kind:u8 · payload`, ≤ 1 MiB,
  hand-encoded in `crates/proto/src/data.rs` (no serialisation crate) and in `app/src/terminal/frames.ts`; the two must
  match byte for byte (golden frames). Client → plyd: `0x10 ATTACH {v, pane_id, cols, rows, px_w, px_h}` (px = cell size
  in pixels), `0x11 INPUT_RAW`, `0x12 RESIZE`, `0x13 FETCH_HISTORY {start, count ≤ 1000}`, `0x14 ACK {seq}`, `0x15 KEY
  {key, mods u16, consumed_mods u16, unshifted codepoint, composing, action, text}`, `0x16 MOUSE {action, button, mods
  u16, col, row, x f32, y f32}`, `0x17 PASTE {allow_unsafe, text}`, `0x18 FOCUS {in}`. plyd → client: `0x20 SNAPSHOT`,
  `0x21 DELTA` (dirty rows + new styles), `0x22 HISTORY`, `0x23 TITLE` (coalesced to the Delta cadence), `0x24 BELL`,
  `0x25 EXIT {code}`, `0x26 PASTE_REJECTED`. Cells: `codepoint u32 · style u16 · flags u8 (wide, spacer, spacer_head,
  grapheme) [· extra grapheme codepoints]`; styles: fg/bg/underline colour each `default | indexed u8 | rgb`, attrs u16.
  Colours stay symbolic; the app resolves them against the theme. Flow control: ≤ 4 unacked Deltas, ≤ 120 Hz, send
  immediately on the first change, forced Snapshot after 3 s blocked, disconnect after 30 s without Ack. Attach always
  answers with a Snapshot. No Delta while DEC 2026 synchronized output is open (150 ms cap). No Delta without dirty rows.
- **C3 hook ingress** — `run/hook.sock`, one line `{"v":1,"pane_id","cli","event","payload":<raw CLI JSON>}`.
  `ply-hook <cli> [event]` reads stdin ≤ 1 MiB (Codex notify: the JSON is the last argv argument), reads `PLY_PANE_ID`
  and `PLY_HOOK_SOCK`, writes one line and exits 0 on every path within 200 ms. It never prints to stdout.
- **C4 Codex rollouts** — `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<thread_uuid>.jsonl`, one `{timestamp, ordinal,
  type, payload}` per line, tailed by offset. Unknown record types are skipped and counted. Claude Code transcripts are
  never read (their format is internal).
- **C8 OSC 9** — Codex notifications arrive in its pty stream; plyd receives them (and OSC 7 cwd) through libghostty-vt's
  `OPT_DESKTOP_NOTIFICATION` / `OPT_PWD_CHANGED` callbacks.

### Terminal rendering (React on plain GPUIX)

- The pane body is `TerminalView` (`app/src/features/panes/terminal-view.tsx`). It mounts only for visible panes (the
  active tab; only the focused pane when zoomed); switching tabs detaches old panes and attaches new ones. Hidden panes
  keep running in plyd, and their status keeps arriving over C1.
- `app/src/terminal/`: `frames.ts` (C2 codec), `data-client.ts` (Bun unix socket, ATTACH/ACK/RESIZE, reconnect with
  backoff 100 ms → 2 s), `replica.ts` (applies frames; a version per row), `runs.ts` (row → the fewest style runs),
  `input.ts` (GPUIX key/mouse events → C2 frames).
- A row is one `<div>` of adjacent `<text>` runs; GPUIX merges adjacent `<text>` siblings into one line. Merge cells
  with the same resolved style, keep wide characters and grapheme clusters whole, skip spacer cells, drop trailing
  default blanks. Memoize rows by version so only changed rows re-render.
- Every GPUIX host node costs about 0.01 ms per frame on rebuild. Keep the whole tree under 2 000 host nodes; a test
  asserts a 4-pane tab stays under it.
- Cell width = advance of `m` in Geist Mono at the configured size; cell height = `max(size × lineHeight, ascent +
  descent)`, rounded to device pixels. Box drawing, blocks and Braille come from the font; the line height must make
  them join.
- GPUIX pumps frames from an 8 ms JS `setTimeout`: no synchronous JS task on the main thread may exceed 2 ms; move
  heavy work (e.g. large frame decodes) into a Bun Worker. Frame target: p99 ≤ 16.6 ms with 6 streaming panes.

### Agent integration (facts verified against claude 2.1.282 and codex 0.156.1)

**Claude Code**

- argv: `claude --settings <run/panes/<id>/claude-settings.json> [--worktree <name>] [--resume <session_id>] [prompt]`.
  env: `PLY_PANE_ID`, `PLY_HOOK_SOCK`, `TERM=xterm-256color`, `COLORTERM=truecolor`, `CLAUDE_CODE_FORCE_SYNC_OUTPUT=1`.
- The settings file wins over the user's settings, so it contains **only** `hooks` (plus `"theme":"dark-ansi"` when
  "Use ply colours in Claude Code" is on). Never `--model`.
- Registered hooks, all as `ply-hook claude <Event>`: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse,
  PostToolUseFailure, PermissionRequest, PermissionDenied, Notification, Stop, StopFailure, SessionEnd, CwdChanged.
  **Never WorktreeCreate or WorktreeRemove**: registering WorktreeCreate makes Claude Code hand `git worktree add` to the
  hook. The worktree label comes from the `cwd` of CwdChanged/SessionStart under `<repo>/.claude/worktrees/<name>`.
- `session_id` comes from the first hook payload and is stored for `--resume`. SessionStart carries `model`.
  `Notification.notification_type` is `permission_prompt` or `idle_prompt`.
- No PermissionRequest hook, silent or deciding, changes the dialog in 2.1.282; a manual deny fires no hook (sometimes
  not even Stop). The permission dialog answers to the digits 1/2/3, which `pane.answer` writes to the pty.
- 2.1.282 has neither TodoWrite nor the Task tools. Keep both progress parsers; hide progress when neither appears.

**Codex**

- argv: `codex -c 'notify=["<bundle>/ply-hook","codex"]' -c 'tui.notification_method="osc9"'
  -c 'tui.notification_condition="always"' -c 'tools.update_plan.enabled=true' [resume <thread_uuid>] [prompt]`
  (`tools.update_plan.enabled` is off by default; the `codex_plan_tool` setting turns ply's override off).
  Any `-c` makes the TUI run embedded instead of attaching to Codex's shared daemon; that is fine.
- `notify` payloads are kebab-case (`type: "agent-turn-complete"`, `thread-id`, `turn-id`, `cwd`,
  `last-assistant-message`) and fire after each completed turn. The first notify can come from a title-generation
  micro-turn with a different thread id: bind a pane to a notify's `thread-id` only once a rollout with that id exists;
  until then find the rollout by cwd (newest rollout created after spawn whose `session_meta.cwd` equals the pane cwd).
- OSC 9 classification, checking these fixed prefixes in order: `Approval requested: <cmd>`, `Codex wants to edit
  <path>` / `Codex wants to edit <n> files`, `Approval requested by <mcp server>` → `waiting_permission`;
  `Plan mode prompt: <title>`, `Question: <title>` → `waiting_input`. Any other body is the assistant's own final text
  (or `Agent turn complete`) and means the turn completed (→ `idle`). libghostty-vt swallows bodies that start with
  `5` or `12` (and `1;`…`12;`) as ConEmu sub-commands, so those never reach the classifier.
- Progress: the latest `update_plan` call in the rollout — a `function_call` with JSON arguments `{explanation?, plan:
  [{step, status}]}`, or a code-mode `custom_tool_call` named `exec` whose JS input calls `tools.update_plan({...})`.
- The model comes from the rollout's `turn_context.model`. Resume with `codex resume <thread_uuid>`.
- ply's spawn-time version check must never trigger Codex's self-updater (running `codex --version`/`--help`/`doctor`
  updated an install once). Codex rewrites its own `last_updated`/`last_revision` in `~/.codex/config.toml` through
  its plugin-marketplace auto-upgrade; that is Codex, not ply.
- Codex's TUI does not use the alternate screen or mouse reporting by default; it enables bracketed paste and focus
  events, and probes `CSI 6n`, OSC 10/11, `CSI ?u` and DA1 within 250 ms. plyd's terminal answers them from the palette,
  so the palette must be set before any child starts.

### Status state machine (owned by plyd)

`starting` → (Claude SessionStart · Codex first output byte) → `idle` ("your turn") → (Claude UserPromptSubmit · Codex
Enter typed) → `running` → (Claude PermissionRequest · Codex OSC 9 approval) → `waiting_permission`; (Codex OSC 9
question/plan prompt · Claude Notification that is not a permission prompt) → `waiting_input`; (Claude Stop/StopFailure
· Codex notify or OSC 9 turn complete) → `idle`. Claude Pre/PostToolUse → `running` from any live state except
`waiting_permission`; PostToolUse/PostToolUseFailure/PermissionDenied for the same call leaves `waiting_permission`.
Fallbacks: any key typed in a `waiting_permission`/`waiting_input` pane → `running`; a Claude pane `running` with a
silent pty and no hook for 5 s → `idle`. SessionEnd or pty EOF → `exited(code)`. After a plyd restart a live pane
whose process is gone → `lost` (resumable). Transitions not listed are ignored and counted. Presentation: "done" =
`idle` with every plan item complete; "needs you" = `waiting_permission` or `waiting_input`.

### Keybindings

- The app owns only ⌘ chords. Every other key goes to the focused pane's pty — never bind Esc, Tab, ⇧Tab, Enter, Ctrl
  or ⌥ chords, or bare keys while a terminal has focus.
- GPUIX's app menu owns ⌘Q, ⌘H, ⌥⌘H, ⌘M and ⌘W (⌘W = Close Window; it never reaches the app, and sessions keep
  running). Close pane is **⌘⇧W**. Do not shadow ⌘` ⌘, ⌘C ⌘V ⌘X ⌘A ⌘Z ⌘F with other meanings.
- GPUIX binds neither Tab nor ⇧Tab; both reach `onKeyDown` and go to the pty. JS key handlers cannot stop propagation,
  so TerminalView never sends ⌘ chords and the keymap dispatcher ignores keys the terminal already sent.
- ⌥ is not Meta by default (`option_as_meta: off | left | right | both`). ⇧⏎ becomes LF unless the pane enabled the
  kitty keyboard protocol (plyd applies this).
- Global keys: ⌘K palette · ⌘N new pane · ⌘T new tab · ⌘J next pane that needs you (any tab) · ⌘1–9 tab n ·
  ⌘[ ⌘] previous/next pane · ⌘⇧[ ⌘⇧] previous/next tab · ⌘⏎ zoom · ⌘D shell in the focused pane's cwd · ⌘⇧W close
  pane · ⌘= ⌘- ⌘0 font size · ⌘, settings. In a pane: ⌘C copies the selection (never sends ^C), ⌘V pastes (bracketed
  when enabled), ⌘A selects all scrollback, ⌘F searches scrollback.
- Every binding is declared once in `app/src/keymap/keymap.ts`; `keymap.test.ts` fails on duplicates, reserved hits
  and chords without ⌘. `onKeyDown` appears only in `keymap/dispatcher.ts`, the overlay forms and
  `terminal-view.tsx`.

### Data and filesystem (macOS)

- `~/Library/Application Support/ply/`: `ply.db` (SQLite), `config.toml`, `run/` (mode 0700: sockets, and per pane
  `run/panes/<id>/claude-settings.json` and `launch.json`, the launch spec used for resume). Socket paths < 104 bytes.
- Logs: `~/Library/Logs/ply/{app,plyd}.YYYY-MM-DD.log`, 14 days. Every path is overridable with `PLY_HOME` for tests.
- LaunchAgent `~/Library/LaunchAgents/dev.ply.app.plyd.plist`, started with `launchctl kickstart`, `KeepAlive` only after
  a crash, `ProcessType` Standard. While any pane is `running`, plyd holds a prevent-idle-sleep assertion
  (`keep_awake_while_running`, default on).
- Schema v1: `schema_version`, `workspaces`, `tabs`, `panes` (`cli`, `cwd`, `model_seen`, `worktree_seen`, `status`,
  `session_ref`, `exit_code`, times). Migrations are forward-only in `crates/daemon/src/db/migrations/`; a newer
  `schema_version` makes plyd refuse to start. Finished sessions stay in the database and are returned by
  `session.list {include_closed:true}`.
- Bundle id `dev.ply.app`. One default workspace (the home directory); tabs are named after their first pane's
  directory.

## Repository layout

```
ply/
├─ Cargo.toml · Cargo.lock · rust-toolchain.toml (1.97.1) · clippy.toml · deny.toml
├─ package.json · bun.lock · biome.json            bun workspace ["app"]
├─ justfile                                        check · test · gen · dev
├─ deps.allow.toml                                 admitted dependencies with their INV-16 numbers
├─ vendor/libghostty-vt/                           ghostty 44f2a44, pristine · vendor.json · patches.md
├─ crates/{proto,ghostty-sys,term,agents,daemon,hook}/
├─ app/
│  ├─ assets/fonts/                                Geist, Geist Mono (SIL OFL)
│  └─ src/
│     ├─ main.tsx · app/App.tsx                    render() entry, composition root
│     ├─ theme/tokens.ts                           the only palette and type scale
│     ├─ state/                                    store · actions · reducer · selectors · effects
│     ├─ ipc/                                      control-client.ts · proto.gen.ts (generated)
│     ├─ terminal/                                 frames · data-client · replica · runs · input
│     ├─ keymap/                                   keymap · dispatcher · reserved
│     ├─ features/                                 panes/ · tabs/ · statusbar/ · palette/ · new-pane/
│     └─ ui/                                       presentational primitives
├─ packaging/ · scripts/ (check-rules.ts, check-deps.ts, check-pr.ts, gen.ts)
├─ docs/                                           spec/ply-spec.html · development.md · perf/
└─ .github/workflows/                              ci.yml · release.yml
```

Folder names carry no prefix; Cargo package names are `ply-<folder>` (except `ghostty-sys`).

### Dependency layers

| Unit | May depend on | Must not depend on |
|---|---|---|
| ply-proto | serde, serde_json, ts-rs, thiserror | any ply crate |
| ghostty-sys | nothing at runtime; Zig at build time | any ply crate |
| ply-term | ply-proto; ghostty-sys only with feature `engine` | gpui, tokio, any I/O crate |
| ply-agents | ply-proto, serde_json, toml | ply-term, tokio, gpui |
| ply-daemon | ply-proto, ply-term (`engine`), ply-agents, rustix, tokio, rusqlite, notify, tracing, libc | gpui |
| ply-hook | serde_json | everything else, tokio included |
| app/src/features/* | state, ui, keymap types, theme, terminal | another feature, ipc directly |
| app/src/ui/* | theme | state, ipc, features |
| app/src/ipc/*, app/src/terminal/* | proto.gen, theme, state/actions | features, ui |

## Build, run, test

| Command | Does |
|---|---|
| `just check` | fmt, clippy `-D warnings`, `cargo doc`, check-rules, check-deps, biome, tsc |
| `just test` | cargo tests + `bun test` |
| `bun run gen` | regenerates `app/src/ipc/proto.gen.ts` from ply-proto (never edit that file by hand) |
| `bun run dev` | the app, hot-reloaded (`bun --hot app/src/main.tsx`) |
| `cargo run -p ply-daemon -- --foreground` | plyd in the foreground; `PLY_HOME=<dir>` isolates all state |

Toolchain: Rust 1.97.1, Zig 0.16.0 (libghostty-vt builds through `crates/ghostty-sys/build.rs`; set `ZIG` or put `zig`
on PATH), Bun 1.3.10, just. There is no network access at runtime and no remote for this repository.

## Invariants (every change keeps them; `scripts/check-rules.ts` and tests enforce most)

1. **INV-1** No network requests: no model API, keys, telemetry or updater. No HTTP client crates in ply crates; no
   `fetch`, `WebSocket`, `XMLHttpRequest` or `<img src="http` in `app/`.
2. **INV-2** Raw pty output never enters JavaScript; only decoded screen rows do. C1 types carry no pty bytes.
3. **INV-3** No ply crate depends on `gpui`. The UI is plain GPUIX + React.
4. **INV-4** One keymap; no key handling outside it (see Keybindings).
5. **INV-5** One palette: no hex colour literal outside `app/src/theme/tokens.ts` and test fixtures.
6. **INV-6** No macOS standard shortcut is shadowed.
7. **INV-7** ply never creates, removes or records git worktrees; the CLI owns them. ply passes the option and shows
   what the CLI reports (`worktree_seen` is the only worktree column).
8. **INV-8** ply never writes the user's `~/.claude/settings.json`, `~/.claude.json` or `~/.codex/config.toml`; all
   CLI configuration is per invocation (`--settings`, `-c`).
9. **INV-9** Every error-handling site logs.
10. **INV-10** Protocols are versioned and strict (`deny_unknown_fields`, version checked at hello/ATTACH).
11. **INV-11** No personal data in the repo: no real user names, `/Users/<name>` paths, hostnames or machine values in
    code, docs, tests or fixtures. Use neutral `example` placeholders.
12. **INV-12** `ply-hook` never blocks or fails a CLI (exit 0 in < 250 ms with plyd down).
13. **INV-13** GPUIX is used as released: `@gpuix/react` and `@gpuix/native` pinned exactly at 0.10.0 from npm, with no
    patches, overrides, forks or local builds.
14. **INV-14** ply's hooks and notify handler never decide anything for the CLI: no stdout, exit 0.
15. **INV-15** No GPL-licensed source is copied into ply (Zed's `terminal`, `terminal_view`, `ui`; kitty).
16. **INV-16** Every direct third-party dependency has ≥ 1 000 stars, a commit in the last 6 months, ≥ 3 contributors
    with ≥ 5 commits, and is not declared experimental — recorded in `deps.allow.toml` (repo, stars, last_commit,
    contributors_5plus, checked). Exceptions by decision: GPUIX (D4) and libghostty-vt (D5).
17. **INV-17** libghostty-vt is linked only by plyd (through ply-term's `engine` feature), vendored pristine at ghostty
    44f2a44; any local patch is listed in `vendor/libghostty-vt/patches.md`.

## Coding rules

### General

- Prefer the plain API of a dependency. Never patch, fork or vendor a dependency, or drop to a lower layer (GPUI Rust,
  native addons), when the library's public API can do the job. If a limit seems to force it, stop and ask first.
- Implement what is asked, in the files that own it. No speculative features, no compatibility shims.
- Match the surrounding code: its naming, structure, idioms and comment density.

### Rust

- Edition 2024 per the workspace; `lib.rs` holds only `mod` declarations and re-exports.
- One error enum per crate with `thiserror`; public functions return `Result<T, crate::Error>`. `anyhow` only in a
  binary's `main.rs`.
- No `unwrap`/`expect` outside tests. `#![forbid(unsafe_code)]` in every crate except `ghostty-sys` (FFI), ply-term's
  `engine` module (behind a safe API) and the one audited `pre_exec` block in `crates/daemon/src/pty.rs`; every
  `unsafe` block carries a `// SAFETY:` line.
- Every error-handling site (`match Err`, `if let Err`, block-bodied `map_err`, `unwrap_or_else`) logs through `tracing`
  with `pane_id` when one exists. `?` propagation needs no log.
- Async only in `ply-daemon` (tokio). Pty reads run on a dedicated std thread per pane feeding a bounded channel (64).
  Every channel is bounded; every socket read has a size cap (1 MiB per C1 line, C2 frame and C3 line).
- Workspace dependencies are pinned exactly (`=x.y.z`) in `[workspace.dependencies]`.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo doc --no-deps` (no warnings) must pass.

### TypeScript and React

- Strict mode, no `any`, named exports only, kebab-case file names, PascalCase components.
- One store (`state/store.ts`) read through `useSyncExternalStore` selectors. State changes only through typed actions,
  a pure reducer and `effects.ts`, the only place that calls ipc.
- Features are containers (select state, dispatch actions); `ui/` components are presentational (props in, markup
  out).
- GPUIX specifics: every `<text>` needs an explicit `color` (text does not inherit colour); `div` is block until
  `display: "flex"`; a shrinking flex child needs `minWidth: 0`; no CSS shorthand strings (`padding` etc. take numbers;
  `boxShadow` is an object); there is no `<button>` (use `<div onClick>` with `cursor: "pointer"`); never nest `<text>`
  inside `<text>`; long lists use `<virtual-list>`.
- Colours, fonts and sizes come from `tokens.ts` only.
- `bunx biome check` and `bunx tsc -p app` must pass.

### Documentation and comments

The owner asked for proper code documentation. Documentation is required; inline commentary is not.

- **Crate docs.** Every `lib.rs`/`main.rs` opens with a `//!` doc: what the crate is for, which contract it implements
  (e.g. "C2 per spec 4.2"), and what it must never do.
- **Public Rust items.** Every public type, trait, function, method, field, variant, const and module has a `///` doc
  stating its **contract**: guarantees, units, ranges, `# Errors`, threading and blocking behaviour, invariants the
  caller must keep. Never restate the name or the signature. `missing_docs` is on (warnings are errors), and
  `broken_intra_doc_links` is denied.
- **TypeScript.** Exported symbols in `app/src/{ipc,state,keymap,terminal,theme}` carry TSDoc stating the contract
  beyond what the types say. Each component gets a one-line TSDoc of its role.
- **Inline comments** (`//`, `/* */`, `#`) only for a non-obvious **why**: a hidden invariant, a workaround with its
  upstream link, a surprising performance choice, a rule with no source in the code. One line (a second only for a
  tracking link). Never restate the code, never describe the current task or caller, no section dividers, no
  commented-out code, no TODO without an owner or trigger.
- **Human docs.** `README.md` (what ply is, layout, prerequisites, commands) and `docs/development.md` (toolchain, run
  dir, logs, `PLY_HOME`, gates, adding a dependency) stay current with every change that affects them.

### Tests

- Rust unit tests next to the code; integration tests in `crates/*/tests/`; fixtures in `tests/fixtures/`, scrubbed
  (INV-11).
- Protocol: a golden file per C1 message and C3 envelope, round-tripped both ways; golden C2 frames written by the Rust
  side and decoded and re-encoded byte-identically by `app/src/terminal/frames.ts`.
- Terminal: the reference Replica fed random Snapshot/Delta sequences equals the Engine's grid; recorded Claude Code
  and Codex byte streams replay through the Engine against stored row snapshots.
- Daemon: fake CLIs (`crates/daemon/tests/fake/*.sh`) print scripted output and call `ply-hook`, so the state machine
  is tested without real agents. Tests that run CLIs use a sandboxed `HOME`/`CODEX_HOME`.
- UI: GPUIX's test renderer (`@gpuix/react/testing`: `createTestRoot`, `flush`, `nativeSimulateKeystrokes`,
  `getPaintedText`, `captureScreenshot`) and `connectTest`/`getByTestId` for end-to-end journeys.
- Tests assert behaviour, not mocks; output stays free of warnings.

## Workflow

- Commits follow Conventional Commits with the work package in the scope: `feat(wp5): memoize terminal rows`.
- Commit with the repository's configured git identity; never change `user.email`/`user.name`. End every commit
  message with `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`.
- Stage explicit paths (`git add <paths>`), never `git add -A`. There is no remote; never push.
- Never run a CLI in a way that changes the user's real configuration. Never pass permission-skipping flags to
  `claude` or `codex`.
- Reference docs: `docs/spec/ply-spec.html` (spec 6.0.0) and `docs/development.md`. The WP0 spike records (ADRs
  0001–0008) are in git history at commit 2944769 (`git show 2944769:docs/adr/<file>`); the facts that matter are
  summarised in this file, which wins where they describe the removed native GPUI terminal element.
