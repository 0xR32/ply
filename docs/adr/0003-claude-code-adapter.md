# ADR-0003: Claude Code adapter (S2b spike)

- Status: Accepted
- Date: 2026-09-25
- Work package: WP0 (spike S2b)
- Spec version: 5.0.1 → 6.0.0 (applied by the spec-sync task; two contract changes
  — the 6.1 hook list and 6.4's progress source — make this a major bump under
  rule 0.1.2)

## Context

Spec 6.1, 6.3, 6.4, 6.5, 3.3 (C3, C7), 4.3, INV-8, INV-12, INV-14 and R8 rest on
assumptions about Claude Code's `--settings` flag, its hooks, and `--worktree` that
were marked `VERIFY S2b`. This spike drove the installed `claude` 2.1.282 (at
`~/.local/bin/claude`, signed in) through a stdlib-`pty` harness (`pty`/`os`/`select`,
no `pexpect`) in a throwaway scratch tree, never touching `~/.claude/settings.json`
or `~/.claude.json` except as Claude Code's own unavoidable session bookkeeping
writes. All hooks were injected only via `claude --settings <file>` containing
`{"theme": "dark-ansi", "hooks": {...}}`, per rule 6.1.

Every claim below is evidence-backed by a scrubbed JSON hook capture and/or a raw
pty transcript recorded during this spike. Scrubbed fixtures for WP6 are committed
at `.superpowers/plan/fixtures/s2b/*.json` (repo-relative). The raw, unscrubbed
evidence backing every claim below — `logs/`, the harness scripts (`ptyutil.py`,
`run*.py`, `hook.py`, `scrub.py`) and the `--settings` files used — is preserved at
`.superpowers/plan/evidence/s2b/` (also repo-relative; both directories are
gitignored, not part of any commit). The harness itself is throwaway per rule
0.1's "spike code is thrown away" — it is kept only as evidence, not as code WP6
should reuse.

Three results are load-bearing decisions, not just confirmations — Controller
rulings R15, R16 and R17 (below) settle them:

1. **R15 — `WorktreeCreate` is authoritative, not observational, once registered,
   so ply does not register it (or `WorktreeRemove`).** If a `WorktreeCreate` hook
   is present in the merged settings, Claude Code delegates the *entire*
   `git worktree add` to that hook and requires it to print the created
   directory's path as the last line of stdout — silence is a hard error that
   aborts the session before it starts. This directly conflicts with INV-7 ("ply
   never creates, removes or records git worktrees") and with INV-14's blanket
   "no stdout, ever" hook design. ply's worktree label instead comes from
   `CwdChanged`/`SessionStart`'s `cwd` field, per R15.
2. **R16 — `TodoWrite` and the Task tools (`TaskCreate`/`TaskUpdate`) do not exist
   in this installed build, so ply keeps both parsers but hides progress when
   neither appears.** Confirmed deterministically from the `system/init` record
   of a `stream-json` run (the directly-available `tools` list has neither),
   corroborated by two independent `ToolSearch` queries (`select:TodoWrite,
   TaskCreate,TaskUpdate,TaskList` and a semantic search for "todo task list
   tracking") both returning zero matches out of 48 deferred tools. Spec 6.4's
   Claude Code progress mechanism has no tool to hang off in 2.1.282 today, but
   per R16 the parsers stay in place for when/if it does.
3. **R17 — the `waiting_permission`/`running` state machine gets two fallback
   rows**, because no hook (silent or deciding, from any source) was found to
   actually decide a `PermissionRequest` in this build (see Decision §2 and §11):
   any key typed in a Claude `waiting_permission` pane → `running`; a Claude pane
   in `running` with a silent pty and no hook for 5 s → `idle`.

All three are detailed in the Decision section with the evidence file names.

## Decision

### 1. `--settings` merge (spec 6.1, first two bullets) — PARTLY CONFIRMED, PARTLY UNTESTED

A `--settings <file>` containing only `{"theme":"dark-ansi","hooks":{...}}` merges
with the user's real settings and does not touch `permission_mode`, the model, or
the statusline: across every run, a user's own default permission mode (this
account's is `auto`) stayed in force until a session explicitly passed
`--permission-mode`. Hook commands inherit the full `claude` process environment:
`PLY_PANE_ID` and `PLY_HOOK_SOCK` set on the child process both reached
`hook.py` unchanged (captured in every `hooks_run*.jsonl` in the evidence dir;
see `fixtures/PreToolUse.json` for the payload shape, though the env capture
itself was stripped from the scrubbed fixtures — see the raw, evidence-dir
`hooks_run1.jsonl` for the `env` field showing `PLY_PANE_ID`).

This confirms `--settings` *not clobbering* the user's settings. It does **not**
by itself demonstrate `--settings` *winning* a genuine conflict (both sources
setting the same key to different values) — ply's file never sets a key the
user's real settings also set (only `hooks`, which merge additively, and
`theme`, never independently tested against a conflicting project-level
`theme`). **This half of item 1 is untested empirically in this spike** and is
taken on Anthropic's documented settings-precedence order instead (managed >
command line/`--settings` > project local > shared project > user — code.claude.
com/docs, "Settings files and precedence"). WP6 should not treat "wins over" as
spike-verified beyond that citation.

A project `.claude/settings.json` inside a scratch project directory (standing
in for "the user's own hooks", per the brief, rather than the real `~/.claude`)
registered `PreToolUse`/`PermissionRequest`/`PostToolUse` hooks pointed at a
second script, `user_hook.py`. In the run built to check this (`run1`), ply's
hook (via `--settings`) and the project's hook both fired for the same
`PreToolUse`/`PostToolUse` events — `logs/user_hooks_run1.jsonl` contains the
same payloads (matching `session_id`) as `logs/hooks_run1.jsonl`. **This was only
shown for `PreToolUse`/`PostToolUse`** (both silent, observational hooks); it was
not, in `run1`, shown for `PermissionRequest`, the one event where "alongside"
could matter for an outcome, not just a log line. Follow-up runs (`run12`–`run15`,
below and in §2/§11) closed that gap: a *deciding* `PermissionRequest` hook
(returning `permissionDecision:"allow"` or `"deny"`) was tested alongside ply's
hook, alone, from a project-scoped file, and from the `--settings` position — in
every combination the hook fired (confirmed via `deciding_user_hook.jsonl`) but
**none influenced the outcome**: the interactive dialog appeared regardless. So
"hooks from different sources are additive" is confirmed for *execution*
(all matching hooks always run) but the practical question — can a user's own
deciding `PermissionRequest` hook actually decide anything next to ply's — turned
out to be moot in this build, because no `PermissionRequest` hook, from any
source, decides anything (see §2 and §11 for the full finding).

### 2. Silent exit-0 `PermissionRequest` leaves the prompt with the user — CONFIRMED, and stronger than expected: no hook decides it at all

With ply's `PermissionRequest` hook silent (logs, no stdout, exit 0) and
`--permission-mode manual` forcing normal prompting (see the methodology note
below on why this flag was needed), the TUI showed the ordinary "Do you want to
create b.txt?" dialog and waited for the human. Confirmed by
`fixtures/PermissionRequest.json` plus the raw dialog text captured in
`logs/run2_decoded.txt`.

**Follow-up finding (fix-round-1), stronger than the original claim**: a *silent*
hook isn't the only thing that leaves the prompt with the user — in this build,
**no `PermissionRequest` hook was found that could decide the outcome at all**.
Three separate runs each registered a `command`-type hook
(`deciding_user_hook.py`) that printed a well-formed
`{"hookSpecificOutput":{"hookEventName":"PermissionRequest","permissionDecision":
"allow"|"deny", ...}}` to stdout with exit 0 — exactly the shape the fetched
hooks-reference documentation (code.claude.com/docs, "Hooks reference" —
Decision Fields) says should decide it:
- `run12`: the deciding hook (`permissionDecision:"allow"`) in a project
  `.claude/settings.json`, registered *alongside* ply's own silent
  `PermissionRequest` hook (via `--settings`) — dialog still appeared.
- `run13`: the same deciding hook alone (ply's `--settings` file had
  `PermissionRequest` removed entirely, ruling out ply's hook as the cause) —
  dialog still appeared.
- `run14`: the same setup as `run13` but with `permissionDecision:"deny"` — dialog
  still appeared (so it isn't only "allow" that's ignored).
- `run15`: the deciding hook moved to the `--settings` (command-line) position
  instead of a project file, ruling out precedence tier as the cause — dialog
  still appeared.

In every case `deciding_user_hook.jsonl` (evidence dir) confirms the hook ran and
printed exactly that JSON; the dialog appeared regardless every time. **This
contradicts the hooks-reference doc's specific claim that "exit 2 not honored on
`PermissionRequest`; use JSON decision instead"** — the JSON-decision path was
tested here, not exit 2 (which this spike never tried), and it did not work
either. This may be a bug in 2.1.282, or the doc may describe a different/newer
version's behavior; either way, the practical conclusion for WP6 is the same:
**do not build anything — ply's own code or a promise to users — that assumes a
`PermissionRequest` hook can decide the outcome in this installed version.** This
also explains finding §11 (`PermissionDenied` never fires on a manual "No"): if
no hook can produce a decision here at all, there is nothing to fire
`PermissionDenied` from in practice, hook-programmatic denial or human denial
alike.

Note on methodology: this account's real `~/.claude/settings.json` defaults new
sessions to `permission_mode: "auto"`, which auto-approves tool calls and never
fires `PermissionRequest` at all. To exercise the dialog without editing that
file, every permission-prompt run passed the per-invocation flag
`--permission-mode manual` (a session-only override, never `--dangerously-skip-
permissions` or any flag that skips prompts — it does the opposite). Note the
flag's *own* name is `manual`, but the value that shows up in every hook
payload's `permission_mode` field for these runs is `"default"`, not
`"manual"` — a naming mismatch between the CLI flag and the wire field, not a
run that used the wrong flag (see `fixtures/PermissionRequest.json`,
`permission_mode:"default"`, produced by `--permission-mode manual`). This is a
diagnostic technique for the spike, not a recommendation that ply pass this
flag; ply per spec 6.1 passes no permission-mode flag at all and simply shows
whatever the user's own default produces (which means, for a user whose default
permission mode is `auto`, ply's amber "needs you" state may rarely trigger —
worth a note for the owner, not a spec change).

### 3. Payload shapes (spec 6.1's field list, item 3) — 10 of 14 events have a fixture; 4 do not

`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`,
`PostToolUseFailure`, `PermissionRequest`, `Notification`, `Stop`, `SessionEnd`
and `WorktreeCreate` (10 of spec 6.1's 14 named events) were each observed at
least once and have a scrubbed fixture below.

For the remaining 4, here is what exists in 2.1.282 and what was tried for each,
rather than a bare "did not fire": all four names are recognized event
identifiers in this build — `strings $(readlink -f ~/.local/bin/claude) | grep
-c -w <Event>` (read-only; evidence: `logs/run_strings_check.txt`) finds
`CwdChanged` (10 occurrences), `PermissionDenied` (20), `WorktreeRemove` (30) and
`StopFailure` (11) as literal strings in the binary, and all four were accepted
without any validation error when registered in `ply-settings.json`'s `hooks`
object (a schema that rejects unrecognized keys would have errored at startup;
it didn't). So the event names exist and are wired into the settings validator;
what's unconfirmed is only whether/when they *fire*:
- `CwdChanged` and `PermissionDenied` were deliberately tested and **confirmed
  not to fire** in the scenarios exercised (see subsections 6 and 11 — a
  negative result from real attempts, not a gap in testing effort).
- `WorktreeRemove` was **not tested** — forcing a real worktree removal is
  destructive and was judged out of scope for a spike.
- `StopFailure` was **not encountered** — no run hit a rate-limit/overload/
  auth-style error (the class of error the hooks-reference doc's `error_type`
  enum for this event names) to trigger it.

Common fields, corrected: `session_id`, `transcript_path`, `cwd` and
`hook_event_name` are present on every event observed. **`permission_mode` is
NOT universal** — it is absent from `SessionStart` and `Notification` payloads
(see `fixtures/SessionStart.json` and `fixtures/Notification_idle_prompt.json`,
neither has the key). Additional common-ish fields not in spec's list but present
on most tool/turn events: `prompt_id`, `scratchpad_dir`, `effort:{level}`.

Per-event fixtures (all under `.superpowers/plan/fixtures/s2b/`, scrubbed — real
home path, username, and the sandbox session token replaced with
`example`/`example-sandbox`):

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

### 4. `TodoWrite` / Task tools — CORRECTED: neither exists in 2.1.282; R16 (keep both parsers, hide progress when neither appears)

**Deterministic evidence first** (fix-round-1 addition, not resting on the
model's own answers): `claude -p --output-format stream-json --verbose "say
hi" --permission-mode auto`, run once in the trusted scratch project
(`run11_stream_init.sh`/`run11_stream_init.jsonl` in the evidence dir), prints a
`{"type":"system","subtype":"init",...}` record listing the 27 tools directly
available in this session, with no `ToolSearch` round-trip involved. That list
contains `Task`, `Bash`, `Edit`, `Read`, `Write`, `ToolSearch` and others — but
**no `TodoWrite`, `TaskCreate` or `TaskUpdate`**. (`Task` itself is present, but
based on its co-occurrence with `ListAgents`/`SendMessage`/`TaskStop` in that
same list, it reads as a subagent-dispatch tool, not a todo/plan tracker — this
spike did not separately verify `Task`'s own input schema.)

This is corroborated by the model's own `ToolSearch` answers, which were the
original (weaker) evidence: asking directly for `TodoWrite`
(`use TodoWrite to plan 2 steps`, `run3`) produced no `TodoWrite` tool call. The
model's own text: *"I can't use TodoWrite because it isn't available in this
session; searching the deferred tools for it found nothing."* — captured
verbatim in `fixtures/Stop.json`'s `last_assistant_message`. The tool call
behind that turn was a single `ToolSearch` invocation (`select:TodoWrite`),
logged with `"matches": []` and `"total_deferred_tools": 48`. A follow-up run
(`run4`, asking generically for "a 2-step tracked plan" rather than naming
`TodoWrite`) made two `ToolSearch` calls — a semantic query (`"todo task list
tracking"`, which surfaced the *closest* available deferred tools by name:
`CronList`, `TaskStop`, `EndConversation`, `EnterWorktree`, `SendMessage` — none
of which hold a checklist) and an exact-name query (`select:TodoWrite,
TaskCreate,TaskUpdate,TaskList`, `matches: []`). A third run (`run5`) with no
`--settings` flag at all (ruling out ply's hook config as the cause) asked
directly and got the same conclusion in the model's own words: *"There's no
TodoWrite: searching for that exact name found nothing, and a keyword search
for 'task todo plan tracking list' didn't turn up any equivalent,"*
additionally naming `EnterPlanMode`/`ExitPlanMode`, `TaskStop`, the `Cron*`
tools, and several MCP-connector `authenticate` tools as the nearest matches.

Separately, `strings` on the installed binary (read-only; `logs/
run_strings_check.txt`) finds the literal tokens `TodoWrite` (12 occurrences),
`TaskCreate` (10) and `TaskUpdate` (17) present — so the names exist *somewhere*
in the binary (plausibly leftover matcher/schema/migration strings from an
earlier or gated implementation), even though the `system/init` tool list and
every `ToolSearch` query agree neither is reachable from an ordinary session on
this account. This nuance doesn't change the practical conclusion below.

**R17 note**: this same set of runs is also the evidence base for §11's finding
that no `PermissionRequest` hook decides anything in this build (§2) — the two
findings were investigated together.

Per **R16** (controller ruling, this fix round): ply keeps both the `TodoWrite`
`PostToolUse` parser and the Task-tool parser in `crates/agents` — cheap,
forward-compatible, and ready for a different account/plan tier or a future
patch version that re-enables either — but the Claude Code pane's progress bar
is **hidden, not a permanent placeholder**, whenever neither source ever
appears for that pane's session. This is now a decided design, not an open
question for the owner.

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
- **Decided: R15 (controller ruling, this fix round)** — ply does not register
  `WorktreeCreate` or `WorktreeRemove`. `WorktreeRemove`'s exact contract was not
  independently tested here (forcing a real removal is destructive and was
  judged out of scope for a spike), but R15 applies the same rule to it by
  design, not only by the `WorktreeCreate` evidence's symmetry — WP6 does not
  need to re-litigate this, only to verify `WorktreeRemove`'s behavior stays
  consistent with it before shipping. Registering `WorktreeCreate` would force
  ply to implement real worktree creation, which is the one thing INV-7 forbids.
  The correct integration, already proven to work, is to **omit these two
  hooks** and let Claude Code's built-in default run; per R15, ply's worktree
  label comes from `CwdChanged`/`SessionStart`'s `cwd` field (in practice
  `SessionStart`, since the process is already spawned inside the worktree by
  the time hooks can run — see the next bullet) and from the branch label
  mechanism spec 6.3 already has (`git rev-parse --abbrev-ref HEAD` run against
  the pane's cwd).
- **`CwdChanged` did not fire** for the `--worktree`-induced redirection, in
  either the failing or the succeeding run. This makes sense once the mechanism
  is understood: `--worktree` changes the process's cwd *before* Claude Code
  starts, so there is nothing to observe as a "change" during the session's
  life — `SessionStart.cwd` already reads the worktree path directly. ply should
  not wait on `CwdChanged` to learn about a `--worktree` launch; it already knows
  the worktree name it asked for (spec 6.1's `--worktree <name>` argv), and
  `SessionStart.cwd` confirms where Claude Code actually put it.

### 7. Theme is per-invocation only — CONFIRMED

`~/.claude/settings.json` was **byte-identical (hash-compared, SHA-256)** before
the very first `claude` invocation of this spike and after the last one (15
`claude` invocations across `run1`–`run10`, the trust-acceptance helper `run6a`,
and the four extra `--worktree` attempts inside the `run6` family needed to work
out section 6's finding — the actual hash values are recorded, not reproduced
here, in the fix-round-1 report and are available on request; they are the real
account's machine-specific file hashes and are kept out of this committed
document per INV-11). `~/.claude.json` did **change hash** — expected and
acceptable: Claude Code writes its own recent-projects/session bookkeeping there
on every invocation, which the brief's safety rules explicitly carve out as
unavoidable. The specific claim this item exists to check — **no `theme` key
ever appears in `~/.claude.json`, before or after** — held in both snapshots
(`"theme" in d` was `False` both times). `dark-ansi` is accepted as a settings
value with no error in every run.

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

### 11. `PermissionDenied` does not fire on a manual TUI denial — R17 (two new 6.3 fallback rows)

Not one of spec 6.1's items, but load-bearing for spec 6.3's state machine. Two
isolated runs (`run7`, and a clean single-turn repeat `run8`) denied a `Write`
permission request with digit `3` ("No"). In neither run did `PermissionDenied`,
`PostToolUse`, or `PostToolUseFailure` fire for that tool call — the log jumps
straight from `PermissionRequest` to the *next* turn's `UserPromptSubmit` (run7)
or, within a 10s idle window, to nothing further at all including no `Stop`
(run8, isolated). The transcript shows the tool result as a plain "User rejected
write to b.txt" block with no hook-visible signal. §2's fix-round-1 finding
explains why: no `PermissionRequest` hook, human-triggered or
`permissionDecision`-based, was found to produce a decision signal in this
build, so `PermissionDenied` firing from *any* source is, in practice,
unreachable — this isn't a gap specific to manual denials.

**Decided: R17 (controller ruling, this fix round)** — spec 6.3 gets two new
fallback rows rather than the state machine relying on `PermissionDenied` (or
`PostToolUse`/`PostToolUseFailure`) to always follow a `PermissionRequest`:

| From | Signal | To |
|---|---|---|
| `waiting_permission` (Claude) | any key typed in the pane | `running` |
| `running` (Claude) | silent pty and no hook for 5 s | `idle` |

The first row closes exactly the gap this run demonstrates: a human "No" (or any
other keypress at the dialog, including the digits that do resolve it) always
moves the pane out of `waiting_permission`, without depending on which specific
hook event follows. The second row is the general safety net spec 6.3's table
was missing for a Claude pane that goes quiet without a clean `Stop`/`SessionEnd`
(as `run8` shows can happen) — it does not replace the specific transitions
already in the table, only catches what they miss.

## Consequences

- **ply must not register `WorktreeCreate` or `WorktreeRemove` hooks (R15).**
  Spec 6.1's hook list needs a line removed, not just a footnote — this is the
  ADR's headline correction, now a controller-decided ruling rather than an
  open recommendation. `crates/agents`' Claude launch-spec builder (WP6) must
  hard-code this omission, and a regression test should assert the generated
  `--settings` file never contains a `WorktreeCreate`/`WorktreeRemove` key.
  ply's worktree label comes from `CwdChanged`/`SessionStart.cwd` instead.
- **Claude Code progress tracking in ply has no data source in 2.1.282, and no
  `PermissionRequest` hook (ply's or a user's) was found to decide anything in
  this build either (R16, and §2/§11's fix-round-1 finding).** WP6 keeps the
  `TodoWrite`/Task-tool parsers (cheap, forward-compatible per R16) but hides
  the Claude Code pane's progress bar, rather than showing a permanent
  placeholder, whenever neither source appears. The `PermissionRequest`
  decision finding means WP6 should also not build any ply-side automation
  (or promise to users) that assumes a hook can auto-approve/auto-deny a Claude
  Code permission prompt in this installed version — the dialog is always
  interactive here, regardless of hook output.
- **The `waiting_permission`/`running` state machine gets the two R17 fallback
  rows** in §11 — decided, not an open gap: any key typed in a Claude
  `waiting_permission` pane moves it to `running`; a Claude pane in `running`
  with a silent pty and no hook for 5 s moves to `idle`. These are additive to
  spec 6.3's existing table, not a replacement of its other rows.
- **`CLAUDE_CODE_FORCE_SYNC_OUTPUT` is unverified.** WP6 should not build
  `pane.answer`/rendering logic that assumes DEC 2026 framing is active for
  Claude Code panes until it is re-tested against a pty that answers terminal
  capability queries.
- **"`--settings` wins over user settings" (spec 6.1) is untested for a genuine
  key conflict** — see §1. WP6 should treat this as resting on Anthropic's
  documented precedence order, not on this spike's own observation, until a
  real conflicting-key test is run.
- Everything else in spec 6.1 (the `--settings`-doesn't-clobber-user-settings
  half of the merge contract, the hook event list minus the two removed above,
  `--resume`, theme injection, the permission dialog's digit keys, model
  reporting via `SessionStart`) is confirmed and can be relied on as specified.

## Spec delta

Applying Controller rulings R15, R16 and R17 (this fix round) plus the S2b
confirmations/corrections. Overall bump for this ADR: **5.0.1 → 6.0.0** (two
independent major-bump changes below; rule 0.1.2 says a changed contract or
invariant is major, and both land here).

- **R15 / 6.1** "Hooks registered, all as `ply-hook claude <Event>`: ...
  WorktreeCreate, WorktreeRemove, CwdChanged." — **edit**: remove
  `WorktreeCreate` and `WorktreeRemove` from the registered-hooks list (ply
  does not register them); keep `CwdChanged`. Add a line: "ply's worktree label
  and cwd come from `CwdChanged` while a session runs, and from `SessionStart`'s
  `cwd` at launch (including a `--worktree` launch, since `--worktree` redirects
  cwd before Claude Code starts and no `CwdChanged` fires for that initial
  redirection)." **Major bump** — this removes a hook registration every later
  WP would otherwise build against, and INV-7 compliance now depends on it.
- 6.1 "The exact flag name `--worktree` and its payload fields are VERIFY S2b." —
  **confirm** the flag (`-w`/`--worktree [name]`) and the default path
  (`<repo>/.claude/worktrees/<name>`); **correct** the payload-fields premise:
  there is no safe payload to consume, because (per R15) the hook must not be
  registered. Folded into the R15 major bump above.
- 6.1 "Session id: taken from the first hook payload and stored for `--resume`.
  `--session-id` is not documented and is not used. VERIFIED" — **correct**:
  `--session-id <uuid>` *is* documented in this build's `--help`. The
  "taken from the first hook payload, used with `--resume`" half is still
  confirmed and remains ply's mechanism; only the "not documented" clause is
  wrong. Patch bump (wording).
- **R17 / 6.3** — **add** the two fallback rows from §11 to the state-machine
  table: `waiting_permission` (Claude) + any key typed in the pane → `running`;
  `running` (Claude) + silent pty and no hook for 5 s → `idle`. Additive to the
  existing table (no existing row is removed). Minor bump on its own (a new
  rule), but see the combined major bump note below.
- **R16 / 6.4** "Both shapes are supported, because Claude Code versions
  differ. VERIFY S2b" — **edit**: neither shape exists in 2.1.282, but ply
  keeps both parsers per R16. Add: "the Claude Code pane's progress bar is
  hidden, not shown as an empty placeholder, for the lifetime of a session in
  which neither `TodoWrite` nor a Task-tool call ever appears in `PostToolUse`."
  **Major bump** — this changes what WP6 can build against (progress becomes
  conditional, not guaranteed) and is a real behavior change from "always
  shown" to "shown only when a source exists."
- 6.5 "the first source that S2b confirms (hook payload or transcript record)" —
  **confirm**: the source is `SessionStart.model`. Minor bump (fills in a
  previously-open value).
- INV-14 "no stdout, exit 0" — **scoping note, no wording change needed**: this
  remains true for every hook ply registers, precisely because
  `WorktreeCreate`/`WorktreeRemove` — the only events whose contract requires
  stdout — are excluded from ply's hook set under R15.

The two **major** bumps (R15's 6.1 edit and R16's 6.4 edit) are what make this
ADR's overall version delta 5.0.1 → 6.0.0 rather than a smaller bump.

## Evidence index

Fixtures (scrubbed, committed) are under `.superpowers/plan/fixtures/s2b/`.
Everything else (raw/unscrubbed, gitignored — not part of any commit, per rule
0.1 the harness itself is throwaway) is under
`.superpowers/plan/evidence/s2b/`, both repo-relative:

- `.superpowers/plan/fixtures/s2b/*.json` — scrubbed payload samples, one per
  confirmed event shape (12 files; see section 3 above for the map).
- `.superpowers/plan/evidence/s2b/logs/hooks_run*.jsonl` — raw (unscrubbed)
  hook captures backing every claim above, including the fix-round-1 additions
  `hooks_run12.jsonl`–`hooks_run14.jsonl` and `deciding_user_hook.jsonl` (§2/§11).
- `.superpowers/plan/evidence/s2b/logs/run*_decoded.txt`,
  `.../logs/run*_raw.bin` — ANSI-stripped and raw pty transcripts, including the
  two worktree error messages quoted verbatim in section 6 and the four
  permission-decision runs (`run12`–`run15`) quoted in section 2.
- `.superpowers/plan/evidence/s2b/logs/run11_stream_init.jsonl` — the
  deterministic `system/init` tool-list capture backing section 4's primary
  evidence.
- `.superpowers/plan/evidence/s2b/logs/run_strings_check.txt` — the read-only
  `strings`-on-the-binary output backing section 3's and section 4's existence
  checks.
- `.superpowers/plan/evidence/s2b/{ptyutil.py,hook.py,user_hook.py,
  deciding_user_hook.py,scrub.py,run1..run15*.py,run11_stream_init.sh,
  ply-settings*.json}` — the spike harness itself (stdlib `pty`/`os`/`select`,
  no `pexpect`).
- Hash snapshots (fix-round-1: literal hash values removed from this document
  per INV-11; the comparison result, not the hashes, is what matters):
  `~/.claude/settings.json` byte-identical (hash-compared) across all 15
  `claude` invocations; `~/.claude.json` changed as expected (Claude Code's own
  bookkeeping) but never gained a `theme` key. The actual hash values are
  recorded in the fix-round-1 addendum to `.superpowers/sdd/ply-plan/
  task-2-report.md` for anyone who needs to re-verify byte-identity themselves.
