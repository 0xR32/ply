# Architecture

ply is three processes and three pieces of wire:

| Piece | Where | Talks to |
|---|---|---|
| the app | `app/` (TypeScript, React on GPUIX from npm) | plyd over C1 (`run/plyd.sock`) and C2 (`run/data.sock`); nothing else |
| `plyd`, the daemon | `crates/daemon` (Rust, tokio) over `crates/term`, `crates/agents`, `crates/proto` | the app over C1 and C2; every pane's child process over its pty; `ply-hook` over C3 (`run/hook.sock`); Codex rollout files and Claude Code's usage cache on disk, read-only |
| `ply-hook` | `crates/hook` (std + serde_json) | plyd over C3, once per hook or notify invocation |
| the agents | the user's own `claude` and `codex`, found on the login-shell `PATH` | their pty; `ply-hook` through the hooks and notify program ply passes per invocation |

The app is a **view**. It opens no pty, spawns no agent, reads no transcript and
emulates no terminal. plyd is a per-user LaunchAgent, not a child of the app, so
quitting, closing or crashing the app stops nothing: every pty and every agent
keeps running, and reopening the app shows each pane's current screen because
every attach is answered with a Snapshot. plyd has no idle exit; only closing a
pane or `daemon.shutdown {kill_panes:true}` (the palette's "Quit ply and stop
sessions") stops a process, and a rebuilt plyd replaces the running one after
"Restart plyd" (`docs/development.md`).

ply makes no network request (INV-1). It never calls a model, holds a key,
rewrites a prompt or picks a model; everything it knows about a session comes
from the CLI's own hooks, notify payloads, OSC 9 notifications and Codex's
rollout files, and the plan usage ⌘U shows is what the CLIs last recorded on
disk (`usage.get`).

## Three boundaries

**C1, control: app ⇄ plyd.** JSON lines over `run/plyd.sock`, at most 1 MiB a
line, `#[serde(deny_unknown_fields)]` on every struct, `PROTOCOL_VERSION = 1`
checked at `hello`. Requests carry an id and a method (`workspace.*`, `pane.*`,
`session.list`, `theme.set`, `layout.*`, `settings.*`, `daemon.shutdown`,
`usage.get`);
plyd broadcasts events (`pane.added`, `pane.removed`, `pane.status`,
`pane.progress`, `pane.meta`, `pane.exit`, `daemon.stopping`) to every client.
The Rust types in `crates/proto/src/control.rs` and `pane.rs` are the single
source; `bun run gen` writes `app/src/ipc/proto.gen.ts` from them with ts-rs, and
`check-rules` fails when that file is stale. C1 carries no pty bytes (INV-2).
`docs/control-channel.md` is the whole protocol.

**C2, screen data: plyd ⇄ each visible pane.** Binary frames over
`run/data.sock`, one connection per attached pane, `len:u32 LE · kind:u8 ·
payload`, at most 1 MiB, encoded by hand in `crates/proto/src/data.rs` and again
in `app/src/terminal/frames.ts`; golden frames in `crates/proto/tests/golden/c2/`
keep the two byte-identical, and ATTACH carries `C2_VERSION` (2), which plyd
checks first. The client sends ATTACH, RESIZE, KEY, MOUSE, PASTE,
FOCUS, FETCH_HISTORY and ACK; plyd sends SNAPSHOT, DELTA (changed rows only),
HISTORY, TITLE, BELL, EXIT, PASTE_REJECTED, ATTACH_REFUSED and CLIPBOARD_WRITE
(OSC 52, which the app writes to the pasteboard). Scrollback is addressed by
absolute line from each frame's `scrollback_base`. Colours stay
symbolic and are resolved against the app's theme. Flow control: at most 4
unacked Deltas, at most 120 Hz, a forced Snapshot after 3 s blocked, a
disconnect after 30 s without an Ack. `docs/screen-protocol.md` has every byte.

**C3, hook ingress: `ply-hook` → plyd.** One JSON line over `run/hook.sock`:
`{v, pane_id, cli, event, payload}` with the CLI's raw JSON inside. `ply-hook`
reads `PLY_PANE_ID` and `PLY_HOOK_SOCK` from the environment the pane was
launched with, gives up after 200 ms, never prints to stdout and exits 0 on
every path, so it can neither block nor decide for the CLI (INV-12, INV-14).
Codex also reports through OSC 9 in its pty stream (read through libghostty-vt's
notification callback) and through its rollout JSONL files (tailed by offset).
`docs/agents.md` covers every signal and the status state machine.

## The module map

```
crates/proto/src
├── lib.rs           the three protocols and their versions
├── control.rs       C1: ClientMsg, ServerMsg, every request and event, ErrorCode, MAX_LINE_BYTES
├── pane.rs          Pane, PaneStatus, Progress, Workspace, Tab, Layout, Session, TerminalTheme, Settings,
│                    Usage (the CLIs' plan usage)
├── data.rs          C2: Frame, the hand-written little-endian codec, FrameReader, cells, styles, input payloads
├── hook.rs          C3: HookEnvelope
└── version.rs       PROTOCOL_VERSION, C2_VERSION, HOOK_VERSION and the one comparison

crates/ghostty-sys   build.rs + fetch.rs (verified ghostty 44f2a44 download, zig build into OUT_DIR) · lib.rs (the only FFI)

crates/term/src      (feature `engine` = plyd only)
├── engine.rs        Engine: one pane's libghostty-vt terminal behind a safe API; palette replies, idle compression
├── engine/          cells.rs (cells and styles → C2) · effects.rs (the C callbacks) · encoders.rs · render.rs
├── delta.rs         per-client Snapshot / Delta / History building
├── input.rs         KEY, MOUSE, PASTE, FOCUS encoded against the pane's live modes
├── replica.rs       the reference C2 decoder (tests, the command-line client), always compiled
└── palette.rs       16 ANSI colours, fg/bg/cursor/selection, the 256-colour cube

crates/agents/src
├── adapter.rs       Adapter and AgentSession: launch specs and signals, the contract plyd drives
├── claude/          mod.rs (launch, hook payloads → signals) · settings.rs (the hooks-only --settings file) ·
│                    progress.rs (TodoWrite and Task tools) · usage.rs (the usage cache in .claude.json)
├── codex/           mod.rs (launch, -c overrides, thread binding) · notify.rs · osc9.rs (classification) ·
│                    rollout.rs (records, update_plan, discovery) · literal.rs (code-mode JS literals) ·
│                    usage.rs (token_count rate limits)
├── install.rs       reads a CLI's version from its install layout without executing it
├── usage.rs         what both usage parsers share: window labels, RFC 3339 times
├── meta.rs · plan.rs · version.rs

crates/hook/src      main.rs: stdin (or Codex's last argv) → one C3 line, 200 ms, exit 0

crates/daemon/src
├── main.rs          plyd's command line (--foreground, install-agent, …)
├── daemon.rs        the lifetime: open the store, restore panes, serve C1, C2 and C3, stop only when asked
├── server/          control.rs (C1) · data.rs (C2) · hooks.rs (C3) · mod.rs
├── panes/           registry.rs (workspaces, tabs, panes, mirrored to SQLite) · pane.rs (one task per pane:
│                    the only owner of its Engine, pty channels and attached clients) · agent.rs (one agent
│                    process's session, status machine, progress limit and tailer) · state.rs (the spec 6.3
│                    machine) · launch.rs (create, resume, restore) · mod.rs
├── tail.rs          C4: finding and tailing a Codex pane's rollout
├── usage.rs         usage.get: the CLIs' plan usage read from their own files, bounded, cached 5 s
├── osc.rs · branch.rs   OSC 7/9 and typed input; the git branch label
├── publisher.rs     the C2 delivery rules as small clocked state machines
├── pty.rs           rustix pty + Command, setsid/TIOCSCTTY in the one audited pre_exec block
├── login.rs         the login shell and the environment every pane starts with
├── db/              mod.rs · migrations/0001_init.sql (schema v1)
├── config.rs · paths.rs · lock.rs (one plyd per data directory)
├── launchd.rs       the LaunchAgent plyd installs for itself (`plyd install-agent`)
└── power.rs         prevent idle sleep while a pane is running (caffeinate -i -w)

app/src
├── main.tsx · app/  render(), window options, App, top bar, overlays
├── state/           store.ts (useSyncExternalStore) · actions.ts · reducer.ts (pure) · selectors.ts ·
│                    effects.ts (the only ipc caller)
├── ipc/             control-client.ts (C1, reconnect 100 ms → 2 s) · daemon-launcher.ts (dev plyd or the
│                    LaunchAgent) · dirs.ts (the new-pane form's folder sources: recents, the bounded
│                    repository scan, path completion; async fs only) · os.ts · paths.ts ·
│                    proto.gen.ts (generated) · mock-server.ts (tests only)
├── terminal/        frames.ts (C2 codec) · data-client.ts · replica.ts · runs.ts (row → <text> runs) ·
│                    input.ts · selection.ts · links.ts (⌘-click URLs) · session.ts · host.ts · metrics.ts
├── keymap/          keymap.ts (every binding, once) · dispatcher.ts (the window's keys, the ⌘U hold) · reserved.ts
├── features/        panes/ (grid, frame, header, waiting and lost strips, terminal-view, close confirm) · tabs/ ·
│                    statusbar/ · palette/ · new-pane/ · settings/ · usage/ (the hold-⌘U usage card)
├── ui/              presentational primitives: text, kbd, chip, button, segments, switch, overlay card
└── theme/           tokens.ts (the only palette and type scale) · chrome.ts
```

## The terminal, end to end

plyd reads each pty on a dedicated std thread into a bounded channel; the pane's
task writes the bytes into its libghostty-vt `Engine`, which answers colour and
device queries itself (OSC 4/10/11, DA, DSR, `CSI ?u`, XTVERSION `ply <version>`)
from the palette the app sent with `theme.set` — set before any child starts,
because Codex probes within 250 ms. Dirty rows become a per-client Delta; while
DEC 2026 synchronized output is open no Delta is built (150 ms cap); an idle pane
sends nothing and is compressed after it goes quiet.

In the app, `terminal-view.tsx` mounts only for the active tab's panes (only the
focused one when zoomed), which the grid lays out as equal columns up to three
and quadrants at four; plyd caps a tab at four panes (`tab_full`). `data-client.ts` attaches, `replica.ts` applies frames,
versions and hashes each row, and each visible row renders as one memoized
`TerminalRow` (`terminal-row.tsx`): a `<div>` whose `<text>` runs are each placed
absolutely at their own rounded column, `round(col × cell width)`, so neither
layout rounding nor a fallback glyph can drift the columns after them; rows are
keyed by content hash, so a scroll moves the rows it kept. Every GPUIX host node
costs about 0.01 ms per frame, so runs are coalesced and a test keeps a 4-pane
tab under 2 000 host nodes. Keys, mouse, paste and focus go
back as C2 events and plyd encodes them against the pane's live modes; the app
never mirrors terminal modes. `docs/terminal.md` has the details and the
measured numbers.

## Startup

`bun run dev` connects to `run/plyd.sock`. If nothing
answers, `daemon-launcher.ts` starts plyd: with `PLY_HOME` set it runs the
cargo-built `plyd --foreground` against that directory; without it, it calls
`plyd install-agent`, which writes `~/Library/LaunchAgents/dev.ply.app.plyd.plist`
and kickstarts it. plyd takes the single-instance lock, opens `ply.db`, restores
tabs and panes (a pane whose process is gone becomes `lost` and can be resumed
with the CLI's own resume), binds its sockets under `run/` (mode 0700), reopens
the lost panes that have no session to resume as fresh shells, and serves until
`daemon.shutdown`.

## Layering rules

- No ply crate depends on `gpui`; the UI is plain GPUIX + React
  (`checkInv3NoGpui`). GPUIX is used exactly as released at 0.10.0, with no
  patches, overrides or local builds (`checkInv13Gpuix`).
- libghostty-vt is linked only by plyd: `ghostty-sys` is reached through
  ply-term's `engine` feature, and only ply-daemon enables it
  (`checkInv17Ghostty`, which also checks that `ghostty-sys/build.rs` pins
  the ghostty commit, an archive URL naming it and the archive's SHA-256).
- Each crate's dependencies are checked against the table in `CLAUDE.md`
  (`checkCrateLayers`): every crate has an allow-list, so a new dependency
  needs a row change (dev- and build-dependencies may go beyond it), and
  ply-agents does no I/O beyond reading the files it is given.
- In the app, features import state, ui, keymap types, theme and terminal but
  never ipc; ui imports only theme; ipc and terminal never import features or ui
  (`checkAppLayers`). `effects.ts` is the only file that calls ipc.
- `onKeyDown` appears only in `keymap/dispatcher.ts`, the overlay forms and
  `terminal-view.tsx` (`checkInv4KeyHandlers`); no hex colour outside
  `theme/tokens.ts` (`checkInv5Palette`); no `fetch`, `WebSocket`,
  `XMLHttpRequest` or remote `<img>` in `app/` (`checkInv1NoNetwork`).
- ply never creates, removes or records git worktrees; the only worktree column
  is `worktree_seen`, what the CLI reported (`checkInv7Worktrees`).
- Every direct dependency is admitted in `deps.allow.toml` with its INV-16
  numbers (`scripts/check-deps.ts`); `cargo deny` bans HTTP clients and `gpui`
  for every ply crate.
