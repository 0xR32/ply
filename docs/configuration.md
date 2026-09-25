# Configuration

Where ply keeps its files, the one settings file, the environment variables
that change what it does, and what a run writes to the machine. ply runs from a
checkout: the app is `bun run dev` and the daemon is the cargo-built `plyd`.
There is no bundle and no installer.

`crates/daemon/src/paths.rs` resolves every path, `crates/daemon/src/config.rs`
reads and writes `config.toml`, and the app's side is `app/src/ipc/paths.ts`
and `app/src/ipc/log.ts`.

## Where it lives

| What | Default | With `PLY_HOME=<dir>` |
|---|---|---|
| data directory | `~/Library/Application Support/ply/` | `<dir>/` |
| database | `ply.db` in the data directory, with SQLite's `ply.db-wal` and `ply.db-shm` beside it | same |
| settings | `config.toml` in the data directory | same |
| instance lock | `plyd.lock` in the data directory, holding the running plyd's pid | same |
| run directory | `run/` in the data directory, mode `0700` | same |
| control socket (C1) | `run/plyd.sock` | same |
| screen socket (C2) | `run/data.sock` | same |
| hook socket (C3) | `run/hook.sock`: agent panes get its path as `PLY_HOOK_SOCK`, and `ply-hook` writes one line per hook to it | same |
| per-pane files | `run/panes/<id>/` (mode `0700`): `launch.json`, and `claude-settings.json` for Claude Code panes (mode `0600`) | same |
| logs | `~/Library/Logs/ply/plyd.YYYY-MM-DD.log` and `app.YYYY-MM-DD.log`, 14 days each | `<dir>/logs/` |
| LaunchAgent | `~/Library/LaunchAgents/dev.ply.app.plyd.plist` | never installed |
| ghostty source (builds only) | `~/Library/Caches/ply/ghostty/<commit>/` | same |

plyd creates the directories; it forces the run directory and each pane
directory to `0700` even when they existed, and writes `config.toml`,
`plyd.lock` and the pane files as `0600`. A pane's directory is removed when the
pane is closed.

**Socket paths must stay under 104 bytes**, the macOS limit for a Unix socket
path; plyd refuses to start when one would not (103 bytes plus the NUL). Keep
`PLY_HOME` short.

`plyd --run-dir <dir>` moves only the run directory, to an absolute path. Tests
use it; the app always looks under `$PLY_HOME/run` or the default, so it does
not find a plyd started that way.

## `config.toml`

The settings of C1's `settings.get` and `settings.set`, plus the palette of the
last `theme.set`. plyd is its only reader and writer: it reads the file once at
startup and rewrites all of it on every `settings.set` and `theme.set`, to
`config.toml.tmp` and then renamed over the old file. The new values apply
before the write, so a file that cannot be written (a full disk, a directory in
the way) costs only persistence: plyd logs it, answers the request `internal`,
and runs on the new values until it stops. The app gets the values over C1.

```toml
accent = "blue"
option_as_meta = "off"
keep_awake_while_running = true
use_ply_colours_in_claude = true
codex_plan_tool = true
scrollback_lines = 10000
font_size = 12.5

[palette]
ansi = ["#RRGGBB", …]            # exactly 16, ANSI 0–15
fg = "#RRGGBB"
bg = "#RRGGBB"
cursor = "#RRGGBB"
cursorText = "#RRGGBB"
selectionBg = "#RRGGBB"
selectionFg = "#RRGGBB"
```

Every key is optional; a missing one takes its default. A file with an unknown
key, a wrong type or bad TOML is ignored as a whole: plyd logs a warning, runs
on the defaults, and leaves the file alone until the next `settings.set` or
`theme.set` replaces it. A missing file is the defaults.

Because plyd reads the file only at startup and rewrites it on the next change,
edit it by hand only while plyd is stopped.

### Every key

| Key | Values | Default | Effect | Takes effect |
|---|---|---|---|---|
| `accent` | `blue`, `mint`, `violet`, `sand` | `blue` | The chrome's accent, and the terminal cursor and selection colours (the ANSI colours never change). The app sends a new `theme.set` when it changes. | at once |
| `option_as_meta` | `off`, `left`, `right`, `both` | `off` | Which ⌥ acts as Meta; `off` keeps ⌥ for the layout's characters (`docs/keybindings.md`). | at once, in every pane |
| `keep_awake_while_running` | bool | `true` | While a pane is `running`, plyd holds a prevent-idle-sleep assertion through `caffeinate -i -w <plyd pid>`, released as soon as none is; closing the lid still sleeps the Mac. Never held by a plyd under `PLY_HOME`; an agent pane is `running` as `docs/agents.md` describes. | at once |
| `use_ply_colours_in_claude` | bool | `true` | Adds `"theme":"dark-ansi"` to Claude Code's per-pane settings file, so Claude Code draws with the terminal's ANSI colours. | Claude Code panes started or resumed afterwards |
| `codex_plan_tool` | bool | `true` | Passes `-c tools.update_plan.enabled=true` to Codex, the only source of Codex progress. | Codex panes started or resumed afterwards |
| `scrollback_lines` | u32 | 10 000 | Scrollback lines each pane's terminal keeps; libghostty-vt keeps up to 300 more. | panes created afterwards, and panes restored at plyd's start |
| `font_size` | points | 12.5 | The size of the terminal text; the chrome scales with it. The app's ⌘= / ⌘- / ⌘0 keep it within 9.5–24.5. plyd only stores it. | at once |
| `palette` | a terminal theme (`docs/control-channel.md`, `TerminalTheme`) | none | The last `theme.set`. A restarted plyd uses it at once, so a pane can start before the app has sent the palette again. | written by `theme.set` |

The Settings overlay (⌘,) edits `accent`, `option_as_meta`,
`keep_awake_while_running` and `use_ply_colours_in_claude`; the font keys edit
`font_size`. `codex_plan_tool` and `scrollback_lines` have no control in the app:
set them in the file with plyd stopped, or with `settings.set`.

## Environment variables

**At run time:**

| Variable | Read by | Effect |
|---|---|---|
| `PLY_HOME` | plyd, the app, the mock server | Moves every path above under one directory (logs to `$PLY_HOME/logs`). plyd requires an absolute path and treats an empty value as unset. A plyd under `PLY_HOME` is *sandboxed*: it never installs a LaunchAgent and never holds a power assertion. The app then spawns plyd itself instead of going through the LaunchAgent. The mock server refuses to start without it. |
| `HOME` | plyd | The base of the default paths; also the directory of the default workspace. Must be absolute when `PLY_HOME` is unset. |
| `PLY_LOG` | plyd | Log level: `error`, `warn`, `info` (default), `debug`, `trace` or `off`. The app's log has no level filter. |
| `PLY_PLYD` | the app | The plyd binary to start, tried before `target/release/plyd` and then `target/debug/plyd` (release wins when both exist). |
| `PLY_WINDOW_FOCUS` | the app | `0` opens the window without taking focus, for scripted and agent-driven runs. |
| `PLY_TERMINAL_STATS` | the app | `1` shows each terminal's last decode and render time in its corner. |
| `SHELL`, `USER` | plyd | The login shell: `$SHELL` when it is an absolute executable, else the `UserShell` of `dscl . -read /Users/$USER`, else the passwd entry from `id -P`, else `/bin/zsh`. plyd then asks it once for its `PATH` as an interactive login shell (`$SHELL -l -i -c`, 5 s at most, stdin from `/dev/null`), because zsh reads `.zshrc`, where installers put `~/.local/bin`, only when interactive; a shell that fails or hangs so is asked as a plain login shell (`-l -c`). The log says which answered. The same probe reports the variables of the next row that the shell exports. |
| `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `ALL_PROXY` (and `http_proxy`, `https_proxy`, `no_proxy`, `all_proxy`), `SSL_CERT_FILE`, `SSL_CERT_DIR`, `NODE_EXTRA_CA_CERTS`, `LANG`, `LC_ALL`, `LC_CTYPE` | the login-shell probe, then every pane | Ruling R52: when the login shell (its rc files included) exports one of these, every pane gets that value, as in a terminal; plyd's own value goes into the probe and is kept when the shell does not change it, or when no shell answers. No other variable is read from the shell, so an API key an rc file exports never reaches plyd or a pane. The log names the variables taken (`crates/daemon/src/login.rs`, `CAPTURED`). |
| `LANG`, `LC_*`, `TMPDIR`, `LOGNAME`, `SSH_AUTH_SOCK`, `__CF_USER_TEXT_ENCODING` | plyd | Passed through to every pane when set (the login shell's `LANG`, `LC_ALL` and `LC_CTYPE` win, above); `LANG` defaults to `en_US.UTF-8`. |
| `NODE_ENV` | the app | `test` (set by `bun test`) keeps the log in memory instead of a file, and gives terminal views an inert host that never opens a socket or touches the pasteboard. |
| `CODEX_HOME` | Codex, plyd | Where Codex keeps its rollouts (`$CODEX_HOME/sessions/`, default `~/.codex`). plyd follows a Codex pane's rollouts under the `CODEX_HOME` that pane runs with, the login shell's when it exports one (above). Tests that run a CLI sandbox it together with `HOME`. |

**Set by plyd for panes** (`docs/agents.md`): `TERM=xterm-256color` and
`COLORTERM=truecolor` for every pane, `SHELL` and the login `PATH`;
`PLY_PANE_ID` and `PLY_HOOK_SOCK` for agent panes, which `ply-hook` reads; and
`CLAUDE_CODE_FORCE_SYNC_OUTPUT=1` for Claude Code panes.

**At build and test time:**

| Variable | Read by | Effect |
|---|---|---|
| `ZIG` | `crates/ghostty-sys/build.rs` | The Zig binary (else `zig` on `PATH`); it must be 0.16.0. |
| `LIBGHOSTTY_VT_OPTIMIZE` | `build.rs` | Zig's optimize mode for libghostty-vt: `Debug`, `ReleaseSafe`, `ReleaseFast` (default) or `ReleaseSmall`. |
| `PLY_GHOSTTY_SRC` | `build.rs` | An existing ghostty checkout at commit 44f2a44 to build from, instead of the downloaded and verified archive; the way to build offline. |
| `PLY_ZIG_PKG_DIR` | `build.rs` | A directory holding libghostty-vt's Zig packages extracted by hash, used instead of Zig's global cache; with `PLY_GHOSTTY_SRC`, a build needs no network. |
| `PLY_BLESS` | `crates/proto/tests/golden_c2.rs`, `crates/term/tests/replay.rs` | Rewrites the golden C2 frames, or the stored replay screens, instead of comparing. |
| `PLY_GEN_OUT` | `crates/proto/tests/export_bindings.rs` | Set by `scripts/gen.ts`: where the generated TypeScript goes. |
| `PLY_AGENT_CORPUS_DIR` | `crates/term/benches/throughput.rs` | The recorded agent captures the bench replays. |
| `PLY_SCREENSHOT_DIR` | `app/src/app/app.test.tsx` | Writes the test renderer's screenshots there. |

The terminal demo's `PLY_DEMO_*` variables are in `docs/development.md`.

## plyd's command line

```
plyd [--foreground] [--run-dir <dir>]    serve until daemon.shutdown, SIGTERM, SIGINT or SIGHUP
plyd install-agent [--dry-run]           install and start the LaunchAgent for this binary
plyd --version | --help
plyd --replay <dir> [--speed N] [--panes K]   debug builds only: stream recorded output into panes
```

`--foreground` also logs to stderr; both modes serve the same way. A second
plyd for the same data directory prints that one is running, with its pid, and
exits 0 — so launchd's restart-after-a-crash never loops on it. A database
written by a newer plyd is refused the same way, with status 0. Any other
startup failure (a socket that cannot be bound, a socket path too long) exits
1; a usage error exits 2.

`--version` prints plyd's build id, `plyd <version>+<commit>` (the id
`welcome.daemon_version` carries, `docs/control-channel.md`).

`--replay` (and the `--replay-feed` it runs in each pane) exists only in debug
builds, for the P1 bench; `docs/development.md` describes it.

`plyd install-agent` renders `packaging/plyd.plist.template` with the resolved
path of the plyd binary it runs as, writes the plist only when it changed
(booting the old definition out first — unless a plyd holds the instance lock:
booting it out would kill every session, so it reports that and leaves the
plist as it is), then runs `launchctl bootstrap
gui/<uid> <plist>` (an agent that is already loaded is fine) and `launchctl
kickstart gui/<uid>/dev.ply.app.plyd`. `--dry-run` prints the plist and the
commands and writes nothing. It refuses to run under `PLY_HOME`, which a
LaunchAgent would not see. plyd is the only writer of the plist
(`crates/daemon/src/launchd.rs`). Its tests run the same steps with a recording
runner in place of `launchctl` (`install_with`), so they never touch launchd.
When it leaves the plist alone because a plyd runs, its report names the ways
to stop that plyd: the palette's "Restart plyd" or "Quit ply and stop
sessions", or `launchctl kill TERM gui/$(id -u)/dev.ply.app.plyd`
(`docs/development.md`, "Replacing a running plyd").

The agent is `Label` `dev.ply.app.plyd` with plyd as its only argument,
`RunAtLoad` false (it does not start at login; the app starts it), `KeepAlive`
with `SuccessfulExit` false (launchd restarts it only after a crash) and
`ProcessType` `Standard` (`Background` would throttle every pane's I/O).

## What a run writes to the machine

### `bun run dev` with `PLY_HOME`

```sh
cargo build -p ply-daemon -p ply-hook
PLY_HOME=/tmp/ply-dev bun run dev
```

- The app connects to `/tmp/ply-dev/run/plyd.sock`. When nothing answers it
  starts the cargo-built plyd (`$PLY_PLYD`, else `target/release/plyd`, else
  `target/debug/plyd`) as `plyd --foreground`, detached, with its output
  discarded.
- plyd writes only under `/tmp/ply-dev`: the database, `config.toml`, the lock,
  `run/` and `logs/`. The app logs to `/tmp/ply-dev/logs/app.*.log`.
- No LaunchAgent, no power assertion, nothing under `~/Library`.
- The spawned plyd outlives the app, as it is meant to. Stop it with
  `kill $(cat /tmp/ply-dev/plyd.lock)` (SIGTERM, a clean stop) or a C1
  `daemon.shutdown`, which the palette's "Restart plyd" and "Quit ply and stop
  sessions" send; panes whose process was still running come back `lost` next
  time (shells reopen by themselves).

A plyd started by hand with `PLY_HOME=/tmp/ply-dev cargo run -p ply-daemon --
--foreground` is found by the app the same way.

### `bun run dev` without `PLY_HOME`

The everyday setup.

- The app connects to `~/Library/Application Support/ply/run/plyd.sock`. When
  nothing answers it runs `plyd install-agent` with the cargo-built plyd, which
  writes `~/Library/LaunchAgents/dev.ply.app.plyd.plist` naming that binary's
  path in the checkout and kickstarts it.
- plyd writes `~/Library/Application Support/ply/` and `~/Library/Logs/ply/`,
  and holds the keep-awake assertion while a pane runs.
- Quitting the app stops nothing; launchd keeps plyd and every pane running.
- The plist names a path inside `target/`. After `cargo clean` the agent points
  at nothing until plyd is built again at the same path; a build at another
  path is installed the next time the app has to start plyd.

`cargo run -p ply-daemon -- --foreground` without `PLY_HOME` serves the same
real data directory, without a LaunchAgent, unless the agent's plyd already
holds it.

### In both

- The panes' programs run as the user, in the user's `HOME`, and write what they
  always write — Claude Code under `~/.claude/`, Codex under `~/.codex/` — but
  ply never writes the CLIs' configuration files (INV-8, `docs/agents.md`).
- The app runs `defaults read com.apple.universalaccess reduceMotion` and
  `git rev-parse --short=12 HEAD` in its checkout (its build id) once at
  startup, and `pbcopy` or `pbpaste` when you copy or paste in a pane.
- Geist and Geist Mono are used only when installed (in `~/Library/Fonts` or
  `/Library/Fonts`), because GPUIX cannot load a font file; otherwise the chrome
  uses the system font and the terminal Menlo. The TTFs, under the SIL Open Font
  Licence, are in `app/assets/fonts/`; `just fonts` copies them into
  `~/Library/Fonts`, the one thing it writes.
- ply makes no network request while it runs (INV-1).

### Building

The first build of `ghostty-sys` downloads the ghostty source for commit 44f2a44
from `codeload.github.com`, checks its SHA-256, and keeps it in
`~/Library/Caches/ply/ghostty/<commit>/`; Zig fetches the library's packages
into its own cache when they are missing. Everything else a build writes stays
under `target/`. With `PLY_GHOSTTY_SRC` set nothing is downloaded for the
source (`docs/terminal.md`). `crates/daemon/build.rs` runs `git rev-parse` in
the checkout for plyd's build id (read-only, offline) and runs again when
`HEAD` or a branch moves.

### Removing it

```sh
launchctl bootout gui/$(id -u)/dev.ply.app.plyd
rm ~/Library/LaunchAgents/dev.ply.app.plyd.plist
rm -r ~/Library/Application\ Support/ply ~/Library/Logs/ply ~/Library/Caches/ply
```

`bootout` stops plyd with SIGTERM, and the panes' processes lose their pty with
it. To end them first, run "Quit ply and stop sessions" from the palette, or
close each pane with ⌘⇧W, which stops a live process before it closes the
pane.
