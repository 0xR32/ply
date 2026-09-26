//! Codex adapter (spec 6.2, ADR-0004): per-invocation `-c` overrides, and notify, OSC 9 and rollout records to signals.

mod literal;
pub mod notify;
pub mod osc9;
pub mod rollout;
pub mod usage;

use std::path::Path;
use std::time::Duration;

use ply_proto::hook::HookEnvelope;
use ply_proto::pane::{AgentCli, Cli, Progress};

use crate::adapter::{
    Adapter, AdapterSignal, AgentEvent, AgentSession, Launch, LaunchRequest, LaunchSpec,
    SessionStats, StatusSignal, base_env, cli_name, push_prompt, utf8,
};
use crate::error::{Error, Result};
use crate::install;
use crate::meta::SessionMeta;
use crate::version::CliVersion;

use self::notify::NotifyPayload;
use self::osc9::{Osc9Kind, classify_osc9};
use self::rollout::{RolloutRecord, TurnEvent, is_uuid, parse_record};

/// Oldest supported Codex (spec 6.2, ADR-0004 item 8).
pub const MIN_VERSION: CliVersion = CliVersion::new(0, 156, 1);

/// Makes the TUI raise every notification as OSC 9 in the pty, where plyd reads it (C8).
pub const NOTIFICATION_METHOD: &str = r#"tui.notification_method="osc9""#;

/// Raises notifications while the pane has focus too; Codex's default is only while unfocused.
pub const NOTIFICATION_CONDITION: &str = r#"tui.notification_condition="always""#;

/// Registers the `update_plan` tool, off by default in 0.156.1, so progress has a source (R26, setting `codex_plan_tool`).
pub const PLAN_TOOL: &str = "tools.update_plan.enabled=true";

/// Notify threads a session remembers until a rollout names one; beyond it the oldest is forgotten and counted.
pub const PENDING_TURNS: usize = 8;

/// The Codex adapter; see [`crate::adapter`] for the contract.
#[derive(Debug, Clone, Copy, Default)]
pub struct CodexAdapter;

impl Adapter for CodexAdapter {
    fn cli(&self) -> AgentCli {
        AgentCli::Codex
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<Launch> {
        if request.worktree.is_some() {
            return Err(Error::InvalidLaunch(
                "Codex has no named worktree option; open the pane in the worktree directory"
                    .to_owned(),
            ));
        }
        let mut argv = vec![utf8(request.program)?];
        let mut overrides = vec![notify_override(&utf8(request.hook_program)?)];
        overrides.extend([NOTIFICATION_METHOD, NOTIFICATION_CONDITION].map(str::to_owned));
        if request.settings.codex_plan_tool {
            overrides.push(PLAN_TOOL.to_owned());
        }
        for value in overrides {
            argv.extend(["-c".to_owned(), value]);
        }
        if let Some(thread) = request.resume {
            if !is_uuid(thread) {
                return Err(Error::InvalidLaunch(format!(
                    "{thread:?} is not a Codex thread id"
                )));
            }
            argv.extend(["resume".to_owned(), thread.to_owned()]);
        }
        push_prompt(&mut argv, request.prompt)?;
        Ok(Launch {
            spec: LaunchSpec {
                cli: Cli::Codex,
                argv,
                env: base_env(request)?,
                cwd: utf8(request.cwd)?,
                worktree: None,
                resume: request.resume.map(str::to_owned),
            },
            files: vec![],
        })
    }

    fn new_session(&self, spec: &LaunchSpec) -> Box<dyn AgentSession> {
        Box::new(CodexSession::new(spec))
    }

    fn min_version(&self) -> CliVersion {
        MIN_VERSION
    }

    /// Standalone/package layout (`<pkg>/bin/codex` + `<pkg>/codex-package.json`), npm `@openai/codex`, or Homebrew.
    fn installed_version(&self, exe: &Path) -> Result<CliVersion> {
        let exe = install::resolve(exe)?;
        if let Some(pkg) = exe
            .parent()
            .filter(|d| d.ends_with("bin"))
            .and_then(Path::parent)
        {
            if let Some(version) = install::json_version(&pkg.join("codex-package.json"), None)? {
                return Ok(version);
            }
            let manifest = pkg.join("package.json");
            if let Some(version) = install::json_version(&manifest, Some(NPM_PACKAGE))? {
                return Ok(version);
            }
        }
        [["Caskroom", "codex"], ["Cellar", "codex"]]
            .iter()
            .find_map(|marker| install::version_after(&exe, marker))
            .ok_or(Error::VersionUnknown { exe })
    }

    /// `--version` is answered by clap before config, TUI, plugins or daemon start (codex-rs/cli/src/main.rs), so it cannot update.
    fn version_probe_args(&self) -> &'static [&'static str] {
        &["--version"]
    }

    fn quiet_timeout(&self) -> Option<Duration> {
        None
    }

    fn acknowledges_prompt(&self, signal: &StatusSignal) -> bool {
        matches!(signal, StatusSignal::TurnStarted)
    }
}

const NPM_PACKAGE: &str = "@openai/codex";

/// The `-c notify=[…]` value that makes Codex run `<hook> codex <json>` after each turn.
pub fn notify_override(hook_program: &str) -> String {
    format!(
        "notify=[{},{}]",
        toml_string(hook_program),
        toml_string("codex")
    )
}

/// `s` as a TOML basic string, the value syntax `-c` parses.
pub fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Per-pane interpreter of Codex notify payloads, OSC 9 bodies and rollout lines, following one thread: the first `session_meta` binds it, and another thread's rebinds it (session id included) after `AgentEvent::RolloutSwitched` or a notify naming that thread (R49).
#[derive(Debug, Clone, Default)]
pub struct CodexSession {
    meta: SessionMeta,
    bound: Option<String>,
    pending_turns: Vec<String>,
    switched: bool,
    turn: Option<String>,
    progress: Option<Progress>,
    stats: SessionStats,
}

impl CodexSession {
    /// A session for a pane launched with `spec`; a resumed pane starts bound to its thread.
    pub fn new(spec: &LaunchSpec) -> Self {
        let mut meta = SessionMeta::default();
        meta.set_cwd(&spec.cwd);
        if let Some(thread) = &spec.resume {
            meta.set_session_ref(thread);
        }
        Self {
            meta,
            bound: spec.resume.clone(),
            ..Self::default()
        }
    }

    /// The thread whose rollout this pane follows: the first `session_meta` it was fed, or the resumed thread.
    pub fn bound_thread(&self) -> Option<&str> {
        self.bound.as_deref()
    }

    /// Keeps a notify's thread until a rollout names it; the title-generation turn's thread never gets one (R28).
    fn remember_turn(&mut self, thread: &str) {
        if self.pending_turns.iter().any(|t| t == thread) {
            return;
        }
        if self.pending_turns.len() == PENDING_TURNS {
            self.pending_turns.remove(0);
            self.stats.forgotten_turns += 1;
        }
        self.pending_turns.push(thread.to_owned());
    }

    fn on_notify(&mut self, envelope: &HookEnvelope) -> Result<Vec<AdapterSignal>> {
        if envelope.cli != AgentCli::Codex {
            return Err(Error::WrongCli {
                expected: "codex",
                found: cli_name(envelope.cli),
            });
        }
        let notify = NotifyPayload::from_value(&envelope.payload)?;
        if !notify.is_turn_complete() {
            self.stats.unknown_hook_events += 1;
            return Ok(vec![]);
        }
        if self.bound.as_deref() == Some(notify.thread_id.as_str()) {
            return Ok(vec![status(StatusSignal::TurnComplete)]);
        }
        // Another thread counts once its rollout binds: the pane's own before binding, a `/new` one after (R49), never the title turn's (R28).
        self.remember_turn(&notify.thread_id);
        Ok(vec![AdapterSignal::FindRollout {
            thread_id: notify.thread_id,
        }])
    }

    /// One rollout line; a `history` line (the thread's past, before this process) never starts or ends a turn.
    fn on_rollout_line(&mut self, line: &[u8], history: bool) -> Result<Vec<AdapterSignal>> {
        let record = parse_record(line).inspect_err(|_| self.stats.malformed_rollout_lines += 1)?;
        let mut signals = Vec::new();
        match record {
            RolloutRecord::Turn(_) if history => {}
            RolloutRecord::SessionMeta(meta) => {
                let switched = std::mem::take(&mut self.switched);
                let rebind = self.bound.as_ref().is_some_and(|b| *b != meta.thread_id)
                    && (switched || self.pending_turns.contains(&meta.thread_id));
                if rebind {
                    self.bound = Some(meta.thread_id.clone());
                    self.turn = None;
                    if self.progress.take().is_some() {
                        signals.push(AdapterSignal::Progress(None));
                    }
                }
                let thread = self.bound.get_or_insert_with(|| meta.thread_id.clone());
                if *thread != meta.thread_id {
                    return Ok(signals);
                }
                let changed =
                    self.meta.set_session_ref(&meta.thread_id) | self.meta.set_cwd(&meta.cwd);
                if changed {
                    signals.push(AdapterSignal::Meta(self.meta.clone()));
                }
                if self.pending_turns.contains(&meta.thread_id) {
                    signals.push(status(StatusSignal::TurnComplete));
                }
                self.pending_turns.clear();
            }
            RolloutRecord::TurnContext(context) => {
                let mut changed = false;
                if let Some(model) = &context.model {
                    changed |= self.meta.set_model(model);
                }
                if let Some(cwd) = &context.cwd {
                    changed |= self.meta.set_cwd(cwd);
                }
                if changed {
                    signals.push(AdapterSignal::Meta(self.meta.clone()));
                }
            }
            RolloutRecord::PlanUpdate(plan) => {
                let progress = plan.progress();
                if progress != self.progress {
                    self.progress = progress;
                    signals.push(AdapterSignal::Progress(self.progress.clone()));
                }
            }
            RolloutRecord::Turn(TurnEvent::Started { turn_id }) => {
                self.turn = turn_id;
                signals.push(status(StatusSignal::TurnStarted));
            }
            RolloutRecord::Turn(
                TurnEvent::Complete { turn_id } | TurnEvent::Aborted { turn_id },
            ) => {
                let current = self.turn.is_none() || turn_id.is_none() || self.turn == turn_id;
                if current {
                    self.turn = None;
                    signals.push(status(StatusSignal::TurnComplete));
                }
            }
            RolloutRecord::Ignored => {}
            RolloutRecord::Unknown(_) | RolloutRecord::UnknownEvent(_) => {
                self.stats.unknown_rollout_records += 1;
            }
        }
        Ok(signals)
    }
}

impl AgentSession for CodexSession {
    fn handle(&mut self, event: AgentEvent<'_>) -> Result<Vec<AdapterSignal>> {
        match event {
            AgentEvent::Hook(envelope) => self.on_notify(envelope),
            AgentEvent::Osc9(body) => Ok(vec![status(match classify_osc9(body) {
                Osc9Kind::Approval => StatusSignal::PermissionRequested {
                    call: None,
                    detail: Some(body.to_owned()),
                },
                Osc9Kind::PlanPrompt | Osc9Kind::Question => StatusSignal::InputRequested {
                    detail: Some(body.to_owned()),
                },
                Osc9Kind::TurnComplete => StatusSignal::TurnComplete,
            })]),
            AgentEvent::RolloutLine(line) => self.on_rollout_line(line, false),
            AgentEvent::RolloutHistory(line) => self.on_rollout_line(line, true),
            AgentEvent::KeyTyped { enter } => {
                let mut signals = vec![status(StatusSignal::KeyTyped)];
                if enter {
                    signals.push(status(StatusSignal::PromptSubmitted));
                }
                Ok(signals)
            }
            AgentEvent::FirstOutput => Ok(vec![status(StatusSignal::Ready)]),
            AgentEvent::RolloutSwitched => {
                self.switched = true;
                Ok(vec![])
            }
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
