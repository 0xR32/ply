# Performance and commissioning

The evidence for the success criteria of spec 1.2 (P1–P5, F1–F4), the journeys
of spec 14 (J1–J6, and J7 for the task queue of Ruling R60), one run each of the real `claude` and `codex` through the
app, and a shortened soak: how each was run and what it measured, marked
against its target. This is work package 11 as Ruling R51 scoped it (no bundle,
Ruling R41; a 20-minute soak instead of 8 hours).

Every number below was measured on 2026-09-25 on an Apple M1 Pro (8 cores,
16 GB), macOS 26.6, with the release `plyd` and `ply-hook`
(`cargo build --release -p ply-daemon -p ply-hook`), the app from the checkout
(`bun app/src/main.tsx`) on npm GPUIX 0.10.0 as released, and Menlo as the
terminal font (Geist is not installed, see F4). The machine was shared with
parallel cargo builds and test runs (load average 3–5 on 8 cores for most runs,
9 at the start of P1), so
the frame numbers are those of a busy machine, not a quiet one.

The runs predate two later changes: the needs-you strip now answers Yes (`1`) and No (Esc) only (b0389ca), and a
tab holds at most four panes laid out as columns, then quadrants (8f51326). Where a row below names Yes/Always/No or
the main-plus-stack layout, that is what was measured then.

## Results against the targets

| ID | Criterion (spec 1.2) | Target | Measured | Verdict |
|---|---|---|---|---|
| P1 | frame time, 6 panes streaming recorded agent output at 10× | p99 ≤ 16.6 ms (60 Hz), no frame over 33 ms | GPUI draw: no frame over 15.2 ms in 60 s (the per-second p99 had a median of 6.4 ms and a worst second of 10.8 ms), 110 frames/s; main thread: busy for more than 16.7 ms at a stretch 17 times in 59 s against about 6 500 frames drawn (0.26 %), never for more than 33.3 ms (longest 32.6 ms) | met, with no margin on "no frame over 33 ms" |
| P2 | keypress to bytes written to the pty | p99 ≤ 2 ms | plyd, KEY frame → `write(2)`: p99 0.079 ms (max 0.74) with six idle panes, 0.32 ms (max 0.70) with five streaming; keystroke injected by automation → `write(2)`: p50 0.52 / p99 6.2 ms idle, p50 1.2 / p99 6.4 ms streaming | met for ply's path; the injected end-to-end is over 2 ms at p90 and p99 (see below) |
| P3 | app CPU with 6 idle panes | ≤ 0.5 % of one core | app 2.02 % over the last 5 of 10 idle minutes (1.7–2.7 % with no pane at all); plyd 0.000 % (no CPU tick in 300 s) | not met by the app; met by plyd |
| P4 | memory: app with 6 visible panes / plyd per pane with 10 000 lines of 168-column scrollback | ≤ 250 MB / ≤ 15 MB per pane | app 105 MB after 10 min idle (99 MB empty); plyd 13.0 MB with six filled panes over 2.6 MB empty = 1.73 MB per pane (1.39 MB per pane before the app attached) | met |
| P5 | tab switch with 4 panes in the new tab | first full frame ≤ 50 ms | 100 switches over 4 tabs × 4 panes, two streaming: p50 30.0, p90 35.0, max 55.7 ms (two earlier 40-switch runs with a coarser 20 ms poll: worst 40.3 and 52.1 ms) | met typically; about 1 switch in 50 is over 50 ms |
| F1 | a pane that needs permission turns amber | ≤ 250 ms after the CLI's hook fires | full app, J2: the needs-you strip painted 22–26 ms after the fake CLI was told to fire (its `sh` and `sed` included); plyd alone (WP6): about 3 ms, 3.8 ms with the real Claude Code | met |
| F2 | quitting the app leaves every agent running; reopening shows each pane's current screen | 100 % of panes | J5: after ⌘Q every agent is still `idle` under the same plyd, output written while the app was closed is on screen after reopening, 3 of 3 panes | met |
| F3 | every session is tracked and stored, visible again after an app restart and after a daemon restart | 100 % of panes | J5 (app restart) and J6 (plyd SIGKILLed: `session.list` keeps both session ids, the session-less pane as `shell`, `launch.json` records the resume); `crates/daemon/tests/resume.rs` `f3_…` checks status, times and exit codes | met |
| F4 | the terminal looks like part of the app | Geist Mono in every pane and key label; palette and surfaces from the chrome's tokens; box drawing joins | palette and surfaces come from `tokens.ts` in both (checked on screenshots of the real Claude Code and Codex panes); box drawing joins (Claude's rules, Codex's framed header); the font is Menlo, because Geist is not installed here and GPUIX cannot load a font file (Ruling R41: `just fonts`, run by the owner) | met except the font, pending the owner's `just fonts` and visual acceptance (V1) |

**P2 in detail.** ply's own path is the app encoding a KEY frame (0.002 ms p50,
0.02 ms p99, WP5), the socket, and plyd decoding it and writing the pty (the
probe above, p99 at most 0.32 ms): about a sixth of the budget. The injected
number is an upper bound that adds GPUIX's automation transport (one round trip
alone is p50 0.41 ms, p99 3.5 ms) and the wait for the app's main thread while
it draws a frame. A real key event waits for that thread too: GPUIX pumps
AppKit from a JavaScript timer every 8 ms (spec R6), so a keypress can wait up
to one pump interval before JavaScript sees it. That wait belongs to GPUIX as
released and is not measurable from outside without an OS-level key source.

**P3 in detail.** The cost is the app, not the panes: with no pane at all it
used 1.7–2.7 % of a core in four separate 60 s windows, and six idle panes add
about nothing (1.87 % with the focused cursor blinking, 1.87 % with Settings
open and nothing blinking, same run). GPUIX's frame loop calls `tick()` from an
8 ms `setTimeout`, about 125 times a second, whether or not anything changed;
GPUIX's README quotes 1.5 % for an idle paced app. plyd stays at zero: an idle
pane sends nothing and is compressed. Meeting 0.5 % needs a frame loop that
sleeps when idle, which is GPUIX's to change (ply uses it as released, INV-13).

**P5 in detail.** A switch mounts four terminal views, which open four C2
connections, receive four Snapshots and render them before the frame is drawn
(only visible panes are attached, R-R20); the median of 30 ms is that work plus
at most one 16 ms render cadence of the terminal flush. The slow tail (the worst
of each run: 40, 52, 56 ms) coincides with the two streaming panes' flushes and
a machine at load 4–5; it was not investigated further.

## Journeys J1–J7

`just e2e`, all six of J1–J6 passing in about 13 s on 2026-09-25; J7 was added with the task queue on 2026-09-26 (last run: see the list of runs at the
end). Each starts from an empty `PLY_HOME`, lets the app start plyd, and checks
what the window shows (terminal rows, status chips, strips, tab contents) and
what plyd holds (a C1 client of its own).

| Journey | What it proves |
|---|---|
| J1 first run | no plyd before; the app's launcher starts `target/{release,debug}/plyd --foreground` (no LaunchAgent written); "No panes yet"; ⌘T, the directory `~/code/ply` typed into the form, ⌘⏎: a Claude pane whose screen shows the fake's first line in that repository, the model it reported, "Your turn", and a tab named `ply` |
| J2 parallel work | Claude, Codex, Claude through the form and a shell through ⌘D in one tab; "2 claude · 1 codex · 1 sh"; a Codex OSC 9 approval and a Claude question each paint their needs-you strip within 250 ms (22–26 ms); "2 need you"; ⌘J goes to the Codex pane, to the Claude pane, and wraps; the strip's Yes button answers and the pane runs again |
| J3 many agents | two tabs of three agents; from the first tab ⌘J jumps to the second tab's waiting pane and shows that tab; a second waiting pane in the first tab makes ⌘J jump back and cycle |
| J4 terminal here | a Claude pane opened with the worktree switch on and the name `feature-x` runs `claude --worktree feature-x`, reports `<repo>/.claude/worktrees/feature-x` and shows "worktree feature-x"; ⌘D opens a shell whose directory is that worktree, and `pwd -P` typed into it prints it |
| J5 quit and return | a Claude, a Codex and a shell pane with output; ⌘Q quits the app (through the app menu); the agents stay `idle` under the same plyd while more output is written; the reopened app shows all three panes with the output from before and during the quit, no CLI was launched again |
| J6 daemon restart | plyd SIGKILLed; the app's launcher starts a new one; the Claude and Codex panes (with session ids) show "Lost" and a Resume button, the agent pane that never reported a session and the shell are back as fresh shells by themselves (R50); Resume brings Claude back with `--resume <id>` and Codex with `resume <thread>`, both idle; `session.list` keeps the ids |
| J7 task queue | a Claude pane made busy by its own prompt; two tasks queued through ⌘E (Tab to Prompt, typed, ⌘⏎); the header badge counts 2 and the top bar pill says "2 queued"; the turn ends: the fake reads "first task", then "second task", each typed only after the previous turn ended (`sent_at` after the other's `ended_at`); both `ended`; badge and pill gone |

## The real Claude Code and Codex, through the app

2026-09-25, Claude Code 2.1.282 and Codex 0.157.0 (Codex had updated itself
earlier; ply never runs it for a version), both signed in, the user's real
`HOME` and login shell, `PLY_HOME` a throwaway directory, each in its own
scratch directory. No permission-skipping flag was passed; each dialog was
answered with the needs-you strip's buttons in the app. The status times are
the `pane.status` events as a C1 client of the test received them.

**Claude Code.** Claude first asked its own folder-trust question (↓ ⏎ in the
pane) and started in its "auto" permission mode, which could have approved the
command by itself, so the mode was cycled to "manual" with ⇧⇥ inside the
session. The prompt "Run exactly this shell command and nothing else: touch
probe.txt":

| Step | Time |
|---|---|
| Enter → `running` (UserPromptSubmit) | immediately |
| `running` → `waiting_permission`, detail `Bash`, strip with Yes / Always / No | 4.73 s (the model deciding) |
| strip's Yes clicked → `running` | same millisecond as the click returned |
| `running` → `idle` (Stop) | 3.38 s; `probe.txt` exists |

Then plyd was SIGKILLed (see Codex below): the pane came back `lost`, Resume
ran `claude --resume <session id>`, and it was `idle` 0.75 s later with the
conversation on screen. `/exit` in the pane: "Exited 0".

**Codex.** Codex first asked to trust the scratch folder ("Your trust decision
will be saved"); a new folder offers no other way on, so it was trusted, and
the Enter on that screen took the R48 fast path (`running`, then `idle` 3.0 s
later with no `task_started`). The prompt "Run exactly this shell command and
nothing else: mkdir -p $HOME/Library/Caches/ply-wp11-probe" (a write outside
the sandbox's writable roots, so it needs an approval; the directory was
removed afterwards):

| Step | Time |
|---|---|
| Enter → `running` (fast path; the rollout's `task_started` follows) | immediately |
| `running` → `waiting_permission`, detail "Approval requested: /bin/zsh -lc 'mkdir -p $HOM…" (OSC 9) | 5.15 s |
| strip's Yes clicked (the digit 1, which Codex's dialog takes) → `running` | same millisecond |
| `running` → `idle` | 2.23 s, 1 ms after the rollout's `task_complete` |

**R48 against the real rollout.** The session's rollout holds `event_msg`
records `task_started` and `task_complete` for the approved turn, and
`task_started` then `turn_aborted` (with `reason: "interrupted"`) for a turn
stopped with Esc, which turned the pane `idle` 0.9 s after the record was
written. Its other records: `session_meta`, `turn_context`, `response_item`,
`event_msg` `item_completed` and `token_count`, `token_usage_record`,
`world_state` — all known to the parser. So the three names drive the status as
R48 says.

**A Codex pane lost mid-turn.** A third prompt started a turn (`task_started`
written, no `task_complete`), and plyd was SIGKILLed 3 s later. The app's
launcher started a new plyd, both panes showed "Lost", the Codex one with
"resumes with codex resume", and Resume ran `codex resume <thread>`: the pane went `starting` →
`idle` in 70 ms and stayed `idle` (watched for 20 s), with Codex itself showing
the turn as interrupted. The dangling `task_started` of the rollout's past did
not make it `running` (fix 7914e07). `/quit`: "Exited 0".

**INV-8.** SHA-256 before and after both runs: `~/.claude/settings.json`
unchanged; the `theme` key of `~/.claude.json` unchanged (the file itself is
Claude's own bookkeeping and changes by itself); `~/.codex/config.toml` changed
by one addition only, Codex saving the trust decision it asked for:
`[projects."<the scratch folder>"] trust_level = "trusted"`. No other key
(`last_updated`, `last_revision` or any other) changed. Nothing of it came from
ply: Codex received only the four `-c` overrides.

## The task queue with the real Claude Code and Codex

2026-09-26, Claude Code 2.1.283 and Codex 0.157.1, both signed in, the user's
real `HOME`, `PLY_HOME` a throwaway directory, the release plyd of 4366b4c and a
C1 client of the test's own (no app). Each task's text asked for a one-word
reply and no tools.

**Claude Code**, in an already trusted folder: a plain prompt, then the
slash-command skill of an installed plugin (`/<plugin>:<skill>`) with arguments, both queued on
an idle pane. Each was pasted (bracketed), entered 50 ms later and acknowledged
by UserPromptSubmit: `sent` → `running` in 0.08–0.1 s, `running` → `ended`
(Stop) in 1.8–2.6 s; the second was typed 1 s after the first ended. So Claude
Code takes a bracketed paste and one Enter as one prompt, slash command and
arguments included, and fires UserPromptSubmit for a skill. Again on 0d23e7c,
after the review's fixes (a task waits for its process's prompt; the user's
Enter no longer clears the typing block): two plain tasks queued on a fresh
pane, `sent` → `running` → `ended` in about 3 s and 1 s, the second typed 1 s
after the first ended.

**Codex** opened, with a first prompt, on its own "Hooks need review" screen
(hooks in `~/.codex` had changed): three choices, Enter to confirm. plyd showed
the pane `idle` from its first output byte, as the status machine does for
Codex, but the pane had reported no session, so the queued task stayed queued
and nothing was typed into the dialog (the rule of 4366b4c; before it, the
paste and Enter would have confirmed "Review hooks"). Since the review's fixes
the rule is per process: a Codex pane, fresh or resumed, takes tasks only after
a turn of its own, since a resumed pane has its stored session id before its
startup screens are answered. Trusting hooks is the user's decision, so the
run stopped there: Codex taking a queued task is shown by the fake-Codex tests
only.

## Soak

`bun app/e2e/perf.ts soak 20`, 7d7ccfd, 20 minutes: two fake Claude Code panes,
a fake Codex pane and three shells in one tab of the full app. Each agent went
through 371 full cycles (prompt, `running`, a permission request, answered with
the needs-you strip's Yes button, `running`, output, `idle`): 1 113 answered
prompts, every expected status reached within 15 s (no stuck status), the app
and plyd alive throughout and every pane `idle` at the end.

| Minute | 1 | 5 | 10 | 15 | 20 |
|---|---|---|---|---|---|
| app footprint | 145 MB | 147 MB | 147 MB | 149 MB | 155 MB |
| plyd footprint | 19 MB | 20 MB | 24 MB | 27 MB | 31 MB |
| app CPU, cumulative | 10.8 s | 48.9 s | 85.8 s | 118.8 s | 151.8 s |
| plyd CPU, cumulative | 0.6 s | 2.4 s | 5.0 s | 7.5 s | 9.9 s |

The app's memory is stable (145–155 MB, within the noise of a garbage-collected
heap). plyd's grows by about 0.6 MB a minute, which is what the scrollback of
the four slower panes costs as it fills toward its 10 000-line cap (the two fast
shells reach the cap within seconds); a 20-minute run does not show it level
off, so the 8-hour soak should confirm the plateau. At 31 MB for six live,
uncompressed panes it stays within P4's 15 MB per pane. Under this load the app
used 12.6 % of a core on average and plyd 0.8 %.

## Checked by eye in the live app

| What | Result |
|---|---|
| bell | `printf '\a'` in an unfocused shell pane puts the bell mark in its header; focusing the pane clears it |
| EXIT marking | `exit 3` in a shell pane: the status chip reads "Exited 3", and the screen stays readable; the real Claude Code and Codex quit with "Exited 0" |
| OSC 52 | `printf '\033]52;c;%s\007' aGkgZnJvbSBwbHk=` in a shell pane put "hi from ply" on the macOS pasteboard (the pasteboard was restored afterwards) |
| narrow pane header | at the default text size a stack pane shortens its title ("co…", "cla…") and its branch label and nothing overlaps; four steps larger (⌘= four times) the progress bar is drawn over the branch label, the title shrinks to nothing, a Claude pane's branch runs under its CLI and model chip, and the status bar's counts are cut off at the window's right edge |
| real CLIs | the Nerd Font icons of the user's own Claude Code status line render as missing-glyph boxes in Menlo |

## Open

- **The needs-you strip's No can approve.** With auto mode available Claude
  Code 2.1.282 shows four choices (1 Yes, 2 "Yes, and always allow access to
  <dir>", 3 "Yes, and switch to auto mode", 4 No), so the strip's No, which
  sends the digit 3, would approve the call and switch the session to auto mode.
  The strip (`app/src/features/panes/waiting-strip.tsx`) assumes three
  choices; `docs/agents.md` says the dialog answers to 1, 2 and 3. Owner's
  decision; not changed here.
- **P3**: the app idles at about 2 % of a core, set by GPUIX's always-on frame
  pump (above).
- **P5**: about one switch in fifty takes 50–56 ms.
- **P2** end to end: the key's wait for GPUIX's pump is outside what can be
  measured here and may exceed 2 ms.
- **The crowded pane header** at larger text sizes (above).
- **F4**: Geist is not installed on this machine; V1 is the owner's visual
  acceptance.
- **The 8-hour soak** with six real agent panes (spec 17) remains open; the
  20-minute soak above ran fake agents, and whether plyd's memory levels off at
  the scrollback cap is for that run to show.
- Once, in a first 10-minute idle run, the relaunched app process ended between
  minute 2 and 3 with its logs deleted along with the sandbox; three later runs
  of the same bench (and the soak) did not repeat it, and the harness now
  reports how the app exits and keeps logs with `PLY_E2E_KEEP=<dir>`.

- **J4** fails on this machine since 2026-09-26, on `main` as on the task
  queue's branch: it reads `.claude/worktrees/feature-x` right after the pane
  appears, before the fake Claude Code has created it.
- **A Codex startup screen shows as "Your turn".** Codex is `idle` from its
  first output byte, so its trust, hooks or update screens read as the user's
  turn in the header. The task queue waits there for the process's first turn
  (`blocked: startup`); the status itself is unchanged.
- **The real Codex run of the task queue** waits for the hooks review above.

## How the runs work

All of them drive the full app, `bun app/src/main.tsx`, in its own process
through GPUIX's stdio automation (`app/e2e/harness.ts`): the app starts `plyd`
itself through its launcher, exactly as `bun run dev` does, and a second C1
client of the test's own reads what plyd holds. Each run makes a throwaway
`PLY_HOME` and `HOME` under `$TMPDIR` (the real-CLI runs keep the real `HOME`),
opens the window without taking focus (`PLY_WINDOW_FOCUS=0`), and stops plyd
and deletes the directory at the end. Nothing writes to the user's
configuration (INV-8), and no LaunchAgent is installed.

| Command | Runs |
|---|---|
| `cargo build --release -p ply-daemon -p ply-hook` | the plyd the app starts |
| `cargo build -p ply-daemon` | the debug plyd, whose `--replay-feed` feeds P1 and P2 |
| `just e2e` | J1–J7 (`PLY_E2E=1 bun test ./app/e2e`, after the release build) |
| `bun app/e2e/perf.ts p1 60` | P1: six replayed panes at 10× for 60 s |
| `bun app/e2e/perf.ts p2` | P2: 300 keys with six idle panes, 300 with five of them streaming |
| `bun app/e2e/perf.ts idle 10` | P3 and P4: six filled panes, 10 minutes idle |
| `bun app/e2e/perf.ts p5 100` | P5: 100 tab switches over 4 tabs × 4 panes |
| `bun app/e2e/perf.ts soak 20` | the 20-minute soak |

The journeys need a logged-in GUI session (they open app windows, unfocused);
without `PLY_E2E=1` they are skipped, so `just test` and CI never open a window.

| Run | What it does | What it reads |
|---|---|---|
| `p1` | six shell panes in one tab; each `exec`s the debug plyd's `--replay-feed` (the feeder of `plyd --replay`) on the recorded Claude Code and Codex streams of `crates/term/tests/fixtures/`, round robin, at 10 × 4 KiB/s; 10 s warm-up, then the measurement | `PLY_TERMINAL_STATS=1` makes the app log a `frame stats` line every second (`app/src/app/frame-stats.tsx`): GPUI's draw time from GPUIX's frame overlay (p90, p99, max of that second, reset after each read) and the main thread's stalls, measured as the gaps of a 1 ms timer, which only fires between tasks |
| `p2` | six shell panes, 300 keys typed through the app 10 ms apart into one of them (automation `keystrokes`), then again while the other five replay at 10× | plyd's P2 probe (`PLY_LOG=debug`: every KEY frame logs `P2: key frame to pty write` with `latency_us`, from the data server decoding the frame to the pty writer's `write(2)` returning), and the wall-clock time from sending the keystroke to that log line |
| `idle` | the empty app for 60 s (the floor); then six shell panes, each sized 168 × 50 by a C2 attach of the bench and given `seq -f %0168.0f 1 10000` (10 000 lines of 168 columns, checked through the frame's `scrollback_rows`); then the app with all six visible, idle for 10 minutes | `ps -o time` (cumulative CPU) over the last 5 minutes for P3; `footprint -p` (the physical footprint Activity Monitor shows) at the start, after the fill and every minute for P4; per pane is plyd's growth over its no-pane start divided by six |
| `p5` | four tabs of four shell panes; each pane prints a marker (two of the sixteen print one every 10 ms, streaming); then ⌘1–⌘4 in turn, 100 times | the time from sending the chord until GPUI has painted the markers of all four panes of the new tab (`getPaintedText`, so the frame was drawn, not only committed), which includes attaching four C2 connections |
| `soak` | two fake Claude panes, a fake Codex pane and three shells in one tab; the shells loop `date`, `seq 1 3000` and `ls -la /usr/bin`; each agent cycles prompt → running → needs permission → answered through the needs-you strip's button → idle, again and again | every expected status is checked through C1 within 15 s (a miss is a stuck status); footprint and CPU of the app and plyd every minute; both processes alive throughout |

## The runs recorded here

All on 2026-09-25, Apple M1 Pro (8 cores, 16 GB), macOS 26.6; load averages as
`uptime` showed them when each run started.

| Run | plyd build | Load |
|---|---|---|
| J1–J6 (`just e2e`), 6 of 6 passing in 13.4 s | f99a7b0, the checkout's release build | about 4 |
| J1–J6 on earlier builds, 6 of 6 each time (12.5–12.7 s) | 2d601ad, 999d77f, 7d7ccfd | 3–4 |
| `idle 10` (P3, P4) | 999d77f | 4–5 |
| `p1 60` | 7d7ccfd | 9.4 falling to 4.8 |
| `p2`, `p5 40`, `p5 100` | 7d7ccfd | 4–5 |
| `soak 20` | 7d7ccfd | 3–5 |
| the real Claude Code and Codex runs, the live checks | 999d77f | 4–5 |
| J1–J7 (`just e2e`) on 2026-09-26, 6 of 7 passing (J4 failing as on `main`, see **Open**) | 4366b4c | about 3 |
| the task queue with the real Claude Code and Codex, 2026-09-26 | 4366b4c | about 3 |
| J1–J7 again, 6 of 7 (J4 as before), and the real Claude Code run again, after the review's fixes | 0d23e7c | about 3 |

Other work (cargo builds and test runs of the same checkout) ran on the machine
throughout, so these are numbers of a busy machine. A quieter one should do
better on P1, P5 and the injected P2; P3's floor does not depend on load.
