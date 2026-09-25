# ply

ply is a macOS app that runs the real `claude` (Claude Code) and `codex` (Codex CLI) terminal UIs side by side, each
in its own terminal pane, and adds what a terminal multiplexer lacks: a status per pane (running, needs permission,
needs input, your turn, exited), progress from the agent's own plan, a stored history of every session, the CLIs' own
worktrees, and a keyboard-first layout of tabs with several panes each. The agents keep running when the window
closes.

ply is **not** an LLM wrapper. It makes no network requests, calls no model API, holds no keys, rewrites no prompts,
hands no transcripts between CLIs and never picks a model or effort level: those are changed inside the CLI session.
ply only manages terminal panes and tracks and stores sessions.

## How it works

Two processes cooperate over three local Unix-socket protocols:

- **Ply.app** is a Bun process running React on [GPUIX](https://github.com/remorses/gpuix) (`@gpuix/react` and the
  prebuilt `@gpuix/native` from npm, painted by Zed's GPUI with Metal). It draws the tabs, panes, overlays and the
  terminal view, owns the keymap and the UI state, and sends control requests. It never opens a pty, never spawns an
  agent and never sees raw pty bytes: it receives decoded screen rows and draws each row as a line of styled text
  runs in Geist Mono.
- **plyd** is a per-user LaunchAgent, not a child of the app. It owns every pane's process and pty, runs one
  [libghostty-vt](https://github.com/ghostty-org/ghostty) terminal per pane, encodes keys, mouse, focus and paste
  against that terminal's live modes, derives each pane's status from the CLIs' own hooks and notifications, and
  stores sessions in SQLite. Quitting or crashing the app stops nothing; reopening it shows every pane's current
  screen at once.

The protocols are **C1** control (JSON lines on `run/plyd.sock`: panes, tabs, sessions, status events), **C2** screen
data (binary frames on `run/data.sock`, one connection per visible pane: snapshots, deltas, history, input) and **C3**
hook ingress (`ply-hook`, which Claude Code hooks and Codex's notify program run, forwards one JSON line to
`run/hook.sock`). The wire types live in `crates/proto`; the TypeScript side of C1 and C3 is generated from them.

## Repository layout

```
Cargo.toml, rust-toolchain.toml   Rust workspace (toolchain 1.97.1), exact dependency pins
package.json, bun.lock            Bun workspace ["app"]
crates/proto                      ply-proto: C1, C2, C3 wire types and protocol versions
crates/ghostty-sys                the Zig build of libghostty-vt and its C declarations (the only FFI crate)
crates/term                       ply-term: the libghostty-vt engine wrapper (plyd only) and the C2 replica
crates/agents                     ply-agents: Claude Code and Codex launch specs, hook and rollout parsing
crates/daemon                     ply-daemon, binary plyd
crates/hook                       ply-hook, the hook and notify forwarder
app/                              the React app (src/) and bundled fonts (assets/fonts, SIL OFL)
scripts/                          repository gates: check-rules, check-deps, check-pr, gen
deps.allow.toml                   every admitted dependency with its INV-16 numbers
docs/                             one document per subject (control channel, screen protocol, agents, terminal, …)
ARCHITECTURE.md                   processes, wire boundaries, module map, layering
CLAUDE.md                         rules for agents working in this repository
```

## Prerequisites

macOS 13 or later on Apple Silicon, with:

- Rust 1.97.1 (`rust-toolchain.toml` selects it; `rustup` installs it), plus `cargo-nextest` and `cargo-deny` for the
  full gate set
- Zig 0.16.0 on `PATH` or in `$ZIG` (libghostty-vt builds with it; the first build downloads the ghostty source, about
  40 MB, once)
- Bun 1.3.10
- [just](https://github.com/casey/just)
- `claude` and/or `codex` installed and signed in, for real agent panes

## Commands

| Command | What it does |
|---|---|
| `bun install` | installs the app's dependencies (exact pins from `bun.lock`) |
| `bun run dev` | opens the ply window with hot reload (`bun --hot app/src/main.tsx`) |
| `cargo build --workspace` | builds every crate, including `plyd` and `ply-hook` |
| `just check` | rustfmt, clippy `-D warnings`, rustdoc, check-rules, check-deps, Biome, tsc |
| `just test` | `cargo nextest run` and `bun test` |
| `just deny` | cargo-deny: licences, advisories, the HTTP-client and gpui bans |
| `bun run gen` | regenerates `app/src/ipc/proto.gen.ts` from ply-proto |

## Documentation

- [Architecture](ARCHITECTURE.md): the three processes, the C1/C2/C3 boundaries, the module map and the layering rules.
- [Control channel](docs/control-channel.md) and [screen protocol](docs/screen-protocol.md): the two protocols
  between the app and plyd, byte for byte.
- [Agents](docs/agents.md): how Claude Code and Codex are launched and tracked, and the status state machine.
- [Terminal](docs/terminal.md): libghostty-vt, input encoding and the React terminal view.
- [Keybindings](docs/keybindings.md) and [configuration](docs/configuration.md).
- [Development guide](docs/development.md): toolchain setup, the run directory and logs, running plyd in the
  foreground, how the gates work, and how to admit a dependency.
- [Specification](docs/spec.html): the implementation specification the build started from.
