//! The per-CLI adapter contract: launch specs (C7, spec 6.1/6.2), and C3/C4/C8 events turned into the signals of spec 6.3.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ply_proto::hook::HookEnvelope;
use ply_proto::pane::{AgentCli, Cli, PaneId, Progress, Settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::claude::ClaudeAdapter;
use crate::codex::CodexAdapter;
use crate::error::{Error, Result, json};
use crate::meta::SessionMeta;
use crate::version::CliVersion;

/// Env var carrying the pane id to `ply-hook`; the CLIs pass their environment on to hooks and notify.
pub const ENV_PANE_ID: &str = "PLY_PANE_ID";

/// Env var carrying the absolute path of `run/hook.sock` to `ply-hook`.
pub const ENV_HOOK_SOCK: &str = "PLY_HOOK_SOCK";

/// `TERM` every agent pane gets (C5).
pub const TERM: &str = "xterm-256color";

/// `COLORTERM` every agent pane gets (C5).
pub const COLORTERM: &str = "truecolor";

/// File name of the persisted launch spec inside `run/panes/<id>/` (Ruling R6).
pub const LAUNCH_FILE: &str = "launch.json";

/// Inputs for one spawn; paths are absolute and must be valid UTF-8 because they travel in argv, env and JSON.
#[derive(Debug, Clone, Copy)]
pub struct LaunchRequest<'a> {
    /// The pane the process runs in; becomes `PLY_PANE_ID`.
    pub pane_id: PaneId,
    /// The CLI executable as plyd resolved it; becomes `argv[0]`.
    pub program: &'a Path,
    /// Working directory the process starts in.
    pub cwd: &'a Path,
    /// The bundled `ply-hook` binary that hooks and notify run.
    pub hook_program: &'a Path,
    /// `run/hook.sock`; becomes `PLY_HOOK_SOCK`.
    pub hook_socket: &'a Path,
    /// `run/panes/<id>/`, where [`Launch::files`] and [`LAUNCH_FILE`] go; plyd creates it (mode 0700).
    pub pane_dir: &'a Path,
    /// User settings; Claude reads `use_ply_colours_in_claude`, Codex reads `codex_plan_tool`.
    pub settings: &'a Settings,
    /// Claude Code worktree name (`--worktree <name>`); [`Error::InvalidLaunch`] for Codex (spec 6.2).
    pub worktree: Option<&'a str>,
    /// The CLI session id to resume (`--resume <id>` / `resume <thread>`).
    pub resume: Option<&'a str>,
    /// First prompt, passed as the CLI's positional prompt argument.
    pub prompt: Option<&'a str>,
}

/// What plyd needs to spawn a pane: the spec, plus files it must write (mode 0600) before the spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// argv, env additions and cwd; plyd also persists it as [`LAUNCH_FILE`].
    pub spec: LaunchSpec,
    /// Files the spec refers to (the Claude settings file); empty for Codex.
    pub files: Vec<PaneFile>,
}

/// One generated per-pane file with its full contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneFile {
    /// Absolute path inside the request's `pane_dir`.
    pub path: PathBuf,
    /// UTF-8 contents to write verbatim.
    pub contents: String,
}

/// A spawn as persisted in `run/panes/<id>/launch.json` (Ruling R6); `argv[0]` is the program and argv is never empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchSpec {
    /// Program the pane runs.
    pub cli: Cli,
    /// Full argv, program first; passed to exec as is (no shell).
    pub argv: Vec<String>,
    /// Variables added to plyd's environment for the child; nothing is removed.
    pub env: BTreeMap<String, String>,
    /// Absolute working directory.
    pub cwd: String,
    /// The worktree option as requested, kept for relaunch (INV-7: ply only passes it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// The session id this spawn resumes, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
}

impl LaunchSpec {
    /// Serialises the spec as pretty JSON for [`LAUNCH_FILE`]; fails only if serde_json does.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(json("launch spec"))
    }

    /// Reads a spec written by [`LaunchSpec::to_json`]; unknown fields and an empty argv are rejected.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let spec: Self = serde_json::from_slice(bytes).map_err(json("launch spec"))?;
        if spec.argv.is_empty() {
            return Err(Error::InvalidLaunch(
                "launch spec has an empty argv".to_owned(),
            ));
        }
        Ok(spec)
    }
}

/// One thing plyd observed for a pane, handed to its [`AgentSession`].
#[derive(Debug, Clone, Copy)]
pub enum AgentEvent<'a> {
    /// A decoded C3 envelope for this pane (Claude hook or Codex notify).
    Hook(&'a HookEnvelope),
    /// The body of an OSC 9 desktop notification from the pane's pty (C8).
    Osc9(&'a str),
    /// One complete line of the pane's Codex rollout file, without its newline (C4).
    RolloutLine(&'a [u8]),
    /// The user typed into the pane; `enter` is true when the input was the Enter key.
    KeyTyped {
        /// Whether the key was Enter (`\r`).
        enter: bool,
    },
    /// The pane's pty produced its first byte of output.
    FirstOutput,
}

/// What an event means for plyd; signals come in the order plyd should apply them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterSignal {
    /// Input for the spec 6.3 state machine.
    Status(StatusSignal),
    /// The plan changed; `None` hides progress (no plan, or an empty one). plyd throttles `pane.progress` to 4/s.
    Progress(Option<Progress>),
    /// The session metadata changed; carries the full new value for `pane.meta` and the database.
    Meta(SessionMeta),
    /// Codex notify named this thread and the pane has no rollout yet: tail `rollout-*-<thread_id>.jsonl` if it exists (R28).
    FindRollout {
        /// The notify's `thread-id`.
        thread_id: String,
    },
}

/// The signals of the spec 6.3 table (with Rulings R17 and R27); plyd owns the table, these name its signal column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusSignal {
    /// Every live state → `idle`: Claude SessionStart (a new or resumed session, not a compaction), Codex first output byte.
    Ready,
    /// `idle`, `waiting_input` → `running`: Claude UserPromptSubmit, Codex Enter typed.
    PromptSubmitted,
    /// Any live state except `waiting_permission` → `running`: Claude PreToolUse or PostToolUse.
    ToolUse(ToolCall),
    /// `running` → `waiting_permission`: Claude PermissionRequest (with its call), Codex OSC 9 approval (no call).
    PermissionRequested {
        /// The tool call the prompt is for, used to match [`StatusSignal::CallSettled`].
        call: Option<ToolCall>,
        /// One line for the pane header, e.g. the tool name or the OSC 9 text.
        detail: Option<String>,
    },
    /// `running` (and for Claude `idle`) → `waiting_input`: Codex OSC 9 question or plan prompt, Claude non-permission Notification.
    InputRequested {
        /// One line for the pane header (the notification message).
        detail: Option<String>,
    },
    /// `waiting_permission` → `running` when [`ToolCall::same_call`] matches the pending call: PostToolUse(Failure), PermissionDenied.
    CallSettled(ToolCall),
    /// `running` → `idle`: Claude Stop or StopFailure, Codex notify or OSC 9 turn complete.
    TurnComplete,
    /// `waiting_permission`, `waiting_input` → `running`: any key typed (R17; a manual deny fires no hook).
    KeyTyped,
    /// `running` → `idle` for a pane whose adapter has an [`Adapter::quiet_timeout`]; plyd raises it itself (R17).
    QuietTimeout,
    /// Claude SessionEnd, no status change (Ruling R47): `/clear` and `/resume` end a session while the process runs on.
    SessionEnded {
        /// The CLI's stated reason, e.g. `"prompt_input_exit"`.
        reason: Option<String>,
    },
}

/// Identity of one tool call: matched by id when both sides carry one, else by tool name and input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// The CLI's call id (`tool_use_id`); Claude's PermissionRequest payload has none.
    pub id: Option<String>,
    /// Tool name, e.g. `"Write"`.
    pub name: String,
    input_hash: u64,
}

impl ToolCall {
    /// Builds a call from its parts; `input` is hashed (serde_json's sorted-key form), so only equality survives.
    pub fn new(id: Option<String>, name: impl Into<String>, input: Option<&Value>) -> Self {
        let mut hasher = DefaultHasher::new();
        input.map(Value::to_string).hash(&mut hasher);
        Self {
            id,
            name: name.into(),
            input_hash: hasher.finish(),
        }
    }

    /// Whether two reports describe the same call: equal ids if both have one, else equal name and input.
    pub fn same_call(&self, other: &Self) -> bool {
        match (&self.id, &other.id) {
            (Some(a), Some(b)) => a == b,
            _ => self.name == other.name && self.input_hash == other.input_hash,
        }
    }
}

/// Counters of input the session skipped, for plyd's metrics (spec 6.3 "ignored and counted", C4 "skipped and counted").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionStats {
    /// Hook events or payload kinds with no meaning for ply (e.g. an event name ply did not register).
    pub unknown_hook_events: u64,
    /// Rollout records of a type Codex 0.156.1 does not write.
    pub unknown_rollout_records: u64,
    /// Rollout lines that were not a JSON record, or a record ply could not read.
    pub malformed_rollout_lines: u64,
    /// Claude TodoWrite/Task-tool calls whose input could not be read (the plan kept its value); Codex's count as malformed lines.
    pub unreadable_progress: u64,
}

/// Per-pane interpreter of one CLI's signals; plyd owns one per agent pane and calls it from one task at a time.
pub trait AgentSession: Send {
    /// Interprets one event; errors describe a malformed payload (plyd logs it with the pane id) and leave the state unchanged.
    fn handle(&mut self, event: AgentEvent<'_>) -> Result<Vec<AdapterSignal>>;

    /// The session metadata reported so far.
    fn meta(&self) -> &SessionMeta;

    /// The current plan progress; `None` while no plan source has appeared (R16) or the plan is empty.
    fn progress(&self) -> Option<&Progress>;

    /// What the session has skipped so far.
    fn stats(&self) -> SessionStats;
}

/// One CLI's integration: stateless, shared by every pane of that CLI; see [`adapter`].
pub trait Adapter: Send + Sync {
    /// The CLI this adapter launches.
    fn cli(&self) -> AgentCli;

    /// Builds argv, env and the files to write for a spawn (spec 6.1/6.2); fails with [`Error::InvalidLaunch`] or [`Error::NonUtf8Path`].
    fn launch(&self, request: &LaunchRequest<'_>) -> Result<Launch>;

    /// A fresh interpreter for a pane launched with `spec`, seeded with its cwd and resumed session id.
    fn new_session(&self, spec: &LaunchSpec) -> Box<dyn AgentSession>;

    /// The oldest version ply supports (C7).
    fn min_version(&self) -> CliVersion;

    /// Reads the version from the install's own metadata beside `exe` without running it (R14); reads files, never writes.
    fn installed_version(&self, exe: &Path) -> Result<CliVersion>;

    /// Arguments for the fallback probe `<exe> <args>` when [`Adapter::installed_version`] finds no metadata; parse with [`CliVersion::from_version_output`].
    fn version_probe_args(&self) -> &'static [&'static str];

    /// Silence after which a `running` pane with no hook becomes `idle` ([`StatusSignal::QuietTimeout`]); `None` disables it.
    fn quiet_timeout(&self) -> Option<Duration>;

    /// Passes when `found` is at least [`Adapter::min_version`]; else [`Error::CliTooOld`].
    fn check_version(&self, found: &CliVersion) -> Result<()> {
        let min = self.min_version();
        if *found < min {
            return Err(Error::CliTooOld {
                cli: cli_name(self.cli()),
                found: found.clone(),
                min,
            });
        }
        Ok(())
    }
}

/// The adapter of `cli`; both adapters are zero-sized statics.
pub fn adapter(cli: AgentCli) -> &'static dyn Adapter {
    match cli {
        AgentCli::Claude => &ClaudeAdapter,
        AgentCli::Codex => &CodexAdapter,
    }
}

/// The lower-case CLI name used on the wire and in messages.
pub fn cli_name(cli: AgentCli) -> &'static str {
    match cli {
        AgentCli::Claude => "claude",
        AgentCli::Codex => "codex",
    }
}

pub(crate) fn utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::NonUtf8Path(path.to_path_buf()))
}

pub(crate) fn check_value(what: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::InvalidLaunch(format!("{what} is empty")));
    }
    if value.contains('\0') {
        return Err(Error::InvalidLaunch(format!("{what} contains a NUL byte")));
    }
    if value.starts_with('-') {
        return Err(Error::InvalidLaunch(format!(
            "{what} {value:?} would be read as an option"
        )));
    }
    Ok(())
}

pub(crate) fn base_env(request: &LaunchRequest<'_>) -> Result<BTreeMap<String, String>> {
    Ok(BTreeMap::from([
        (ENV_PANE_ID.to_owned(), request.pane_id.to_string()),
        (ENV_HOOK_SOCK.to_owned(), utf8(request.hook_socket)?),
        ("TERM".to_owned(), TERM.to_owned()),
        ("COLORTERM".to_owned(), COLORTERM.to_owned()),
    ]))
}

pub(crate) fn push_prompt(argv: &mut Vec<String>, prompt: Option<&str>) -> Result<()> {
    let Some(prompt) = prompt else {
        return Ok(());
    };
    if prompt.contains('\0') {
        return Err(Error::InvalidLaunch(
            "prompt contains a NUL byte".to_owned(),
        ));
    }
    // Both CLIs end option parsing at `--`, so a prompt such as "-v" stays a prompt.
    if prompt.starts_with('-') {
        argv.push("--".to_owned());
    }
    argv.push(prompt.to_owned());
    Ok(())
}

pub(crate) fn str_field<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tool_calls_match_by_id_else_by_name_and_input() {
        let input = json!({"file_path": "/Users/example/b.txt", "content": "hi\n"});
        let permission = ToolCall::new(None, "Write", Some(&input));
        let post = ToolCall::new(Some("toolu_1".into()), "Write", Some(&input));
        assert!(permission.same_call(&post));
        assert!(!permission.same_call(&ToolCall::new(None, "Write", Some(&json!({})))));
        assert!(!permission.same_call(&ToolCall::new(None, "Edit", Some(&input))));
        let other_id = ToolCall::new(Some("toolu_2".into()), "Write", Some(&input));
        assert!(!post.same_call(&other_id));
    }

    #[test]
    fn launch_values_that_read_as_options_are_refused() {
        assert!(check_value("worktree", "feat").is_ok());
        for bad in ["", "-x", "a\0b"] {
            assert!(check_value("worktree", bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn prompts_starting_with_a_dash_follow_a_separator() {
        let mut argv = vec![];
        push_prompt(&mut argv, Some("-v means verbose?")).unwrap();
        assert_eq!(argv, ["--", "-v means verbose?"]);
        let mut argv = vec![];
        push_prompt(&mut argv, Some("hello")).unwrap();
        assert_eq!(argv, ["hello"]);
    }
}
