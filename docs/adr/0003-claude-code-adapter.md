# ADR-0003: Claude Code adapter (S2b spike)

- Status: Accepted
- Date: 2026-09-25
- Work package: WP0 (spike S2b)
- Spec version: 5.0.1 (confirms/corrects VERIFY S2b items; no version bump — WP1
  applies the deltas listed below when it commits this ADR)

## Context

Spec 6.1, 6.3, 6.4, 6.5, 3.3 (C3, C7), 4.3, INV-8, INV-12, INV-14 and R8 rest on
assumptions about Claude Code's `--settings` flag, its hooks, and `--worktree` that
were marked `VERIFY S2b`. This spike drove the installed `claude` 2.1.282 (at
`~/.local/bin/claude`, signed in) through a stdlib-`pty` harness (`pty`/`os`/`select`,
no `pexpect`) in a throwaway scratch tree, never touching `~/.claude/settings.json`
or `~/.claude.json` except as Claude Code's own unavoidable session bookkeeping
writes. All hooks were injected only via `claude --settings <file>` containing
`{"theme": "dark-ansi", "hooks": {...}}`, per rule 6.1.

Every claim below is evidence-backed by a scrubbed JSONL hook capture and/or a raw
pty transcript recorded during this spike. Scrubbed fixture candidates for WP6 are
at `.../scratchpad/s2b/fixtures/*.json` (paths below are relative to that
directory; the harness scripts (`ptyutil.py`, `run*.py`, `hook.py`) live alongside
them, one level up, and are throwaway per rule 0.1's "spike code is thrown away").

Two results are load-bearing corrections, not confirmations:

1. **`WorktreeCreate` is authoritative, not observational, once registered.** If a
   `WorktreeCreate` hook is present in the merged settings, Claude Code delegates
   the *entire* `git worktree add` to that hook and requires it to print the
   created directory's path as the last line of stdout — silence is a hard error
   that aborts the session before it starts. This directly conflicts with INV-7
   ("ply never creates, removes or records git worktrees") and with INV-14's
   blanket "no stdout, ever" hook design. Registering it exactly as spec 6.1 lists
   it would force ply to either implement real worktree creation (breaking INV-7)
   or always fail `--worktree` (breaking F-criteria that depend on worktrees).
2. **TodoWrite and the Task tools (`TaskCreate`/`TaskUpdate`) do not exist in this
   installed build.** Two independent `ToolSearch` queries (`select:TodoWrite,
   TaskCreate,TaskUpdate,TaskList` and a semantic search for "todo task list
   tracking") both returned zero matches out of 48 deferred tools, reproduced with
   and without ply's `--settings` file (control run). Spec 6.4's entire Claude Code
   progress mechanism has no tool to hang off in 2.1.282.

Both are detailed in the Decision section with the evidence file names.

## Decision

### 1. `--settings` merge (spec 6.1, first two bullets) — CONFIRMED

A `--settings <file>` containing only `{"theme":"dark-ansi","hooks":{...}}` merges
with the user's real settings and does not touch `permission_mode`, the model, or
the statusline: across every run the user's own default (`permission_mode:"auto"`
in this signed-in account) stayed in force until a session explicitly passed
`--permission-mode`. Hook commands inherit the full `claude` process environment:
`PLY_PANE_ID` and `PLY_HOOK_SOCK` set on the child process both reached
`hook.py` unchanged (captured in every `hooks_run*.jsonl`; see
`fixtures/PreToolUse.json` for the shape, though the env capture itself was
stripped from the scrubbed fixtures — see the raw `hooks_run1.jsonl` for the
`env` field showing `PLY_PANE_ID`).

A project `.claude/settings.json` inside the scratch project directory (standing
in for "the user's own hooks", per the brief, rather than the real `~/.claude`)
registered `PreToolUse`/`PermissionRequest`/`PostToolUse` hooks pointed at a
second script, `user_hook.py`. In the one run built to check this (`run1`, the
only scratch directory carrying that project-level settings file), both ply's
hook (via `--settings`) and the project's hook fired for the same events —
`logs/user_hooks_run1.jsonl` contains the same `PreToolUse`/`PostToolUse`
payloads (matching `session_id`) as `logs/hooks_run1.jsonl`. Hooks from different
sources were additive, not mutually exclusive, in this test: this **confirms**
the rule 6.1 requirement (on a single scenario — not repeated across every run).

### 2. Silent exit-0 `PermissionRequest` leaves the prompt with the user — CONFIRMED

With ply's `PermissionRequest` hook silent (logs, no stdout, exit 0) and
`--permission-mode manual` forcing normal prompting (see note below on why this
flag was needed), the TUI showed the ordinary "Do you want to create b.txt?"
dialog and waited for the human. Exit code 2 is explicitly documented as *not*
honored on `PermissionRequest` (only a JSON `permissionDecision` can decide it),
so a passive hook can never accidentally block or auto-approve. Confirmed by
`fixtures/PermissionRequest.json` plus the raw dialog text captured in
`logs/run2_decoded.txt`.

Note on methodology: this machine's real `~/.claude/settings.json` defaults new
sessions to `permission_mode: "auto"`, which auto-approves tool calls and never
fires `PermissionRequest` at all. To exercise the dialog without editing that
file, every permission-prompt run passed the per-invocation flag
`--permission-mode manual` (a session-only override, never `--dangerously-skip-
permissions` or any flag that skips prompts — it does the opposite). This is a
diagnostic technique for the spike, not a recommendation that ply pass this flag;
ply per spec 6.1 passes no permission-mode flag at all and simply shows whatever
the user's own default produces (which means, for a user like this one, ply's
amber "needs you" state may rarely trigger — worth a note for the owner, not a
spec change).

### 3. Payload shapes (spec 6.1's field list, item 3) — 10 of 14 events have a fixture; 4 do not

`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`,
`PostToolUseFailure`, `PermissionRequest`, `Notification`, `Stop`, `SessionEnd`
and `WorktreeCreate` (10 of spec 6.1's 14 named events) were each observed at
least once and have a scrubbed fixture below. Of the remaining 4: `CwdChanged`
and `PermissionDenied` were deliberately tested and **confirmed not to fire** in
the scenarios exercised (see subsections 6 and 11 — a negative result, not a gap
in testing effort); `WorktreeRemove` was **not tested** (forcing a real removal
is destructive, out of scope for a spike); `StopFailure` was **not encountered**
(no run hit a rate-limit/overload/auth-style error to trigger it).

Common fields, corrected: `session_id`, `transcript_path`, `cwd` and
`hook_event_name` are present on every event observed. **`permission_mode` is
NOT universal** — it is absent from `SessionStart` and `Notification` payloads
(see `fixtures/SessionStart.json` and `fixtures/Notification_idle_prompt.json`,
neither has the key). Additional common-ish fields not in spec's list but present
on most tool/turn events: `prompt_id`, `scratchpad_dir`, `effort:{level}`.

Per-event fixtures (all under `fixtures/`, scrubbed — real home path, username,
and the sandbox session token replaced with `example`/`example-sandbox`):

- `SessionStart.json` — fresh start (`source:"startup"`), carries `model`.
- `SessionStart_resume.json` — via `--resume` (`source:"resume"`), adds
  `seconds_since_last_response`, `context_tokens`, `prompt_cache_likely_expired`,
  `estimated_cache_write_usd`.
- `UserPromptSubmit.json` — adds `prompt`.
- `PreToolUse.json` — Write tool call, adds `tool_name`, `tool_input`,
  `tool_use_id`.
- `PostToolUse.json` — adds `tool_response`, `duration_ms`.
- `PostToolUseFailure.json` — a Read on a nonexistent file; adds `error`,
  `is_interrupt`, `duration_ms` (no `tool_response`).
- `PermissionRequest.json` — adds `permission_suggestions` (not in spec at all;
  a `setMode` suggestion offering to switch the session to `acceptEdits`).
- `Notification_permission_prompt.json` — `notification_type:"permission_prompt"`.
- `Notification_idle_prompt.json` — `notification_type:"idle_prompt"` (fired
  after Claude asked a clarifying question and had no more tool calls queued).
- `Stop.json` — adds `stop_hook_active`, `last_assistant_message`,
  `background_tasks`, `session_crons`.
- `SessionEnd.json` — adds `reason` (observed value: `"prompt_input_exit"`).
- `WorktreeCreate.json` — see the dedicated subsection; adds only `name`, no
  default path is supplied by Claude Code.

`CwdChanged` never fired in any run, including the `--worktree` run — see
subsection 6 below; no fixture exists for it.

### 4. `TodoWrite` / Task tools — CORRECTED: neither exists in 2.1.282

Asking directly for `TodoWrite` (`use TodoWrite to plan 2 steps`, `run3`) produced
no `TodoWrite` tool call. The model's own text: *"I can't use TodoWrite because
it isn't available in this session; searching the deferred tools for it found
nothing."* — captured verbatim in `fixtures/Stop.json`'s `last_assistant_message`.
The tool call behind that turn was a single `ToolSearch` invocation
(`select:TodoWrite`), logged with `"matches": []` and `"total_deferred_tools":
48`. A follow-up run (`run4`, asking generically for "a 2-step tracked plan"
rather than naming `TodoWrite`) made two `ToolSearch` calls — a semantic query
(`"todo task list tracking"`, which surfaced the *closest* available deferred
tools by name: `CronList`, `TaskStop`, `EndConversation`, `EnterWorktree`,
`SendMessage` — none of which hold a checklist) and an exact-name query
(`select:TodoWrite,TaskCreate,TaskUpdate,TaskList`, `matches: []`). A third run
(`run5`) with no `--settings` flag at all (ruling out ply's hook config as the
cause) asked directly and got the same conclusion in the model's own words:
*"There's no TodoWrite: searching for that exact name found nothing, and a
keyword search for 'task todo plan tracking list' didn't turn up any
equivalent,"* additionally naming `EnterPlanMode`/`ExitPlanMode`, `TaskStop`,
the `Cron*` tools, and several MCP-connector `authenticate` tools as the nearest
matches.

This is a hard correction to spec 6.4, which assumes `TodoWrite`'s
`tool_input.todos[]` (or `TaskCreate`/`TaskUpdate`) is the Claude Code progress
source. **In this installed version, Claude Code progress tracking has no
supported source at all.** WP6 must not assume either tool ever fires; the
adapter should watch for `PostToolUse` where `tool_name` is `"TodoWrite"` (in
case a different account/plan tier or a future patch version re-enables it) but
must degrade gracefully — Claude Code panes simply show no progress bar — when
it never appears. This is exactly the kind of forced consequence rule
`dont-infer-scope-beyond-the-source` calls out: it is named here, not silently
worked around.

### 5. Model reporting (spec 6.5, item 5) — CONFIRMED

`SessionStart.model` is populated on every session start (`"claude-opus-5-
5[1m]"` in this account/session — the exact string is a real product model
identifier, not personal data, so it is left unscrubbed). This is the "first
source S2b confirms" spec 6.5 asks the pane header to read from; no need to fall
back to the transcript.

### 6. `--worktree <name>` — CORRECTED: default path confirmed, but registering the hooks spec lists breaks it

- The flag is `-w, --worktree [name]` (confirmed via `claude --help`; optional
  value).
- **Default location confirmed exactly as spec predicted**, `<repo>/.claude/
  worktrees/<name>`, on a new branch `worktree-<name>`, as a locked git worktree
  (`git worktree list` showed `... [worktree-spike-test2] locked`) — but **only
  when no `WorktreeCreate` hook is registered**. Confirmed with a settings file
  that omits `WorktreeCreate`/`WorktreeRemove` entirely (`ply-settings-
  noworktreehook.json`): the worktree was created at the exact predicted path and
  the session started normally inside it.
- **With `WorktreeCreate` registered** (ply's full `ply-settings.json`, whose hook
  is silent per INV-14), the session refused to start at all:
  `Error creating worktree: WorktreeCreate hook failed: hook succeeded but
  returned no worktree path (command: echo the path to stdout; http/callback:
  return hookSpecificOutput.worktreePath)`. The `WorktreeCreate` payload that did
  fire (`fixtures/WorktreeCreate.json`) shows Claude Code hands the hook only
  `{session_id, transcript_path, cwd (repo root), scratchpad_dir,
  hook_event_name, name}` — no default path, no help. A follow-up test made the
  hook print a plausible default path (`<cwd>/.claude/worktrees/<name>`) to
  stdout *without* creating the directory itself: Claude Code then errored
  differently — `Error: worktree directory .../spike-test4 does not exist or is
  not a directory. The path came from a WorktreeCreate hook — the hook must
  print the directory it created as the last line of its stdout.` This proves the
  hook must perform the actual `git worktree add` (or equivalent) itself; there
  is no "just name it, I'll create it" mode.
- **Correction to spec 6.1's hook list**: ply must **not** register `WorktreeCreate`
  (and, by the identical documented failure mode for nonzero exit codes,
  almost certainly not `WorktreeRemove` either — not independently tested here,
  since forcing a real removal is destructive and out of scope for a spike; WP6
  should verify this specific point before shipping, but should design for it
  now). Doing so would force ply to implement real worktree creation, which is
  the one thing INV-7 forbids. The correct integration, already proven to work,
  is to **omit these two hooks** and let Claude Code's built-in default run;
  ply discovers the worktree's existence and path purely from the `cwd` field of
  whatever hook fires next (in practice `SessionStart`, since the process is
  already spawned inside the worktree by the time hooks can run) and from the
  branch label mechanism spec 6.3 already has (`git rev-parse --abbrev-ref HEAD`
  run against the pane's cwd).
- **`CwdChanged` did not fire** for the `--worktree`-induced redirection, in
  either the failing or the succeeding run. This makes sense once the mechanism
  is understood: `--worktree` changes the process's cwd *before* Claude Code
  starts, so there is nothing to observe as a "change" during the session's
  life — `SessionStart.cwd` already reads the worktree path directly. ply should
  not wait on `CwdChanged` to learn about a `--worktree` launch; it already knows
  the worktree name it asked for (spec 6.1's `--worktree <name>` argv), and
  `SessionStart.cwd` confirms where Claude Code actually put it.

### 7. Theme is per-invocation only — CONFIRMED

`~/.claude/settings.json` was byte-identical (SHA-256
`59e000621e74c6241ac3de9f0d038e943a4ceec7c4e8008d67a942815b5d993e`) before the
very first `claude` invocation of this spike and after the last one (15 `claude`
invocations across `run1`–`run10`, the trust-acceptance helper `run6a`, and the
four extra `--worktree` attempts inside the `run6` family needed to work out
section 6's finding). `~/.claude.json` did change hash
(`abc8bcf9...` → `dc519c43...`) — expected and acceptable: Claude Code writes its
own recent-projects/session bookkeeping there on every invocation, which the
brief's safety rules explicitly carve out as unavoidable. The specific claim
this item exists to check — **no `theme` key ever appears in `~/.claude.json`,
before or after** — held in both snapshots (`"theme" in d` was `False` both
times). `dark-ansi` is accepted as a settings value with no error in every run.

### 8. `--resume` and `CLAUDE_CODE_FORCE_SYNC_OUTPUT` — CONFIRMED / UNCONFIRMED

- `--resume <session_id>` taken from a prior run's `SessionStart.session_id`
  **worked**: the resumed session replayed the prior transcript and
  `SessionStart` fired again with `source:"resume"` (see
  `fixtures/SessionStart_resume.json`). `claude --help` also documents
  `--session-id <uuid>` (**correcting** spec 6.1's "`--session-id` is not
  documented" — it is documented in this build's `--help`, though S2b did not
  test using it to *set* a session id, only confirmed `--resume` with a
  captured id, which is what spec 6.1 actually relies on).
- `CLAUDE_CODE_FORCE_SYNC_OUTPUT=1` was set in the child env for `run1`, `run2`,
  `run3`, every `run6` family invocation, `run7`, `run8` and `run9`.
  **No `ESC[?2026h`/`ESC[?2026l` sequence was ever observed** in any raw pty
  capture at all — all 11 `logs/run*_raw.bin` files on disk were searched
  byte-for-byte (note `run6_raw.bin` only retains the last of that family's five
  invocations, since each overwrote the same path), including the runs that
  never set the flag, as a control. This env var
  is also absent from `claude --help` and from the fetched CLI/hooks docs.
  **This item is UNCONFIRMED, not disproved**: this harness never answers the
  terminal capability queries (DA1, DECRQM, etc.) the real GPUIX `<terminal>`
  element would answer via libghostty-vt (R-R4), and Claude Code may gate
  synchronized output on detecting those capabilities regardless of the "force"
  env var's name. WP6 should re-test this specific point against a pty that
  answers those queries (or against plyd directly) before relying on it.

### 9. Permission dialog keys — CONFIRMED

The dialog is:
```
Do you want to create b.txt?
❯ 1. Yes
  2. Yes, and switch to accept edits (auto-approve file edits and common file
     commands) for this session (shift+tab)
  3. No
Esc to cancel · Tab to amend
```
Digits `1`/`2`/`3` select directly (no Enter needed afterward — sending the bare
byte `"1"` or `"3"` resolved the dialog immediately in every run). `pane.answer`
writing a single digit byte (spec 4.1) is therefore correct as specified.

### 10. Minimum supported version — UNCONFIRMED, informational only

Only `2.1.282` is installed on this machine (`~/.local/share/claude/versions/`
has exactly one entry, no bundled changelog). No lower version was available to
bisect which hook events/fields were added when. **Record 2.1.282 as the floor
this ADR's evidence covers**; do not infer it is the actual minimum. If WP6 needs
a real minimum, it must come from the owner or from Anthropic's own changelog,
not from this spike.

### 11. New finding: `PermissionDenied` does not fire on a manual TUI denial

Not one of spec 6.1's items, but load-bearing for spec 6.3's state machine. Two
isolated runs (`run7`, and a clean single-turn repeat `run8`) denied a `Write`
permission request with digit `3` ("No"). In neither run did `PermissionDenied`,
`PostToolUse`, or `PostToolUseFailure` fire for that tool call — the log jumps
straight from `PermissionRequest` to the *next* turn's `UserPromptSubmit` (run7)
or, within a 10s idle window, to nothing further at all including no `Stop`
(run8, isolated). The transcript shows the tool result as a plain "User rejected
write to b.txt" block with no hook-visible signal.

**Correction to spec 6.3**: the row "`waiting_permission` | Claude `PostToolUse`,
`PostToolUseFailure` or `PermissionDenied` for the same tool call | `running`"
assumes one of those three always follows a `PermissionRequest`. It does not, for
a human "No" at the dialog. `PermissionDenied` appears to be reserved for a
*programmatic* denial (a hook returning `permissionDecision:"deny"` — ply's own
hooks never do this, per INV-14, so ply will likely never see this event from its
own hooks either). WP6's state machine needs an additional, catch-all transition
out of `waiting_permission` — e.g. on the next `PreToolUse`, `UserPromptSubmit`,
or `Stop` for the same `session_id` — so a manual denial cannot leave a pane
stuck showing "needs you" forever. This is flagged as an open risk for the owner
in the Consequences section; S2b's scope is to confirm/correct, not redesign the
state machine.

## Consequences

- **ply must not register `WorktreeCreate` or `WorktreeRemove` hooks.** Spec 6.1's
  hook list needs a line removed, not just a footnote — this is the ADR's
  headline correction. `crates/agents`' Claude launch-spec builder (WP6) must
  hard-code this omission, and a regression test should assert the generated
  `--settings` file never contains a `WorktreeCreate`/`WorktreeRemove` key.
- **Claude Code progress tracking in ply has no data source in 2.1.282.** WP6
  should still wire the `PostToolUse`/`tool_name == "TodoWrite"` watch (cheap,
  forward-compatible) but must ship the Claude Code pane's progress bar as
  "absent unless it appears" rather than a load-bearing feature. This is a
  question for the owner: F-criteria and the design canvas may need to know a
  Claude Code pane can show status but not a plan/progress bar in this version.
- **The `waiting_permission` state machine needs a manual-denial escape hatch**
  that spec 6.3 doesn't currently have (see item 11). This is a design gap for
  WP6/the owner to close, not something this spike can decide.
- **`CLAUDE_CODE_FORCE_SYNC_OUTPUT` is unverified.** WP6 should not build
  `pane.answer`/rendering logic that assumes DEC 2026 framing is active for
  Claude Code panes until it is re-tested against a pty that answers terminal
  capability queries.
- Everything else in spec 6.1 (the `--settings` merge contract, the hook event
  list minus the two removed above, `--resume`, theme injection, the permission
  dialog's digit keys, model reporting via `SessionStart`) is confirmed and can
  be relied on as specified.

## Spec delta

- 6.1 "Hooks registered, all as `ply-hook claude <Event>`: ... WorktreeCreate,
  WorktreeRemove, CwdChanged." — **correct**: remove `WorktreeCreate` and
  `WorktreeRemove` from the registered-hooks list (they must not be registered);
  keep `CwdChanged` (harmless even though it did not fire in this spike's
  scenarios — it may still fire for an in-session `cd`, not tested here).
  Major bump (changes a contract every later WP would otherwise rely on).
- 6.1 "The exact flag name `--worktree` and its payload fields are VERIFY S2b." —
  **confirm** the flag (`-w`/`--worktree [name]`) and the default path
  (`<repo>/.claude/worktrees/<name>`); **correct** the payload-fields premise:
  there is no safe payload to consume, because the hook must not be registered.
- 6.1 "Session id: taken from the first hook payload and stored for `--resume`.
  `--session-id` is not documented and is not used. VERIFIED" — **correct**:
  `--session-id <uuid>` *is* documented in this build's `--help`. The
  "taken from the first hook payload, used with `--resume`" half is still
  confirmed and remains ply's mechanism; only the "not documented" clause is
  wrong. Patch bump (wording).
- 6.3 the `waiting_permission` exit-transition row — **flag as needing a fix**,
  not itself correctable by this ADR (owner/WP6 design decision); see
  Consequences. No bump from this ADR; whoever changes 6.3 bumps it then.
- 6.4 "Both shapes are supported, because Claude Code versions differ. VERIFY
  S2b" — **correct**: neither shape exists in 2.1.282; mark the whole paragraph
  version-conditional and add the graceful-degradation requirement. Major bump
  (changes what WP6 can build against).
- 6.5 "the first source that S2b confirms (hook payload or transcript record)" —
  **confirm**: the source is `SessionStart.model`. Minor bump (fills in a
  previously-open value).
- INV-14 "no stdout, exit 0" — **flag a scoping note**: this remains true for
  every hook ply registers, precisely because `WorktreeCreate`/`WorktreeRemove`
  — the only events whose contract requires stdout — are excluded from ply's
  hook set. No wording change needed if the 6.1 list is corrected as above.

## Evidence index

All paths relative to the S2b scratch directory named in the task brief
(`.../scratchpad/s2b/`, throwaway; not committed — ask the task owner for the
current session's scratch root if these need to be re-read):

- `fixtures/*.json` — scrubbed payload samples, one per confirmed event shape
  (12 files; see section 3 above for the map).
- `logs/hooks_run*.jsonl` — raw (unscrubbed) captures backing every claim above.
- `logs/run*_decoded.txt`, `logs/run*_raw.bin` — ANSI-stripped and raw pty
  transcripts, including the two worktree error messages quoted verbatim in
  section 6.
- `ptyutil.py`, `hook.py`, `user_hook.py`, `scrub.py`, `run1..run10*.py` — the
  spike harness itself (stdlib `pty`/`os`/`select`, no `pexpect`).
- Hash snapshots: `~/.claude/settings.json` unchanged across all 15 `claude`
  invocations (`59e00062...b5d993e`); `~/.claude.json` changed as expected
  (Claude Code's own bookkeeping) but never gained a `theme` key.
