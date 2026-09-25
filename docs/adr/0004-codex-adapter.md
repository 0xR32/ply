# ADR-0004: Codex adapter (S3b)

- Status: Accepted
- Date: 2026-09-25
- Work package: WP0 (spike S3b)
- Spec version: 5.0.1 → 5.1.0 (new evidence/rules; no contract or invariant changes are
  proposed here — see Spec delta)

## Context

Spec sections 3.3 (C4, C8), 6.2–6.5 and WP0's S3b list six things WP6's Codex adapter needs
exact strings for: the `notify` payload shape, the OSC 9 message catalogue for
`classify_osc9`, the rollout file layout and record shapes, resume/`-C`, the startup
terminal probes, the TUI's terminal modes, and the approval dialog's answer keys, plus
whether per-invocation `-c` can trust a ply hook.

Evidence was gathered two ways, both recorded:

1. **Real runs** of the installed `codex-cli 0.156.1` (`~/.local/bin/codex`), driven over a
   pty by a small Python harness (`stdlib pty`/`os`/`select`,
   `.superpowers/sdd/ply-plan/../../../scratchpad/s3b/drive.py` — not committed, spike-only),
   against a **sandboxed `CODEX_HOME`** (`scratchpad/s3b/codex_home`) that reuses the real
   `auth.json` read-only (copied once, never written back) so no ChatGPT login flow was
   needed. `~/.codex/config.toml` was never opened for writing; every override went through
   `-c`. Hashes: `025ef1fc7456…` before the spike's first `codex` invocation,
   `8eb1a1eef70c…` after. **They differ**, and not because of anything this spike wrote — see
   the safety note below.
2. **Source reading** of a shallow clone of `openai/codex` tag `rust-v0.156.1` (resolves to
   commit `b412ff32c417f855c2b2d1581b77058eed87c84b`; git printed "not a commit" for the tag
   object itself, which is normal for an annotated tag) into `scratchpad/codex-src`. File:line
   citations below are against that checkout.

**Safety note (read before WP6 reruns any of this):** partway through this spike, invoking
the bare `codex --version` / `--help` / `doctor` (outside the sandboxed `CODEX_HOME`, because
those are discovery calls against the real install) triggered Codex's own background
self-updater: `~/.local/bin/codex` is a symlink into `~/.codex/packages/standalone/current`,
and that target moved from 0.156.1 to 0.157.0 mid-session, without any flag from this spike
asking for it. The updater also rewrote `~/.codex/config.toml`'s `last_updated`/
`last_revision` bookkeeping lines, which is why the before/after hashes differ — this spike
never opened that file for writing, and every experiment that touched config used `-c`
against a sandboxed `CODEX_HOME`. **Implication for INV-8's "byte-identical" daemon test:**
Codex can rewrite its own real config out from under a byte-identity check for reasons that
have nothing to do with ply, just from being invoked at all with an update available. The
INV-8 test should diff a fixed set of user-meaningful keys (or exclude `last_updated`/
`last_revision`), not the whole file, or it will flake independently of ply. All the
load-bearing findings below (OSC 9 catalogue, approval keys, terminal probes, modes, rollout
shapes, resume) were captured **before** the version moved, i.e. on 0.156.1 exactly. Only the
`update_plan`/code-mode-wrapping finding (Decision, item 6) was captured after, on 0.157.0;
it is flagged as such.

No process this spike started was left running; the two accidentally-hung `exec` calls
(malformed `-c` array-index syntax, see item 3) were killed by PID. An unrelated `codex`
process already running on this machine under a different working directory (a separate
harness session, PID 20526/36077/75293 group) was left untouched — it is not this spike's.

## Decision

Confirming/correcting each VERIFY S3b line:

**1. `notify` argv + payload — CONFIRMED, shape fully enumerated.**
`-c 'notify=["<script>","codex"]'` runs `<script> codex '<json>'` (the fixed `"codex"` arg
plus one JSON argument) after each completed turn (`codex-rs/hooks/src/legacy_notify.rs:45-73`,
registered in `codex-rs/hooks/src/registry.rs:125-127` on `HookEvent::AfterAgent`, which is
core, not TUI-specific — it fires the same way under `codex exec`). Captured verbatim
(scrubbed, `fixtures/notify-payloads.log`):

```json
{"type":"agent-turn-complete","thread-id":"<uuid>","turn-id":"<uuid>","cwd":"/example/workspace","client":"codex-tui","input-messages":["create a file a.txt containing hi"],"last-assistant-message":"Created [a.txt](/example/workspace/a.txt) containing `hi`."}
```

Full field list (kebab-case): `type` (always `"agent-turn-complete"` — the enum in
`legacy_notify.rs:16` has exactly one variant), `thread-id`, `turn-id`, `cwd`, `client`
(the front-end: `"codex-tui"` or `"codex_exec"`, omitted if `None`), `input-messages`
(array), `last-assistant-message` (nullable). Spec 6.2's list ("type, thread-id, turn-id,
cwd, last-assistant-message") was missing `client` and `input-messages` — both present on
every run in this spike.

**Gotcha for WP6:** Codex fires an extra, *internal* AfterAgent turn per thread to generate a
short conversation title, and that also runs `notify` — with `input-messages` being the
title-generation meta-prompt, not the user's actual message, and `last-assistant-message`
being a JSON string like `{"title":"Create a.txt with hi"}`. This happened once, on the
thread's first turn, in every interactive-TUI run in this spike. If ply-agents matches
`notify` firings to "the visible turn finished" without checking `turn-id` against the turn
it's tracking, this hidden micro-turn will fire an extra, spurious idle-transition. Recorded
in `fixtures/notify-payloads.log` (both entries, from the same run).

**2. Rollout files — CONFIRMED, layout and field names as spec states, plus exact names.**
Layout: `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<thread_uuid>.jsonl` where `<ts>` is
`2026-09-25T09-28-11` (colons become dashes; matches the glob
`rollout-*-<thread-id>.jsonl` spec 6.2 relies on for pre-notify discovery). Record shape is
`{"timestamp", "ordinal", "type", "payload"}` — `ordinal` was present on every record in
every run (spec marks it optional). `session_meta.payload` fields: `cwd` (literal key
`"cwd"`), and **two** id fields, `id` and `session_id`, both equal to the thread UUID.
`turn_context.payload.model` is the literal key `"model"` (e.g. `"gpt-6-sol"`). Approval
requests are confirmed **never** written to the rollout (grepped every record `type` and
`payload.type` across a run that went through an `EditApprovalRequested` dialog — no
approval-shaped record exists; the approval only shows up as the eventual
`custom_tool_call`/`custom_tool_call_output` pair and the final assistant message).
Fixtures: `fixtures/rollout-basic-session-meta-turn-context.jsonl`,
`fixtures/rollout-approval-and-resume-source.jsonl`.

**3. Hook trust via per-invocation `-c` — mechanism CONFIRMED from source, exact
`trusted_hash` NOT reproduced empirically; treat as open.**
`-c` overrides land in a `ConfigLayerSource::SessionFlags` layer
(`codex-rs/config/src/loader/mod.rs:409-414`), and hook trust state is read from exactly the
`User` and `SessionFlags` layers (`codex-rs/hooks/src/config_rules.rs:15-30`) — so a
`-c`-declared trust decision is real and does not touch `~/.codex/config.toml`. A hook fully
*declared* via `-c hooks.<Event>=[...]` (not a project `.codex/hooks.json`) gets a
**synthetic, deterministic** `key_source` of `<session-flags>/config.toml`
(`codex-rs/hooks/src/engine/discovery.rs:421`, `hook_metadata_for_config_layer_source` at
:823-838 marks it `HookSource::SessionFlags`, not managed), so its trust key is fully known
ahead of time: `hook_key(key_source, event, group_idx, handler_idx)` =
`"<key_source>:<event_label>:<group_idx>:<handler_idx>"` (`codex-rs/hooks/src/lib.rs:113-123`,
event labels e.g. `session_start` at :95-109). The value that must match is
`sha256:<hex>` of the canonical-JSON form of `{event_name, matcher, hooks:[handler]}`
(`codex-rs/config/src/fingerprint.rs:54-66`, `codex-rs/hooks/src/engine/discovery.rs:769-792`).

I confirmed empirically that an untrusted `-c`-declared hook is silently skipped under
`codex exec` (no error, no crash — good, matches "no-op" expectations) using a `SessionStart`
command hook pointed at a marker script. I then tried to compute the matching
`trusted_hash` myself in Python (sha256 of the compact, key-sorted JSON, with `Option::None`
fields omitted per toml-rs's usual struct-serialization behaviour) and pass it back via
`-c hooks.state={"<key>"={trusted_hash="sha256:..."}}` in the same invocation. **Both
attempts I tried did not trust the hook** (marker still did not fire) — I could not, within
this spike's budget, nail the exact byte-for-byte canonicalization (candidate:
`{"async":false,"command":"<path>","type":"command"}` under `{"event_name":"session_start","hooks":[...]}`,
all `Option::None` fields dropped, sorted keys, compact separators). **One pitfall found on
the way:** array-index dotted paths like `-c 'hooks.SessionStart[0].hooks[0].type="command"'`
do **not** work — `-c`'s key parsing splits on literal `.` only
(`codex-rs/config/src/overrides.rs:22`), so `"SessionStart[0]"` becomes a literal table key,
not an array index, and in one case this hung the process for 120s+ rather than failing
cleanly (I killed it by PID; root cause not chased further, likely the resulting bogus TOML
shape confusing something downstream). Use `-c 'hooks.SessionStart=[{hooks=[{type="command",command="..."}]}]'`
(the whole array as one TOML value) instead. **Recommendation for WP6** if hooks are ever
adopted: get the real hash by accepting it once through the TUI's interactive review
(`codex-rs/tui/src/startup_hooks_review.rs`, which calls `write_hook_trusts` — this writes
into the *user's* config today, so do this once against a disposable `CODEX_HOME`, read the
hash back out of its `hooks.state`, and hardcode/ship that hash) rather than re-deriving
`version_for_toml` bit-for-bit. Spec's "ply does not use hooks in v1" stands; this is
recorded for whoever revisits it.

**4. `resume` and `-C` — CONFIRMED.**
`codex resume <thread_uuid> "<prompt>"` (also `codex exec resume <thread_uuid> "<prompt>"`
non-interactively) resumes the same thread and appends to the same rollout file — verified
end to end (resumed session printed `session id: <same uuid>` and answered the new prompt).
`-C, --cd <DIR>` exists as a **global** flag (`codex-rs/utils/cli/src/shared_options.rs:66-68`,
doc: "Tell the agent to use the specified directory as its working root").

**Correction:** spec 6.2 says Codex's flags, "checked at 3e27195 ... include -C and
--add-dir but no worktree option". The installed 0.156.1 binary's `--help` **does** have a
`--worktree` flag ("Run the session in a new managed Git worktree"), and it's in the same
struct as `-C`/`--add-dir` (`shared_options.rs:69-70`). 3e27195 (spec's checked commit) must
predate it. This contradicts the spec's "no worktree option" conclusion and the "Codex panes
get no worktree UI" design choice that follows from it — recorded as a spec delta below, not
acted on (out of this ADR's scope to redesign the new-pane form).

**5. Startup terminal probes — CONFIRMED, exact bytes and timeout.**
Captured over a pty with nothing answering (`TERM=xterm-256color`, no real terminal), before
the first screen paint:

```
\x1b[?2004h\x1b[>4;0m\x1b[>5u\x1b[?1004h\x1b[6n\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[?u\x1b[c\x1b[?2026h...
```

i.e., in order: bracketed paste on (`?2004h`), reset xterm modifyOtherKeys (`>4;0m`), push
kitty keyboard enhancement flags (`>5u`), focus events on (`?1004h`), **CSI 6n** (cursor
position), **OSC 10** and **OSC 11** queries (default fg/bg), **CSI ?u** (kitty keyboard
support query), **DA1** (`\x1b[c`), then synchronized-output on (`?2026h`) as the first real
screen paint begins. Timeout is exactly `Duration::from_millis(250)`
(`codex-rs/tui/src/terminal_probe.rs:23`, doc comment there: "Crossterm's public helpers wait
up to two seconds ... too long for TUI startup"). On timeout: cursor position defaults to
`(0,0)` with a `tracing::warn!`, default colours stay `None` (nothing paints if plyd doesn't
answer OSC 10/11 itself), and keyboard-enhancement support defaults to `false`
(`codex-rs/tui/src/tui.rs:446-493`). Matches spec R-R4's "within 250ms" exactly. Fixture:
`fixtures/terminal-startup-probes.txt`.

**6. TUI modes — CORRECTED. Alt screen and mouse capture are OFF by default; only
bracketed paste and focus events are on.**
Across every interactive-TUI run in this spike (multiple, minutes apart), the raw pty stream
contained `?2004h` (bracketed paste) and `?1004h` (focus events) exactly once each at
startup, and **zero** occurrences of `?1049h` (alt screen), `?1000h`/`?1002h`/`?1003h`/
`?1006h` (any SGR mouse mode) or `?1007h` (alternate scroll) — checked by byte-count over the
full capture, not a snippet. Source explains why: alt-screen entry and mouse capture are both
gated behind `owned_screen`, which is only set true via `prepare_owned_screen(config.tui_fullscreen_transcript)`
at startup (`codex-rs/tui/src/app/startup.rs:197`; `captures_mouse` at `tui.rs:606-608`
returns `owned_screen` for the default overlay). `tui_fullscreen_transcript` **defaults to
`false`** (`codex-rs/core/src/config/mod.rs:4416-4419`, `resolve_update_plan_enabled`-style
`is_some_and`), i.e. Codex's default TUI mode is an **inline, scrollback-preserving**
transcript, not a classic full alt-screen/mouse-capturing TUI. `AltScreenMode` itself
defaults to `Auto` (which *would* enable alt screen), but `determine_alt_screen_mode`
(`tui/src/lib.rs:2047-2052`) only decides *whether alt screen is allowed*; whether the TUI
ever actually asks to *own* the screen (and only then does `enter_alt_screen()`/mouse capture
run) is the separate `tui_fullscreen_transcript` gate, and that one is off unless the user
opts in. **This directly contradicts spec 6.2's "The TUI uses the alternate screen and
enables SGR mouse modes (1000, 1002, 1003, 1006), alternate scroll (1007) ... VERIFIED".**
Recorded as a spec delta below. Practical upshot for ply: plyd's default assumption about a
Codex pane's terminal modes should be "bracketed paste + focus events only", with alt-screen/
mouse/alternate-scroll appearing only if the user has `tui.fullscreen_transcript = true` (or
whatever the equivalent ply setting ends up being) — R-R5/R-R9's mouse/kitty-key encoding
still needs to exist in plyd (a user can turn this on), but it should not be assumed live by
default for Codex the way it might be for an interactive shell.

**7. Approval dialog keys — CONFIRMED: a bare digit answers immediately, no Enter.**
Captured dialog text (apply_patch/edit approval; `fixtures/approval-dialog-transcript.txt`):

```
› 1. Yes, proceed (y)
  2. Yes, and don't ask again for these files (a)
  3. No, and tell Codex what to do differently (esc)

Press enter to confirm or esc to cancel
```

Sending the single byte `"1"` (no trailing `\r`) approved the edit immediately — `a.txt` was
created on the next read. This is exactly spec 4.1's `pane.answer {pane_id, choice: 1|2|3}`:
"writes that digit to the pty" — no newline needed. The letter shortcuts (`y`/`a`/`esc`) are
documented in the same row and are presumably equivalent, not tested directly. Only the
edit-approval dialog was exercised; the exec-approval dialog
(`ExecApprovalRequested`/`tool_requests.rs:287`) shares the same `ListSelectionView` widget
and its `available_decisions` can change the option list, so WP6 should read the rendered
option text rather than assume "1" is always "yes" for every approval kind.

**8. Minimum supported version — 0.156.1, as spec states; note the auto-update risk.**
The spike's environment was 0.156.1 for the great majority of the evidence above; see the
Context section's safety note for the mid-spike auto-update to 0.157.0. Item 6.2's version
note ("at least 0.156.1, latest stable on 2026-09-24") is otherwise unaffected by this spike.

**OSC 9 classification table** (for WP6's `classify_osc9`; source:
`codex-rs/tui/src/chatwidget/notifications.rs:26-71` and call sites in
`turn_runtime.rs:217,262`, `tool_requests.rs:287,330,343,458`, `questions.rs:25`,
`model_popups.rs:471`; message texts for approval/edit/plan/question empirically confirmed
where noted):

| Message text / prefix | Category | Source notification | Notes |
| --- | --- | --- | --- |
| `Approval requested: <cmd, ≤30 graphemes>` | `approval` | `ExecApprovalRequested` | shell command approval |
| `Codex wants to edit <path>` (single file) or `Codex wants to edit <n> files` | `approval` | `EditApprovalRequested` | **empirically confirmed** verbatim, incl. the trailing lone `ESC \` byte some terminals see right after the `BEL` (not part of the OSC 9 payload itself — appears to be unrelated synchronized-output bookkeeping the TUI writes right after; a byte-stream OSC parser watching for `ESC ] 9 ; ... BEL` is unaffected) |
| `Approval requested by <mcp_server_name>` | `approval` | `ElicitationRequested` | MCP tool elicitation |
| `Plan mode prompt: <title>` | `plan_prompt` | `PlanModePrompt` | fires both for the actual plan-mode implementation prompt **and** for `ToolRequestUserInput` (a generic tool-driven multi-question prompt, `tool_requests.rs:458`) — same OSC 9 text for two different underlying flows |
| `Question: <title, ≤30 graphemes>` | `question` | `AsyncQuestion` | async user-input question |
| anything else, including the literal fallback `Agent turn complete` | `turn_complete` | `AgentTurnComplete` | **no fixed prefix** — the message is the assistant's own response text (≤200 graphemes, whitespace-normalized), verbatim, e.g. `` Created [a.txt](/example/workspace/a.txt) containing `hi`. `` (**empirically confirmed**). The fallback string `"Agent turn complete"` only appears when the response text is empty. **This must be the classifier's default/last-checked bucket**, not a special case, and it must win any string that fails to match the five prefixes above — including strings that happen to start with something else entirely, since an assistant's final message text is unconstrained. |

**Resolving the C8/6.3 tension this creates:** spec 4.1's C8 row says "an unknown prefix
counts as `waiting_input`", and spec 6.3 says "Codex OSC 9 approval message → waiting_permission"
/ "Codex OSC 9 question or plan prompt → waiting_input", listing `notify` as the sole source
of the turn-complete transition. But OSC 9 **also** carries turn-complete (table above), with
no reliable prefix to key on — so "unknown prefix → waiting_input" cannot be applied
literally, or every ordinary turn-complete message would misclassify a pane as needing
input. The fix, which the table above already encodes: check the five known prefixes first,
in the order listed (longest/most specific match wins where there's any ambiguity — none of
the five actually collide); anything that matches none of them is `turn_complete`, and
`turn_complete` from OSC 9 drives the same `→ idle` transition that `notify` already drives
(6.3's row), so it's a redundant confirmation of the same signal, not a new state. There is
no remaining "truly unknown" OSC 9 message once `turn_complete` is the default bucket — spec
4.1's "unknown prefix" case in practice never occurs for Codex's own five notification kinds,
but should still exist in the implementation, mapped to `waiting_input`, purely as a safety
net for a future Codex release adding a sixth kind this ADR doesn't know about.

## Consequences

- WP6's Codex adapter can implement `classify_osc9` directly from the table above, and use
  the three scrubbed rollout fixtures and `notify-payloads.log` as test fixtures, per the
  task brief.
- ply-agents must not treat every `notify` firing as "the tracked turn finished" — match
  `turn-id` (or at least `thread-id`) against the turn plyd is actually waiting on, because
  the title-generation micro-turn fires `notify` too (item 1).
- plyd's default assumption for a fresh Codex pane's terminal modes is now "bracketed paste
  + focus events, no alt screen, no mouse capture, no alternate scroll" (item 6), which is
  *simpler* than spec 6.2 assumed, not harder — R-R5/R-R9's encoders are still needed for the
  case a user turns `tui.fullscreen_transcript` on, but plyd should not assume Codex is
  always driving the terminal like an alt-screen full TUI.
- Hook trust via `-c` (item 3) stays unresolved in the "exact hash" sense; ADR-0004 does not
  adopt Codex hooks. If a future ADR wants to, the path is: run once interactively against a
  disposable `CODEX_HOME`, accept the hook through the TUI's own review screen, and copy the
  hash it wrote — not re-derive `version_for_toml`.
- The `-c` dotted-path pitfall (array indices don't work; pass a full TOML array/table as one
  `-c` value instead) is a real footgun `ply-agents`' own `-c` construction must avoid, since
  it produced a 120s hang, not a clean error, in this spike.
- Codex's own background self-updater can rewrite `~/.codex/config.toml`'s bookkeeping lines
  as a side effect of being invoked at all (item, Context section) — INV-8's daemon test
  should scope its byte-identity check away from those lines or it will flake independently
  of anything ply does.
- The `--worktree` correction (item 4) is left for the owner/WP6 to decide whether to revisit
  "no worktree UI for Codex panes" — this ADR only records that the premise changed.

## Spec delta

- Confirms 3.3 C4 (rollout record shape `{timestamp, ordinal?, type, payload}`, glob-based
  discovery) and C8 (OSC 9 framing `ESC ] 9 ; msg BEL`, read through libghostty-vt) as
  written, with the classification table above filling in `classify_osc9`.
- Confirms 6.2's `notify` argv/payload shape, adds the previously-unlisted `client` and
  `input-messages` fields, and records the internal title-generation micro-turn as a source
  of extra `notify` firings (new rule — minor bump).
- Confirms 6.2's `resume`/`-C` line and 6.5's "read model from turn_context" line
  (`turn_context.model`, confirmed as the literal key name).
- **Corrects** 6.2's "no worktree option" conclusion: 0.156.1 has `--worktree`. The spec's
  "offers no worktree for Codex" design line is now based on a stale premise; not changed by
  this ADR (out of scope), but flagged for the owner (new evidence contradicting an existing
  VERIFIED line — treat as at least a wording/patch bump on that line pending a decision on
  whether the design itself changes, which would be a bigger bump).
- **Corrects** 6.2's "uses the alternate screen and enables SGR mouse modes ... VERIFIED"
  line: false by default (`tui_fullscreen_transcript` defaults off); only bracketed paste and
  focus events are on by default. This is a contract-relevant correction to R-R4/R-R5/R-R9's
  implicit assumption about what plyd will see from a Codex pane — recommend a minor bump for
  the corrected rule text once the owner reviews.
- Confirms 4.1's `pane.answer {choice: 1|2|3}` behaviour against Codex (bare digit, no
  newline) — extends the same mechanism spec already defines for Claude Code to Codex,
  confirming no adapter-specific special-casing is needed there.
- Adds a note to INV-8's verification (14) that the daemon's byte-identity config check must
  exclude Codex's own self-update bookkeeping fields, or scope the check to keys ply itself
  could plausibly have changed.
