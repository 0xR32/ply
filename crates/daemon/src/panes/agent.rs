//! One agent process's integration inside its pane task: the `ply-agents` session, the spec 6.3 machine, the
//! `pane.progress` rate limit, the R17 quiet timer and, for Codex, the rollout tailer.
//!
//! The pane task feeds it everything it observes for the process: C3 envelopes, rollout lines, OSC 9 bodies, the
//! first pty byte, pty activity and keys typed. [`Agent`] hands each to the session, turns the resulting signals into
//! `pane.status` (through [`StatusMachine`]), `pane.progress` (at most 4 a second), `pane.meta` and the stored session
//! record, and asks the tailer for a notify's thread. It runs entirely on the pane task, one event at a time.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use ply_agents::{AdapterSignal, AgentEvent, AgentSession, LaunchSpec, StatusSignal, adapter};
use ply_proto::hook::HookEnvelope;
use ply_proto::pane::{AgentCli, PaneId, PaneStatus};
use tokio::sync::mpsc::WeakSender;

use crate::branch;
use crate::daemon::Shared;
use crate::panes::pane::PaneCmd;
use crate::panes::state::{ProgressGate, StatusMachine, Step};
use crate::tail::{TailOptions, Tailer};

/// The integration of one agent process; see the module docs.
pub struct Agent {
    pane_id: PaneId,
    cli: AgentCli,
    session: Box<dyn AgentSession>,
    machine: StatusMachine,
    quiet: Option<Duration>,
    active_at: Instant,
    seen_output: bool,
    progress: ProgressGate,
    tailer: Option<Tailer>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("pane_id", &self.pane_id)
            .field("cli", &self.cli)
            .field("machine", &self.machine)
            .field("tailing", &self.tailer.is_some())
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// The integration for a process of `cli` about to start from `spec`; Codex also starts following its rollout.
    pub fn new(
        shared: &Arc<Shared>,
        pane_id: PaneId,
        cli: AgentCli,
        spec: &LaunchSpec,
        pane: WeakSender<PaneCmd>,
    ) -> Self {
        let adapter = adapter(cli);
        let tailer = (cli == AgentCli::Codex)
            .then(|| codex_tailer(shared, pane_id, spec, pane))
            .flatten();
        Self {
            pane_id,
            cli,
            session: adapter.new_session(spec),
            machine: StatusMachine::new(cli),
            quiet: adapter.quiet_timeout(),
            active_at: Instant::now(),
            seen_output: false,
            progress: ProgressGate::default(),
            tailer,
        }
    }

    /// The machine's state and detail line, which the pane shows while this process runs.
    pub fn status(&self) -> (PaneStatus, Option<String>) {
        (
            self.machine.status(),
            self.machine.detail().map(str::to_owned),
        )
    }

    /// A C3 envelope for this pane (hook or notify); any hook also counts as activity for the quiet timer.
    pub fn on_hook(&mut self, shared: &Arc<Shared>, envelope: &HookEnvelope, now: Instant) {
        self.active_at = now;
        self.on_event(shared, AgentEvent::Hook(envelope), now);
    }

    /// Complete lines of the pane's rollout (Codex).
    pub fn on_rollout(&mut self, shared: &Arc<Shared>, lines: &[Vec<u8>], now: Instant) {
        for line in lines {
            self.on_event(shared, AgentEvent::RolloutLine(line), now);
        }
    }

    /// The body of one OSC 9 notification from the pane's pty (C8).
    pub fn on_osc9(&mut self, shared: &Arc<Shared>, body: &str, now: Instant) {
        self.on_event(shared, AgentEvent::Osc9(body), now);
    }

    /// The pty produced output: activity for the quiet timer, and the first output byte signal once per process.
    pub fn on_output(&mut self, shared: &Arc<Shared>, now: Instant) {
        self.active_at = now;
        if !self.seen_output {
            self.seen_output = true;
            self.on_event(shared, AgentEvent::FirstOutput, now);
        }
    }

    /// The user typed into the pane; `enter` when the input submits a line.
    pub fn on_key(&mut self, shared: &Arc<Shared>, enter: bool, now: Instant) {
        self.on_event(shared, AgentEvent::KeyTyped { enter }, now);
    }

    /// The next moment [`Agent::on_tick`] has work: the quiet timeout of a running Claude pane or a held progress value.
    pub fn deadline(&self) -> Option<Instant> {
        let quiet = self.quiet_deadline();
        [quiet, self.progress.due()].into_iter().flatten().min()
    }

    /// Raises the quiet timeout (R17) and sends a held progress value once their time has come; a past deadline never stays due.
    pub fn on_tick(&mut self, shared: &Arc<Shared>, now: Instant) {
        if self.quiet_deadline().is_some_and(|at| at <= now) {
            self.step(shared, &StatusSignal::QuietTimeout, now);
            // A due deadline the machine did not act on would wake the pane task again at once, forever.
            if self.quiet_deadline().is_some_and(|at| at <= now) {
                tracing::warn!(
                    pane_id = self.pane_id,
                    status = ?self.machine.status(),
                    "the quiet timeout changed nothing; restarting it"
                );
                self.active_at = now;
            }
        }
        if let Some(progress) = self.progress.take_due(now) {
            shared.registry().set_progress(self.pane_id, progress);
        }
    }

    /// The process ended: the machine stops, the tailer stops, and what the session skipped is logged.
    pub fn exit(&mut self) {
        self.machine.exit();
        self.tailer = None;
        let stats = self.session.stats();
        tracing::info!(
            pane_id = self.pane_id,
            cli = ?self.cli,
            ignored_signals = self.machine.ignored(),
            unknown_hook_events = stats.unknown_hook_events,
            unknown_rollout_records = stats.unknown_rollout_records,
            malformed_rollout_lines = stats.malformed_rollout_lines,
            unreadable_progress = stats.unreadable_progress,
            forgotten_turns = stats.forgotten_turns,
            "agent session finished"
        );
    }

    fn quiet_deadline(&self) -> Option<Instant> {
        let quiet = self.quiet?;
        (self.machine.status() == PaneStatus::Running && !self.machine.has_ended())
            .then(|| self.active_at + quiet)
    }

    fn on_event(&mut self, shared: &Arc<Shared>, event: AgentEvent<'_>, now: Instant) {
        let signals = match self.session.handle(event) {
            Ok(signals) => signals,
            Err(e) => {
                tracing::warn!(pane_id = self.pane_id, error = %e, "malformed agent input ignored");
                return;
            }
        };
        for signal in signals {
            match signal {
                AdapterSignal::Status(s) => self.step(shared, &s, now),
                AdapterSignal::Progress(p) => {
                    if let Some(p) = self.progress.offer(p, now) {
                        shared.registry().set_progress(self.pane_id, p);
                    }
                }
                AdapterSignal::Meta(meta) => {
                    let moved = shared.registry().set_meta(self.pane_id, &meta);
                    if let (true, Some(cwd)) = (moved, meta.cwd) {
                        branch::lookup(shared, self.pane_id, cwd);
                    }
                }
                AdapterSignal::FindRollout { thread_id } => match &self.tailer {
                    Some(tailer) => tailer.find(thread_id),
                    None => {
                        tracing::debug!(
                            pane_id = self.pane_id,
                            thread_id,
                            "no rollout tailer for this pane"
                        );
                    }
                },
            }
        }
    }

    fn step(&mut self, shared: &Arc<Shared>, signal: &StatusSignal, now: Instant) {
        let before = self.machine.status();
        match self.machine.apply(signal) {
            Step::To { status, detail } => {
                if status == PaneStatus::Running && before != PaneStatus::Running {
                    self.active_at = now;
                }
                shared.set_status(self.pane_id, status, detail);
            }
            Step::SessionEnded => {
                let reason = match signal {
                    StatusSignal::SessionEnded { reason } => reason.as_deref(),
                    _ => None,
                };
                tracing::debug!(
                    pane_id = self.pane_id,
                    reason,
                    status = ?before,
                    "the CLI ended a session; the status stays until its next session or the process exit"
                );
            }
            Step::Ignored => {
                tracing::trace!(pane_id = self.pane_id, ?signal, status = ?before, ignored = self.machine.ignored(), "no transition for this signal");
            }
        }
    }
}

fn codex_tailer(
    shared: &Arc<Shared>,
    pane_id: PaneId,
    spec: &LaunchSpec,
    pane: WeakSender<PaneCmd>,
) -> Option<Tailer> {
    let env = shared.login.env_for(&spec.env);
    let codex_home = env.get("CODEX_HOME").map(PathBuf::from).or_else(|| {
        env.get("HOME")
            .map(|home| PathBuf::from(home).join(".codex"))
    });
    let Some(codex_home) = codex_home else {
        tracing::warn!(
            pane_id,
            "no CODEX_HOME or HOME for the pane; its rollout is not followed"
        );
        return None;
    };
    let options = TailOptions {
        pane_id,
        codex_home,
        cwd: spec.cwd.clone(),
        spawned_at: SystemTime::now(),
        thread: spec.resume.clone(),
    };
    match Tailer::spawn(Arc::clone(shared), options, pane) {
        Ok(tailer) => Some(tailer),
        Err(e) => {
            tracing::warn!(pane_id, error = %e, "cannot follow the pane's rollout");
            None
        }
    }
}
