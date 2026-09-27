//! Claude Code adapter (spec 6.1, ADR-0003): launch with a hooks-only `--settings` file, and hook payloads to signals.

pub mod paste;
pub mod progress;
pub mod settings;
pub mod usage;

use std::path::Path;
use std::time::Duration;

use ply_proto::hook::HookEnvelope;
use ply_proto::pane::{AgentCli, Cli, Progress};
use serde_json::Value;

use crate::adapter::{
    Adapter, AdapterSignal, AgentEvent, AgentSession, Launch, LaunchRequest, LaunchSpec, PaneFile,
    SessionStats, StatusSignal, ToolCall, base_env, check_value, cli_name, push_prompt, str_field,
    utf8,
};
use crate::error::{Error, Result, invalid};
use crate::install;
use crate::meta::SessionMeta;
use crate::version::CliVersion;

use self::progress::TodoState;
use self::settings::{SETTINGS_FILE, claude_settings_json};

/// Oldest supported Claude Code: the version ADR-0003's evidence covers, not a known lower bound.
pub const MIN_VERSION: CliVersion = CliVersion::new(2, 1, 282);

/// Env var ply sets for Claude Code panes (spec 6.1); asks for DEC 2026 synchronized output, unverified in 2.1.282.
pub const ENV_FORCE_SYNC_OUTPUT: &str = "CLAUDE_CODE_FORCE_SYNC_OUTPUT";

/// A `running` Claude pane with a silent pty and no hook for this long becomes `idle` (R17).
pub const QUIET_TIMEOUT: Duration = Duration::from_secs(5);

/// `Notification.notification_type` of a permission dialog: `waiting_permission` (Ruling R46).
pub const PERMISSION_PROMPT: &str = "permission_prompt";

/// `Notification.notification_type` Claude sends after about 60 s at its prompt; the pane stays `idle`, "your turn" (Ruling R46).
pub const IDLE_PROMPT: &str = "idle_prompt";

/// The Claude Code adapter; see [`crate::adapter`] for the contract.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeAdapter;

impl Adapter for ClaudeAdapter {
    fn cli(&self) -> AgentCli {
        AgentCli::Claude
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<Launch> {
        let settings_path = request.pane_dir.join(SETTINGS_FILE);
        let mut argv = vec![
            utf8(request.program)?,
            "--settings".to_owned(),
            utf8(&settings_path)?,
        ];
        if let Some(name) = request.worktree {
            check_value("worktree name", name)?;
            argv.extend(["--worktree".to_owned(), name.to_owned()]);
        }
        if let Some(id) = request.resume {
            check_value("session id", id)?;
            argv.extend(["--resume".to_owned(), id.to_owned()]);
        }
        push_prompt(&mut argv, request.prompt)?;
        let mut env = base_env(request)?;
        env.insert(ENV_FORCE_SYNC_OUTPUT.to_owned(), "1".to_owned());
        let contents = claude_settings_json(
            request.hook_program,
            request.settings.use_ply_colours_in_claude,
            request.status_line,
        )?;
        Ok(Launch {
            spec: LaunchSpec {
                cli: Cli::Claude,
                argv,
                env,
                cwd: utf8(request.cwd)?,
                worktree: request.worktree.map(str::to_owned),
                resume: request.resume.map(str::to_owned),
            },
            files: vec![PaneFile {
                path: settings_path,
                contents,
            }],
        })
    }

    fn new_session(&self, spec: &LaunchSpec) -> Box<dyn AgentSession> {
        Box::new(ClaudeSession::new(spec))
    }

    fn min_version(&self) -> CliVersion {
        MIN_VERSION
    }

    /// Native installer (`…/claude/versions/<version>`), npm (`@anthropic-ai/claude-code/package.json`) or a Homebrew cask.
    fn installed_version(&self, exe: &Path) -> Result<CliVersion> {
        let exe = install::resolve(exe)?;
        let parent = exe.parent();
        if parent
            .and_then(Path::file_name)
            .is_some_and(|d| d == "versions")
            && let Some(version) = exe.file_name().and_then(|f| f.to_str()?.parse().ok())
        {
            return Ok(version);
        }
        for dir in [parent, parent.and_then(Path::parent)]
            .into_iter()
            .flatten()
        {
            let manifest = dir.join("package.json");
            if let Some(version) = install::json_version(&manifest, Some(NPM_PACKAGE))? {
                return Ok(version);
            }
        }
        install::version_after(&exe, &["Caskroom", "claude-code"])
            .ok_or(Error::VersionUnknown { exe })
    }

    fn version_probe_args(&self) -> &'static [&'static str] {
        &["--version"]
    }

    fn quiet_timeout(&self) -> Option<Duration> {
        Some(QUIET_TIMEOUT)
    }

    fn acknowledges_prompt(&self, signal: &StatusSignal) -> bool {
        matches!(signal, StatusSignal::PromptSubmitted)
    }

    fn shows_prompt(&self, signal: &StatusSignal) -> bool {
        matches!(
            signal,
            StatusSignal::Ready | StatusSignal::PromptSubmitted | StatusSignal::TurnComplete
        )
    }

    /// Claude Code reads the images before it takes keys again and drops an Enter that came meanwhile (2.1.283).
    fn paste_reads_images(&self, text: &str) -> bool {
        paste::reads_images(text)
    }
}

const NPM_PACKAGE: &str = "@anthropic-ai/claude-code";

/// Per-pane interpreter of Claude Code hook payloads (C3); Claude transcripts are never read (C4).
#[derive(Debug, Clone, Default)]
pub struct ClaudeSession {
    meta: SessionMeta,
    todos: TodoState,
    progress: Option<Progress>,
    stats: SessionStats,
}

impl ClaudeSession {
    /// A session for a pane launched with `spec`: cwd from the spec, `session_ref` from its resume id.
    pub fn new(spec: &LaunchSpec) -> Self {
        let mut meta = SessionMeta::default();
        meta.set_cwd(&spec.cwd);
        if let Some(id) = &spec.resume {
            meta.set_session_ref(id);
        }
        Self {
            meta,
            ..Self::default()
        }
    }

    fn on_hook(&mut self, envelope: &HookEnvelope) -> Result<Vec<AdapterSignal>> {
        if envelope.cli != AgentCli::Claude {
            return Err(Error::WrongCli {
                expected: "claude",
                found: cli_name(envelope.cli),
            });
        }
        let payload = &envelope.payload;
        if !payload.is_object() {
            return Err(invalid("Claude hook payload", "not a JSON object"));
        }
        let event = envelope
            .event
            .as_deref()
            .or_else(|| str_field(payload, "hook_event_name"))
            .ok_or_else(|| invalid("Claude hook payload", "no event name"))?;
        let mut signals = Vec::new();
        if self.update_meta(event, payload) {
            signals.push(AdapterSignal::Meta(self.meta.clone()));
        }
        match event {
            // A compaction continues the session mid-turn, so it is no new session to be idle in.
            "SessionStart" if str_field(payload, "source") == Some("compact") => {}
            "SessionStart" => signals.push(status(StatusSignal::Ready)),
            "UserPromptSubmit" => signals.push(status(StatusSignal::PromptSubmitted)),
            "PreToolUse" => signals.push(status(StatusSignal::ToolUse(tool_call(payload)?))),
            "PostToolUse" => {
                let call = tool_call(payload)?;
                signals.push(status(StatusSignal::ToolUse(call.clone())));
                signals.push(status(StatusSignal::CallSettled(call.clone())));
                if let Some(input) = payload.get("tool_input") {
                    let applied = self
                        .todos
                        .apply(&call.name, input, payload.get("tool_response"));
                    self.stats.unreadable_progress += u64::from(applied.is_err());
                    if applied.unwrap_or(false) {
                        self.progress = self.todos.progress();
                        signals.push(AdapterSignal::Progress(self.progress.clone()));
                    }
                }
            }
            "PostToolUseFailure" | "PermissionDenied" => {
                signals.push(status(StatusSignal::CallSettled(tool_call(payload)?)));
            }
            "PermissionRequest" => {
                let call = tool_call(payload)?;
                signals.push(status(StatusSignal::PermissionRequested {
                    detail: Some(call.name.clone()),
                    call: Some(call),
                }));
            }
            "Notification" => {
                let detail = str_field(payload, "message").map(str::to_owned);
                match str_field(payload, "notification_type") {
                    Some(PERMISSION_PROMPT) => {
                        signals.push(status(StatusSignal::PermissionRequested {
                            call: None,
                            detail,
                        }));
                    }
                    Some(IDLE_PROMPT) => {}
                    _ => signals.push(status(StatusSignal::InputRequested { detail })),
                }
            }
            "Stop" | "StopFailure" => signals.push(status(StatusSignal::TurnComplete)),
            "SessionEnd" => {
                let reason = str_field(payload, "reason").map(str::to_owned);
                signals.push(status(StatusSignal::SessionEnded { reason }));
            }
            "CwdChanged" => {}
            _ => self.stats.unknown_hook_events += 1,
        }
        Ok(signals)
    }

    fn update_meta(&mut self, event: &str, payload: &Value) -> bool {
        let mut changed = false;
        if let Some(id) = str_field(payload, "session_id")
            && (event == "SessionStart" || self.meta.session_ref.is_none())
        {
            changed |= self.meta.set_session_ref(id);
        }
        let cwd = str_field(payload, "new_cwd").or_else(|| str_field(payload, "cwd"));
        if let Some(cwd) = cwd {
            changed |= self.meta.set_cwd(cwd);
        }
        if event == "SessionStart"
            && let Some(model) = str_field(payload, "model")
        {
            changed |= self.meta.set_model(model);
        }
        changed
    }
}

impl AgentSession for ClaudeSession {
    fn handle(&mut self, event: AgentEvent<'_>) -> Result<Vec<AdapterSignal>> {
        match event {
            AgentEvent::Hook(envelope) => self.on_hook(envelope),
            AgentEvent::KeyTyped { .. } => Ok(vec![status(StatusSignal::KeyTyped)]),
            AgentEvent::Osc9(_)
            | AgentEvent::RolloutLine(_)
            | AgentEvent::RolloutHistory(_)
            | AgentEvent::FirstOutput
            | AgentEvent::RolloutSwitched => Ok(vec![]),
        }
    }

    fn meta(&self) -> &SessionMeta {
        &self.meta
    }

    fn progress(&self) -> Option<&Progress> {
        self.progress.as_ref()
    }

    fn stats(&self) -> SessionStats {
        self.stats
    }
}

fn status(signal: StatusSignal) -> AdapterSignal {
    AdapterSignal::Status(signal)
}

fn tool_call(payload: &Value) -> Result<ToolCall> {
    let name = str_field(payload, "tool_name")
        .ok_or_else(|| invalid("Claude tool hook payload", "no tool_name"))?;
    let id = str_field(payload, "tool_use_id").map(str::to_owned);
    Ok(ToolCall::new(id, name, payload.get("tool_input")))
}
