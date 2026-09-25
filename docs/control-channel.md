# The control channel

How a client drives plyd: C1, JSON lines over a Unix socket. This document is
the whole protocol. A client that links no ply crate has to be able to
implement it from here; `crates/daemon/examples/ply-cli.rs` (through
`crates/daemon/tests/common/client.rs`) speaks it by hand against a running
plyd to keep that true.

The Rust types in `crates/proto/src/control.rs` and `crates/proto/src/pane.rs`
are the single source. `bun run gen` writes `app/src/ipc/proto.gen.ts` from them
with ts-rs, and `check-rules` (`checkGeneratedTypes`) fails when the committed
file is stale. The screen itself travels on a second socket;
`docs/screen-protocol.md` is that one. C1 carries no pty bytes (INV-2,
`checkInv2NoPtyBytes` and `c1_types_carry_no_pty_bytes`).

## Where it is

| Run | Socket |
|---|---|
| default | `~/Library/Application Support/ply/run/plyd.sock` |
| `PLY_HOME=<dir>` | `<dir>/run/plyd.sock` |
| `plyd --run-dir <dir>` | `<dir>/plyd.sock` |

plyd creates the run directory and forces it to mode `0700`, even when it
already existed (`crates/daemon/src/paths.rs`, `create_private_dir`). That
directory is the only access control: the socket carries no credential, and
whoever can open it is the OS user plyd runs as.

A socket path longer than 103 bytes cannot be bound on macOS (`sun_path` is 104
bytes with its NUL), and plyd refuses to start rather than truncate it
(`check_socket_path`). This is why `PLY_HOME` must stay short.

**A stale socket.** plyd takes the single-instance lock (`plyd.lock`, an
exclusive `flock` beside the database) before it binds anything, so no other
plyd serves this data directory. It then removes whatever file is at the socket
path and binds a fresh one (`crates/daemon/src/daemon.rs`, `bind`). On a clean
stop it removes the socket again.

## Framing

UTF-8 JSON, one object per line, each line ending in `\n`. A line is at most
1 MiB (`MAX_LINE_BYTES`, newline included). plyd closes a connection whose line
passes the cap without answering it; the app's client drops a connection whose
line from plyd does.

Every message is an object tagged by `"t"`:

| `t` | Direction | Carries |
|---|---|---|
| `hello` | client → plyd | the handshake, first line of every connection |
| `req` | client → plyd | one method call |
| `welcome` | plyd → client | the handshake answer |
| `res` | plyd → client | the answer to one request |
| `evt` | plyd → client | a daemon event; events have no id |

Every struct rejects unknown fields (`#[serde(deny_unknown_fields)]`, INV-10),
nested ones included. An optional field is omitted when absent; `null` is
accepted in its place. Integers that are ids or times are unsigned and stay
below 2^53, so a JavaScript number holds them. Times are Unix seconds, UTC.

## The handshake

```
→ {"t":"hello","v":1,"client":"ply-app","app_version":"0.1.0"}
← {"t":"welcome","v":1,"daemon_version":"0.1.0+0a1b2c3d4e5f"}
```

1. The client sends `hello` as its first line: `v` is the protocol version it
   speaks, `client` its name (`"ply-app"` for the app) and `app_version` its
   build version.
2. If `v` equals plyd's `PROTOCOL_VERSION` (1, `crates/proto/src/version.rs`),
   plyd answers `welcome` with its own `v` and `daemon_version`, its build id
   `<package version>+<commit>`: the first 12 hex digits of the checkout's
   `HEAD` when plyd was built (`crates/daemon/build.rs`), or `t<unix seconds>`
   of the build outside a git checkout. Two plyd builds of one version but
   different commits say so here; the protocol version does not change with
   them.
3. Otherwise plyd answers a `version_mismatch` error on request id 0 and closes:

   ```
   ← {"t":"res","id":0,"ok":false,"err":{"code":"version_mismatch","msg":"…"}}
   ```

The rule is equality, not "at least". Version 1 is frozen; a change to any
message bumps it. The client checks `welcome.v` the same way.

Anything else as the first line also ends the connection: a `req` is answered
`bad_request` ("send hello first") on its own id, and an unreadable line is
answered on its id when one can be read, else on id 0. A second `hello` later on
the same connection is ignored and logged.

plyd subscribes the connection to the event broadcast *before* it writes
`welcome`, so a client that has read `welcome` misses no event emitted after it.

## Requests and responses

```
→ {"t":"req","id":7,"m":"pane.close","p":{"pane_id":2,"kill":false}}
← {"t":"res","id":7,"ok":true,"r":{}}
← {"t":"res","id":7,"ok":false,"err":{"code":"pane_alive","msg":"the pane's process is still running"}}
```

- `id` is the client's, a u64 it chooses; plyd echoes it. The app numbers from
  1 when it starts and keeps counting across reconnects, so no id repeats
  within one run of the app. Id 0 is reserved for plyd's answer to a refused `hello`
  (`HANDSHAKE_ID`).
- `m` is the method name and `p` its params. `p` is required; methods without
  params take `{}`.
- A success is `ok: true` with the result in `r`. A failure is `ok: false` with
  `err: {code, msg}`. A response has exactly one of `r` and `err`.
- `code` is one of the closed list under **Errors**. Clients match on `code`,
  never on `msg`; `msg` is a sentence meant for the user and is shown as is.

**Refused lines.** A `req` naming no known method is answered
`unknown_method`; one whose params do not deserialize (a missing field, an
unknown field, a wrong type, a value out of range) is answered `bad_request`.
Both keep the request's id (`ClientMsg::decode` returns a `Rejection` carrying
it). A line with no readable id gets no answer; plyd logs it and reads on.

**Order.** plyd handles one connection's requests one at a time, in the order
they arrived, and answers them in that order. A slow request delays the ones
behind it on the same connection: `pane.create` and `pane.resume` can wait up
to 5 s for a palette (see `pane.create`). Connections do not wait for each
other.

## Methods

The table in `crates/proto/src/control.rs` (`Call` and `METHODS`) is the
complete list; `proto.gen.ts` exports it as the `Methods` type map. The records
the results are made of are under **Records**.

| Method | Params | Result |
|---|---|---|
| `workspace.list` | `{}` | `Workspace[]` |
| `workspace.open` | `{path}` | `Workspace` |
| `pane.list` | `{workspace_id}` | `Pane[]` |
| `pane.create` | `{workspace_id, tab_id?, cli, cwd, worktree?: {name}, prompt?}` | `Pane` |
| `pane.close` | `{pane_id, kill}` | `{}` |
| `pane.answer` | `{pane_id, choice}` | `{}` |
| `pane.resume` | `{pane_id}` | `Pane` |
| `session.list` | `{workspace_id, include_closed}` | `Session[]` |
| `theme.set` | `{palette}` | `{}` |
| `layout.get` | `{workspace_id}` | `Layout` |
| `layout.save` | `{workspace_id, layout}` | `{}` |
| `settings.get` | `{}` | `Settings` |
| `settings.set` | `{settings}` | `{}` |
| `daemon.shutdown` | `{kill_panes}` | `{}` |

The implementations are `crates/daemon/src/server/control.rs` (`dispatch`),
`crates/daemon/src/panes/launch.rs` and `crates/daemon/src/panes/registry.rs`.

### `workspace.list`

Every workspace, by id. The first start of plyd creates one for the home
directory (`HOME` of plyd's environment), named after its basename, so the list
is never empty.

### `workspace.open`

`path` must be absolute and name a directory, else `bad_request`. A known path
returns the existing workspace with `opened_at` set to now; a new one is created
and named after its basename.

### `pane.list`

The open panes of the workspace: every pane that has not been closed, whatever
its status (`exited` and `lost` panes are open until `pane.close`), once
`pane.added` announced it (a pane whose spawn is still running is not listed). `not_found`
for an unknown workspace. `Pane.position` and `Pane.tab_id` give the
arrangement; `layout.get` gives the tab order.

### `pane.create`

Starts a process in a new pane and returns it.

| Param | Type | Meaning |
|---|---|---|
| `workspace_id` | u64 | Workspace to create the pane in. |
| `tab_id` | u64, optional | Existing tab to append the pane to. Without it the pane opens a new tab at the end, named after `basename(cwd)`. |
| `cli` | `"claude"` \| `"codex"` \| `"shell"` | What to run. |
| `cwd` | string | Absolute path of an existing directory. |
| `worktree` | `{name}`, optional | Claude Code only: passed as `claude --worktree <name>`. The CLI creates and tracks the worktree; ply never does (INV-7). |
| `prompt` | string, optional | First prompt, passed as the CLI's positional prompt argument. Not for `shell`. |

In order, plyd:

1. refuses with `shutting_down` once a shutdown was requested;
2. checks the params: `cwd` absolute and a directory, `worktree` only with
   `claude`, `prompt` not with `shell` (`bad_request`);
3. waits up to 5 s for a palette — the one stored from an earlier `theme.set`,
   or the next one to arrive — because a pane's terminal must answer colour
   queries before its child's first byte (`invalid_state` "send theme.set
   first" when none comes);
4. resolves the program: the login shell for `shell`, else `claude` or `codex`
   on the login shell's `PATH` (`cli_not_found`), and reads the CLI's version
   from its install layout without running it (`cli_too_old` below the minimum;
   an unknown version is logged and allowed; `docs/agents.md`);
5. records the pane (`not_found` for an unknown workspace or a `tab_id` outside
   it), writes `run/panes/<id>/` with the launch files and `launch.json`,
   sizes a new terminal to the last view size any client attached with,
   starts the pane's task with the launch spec, and spawns the process
   (`spawn_failed`; `bad_request` when the adapter refuses a value, such as a
   worktree name starting with `-`); the task adopts it and publishes its
   status;
6. broadcasts `pane.added` and returns the pane. An agent that already reported
   readiness is `idle` in both.

A failed spawn leaves no pane, tab or directory behind. A shell pane starts
`idle`; an agent pane starts `starting`.

### `pane.close`

| `kill` | Pane | What happens |
|---|---|---|
| `false` | live (any status but `exited`, `lost`) | `pane_alive`; nothing changes. |
| `true` | live | Answers `{}` at once. plyd sends SIGHUP to the process group and, 2 s later, SIGKILL to the whole group, even when its leader has already exited (a member that ignored SIGHUP would keep the pty open). When the leader has exited: `pane.status` (`exited`), `pane.exit`, then `pane.removed`. |
| either | `exited` or `lost` | Closed now: `closed_at` is stored, the pane leaves its tab (a tab with no pane left is deleted), `run/panes/<id>/` is removed and `pane.removed` is broadcast. |

A closed pane's record stays in the database and is returned by
`session.list {include_closed:true}`. `not_found` for an unknown pane.

### `pane.answer`

Writes `choice` (1, 2 or 3; anything else is `bad_request`) to the pane's pty
as that digit, which is how the CLIs' permission dialogs are answered. The pane
must be `waiting_permission` or `waiting_input`, else `invalid_state`. The
digit also counts as a key typed, so the pane moves to `running`
(`docs/agents.md`).

### `pane.resume`

Relaunches a `lost` pane (`invalid_state` for any other status, and for a
pane another `pane.resume` is relaunching) into the same pane and terminal, from
its `run/panes/<id>/launch.json`. The pane leaves `lost` at once (`pane.status`
`starting`), and goes back to `lost` when the relaunch fails. An agent pane's launch
spec is rebuilt through its adapter with the pane's `session_ref` as the session
to resume (`claude --resume <id>`, `codex resume <thread>`), in the stored
working directory and with the stored worktree option; a pane without a
session id (a shell, or an agent whose CLI never reported one) reopens as a
fresh login shell in its last directory and is a `shell` pane from then on;
plyd does that by itself at its start (Ruling R50), so such a pane is `lost`
only when that reopening failed.
Waits for a palette like `pane.create`. `spawn_failed` when `launch.json` is
missing or unreadable, the directory is gone or the spawn fails;
`cli_not_found`, `cli_too_old` and `bad_request` as for `pane.create`. Returns
the pane, which is `starting` (agents) or `idle` (shells) again.

### `session.list`

The stored record of every pane of the workspace, one `Session` each: open
panes first, then closed ones, each group by id. `include_closed: false` returns
only the open ones. `not_found` for an unknown workspace.

### `theme.set`

Stores `palette` in `config.toml` and applies it to every pane's terminal,
which answers OSC 4, 10, 11 and 12 and the colour-scheme query from it
(`docs/terminal.md`). A spawn waiting for a palette proceeds. The app sends it
on every connect, before anything else that could spawn, and again whenever the
accent changes.

### `layout.get`

The workspace's tabs in bar order, each with its panes in position order, its
focused pane and its zoom, and `active_tab_id`. `active_tab_id` is kept in
memory only (schema v1 has no column for it), so it survives app restarts but
not plyd restarts; it is absent when unknown.

### `layout.save`

Replaces the workspace's arrangement with `layout`:

- every tab id must be a tab of this workspace, every pane id an open pane of
  it, and no pane may appear twice, else `bad_request` and nothing changes;
- tabs take the order of their `position` values; tabs the layout leaves out
  keep their relative order after the ones it names;
- each named tab takes `name`, `zoomed` and the panes in `pane_ids` order,
  followed by any pane it held that the layout names nowhere; `focus_pane_id`
  replaces the focus when given, and a focus that is not in the tab falls back
  to its first pane and turns zoom off;
- a tab left without panes is deleted, and positions are renumbered from 0.

Moving panes between tabs changes their `tab_id` and `position` without an
event; `pane.list` and `layout.get` show the result.

### `settings.get` and `settings.set`

`settings.get` returns the stored `Settings`. `settings.set` replaces all of
them (every field is required) and writes `config.toml`. A changed
`option_as_meta` is pushed to every pane's key encoder at once, and the
keep-awake assertion is re-evaluated. The other fields take effect where
`docs/configuration.md` says.

### `daemon.shutdown`

Answers `{}`, then plyd stops: it broadcasts `daemon.stopping`, and with
`kill_panes: true` sends every live pane SIGHUP (SIGKILL after 2 s) and waits up
to 4 s for them. It then stops serving, removes its sockets and exits. Without
`kill_panes` the processes lose their pty with plyd and their panes come back
`lost` on the next start. The first shutdown request wins; SIGTERM, SIGINT and
SIGHUP stop plyd the same way with `kill_panes: false`.

The app sends it from two palette commands (Ruling R53): "Restart plyd" sends
`kill_panes: false`, and the reconnect that follows starts the build in
`target/` (see **Starting plyd** below), whose panes come back as a restart
leaves them (a shell reopens by itself, an agent pane is `lost` until
resumed); "Quit ply and stop sessions" asks first, sends `kill_panes: true`
and quits the app once the answer arrives (or at once when plyd is not
connected).

## Events

```
← {"t":"evt","e":"pane.status","p":{"pane_id":2,"status":"waiting_permission","detail":"Edit GuideDot.tsx","at":1790276000}}
```

`e` names the event and `p` carries its payload. plyd broadcasts every event to
every connection that completed the handshake. An event emitted while no client
is connected is dropped; a client learns the current state from `pane.list`,
`layout.get` and `session.list` when it connects.

| Event | Payload | When |
|---|---|---|
| `pane.added` | a `Pane` | A pane was created, by any client (`pane.create`). Panes restored at plyd's start are not announced. |
| `pane.removed` | `{pane_id}` | A pane was closed (`pane.close`, or a `kill:true` close whose process has now exited). |
| `pane.status` | `{pane_id, status, detail?, exit_code?, at}` | The pane's status or its detail changed. `exit_code` is present exactly when `status` is `exited`; `at` is when plyd observed the change. An unchanged status and detail send nothing. |
| `pane.progress` | `{pane_id, progress?}` | The agent's plan changed; no `progress` hides the bar. At most 4 a second per pane; the last value of a burst is never dropped. |
| `pane.meta` | `{pane_id, model?, worktree?, cwd, branch?}` | What the session reports about itself changed. Absent fields are unknown. Claude Code's hooks, Codex's rollout and OSC 7 drive it; `branch` follows each directory change, from git (`docs/agents.md`). |
| `pane.exit` | `{pane_id, code, at}` | The pane's process ended, after its last output was published. `code` is 128 + signal for a signal death, -1 when plyd could not wait for it. Always preceded by `pane.status` `exited`. |
| `daemon.stopping` | `{kill_panes}` | plyd is about to exit; `kill_panes` says whether the processes are being stopped too. |

`pane.status` values are `starting`, `idle`, `running`, `waiting_permission`,
`waiting_input`, `exited` and `lost`; `docs/agents.md` has the machine behind
them.

## Records

Defined in `crates/proto/src/pane.rs`.

**`Pane`** — an open or finished pane.

| Field | Type | Meaning |
|---|---|---|
| `id` | u64 | The pane id (its `panes` row id); also the C2 `pane_id` a terminal view attaches with. An id that was ever announced is never reused. |
| `workspace_id` | u64 | |
| `tab_id` | u64 | |
| `position` | u32 | 0-based place in its tab: 0 is the main pane, then the stack from top to bottom. |
| `cli` | `"claude"` \| `"codex"` \| `"shell"` | |
| `cwd` | string | Working directory as last reported, absolute. |
| `title` | string | The terminal title when the program set one, else `claude`, `codex` or the shell's name. Title changes travel on C2 (TITLE), not as C1 events. |
| `status` | string | See **Events**. |
| `detail` | string, optional | One line on the status, such as the tool a permission prompt is for. |
| `progress` | `{done, total, current?}`, optional | Plan progress; `done ≤ total`, `current` is the in-progress item's text. |
| `model_seen` | string, optional | The model as the session reports it; ply never chooses it. |
| `worktree_seen` | string, optional | The worktree name the CLI reports (INV-7). |
| `branch` | string, optional | Git branch of `cwd`, display only. |
| `session_ref` | string, optional | The CLI's own session id (Claude `session_id`, Codex thread id), used by `pane.resume`. |
| `exit_code` | i32, optional | Present exactly when `status` is `exited`. |
| `created_at` | u64 | |
| `closed_at` | u64, optional | Absent while the pane is open. |
| `last_activity_at` | u64, optional | Last pty output (written at most every 5 s) or exit. |

**`Workspace`** — `{id, path, name, opened_at}`; `path` is absolute and unique.

**`Tab`** — `{id, name, position, pane_ids, focus_pane_id?, zoomed}`;
`pane_ids` in position order, the first being the main pane.

**`Layout`** — `{tabs, active_tab_id?}`.

**`Session`** — the stored record of a pane: `{pane_id, workspace_id, cli, cwd,
title, status, session_ref?, model_seen?, worktree_seen?, exit_code?,
created_at, closed_at?, last_activity_at?}`.

**`Settings`** — every field required on the wire:

| Field | Type | Default |
|---|---|---|
| `accent` | `"blue"` \| `"mint"` \| `"violet"` \| `"sand"` | `blue` |
| `option_as_meta` | `"off"` \| `"left"` \| `"right"` \| `"both"` | `off` |
| `keep_awake_while_running` | bool | `true` |
| `use_ply_colours_in_claude` | bool | `true` |
| `codex_plan_tool` | bool | `true` |
| `scrollback_lines` | u32 | 10 000 |
| `font_size` | f32, points | 12.5 |

`docs/configuration.md` says what each one does.

**`TerminalTheme`** (the `palette` of `theme.set`) — camelCase keys, colours as
`"#RRGGBB"` strings (either case read, upper case written):

| Field | Meaning |
|---|---|
| `ansi` | Exactly 16 colours, ANSI 0–15. Any other length is `bad_request`. |
| `fg`, `bg` | Default foreground and background (OSC 10 and 11). |
| `cursor` | Cursor colour (OSC 12). |
| `cursorText` | Text under a block cursor. |
| `selectionBg`, `selectionFg` | Selection fill, already blended over `bg`, and the text inside it. |

## Errors

`ErrorCode` in `crates/proto/src/control.rs` is closed; a new code is a
protocol change.

| Code | Meaning | Returned by |
|---|---|---|
| `cli_not_found` | The CLI is not on the login shell's `PATH`. | `pane.create`, `pane.resume` |
| `cli_too_old` | The CLI's installed version is below ply's minimum. | `pane.create`, `pane.resume` |
| `bad_request` | Malformed line, unknown field, invalid params, or a value the launch refuses. | any method; a request before `hello` |
| `unknown_method` | `m` names no method. | any request |
| `version_mismatch` | `hello.v` differs from plyd's; sent on id 0, then the connection closes. | the handshake |
| `not_found` | No such workspace, tab or pane. | methods naming one |
| `pane_alive` | `pane.close {kill:false}` on a live pane. | `pane.close` |
| `invalid_state` | The pane's state does not allow the request: `pane.answer` without a dialog, `pane.resume` on a pane that is not `lost`, a spawn with no palette after 5 s. | `pane.answer`, `pane.resume`, `pane.create` |
| `spawn_failed` | The process could not start: pty, exec, the pane files or a missing launch spec. | `pane.create`, `pane.resume` |
| `shutting_down` | plyd is stopping and takes no new work. | `pane.create`, `pane.resume` |
| `internal` | Anything else, such as a failed SQLite or `config.toml` write; the details are in plyd's log. | any method |

The app's client adds two codes of its own, never sent by plyd: `disconnected`
(the connection dropped, or the request was made while not connected) and
`timeout` (no answer in 10 s).

## What plyd guarantees

- **Responses in request order** on each connection.
- **Events in one order for everyone.** Events go through one broadcast
  channel, so every client sees them in the order plyd emitted them, and each
  change is stored before its event is sent.
- **An answer before its consequence where it matters.** When a response and an
  event are both ready, the connection's writer sends the response first (a
  biased select in `write_loop`). So `daemon.shutdown`'s `{}` always precedes
  `daemon.stopping`. `pane.create`'s response and its `pane.added` can arrive
  in either order: a client must accept either and treat them as one pane.
  Events plyd emits after it built a response can also reach the client before
  that response, so a pane in a response (`pane.create`, `pane.resume`,
  `pane.list`) may be older than the events already read: the app places a
  pane a response brings that it does not know yet, and of a pane it knows it
  takes only `cli` and `title` from a `pane.create` or `pane.resume` answer.
- **A slow client is dropped, not waited for.** A connection may fall 1 024
  events behind (`EVENT_CAPACITY`); one step further and plyd closes it, so the
  client reconnects and reloads rather than holding a gap.
- **Every connection is served the same.** There is no identity and no
  per-client state beyond the connection: any client may call any method and
  sees every event.

## The app's client

`app/src/ipc/control-client.ts`, driven by `app/src/state/effects.ts`, the only
module that calls it.

- **States.** `connecting`, then `connected` after `welcome`. `down` while plyd
  cannot be reached, with the reason and the next retry. `incompatible` when
  plyd answered `version_mismatch` or a `welcome` with another version; the grid
  shows the reason.
- **Another build.** At start the app reads its own build id,
  `<app/package.json version>+<commit>`, with `git rev-parse --short=12 HEAD`
  in its checkout (`readBuildId`, `app/src/ipc/os.ts`). While it is connected
  to a plyd whose `daemon_version` differs, the status bar says "plyd is from
  another build — Restart plyd" (`selectForeignDaemon`); with no id (git
  cannot tell) it says nothing.
- **Reconnect.** A failed connect or a closed connection retries after 100 ms,
  doubling to at most 2 s; a `welcome` resets the delay. The retries continue
  in `incompatible` too, until a compatible plyd answers.
- **Starting plyd.** The first failed connect of an outage calls
  `createDaemonStarter()` (`app/src/ipc/daemon-launcher.ts`) once, before the
  first retry: with `PLY_HOME` set it spawns the cargo-built `plyd --foreground`
  detached; without it, it runs `plyd install-agent`, which installs and
  kickstarts the LaunchAgent (`docs/configuration.md`). A start that fails is
  appended to the `down` reason.
- **Timeouts.** A `welcome` must arrive within 5 s of connecting, else the
  client closes and retries. A request with no answer in 10 s fails with
  `timeout`; a late answer is logged and dropped.
- **Drops.** Every pending request fails with `disconnected` when the
  connection goes. Events arriving before `welcome` are ignored.
- **Loading.** On every `connected` the effects run, in order: `settings.get`;
  `theme.set` with the palette for the stored accent (a failure is shown and the
  load goes on); `workspace.list`, taking the workspace whose path is the home
  directory, else the most recently opened one, else `workspace.open` of the
  home directory; then `layout.get` and `pane.list` together. A reconnect runs
  the same load, so nothing depends on events missed while away. Every pane
  event read from sending `pane.list` until the loaded session is in the store
  is applied again after it (the reductions are idempotent): one that arrives
  in the same socket read as the `pane.list` answer is applied before the
  answer's promise resumes, and the older list would otherwise replace it.
- **Saving.** Layout changes (tab order, focus, zoom, active tab) are sent with
  `layout.save` 250 ms after the last one; settings changes with `settings.set`
  300 ms after the last one.

`app/src/ipc/mock-server.ts` is a C1 server for development and tests only. It
answers every method from memory, refuses to start without `PLY_HOME`, and is
never imported by `main.tsx`.

## A worked example

A fresh plyd, the app connecting, a shell pane opened, the shell told to
`exit 3` (over C2), and the finished pane closed. Lines are shortened with `…`
where the whole object is shown elsewhere.

```
→ {"t":"hello","v":1,"client":"ply-app","app_version":"0.1.0"}
← {"t":"welcome","v":1,"daemon_version":"0.1.0+0a1b2c3d4e5f"}
→ {"t":"req","id":1,"m":"settings.get","p":{}}
← {"t":"res","id":1,"ok":true,"r":{"accent":"blue","option_as_meta":"off","keep_awake_while_running":true,"use_ply_colours_in_claude":true,"codex_plan_tool":true,"scrollback_lines":10000,"font_size":12.5}}
→ {"t":"req","id":2,"m":"theme.set","p":{"palette":{"ansi":[…16 colours…],"fg":…,"bg":…,"cursor":…,"cursorText":…,"selectionBg":…,"selectionFg":…}}}
← {"t":"res","id":2,"ok":true,"r":{}}
→ {"t":"req","id":3,"m":"workspace.list","p":{}}
← {"t":"res","id":3,"ok":true,"r":[{"id":1,"path":"/Users/example","name":"example","opened_at":1790270000}]}
→ {"t":"req","id":4,"m":"layout.get","p":{"workspace_id":1}}
→ {"t":"req","id":5,"m":"pane.list","p":{"workspace_id":1}}
← {"t":"res","id":4,"ok":true,"r":{"tabs":[]}}
← {"t":"res","id":5,"ok":true,"r":[]}
→ {"t":"req","id":6,"m":"pane.create","p":{"workspace_id":1,"cli":"shell","cwd":"/Users/example/project"}}
← {"t":"evt","e":"pane.added","p":{"id":1,"workspace_id":1,"tab_id":1,"position":0,"cli":"shell","cwd":"/Users/example/project","title":"zsh","status":"idle","created_at":1790276000}}
← {"t":"res","id":6,"ok":true,"r":{"id":1,"workspace_id":1,"tab_id":1,"position":0,"cli":"shell",…}}
   (the terminal view attaches to pane 1 over C2; the user types `exit 3`)
← {"t":"evt","e":"pane.status","p":{"pane_id":1,"status":"exited","exit_code":3,"at":1790276060}}
← {"t":"evt","e":"pane.exit","p":{"pane_id":1,"code":3,"at":1790276060}}
→ {"t":"req","id":7,"m":"pane.close","p":{"pane_id":1,"kill":false}}
← {"t":"evt","e":"pane.removed","p":{"pane_id":1}}
← {"t":"res","id":7,"ok":true,"r":{}}
→ {"t":"req","id":8,"m":"session.list","p":{"workspace_id":1,"include_closed":true}}
← {"t":"res","id":8,"ok":true,"r":[{"pane_id":1,"workspace_id":1,"cli":"shell","cwd":"/Users/example/project","title":"zsh","status":"exited","exit_code":3,"created_at":1790276000,"closed_at":1790276075,"last_activity_at":1790276060}]}
```

Two details the example shows. `layout.get` and `pane.list` were sent together
and answered in request order. `pane.added` came before the `pane.create`
response here, and `pane.removed` before the `pane.close` response; both orders
are allowed for those two pairs.

## Tests

- `crates/proto/tests/golden.rs` with one golden file per message in
  `crates/proto/tests/golden/c1/`: `c1_goldens_round_trip_both_ways`,
  `c1_goldens_cover_every_method_and_event`,
  `c1_unknown_fields_are_rejected_everywhere`,
  `c1_unknown_method_and_bad_params_keep_the_id`,
  `c1_invalid_values_are_rejected`, `c1_line_cap_is_enforced`,
  `c1_version_mismatch_is_detectable`, `c1_types_carry_no_pty_bytes`.
- `crates/daemon/tests/lifecycle.rs` against a real plyd:
  `handshakes_check_versions_and_panes`,
  `a_live_pane_closes_only_with_kill_and_its_session_is_kept`,
  `an_exiting_shell_reports_its_code_on_c1_and_c2`,
  `a_restart_reopens_a_shell_by_itself_and_keeps_the_settings`,
  `a_second_plyd_refuses_to_start`.
- `crates/daemon/src/panes/registry.rs` unit tests (layout, closing, event
  order) and `crates/daemon/src/server/control.rs`
  (`lines_are_capped_and_split_at_newlines`).
- `app/src/ipc/control-client.test.ts` (handshake, errors, reconnect, starting
  plyd once per outage, incompatibility) and `app/src/state/effects.test.ts`
  (the load order, an event in the same read as the `pane.list` answer, a
  failed `theme.set`, the saves) against the mock server, and
  `app/src/state/reducer.test.ts` (a `pane.create` answer that events
  overtook).
