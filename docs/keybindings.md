# Keybindings

Every key the app acts on, and every key it leaves to the terminal. The app
owns only ⌘ chords; every other key goes to the focused pane's pty. Both CLIs
are driven from the keyboard — Claude Code uses Esc, Esc Esc, Tab, ⇧Tab,
Ctrl+A/B/C/D/J/K/R/T/V/W and ⌥B/F/D/Y/P; Codex uses Esc, ⇧Tab, most Ctrl
letters, Ctrl+/ and Ctrl+] — so a binding without ⌘ would take a key away from
them.

Every binding is declared once, in `app/src/keymap/keymap.ts`. The reserved
chords are in `app/src/keymap/reserved.ts`, and `app/src/keymap/keymap.test.ts`
enforces the rules below.

## Global

These run from anywhere in the window, whether a pane or nothing has focus,
unless an overlay is open.

| Keys | Command | What it does |
|---|---|---|
| ⌘K | `palette.open` | Opens the command palette. |
| ⌘N | `pane.new` | Opens the new-pane form for the active tab (for a new tab when there is none). |
| ⌘T | `tab.new` | Opens the new-pane form for a new tab. |
| ⌘J | `pane.nextWaiting` | Focuses the next pane that needs you (`waiting_permission` or `waiting_input`) after the focused one, across every tab in tab order; says "Nothing needs you" when none does. |
| ⌘1 – ⌘9 | `tab.go.1` – `tab.go.9` | Shows tab n, if there is one. |
| ⌘[ / ⌘] | `pane.prev` / `pane.next` | Previous / next pane in this tab, wrapping. |
| ⌘← / ⌘→ / ⌘↑ / ⌘↓ | `pane.left` / `pane.right` / `pane.up` / `pane.down` | Focuses the spatially adjacent pane (Ruling R58): in columns ←/→ move and ↑/↓ do nothing; in the four-pane quadrants ←/→ move within the row and ↑/↓ within the column. No wrap-around, and one pane means no neighbour either way. While zoomed, the zoom follows: the neighbour becomes the zoomed pane. |
| ⌘⇧[ / ⌘⇧] | `tab.prev` / `tab.next` | Previous / next tab, wrapping. |
| ⌘⏎ | `pane.zoom` | Zooms or unzooms the focused pane. |
| ⌘D | `pane.terminalHere` | Opens a shell pane in the focused pane's working directory (the workspace's directory when no pane has focus), in this tab. |
| ⌘⇧W | `pane.close` | Closes the focused pane. A live pane asks first ("Close and stop": ⏎ stops it with `pane.close {kill:true}`, esc cancels); an exited or lost pane closes at once. |
| ⌘= / ⌘- / ⌘0 | `font.up` / `font.down` / `font.reset` | Text size up or down by 1 pt within 9.5–24.5, or back to 12.5. The chrome and the terminals scale together; the size is stored with `settings.set`. |
| ⌘, | `settings.open` | Opens Settings. |
| ⌘U, held | `usage.show` | Shows Claude Code's and Codex's plan usage for as long as it is held, and hides it on release (see **Holding ⌘U**). |

The commands are implemented in `app/src/state/reducer.ts` (`runCommand`) and,
for the ones that talk to plyd (`pane.terminalHere`, `pane.close`, and
`usage.show`'s `usage.get`), in `app/src/state/effects.ts`.

## Holding ⌘U

⌘U shows a card with one section per CLI and a row per usage window: its label
(Claude Code: "Session · 5h", "Week · all models", "Week · Opus", "Week ·
Sonnet"; Codex: "Session · 5h", "Week", with the models the sessions ran on), a
bar that turns amber over 80 % and red at 100 %, when the window resets, and
how old the numbers are ("updated 12 min ago", "as of 14:02"). A CLI without a
record says "No usage recorded yet". The numbers are the CLIs' own records on
disk, read by plyd (`usage.get`, `docs/agents.md`, **Plan usage**); the app
asks when the card opens and every 5 s while it is held. It is not an overlay:
it takes no keys and does not stop the other bindings.

The card must go the moment ⌘U is let go, but AppKit sends **no key-up for a
key released while ⌘ is down** (checked on GPUIX 0.10.0 with events posted into
the app's own queue: ⌘U's key-down and its key repeats, `isHeld`, arrive; the
key-up of `u` arrives only when ⌘ is let go first; a ⌘ change arrives as
nothing at all). So `keymap/dispatcher.ts` hides it at the first of:

- the key-up of `u` (⌘ released before `u`);
- any other key-down, a `u` without ⌘ included (⌘ released while `u` repeats);
  that key still does what it always does;
- a blur: when the window loses focus its focused element blurs, which GPUIX
  reports to the window's `onEvent`;
- the key repeats stopping. While ⌘U is held macOS repeats it, after
  `InitialKeyRepeat` and then every `KeyRepeat` (`defaults read -g`, in 15 ms
  units, read once at startup; 500 ms and 83 ms, macOS's values, when never
  set). The card hides when no repeat came within that time plus 150 ms, and
  after 2 s at most, so with key repeat turned off it shows for 2 s. Letting go
  of `u` first therefore hides it about a quarter of a second later.

While an overlay is open ⌘U does nothing, like every global binding.

## In a pane

A focused terminal handles four chords itself. Each is a standard macOS
shortcut used with its standard meaning, so it is listed as reserved too; they
are declared once in `terminalBindings`.

| Keys | Command | What it does |
|---|---|---|
| ⌘C | `terminal.copy` | Copies the selection. With no selection it does nothing, and it never sends ^C. |
| ⌘V | `terminal.paste` | Pastes the pasteboard's text, bracketed when the program enabled bracketed paste. |
| ⌘A | `terminal.selectAll` | Selects all of the pane's scrollback and screen. |
| ⌘F | `terminal.find` | Opens the find bar over the pane's scrollback. |

`docs/terminal.md` describes selection, copy, paste and search.

## Reserved

`RESERVED` in `app/src/keymap/reserved.ts`. No binding may use one of these
except for its standard meaning (only `settings.open` may bind ⌘,).

| Keys | Meaning | Owner |
|---|---|---|
| ⌘Q | Quit | GPUIX's app menu |
| ⌘H | Hide | GPUIX's app menu |
| ⌥⌘H | Hide others | GPUIX's app menu |
| ⌘M | Minimize | GPUIX's app menu |
| ⌘W | Close window; sessions keep running | GPUIX's app menu |
| ⌘` | Cycle windows | macOS |
| ⌘, | Settings | macOS (bound by `settings.open`) |
| ⌘C ⌘V ⌘A ⌘F | Copy, Paste, Select all, Find | the terminal view |
| ⌘X ⌘Z | Cut, Undo | the terminal view (unbound; a pane has nothing to cut or undo) |

**GPUIX's menu keys never reach the app.** GPUIX installs a fixed macOS app
menu that binds ⌘Q, ⌘H, ⌥⌘H, ⌘M and ⌘W as GPUI actions, which run before any
JavaScript handler. So ⌘W closes the window, not a pane — and closing the
window stops nothing, because plyd owns every session — and closing a pane is
⌘⇧W. The menu has no Edit menu, which is why the terminal view handles ⌘C, ⌘V
and ⌘A itself; in the overlays' text fields GPUIX's native `<input>` handles
them.

## What goes to the pty

Everything that is not a ⌘ chord: Esc, Tab, ⇧Tab, Enter, every Ctrl chord,
every ⌥ chord, bare letters and digits, function keys, arrows. GPUIX binds
neither Tab nor ⇧Tab, so both reach the terminal. Presses, repeats and releases
are sent as KEY frames and plyd encodes them against the pane's modes
(`docs/terminal.md`). Two rules on the way:

- **⇧⏎ inserts a newline.** plyd sends LF (`0x0A`, the same as Ctrl+J, which
  both CLIs read as "insert newline") unless the pane enabled the kitty keyboard
  protocol, in which case the encoder's own report goes out.
- **The CLIs' dialogs are answered in the pane** like any key: their option
  lists differ by CLI, version and permission mode. The needs-you strip offers
  only Yes and No, which `pane.answer` sends by meaning — plyd types `1` for Yes
  and ESC for No, so No can never approve (Ruling R55); every other option
  ("always allow", "switch to auto mode", …) is picked in the pane. The strip
  binds no key.
- **A lost pane resumes by mouse or palette.** The Resume button of the strip
  under it and the palette's "Resume <pane>" command both send `pane.resume`;
  neither binds a key.
- **A tab holds at most four panes** (Ruling R56): side by side up to three,
  quadrants at four. In a full tab ⌘N and ⌘D open nothing and the status bar
  says "This tab has 4 panes — ⌘T opens a new tab" for a few seconds; the
  palette shows New pane and Terminal here as unavailable, and the new-pane
  form never adds to a full tab. ⌘T is unaffected.
- **plyd is replaced from the palette** (Ruling R53). "Restart plyd" stops it
  without the sessions and the app starts the build in `target/`; "Quit ply and stop
  sessions" asks first, then stops every pane's process and quits
  (`docs/control-channel.md`, `daemon.shutdown`). Neither binds a key: ⌘Q
  belongs to GPUIX and quits only the window's app, leaving plyd running.

**⌥ is not Meta by default.** `option_as_meta` (Settings, "⌥ as Meta") is
`off`, `left`, `right` or `both`. With `off`, ⌥ types the layout's character,
so layouts that type `@ [ ] { } | ~` with ⌥ (German, Nordic and others) keep
working: German ⌥L sends `@`. With any other value, ⌥ plus a key sends ESC and
the key, which Claude Code's ⌥B/F/D/Y/P need. GPUIX reports no left or right
for a modifier, so `left` and `right` both apply to either ⌥ key; the setting
does reach plyd's encoder, which honours the side when one is reported.

## How a key is dispatched

GPUIX delivers a key in this order: the GPUI actions (the menu keys above),
the focused element's `onKeyDown`, its ancestors', and last the window-level
listener passed to `render()`. A JavaScript handler cannot stop the event from
travelling on, so each stage decides for itself:

1. **The terminal view** (`app/src/features/panes/terminal-view.tsx`) sends
   every key without ⌘ to plyd and never a ⌘ chord (`keyFrame` returns `null`
   for one). Of the ⌘ chords it runs only the four terminal commands, and
   ignores the rest.
2. **An open overlay's form** (palette, new pane, settings, close and quit
   confirmation) handles its own keys, listed below.
3. **The dispatcher** (`app/src/keymap/dispatcher.ts`) is the window-level
   listener. It runs only ⌘ chords, and nothing at all while an overlay is
   open — so a form's own ⌘⏎ cannot also zoom a pane. A chord that is not a
   global binding, such as ⌘C, does nothing here. It also takes the window's
   key-ups and its handled events (`onKeyUp`, `onEvent`), for the ⌘U hold
   only; `windowKeyListeners` hands all three to `render()`.

`onKeyDown` appears in exactly these places: `keymap/dispatcher.ts`,
`features/panes/terminal-view.tsx`, `features/palette/palette.tsx`,
`features/new-pane/new-pane.tsx`, `features/settings/settings.tsx`,
`features/panes/close-confirm.tsx` and `features/palette/quit-confirm.tsx`
(`checkInv4KeyHandlers` in `scripts/check-rules.ts`).

**Keystrokes** are written canonically as `[ctrl-][alt-]cmd-[shift-]<key>`, for
example `cmd-shift-]` or `cmd--`. `keysOfEvent` turns a GPUIX event into that
form: single characters are lower-cased, `return` is `enter`, and the shifted
characters AppKit reports instead of the key fold back (`}` → `cmd-shift-]`,
`{` → `cmd-shift-[`, `+` and ⇧= → `cmd-=`, `_` → `cmd--`). `keyLabel` writes
one the way the UI shows it: ⌘⇧], ⌘⏎, ⌘⌥H.

## Overlays

While an overlay is open it takes the keys; global chords do nothing.

| Overlay | Keys |
|---|---|
| Palette (⌘K) | Type to filter commands, tabs and panes · ↑↓ move, wrapping · ⏎ run · esc close · a digit 1–9 on an empty query shows that tab |
| New pane, New tab (⌘N, ⌘T) | ←→ choose the CLI, or 1 Claude Code, 2 Codex, 3 Shell while the CLI field has focus · typing in Directory opens its suggestion list: ↑↓ move, wrapping · ⏎ or a click takes the highlighted folder into the field and closes the list · Tab / ⇧Tab move between fields (Directory, then Browse…), closing the list · Space or ⏎ toggles the worktree switch or presses a button (Browse… opens the folder picker) · ⌘⏎ open, with the Directory field as it reads · esc closes the list when it is open, else cancels |
| Settings (⌘,) | Tab / ⇧Tab move between fields · ←→ change the accent or ⌥ as Meta · Space or ⏎ toggles a switch · esc close |
| Close confirmation (⌘⇧W on a live pane) | ⏎ close and stop · esc cancel |
| Quit confirmation ("Quit ply and stop sessions") | ⏎ quit and stop · esc cancel |
| Unsafe paste (in a pane) | ⏎ paste anyway · esc cancel · any other key cancels the paste and is typed as usual |
| Find bar (⌘F in a pane) | Type to search · ↑ or ⏎ older match · ↓ newer match · esc close; nothing typed here reaches the pty |

**The Directory field searches** (Ruling R57). Its list shows at most eight
folders, each as a `~`-abbreviated path with the folder's own name emphasised and
a tag: `recent` for a folder a pane of this workspace used (its open panes and
its stored sessions, most recent first, only folders that still exist), `repo`
for a git repository the background scan of the home directory found
(`docs/configuration.md` has its bounds), none for a completion. An empty field
lists the recent folders. Anything else matches as a case-insensitive fuzzy
subsequence of the `~`-abbreviated path: a match in the folder's name ranks
above a match elsewhere in the path; within each, an exact match ranks above a
prefix, a prefix above a contiguous run and that above scattered letters; a
recent folder counts one step more; and ties go to recents, then repositories,
then by recency and the shorter path. Text starting with `/`, `~` or `.` also
completes: the sub-folders of the deepest existing folder it names, matched by
the segment typed after it (hidden ones once that segment starts with `.`; `.`
and `..` start from the folder the form opened in). Taking a suggestion writes
the folder's absolute path, with `~` for the home directory; ⌘⏎ always opens
what the field says. Browse… opens the macOS folder picker and writes the
chosen folder the same way.

## Adding a binding

1. Add the command id to `CommandId` in `app/src/state/actions.ts` and handle
   it in `runCommand` (`app/src/state/reducer.ts`), or in `effects.ts` if it
   talks to plyd.
2. Add one entry to `bindings` in `app/src/keymap/keymap.ts`: the canonical
   keystroke, the command and its label. The footer, the palette and the key
   hints take the label from there.
3. Add the command to the list in `keymap.test.ts` (`SPEC_7_2`), which must
   match `bindings` exactly.
4. Run `bun test app/src/keymap`. The test fails when:
   - a command is bound twice, or not at all;
   - two bindings share a keystroke, or a keystroke is not in canonical form;
   - a chord has no ⌘, or has ⌃ or ⌥ (a key the terminal needs);
   - a chord hits `RESERVED` with another meaning than its standard one.
5. A chord the terminal view should handle itself goes into
   `terminalBindings` instead; it must be a reserved chord owned by `terminal`,
   used with that meaning, and must not also be a global binding.

Never bind Esc, Tab, ⇧Tab, Enter, a Ctrl or ⌥ chord or a bare key, and never
add a second `onKeyDown` outside the files listed above.
