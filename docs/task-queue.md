# The task queue

The task queue (Ruling R60) lets the user line up prompts and skills for a
Claude Code or Codex pane and has plyd type each one into that pane when it is
the user's turn there. This document is the architecture of that feature: the
pieces and who owns what, a task's life, the queue's data model, the dispatch
engine that decides when to type, and how failures, pools and restarts are
handled. `docs/control-channel.md` (**The task queue**, **Records**) has every
method, record and error code on the wire; `docs/agents.md` (**Dispatching
tasks**, **Skills**) has the CLI signals the engine listens to and the skill
folders; `docs/perf.md` has the runs against the real CLIs.

**The rule everything serves.** ply types only a task the user wrote, text
unchanged, as one paste and one Enter, into an idle agent pane whose running
process has shown its prompt and whose input holds nothing the user typed. It
never answers a dialog, passes an option, changes a prompt, or hands one
session's output to another (spec 1.4). When it cannot be sure a moment is
safe, it waits; when a typed task goes wrong, it pauses the pane's queue and
leaves the next move to the user.

## The pieces

| Piece | File | Owns |
|---|---|---|
| wire types | `crates/proto/src/pane.rs`, `control.rs` | `Task`, `TaskState`, `TaskTarget`, `TaskPool`, `QueueState`, `PauseReason`, `BlockReason`, the limits (`MAX_TASK_TEXT_BYTES`, `MAX_QUEUED_TASKS`, `MAX_OPEN_TASK_TEXT_BYTES`), the `task.*` and `queue.pause` params |
| C1 handlers | `crates/daemon/src/server/control.rs` | `task.list`, `task.add`, `task.cancel`, `task.move`, `task.send`, `queue.pause`; resolving a pool's directory; nudging the panes a change concerns |
| the registry | `crates/daemon/src/panes/registry.rs` | validation of `task.add`, the one lock, handing a pane its next task (`next_task`), claiming it for typing (`take_task`), recording progress (`task_progress`), storing and announcing every change (`apply_tasks`) |
| the queue | `crates/daemon/src/panes/queue.rs` | `Queues`: every task plyd knows, each queue's order, each pane's pause and block. No I/O |
| the dispatch engine | `crates/daemon/src/panes/dispatch.rs` | `Dispatch`: one clocked state machine per agent process that decides when to type, and follows the typed task. No I/O |
| the agent | `crates/daemon/src/panes/agent.rs` | owns the process's `Dispatch`, feeds it every status signal and status, carries out its actions against the registry (`run_dispatch`) |
| the pane task | `crates/daemon/src/panes/pane.rs` | the terminal: encodes the paste and the Enter against the pane's modes and writes them to the pty; reports the user's own keys and pastes; sleeps until the engine's next deadline |
| the adapters | `crates/agents/src/{claude,codex}/mod.rs` | `Adapter::shows_prompt` and `Adapter::acknowledges_prompt`: which signals count as "at its prompt" and "took the prompt" for each CLI |
| storage | `crates/daemon/src/db/mod.rs`, `migrations/0002_tasks.sql` | the `tasks` table (schema v2) |
| skills | `crates/daemon/src/skills.rs`, `crates/agents/src/skills.rs` | `skill.list` (Ruling R61), read-only, for the task form |
| the app | `app/src/features/dispatch/`, `app/src/features/panes/queued-strip.tsx`, `app/src/state/` | the ⌘E task form, the ⌘⇧E queue sheet, the strip under a pane, the header badge, the top bar pill; the store's view of tasks and queues |

## How the pieces fit

```
 app                          plyd
 ───                          ────
 ⌘E form ── task.add ───────▶ control.rs ──▶ Registry::add_task ──▶ Queues::insert, tasks row, task.changed
                                   │
                                   └── nudge: PaneCmd::Queue ──▶ pane task ──▶ Agent::nudge ──▶ Dispatch::nudge
                                                                                                      │
                               ┌──────────────────────── Action::Check ◀──────────────────────────────┘
                               ▼
                   Registry::next_task ── head, or a pool task claimed ──▶ Dispatch::offer
                                                                                 │ Action::Take
                   Registry::take_task ── queued → sent, the text ────▶ Dispatch::taken
                                                                                 │ Action::Paste, 50 ms later Action::Enter
                                                                                 ▼
                                                  Agent::take_writes ──▶ PaneTask::type_queued ──▶ encode_input ──▶ pty
                                                                                                                    │
 queue sheet, strip, badge ◀── task.changed ◀── Registry::task_progress ◀── Action::Report ◀── Dispatch::on_signal ◀─┘
                          ◀── queue.changed ◀── Registry::block_queue   ◀── Action::Block      (hooks, notify, rollout)
```

The design splits the feature into two pure parts and an effectful shell.

**`Queues` is data; `Dispatch` is timing.** `Queues` knows which task is next
and nothing about terminals. `Dispatch` knows when a pane may be typed into and
nothing about which task that is: it asks for the head, claims it by id and
reports back by id. Neither reads the clock or does I/O: every method takes
`now` and returns what changed (`Vec<Task>`, `Option<QueueState>`) or what to do
(`Vec<Action>`). The registry, the agent and the pane task are the shell that
stores, announces, locks and writes bytes.

**The queue lives per pane; the machine lives per process.** A pane's tasks
outlive its process: they wait through a crash, a `lost` pane and its resume,
and a restart of plyd. The machine does not. Every new process gets a new
`Agent` and with it a new `Dispatch` that has not seen the process's prompt, so
a resumed process has to show its prompt again before anything is typed into
it; a stored session id never counts.

**The clock is a parameter.** Because the machine takes `now: Instant` and
answers with `deadline()`, its unit tests drive it with synthetic instants
(`at(t0, 1000)`) and check every settle time and timeout exactly, without
sleeping, and the pane task sleeps until the next deadline instead of polling.

## A task's life

```
queued ──take──▶ sent ──ack──▶ running ──turn ends──▶ ended
  │               │               │
  │               └───────┬───────┘
  ▼                       ▼
cancelled               failed ──▶ the pane's queue pauses (`failed`)
```

| From | To | When | Decided by |
|---|---|---|---|
| — | `queued` | `task.add` passed its checks; stored, then announced | `Registry::add_task` |
| `queued` | `sent` | the pane's engine claimed it: the head (or any task, for `task.send`), the pane `idle` with no task typed and a session reported | `Registry::take_task` |
| `sent` | `running` | the CLI acknowledged the prompt (Claude Code's UserPromptSubmit, Codex's rollout `task_started`) | `Dispatch::on_signal` |
| `running` | `ended` | the turn ended (`TurnComplete`: Stop, notify, `task_complete`, `turn_aborted`) or Claude Code's quiet timeout | `Dispatch::on_signal` |
| `sent` | `failed` | no acknowledgement within `ACK_WAIT`, or a Codex Enter that started no turn (`NoTurnStarted`) | `Dispatch` |
| `sent` | `failed` | the terminal refused the paste (text with line breaks for a CLI without bracketed paste, Ruling R21) | `PaneTask::type_queued` → `Dispatch::paste_refused` |
| `sent`, `running` | `failed` | the process exited, the pane closed, plyd restarted | `Dispatch::on_status`, `Queues::close_pane`, `Queues::restore` |
| `queued` | `cancelled` | `task.cancel`; the pane closed or reopened as a shell; plyd restarted with it on a closed pane or in a pool | `Queues::cancel`, `end_pane`, `restore` |

`ended` means the turn the task started ended, which ply cannot tell from the
user interrupting it. A task waiting for permission or input in the CLI stays
`running`: the user answers the CLI as always. A typed task cannot be taken
back; `task.cancel` of anything but a `queued` task is `invalid_state`.

Each state stamps its time (`sent_at`, `started_at`, `ended_at`), and `detail`
says why a task failed or was cancelled, from the constants in `dispatch.rs`
(`NOT_SUBMITTED`, `PASTE_REFUSED`, `PROCESS_EXITED`) and `queue.rs`
(`PANE_CLOSED`, `PLYD_RESTARTED`, `REOPENED_AS_SHELL`, `POOL_RESTARTED`).

## The queue (`queue.rs`)

`Queues` is a `BTreeMap<TaskId, Task>` of every task plyd knows, plus a
`HashMap<PaneId, Flags>` of each pane's pause and block. Queues are not objects
of their own: a queued task belongs to the queue its fields name
(`QueueKey::of`):

| `QueueKey` | A task is in it when | Taken from by |
|---|---|---|
| `Pane(id)` | `pane_id` is set | that pane only |
| `Pool {workspace_id, cli, cwd}` | `pane_id` is absent and `pool` is set | the first free pane of that CLI in `cwd` or below (see **Pools**) |

**Order.** `position` numbers a queue's queued tasks 0, 1, 2… without gaps.
A new task goes last; `task.move` reorders (a position past the end means last);
a task leaving `queued` renumbers the rest (`renumber`). A task that has left
keeps the position it last had. Every task whose position changed is part of
the change and is announced.

**Head and active.** A pane's head (`head`) is its first queued task, or none
while the queue is paused. Its active task (`active`) is the one `sent` or
`running`; there is at most one, and a pane with one gets no other.

**Flags.** A queue can be paused, which stops it until the user resumes it, and
blocked, which the engine sets and clears by itself:

| Flag | Value | Set when | Cleared when |
|---|---|---|---|
| `paused` | `user` | `queue.pause {paused: true}`, the strip's Hold | `queue.pause {paused: false}` |
| `paused` | `restored` | plyd started with queued tasks on this open pane | the user resumes it |
| `paused` | `failed` | a task of this pane failed | the user resumes it |
| `blocked` | `startup` | the head waits for the process to show its prompt | the process shows it |
| `blocked` | `typing` | the head waits because the user typed into the pane | the input clears, the CLI takes a prompt, or `task.send` |

Flags live in memory only. A change comes back from `pause` or `block` as the
new `QueueState`, which the registry announces with `queue.changed`; a call that
changes nothing returns `None` and announces nothing. `task.list` returns the
flags of every open pane of the workspace that is paused or blocked.

**Limits.** `task.add` refuses text that is empty, over 16 KiB
(`MAX_TASK_TEXT_BYTES`) or holds a control character other than newline and tab
(`check_text`: a carriage return, ESC or NUL would act as keys, not text); a
skill invocation over 256 bytes (`MAX_SKILL_BYTES`); a 33rd queued task in one
queue or pool (`MAX_QUEUED_TASKS`); and text that would take the workspace's
queued, sent and running tasks past 256 KiB together
(`MAX_OPEN_TASK_TEXT_BYTES`).

**History.** Finished tasks stay as history, the newest 200 per workspace
(`KEEP_FINISHED`); every change that finishes a task prunes its workspace
(`prune`) and deletes the pruned rows. `task.list` answers open tasks first
(sent and running, then queued by pane and position, pool tasks last), then
the finished ones newest first for as long as the tasks fit in 960 KiB of JSON
(`LIST_BYTES`, the 1 MiB C1 line less room for the response and the flags).
Open tasks are always listed.

## The dispatch engine (`dispatch.rs`)

One `Dispatch` per agent process, owned by the process's `Agent` on the pane
task and driven one event at a time. It holds a phase and a few facts about the
pane:

| Field | Meaning |
|---|---|
| `phase` | `Waiting`, `Taking {task}`, `Pasted {task, enter_at}`, `Sent {task, deadline}` or `Running {task}` |
| `status`, `idle_since` | the pane's status as the status machine last set it, and since when it has been `idle` |
| `live` | the process has shown its prompt (`Sense::shows_prompt`) |
| `typed` | the user typed into the pane since their last prompt, so the CLI's input may hold text |
| `quiet_until` | no typing before this: the user's last key plus `SETTLE`, or plus `SETTLE_AFTER_ENTER` after their Enter |
| `check` | something changed; ask for the head once the pane has settled |
| `force` | a task `task.send` asked for |
| `interrupted` | the user wrote to the pane between the paste and its Enter |
| `blocked` | the block last reported, so a repeat is not reported again |

### Inputs and actions

| Input | Called by the agent when |
|---|---|
| `new(status, now)` | a process starts, fresh or resumed; `check` starts true |
| `on_signal(signal, sense, status, now)` | the adapter produced a status signal and the status machine applied it; `sense` is the adapter's reading of it |
| `on_status(status, now)` | the status changed without a signal: the process exited |
| `on_user_input(bytes, now)` | the user's own KEY or INPUT_RAW bytes went to the pty |
| `on_user_paste(now)` | the user pasted |
| `nudge(now)` | the pane's queue changed (`PaneCmd::Queue`) |
| `send_now(task, now)` | `task.send` (`PaneCmd::SendNow`) |
| `offer(head, now)`, `taken(task, text, now)` | the registry's answers to `Check` and `Take` |
| `paste_refused(now)` | the terminal refused the paste |
| `tick(now)` | time passed; the pane task wakes at `deadline()` |

| Action | The agent |
|---|---|
| `Check` | asks `Registry::next_task(pane, may_claim())` for the head, or a pool task, and answers with `offer` |
| `Take(task)` | asks `Registry::take_task`, which marks the task `sent` and returns its text if it is still takeable, and answers with `taken` |
| `Paste(text)` | queues a `Typed::Paste` for the pane task |
| `Enter` | queues a `Typed::Enter` for the pane task |
| `Report {task, state, detail}` | `Registry::task_progress`: stores, announces, and pauses the queue on `failed` |
| `Block(reason)` | `Registry::block_queue`: `queue.changed` when the block changed |

### Phases

```
                   settled, Check, offer(head) → Take
      ┌─────────┐ ─────────────────────────────────▶ ┌─────────┐  taken(text)  ┌─────────┐  ENTER_DELAY  ┌─────────┐
      │ Waiting │                                    │ Taking  │ ────────────▶ │ Pasted  │ ────────────▶ │  Sent   │
      └─────────┘ ◀───────────────────────────────── └─────────┘   (Paste)     └─────────┘   (Enter)     └─────────┘
        ▲     ▲       taken(None): not takeable any more                          │   │ ack               │ ack
        │     │                                                                   │   ▼                   ▼
        │     └────────────── paste refused (failed) ─────────────────────────────┘ ┌─────────┐ ◀─────────┘
        │                                                                           │ Running │
        │    turn ends (ended) · ACK_WAIT or NoTurnStarted in Sent (failed)         └─────────┘
        └──────────── process exits in any phase but Waiting (failed) ◀─────────────────┘
```

- **Waiting → Taking.** Only once the pane has settled (below) and either `check`
  or `force` is set. With `force` it takes that task straight away; otherwise it
  emits `Check` and passes the head through the gates in `offer`.
- **Taking → Pasted.** The registry marked the task `sent` and returned its text;
  the machine emits `Paste` and sets `enter_at = now + ENTER_DELAY`. If the
  registry returned nothing (the task was cancelled, moved or no longer the head
  between `Check` and `Take`), it goes back to `Waiting` and checks again.
- **Pasted → Sent.** At `enter_at` it emits `Enter`, unless the user wrote to the
  pane since the paste (`interrupted`: their keys would be submitted with the
  task) or the pane is no longer `idle` (a dialog opened: an Enter would answer
  it). Either way the acknowledgement deadline starts: `now + ACK_WAIT`.
- **Pasted or Sent → Running.** On the CLI's acknowledgement. One that arrives
  while still `Pasted` (the user pressed Enter themselves) counts too, and no
  second Enter follows.
- **Running → Waiting.** On the turn ending: `Report(ended)`, `check` set, and a
  fresh `SETTLE` of quiet before the next task.
- **Sent → Waiting, failed.** On `ACK_WAIT` passing or `NoTurnStarted`. The paste
  may still be in the CLI's input, so the machine sets `typed`, and the registry
  pauses the queue (`failed`). A task whose Enter was left out waits here too:
  the user can submit it themselves (acknowledged, `running`) or it fails as not
  submitted.

### When it looks at the queue

The machine asks for the head only when something may have changed, and never
polls a queue that is empty or blocked. `check` is set:

- when the machine is created (a new process, fresh or resumed);
- whenever the pane turns `idle` (a pool task may be waiting for it);
- when the process first shows its prompt (`live` becomes true);
- when the user's typing clears (`typed` goes from true to false);
- when a typed task ends or fails;
- on a nudge: `task.add`, `task.cancel`, `task.move` or `queue.pause` touching
  this pane, or `task.add` of a pool task of this pane's CLI in its workspace.

`deadline()` is the settle moment while `Waiting` with `check` (or with `force`
and `live`), `enter_at` while `Pasted`, the acknowledgement deadline while
`Sent`, and none otherwise; a blocked or empty queue has no deadline at all.

### The gates

**Settled.** A `Waiting` machine may act only when the pane is `idle`, has
been `idle` for `SETTLE`, and is past `quiet_until`:
`settled_at = max(idle_since + SETTLE, quiet_until)`. A `running`,
`waiting_permission`, `waiting_input`, `starting`, `lost` or `exited` pane has no
settle moment.

**`offer`, in order.** With the head the registry answered:

| Head | Machine | Result |
|---|---|---|
| none | — | unblock (`Block(None)` if it was blocked) |
| a task | not `live` | `Block(Some(Startup))` |
| a task | `typed` | `Block(Some(Typing))` |
| a task | otherwise | unblock, `Take(task)` |

**At its prompt.** A process is `live` once it raised a signal its adapter
counts as showing the prompt, which only the process itself can raise. A queued
Enter must never reach a startup screen (folder trust, hooks review, an update
prompt): those wait for as long as nobody answers, and a Codex pane counts as
`idle` from its first output byte, which such a screen prints too.

| CLI | `shows_prompt` | `acknowledges_prompt` |
|---|---|---|
| Claude Code | `Ready` (SessionStart; hooks run only once the folder is trusted), `PromptSubmitted`, `TurnComplete` | `PromptSubmitted` (UserPromptSubmit) |
| Codex | `TurnStarted` (`task_started` read live from the rollout, not a resumed thread's past), `TurnComplete` | `TurnStarted` |

Codex's first byte, its Enter guess (a `PromptSubmitted` without a turn, R48)
and a stored session id count for neither. So a Codex pane, fresh or resumed,
takes tasks only after a turn of its own: the user opens it with a first prompt
or types one. Until then its queue is `blocked: startup`, and nothing, not even
`task.send`, is typed into it.

### The user's input comes first

Any key, raw input or paste of the user's may leave text in the CLI's input
that a task would be appended to, so it marks the input as typed, and a typed
input blocks the queue.

| The user's input | `typed` |
|---|---|
| a key, raw input or paste while the pane is `idle` or `running` | set (the CLIs accept type-ahead) |
| Esc | set: Claude Code puts an interrupted prompt back into its input |
| Enter alone (`\r`, `CSI 13 u`) | unchanged: it puts nothing into an empty input |
| any key while `waiting_permission` or `waiting_input` | unchanged: it answers the CLI's dialog or question |
| Ctrl+C, Ctrl+U, legacy or kitty encoding (`clears_input`) | cleared |
| the CLI acknowledging a prompt, or starting a session (`Ready`) | cleared |
| a typed task failing unsubmitted | set: its text may still be in the input |

The user's Enter does not clear it: it may add a line to a draft (`\` then
Enter, ⌥⏎) or open a local command's picker (`/model`, `/resume`), and a task
typed there would join the draft or pick an entry. Only the CLI confirming it
took a prompt proves the input is empty.

Every key of the user's also pushes `quiet_until` out by `SETTLE`, and their
Enter by `SETTLE_AFTER_ENTER`, so their own prompt reaches the CLI (and its
status signals reach plyd) before a task can.

### The timings

| Constant | Value | File | Role |
|---|---|---|---|
| `SETTLE` | 1 s | `dispatch.rs` | how long a pane stays `idle`, and quiet after the user's last key, before a task is typed |
| `SETTLE_AFTER_ENTER` | 3 s | `dispatch.rs` | quiet after the user's own Enter |
| `ENTER_DELAY` | 50 ms | `dispatch.rs` | between the paste and its Enter, so a `/` or `$` pop-over the paste opened has settled |
| `ACK_WAIT` | 10 s | `dispatch.rs` | how long the CLI has to acknowledge before the task fails as not submitted |
| `TURN_START_WAIT` | 3 s | `agent.rs` | how long a Codex Enter waits for `task_started` before the agent raises `NoTurnStarted` (R48), which fails a `Sent` task at once |

### `task.send`

`task.send` is the user's own instruction for one task: type it at the pane's
next settled moment, even if it is not the head, the queue is paused or the
input holds typing. `control.rs` checks it first (`Registry::sendable`: the task
is queued on a pane, the pane has reported a session, is `idle` and has no task
typed, else `invalid_state`) and hands it to the pane task
(`PaneCmd::SendNow`). The agent remembers it as `forced`, so its `Take` reaches
`Registry::take_task` with `forced: true`, which skips the head check. The one
gate it does not pass is `live`: a process that has not shown its prompt gets
the task once it has.

## Carrying out an action (`agent.rs`, `pane.rs`)

**`run_dispatch`** turns the machine's actions into effects with a work queue:
the registry's answers to `Check` and `Take` are fed straight back to the
machine (`offer`, `taken`) and their actions appended, until only writes,
reports and blocks are left. A settled pane therefore goes from nudge to a
queued paste within one pane-task event.

**The registry checks again.** The machine decides; the registry verifies under
its lock. `next_task` returns a head only for an agent pane with no active task
and a running queue, and claims a pool task only when the machine says it may
(`may_claim`: `live` and not `typed`) and the pane has a session, is `idle`, has
no active task and is not paused. `take_task` hands out text only when the pane
is `idle`, not a shell, has reported a session and has no active task, and the
task is queued on this pane and is its head (unless forced). A machine that
misjudged cannot type a task twice or into the wrong pane.

**One lock, two steps.** The registry sits behind one mutex, taken for
in-memory work and a few short SQLite writes. `Check` and `Take` take it
separately, so a `task.cancel`, `task.move` or another pane's claim can land
between them; `take_task` then returns `None`, the machine goes back to
`Waiting` and checks again. A pool task is moved onto the pane inside
`next_task`, under the lock, so two free panes never take the same one.

**Writes wait for the pane task.** `Paste` and `Enter` become `Typed` values in
`Agent::writes`. The pane task collects them in `type_queued` at the end of every
loop iteration (`on_tick`) and pushes the bytes onto the same bounded write
queue as the user's own input, in order: the paste goes out as one write. The
pane task's `select!` sleeps until the earliest of its deadlines, the engine's
`deadline()` among them.

### Typing into the terminal

- **The paste** is `Input::Paste` with `allow_unsafe: false`, encoded by
  libghostty-vt against the pane's live modes: bracketed when the CLI enabled
  bracketed paste, as both do. A paste the engine refuses (`PasteRejected`) or
  cannot encode calls `Agent::paste_refused`: the task fails and no Enter
  follows.
- **The Enter** is a real key press and release (`KEY_ENTER`, no modifiers),
  encoded like a typed key, so a CLI in the kitty keyboard protocol gets
  `CSI 13 u`. It is then reported to the agent as a key typed with `enter`, so
  the status machine treats it as it treats the user's Enter (for Codex it
  starts the `running` guess and its `TURN_START_WAIT`).

### What counts as the user's input

| Into the pane | Engine | Status machine |
|---|---|---|
| C2 KEY that encoded to bytes | `on_user_input(bytes)` | `KeyTyped` |
| C2 INPUT_RAW | `on_user_input(bytes)` | `KeyTyped` |
| C2 PASTE that encoded to bytes | `on_user_paste` | — |
| `pane.answer` (`PaneCmd::Write`) | — (it answers a dialog) | `KeyTyped` |
| the queue's own paste and Enter | — | Enter only: `KeyTyped {enter: true}` |

## Pools

`task.add` with `target: {pool: {cli, cwd}}` queues a task for "the next free
pane" of a CLI in a folder instead of one pane.

- **Adding.** `cwd` must be absolute. `control.rs` resolves it through symlinks
  before storing it (`resolve_pool_dir`), since the CLIs report their own
  directory that way (`/var` is `/private/var` on macOS). The task is announced
  with no `pane_id`, and every live pane of that CLI in the workspace is nudged
  (`Registry::pool_panes`).
- **Claiming.** A pane claims a pool task when it asks for its next task and its
  own queue has none: `Queues::claim` picks the oldest queued pool task
  (`position`, then id) of the pane's workspace and CLI whose directory is the
  pane's or above it, compared by path component (a pane in
  `/Users/example/projectile` is not below `/Users/example/project`; one in
  `…/project/.claude/worktrees/x` is). The task gets the pane's `pane_id` and
  the last position on its queue, keeps `pool` for display, and is announced;
  the pool renumbers. It is then that pane's head and is typed at once.
- **Timing.** The claim happens at the moment the pane would type, not when the
  task is added, so the task goes to whichever pane becomes free first,
  including one opened after it.
- **Limits.** A pool cannot be paused (its tasks are cancelled one by one) and a
  pool task cannot be sent with `task.send`. A pool task no pane had taken when
  plyd stopped is cancelled at the next start.

## Failures, pauses and restarts

| Event | Queued tasks | The typed task | Queue |
|---|---|---|---|
| a typed task fails (any reason) | wait | `failed` | paused `failed` |
| the process exits (`exited`) | stay queued until the pane is closed; `task.add` to it is refused | `failed`, `PROCESS_EXITED` | block cleared |
| plyd restarts with the pane open (it comes back `lost`) | wait | `failed`, `PLYD_RESTARTED` | paused `restored` |
| a `lost` pane is resumed | wait for the new process to show its prompt | — | as it was |
| a `lost` pane reopens as a shell (Ruling R50) | `cancelled`, `REOPENED_AS_SHELL` | — | flags dropped |
| the pane is closed | `cancelled`, `PANE_CLOSED` | `failed`, `PANE_CLOSED` | flags dropped |
| plyd restarts: the pane was closed | `cancelled`, `PANE_CLOSED` | `failed`, `PLYD_RESTARTED` | — |
| plyd restarts: a pool task | `cancelled`, `POOL_RESTARTED` | — | — |
| plyd is shutting down | `task.add` refused (`shutting_down`) | — | — |

A `lost` pane still takes new tasks; they wait until it is resumed and its turn
comes. A report for a task that is no longer `sent` or `running` (its pane closed
while the engine was following it) is logged and ignored
(`Registry::task_progress`).

**At startup**, `Registry::load` hands every stored task to `Queues::restore`
with the set of open panes: typed tasks fail, queued tasks of open panes wait
with their queue paused `restored`, every other queued task is cancelled, and
each queue is renumbered. The changed rows are written back and each
workspace's history pruned before any client connects. Holding a restored queue
is deliberate: after a restart the user decides whether the tasks still make
sense.

## Storage and events

**`tasks`** (schema v2, `migrations/0002_tasks.sql`) holds one row per task:
`id` (the task id, never reused), `workspace_id`, `pane_id`, `pool_cli`,
`pool_cwd`, `text`, `skill`, `state`, `position`, `detail`, `created_at`,
`sent_at`, `started_at`, `ended_at`, indexed by queue (`pane_id, state,
position`) and by end (`workspace_id, ended_at`). `Db::insert_task` assigns the
id; `update_task` writes every mutable column; `delete_tasks` removes pruned
history. plyd loads the table once at startup and keeps it in memory; pauses
and blocks are not stored.

**Every change goes through `Registry::apply_tasks`**: write the row, then
broadcast `task.changed` with the whole record, then prune the workspace if a
task finished. A failed write is logged and the change still announced. A
queue's pause or block is announced with `queue.changed` (a `QueueState`). There
is no other task event: clients rebuild their view from `task.list` on connect
and apply these two events after.

**Logs.** Every step is logged at `info` with `pane_id` and `task_id`: "the pane
took a pool task", "typing a queued task" (with `forced`), "queued task
progressed" (with the state and detail); the bytes written are at `debug`.

## The app

**State.** `state.tasks` (`TaskQueueView` in `app/src/state/reducer.ts`) holds
the workspace's tasks by id and the flagged queues by pane, and `available`,
false until `task.list` has answered and for a plyd that predates the queue
(`unknown_method`), which hides every queue control. `task.changed` upserts a
task (and prunes the app's history to 200, as plyd does); `queue.changed` sets
a pane's flags, or drops them when neither is set.

**Effects** (`app/src/state/effects.ts`). `task/add` sends `task.add` for a
pane or pool target. The form's third target, "New pane", queues nothing: it
sends `pane.create` with the text as the CLI's positional prompt argument
(into the active tab, or a new one when it is full). `task/cancel`,
`task/move`, `task/send` and `queue/pause` map one to one onto their methods;
`skills/query` sends `skill.list`.

**Views.**

| View | File | Role |
|---|---|---|
| the task form (⌘E) | `features/dispatch/dispatch-form.tsx` | target "This pane", "Next free pane" or "New pane"; the CLI and folder for the last two; the skill list for the target's CLI and directory (`skill-list.tsx`, from `skill.list`, filtered by name or invocation); the prompt. Picking a skill puts its invocation in front of what is written; the task carries `skill` only while its text still starts with it. ⌘⏎ queues, esc closes |
| the queue sheet (⌘⇧E) | `features/dispatch/queue-sheet.tsx` | every pane's typed and queued tasks, the pool, and the history; ↑↓ select, ⌥↑↓ reorder (`task.move`), ⌫ cancel, `p` pause or resume the pane, ⏎ go to the pane |
| the strip under a pane | `features/panes/queued-strip.tsx` | why its next task is not going (`queueStripView`): "you typed here" with Hold and Send now; "waits for its first prompt" with Hold; a failure with Show queue and Resume; "Queue paused" or "Held after plyd restarted" with Resume |
| the header badge | `features/panes/pane-header.tsx` | "sending", or the queued count, "held" while paused; opens the sheet |
| the top bar pill | `app/top-bar.tsx` | the workspace's queued, running and needs-you counts (`selectQueueCounts`); opens the sheet |

`taskView` in `app/src/state/selectors.ts` turns a task, its pane's status and
its queue's flags into the label every view shows: "next", "2nd"…, "held",
"waits · you typed here", "waits for the CLI's prompt", "sending", "running",
"needs you" (running while the pane waits for the user), "ended", "not
submitted", "failed", "cancelled".

## What the tests pin

| Guarantee | Test |
|---|---|
| no task before the process shows its prompt, not even `task.send`, and no busy wait meanwhile | `dispatch.rs`: `no_task_goes_before_the_process_shows_its_prompt` |
| a running, waiting or lost pane is never typed into | `a_running_pane_is_never_typed_into` |
| paste, Enter after `ENTER_DELAY`, `running` only on the adapter's acknowledgement | `the_head_is_pasted_then_entered_after_the_delay_and_waits_for_the_acknowledgement` |
| no Enter into a dialog or over the user's keys | `no_enter_once_the_pane_left_idle_or_the_user_typed_after_the_paste` |
| unsent typing blocks; only Ctrl+C, Ctrl+U or the CLI clears it; the user's Enter does not | `unsent_typing_blocks_the_queue_until_…`, `a_users_enter_never_clears_their_typing` |
| keys answering a dialog are not typing | `keys_that_answer_a_dialog_are_not_typing` |
| not submitted leaves the text counted as typed | `a_task_that_was_not_submitted_leaves_its_text_in_the_input` |
| order, positions, restart, history, the one-line `task.list`, pool claiming by folder | `queue.rs` tests |
| `task.add`'s refusals and the text limit, pausing and cancelling over the registry | `registry.rs`: `task_add_refuses_what_cannot_be_queued`, `a_workspaces_open_tasks_hold_at_most_the_text_limit`, … |
| end to end on a real plyd with fake CLIs: two tasks one per turn, the typing block and `task.send`, an unacknowledged task pausing its queue, pools, Codex before and after its first turn, a resumed Codex pane, a restart | `crates/daemon/tests/dispatch.rs` |
| the full app: two tasks through ⌘E, badge and pill, typed one per turn | journey J7, `app/e2e` (`docs/perf.md`) |
| the form, the sheet | `dispatch-form.test.tsx`, `queue-sheet.test.tsx` |

## Changing it

- **A new CLI** needs `Adapter::shows_prompt` and `Adapter::acknowledges_prompt`
  from signals the process raises itself, and a fake in
  `crates/daemon/tests/fake/` whose `submit` command reports what the pane
  submitted. If the CLI shows anything before its prompt that an Enter would
  answer, `shows_prompt` must not fire until it is past it.
- **A new reason to look at the queue** sets `check` in the machine (or sends
  `PaneCmd::Queue` from the shell); never add a periodic poll.
- **A new way to reach the pty** on behalf of the user goes through
  `on_user_input` or `on_user_paste`, or the typing block will not see it.
- **A new safety check** belongs in both places: the machine, which decides, and
  `Registry::take_task` or `next_task`, which verify under the lock.
- **Timings** are constants in `dispatch.rs`; the unit tests use them by name,
  and the real-CLI runs in `docs/perf.md` are the evidence for their values.
- A change to when or how a task is typed updates this document,
  `docs/agents.md` (**Dispatching tasks**) and, for the wire,
  `docs/control-channel.md` in the same commit.
