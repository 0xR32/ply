# Architecture

ply is three processes and three pieces of wire:

| Piece | Where | Talks to |
|---|---|---|
| the app | `crates/app` (Rust, GPUI), binary `ply` | plyd over C1 (`run/plyd.sock`) and C2 (`run/data.sock`); nothing else |
| `plyd`, the daemon | `crates/daemon` (Rust, tokio) over `crates/term`, `crates/agents`, `crates/proto` | the app over C1 and C2; every pane's child process over its pty; `ply-hook` over C3 (`run/hook.sock`); Codex rollout files and Claude Code's usage cache on disk, read-only |
| `ply-hook` | `crates/hook` (std + serde_json) | plyd over C3, once per hook or notify invocation |
| the agents | the user's own `claude` and `codex`, found on the login-shell `PATH` | their pty; `ply-hook` through the hooks and notify program ply passes per invocation |

The app is a **view**. It opens no pty, spawns no agent, reads no transcript and
emulates no terminal. plyd is a per-user LaunchAgent, not a child of the app, so
quitting, closing or crashing the app stops nothing: every pty and every agent
keeps running, and reopening the app shows each pane's current screen because
every attach is answered with a Snapshot. plyd has no idle exit; only closing a
pane or `daemon.shutdown {kill_panes:true}` stops a process, and a rebuilt plyd
replaces the running one only once that one stops (`docs/development.md`,
**Replacing a running plyd**).

ply makes no network request (INV-1). It never calls a model, holds a key,
rewrites a prompt or picks a model; everything it knows about a session comes
from the CLI's own hooks, notify payloads, OSC 9 notifications and Codex's
rollout files, and the plan usage `usage.get` answers is what the CLIs last
reported: Claude Code's status line payload in any Claude pane, else what the
CLIs recorded on disk; no app shows it today. Switching the Claude account runs Claude
Code's own `claude auth login` in a pane and restarts the Claude panes on the
new sign-in; ply keeps only the accounts' emails and organizations. Codex's
sign-in runs its own `codex login` the same way and restarts the Codex panes;
ply reads Codex's `auth.json` only for the email, plan and whether it is an API
key, and keeps no token or key.

## Three boundaries

**C1, control: app ⇄ plyd.** JSON lines over `run/plyd.sock`, at most 1 MiB a
line, `#[serde(deny_unknown_fields)]` on every struct, `PROTOCOL_VERSION = 1`
checked at `hello`. Requests carry an id and a method (`workspace.*`, `pane.*`,
`session.list`, `theme.set`, `layout.*`, `settings.*`, `daemon.shutdown`,
`usage.get`, the task queue's `task.*`, `queue.pause` and `skill.list` (which no
app offers today), and
the Claude account switch's `account.*` and Codex's sign-in, `codex.*`); plyd
broadcasts events (`pane.added`, `pane.removed`, `pane.status`, `pane.progress`,
`pane.meta`, `pane.exit`, `daemon.stopping`, `task.changed`, `queue.changed`,
`account.changed`, `codex.changed`) to every client.
The Rust types in `crates/proto/src/control.rs` and `pane.rs` are the single
source; plyd and the app both use them. C1 carries no pty bytes (INV-2).
`docs/control-channel.md` is the whole protocol.

**C2, screen data: plyd ⇄ each visible pane.** Binary frames over
`run/data.sock`, one connection per attached pane, `len:u32 LE · kind:u8 ·
payload`, at most 1 MiB, encoded by hand in `crates/proto/src/data.rs`; golden
frames in `crates/proto/tests/golden/c2/` pin every byte, and ATTACH carries
`C2_VERSION` (2), which plyd checks first. The client sends ATTACH, RESIZE, KEY, MOUSE, PASTE,
FOCUS, FETCH_HISTORY and ACK; plyd sends SNAPSHOT, DELTA (changed rows only),
HISTORY, TITLE, BELL, EXIT, PASTE_REJECTED, ATTACH_REFUSED and CLIPBOARD_WRITE
(OSC 52; the app does not act on BELL, PASTE_REJECTED or CLIPBOARD_WRITE yet). Scrollback is addressed by
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
│                    Usage (the CLIs' plan usage), Task, QueueState, Skill (the task queue, R60/R61)
├── data.rs          C2: Frame, the hand-written little-endian codec, FrameReader, cells, styles, input payloads
├── hook.rs          C3: HookEnvelope
└── version.rs       PROTOCOL_VERSION, C2_VERSION, HOOK_VERSION and the one comparison

crates/ghostty-sys   build.rs + fetch.rs (verified ghostty 0fa3d57 and package downloads, zig build into OUT_DIR) · lib.rs (the only FFI)

crates/term/src      (feature `engine` = plyd only)
├── engine.rs        Engine: one pane's libghostty-vt terminal behind a safe API; palette replies, idle compression
├── engine/          cells.rs (cells and styles → C2) · effects.rs (the C callbacks) · encoders.rs · render.rs
├── delta.rs         per-client Snapshot / Delta / History building
├── input.rs         KEY, MOUSE, PASTE, FOCUS encoded against the pane's live modes
├── replica.rs       the C2 decoder the app draws from (also the tests' and the command-line client's), always compiled
└── palette.rs       16 ANSI colours, fg/bg/cursor/selection, the 256-colour cube

crates/agents/src
├── adapter.rs       Adapter and AgentSession: launch specs and signals, the contract plyd drives
├── claude/          mod.rs (launch, hook payloads → signals) · settings.rs (the hooks-only --settings file) ·
│                    progress.rs (TodoWrite and Task tools) · usage.rs (status line rate limits, the .claude.json cache) ·
│                    account.rs (`claude auth status` and `claude auth login` argv, the status parser)
├── codex/           mod.rs (launch, -c overrides, thread binding) · notify.rs · osc9.rs (classification) ·
│                    rollout.rs (records, update_plan, discovery) · literal.rs (code-mode JS literals) ·
│                    usage.rs (token_count rate limits) · account.rs (`codex login` argv, the auth.json and ID token parser)
├── install.rs       reads a CLI's version from its install layout without executing it
├── usage.rs         what both usage parsers share: window labels, RFC 3339 times
├── skills.rs        skill, command and prompt files and the plugin registry → Skill records (R61)
├── meta.rs · plan.rs · version.rs

crates/hook/src      main.rs: stdin (or Codex's last argv) → one C3 line, 200 ms, exit 0

crates/daemon/src
├── main.rs          plyd's command line (--foreground, install-agent, …)
├── daemon.rs        the lifetime: open the store, restore panes, serve C1, C2 and C3, stop only when asked
├── server/          control.rs (C1) · data.rs (C2) · hooks.rs (C3) · mod.rs
├── panes/           registry.rs (workspaces, tabs, panes, mirrored to SQLite) · pane.rs (one task per pane:
│                    the only owner of its Engine, pty channels and attached clients) · agent.rs (one agent
│                    process's session, status machine, progress limit, tailer and task dispatch) · state.rs (the
│                    spec 6.3 machine) · launch.rs (create, resume, restore, the move onto a CLI's update) · queue.rs (the task queue's state,
│                    R60) · dispatch.rs (when a queued task is typed and how it is followed) · mod.rs
├── tail.rs          C4: finding and tailing a Codex pane's rollout
├── usage.rs         usage.get: Claude panes' status line reports, else the CLIs' own files, bounded, cached 5 s
├── skills.rs        skill.list: the folders each CLI reads its skills from, bounded, read-only, cached 10 s
├── accounts.rs      account.list / account.switch: `claude auth status`, the sign-in pane, the restart on a new sign-in;
│                    accounts/codex.rs: codex.account / codex.sign_in, auth.json read-only, `codex login`, the same restart
├── osc.rs · branch.rs   OSC 7/9 and typed input; the git branch label
├── publisher.rs     the C2 delivery rules as small clocked state machines
├── pty.rs           rustix pty + Command, setsid/TIOCSCTTY in the one audited pre_exec block
├── login.rs         the login shell and the environment every pane starts with
├── db/              mod.rs · migrations/0001_init.sql (schema v1)
├── config.rs · paths.rs · lock.rs (one plyd per data directory)
├── launchd.rs       the LaunchAgent plyd installs for itself (`plyd install-agent`)
└── power.rs         prevent idle sleep while a pane is running (caffeinate -i -w)

crates/app/src       (binary `ply`)
├── main.rs          the window, tabs, the pane grid, the ⌘ chords, the top bar; PaneView: one pane's header, mouse,
│                    wheel, selection, find bar and terminal canvas
├── conn.rs          C2 per pane: a reader thread attaches, decodes into a shared ply_term::Replica off the UI
│                    thread, acks, pages history and re-attaches (100 ms → 2 s)
├── paint.rs         the terminal element: replica rows → background quads and shaped lines snapped to the cell grid,
│                    the cursor, the selection and the link underline; the theme for the accent
├── select.rs        selection, copy text, word and line picks, find, http(s) links under the pointer
├── keys.rs          GPUI keystrokes → C2 KEY frames
├── chrome.rs        the palette and type scale, top bar, tab pills, pane frame and header, status chips, find bar
├── forms.rs         the ⌘N / ⌘T new-pane form, the ⌘⇧A account palette, the Claude and Codex account chips
├── dirs.rs          the new-pane form's folder suggestions: recents, the bounded repository scan, path completion
├── field.rs         a one-value text field with a caret
├── drop.rs          dropped files and pasted images typed as escaped paths; staged drops kept, then released
├── control.rs       the C1 client: the load, one-shot requests, layout.save, the event stream (reconnects 100 ms → 2 s)
└── launcher.rs      the data directory, finding plyd, starting it (install-agent, or --foreground under PLY_HOME)
```

## The terminal, end to end

plyd reads each pty on a dedicated std thread into a bounded channel; the pane's
task writes the bytes into its libghostty-vt `Engine`, which answers colour and
device queries itself (OSC 4/10/11, DA, DSR, `CSI ?u`, XTVERSION `ply <version>`)
from the palette the app sent with `theme.set` — set before any child starts,
because Codex probes within 250 ms. Dirty rows become a per-client Delta; while
DEC 2026 synchronized output is open no Delta is built (150 ms cap); an idle pane
sends nothing and is compressed after it goes quiet.

In the app, only the active tab's panes have a view, which the grid lays out
as equal columns up to three and quadrants at four (only the focused one is
drawn while the tab is zoomed); plyd caps a tab at four panes (`tab_full`). Each
view's connection (`conn.rs`) reads and decodes C2 on a thread of its own into
a `ply_term::Replica` and wakes the view once per batch of frames; the view
repaints on the display's next frame, and an idle pane costs nothing. The
terminal element (`paint.rs`) paints each row as background quads and one
shaped line per run of one style, every glyph snapped to its cell. Keys, mouse,
paste and focus go back as C2 events and plyd encodes them against the pane's
live modes; the app never mirrors terminal modes. A queued task (Ruling R60) is typed the same way,
by plyd: one paste and one Enter encoded against the pane's modes, only when it
is the user's turn there (`docs/agents.md`, **Dispatching tasks**). `docs/terminal.md` has the details and the
measured numbers.

## Startup

The app (`cargo run -p ply-app`, or the bundle's `MacOS/ply`) resolves its data
directory (`PLY_HOME`, else `~/Library/Application Support/ply`) and, before it
opens the window, connects to `run/plyd.sock`. If nothing answers,
`launcher.rs` starts plyd and waits up to 10 s for it: with `PLY_HOME` set it
runs `plyd --foreground` against that directory, detached; without it, it calls
`plyd install-agent`, which writes `~/Library/LaunchAgents/dev.ply.app.plyd.plist`
and kickstarts it. It finds plyd as `$PLY_PLYD`, else beside its own executable
(the bundle's `Contents/MacOS/plyd`, so the LaunchAgent names the bundle's
plyd), else the checkout's `target/release/plyd`, else `target/debug/plyd`. plyd takes the single-instance lock, opens `ply.db`, restores
tabs and panes (a pane whose process is gone becomes `lost` and can be resumed
with the CLI's own resume), binds its sockets under `run/` (mode 0700), reopens
the lost panes that have no session to resume as fresh shells and, with
`resume_sessions_on_start` (the default), resumes the others, and serves until
`daemon.shutdown`. The app then registers its embedded Geist fonts, reads
`settings.get` for the text size, opens the window and loads over C1:
`settings.get`, `theme.set` with the palette for the stored accent,
`workspace.list` (the home directory's workspace, else the most recently
opened), `layout.get` and `pane.list`; an event stream on a connection of its
own (`client` `ply-app`) keeps it current and loads again after a reconnect.

## Layering rules

- The UI is GPUI in Rust (the owner reversed D4, INV-3 and INV-13 on
  2026-11-11). plyd, ply-term and ply-agents never link `gpui`, and the app
  never links ghostty-sys or tokio (`checkCrateLayers`).
- libghostty-vt is linked only by plyd: `ghostty-sys` is reached through
  ply-term's `engine` feature, and only ply-daemon enables it
  (`checkInv17Ghostty`, which also checks that `ghostty-sys/build.rs` pins
  the ghostty commit, an archive URL naming it and the archive's SHA-256).
- Each crate's dependencies are checked against the table in `CLAUDE.md`
  (`checkCrateLayers`): every crate has an allow-list, so a new dependency
  needs a row change (dev- and build-dependencies may go beyond it), and
  ply-agents does no I/O beyond reading the files it is given.
- ply never creates, removes or records git worktrees; the only worktree column
  is `worktree_seen`, what the CLI reported (`checkInv7Worktrees`).
- Every direct dependency is admitted in `deps.allow.toml` with its INV-16
  numbers (`scripts/check-deps.ts`); `cargo deny` bans HTTP clients for every
  ply crate.
