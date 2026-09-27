# Agents

How ply runs Claude Code and Codex and learns what they are doing. ply starts
the user's own `claude` and `codex`, found on the login shell's `PATH`, in a pty
like any terminal would, and adds per-invocation options so the CLIs report
back. It never calls a model, holds a key, rewrites a prompt, picks a model or
effort level, or reads a Claude Code transcript. Everything it knows about a
session comes from four sources the CLIs produce themselves:

| Source | CLI | Path into plyd |
|---|---|---|
| hooks (C3) | Claude Code | the CLI runs `ply-hook claude <Event>`, which writes one line to `run/hook.sock` |
| notify (C3) | Codex | the CLI runs `ply-hook codex <json>` after each turn |
| OSC 9 (C8) | Codex | a desktop notification in the pty stream, read by libghostty-vt's notification callback |
| rollouts (C4) | Codex | the session's JSONL file under `$CODEX_HOME/sessions/`, tailed by offset |

`crates/agents` (`ply-agents`) turns each of these into signals and does no I/O
beyond reading the files it is handed. `crates/hook` (`ply-hook`) is the
forwarder. plyd (`crates/daemon`) launches the CLIs and owns the status machine.

## How plyd feeds them

Each agent process has one `AgentSession` (`ply_agents::Adapter::new_session`)
and one status machine, owned by its pane's task
(`crates/daemon/src/panes/agent.rs`) and prepared from the launch spec before
the process spawns, so a hook the CLI fires at once already finds them:

| File | Owns |
|---|---|
| `crates/daemon/src/server/hooks.rs` | the C3 server on `run/hook.sock`: one line per `ply-hook` run (1 MiB, 2 s), decoded strictly and handed to the pane's task; an envelope for an unknown pane, a shell or the other CLI is logged and dropped, and nothing is ever answered |
| `crates/daemon/src/panes/state.rs` | the status machine below, and the `pane.progress` rate limit |
| `crates/daemon/src/tail.rs` | C4: finding and tailing a Codex pane's rollout |
| `crates/daemon/src/osc.rs` | OSC 7 paths, OSC 9 bodies, and which input counts as a key typed |
| `crates/daemon/src/branch.rs` | the branch label |

The pane task hands the session every `AgentEvent`
(`crates/agents/src/adapter.rs`): `Hook` for each C3 envelope, `RolloutLine`
for each tailed line (`RolloutHistory` for a resumed thread's past, see
**Rollouts**), `Osc9` for each OSC 9 body the engine reports (OSC 777
carries a title and is some other program's), `FirstOutput` for the process's
first pty byte, and `KeyTyped` for every key typed — a KEY frame that encoded to
bytes, INPUT_RAW, or a `pane.answer` (`1` or ESC) — with `enter` when those bytes
submit a line (a carriage return, or `CSI 13 u` in the kitty protocol). It
applies the `AdapterSignal`s it gets back: status signals to the machine
(`pane.status`), `Progress` through the rate limit (`pane.progress`), `Meta` to
the pane's row (`pane.meta`), and `FindRollout` to the tailer.

## Launching a pane

`pane.create` (`crates/daemon/src/panes/launch.rs`) does the same for every
CLI:

1. **The program.** `claude` or `codex` is the first executable of that name on
   the login shell's `PATH`, which plyd reads once at startup from an
   interactive login shell, `$SHELL -l -i -c`, falling back to `$SHELL -l -c`
   (`crates/daemon/src/login.rs`), because launchd gives plyd a
   minimal environment. Not found is `cli_not_found`. A shell pane runs the
   login shell as `<shell> -l`.
2. **The version**, read without running the CLI (see **The version check**).
3. **The pane directory**, `run/panes/<id>/`, mode `0700`, holding the
   adapter's files (mode `0600`) and `launch.json`.
4. **The terminal**, created with the palette and sized to the last view size a
   client attached with, before the child exists: Codex probes the terminal
   within 250 ms of starting (`CSI 6n`, OSC 10 and 11, `CSI ?u`, DA1) and
   libghostty-vt answers from the palette (`docs/terminal.md`).
5. **The process**, on a new pty with a cleared environment.

**The environment** of every pane is exactly: `HOME`, `USER`, `LOGNAME`,
`TMPDIR`, `LANG`, the `LC_*` locale variables, `SSH_AUTH_SOCK` and
`__CF_USER_TEXT_ENCODING` from plyd's own environment when set; `LANG` as
`en_US.UTF-8` when plyd has none; `SHELL` (the resolved login shell); `PATH`
(the login shell's); the variables of Ruling R52 that the login shell exports
— `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, the proxy variables in both cases,
`SSL_CERT_FILE`, `SSL_CERT_DIR`, `NODE_EXTRA_CA_CERTS`, `LANG`, `LC_ALL`,
`LC_CTYPE` (`docs/configuration.md`) and never another, so no API key an rc
file exports; `TERM=xterm-256color` and `COLORTERM=truecolor`; and the launch
spec's additions below. Nothing else of plyd's leaks through
(`a_pane_gets_only_plyds_allow_listed_environment`,
`the_probe_passes_on_only_the_captured_variables_the_shell_exports`). A Codex
pane's rollouts are followed under the `CODEX_HOME` it runs with
(`codex_home_from_the_login_shell_reaches_the_pane_and_its_rollout_tailer`).

**`launch.json`** is the spawn as stored for `pane.resume` (`LaunchSpec` in
`crates/agents/src/adapter.rs`, strict JSON):

| Field | Meaning |
|---|---|
| `cli` | `claude`, `codex` or `shell` |
| `argv` | the full argv, program first; exec'd without a shell |
| `env` | the variables added to the base environment |
| `cwd` | the absolute working directory |
| `worktree` | the worktree option as requested, if any |
| `resume` | the session id this spawn resumed, if any |

Values that would read as options are refused before anything runs: a worktree
name or session id that is empty, contains a NUL or starts with `-` is
`bad_request`, and a prompt that starts with `-` is preceded by `--` (both CLIs
end option parsing there).

## Claude Code

```
claude --settings <run/panes/<id>/claude-settings.json> [--worktree <name>] [--resume <session_id>] [--] [prompt]
env: PLY_PANE_ID=<id>  PLY_HOOK_SOCK=<run/hook.sock>  TERM=xterm-256color  COLORTERM=truecolor
     CLAUDE_CODE_FORCE_SYNC_OUTPUT=1
```

`crates/agents/src/claude/mod.rs` builds it. `PLY_PANE_ID` and `PLY_HOOK_SOCK`
reach the hooks because Claude Code runs hook commands with its own
environment. `CLAUDE_CODE_FORCE_SYNC_OUTPUT=1` asks for DEC 2026 synchronized
output, which plyd honours (`docs/screen-protocol.md`). ply never passes
`--model`: a session starts on the user's own default, and the user changes
model and effort inside the pane.

**`claude-settings.json`** (`crates/agents/src/claude/settings.rs`). A
`--settings` file merges with the user's settings and wins over them, so it
holds only `hooks`, the `statusLine`, and `"theme":"dark-ansi"` when "Use ply
colours in Claude Code" is on. The user's own model, permissions and hooks stay
in force, and so does their status line: ply's `statusLine` runs it. Each hook
is one command:

```json
{
  "hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": "'<plyd dir>/ply-hook' claude SessionStart"}]}],
    …
  },
  "statusLine": {"type": "command", "command": "'<plyd dir>/ply-hook' statusline '~/.claude/statusline.sh'", "refreshInterval": 2},
  "theme": "dark-ansi"
}
```

**The status line.** plyd reads the user's own `statusLine` when it starts the
pane, as Claude Code would pick it without ply: the project's
`.claude/settings.local.json`, then its `.claude/settings.json`, then
`settings.json` in `$CLAUDE_CONFIG_DIR` or `~/.claude`. A command status line's
command becomes the argument of `ply-hook statusline`, and its other keys
(`refreshInterval`, `padding`) are copied; without one, `ply-hook statusline`
stands alone and prints nothing. Each run sends the payload Claude Code pipes in
to plyd as a `StatusLine` envelope, for the plan usage in its `rate_limits`
(see **Plan usage**), then runs the user's command through `/bin/sh -c` with
the same payload and prints what it prints, exiting with its code. plyd takes a
`StatusLine` envelope as plan usage only: it never reaches the pane, so a status
line that refreshes every few seconds cannot hold the R17 quiet timer open
(`a_status_line_report_is_live_usage_and_never_pane_activity`). A setting
changed after the pane started applies from the pane's next start.

The path of `ply-hook` is single-quoted for `sh`, which Claude Code runs hook
commands with. `ply-hook` is the binary beside the running plyd (build both:
`cargo build -p ply-daemon -p ply-hook`); plyd warns at startup when it is not
there, and the hook commands then fail without reaching plyd.

**The hooks registered**, all as `ply-hook claude <Event>` (`HOOK_EVENTS`):
SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure,
PermissionRequest, PermissionDenied, Notification, Stop, StopFailure,
SessionEnd, CwdChanged.

**Never WorktreeCreate or WorktreeRemove.** Registering WorktreeCreate makes
Claude Code hand `git worktree add` to the hook: the hook would then create the
worktree, which is ply managing worktrees (INV-7) and a hook deciding for the
CLI (INV-14). ply passes `--worktree <name>` and learns the worktree from the
working directory the other hooks report instead.
`claude_settings_contain_only_hooks_and_the_theme` asserts both lists.

**Facts verified against Claude Code 2.1.282** (the minimum ply accepts):

- Every payload carries `session_id`, `transcript_path`, `cwd`,
  `permission_mode` and `hook_event_name`; SessionStart carries `model`.
- `Notification.notification_type` is `permission_prompt` or `idle_prompt`.
  Claude sends `idle_prompt` after about 60 s at its prompt, so it changes no
  status: the pane stays `idle`, "your turn" (Ruling R46). `permission_prompt`
  asks for permission (it follows the PermissionRequest hook, which has already
  done so, and stands in for it if that hook got lost); any other type asks for
  input.
- No PermissionRequest hook, silent or deciding, changes the dialog, and the
  user's own hooks still run beside ply's. A manual deny fires no hook,
  sometimes not even Stop.
- The permission dialog numbers its options, and their list varies: 2.1.282 in
  one run offered 1 Yes, 2 always allow for the directory, 3 "Yes, and switch
  to auto mode", 4 No, so a positional "3 is No" approved. `pane.answer` is
  therefore by meaning (Ruling R55): Yes types `1`, the first option, "Yes" in
  every Claude Code and Codex dialog seen, and No types ESC, which cancels the
  dialog in both TUIs and can never approve.
- There is neither a TodoWrite nor a Task tool, so progress stays hidden.

## ply-hook

`crates/hook/src/main.rs`, depending on std and serde_json only.

```
ply-hook claude <Event>     the payload is the hook's JSON on stdin
ply-hook codex <json>       the payload is the last argument (Codex's notify)
```

It reads `PLY_PANE_ID` and `PLY_HOOK_SOCK`, wraps the payload verbatim in the
C3 envelope and writes it as one line to the socket:

```
{"v":1,"pane_id":2,"cli":"claude","event":"Notification","payload":{…the CLI's JSON, byte for byte…}}
```

`event` is absent for Codex. `ply_proto::hook::HookEnvelope` is the decoder and
the contract; `ply-hook` builds the line by hand so as not to link it.

Its guarantees, all tested in `crates/hook/tests/hook.rs`:

- **It never blocks the CLI** (INV-12). The work runs on a worker thread and
  the process exits 200 ms after it started, whatever the worker is doing —
  stdin never closed, a socket nobody accepts on, a write that blocks. With
  plyd down it exits in a few milliseconds.
- **It never decides for the CLI** (INV-14). It writes nothing to stdout or
  stderr (the panic hook is silenced) and exits 0 on every path.
- **It forwards or drops, never alters.** The payload must be one JSON value;
  raw CR and LF (whitespace in JSON) become spaces so the line stays one line,
  and nothing else changes. A payload over 1 MiB − 4 KiB (`MAX_PAYLOAD_BYTES`)
  is drained from stdin and dropped. An unknown CLI name, an event name over
  64 bytes, a missing or unparsable `PLY_PANE_ID`, a missing `PLY_HOOK_SOCK` or
  a payload that is not JSON sends nothing.

A dropped payload is a missing signal, not an error the CLI sees; the status
machine has fallbacks for exactly that.

## Codex

```
codex -c 'notify=["<plyd dir>/ply-hook","codex"]'
      -c 'tui.notification_method="osc9"'
      -c 'tui.notification_condition="always"'
      -c 'tools.update_plan.enabled=true'
      [resume <thread_uuid>] [--] [prompt]
env: PLY_PANE_ID=<id>  PLY_HOOK_SOCK=<run/hook.sock>  TERM=xterm-256color  COLORTERM=truecolor
```

`crates/agents/src/codex/mod.rs` builds it; `-c` values are TOML, and the
`notify` array's strings are escaped as TOML basic strings (`toml_string`).
These four overrides are the only options ply passes (`launch.rs` asserts the
exact argv):

| Override | Why |
|---|---|
| `notify=[…]` | Codex runs `ply-hook codex <json>` after each completed turn. |
| `tui.notification_method="osc9"` | Notifications arrive as OSC 9 in the pty, where plyd reads them. |
| `tui.notification_condition="always"` | Notify while the pane has focus too; Codex's default is only while unfocused. |
| `tools.update_plan.enabled=true` | The plan tool is off by default in 0.156.1; without it progress has no source. The setting `codex_plan_tool` (default on) leaves this override out. |

Any `-c` makes the Codex TUI run embedded instead of attaching to Codex's
shared background daemon, which is fine: plyd keeps the process alive. Codex
has no named worktree option, so a `worktree` in `pane.create` is `bad_request`
for Codex; open the pane in the worktree's directory instead. `resume` takes a
thread id and only the hyphenated 8-4-4-4-12 hex form is accepted.

Codex's TUI does not use the alternate screen or mouse reporting by default; it
enables bracketed paste and focus events.

### Notify

The JSON Codex appends to the notify argv (`crates/agents/src/codex/notify.rs`)
has kebab-case keys:

| Key | Meaning |
|---|---|
| `type` | `agent-turn-complete`, the only type 0.156.1 sends; any other is counted and ignored |
| `thread-id` | the thread the turn ran in; must not be empty |
| `turn-id` | the completed turn |
| `cwd` | the session's working directory |
| `client` | the front end, when Codex reports one |
| `input-messages` | the user messages of the turn |
| `last-assistant-message` | the assistant's final message, or `null` |

Unknown keys are ignored so a newer Codex still parses. Notify fires after each
completed turn and not when a turn is aborted.

**A new thread rebinds the pane** (Ruling R49). `/new` and `/clear` start a new
thread with its own rollout. The pane switches to it when a notify names a thread
whose rollout exists (the title turn's never does), or when, within 5 s of an
Enter, a new unclaimed rollout of the pane's directory appears; the session then
binds to the new thread, `session_ref` follows it (so `pane.resume` resumes the
current conversation), and the old plan's progress is dropped
(`a_new_thread_rebinds_the_session_when_plyd_switches_files_or_a_notify_named_it`,
`codex_a_new_thread_rebinds_the_pane_and_resume_follows_it`).

**The first notify can lie.** Codex runs a title-generation micro-turn in its
own thread, and its notify can arrive first, while the real turn still runs.
So a pane binds to a thread only once a rollout file with that thread id
exists (Ruling R28): a notify for the bound thread completes the turn, a notify
for another thread does nothing, and a notify that arrives before any thread is
bound asks plyd to look for `rollout-*-<thread-id>.jsonl`
(`AdapterSignal::FindRollout`) and completes the turn only when that thread's
`session_meta` binds the pane. Every thread notified before the binding is
remembered (up to `PENDING_TURNS`, 8; older ones are forgotten and counted as
`forgotten_turns`), so the title turn's notify cannot hide the real one's
(`title_turn_notify_is_ignored_and_the_real_thread_binds`,
`a_notify_before_its_rollout_completes_the_turn_once_bound`,
`a_second_notify_before_binding_keeps_the_first_threads_turn`).

### OSC 9

With the two notification overrides, every Codex notification arrives as
`OSC 9 ; <body>` in the pty stream. libghostty-vt delivers the body through its
desktop-notification callback; plyd passes it to the pane's session as
`AgentEvent::Osc9`, and `classify_osc9` (`crates/agents/src/codex/osc9.rs`)
checks these fixed prefixes, taken from Codex 0.156.1's
`codex-rs/tui/src/chatwidget/notifications.rs`:

| Body starts with | Codex notification | Signal | Status |
|---|---|---|---|
| `Approval requested: ` | exec approval | `PermissionRequested` | `waiting_permission` |
| `Codex wants to edit ` | edit approval (`<path>` or `<n> files`) | `PermissionRequested` | `waiting_permission` |
| `Approval requested by ` | MCP elicitation (`<server>`) | `PermissionRequested` | `waiting_permission` |
| `Plan mode prompt: ` | plan-mode or tool question prompt | `InputRequested` | `waiting_input` |
| `Question: ` | asynchronous question | `InputRequested` | `waiting_input` |
| anything else | the assistant's final text, or `Agent turn complete` | `TurnComplete` | `idle` |

The body travels as the signal's `detail`, which the pane header shows. The
prefixes do not overlap, and an unknown body is a completed turn, never an
error (Ruling R27). Approval requests are never written to the rollout, so OSC 9
is their only source.

libghostty-vt parses some OSC 9 bodies as ConEmu commands: a body that begins
with `5` or `12`, or with a ConEmu sub-command number and a semicolon (`1;`
to `12;`), never reaches the callback. None of the prefixes above does.
`the_observed_bodies_classify_as_recorded` checks the table against the bodies a
real session produced.

### Rollouts

Codex writes each session to
`$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<YYYY-MM-DDThh-mm-ss>-<thread_uuid>.jsonl`
(`CODEX_HOME` defaults to `~/.codex`), one `{timestamp, ordinal, type, payload}`
record per line (`crates/agents/src/codex/rollout.rs`):

| Record `type` | What ply takes from it |
|---|---|
| `session_meta` | The first record: the thread (`payload.id`, else `session_id`) and `cwd`. The first one fed to a pane binds it. |
| `turn_context` | `model` and `cwd` of the turn. |
| `response_item` | An `update_plan` call, in either shape (see **Progress**). |
| `event_msg` | `task_started`, `task_complete` and `turn_aborted` start and end turns (R48); `item_completed`, `token_count` and `thread_settings_applied` are known and ignored; any other subtype is skipped and counted (`unknown_rollout_records`). |
| `inter_agent_communication`, `inter_agent_communication_metadata`, `compacted`, `token_usage_record`, `world_state`, `retained_context`, `security_risk_score`, `realtime_item` | Nothing; known and ignored. |
| anything else | Skipped and counted (`unknown_rollout_records`). |

A line that is not a `{type, payload}` record is an error and is counted
(`malformed_rollout_lines`); the session's state does not change. `LineBuffer`
frames tailed bytes into lines in any chunking and skips a line over 8 MiB
without buffering it (inline images can be that large).

**Finding a pane's rollout.** Before any notify, the rollout is the newest file
created at or after the pane's spawn whose `session_meta.cwd` equals the pane's
working directory (`pick_rollout_by_cwd`). After a notify, it is the file whose
name carries that notify's thread id (`rollout_thread_id`). A resumed pane
follows only its own thread's file, wherever it is under `sessions/`, and two
panes never follow the same file. `tail.rs` runs one std thread per Codex
process: it wakes on FSEvents for `sessions/` once the directory exists (plyd
never creates it; a failed watch is retried every 30 s) and polls besides, every
250 ms until it follows a file and every second after; it reads the file from
the start by offset, again from the start when the file shrinks or is replaced,
and switches files for a new thread (above). Requests from the pane (a notify's
thread, the Enter window) sit in a shared list, so filesystem events cannot
crowd them out. It only ever reads under `$CODEX_HOME`.

**A resumed thread's past is history.** What a file already holds when the
tailer binds it, up to the first end of file, is the thread's past when the
file is the resumed thread's or was created before the process (and so is a
file read again from its start): the tailer sends those lines as history
(`TailMsg::History`, `AgentEvent::RolloutHistory`), and the session takes the
binding, the model, the directory and the plan from them but no turn. So a
resumed pane does not replay every `task_started` and `task_complete` it ever
had through the status machine, and a rollout whose last turn never completed
(a pane lost mid-turn) resumes `idle`, not `running` for good with the
keep-awake assertion held
(`a_codex_pane_lost_mid_turn_resumes_idle_without_replaying_its_past_turns`,
`a_resumed_threads_history_binds_and_reports_but_starts_and_ends_no_turn`).

**Delivery never delays the terminal** (I4). The tailer drops, on its own
thread, every line the session cannot use (`plyd_reads`: only `session_meta`,
`turn_context`, the turn `event_msg`s, `response_item`s mentioning
`update_plan`, and unknown or malformed lines, which are counted), and sends the
rest in batches of at most 256 lines and 1 MiB over a channel of 4 batches. The
pane task reads that channel last, only when no command, exit, input write, pty
output or timer is ready, so a long replay cannot hold back pty output or the
answers to Codex's startup probes.

## Progress

Progress is the agent's own plan, never ply's guess: `{done, total, current?}`
with `done` the completed items, `total` all items and `current` the text of
the first item in progress (`crates/agents/src/plan.rs`). An empty plan, or no
plan source yet, hides the bar. plyd sends `pane.progress` at most 4 times a
second per pane: a change after a quiet 250 ms goes out at once, and the last
value of a burst when the interval ends. Progress is kept in memory only (schema
v1 has no column for it); `pane.list` carries it.

**Claude Code** (`crates/agents/src/claude/progress.rs`), from PostToolUse
payloads only:

- `TodoWrite`: `tool_input.todos` is the whole list, each item's text from
  `content` (else `activeForm`, `subject`) and `status` `pending`,
  `in_progress` or `completed`.
- `TaskCreate` adds a task whose id comes from `tool_response` (`task.id`,
  `taskId`, `task_id`, `id`, or the number in a `"Task #N"` text; else the next
  number) and whose text is `subject`, `title`, `content` or `description`.
- `TaskUpdate` changes the task named by `taskId` (or `task_id`, `id`); status
  `deleted` removes it.
- The source used last wins. A call whose input cannot be read keeps the plan
  and is counted (`unreadable_progress`).

Claude Code 2.1.282 has neither tool, so its panes show no progress; the
parsers stay for the versions that do.

**Codex**: the latest `update_plan` call in the rollout, in either shape:

- a `response_item` of type `function_call` named `update_plan`, whose
  `arguments` string is JSON `{explanation?, plan: [{step, status}]}`;
- in code mode, a `custom_tool_call` named `exec` whose JavaScript `input`
  calls `tools.update_plan({…})`. A small reader
  (`crates/agents/src/codex/literal.rs`) takes the object literal: unquoted
  keys, `'`, `"` and backtick strings, trailing commas, comments (also between
  `tools.update_plan` and its `(`), `undefined`, nesting to depth 64. Of
  several calls in one input the last wins; a call that
  passes no literal (`tools.update_plan(plan)`) is an error.

An unknown status counts as `pending`.

## Model, worktree, directory and branch

`SessionMeta` (`crates/agents/src/meta.rs`) is what the session reports, and
nothing else:

| Fact | Claude Code | Codex |
|---|---|---|
| `session_ref` | `session_id` of the first hook payload; a later SessionStart replaces it | the bound thread id |
| `model` | `SessionStart.model` | `turn_context.model` |
| `cwd` | `new_cwd` of CwdChanged, else the payload's `cwd` | `session_meta.cwd`, then `turn_context.cwd` |
| `worktree` | derived from `cwd` | derived from `cwd` |

The worktree label is `<name>` when the working directory is at or below
`<repo>/.claude/worktrees/<name>` (the innermost such directory), else none.
It is display and session record only: `worktree_seen` is the only worktree
column ply keeps (INV-7). The live label (in `pane.meta` and `pane.list`)
clears when the pane leaves the worktree; the stored session record keeps the
last worktree the pane was in. The model is shown exactly as reported; with none
reported the pane header shows only the CLI name.

The git labels come from two git runs in the pane's directory
(`crates/daemon/src/branch.rs`): `git rev-parse --show-toplevel
--absolute-git-dir --git-common-dir`, then `git symbolic-ref --short -q HEAD`
(`git rev-parse --short HEAD` for a detached `HEAD`), each on its own task with
the pane's base environment, stdin from `/dev/null` and a 2 s limit. They run at
spawn, whenever the directory changes, and whenever the repository's `HEAD` file
changes. plyd looks at each pane's `HEAD` once a second, a file stat and no git
run, so a `git switch` in the pane or anywhere else shows within a second.

- `branch`: the branch checked out, or the short commit of a detached `HEAD`.
- `project`: the folder name of the repository, of the main one when the
  directory is in a linked worktree (its `--git-common-dir`); outside a
  repository, the directory's own folder name.
- `git_worktree`: the folder name of the linked worktree the directory is in,
  whatever its path; the header shows it when the CLI reports no worktree.

Outside a repository, without git or on any failure there is no branch and no
worktree. All three are display only: they travel in `pane.meta` and
`pane.list` and are never stored.

`session_ref`, `model_seen`, `worktree_seen` and `cwd` are stored in the pane's
row and returned by `pane.list` and `session.list`.

## The status machine

A pane's status is one of `starting`, `idle`, `running`, `waiting_permission`,
`waiting_input`, `exited` and `lost`. plyd owns the machine; the adapters only
name signals (`StatusSignal` in `crates/agents/src/adapter.rs`), and both CLIs
use the same vocabulary. The table lives in
`crates/daemon/src/panes/state.rs`:

| From | Signal | Source | To |
|---|---|---|---|
| — | spawn | `pane.create`, `pane.resume` | `starting` (a shell pane: `idle`) |
| any live status | `Ready` | Claude SessionStart (not the one after a compaction) · Codex's first pty output byte | `idle` |
| `idle`, `waiting_input` | `PromptSubmitted` | Claude UserPromptSubmit · Enter typed in a Codex pane | `running` |
| `starting`, `idle`, `running` | `TurnStarted` | Codex rollout `event_msg` `task_started` (R48) | `running` |
| `running` (Codex) | `NoTurnStarted` | plyd: an Enter that made the pane `running` saw no `task_started` within 3 s (R48) | `idle` |
| any live status but `waiting_permission` | `ToolUse` | Claude PreToolUse or PostToolUse | `running` |
| `running` (Codex also `idle`, R48) | `PermissionRequested` | Claude PermissionRequest or a `permission_prompt` Notification · Codex OSC 9 approval | `waiting_permission` |
| `running`, `idle` | `InputRequested` | Codex OSC 9 question or plan prompt · a Claude Notification that is neither `permission_prompt` nor `idle_prompt` (R46) | `waiting_input` |
| `waiting_permission` | `CallSettled`, same call | Claude PostToolUse, PostToolUseFailure or PermissionDenied | `running` |
| `waiting_permission`, `waiting_input` | `KeyTyped` | any key typed in the pane | `running` |
| `running` (Claude also `waiting_permission` without a pending call) | `TurnComplete` | Claude Stop or StopFailure · Codex notify for the bound thread · Codex OSC 9 of any other body · Codex rollout `task_complete` or `turn_aborted` for the current turn | `idle` |
| `running` (Claude) | `QuietTimeout` | the pty silent and no hook for 5 s | `idle` |
| any | the process exits | pty EOF | `exited(code)` |
| any live status, after a plyd restart | the process is gone | plyd's start | `lost` |

Transitions not in the table are ignored and counted.

- **"Same call."** Claude's PermissionRequest carries no `tool_use_id`, so a
  call is matched by id when both reports have one, else by tool name and input
  (`ToolCall::same_call`).
- **The fallbacks.** A manual deny fires no hook, so any key typed in a waiting
  pane moves it to `running`; and a Claude pane `running` with a silent pty and
  no hook for 5 s (`Adapter::quiet_timeout`) becomes `idle`. Codex has no quiet
  timeout.
- **Detail.** `PermissionRequested` carries the tool name (Claude) or the OSC 9
  body (Codex); `InputRequested` the notification's message or body. The detail
  travels in `pane.status`.
- **A late permission prompt does not outlive its turn.** Claude's
  `permission_prompt` Notification can arrive after the call it asked about
  settled, putting the pane back into `waiting_permission` with no pending call;
  the turn's Stop then still makes it `idle`. A wait whose call is still pending
  ignores `TurnComplete`, and so does every Codex approval (an OSC 9 approval
  has no call, and the previous turn's `task_complete` can be read from the
  rollout after it) (`a_turn_ends_a_wait_for_permission_that_has_no_pending_call`,
  `a_codex_approval_outlives_a_lagging_turn_end`; Ruling R54).

- **SessionEnd** changes nothing (Ruling R47). Claude fires it for `/clear` and
  an in-session `/resume` while the process keeps running, so it is logged and
  the status stays; the SessionStart that follows updates the session id (which
  `pane.resume` uses) and makes the pane `idle`. A SessionStart after a
  compaction continues the session and changes no status. Only the process's
  exit ends the machine: the pane shows `exited(code)` when the process is
  reaped (`pane.status`, then `pane.exit`, after its last output). `lost` is set
  when plyd starts (`Registry::load`), never by a signal.
- **The quiet timer** runs from the latest of the last pty output, the last hook
  and the moment the pane entered `running`. A timeout the machine does not act
  on is restarted rather than left due, so a pane task never wakes in a loop.
- **Codex turns come from the rollout** (Ruling R48). `task_started` makes the
  pane `running`, `task_complete` and `turn_aborted` (Esc) make it `idle`; a
  `task_complete` of an older turn than the last one started is ignored, and
  the turns of a resumed thread's history count for nothing (see
  **Rollouts**). Enter
  typed in an `idle` Codex pane is a fast path to `running` that the rollout
  must confirm: without a `task_started` within 3 s (`TURN_START_WAIT`, an
  Enter on an empty composer) the pane is `idle` again. OSC 9 approvals and
  questions are accepted from `idle` too, since they only happen inside a
  turn, which covers a pane started with a first prompt, where no Enter was
  typed.
- The pane's task publishes the machine's state when it adopts the process; the
  keep-awake assertion follows `running` panes.

**Presentation** (`app/src/state/selectors.ts`): "your turn" is `idle`; "done"
is `idle` with every plan item complete; "needs you" is `waiting_permission` or
`waiting_input`, which ⌘J cycles through across tabs; `exited` shows its code;
a shell pane shows the shell's name while it runs.

## The version check

Claude Code must be at least 2.1.282 and Codex at least 0.156.1, the versions
the integration was verified against; below that `pane.create` answers
`cli_too_old`.

plyd reads the version from the install's own files and never executes the CLI
for it: running `codex --version`, `codex --help` or `codex doctor` once started
Codex's self-updater on a machine. `Adapter::installed_version`
(`crates/agents/src/install.rs`, `claude/mod.rs`, `codex/mod.rs`) resolves
symlinks and reads, in order:

| CLI | Where the version is read |
|---|---|
| Claude Code | the native installer's `…/claude/versions/<version>`; npm's `@anthropic-ai/claude-code/package.json` beside the binary or one level up; Homebrew's `Caskroom/claude-code/<version>/` |
| Codex | the standalone install's `<pkg>/bin/codex` with `<pkg>/codex-package.json`; npm's `@openai/codex/package.json`; Homebrew's `Caskroom/codex/<version>/` or `Cellar/codex/<version>/` |

When none of them is there the version is unknown, plyd logs a warning and
launches anyway. The adapters also name a `--version` probe
(`version_probe_args`) as a last resort, but plyd does not run it.

## Plan usage

Holding ⌘U shows each CLI's plan usage (`usage.get`, Ruling R59). ply computes
none of it and asks no server: it shows what the CLIs themselves last reported,
with its age, and reads their files without ever writing them
(`crates/daemon/src/usage.rs`; the parsers are
`crates/agents/src/claude/usage.rs` and `crates/agents/src/codex/usage.rs`).

- **Claude Code, live:** the status line payload of every Claude Code session
  carries `rate_limits: {five_hour, seven_day, …}`, each window
  `{used_percentage, resets_at (Unix seconds)}`, which Claude Code updates from
  every API response of the session (absent for an API-key login, and until the
  session's first response). Claude panes report it through their
  `statusLine`; a session outside ply reports the same way when its own status
  line pipes its input to `ply-hook statusline`, which then sends it as pane 0
  to `PLY_HOOK_SOCK`, else to `run/hook.sock` under `PLY_HOME` or
  `~/Library/Application Support/ply`. plyd keeps each session's last report,
  by the payload's `session_id`, and when its numbers last changed, since a
  status line repeats unchanged numbers on every refresh; the report whose
  numbers changed most recently answers, with that time as its age, whenever it
  is newer than the cache below. It carries the windows it has, "Session · 5h"
  and "Week · all models"; the per-model weeks come only from the cache, when
  the cache is the newer. At most 64 sessions' reports are kept.
- **Claude Code, cached:** it caches the usage it fetches in `.claude.json`
  (`$CLAUDE_CONFIG_DIR/.claude.json` when the login shell exports
  `CLAUDE_CONFIG_DIR`, else `~/.claude.json`) under `cachedUsageUtilization`:
  `{fetchedAtMs, accountUuid, utilization: {five_hour, seven_day,
  seven_day_opus, seven_day_sonnet, …}}`, each window `{utilization (percent),
  resets_at (RFC 3339)}`. ply takes those four windows when they hold a number
  — "Session · 5h", "Week · all models", "Week · Opus", "Week · Sonnet" — and
  `fetchedAtMs` as their age. It never reads the account id or any of the
  other keys (there are many, under internal code names). The cache is Claude
  Code's internal state, not a published format: a window that is missing,
  `null` or renamed is left out, and a cache without `fetchedAtMs` or any
  readable window counts as none, so a change in a Claude Code release makes
  the view show less, never fail. plyd parses the whole file (64 MiB at most)
  and keeps only that object.
- **Codex** records the account's rate limits in every `token_count`
  `event_msg` of its rollouts: `rate_limits: {limit_id, limit_name, primary,
  secondary, plan_type, …}`, each window `{used_percent, window_minutes,
  resets_at (Unix seconds)}` (`resets_in_seconds` is read too). plyd reads the
  20 most recently written rollouts from their ends — at most 2 MiB of each and
  8 MiB in all, a line over 1 MiB skipped — takes each file's newest record and
  the model of the turn behind it (`turn_context.model`), and keeps the newest
  record per `limit_id` with every model seen with it. 300 minutes is "Session ·
  5h", 10 080 "Week", any other window is named by its length ("2 d"), and a
  limit other than `codex` adds its name. `plan_type` and the age come from the
  newest record.

An answer is reused for 5 s. A CLI with no readable record is simply absent,
and the view says "No usage recorded yet".

## Dispatching tasks

The task queue (Ruling R60) lets the user line up prompts and skills for a pane
and has plyd type each one in when it is their turn there
(`docs/control-channel.md`, **The task queue**). plyd types a task's text
exactly as written, into the pane's terminal as the user would, and nothing
else: it never answers a dialog, passes an option or changes a prompt. Each
agent process's `Agent` owns a small clocked machine for it
(`crates/daemon/src/panes/dispatch.rs`); the registry holds the queue.

**When a task is typed.** Only into an agent pane whose running process has
shown its prompt, that has been `idle` for 1 s (`SETTLE`), has no other task
typed, and whose queue is not paused, and only the queue's first task. The
prompt rule keeps a queued Enter away from a CLI's startup screens (trust,
hooks review, update), which wait for as long as nobody answers: a Codex pane
counts as `idle` from its first output byte, which such a screen prints too.
`Adapter::shows_prompt` names the signals that count, raised by the process
itself: for Claude Code SessionStart (`Ready`; its hooks run only once the
folder is trusted), UserPromptSubmit or Stop; for Codex a turn starting
(`task_started` read live from the rollout, not the thread's past) or ending.
Codex's first byte and its Enter guess do not count, and neither does a stored
session id: a resumed process starts over. So a Codex pane, fresh or resumed,
takes tasks after its first turn (open it with a first prompt, or type one);
meanwhile its queue is `blocked: startup` and nothing, not even `task.send`,
types into it. Every new process, every change to `idle`, the process first
showing its prompt and the input clearing make the machine look at its queue
again, so a task added while the pane was busy, lost or starting still goes.
The user's own input comes first:

- any key, raw input or paste of the user's marks the pane's input as typed,
  since the CLI may be holding text the task would be appended to. Only Ctrl+C,
  Ctrl+U (in either key encoding), the CLI acknowledging a prompt
  (`Adapter::acknowledges_prompt`) or starting a new session (`Ready`) clear
  it. The user's Enter does not: it may add a line to a draft (`\` then Enter,
  ⌥⏎) or open a local command's picker (`/model`, `/resume`), and a task typed
  there would join the draft or pick an entry. Enter alone on an empty input
  marks nothing, and neither do keys typed while the pane waits for permission
  or input, which answer the CLI's dialog or question. Esc counts as typing:
  Claude Code puts an interrupted prompt back into its input;
- while the input is typed, the queue is `blocked: typing` and waits; the user
  submits or clears their text, or sends the task anyway with `task.send`;
- every key the user types restarts the 1 s, and an Enter of theirs holds the
  pane for 3 s (`SETTLE_AFTER_ENTER`), so their own prompt reaches the CLI
  before a task can.

**A pool task** (sent to "the next free pane") goes to the first pane that
asks for its next task with its own queue empty and running, a process that
has shown its prompt, no unsent typing, the pool's CLI, and a directory at or
below the pool's. The pane takes it at the
moment it would type it, under the registry's lock, so one task is never typed
twice; a new pool task nudges every live pane of its CLI in the workspace.

**How it is typed.** The registry marks the task `sent` and hands over its
text; the pane task writes it as one paste encoded against the pane's modes
(bracketed when the CLI enabled bracketed paste, as both do), and 50 ms later
(`ENTER_DELAY`, so a slash or `$` pop-over the text opened has settled) one
Enter, a real key press and release encoded like a typed one (so a CLI in the
kitty keyboard protocol gets `CSI 13 u`). The Enter counts as a key typed for
the status machine, like any other. It is left out when, by then, the pane is
no longer `idle` (a dialog opened) or the user wrote to it after the paste; the
task then waits for an acknowledgement like any other and fails as not
submitted without one. A paste the terminal refuses (text with line breaks for
a CLI without bracketed paste, Ruling R21) fails the task and no Enter
follows.

**A task with an image path** waits 2 s for its Enter instead
(`IMAGE_ENTER_DELAY`). Claude Code splits a paste at line breaks and at a space
before `/`, and a piece that, trimmed, unquoted and unescaped, ends in `.png`,
`.jpg`, `.jpeg`, `.gif` or `.webp` becomes an image it reads before it takes
keys again, turning it into `[Image #N]` ahead of the rest of the text; an
Enter that arrives meanwhile is dropped, where after a text-only paste it is
replayed. It decides from the text alone, so `optimize logo.png` takes that
path too, file or no file. `Adapter::paste_reads_images` applies the same rule
(`crates/agents/src/claude/paste.rs`); for Codex it is always false. Measured
with Claude Code 2.1.283 on an Apple-silicon Mac, one to three Retina
screenshots (3.5–8.6 MB) took 0.41–0.55 s to read, idle or with every core busy,
and a 50 ms Enter was lost every time; 2 s is about four times that. A read
that takes longer still leaves the task failing as not submitted, with its text
in the input for the user's Enter. Files dropped on the ⌘E form put their paths
into the task this way, one per line; a ⌘⇧4 thumbnail's is the path of the copy
ply kept, since its own file is gone before the task is typed, and that copy is
deleted when the task ends, fails or is cancelled (`docs/terminal.md`,
**Dropped files**).

**How it is followed**, from the signals the status machine already uses:

| Signal | Task |
|---|---|
| the CLI takes the prompt: Claude Code's UserPromptSubmit (`PromptSubmitted`); Codex's rollout `task_started` (`TurnStarted`) — Codex's Enter fast path is no acknowledgement (R48). `Adapter::acknowledges_prompt` names it | `running` |
| the turn ends: `TurnComplete` (Stop, notify, `task_complete`, `turn_aborted`) or Claude Code's quiet timeout | `ended` |
| no acknowledgement within 10 s (`ACK_WAIT`), or a Codex Enter that started no turn (`NoTurnStarted`) | `failed` (not submitted), queue paused `failed` |
| the process exits, or the pane closes | `failed` |

A task whose pane waits for permission or input stays `running`: the user
answers the CLI as always. `ended` means the turn the task started ended,
which ply cannot tell from the user interrupting it. After a task ends or
fails, the next waits for the pane to settle again; a task that failed as not
submitted may have left its text in the CLI's input, so the input counts as
typed until the user clears or submits it. Every step is logged at
`info` with the pane and task ids.

A task's text is typed as is, so a leading `!` or `#` does in the CLI what it
does when typed there. The queue never hands one session's output to another
(spec 1.4).

## Skills

The task form (⌘E) lists the skills each CLI offers on this machine
(`skill.list`, Ruling R61). ply runs none of them and changes none of their
files: it reads what the CLI itself reads and shows each skill with the text
the user types to run it (`crates/daemon/src/skills.rs` walks the folders,
`crates/agents/src/skills.rs` reads each file).

| CLI | Source | Read from | Typed as |
|---|---|---|---|
| Claude Code | project | `<cwd>/.claude/skills/<name>/SKILL.md`, `<cwd>/.claude/commands/<name>.md` | `/<name>` |
| Claude Code | user | `skills/<name>/SKILL.md` and `commands/<name>.md` in `$CLAUDE_CONFIG_DIR`, else `~/.claude` | `/<name>` |
| Claude Code | plugin | `skills/` and `commands/` of each enabled plugin's install folder: `enabledPlugins` of the user's `settings.json`, then the project's `.claude/settings.json` and `.claude/settings.local.json` (a later file wins), looked up in `plugins/installed_plugins.json` (`installPath`; a project-scoped install only for its own project) | `/<plugin>:<name>` |
| Claude Code | plugin | skills synced from the user's claude.ai account, `skills/synced/<account>/<name>/SKILL.md` | `/anthropic-skills:<name>` |
| Codex | project | `.agents/skills/<name>/SKILL.md` and `.codex/skills/<name>/SKILL.md` in the pane's directory and each parent up to its repository root (8 levels at most, never the home directory) | `$<name>` |
| Codex | user | `$CODEX_HOME/skills/<name>/SKILL.md` (default `~/.codex`), `~/.agents/skills/<name>/SKILL.md` | `$<name>` |
| Codex | system | `$CODEX_HOME/skills/.system/<name>/SKILL.md`, the skills Codex installs itself | `$<name>` |
| Codex | prompt | `$CODEX_HOME/prompts/<name>.md`, custom prompts | `/prompts:<name>` |

A skill's name is its front matter's `name` when that can be typed as one word,
else its folder's; a command or prompt is named after its file. The front
matter's `description` (400 characters at most) and `argument-hint` (80) are
shown beside it. A skill whose front matter says `user-invocable: false` is left
out, as is a name holding white space, `/`, `$` or a control character, or
starting with `-`. An earlier row wins when two offer the same invocation.
Claude Code's built-in commands live inside its binary, so they are not listed;
they can still be typed as a task's text.

**Facts behind the table**, checked on 2026-09-26 against the installs of
Claude Code 2.1.283 and Codex 0.157.1 (the Codex binary was searched, never run): Codex's
bundled instructions tell it to "mention the skill as `$skill-name`" and its TUI
says "Use $ to insert"; it ships its own skills under `skills/.system` and reads
`.agents` folders; Claude Code keeps synced skills under `skills/synced/` with a
`manifest.json` and names them `anthropic-skills:<name>`; skill folders may
hold a colon (`team:deploy`), which the invocation keeps.

Every read is bounded: 256 entries per folder, the first 32 KiB of a skill file
(front matter that does not close within them does not count), 1 MiB of a
settings file or the plugin registry, 512 skills per answer. A missing folder
lists nothing; one that cannot be read is skipped and logged. An answer is
reused for 10 s per CLI and directory, and `crates/daemon/tests/skills.rs`
checks that every file it read is unchanged (INV-8).

## What ply never does to the user's configuration

ply never writes `~/.claude/settings.json`, `~/.claude.json` or
`~/.codex/config.toml` (INV-8); it reads `~/.claude.json` only for the usage
cache above, and the user's `settings.json` files only for their `statusLine`
and their `enabledPlugins` (**Skills**). Everything it configures is per invocation:
Claude Code's `--settings` file lives in ply's own `run/panes/<id>/`, and
Codex's options are `-c` arguments. ply never passes a permission-skipping flag,
never installs, updates or signs in either CLI, and adds no hook to the user's
settings.

The CLIs keep writing their own state as they do in any terminal: Claude Code
its transcripts under `~/.claude/`, Codex its rollouts under
`$CODEX_HOME/sessions/`, and Codex rewrites its own `last_updated` and
`last_revision` in `~/.codex/config.toml` through its plugin-marketplace
auto-upgrade. That is Codex, not ply. Tests that run a CLI, fake or real, set a
sandboxed `HOME` and `CODEX_HOME`.

## Resume

After a logout, a reboot or a plyd restart the processes are gone, and their
panes come back `lost`. Right after its sockets are bound, plyd relaunches them
by itself (`reopen_lost`): every `lost` pane without a session id reopens as
below (Ruling R50), and, with the setting `resume_sessions_on_start` (on by
default), every agent pane with one resumes its session, as `pane.resume`
would. With the setting off those panes stay `lost` until `pane.resume`
relaunches one from its `launch.json` (`crates/daemon/src/panes/launch.rs`,
`resume`); a resume that fails leaves its pane `lost` too:

- **Claude Code**: `claude --settings … --resume <session_ref>` in the stored
  working directory, with the stored `--worktree` option when there was one.
- **Codex**: `codex -c … resume <session_ref>`; the new session starts bound to
  that thread.
- **A pane without a session id** — a shell, or an agent whose CLI never
  reported one — reopens as a fresh login shell in its last directory
  (spec 11.3), at plyd's start without a click. An agent pane becomes a shell
  pane from then on: `cli` is `shell` in its row, in `session.list` and in the
  returned `Pane`, and plyd announces the new record with `pane.added`. Only
  when that reopening fails (the directory is gone, no palette arrived) does
  such a pane stay `lost`.

The settings file is regenerated with the current settings, and `launch.json`
records the new spawn, so a pane resumes again after the next restart. The pane
leaves `lost` at once; a second `pane.resume` while one runs is refused. A
`pane.close {kill:true}` while it runs is kept until the pane adopts the new
process, which it then stops, and closes the pane at once when the resume fails
(`a_close_with_kill_during_a_resume_stops_the_resumed_process_and_closes_the_pane`). The app
offers the action on every lost pane: the Resume button of the strip under it
and a "Resume <pane>" palette command. The screen is not restored: plyd does
not keep screens across its own restarts, so a resumed pane starts on an empty
terminal and the CLI repaints it.

## Tests

- `crates/agents/tests/launch.rs`: the exact argv, environment and settings file
  of both CLIs, the refused values, the plan-tool setting, resume and prompt,
  `launch.json`, the minimums and the install layouts
  (`claude_settings_contain_only_hooks_and_the_theme`,
  `codex_argv_carries_the_per_invocation_overrides`,
  `claude_version_comes_from_the_install_layout`,
  `codex_version_comes_from_the_package_metadata`).
- `crates/agents/tests/claude_hooks.rs`: every scrubbed Claude Code fixture in
  `tests/fixtures/claude/` to its signals, the worktree label, call matching,
  counting unknown events.
- `crates/agents/tests/claude_progress.rs`, `codex_rollout.rs` (records, both
  `update_plan` shapes, turn events, the tailer's pre-filter, discovery, the
  R28 binding flow and the R49 rebinding), `codex_osc9.rs` (the prefix table
  and the observed bodies).
- `crates/agents/tests/usage.rs`: the scrubbed `tests/fixtures/claude/dot-claude.json`
  and the synthetic `rollout-usage-*.jsonl` beside the S3b captures (the four
  windows and nothing else, tolerant parsing, the newest record per limit and
  its models); `crates/daemon/src/usage.rs` (the sources, the bounded
  tail-first reading, missing and malformed files, the 5 s cache) and
  `crates/daemon/tests/usage.rs` (a real plyd, files unchanged).
- `crates/daemon/src/panes/dispatch.rs` (when a task is typed, the settle
  times, the typing block, acknowledgements, failures),
  `crates/daemon/src/panes/queue.rs` (order, positions, restart, history) and
  `crates/daemon/tests/dispatch.rs`: a real plyd typing two tasks into a
  fake Claude Code one per turn, a Codex task queued before its first turn
  going after it and confirmed by its rollout, a Codex pane resumed after a
  restart typed into only after its first turn, the typing block and
  `task.send`, a task never acknowledged failing and pausing its queue, pool
  tasks going to the one free Claude Code pane in their folder and to a pane
  opened after them, and the queue over C1 and across a restart. The fakes' `submit`
  command reads what the pane submits (`crates/daemon/tests/common/fake.rs`).
- `crates/agents/tests/skills.rs`: front matter (plain, quoted, folded,
  nested, CRLF), names and invocations, opting out, the plugin registry;
  `crates/daemon/src/skills.rs` (limits, precedence, the repository root) and
  `crates/daemon/tests/skills.rs` (a real plyd listing both CLIs' skills from a
  sandboxed home, files unchanged).
- `crates/hook/tests/hook.rs`: the C3 line, the payload byte for byte, INV-12
  (plyd down, stdin held open, a listener that never reads) and INV-14 (stdout
  and stderr empty on fourteen bad paths).
- `crates/daemon/tests/states.rs`: every row of the table through a real plyd
  and the fake CLIs of `crates/daemon/tests/fake/` (they run the hook commands
  of their `--settings` file or `-c notify=…`, print OSC 9 and write a rollout
  in the sandbox's `CODEX_HOME`), F1 (a hook turns the pane waiting within
  250 ms), INV-12 with plyd down, the version check at spawn, the R28
  rollout binding, Codex turns from the rollout (an Enter with no turn, a first
  prompt, an Esc abort), a new thread's rebinding and a truncated rollout.
- `crates/daemon/tests/config_untouched.rs`: INV-8, a Claude Code and a Codex
  session leave the user's three config files byte for byte as they were.
- `crates/daemon/tests/resume.rs`: journey J6 (kill plyd, restart it, the
  agent panes are `lost` and the shell is back by itself, `pane.resume` brings a
  `--worktree` Claude pane and a Codex pane back with the same option and
  directory), a pane without a session id reopening as a shell by itself, a
  Codex pane lost mid-turn resuming `idle` with its plan and no replayed turn,
  and F3 (closed sessions keep status, times, exit codes and session ids across
  app and plyd restarts).
- `crates/daemon/tests/lifecycle.rs`:
  `an_agent_pane_runs_the_cli_from_the_login_path_with_its_launch_spec` (a fake
  `claude` on the login `PATH` gets the adapter's argv, environment and files)
  and `a_restart_reopens_a_shell_by_itself_and_keeps_the_settings`.
