# Keybindings

Every key the app acts on, and every key it leaves to the terminal. The app
owns only ⌘ chords; every other key goes to the focused pane's pty, unless an
overlay or the pane's find bar is open. Both CLIs are driven from the keyboard —
Claude Code uses Esc, Esc Esc, Tab, ⇧Tab, Ctrl+A/B/C/D/J/K/R/T/V/W and
⌥B/F/D/Y/P; Codex uses Esc, ⇧Tab, most Ctrl letters, Ctrl+/ and Ctrl+] — so a
binding without ⌘ would take a key away from them.

The chords live in one place, `Workspace::key_down` in `crates/app/src/main.rs`.
An open overlay takes every key in `overlay_key` (`crates/app/src/forms.rs`), and
an open find bar the keys without ⌘ in `PaneView::find_key` (`main.rs`). The
window's menu keys are GPUI key bindings set in `main`.

## Global

These run from anywhere in the window unless an overlay is open.

| Keys | What it does |
|---|---|
| ⌘N | Opens the new-pane form for the active tab (for a new tab when there is none). In a full tab it opens nothing and the top bar says "This tab has 4 panes — ⌘T opens a new tab" for 5 s. |
| ⌘T | Opens the new-pane form for a new tab. |
| ⌘J | Focuses the next pane that needs you (`waiting_permission` or `waiting_input`) after the focused one, across every tab in tab order; nothing when none does. The top bar's "N need you" pill does the same. |
| ⌘1 – ⌘9 | Shows tab n, if there is one. |
| ⌘[ / ⌘] | Previous / next pane in this tab, wrapping. |
| ⌘← / ⌘→ / ⌘↑ / ⌘↓ | Focuses the spatially adjacent pane (Ruling R58): in columns ←/→ move and ↑/↓ do nothing; in the four-pane quadrants ←/→ move within the row and ↑/↓ within the column. No wrap-around. While zoomed, the zoom follows: the neighbour becomes the zoomed pane. |
| ⌘⇧[ / ⌘⇧] | Previous / next tab, wrapping. |
| ⌘⏎ | Zooms or unzooms the focused pane; the zoom is saved with the layout. |
| ⌘= / ⌘- / ⌘0 | Text size up or down by 1 pt within 9.5–24.5, or back to 12.5. The chrome and the terminals scale together. The app starts at the stored `font_size` and does not store a change. |
| ⌘C | Copies the focused pane's selection. With no selection it does nothing, and it never sends ^C. |
| ⌘V | Pastes the pasteboard's text into the focused pane as one PASTE (bracketed when the program enabled bracketed paste); an image on the pasteboard is saved and its path typed (`docs/terminal.md`, **Dropped files**). |
| ⌘F | Opens the find bar over the focused pane's scrollback. |
| ⌘⇧A | Opens the palette on the account rows: "Switch Claude account to …" for each account signed in to before, "Sign in to another Claude account" and "Sign in to another Codex account"; while a sign-in is open, "Go to the … sign-in" instead. The top bar's account chips do the same. When plyd could read neither sign-in it only says so. |

## Window

GPUI key bindings and the app menu:

| Keys | What it does |
|---|---|
| ⌘Q | Quits the app. plyd and every session keep running. |
| ⌘H / ⌥⌘H | Hides ply / the other apps. |
| ⌘M | Minimizes the window. |
| ⌘W | Closes the window; closing the last one quits the app. Sessions keep running. |

There is no key to close a pane, resume a lost one, open settings or show the
plan usage: plyd offers `pane.close`, `pane.resume`, `settings.set` and
`usage.get`, and the app does not offer them yet. The task queue (`task.*`) has
no key either; no app offers it today.

## The mouse

- **Tabs and panes.** A click on a tab pill shows that tab; a click in a pane
  focuses it. Dragging the top bar moves the window, and a double click there
  does what a title bar's does.
- **Selection.** Dragging with the primary button selects by cell; a double
  click selects a word, a triple click the line; ⇧-click extends. A click
  without a drag clears the selection, and so does typing into the pane.
  While the program reports the mouse, clicks and drags go to it; ⇧ selects
  anyway.
- **Links.** ⌘-click on an http or https URL opens it in the default browser,
  even while the program reports the mouse. While ⌘ is held over a link it is
  underlined and the pointer is a hand.
- **The wheel** goes to a program that reports the mouse, sends arrow keys on
  the alternate screen, and otherwise scrolls ply's own view of the scrollback.
- **Dropped files** are typed into the pane they land on as escaped paths, and
  that pane takes focus.

`docs/terminal.md` describes selection, copy, paste, search and links.

## What goes to the pty

Everything that is not a ⌘ chord: Esc, Tab, ⇧Tab, Enter, every Ctrl chord,
every ⌥ chord, bare letters and digits, function keys, arrows. Presses and
repeats are sent as KEY frames and plyd encodes them against the pane's modes
(`keys.rs`, `docs/terminal.md`). A key typed into a scrolled-back pane returns
it to the live screen. Two rules on the way:

- **⇧⏎ inserts a newline.** plyd sends LF (`0x0A`, the same as Ctrl+J, which
  both CLIs read as "insert newline") unless the pane enabled the kitty keyboard
  protocol, in which case the encoder's own report goes out.
- **The CLIs' dialogs are answered in the pane** like any key: their option
  lists differ by CLI, version and permission mode, and ply binds no key to
  them.

**⌥ types the layout's character.** The app sends the character ⌥ produced and
marks ⌥ as consumed when it changed the key's character, so layouts that type
`@ [ ] { } | ~` with ⌥ (German, Nordic and others) keep working: German ⌥L
sends `@`. plyd's encoder applies `option_as_meta` (`docs/configuration.md`);
the app has no control for it.

## The find bar

⌘F opens it in the focused pane; it takes the keys without ⌘ until it closes,
and nothing typed there reaches the pty. ⌘ chords keep working.

| Keys | What it does |
|---|---|
| typing, ⌫ (⌥⌫ a word), ⌦, ←→ | edit the query; each searches again and jumps to the newest match |
| ↑ or ⏎ | the next older match |
| ↓ | the next newer match |
| esc | closes the bar |

## Overlays

While an overlay is open it takes every key, and the global chords do nothing.
A click outside the card closes it.

| Overlay | Keys |
|---|---|
| New pane, New tab (⌘N, ⌘T) | Tab / ⇧Tab move between the fields: the CLI, Directory, Browse…, for Claude Code the worktree switch and its name, for an agent the first prompt, then Cancel and Open · ←→ choose the CLI, or 1 Claude Code, 2 Codex, 3 Shell, while the CLI field has focus · typing in Directory opens its suggestion list, ↓ opens it too: ↑↓ move, wrapping, and ⏎ takes the highlighted folder into the field · Space or ⏎ toggles the worktree switch or presses a button (Browse… opens the macOS folder picker) · ⏎ in the prompt starts a new line · ⌘V pastes into the focused field · ⌘⏎ opens the pane, with the Directory field as it reads · esc closes the list when it is open, else cancels |
| Account palette (⌘⇧A) | It opens on the query "account" · type to filter the rows · ↑↓ move, wrapping · ⏎ run · esc close |

**The Directory field searches** (Ruling R57). Its list shows at most eight
folders, each as a `~`-abbreviated path with the folder's own name emphasised
and a tag: `recent` for a folder a pane of this workspace used (its open panes
first, then its stored sessions, most recent first, only folders that still
exist, at most 50), `repo` for a git repository the background scan of the home
directory found (`docs/configuration.md` has its bounds), none for a
completion. An empty field lists the recent folders. Anything else matches the
segment after the last `/` against the folder's name, else the whole query
against the `~`-abbreviated path, case-insensitively: an exact match ranks above
a prefix, a prefix above a contiguous run and that above scattered letters; a
match in the name ranks above one in the path; a recent folder counts one step
more; and ties go to recents, then repositories, then completions, then by
recency and the shorter path. Text starting with `/`, `~` or `.` also completes:
the sub-folders of the deepest existing folder it names, matched by the segment
typed after it (hidden ones once that segment starts with `.`; a relative path
starts from the folder the form opened in). Taking a suggestion writes the
folder's path with `~` for the home directory; ⌘⏎ always opens what the field
says. Browse… opens the macOS folder picker and writes the chosen folder the
same way.

The form opens in the focused pane's directory, else the workspace's, else the
home directory, on Claude Code. A worktree needs a name. Open sends
`pane.create` and focuses the new pane; plyd's error, such as a full tab, is
shown in the form.
