# ADR-0004: Codex adapter (S3b)

- Status: Accepted
- Date: 2026-09-25
- Work package: WP0 (spike S3b)
- Spec version: 5.0.1 → 6.0.0 (applied by the spec-sync task)

## Context

Spec sections 3.3 (C4, C8), 6.2–6.5 and WP0's S3b list what WP6's Codex adapter needs exact
strings for: the `notify` payload shape, the OSC 9 message catalogue for `classify_osc9`,
the rollout file layout and record shapes, resume/`-C`, the startup terminal probes, the
TUI's terminal modes, the approval dialog's answer keys, the `update_plan` tool, and whether
per-invocation `-c` can trust a ply hook.

Evidence was gathered two ways, both preserved:

1. **Real runs** of the installed `codex-cli 0.156.1` (`~/.local/bin/codex`), driven over a
   pty by a small Python harness (stdlib `pty`/`os`/`select`), against a **sandboxed
   `CODEX_HOME`** that reused the real `auth.json` read-only (copied once, never written
   back) so no ChatGPT login flow was needed. `~/.codex/config.toml` was never opened for
   writing by this spike; every override went through `-c`. The real `~/.codex/config.toml`
   was hash-compared before and after the spike's runs and came back **changed** — not
   because of anything this spike wrote; see the note below. Raw captures, scripts and the
   sandboxed session directory are preserved at `.superpowers/plan/evidence/s3b/` (`drive.py`,
   `notify.sh`, `hookmark.sh`, `notify.log`, `doctor.json`, `hooktest*.out`, `raw_*.bin`,
   `script_*.json`, `codex_home_sessions/`).
2. **Source reading** of a shallow clone of `openai/codex` tag `rust-v0.156.1` (resolves to
   commit `b412ff32c417f855c2b2d1581b77058eed87c84b`; git printed "not a commit" for the tag
   object itself, which is normal for an annotated tag). File:line citations below are
   against that checkout (`codex-rs/...`).

Fixtures for WP6 are at `.superpowers/plan/fixtures/s3b/` (scrubbed rollouts, notify
payloads, OSC 9 message bodies, startup-probe bytes, the approval-dialog transcript).

**Note on the config rewrite (R14, revised):** the real `~/.codex/config.toml` changed size
and hash during this spike. The cause is Codex's **plugin-marketplace auto-upgrade**, not
the binary updater. `last_updated`/`last_revision` are fields of `MarketplaceConfig`
("Last time Codex successfully added or refreshed this marketplace" / "Git revision Codex
last successfully activated for this marketplace" — `codex-rs/config/src/types.rs:1033-1038`),
and a background thread named `plugins-marketplace-auto-upgrade` refreshes any configured
marketplace on every startup whenever plugins are enabled
(`codex-rs/core-plugins/src/manager.rs:2769-2800`, gated on `config.plugins_enabled`, which
resolves to the `plugins` feature flag: `codex-rs/core/src/config/mod.rs:1671-1679`,
`self.features.enabled(Feature::Plugins)` — `plugins` is on by default, confirmed in the
enabled-feature list `doctor --json` prints). Passing `-c features.plugins.enabled=false`
(equivalently `--disable plugins`) suppresses that thread and its config rewrite. Any real
`CODEX_HOME` session — not just this spike's — can trigger it; it is unrelated to any `-c`
flag ply itself passes.

Separately, the installed binary did move from 0.156.1 to 0.157.0 during this spike
(`~/.local/bin/codex` symlinks into `~/.codex/packages/standalone/current`, and that target
changed). That is the work of a persistent, independently-running Codex app-server daemon
with its own background auto-update loop (`codex-rs/app-server-daemon/src/lib.rs`:
`auto_update_enabled`, `ensure_managed_updater`, `run_pid_update_loop`), which was already
present on this machine (its state directory predates this spike) and updates on its own
schedule. No command this spike issued calls into that machinery: `codex-rs/cli/src/main.rs`'s
`--version`/`--help`/`doctor` code paths never call `ensure_managed_updater` or
`run_pid_update_loop`. Per revised R14, ply's own spawn-time version check (a plain
`codex --version`, used to gate C7's minimum-version check) is confirmed not to trigger the
updater by the same absence of any call from that code path into the daemon's update
machinery. Separately, and already established by spec 6.2 (not new to this ADR), every
ply-spawned Codex pane carries at least one `-c` override, which spec 6.2 already documents
as making the TUI "run embedded instead of attaching to its shared background daemon" — so
ply's own panes never attach to that auto-updating daemon in the first place, by
construction, independent of the version-check point above.

No process this spike started was left running; two `exec` calls that hung (one from a
malformed `-c` array-index override, see Decision item 3) were killed by PID. A separate,
pre-existing `codex`-family process already running on this machine under an unrelated
working directory, from an unrelated session, was left untouched.

## Decision

Confirming/correcting each VERIFY S3b line:

**1. `notify` argv + payload — CONFIRMED, shape fully enumerated.**
`-c 'notify=["<script>","codex"]'` runs `<script> codex '<json>'` (the fixed `"codex"` arg
plus one JSON argument) after each completed turn (`codex-rs/hooks/src/legacy_notify.rs:45-73`,
registered in `codex-rs/hooks/src/registry.rs:125-127` on `HookEvent::AfterAgent`, which is
core, not TUI-specific — it fires the same way under `codex exec`). Captured verbatim
(scrubbed, `.superpowers/plan/fixtures/s3b/notify-payloads.log`):

```json
{"type":"agent-turn-complete","thread-id":"<uuid>","turn-id":"<uuid>","cwd":"/example/workspace","client":"codex-tui","input-messages":["create a file a.txt containing hi"],"last-assistant-message":"Created [a.txt](/example/workspace/a.txt) containing `hi`."}
```

Full field list (kebab-case): `type` (always `"agent-turn-complete"` — the enum in
`legacy_notify.rs:16` has exactly one variant), `thread-id`, `turn-id`, `cwd`, `client`
(the front-end: `"codex-tui"` or `"codex_exec"`, omitted if `None`), `input-messages`
(array), `last-assistant-message` (nullable). Spec 6.2's list ("type, thread-id, turn-id,
cwd, last-assistant-message") was missing `client` and `input-messages` — both present on
every run in this spike.

**Gotcha for WP6, corrected (R28):** Codex fires an extra, internal `AfterAgent` turn per
thread to generate a short conversation title, and that also runs `notify`. In the captured
evidence, that micro-turn's notify carries a **different `thread-id`** from the real turn
(`01a0d77c-8216…` vs the real thread's `01a0d77c-7ac9…`), arrives chronologically **first**
(07:34:19Z vs 07:34:24Z for the real turn's notify), and **no rollout file was ever created**
for that title-generation thread-id — only the real thread's rollout exists on disk. So the
risk is not "matching `turn-id` within a thread ply is already tracking" (there is no shared
thread to match within — the two notifies are for two different threads from the start): the
risk is a pane-binding strategy that trusts *whichever thread-id shows up in the first
notify it sees*. That would bind the pane to the title-generation thread, which never gets a
rollout, and progress/model/status reads that depend on the rollout would then never
resolve. The fix (R28): bind a pane to a notify's `thread-id` only once a rollout file with
that id actually exists on disk; until then, keep using the cwd-based discovery spec 6.2
already prescribes for "before the first notify" (the newest rollout created after spawn
whose `session_meta.cwd` equals the pane's cwd) — and keep using it even after a notify
arrives, if that notify's claimed thread has no rollout yet.

**2. Rollout files — CONFIRMED, layout and field names as spec states, plus exact names.**
Layout: `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<thread_uuid>.jsonl` where `<ts>` is
`2026-09-25T09-28-11` (colons become dashes; matches the glob
`rollout-*-<thread-id>.jsonl` spec 6.2 relies on for pre-notify discovery). Record shape is
`{"timestamp", "ordinal", "type", "payload"}` — `ordinal` was present on every record in
every run (spec marks it optional). `session_meta.payload` fields: `cwd` (literal key
`"cwd"`), and **two** id fields, `id` and `session_id`, both equal to the thread UUID.
`turn_context.payload.model` is the literal key `"model"` (e.g. `"gpt-6-sol"`). Approval
requests are confirmed **never** written to the rollout (every record `type` and
`payload.type` was enumerated across a run that went through an `EditApprovalRequested`
dialog — no approval-shaped record exists; the approval only shows up as the eventual
`custom_tool_call`/`custom_tool_call_output` pair and the final assistant message).
Fixtures: `.superpowers/plan/fixtures/s3b/rollout-basic-session-meta-turn-context.jsonl`,
`.superpowers/plan/fixtures/s3b/rollout-approval-and-resume-source.jsonl`.

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

An untrusted `-c`-declared hook was confirmed to be silently skipped under `codex exec` (no
error, no crash — matches expectations) using a `SessionStart` command hook pointed at a
marker script. A matching `trusted_hash` was then attempted by hand (sha256 of the compact,
key-sorted JSON, with `Option::None` fields omitted per toml-rs's usual struct-serialization
behaviour), passed back via `-c hooks.state={"<key>"={trusted_hash="sha256:..."}}` in the
same invocation. Neither candidate encoding tried trusted the hook (the marker did not
fire); the exact byte-for-byte canonicalization was not nailed down within this spike's
budget. **One pitfall found on the way:** array-index dotted paths like
`-c 'hooks.SessionStart[0].hooks[0].type="command"'` do **not** work — `-c`'s key parsing
splits on literal `.` only (`codex-rs/config/src/overrides.rs:22`), so `"SessionStart[0]"`
becomes a literal table key, not an array index; in one case this hung the process for
120s+ rather than failing cleanly (killed by PID; root cause not chased further beyond the
bogus TOML shape). Use `-c 'hooks.SessionStart=[{hooks=[{type="command",command="..."}]}]'`
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
--add-dir but no worktree option". The installed 0.156.1 binary's `--help` shows a
`--worktree` flag ("Run the session in a new managed Git worktree"), a boolean switch
(`default_value_t = false`, taking no value — `codex-rs/utils/cli/src/shared_options.rs:69-70`),
in the same struct as `-C`/`--add-dir`. This ADR does not determine whether commit 3e27195
predates or postdates the flag's addition, or whether the spec's check simply missed it —
only that the installed binary and the spec's stated conclusion disagree. This contradicts
the "Codex panes get no worktree UI" design choice that follows from the spec's conclusion —
recorded as a spec delta below, not acted on (out of this ADR's scope to redesign the
new-pane form).

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
`.superpowers/plan/fixtures/s3b/terminal-startup-probes.txt`.

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
`false`** (`codex-rs/core/src/config/mod.rs:4416-4419`, resolved with `is_some_and`), i.e.
Codex's default TUI mode is an **inline, scrollback-preserving** transcript, not a classic
full alt-screen/mouse-capturing TUI. `AltScreenMode` itself defaults to `Auto` (which *would*
enable alt screen), but `determine_alt_screen_mode` (`tui/src/lib.rs:2047-2052`) only decides
*whether alt screen is allowed*; whether the TUI ever actually asks to *own* the screen (and
only then does `enter_alt_screen()`/mouse capture run) is the separate
`tui_fullscreen_transcript` gate, and that one is off unless the user opts in. **This
directly contradicts spec 6.2's "The TUI uses the alternate screen and enables SGR mouse
modes (1000, 1002, 1003, 1006), alternate scroll (1007) ... VERIFIED".** Recorded as a spec
delta below. Practical upshot for ply: plyd's default assumption about a Codex pane's
terminal modes should be "bracketed paste + focus events only", with alt-screen/mouse/
alternate-scroll appearing only if the user has `tui.fullscreen_transcript = true` (or
whatever the equivalent ply setting ends up being) — R-R5/R-R9's mouse/kitty-key encoding
still needs to exist in plyd (a user can turn this on), but it should not be assumed live by
default for Codex the way it might be for an interactive shell.

**7. Approval dialog keys — CONFIRMED: a bare digit answers immediately, no Enter.**
Captured dialog text (apply_patch/edit approval;
`.superpowers/plan/fixtures/s3b/approval-dialog-transcript.txt`):

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
Context section's note for the mid-spike binary version move to 0.157.0. Item 6.2's version
note ("at least 0.156.1, latest stable on 2026-09-24") is otherwise unaffected by this spike.

**9. `update_plan` tool — ply passes `-c tools.update_plan.enabled=true`; the parser must
read both call shapes (R26).**
`tools.update_plan.enabled` (`ConfigToml.tools.update_plan.enabled`) defaults to **`false`**
in 0.156.1 (`resolve_update_plan_enabled`, `codex-rs/core/src/config/mod.rs:2667-2672`,
`is_some_and(|config| config.enabled)`): stock Codex never registers or calls `update_plan`
unless a config or `-c` override turns it on. Confirmed empirically: an unmodified run asked
to "use the update_plan tool" answered that the tool "isn't available in this session";
adding `-c tools.update_plan.enabled=true` made the call succeed.

The plain, unwrapped tool-call shape (used when Codex's "code_mode" tool-calling wrapper is
not in play) is a `ToolSpec::Function` named `"update_plan"`
(`codex-rs/core/src/tools/handlers/plan_spec.rs:7-53`) whose JSON Schema requires `plan`
(array of `{step: string, status: "pending"|"in_progress"|"completed"}`, both required) and
allows an optional `explanation` string. In the rollout, this shape would appear as a
`response_item` of `type: "function_call"`, `name: "update_plan"`, with `arguments` as a
genuine JSON **string** with quoted keys, e.g.
`{"plan":[{"step":"add README","status":"in_progress"},{"step":"commit","status":"pending"}]}`
— matching spec 6.4's assumption exactly.

However, the same request, captured after the environment's mid-spike move to 0.157.0 (see
Context), produced a **different** rollout shape: a `response_item` of
`type: "custom_tool_call"`, `name: "exec"`, whose `input` is a JS-like object-literal
snippet with **unquoted keys**, not JSON:
`` const r = await tools.update_plan({plan:[{step:"add README",status:"in_progress"},{step:"commit",status:"pending"}]}); text(r); `` —
Codex's "code_mode" tool-calling wrapper (`code_mode_host`, on by default per the
enabled-feature list `doctor --json` printed) routes tool calls, including `update_plan`,
through a generic `exec`-named `custom_tool_call` whose `input` embeds the call as
JavaScript rather than emitting a discrete `function_call`. Fixture (scrubbed):
`.superpowers/plan/fixtures/s3b/rollout-update-plan-code-mode.jsonl`.

Per R26: ply's plan-progress parser (spec 6.4) must read **both** shapes — the plain
`function_call` named `update_plan` with JSON `arguments`, and the `custom_tool_call` named
`exec` whose `input` contains a `tools.update_plan({...})` call with a JS object literal
(unquoted keys) — extracting the same `{plan: [{step, status}]}` data from whichever shape
is present. Whether 0.156.1 itself ever wraps `update_plan` in code_mode (as opposed to only
0.157.0, where this was actually observed) was not independently re-confirmed on a clean
0.156.1 install within this spike's budget; the parser should handle both shapes regardless,
since which one appears is a run-time routing decision (`code_mode_host`), not solely a
version gate.

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
| anything else, including the literal fallback `Agent turn complete` | `turn_complete` | `AgentTurnComplete` | **no fixed prefix** — the message is the assistant's own response text (≤200 graphemes, whitespace-normalized), verbatim, e.g. `` Created [a.txt](/example/workspace/a.txt) containing `hi`. `` (**empirically confirmed**). The fallback string `"Agent turn complete"` only appears when the response text is empty. **This is the classifier's default bucket**: any body that matches none of the five prefixes above is `turn_complete`, full stop — an assistant's final message text is unconstrained, so this is not a "leftover/unknown" case to special-case away. |

**Classifier rule (R27) — replaces C8's "unknown prefix counts as `waiting_input`".** Check
the five known, fixed prefixes above, in the order listed (they do not collide); a body
matching one of them maps to `waiting_permission` (the `approval` rows) or `waiting_input`
(the `plan_prompt`/`question` rows), per spec 6.3. A body matching **none** of them is
`turn_complete` and drives a `→ idle` transition — the same transition `notify`'s
`agent-turn-complete` already drives (6.3's existing row), so OSC 9's turn-complete signal is
a redundant confirmation of the same event, not a new state, and it is a real, reachable
outcome (it is what every ordinary completed turn produces), not a fallback. There is no
remaining "unknown prefix → `waiting_input`" case for Codex: C8's old safety net is removed
by this rule, not kept alongside it — a body that matches nothing maps to `turn_complete`,
never to `waiting_input`. (This ADR does not invent a new "truly unknown, not even
turn-complete" bucket to be safe for some future sixth Codex notification kind; if one
appears, it is a new classifier rule, not something R27 already covers.)

**Cross-reference:** per ADR-0005 (libghostty-vt, S5b), libghostty-vt's own OSC 9 parser
treats a body beginning with a bare integer (`1` through `12`) followed by `;` as a ConEmu
OSC 9 sub-command and consumes it before it would ever reach `classify_osc9` — so a
turn-complete message whose assistant text happens to start with e.g. `"1;"` is intercepted
at the terminal-emulation layer, not seen by the classifier at all. This is out of scope to
fix here (it is ADR-0005's parser, not this adapter); recorded so WP6 does not expect
`classify_osc9` to ever see such a body.

## Consequences

- WP6's Codex adapter can implement `classify_osc9` directly from the table and R27's rule
  above, and use the fixtures under `.superpowers/plan/fixtures/s3b/` as test fixtures, per
  the task brief.
- ply-agents must not bind a pane to a notify's `thread-id` until a rollout with that id
  exists on disk (R28); until then, keep using cwd-based rollout discovery, because the
  title-generation micro-turn's notify arrives first and for a thread that never gets a
  rollout (item 1).
- ply must pass `-c tools.update_plan.enabled=true` on every Codex pane (setting
  `codex_plan_tool`, default true) so `update_plan` is available at all, and the
  plan-progress parser must read both the plain `function_call` shape and the code_mode
  `custom_tool_call` "exec" JS-object-literal shape (item 9, R26).
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
- The config-rewrite cause (Context note, revised R14) is Codex's plugin-marketplace
  auto-upgrade, gated on the `plugins` feature flag (`-c features.plugins.enabled=false`
  suppresses it). INV-8's *automated* `config_untouched.rs` daemon test runs against fake
  stand-in CLIs (`tests/fake/fake-claude.sh`, `fake-codex.sh`) in a sandboxed `HOME`, per the
  spec's own file ledger — it never runs the real `codex` binary, so it cannot flake from
  this cause. The spec's separately-noted **manual** real-CLI check per adapter (recorded in
  the PR) *does* run the real binary, though, and should either pass
  `-c features.plugins.enabled=false` or simply expect `last_updated`/`last_revision` to be
  the only lines that may differ.
- ply's own spawn-time Codex version check (a plain `codex --version`, gating C7's minimum
  version) does not call into Codex's binary-update machinery and is confirmed not to trigger
  it; separately, every ply Codex pane always carries a `-c` override, which spec 6.2 already
  says keeps the TUI off the shared background daemon that the auto-updater lives behind.
- The `--worktree` correction (item 4) is left for the owner/WP6 to decide whether to revisit
  "no worktree UI for Codex panes" — this ADR only records that the premise changed.

## Spec delta

- Confirms 3.3 C4 (rollout record shape `{timestamp, ordinal?, type, payload}`, glob-based
  discovery) as written.
- **Replaces** 3.3 C8's classification clause ("message text is classified by the prefixes
  recorded in ADR-0004; an unknown prefix counts as `waiting_input`") with R27: check the
  five fixed prefixes in the table above; anything else is `turn_complete`. There is no
  "unknown prefix" outcome left for Codex — contract change, major bump.
- **Corrects** 6.2's status-signals line ("notify means turn complete. OSC 9 (C8) means
  approval requested or a question.") — OSC 9 also carries turn-complete (no fixed prefix;
  the classifier's default bucket), redundantly with `notify`. Contract change, major bump.
- **Adds** a row to 6.3's state table: `running`/`idle` + Codex OSC 9 message classified
  `turn_complete` → `idle`, alongside the existing `notify` turn-complete row (same
  transition, two sources). New rule, at least minor; grouped with the C8/6.2 changes above
  as part of the same major bump since it's the same contract.
- **Corrects** 6.2's rollout-discovery line ("after the first notify, glob
  `rollout-*-<thread-id>.jsonl`") per R28: a notify's `thread-id` only binds the pane once a
  rollout with that id exists; cwd-based discovery continues until then, since the very
  first notify for a thread can be the title-generation micro-turn, whose thread never gets a
  rollout. Contract change (binding logic), major bump alongside the above.
- **Adds** 6.4's `update_plan` handling per R26: ply passes `-c tools.update_plan.enabled=true`
  on every Codex pane (the `codex_plan_tool` setting, default true), and the parser reads
  both the plain `function_call` JSON-arguments shape and the code_mode `custom_tool_call`
  "exec" JS-object-literal shape. New rule, minor bump (grouped with the major bump above
  since it lands in the same release).
- Confirms 6.2's `notify` argv/payload shape, adds the previously-unlisted `client` and
  `input-messages` fields (new rule — minor bump).
- Confirms 6.2's `resume`/`-C` line and 6.5's "read model from turn_context" line
  (`turn_context.model`, confirmed as the literal key name).
- **Corrects** 6.2's "no worktree option" conclusion: 0.156.1 has a boolean `--worktree`
  flag. The spec's "offers no worktree for Codex" design line is now based on a stale
  premise; not changed by this ADR (out of scope), but flagged for the owner (wording/patch
  bump on that line pending a decision on whether the design itself changes, which would be
  a bigger bump).
- **Corrects** 6.2's "uses the alternate screen and enables SGR mouse modes ... VERIFIED"
  line: false by default (`tui_fullscreen_transcript` defaults off); only bracketed paste and
  focus events are on by default. Contract-relevant correction to R-R4/R-R5/R-R9's implicit
  assumption about what plyd will see from a Codex pane — minor bump for the corrected rule
  text once the owner reviews.
- Confirms 4.1's `pane.answer {choice: 1|2|3}` behaviour against Codex (bare digit, no
  newline) — extends the same mechanism spec already defines for Claude Code to Codex,
  confirming no adapter-specific special-casing is needed there.
- Corrects the note attached to INV-8's verification (14): the config-rewrite risk is
  Codex's plugin-marketplace auto-upgrade (suppressible via
  `-c features.plugins.enabled=false`), not the binary updater, and the automated
  `config_untouched.rs` test (fake CLIs, sandboxed `HOME`) is unaffected by it either way;
  only the spec's separately-noted manual real-CLI check needs to account for it.
