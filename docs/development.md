# Development

## Toolchain

| Tool | Version | Install |
|---|---|---|
| Rust | 1.97.1 | `rustup toolchain install 1.97.1 --profile minimal --component rustfmt,clippy` (`rust-toolchain.toml` selects it inside the repository) |
| Zig | 0.17.0 | from ziglang.org; put `zig` on `PATH` or point `$ZIG` at it. `crates/ghostty-sys/build.rs` runs it to build libghostty-vt |
| Bun | 1.3.10 | `curl -fsSL https://bun.sh/install \| bash -s bun-v1.3.10` (`package.json` records it as `packageManager`) |
| just | any recent | `brew install just` |
| cargo-nextest, cargo-deny | any recent | `brew install cargo-nextest cargo-deny` or `cargo install --locked cargo-nextest cargo-deny` |

After cloning, run `bun install` once. `bunfig.toml` selects Bun's hoisted linker (so `bunx tsc` and `bunx biome`
resolve from the root) and makes `bun add` write exact versions.

The first build of `ghostty-sys` needs the network once: it downloads the ghostty source (about 40 MB) into
`~/Library/Caches/ply/ghostty/<commit>/` and libghostty-vt's Zig packages (about 6 MB) into
`~/Library/Caches/ply/ghostty/libghostty-vt-<version>-deps/`, checking each SHA-256; later builds use those caches. To
build offline, set `PLY_GHOSTTY_SRC` to an extracted ghostty tree at the pinned commit and `PLY_ZIG_PKG_DIR` to a
directory holding the packages extracted by hash. Never run `zig build` by hand inside the cached source: without the flags `ghostty-sys` passes it writes
`zig-out/`, `.zig-cache/` and `zig-pkg/` into the tree every later build reads.

## just recipes

| Recipe | Runs |
|---|---|
| `just check` (default) | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo doc --workspace --no-deps` with warnings as errors, `scripts/check-rules.ts`, `scripts/check-deps.ts`, `biome check`, `tsc -p scripts` |
| `just test` | `cargo nextest run --workspace`, the doctests (`cargo test --doc`) and `bun test ./scripts` |
| `just deny` | `cargo deny check` (licences, advisories, banned crates) |
| `just fmt` | `cargo fmt --all` and `biome check --write` |
| `just dmg` | `bun scripts/dmg.ts`: builds `dist/ply.app` and `dist/ply-<version>.dmg` (**The app bundle**, below) |
| `just icon` | renders `crates/app/assets/icon/ply.svg` at every size macOS asks for with `resvg` (`brew install resvg`) and packs them with `iconutil` into `crates/app/assets/icon/ply.icns`, which is committed, so `just dmg` needs neither tool |

`bun run check` and `bun run test` run the TypeScript half of the gates. `cargo run -p ply-app` opens the app;
`PLY_WINDOW_FOCUS=0` opens its window without taking focus, which keeps scripted or agent-driven runs from pulling it
in front of your editor.

## Where ply keeps its files

| What | Path |
|---|---|
| Database | `~/Library/Application Support/ply/ply.db` |
| Settings | `~/Library/Application Support/ply/config.toml` (also keeps the last `theme.set` palette) |
| Instance lock | `~/Library/Application Support/ply/plyd.lock` (holds the running plyd's pid) |
| Sockets and per-pane files | `~/Library/Application Support/ply/run/` (mode 0700): `plyd.sock` (C1), `data.sock` (C2), `hook.sock` (C3), `panes/<id>/claude-settings.json` and `panes/<id>/launch.json` |
| Logs | `~/Library/Logs/ply/plyd.YYYY-MM-DD.log`, kept 14 days, and the app's `app.YYYY-MM-DD` |
| LaunchAgent | `~/Library/LaunchAgents/dev.ply.app.plyd.plist` |

Socket paths must stay under 104 bytes, the macOS limit for a Unix socket path.

## Running plyd in the foreground

```sh
cargo build -p ply-daemon -p ply-hook
PLY_HOME=/tmp/ply-dev cargo run -p ply-daemon -- --foreground
```

Build `ply-hook` with plyd: agent panes run it from the directory plyd lives in.

`PLY_HOME` moves every path above but the LaunchAgent (database, settings, lock, run directory, logs, which go to
`$PLY_HOME/logs`) under one directory, so a development daemon never touches the everyday one and a test can start
from an empty state. A sandboxed plyd also never installs a LaunchAgent and never holds the keep-awake power
assertion. Keep `PLY_HOME`
short: the socket paths below it must stay under 104 bytes.

| Flag or variable | Effect |
|---|---|
| `--foreground` | also log to stderr (launchd starts plyd without it; both modes serve the same way) |
| `--run-dir <dir>` | put the sockets and `panes/` in another absolute directory |
| `PLY_LOG` | log level: `error`, `warn`, `info` (default), `debug`, `trace` |
| `plyd install-agent [--dry-run]` | write `~/Library/LaunchAgents/dev.ply.app.plyd.plist` for this plyd binary and `launchctl bootstrap` + `kickstart` it; while a plyd runs (it holds the instance lock) a changed plist is left alone, since booting the old agent out would kill every session, and the report names how to stop that plyd (below). `--dry-run` prints the plist and the commands instead. Refused while `PLY_HOME` is set |
| `plyd --version` | prints the build id, `plyd <version>+<commit>` |

plyd refuses to start a second time for the same data directory (it prints the running pid and exits 0), refuses a
database written by a newer plyd (also with status 0, so launchd does not restart it in a loop), and has no idle
exit: it stops only on `daemon.shutdown`, SIGTERM, SIGINT or SIGHUP. Panes whose process ran when plyd stopped come
back as `lost` and can be relaunched with `pane.resume`.

How the app finds plyd (`crates/app/src/launcher.rs`), once, when it starts and nothing answers on the control
socket: `$PLY_PLYD`, else a `plyd` beside the app's own executable (the bundle's, in `ply.app/Contents/MacOS`, or the
one cargo built into the same `target/<profile>/`), else `target/release/plyd`, else `target/debug/plyd`. With
`PLY_HOME` set it spawns that binary as `plyd --foreground`, detached; otherwise it runs `plyd install-agent` with it,
so `cargo run -p ply-app` without `PLY_HOME` installs a LaunchAgent pointing at the cargo-built plyd. It then waits up
to 10 s for plyd to answer. plyd is the only writer of that plist. `docs/configuration.md` lists everything a run
writes to the machine.

### Replacing a running plyd

A rebuilt plyd runs only once the running one stops: plyd has no idle exit, and `install-agent` never boots out an
agent whose plyd holds the lock. The app does not compare builds; plyd's build id (`welcome.daemon_version` and
`plyd --version`, `<version>+<commit>`, stamped by `crates/daemon/build.rs`) says which one runs. So build first
(`cargo build --release -p ply-daemon -p ply-hook`), stop the running plyd, and start the app again: it starts plyd
only when it starts, and after an outage it reconnects to whatever answers.

- **From a shell**, with the app open or not: `launchctl kill TERM gui/$(id -u)/dev.ply.app.plyd` stops the
  LaunchAgent's plyd cleanly (a SIGTERM stop exits 0, so launchd does not restart it), and
  `kill $(cat "$PLY_HOME/plyd.lock")` one under `PLY_HOME`. Running agents lose their pty with the old plyd and come
  back `lost`, which the new plyd resumes at its start (`resume_sessions_on_start`); shells reopen by themselves.
  `install-agent` names the `launchctl` line when it leaves a running plyd's changed plist alone.
- **Over C1**, `daemon.shutdown` does the same, and with `kill_panes: true` stops every pane's process first; the app
  has no command for it yet.

A newly installed bundle is the same case: while the previous build's plyd still runs, the bundle's app talks to it;
stop it and open the app again to hand the LaunchAgent to the bundle's plyd.

A C2 change bumps `C2_VERSION` and a C1 change `PROTOCOL_VERSION`, so an app and a plyd of different protocol versions
refuse each other cleanly (ATTACH_REFUSED reason 1, `version_mismatch`) rather than misreading frames.

A command-line client drives a running plyd through C1 and C2:

```sh
PLY_HOME=/tmp/ply-dev cargo run -p ply-daemon --example ply-cli -- demo
PLY_HOME=/tmp/ply-dev cargo run -p ply-daemon --example ply-cli -- list
PLY_HOME=/tmp/ply-dev cargo run -p ply-daemon --example ply-cli -- screen 1
```

`demo` opens a shell pane, runs an `echo`, detaches, reattaches and compares the two screens, then closes the pane;
`list` prints every workspace's panes; `screen` attaches at 80 × 24 (which resizes the pane) and prints its screen.
`create <cli> <cwd> [prompt]`, `type <pane> <text>` (`\r`, `\n` and `\e` are escapes), `answer <pane> <yes|no>`,
`close <pane>` and `watch <secs>` (every C1 event, stamped in Unix milliseconds) drive an agent pane by hand, for
example to time a real CLI's hooks against its `pane.status` events. `usage` prints the `usage.get` answer, the plan
usage the CLIs last reported, as JSON.

Tests that start real or fake CLIs also set a sandboxed `HOME` (and `CODEX_HOME`), so `~/.claude/settings.json` and
`~/.codex/config.toml` are never written (INV-8). plyd's own integration tests (`crates/daemon/tests/`) start plyd
with a cleared environment, a temporary `HOME` and `PLY_HOME`, and `/bin/sh` as the login shell. The agent tests
install the fake CLIs of `crates/daemon/tests/fake/` as `claude` and `codex` on that shell's `PATH`; they run the
`ply-hook` cargo built beside plyd, which `cargo nextest run --workspace` builds (run `cargo build -p ply-hook` before
`cargo nextest run -p ply-daemon`).

## The app bundle

`just dmg` (`scripts/dmg.ts`) builds `dist/ply.app` and packs it with an `Applications` link into
`dist/ply-<version>.dmg`. Install it by dragging `ply.app` to `/Applications`; run it from there, never from the
mounted image, because the LaunchAgent will name the plyd inside whichever copy started it.

```
ply.app/Contents/
├── Info.plist                  dev.ply.app, the minimum macOS of the binaries
├── MacOS/ply                   the app (crates/app, GPUI), cargo profile dist
├── MacOS/plyd, MacOS/ply-hook  cargo profile dist; plyd finds ply-hook beside itself; the app finds plyd beside itself
└── Resources/ply.icns          the icon, CFBundleIconFile
```

All three binaries are built at `[profile.dist]`: the release profile with fat LTO and one codegen unit,
whole-program optimisation across ply's crates. libghostty-vt is Zig's `ReleaseFast` in every build. When no plyd
answers, the app runs `plyd install-agent` with the plyd beside it (or, with `PLY_HOME` set, starts `plyd
--foreground`).

The script checks that `plyd --version` names the same `<version>+<commit>` it compiles into the app as
`PLY_BUILD_ID` (the root `package.json`'s version and the checkout's commit; the app reads it at compile time and says
it in `hello`), so the app and plyd of one bundle report the same build; it warns when the tree has uncommitted
changes, which the id does not show. The app carries Geist and Geist Mono compiled in.

Every Mach-O file is signed ad hoc (nested code first, then the bundle) and `codesign --verify --strict` must pass;
nothing is notarised, so a copy downloaded from elsewhere carries the quarantine flag and Gatekeeper refuses it until
`xattr -dr com.apple.quarantine /Applications/ply.app`. An ad-hoc signature changes with every build, so macOS asks
again for any permission it granted plyd's panes, as it does after every cargo build today. The image is LZFSE
(`ULFO`), which opens faster than zlib's `UDZO` at a similar size; `hdiutil verify` checks it. Apple-silicon Macs
only.

## Measuring the app

`ply --stats` (`cargo run --release -p ply-app -- --stats`) logs a `frame stats` line every second to the app log
(`docs/terminal.md`, **Measured**). For streaming load a debug plyd replays the recorded agent streams into live
panes: `plyd --replay <dir> [--speed N] [--panes K]` opens a workspace on `<dir>` in the running plyd of `PLY_HOME`,
asks it for K shell panes (default 6) and makes each `exec` a feeder that writes one `<dir>/*.bytes` stream to its
pty at N × 4 KiB/s, looping, then prints the pane ids:

```sh
PLY_HOME=/tmp/ply-dev target/debug/plyd --replay "$PWD/crates/term/tests/fixtures" --speed 10 --panes 4
```

`docs/perf.md` has the GPUI app's numbers against the React app's.

The golden C2 frames in `crates/proto/tests/golden/c2/*.bin` pin ply-proto's codec: after an intended layout change,
regenerate them with `PLY_BLESS=1 cargo test -p ply-proto --test golden_c2`.

## The gates

CI (`.github/workflows/ci.yml`) runs three jobs on macOS: `rust` (rustfmt, clippy, nextest, doctests, cargo-deny, rustdoc), `ts`
(Biome, tsc and bun test on `scripts/`) and `rules` (check-rules, check-deps, and check-pr on pull requests). Locally, `just check`
runs the formatting, lint, rustdoc, rule and type gates; `just test` runs nextest, the doctests and `bun test`;
`just deny` runs cargo-deny. Only check-pr has no local recipe.

- **Lints.** The workspace sets `missing_docs = "warn"` and rustdoc `broken_intra_doc_links = "deny"`; clippy runs
  with `-D warnings`, so an undocumented public item fails. `clippy::unwrap_used` and `expect_used` are denied outside
  tests (`clippy.toml`).
- **`scripts/check-rules.ts`** prints one line per violation and exits non-zero on any. One function per rule:
  INV-2 (no byte fields in C1 types), INV-7 (no `git worktree` call, no worktree column but `worktree_seen`), INV-11
  (no real home path or committer identity in any file), INV-17 (`ghostty-sys` reaches only `plyd` through
  `ply-term`, and `crates/ghostty-sys/build.rs` pins the ghostty commit, an archive URL naming it and the archive's
  SHA-256), the dependency layers of spec 8.2 for crates, the unsafe and `anyhow` rules of spec 9.1, and exact version
  pins. A check that needs code a later work package writes prints a `notice:` line instead.
- **`scripts/check-deps.ts`** fails when a direct dependency of any `Cargo.toml` or `package.json` is missing from
  `deps.allow.toml`, or when its recorded numbers fail INV-16.
- **`scripts/check-pr.ts`** requires a `Spec: x.y.z` line and a `WP: n` line in the pull-request body (`$PR_BODY`, or a
  file given as the first argument).
- **cargo-deny** (`deny.toml`) bans HTTP client crates for every ply crate (INV-1), allows only the listed licences
  and fails on advisories.

## Adding a dependency

INV-16 admits a direct third-party dependency only with at least 1 000 stars, a commit in the last six months, at
least three contributors with five or more commits, and no "experimental" label from its maintainers. libghostty-vt
(D5), `thiserror` and `anyhow` (R35) are the only exceptions, by decision.

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
