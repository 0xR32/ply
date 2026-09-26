//! Domain records C1 carries: panes, workspaces, tabs, layouts, sessions, the terminal palette, settings, the
//! CLIs' plan usage, the task queue (Ruling R60) and the skills installed for each CLI (Ruling R61).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use ts_rs::TS;

/// Unique id of a pane: C1 names panes by it and C2 ATTACH carries it; ids stay below 2^53 so TypeScript numbers hold them.
pub type PaneId = u64;

/// Most panes one tab holds (Ruling R56): up to three side by side, four as quadrants; plyd refuses a fifth.
pub const MAX_PANES_PER_TAB: usize = 4;

/// Unix time in whole seconds, UTC.
pub type UnixSeconds = u64;

/// The program a pane runs (schema v1 `panes.cli`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Cli {
    /// Claude Code (`claude`).
    Claude,
    /// Codex CLI (`codex`).
    Codex,
    /// The user's login shell.
    Shell,
}

/// The CLIs that send C3 hook and notify payloads; a shell pane has no hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentCli {
    /// Claude Code hooks (`ply-hook claude <Event>`).
    Claude,
    /// Codex's notify program (`ply-hook codex`).
    Codex,
}

/// Pane state per the spec 6.3 machine, a plain string on the wire; for `Exited` the code travels in `exit_code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PaneStatus {
    /// Spawned; the CLI has not reported readiness yet.
    Starting,
    /// Waiting for the user's next prompt ("your turn"; "done" when every plan item is complete).
    Idle,
    /// The agent is working.
    Running,
    /// The CLI shows a permission dialog ("needs you").
    WaitingPermission,
    /// The CLI asks a question or shows a plan prompt ("needs you").
    WaitingInput,
    /// The process ended; `Pane::exit_code` holds its code.
    Exited,
    /// The process vanished across a plyd restart; the pane can be resumed when it has a `session_ref`.
    Lost,
}

/// Plan progress reported by the agent (spec 6.4): `done <= total`; `current` is the in-progress item's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct Progress {
    /// Completed items.
    pub done: u32,
    /// All items.
    pub total: u32,
    /// Text of the item in progress, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
}

/// A live or finished pane as plyd tracks it; `id` is the C2 `pane_id` the terminal view attaches with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct Pane {
    /// Pane id, equal to the C2 ATTACH `pane_id`.
    pub id: PaneId,
    /// Workspace the pane belongs to.
    pub workspace_id: u64,
    /// Tab the pane sits in.
    pub tab_id: u64,
    /// 0-based place in its tab: columns left to right up to three, quadrants in reading order at four (R56).
    pub position: u32,
    /// Program the pane runs.
    pub cli: Cli,
    /// Working directory as last reported (OSC 7 or the CLI's hook `cwd`), absolute.
    pub cwd: String,
    /// Title shown in the pane header (the terminal title when set, else the CLI name).
    pub title: String,
    /// Current state.
    pub status: PaneStatus,
    /// One line on the current state, e.g. the tool a permission prompt is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Plan progress; absent when the agent reports no plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    /// Model name as the session reports it; ply never chooses it (spec 6.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_seen: Option<String>,
    /// Worktree name the CLI reports for the pane's cwd; display and session record only (INV-7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_seen: Option<String>,
    /// Git branch of `cwd` (display only, never stored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The project `cwd` belongs to: the folder name of its repository (a linked worktree's main one), else of `cwd`; display only, never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The linked worktree `cwd` is in, by its folder name, as git reports it; display only, never stored (INV-7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_worktree: Option<String>,
    /// The CLI's own session id (Claude `session_id`, Codex thread id), used to resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    /// Exit code, present exactly when `status` is `exited`; a signal death is reported as 128 + signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// When the pane was created.
    pub created_at: UnixSeconds,
    /// When the pane was closed; absent while it is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<UnixSeconds>,
    /// Last pty output or hook event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<UnixSeconds>,
}

/// A directory ply opened; one default workspace (the home directory) exists after first run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    /// Workspace id.
    pub id: u64,
    /// Absolute path, unique across workspaces.
    pub path: String,
    /// Display name.
    pub name: String,
    /// When it was last opened.
    pub opened_at: UnixSeconds,
}

/// One tab of a workspace with its pane order, focus and zoom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct Tab {
    /// Tab id.
    pub id: u64,
    /// Display name (the basename of its first pane's directory unless renamed).
    pub name: String,
    /// 0-based order in the tab bar.
    pub position: u32,
    /// Panes in position order, at most [`MAX_PANES_PER_TAB`].
    pub pane_ids: Vec<PaneId>,
    /// Pane that has focus in this tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_pane_id: Option<PaneId>,
    /// Whether the focused pane fills the tab.
    pub zoomed: bool,
}

/// The tab arrangement of one workspace, as `layout.get` returns it and `layout.save` stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct Layout {
    /// Tabs in any order; `Tab::position` orders them.
    pub tabs: Vec<Tab>,
    /// Tab shown on open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tab_id: Option<u64>,
}

/// The stored record of a pane, kept after it closes (F3); `session.list` returns these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct Session {
    /// Id of the pane that ran the session.
    pub pane_id: PaneId,
    /// Workspace of that pane.
    pub workspace_id: u64,
    /// Program it ran.
    pub cli: Cli,
    /// Its last working directory.
    pub cwd: String,
    /// Its last title.
    pub title: String,
    /// Its last state.
    pub status: PaneStatus,
    /// The CLI's session id, if one was reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    /// Model the session reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_seen: Option<String>,
    /// Worktree the CLI reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_seen: Option<String>,
    /// Exit code once exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Start time.
    pub created_at: UnixSeconds,
    /// Close time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<UnixSeconds>,
    /// Last activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<UnixSeconds>,
}

/// An opaque sRGB colour, `"#RRGGBB"` on the wire (either case accepted, upper case written).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, TS)]
#[ts(type = "string")]
pub struct Rgb {
    /// Red, 0–255.
    pub r: u8,
    /// Green, 0–255.
    pub g: u8,
    /// Blue, 0–255.
    pub b: u8,
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let hex = s
            .strip_prefix('#')
            .filter(|h| h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| de::Error::custom(format!("expected a #RRGGBB colour, got {s:?}")))?;
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(de::Error::custom);
        Ok(Self {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
        })
    }
}

/// The terminal palette (`theme.set`, spec R-R4); camelCase keys to match the terminal view's `theme` prop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalTheme {
    /// ANSI colours 0–15; exactly 16 entries or the message is rejected (a 16-tuple in TypeScript).
    #[ts(
        type = "[string, string, string, string, string, string, string, string, string, string, string, string, string, string, string, string]"
    )]
    pub ansi: [Rgb; 16],
    /// Default foreground (answers OSC 10).
    pub fg: Rgb,
    /// Default background (answers OSC 11).
    pub bg: Rgb,
    /// Cursor colour (answers OSC 12).
    pub cursor: Rgb,
    /// Text under a block cursor.
    pub cursor_text: Rgb,
    /// Selection fill, already blended over `bg`.
    pub selection_bg: Rgb,
    /// Text inside the selection.
    pub selection_fg: Rgb,
}

/// Accent colour choice; the names match `accentAlternatives` in `app/src/theme/tokens.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AccentName {
    /// The default accent.
    Blue,
    /// Mint.
    Mint,
    /// Violet.
    Violet,
    /// Sand.
    Sand,
}

/// Which ⌥ key acts as Meta (spec K5); `off` keeps ⌥ for typing layout characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum OptionAsMeta {
    /// Neither ⌥ is Meta (the default).
    Off,
    /// Left ⌥ is Meta.
    Left,
    /// Right ⌥ is Meta.
    Right,
    /// Both are Meta.
    Both,
}

/// User settings (`settings.get` / `settings.set`, ADR-0009); every field is required on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Accent colour of the chrome and the terminal cursor.
    pub accent: AccentName,
    /// ⌥-as-Meta per side.
    pub option_as_meta: OptionAsMeta,
    /// Hold a prevent-idle-sleep assertion while any pane is running.
    pub keep_awake_while_running: bool,
    /// When plyd starts, resume every `lost` Claude Code and Codex pane that has a session to resume, as `pane.resume` does.
    pub resume_sessions_on_start: bool,
    /// Pass `"theme":"dark-ansi"` to Claude Code so it uses ply's palette.
    pub use_ply_colours_in_claude: bool,
    /// Enable Codex's plan tool per invocation (`-c tools.update_plan.enabled=true`, Ruling R26).
    pub codex_plan_tool: bool,
    /// Scrollback lines kept per pane in plyd.
    pub scrollback_lines: u32,
    /// Font size in points for chrome and terminals.
    pub font_size: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            accent: AccentName::Blue,
            option_as_meta: OptionAsMeta::Off,
            keep_awake_while_running: true,
            resume_sessions_on_start: true,
            use_ply_colours_in_claude: true,
            codex_plan_tool: true,
            scrollback_lines: 10_000,
            font_size: 12.5,
        }
    }
}

/// What `usage.get` returns (Ruling R59): each CLI's plan usage as it last reported it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct Usage {
    /// Claude Code's, from the newest status line report of a Claude pane or the usage cache in its `.claude.json`, whichever is newer; absent when there is neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<CliUsage>,
    /// Codex's, from the rate limits its newest rollouts recorded; absent when there are none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<CliUsage>,
}

/// One CLI's plan usage, as recorded at `as_of`; the numbers are the CLI's, never computed by ply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct CliUsage {
    /// When the CLI recorded these numbers; they may be hours old.
    pub as_of: UnixSeconds,
    /// The plan as the CLI names it (Codex's `plan_type`), when it records one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The limit windows, never empty: shortest first, the all-models window before per-model ones of equal length.
    pub windows: Vec<UsageWindow>,
}

/// One rate-limit window: how much of it is used and when it starts over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct UsageWindow {
    /// What the window is, for display: `"Session · 5h"`, `"Week · all models"`, `"Week · Opus"`, `"Week"`, `"2 d"`.
    pub label: String,
    /// Length of the window in minutes (300 is the 5-hour session, 10 080 the week), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,
    /// Percent of the limit used, 0–100 (more when the CLI reports an overrun).
    pub used_percent: f64,
    /// When the window starts over; may already be past when the record is old.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<UnixSeconds>,
    /// The models the sessions behind this record used, sorted (Codex's `turn_context.model`); empty when unknown.
    pub models: Vec<String>,
}

/// Id of a queued task (its `tasks` row id); an id that was ever announced is never reused.
pub type TaskId = u64;

/// Longest task text in bytes; `task.add` refuses a longer one with `bad_request`.
pub const MAX_TASK_TEXT_BYTES: usize = 16 * 1024;

/// Most queued tasks one pane's queue, or one workspace's pool, holds; `task.add` beyond it is `invalid_state`.
pub const MAX_QUEUED_TASKS: usize = 32;

/// Where a task stands (Ruling R60); `ended` means the turn it started ended, which ply cannot tell from an interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Waiting in its pane's queue or its pool for the pane to be free.
    Queued,
    /// Typed into the pane; the CLI has not acknowledged the prompt yet.
    Sent,
    /// The CLI acknowledged the prompt and is working on it (it may be waiting for the user meanwhile).
    Running,
    /// The turn the task started ended.
    Ended,
    /// Never acknowledged, or the pane's process or plyd stopped while it ran; `detail` says which.
    Failed,
    /// Removed before it was typed: by `task.cancel` or because its pane closed.
    Cancelled,
}

/// The next-free-pane target: the first idle pane of `cli` whose directory is `cwd` or inside it takes the task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TaskPool {
    /// The CLI the pane must run.
    pub cli: AgentCli,
    /// Absolute directory the pane must be in, or below.
    pub cwd: String,
}

/// Where `task.add` sends a task: one pane's queue, or a pool that the next free matching pane takes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TaskTarget {
    /// The queue of this agent pane.
    Pane(PaneId),
    /// The workspace's pool for this CLI and directory.
    Pool(TaskPool),
}

/// One task: text the user wrote or picked, typed verbatim into a pane when it is the user's turn (Ruling R60).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct Task {
    /// Task id.
    pub id: TaskId,
    /// Workspace the task belongs to.
    pub workspace_id: u64,
    /// The pane it is queued on or ran in; absent while a pool task waits for a pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<PaneId>,
    /// The pool it was added to, kept after a pane took it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<TaskPool>,
    /// Exactly what is typed, at most [`MAX_TASK_TEXT_BYTES`]; ply never changes it.
    pub text: String,
    /// The skill's invocation when the user picked one from `skill.list`; display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    /// Current state.
    pub state: TaskState,
    /// 0-based place among the queued tasks of its queue; kept as it was once the task leaves `queued`.
    pub position: u32,
    /// Why a task failed or was cancelled, one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// When it was added.
    pub created_at: UnixSeconds,
    /// When plyd typed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<UnixSeconds>,
    /// When the CLI acknowledged it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<UnixSeconds>,
    /// When it ended, failed or was cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<UnixSeconds>,
}

/// Why a pane's queue holds its tasks back until the user resumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// The user paused it (`queue.pause`).
    User,
    /// plyd restarted with tasks still queued; they wait for the user.
    Restored,
    /// The last task failed; nothing more is typed until the user resumes.
    Failed,
}

/// Why the next task of an unpaused queue cannot be typed right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    /// The user typed into the pane since its last prompt, so its input may hold unsent text; `task.send` overrides.
    Typing,
    /// The pane's CLI has not shown its prompt in this process yet (Codex until its first turn, Claude Code until SessionStart), so a startup screen may be up; nothing overrides it.
    Startup,
}

/// The state of one pane's queue beyond its tasks; `queue.changed` carries it, and a queue with neither field is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct QueueState {
    /// The pane.
    pub pane_id: PaneId,
    /// Set while the queue is paused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<PauseReason>,
    /// Set while the next task waits for something other than the pane's turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<BlockReason>,
}

/// What `task.list` returns: the open tasks and the most recent finished ones, and every queue that is paused or blocked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct TaskList {
    /// Queued, sent and running tasks, then finished ones newest first.
    pub tasks: Vec<Task>,
    /// Queues that are paused or blocked; every other queue runs.
    pub queues: Vec<QueueState>,
}

/// Where a skill was found (Ruling R61).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// The project: `.claude/` of the pane's directory for Claude Code, `.agents/` or `.codex/` for Codex.
    Project,
    /// The user's own: `~/.claude` (or `CLAUDE_CONFIG_DIR`), `$CODEX_HOME/skills` or `~/.agents/skills`.
    User,
    /// An enabled Claude Code plugin; `Skill::plugin` names it.
    Plugin,
    /// Skills the CLI installs itself (Codex's `$CODEX_HOME/skills/.system`).
    System,
    /// A Codex custom prompt (`$CODEX_HOME/prompts`).
    Prompt,
}

/// One skill or command a CLI offers, as its files on this machine describe it; ply only reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct Skill {
    /// The skill's name.
    pub name: String,
    /// What the user types to run it, e.g. `/superpowers:brainstorming` or `$review-pr`.
    pub invocation: String,
    /// The front matter's `description`, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The front matter's `argument-hint`, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// Where it was found.
    pub source: SkillSource,
    /// The plugin it comes from, for `source: plugin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
}

/// What `skill.list` returns: the CLI's skills, project first, then user, plugins, system and prompts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SkillList {
    /// The skills, each invocation once.
    pub skills: Vec<Skill>,
}
