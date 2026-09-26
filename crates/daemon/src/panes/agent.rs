//! One agent process's integration inside its pane task: the `ply-agents` session, the spec 6.3 machine, the
//! `pane.progress` rate limit, the R17 quiet timer and, for Codex, the rollout tailer and the R48 turn wait.
//!
//! The pane task feeds it everything it observes for the process: C3 envelopes, OSC 9 bodies, the first pty byte, pty
//! activity and keys typed, and the tailer's messages, which it reads last. [`Agent`] hands each to the session, turns
//! the resulting signals into `pane.status` (through [`StatusMachine`]), `pane.progress` (at most 4 a second),
//! `pane.meta` and the stored session record, and asks the tailer for a notify's thread. An Enter that makes a Codex
//! pane `running` is only a guess until the rollout's `task_started` confirms it: without one within
//! [`TURN_START_WAIT`] the pane is `idle` again, and meanwhile the tailer looks for a thread `/new` may have started
//! (R49). It also owns the pane's [`Dispatch`] (Ruling R60): every signal and status goes through it, it asks the
//! registry for the queue's head and records a typed task's progress there, and it leaves the paste and the Enter it
//! decides on in [`Agent::take_writes`] for the pane task, which owns the terminal. It runs entirely on the pane task,
//! one event at a time.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use ply_agents::{
    Adapter, AdapterSignal, AgentEvent, AgentSession, LaunchSpec, StatusSignal, adapter,
};
use ply_proto::hook::HookEnvelope;
use ply_proto::pane::{AgentCli, PaneId, PaneStatus, TaskId};
use tokio::sync::mpsc;

use crate::branch;
use crate::daemon::{Shared, unix_now};
use crate::panes::dispatch::{Action, Dispatch};
use crate::panes::state::{ProgressGate, StatusMachine, Step};
use crate::tail::{TailMsg, TailOptions, Tailer};

/// How long a Codex pane an Enter made `running` waits for its rollout's `task_started` before it is `idle` again (R48).
pub const TURN_START_WAIT: Duration = Duration::from_secs(3);

/// What the pane task writes into the pty for a queued task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// The task's text, as a paste encoded against the pane's modes.
    Paste(String),
    /// One Enter key press and release.
    Enter,
}

/// The integration of one agent process; see the module docs.
pub struct Agent {
    pane_id: PaneId,
    cli: AgentCli,
    adapter: &'static dyn Adapter,
    dispatch: Dispatch,
    forced: Option<TaskId>,
    writes: Vec<Typed>,
    session: Box<dyn AgentSession>,
    machine: StatusMachine,
    quiet: Option<Duration>,
    active_at: Instant,
    seen_output: bool,
    progress: ProgressGate,
    turn_wait: Option<Instant>,
    tailer: Option<Tailer>,
    tail: Option<mpsc::Receiver<TailMsg>>,
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
    pub fn new(shared: &Arc<Shared>, pane_id: PaneId, cli: AgentCli, spec: &LaunchSpec) -> Self {
        let adapter = adapter(cli);
        let (tailer, tail) = (cli == AgentCli::Codex)
            .then(|| codex_tailer(shared, pane_id, spec))
            .flatten()
            .unzip();
        let machine = StatusMachine::new(cli);
        Self {
            pane_id,
            cli,
            adapter,
            dispatch: Dispatch::new(machine.status(), Instant::now()),
            forced: None,
            writes: Vec::new(),
            session: adapter.new_session(spec),
            machine,
            quiet: adapter.quiet_timeout(),
            active_at: Instant::now(),
            seen_output: false,
            progress: ProgressGate::default(),
            turn_wait: None,
            tailer,
            tail,
        }
    }

    /// Whether the pane task should read [`Agent::next_tail`]: a Codex process whose tailer still runs.
    pub fn tailing(&self) -> bool {
        self.tail.is_some()
    }

    /// The tailer's next message; `None` once it stopped (then [`Agent::tailing`] is false). Waits forever without a tailer.
    pub async fn next_tail(&mut self) -> Option<TailMsg> {
        let msg = match self.tail.as_mut() {
            Some(rx) => rx.recv().await,
            None => std::future::pending().await,
        };
        if msg.is_none() {
            self.tail = None;
        }
        msg
    }

    /// One message of the pane's rollout tailer: a switch to another file (R49), lines the session reads, or the thread's past.
    pub fn on_tail(&mut self, shared: &Arc<Shared>, msg: TailMsg, now: Instant) {
        match msg {
            TailMsg::Switched => self.on_event(shared, AgentEvent::RolloutSwitched, now),
            TailMsg::Lines(lines) => {
                for line in &lines {
                    self.on_event(shared, AgentEvent::RolloutLine(line), now);
                }
            }
            TailMsg::History(lines) => {
                for line in &lines {
                    self.on_event(shared, AgentEvent::RolloutHistory(line), now);
                }
            }
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

    /// The pane's queue changed: its dispatch looks at it again once the pane has settled.
    pub fn nudge(&mut self, shared: &Arc<Shared>, now: Instant) {
        let actions = self.dispatch.nudge(now);
        self.run_dispatch(shared, actions, now);
    }

    /// `task.send`: type `task` at the pane's next settled moment, over the user's unsent typing and past a pause.
    pub fn send_now(&mut self, shared: &Arc<Shared>, task: TaskId, now: Instant) {
        self.forced = Some(task);
        let actions = self.dispatch.send_now(task, now);
        self.run_dispatch(shared, actions, now);
    }

    /// Bytes the user's own key or raw input wrote to the pty, which may leave unsent text in the CLI's input.
    pub fn on_user_input(&mut self, shared: &Arc<Shared>, bytes: &[u8], now: Instant) {
        let actions = self.dispatch.on_user_input(bytes, now);
        self.run_dispatch(shared, actions, now);
    }

    /// The user pasted into the pane.
    pub fn on_user_paste(&mut self, shared: &Arc<Shared>, now: Instant) {
        let actions = self.dispatch.on_user_paste(now);
        self.run_dispatch(shared, actions, now);
    }

    /// The terminal refused the queued task's paste; the task fails.
    pub fn paste_refused(&mut self, shared: &Arc<Shared>, now: Instant) {
        let actions = self.dispatch.paste_refused(now);
        self.run_dispatch(shared, actions, now);
    }

    /// What the dispatch decided to type since the last call, in order.
    pub fn take_writes(&mut self) -> Vec<Typed> {
        std::mem::take(&mut self.writes)
    }

    /// The next moment [`Agent::on_tick`] has work: the quiet timeout of a running Claude pane, a Codex turn wait, a held progress value or the dispatch's next step.
    pub fn deadline(&self) -> Option<Instant> {
        let quiet = self.quiet_deadline();
        [
            quiet,
            self.turn_wait,
            self.progress.due(),
            self.dispatch.deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
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
        if self.turn_wait.is_some_and(|at| at <= now) {
            self.turn_wait = None;
            tracing::debug!(pane_id = self.pane_id, "no task_started followed the Enter");
            self.step(shared, &StatusSignal::NoTurnStarted, now);
        }
        if let Some(progress) = self.progress.take_due(now) {
            shared.registry().set_progress(self.pane_id, progress);
        }
        let actions = self.dispatch.tick(now);
        self.run_dispatch(shared, actions, now);
    }

    /// The process ended: a typed task fails, the machine stops, the tailer stops, and what the session skipped is logged.
    pub fn exit(&mut self, shared: &Arc<Shared>, now: Instant) {
        let actions = self.dispatch.on_status(PaneStatus::Exited, now);
        self.run_dispatch(shared, actions, now);
        self.writes.clear();
        self.machine.exit();
        self.turn_wait = None;
        self.tailer = None;
        self.tail = None;
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

    /// Carries out the dispatch's actions: the registry's answers are fed back until only writes remain.
    fn run_dispatch(&mut self, shared: &Arc<Shared>, actions: Vec<Action>, now: Instant) {
        let mut queue: VecDeque<Action> = actions.into();
        while let Some(action) = queue.pop_front() {
            match action {
                Action::Check => {
                    let head = shared.registry().queue_head(self.pane_id);
                    queue.extend(self.dispatch.offer(head, now));
                }
                Action::Take(task) => {
                    let forced = self.forced.take() == Some(task);
                    let text = shared
                        .registry()
                        .take_task(self.pane_id, task, forced, unix_now());
                    if text.is_some() {
                        tracing::info!(
                            pane_id = self.pane_id,
                            task_id = task,
                            forced,
                            "typing a queued task"
                        );
                    }
                    queue.extend(self.dispatch.taken(task, text, now));
                }
                Action::Paste(text) => self.writes.push(Typed::Paste(text)),
                Action::Enter => self.writes.push(Typed::Enter),
                Action::Report {
                    task,
                    state,
                    detail,
                } => {
                    tracing::info!(
                        pane_id = self.pane_id,
                        task_id = task,
                        ?state,
                        detail,
                        "queued task progressed"
                    );
                    shared
                        .registry()
                        .task_progress(task, state, detail, unix_now());
                }
                Action::Block(reason) => shared.registry().block_queue(self.pane_id, reason),
            }
        }
    }

    fn step(&mut self, shared: &Arc<Shared>, signal: &StatusSignal, now: Instant) {
        let acknowledges = self.adapter.acknowledges_prompt(signal);
        let actions = self.dispatch.on_signal(signal, acknowledges, now);
        self.run_dispatch(shared, actions, now);
        let before = self.machine.status();
        match self.machine.apply(signal) {
            Step::To { status, detail } => {
                if status == PaneStatus::Running && before != PaneStatus::Running {
                    self.active_at = now;
                }
                if status != PaneStatus::Running || matches!(signal, StatusSignal::TurnStarted) {
                    self.turn_wait = None;
                } else if self.cli == AgentCli::Codex
                    && before == PaneStatus::Idle
                    && matches!(signal, StatusSignal::PromptSubmitted)
                {
                    self.turn_wait = Some(now + TURN_START_WAIT);
                    if let Some(tailer) = &self.tailer {
                        tailer.rediscover(SystemTime::now());
                    }
                }
                shared.set_status(self.pane_id, status, detail);
                let actions = self.dispatch.on_status(status, now);
                self.run_dispatch(shared, actions, now);
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
) -> Option<(Tailer, mpsc::Receiver<TailMsg>)> {
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
    match Tailer::spawn(Arc::clone(shared), options) {
        Ok(tailer) => Some(tailer),
        Err(e) => {
            tracing::warn!(pane_id, error = %e, "cannot follow the pane's rollout");
            None
        }
    }
}
