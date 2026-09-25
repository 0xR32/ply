# Development

## Toolchain

| Tool | Version | Install |
|---|---|---|
| Rust | 1.97.1 | `rustup toolchain install 1.97.1 --profile minimal --component rustfmt,clippy` (`rust-toolchain.toml` selects it inside the repository) |
| Zig | 0.16.0 | from ziglang.org; put `zig` on `PATH` or point `$ZIG` at it. `crates/ghostty-sys/build.rs` runs it to build libghostty-vt |
| Bun | 1.3.10 | `curl -fsSL https://bun.sh/install \| bash -s bun-v1.3.10` (`package.json` records it as `packageManager`) |
| just | any recent | `brew install just` |
| cargo-nextest, cargo-deny | any recent | `brew install cargo-nextest cargo-deny` or `cargo install --locked cargo-nextest cargo-deny` |

After cloning, run `bun install` once. `bunfig.toml` selects Bun's hoisted linker (so `bunx tsc` and `bunx biome`
resolve from the root) and makes `bun add` write exact versions.

Never run `zig build` by hand inside `vendor/libghostty-vt`: without the flags `ghostty-sys` passes it fetches packages
into the vendor tree, and `check-rules` then reports the tree as modified.

## just recipes

| Recipe | Runs |
|---|---|
| `just check` (default) | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo doc --workspace --no-deps` with warnings as errors, `scripts/check-rules.ts`, `scripts/check-deps.ts`, `biome check`, `tsc -p app`, `tsc -p scripts` |
| `just test` | `cargo nextest run --workspace`, the doctests (`cargo test --doc`) and `bun test ./app ./scripts` |
| `just deny` | `cargo deny check` (licences, advisories, banned crates) |
| `just gen` | `bun scripts/gen.ts`: runs ply-proto's `export_bindings` test with `PLY_GEN_OUT`, formats the ts-rs output with Biome and writes `app/src/ipc/proto.gen.ts` (`--check` compares instead) |
| `just dev` | `bun --hot app/src/main.tsx` |
| `just fmt` | `cargo fmt --all` and `biome check --write` |

`bun run dev`, `bun run check`, `bun run test` and `bun run gen` are the same entry points for the TypeScript side.
`PLY_WINDOW_FOCUS=0 bun run dev` opens the window without taking focus, which keeps scripted or agent-driven runs from
pulling it in front of your editor.

## Where ply keeps its files

| What | Path |
|---|---|
| Database | `~/Library/Application Support/ply/ply.db` |
| Settings | `~/Library/Application Support/ply/config.toml` (also keeps the last `theme.set` palette) |
| Instance lock | `~/Library/Application Support/ply/plyd.lock` (holds the running plyd's pid) |
| Sockets and per-pane files | `~/Library/Application Support/ply/run/` (mode 0700): `plyd.sock` (C1), `data.sock` (C2), `hook.sock` (C3), `panes/<id>/claude-settings.json` and `panes/<id>/launch.json` |
| Logs | `~/Library/Logs/ply/app.YYYY-MM-DD.log` and `plyd.YYYY-MM-DD.log`, kept 14 days |
| LaunchAgent | `~/Library/LaunchAgents/dev.ply.app.plyd.plist` |

Socket paths must stay under 104 bytes, the macOS limit for a Unix socket path.

## Running plyd in the foreground

```sh
cargo build -p ply-daemon -p ply-hook
PLY_HOME=/tmp/ply-dev cargo run -p ply-daemon -- --foreground
```

Build `ply-hook` with plyd: agent panes run it from the directory plyd lives in.

`PLY_HOME` moves every path above (database, settings, run directory, logs, which go to `$PLY_HOME/logs`) under one
directory, so a development daemon never touches the installed one and a test can start from an empty state. A
sandboxed plyd also never installs a LaunchAgent and never holds the keep-awake power assertion. Keep `PLY_HOME`
short: the socket paths below it must stay under 104 bytes.

| Flag or variable | Effect |
|---|---|
| `--foreground` | also log to stderr (launchd starts plyd without it; both modes serve the same way) |
| `--run-dir <dir>` | put the sockets and `panes/` in another absolute directory |
| `PLY_LOG` | log level: `error`, `warn`, `info` (default), `debug`, `trace` |
| `plyd install-agent [--dry-run]` | write `~/Library/LaunchAgents/dev.ply.app.plyd.plist` for this plyd binary and `launchctl bootstrap` + `kickstart` it; `--dry-run` prints the plist and the commands instead. Refused while `PLY_HOME` is set |

plyd refuses to start a second time for the same data directory (it prints the running pid and exits 0), refuses a
database written by a newer plyd, and has no idle exit: it stops only on `daemon.shutdown`, SIGTERM, SIGINT or
SIGHUP. Panes whose process ran when plyd stopped come back as `lost` and can be relaunched with `pane.resume`.

How the app finds plyd (`app/src/ipc/daemon-launcher.ts`): with `PLY_HOME` set it spawns `target/debug/plyd
--foreground` (or `$PLY_PLYD`) detached; otherwise it runs `plyd install-agent` for the bundle's
`Contents/MacOS/plyd`, or in development for the cargo-built one, so `bun run dev` without `PLY_HOME` installs a
LaunchAgent pointing at `target/debug/plyd`. plyd is the only writer of that plist.

A command-line client drives a running plyd through C1 and C2:

```sh
PLY_HOME=/tmp/ply-dev cargo run -p ply-daemon --example ply-cli -- demo
PLY_HOME=/tmp/ply-dev cargo run -p ply-daemon --example ply-cli -- list
PLY_HOME=/tmp/ply-dev cargo run -p ply-daemon --example ply-cli -- screen 1
```

`demo` opens a shell pane, runs an `echo`, detaches, reattaches and compares the two screens, then closes the pane;
`list` prints every workspace's panes; `screen` attaches at 80 × 24 (which resizes the pane) and prints its screen.

Tests that start real or fake CLIs also set a sandboxed `HOME` (and `CODEX_HOME`), so `~/.claude/settings.json` and
`~/.codex/config.toml` are never written (INV-8). plyd's own integration tests (`crates/daemon/tests/`) start plyd
with a cleared environment, a temporary `HOME` and `PLY_HOME`, and `/bin/sh` as the login shell.

## The terminal view on its own

`app/src/dev/terminal-demo.tsx` shows plyd's panes in a bare window, without the app shell. It sends the ply palette
(`theme.set`), then opens `PLY_DEMO_PANES` (1–6) new shell panes in the home directory, or attaches to existing ones
with `PLY_DEMO_ATTACH=<id,id,…>`:

```sh
PLY_HOME=/tmp/ply-dev bun --hot app/src/dev/terminal-demo.tsx
PLY_HOME=/tmp/ply-dev PLY_DEMO_PANES=4 PLY_DEMO_INPUT='yes | head -c 50000000' PLY_TERMINAL_STATS=1 \
  bun app/src/dev/terminal-demo.tsx
```

| Variable | Effect |
|---|---|
| `PLY_DEMO_INPUT` | a command typed into every new pane once it is attached (sent as C2 `INPUT_RAW`) |
| `PLY_DEMO_WIDTH`, `PLY_DEMO_HEIGHT` | the window size (default 1280 × 800); attaching sizes the pane to the view |
| `PLY_TERMINAL_STATS=1` | GPUIX's frame overlay, a decode/render line in each pane, and a timing line in the app log every 2 s |
| `PLY_WINDOW_FOCUS=0` | open the window without taking focus |

For the P1 bench a debug plyd replays the recorded agent streams into live panes: `plyd --replay <dir> [--speed N]
[--panes K]` asks the running plyd of `PLY_HOME` for K shell panes (default 6) and makes each `exec` a feeder that
writes one `<dir>/*.bytes` stream to its pty at N × 4 KiB/s, looping. It prints the pane ids for `PLY_DEMO_ATTACH`:

```sh
PLY_HOME=/tmp/ply-dev target/debug/plyd --replay "$PWD/crates/term/tests/fixtures" --speed 10 --panes 6
PLY_HOME=/tmp/ply-dev PLY_DEMO_ATTACH=<printed ids> PLY_TERMINAL_STATS=1 bun app/src/dev/terminal-demo.tsx
```

The TypeScript C2 codec is pinned to ply-proto's by `crates/proto/tests/golden/c2/*.bin`: after an intended layout
change, regenerate them with `PLY_BLESS=1 cargo test -p ply-proto --test golden_c2` and update
`app/src/terminal/frames.test.ts` to match. The recorded screens in `app/src/terminal/fixtures/` are ply-term
Snapshots and Deltas of the scrubbed streams in `crates/term/tests/fixtures/`. Under `bun test` a terminal view with
no `TerminalHostContext` above it never connects, so no test reaches a real plyd or the pasteboard.

## The gates

CI (`.github/workflows/ci.yml`) runs three jobs on macOS: `rust` (rustfmt, clippy, nextest, doctests, cargo-deny, rustdoc), `ts`
(Biome, tsc, bun test) and `rules` (check-rules, check-deps, and check-pr on pull requests). `just check` runs the
same checks locally, except nextest and cargo-deny.

- **Lints.** The workspace sets `missing_docs = "warn"` and rustdoc `broken_intra_doc_links = "deny"`; clippy runs
  with `-D warnings`, so an undocumented public item fails. `clippy::unwrap_used` and `expect_used` are denied outside
  tests (`clippy.toml`).
- **`scripts/check-rules.ts`** prints one line per violation and exits non-zero on any. One function per rule:
  INV-1 (no network API in `app/`), INV-2 (no byte fields in C1 types, no pty API in `app/`), INV-3 (no ply crate
  reaches `gpui`), INV-4 (`onKeyDown` only in the keymap dispatcher, the overlay forms and the terminal view), INV-5
  (colour literals only in `app/src/theme/tokens.ts` and fixtures), INV-7 (no `git worktree` call, no worktree column
  but `worktree_seen`), INV-11 (no real home path or committer identity outside `vendor/`), INV-13 (GPUIX pinned
  exactly at 0.10.0 from npm, unpatched), INV-17 (`ghostty-sys` reaches only `plyd` through `ply-term`, and
  `vendor/libghostty-vt` matches the content hash in its `vendor.json`), the dependency layers of spec 8.2 for crates
  and for `app/src` imports, the unsafe and `anyhow` rules of spec 9.1, exact version pins, and the freshness of
  `proto.gen.ts`. A check that needs code a later work package writes prints a `notice:` line instead.
  `bun scripts/check-rules.ts --vendor-hash` prints the vendor tree's current hash.
- **`scripts/check-deps.ts`** fails when a direct dependency of any `Cargo.toml` or `package.json` is missing from
  `deps.allow.toml`, or when its recorded numbers fail INV-16.
- **`scripts/check-pr.ts`** requires a `Spec: x.y.z` line and a `WP: n` line in the pull-request body (`$PR_BODY`, or a
  file given as the first argument).
- **cargo-deny** (`deny.toml`) bans HTTP client crates and `gpui` for every ply crate (INV-1, INV-3), allows only the
  listed licences and fails on advisories.

## Adding a dependency

INV-16 admits a direct third-party dependency only with at least 1 000 stars, a commit in the last six months, at
least three contributors with five or more commits, and no "experimental" label from its maintainers. GPUIX (D4) and
libghostty-vt (D5) are the only exceptions, by decision.

1. Check the numbers and add the entry in one step (needs an authenticated `gh`):

   ```sh
   bun scripts/check-deps.ts --refresh --add cargo:tokio=tokio-rs/tokio
   ```

   `--add <cargo|npm|vendor>:<name>=<owner/repo>` records `repo`, `stars`, `last_commit`, `contributors_5plus` and
   `checked` in `deps.allow.toml`. `--refresh` alone re-checks every entry; `--refresh cargo:serde` re-checks one.
2. If `check-deps` reports that the dependency fails INV-16, do not add it: pick an admitted alternative or write the
   code yourself.
3. Pin it exactly: a Rust crate goes into `[workspace.dependencies]` of the root `Cargo.toml` as `"=x.y.z"` and each
   crate uses `name.workspace = true`; an npm package is added with `bun add --exact`.
4. Check that the crate's row of the layer table allows it (`scripts/check-rules.ts`, spec 8.2) and that cargo-deny
   accepts its licence.
