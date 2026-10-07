//! When plyd types a queued task into its pane (Ruling R60), as a small clocked state machine per agent process.
//!
//! The machine owns no queue: the registry holds the tasks, and the machine asks for the queue's head when typing may
//! be possible ([`Action::Check`]: when the process starts, whenever the pane turns `idle`, when the process first shows
//! its prompt, when the user's input clears, and on a nudge), claims it ([`Action::Take`]; the registry marks it `sent`
//! if it is still the head) and follows it. It types only into a process that has shown its prompt
//! ([`Sense::shows_prompt`]: a startup screen must never get a queued Enter, [`BlockReason::Startup`]), into a pane
//! that has been `idle` for [`SETTLE`], and whose input holds nothing the user typed since their last prompt: any key,
//! raw input or paste of theirs marks the input as typed, except a bare Enter and keys that answer the CLI's dialog or
//! question. Only Ctrl+C, Ctrl+U, the CLI acknowledging a prompt ([`Sense::acknowledges`]) or starting a new session
//! clears it; the user's Enter does not, since it may add a line to a draft or open a local command's picker. While
//! typed, the head waits and the queue is blocked ([`BlockReason::Typing`]) until the input clears or the user sends
//! the task anyway (`task.send`).
//!
//! A task is written as one paste (bracketed when the CLI enabled it) and, [`ENTER_DELAY`] later, one Enter, so an
//! autocomplete pop-over the paste opens has settled, or [`IMAGE_ENTER_DELAY`] later when the CLI reads images from the
//! paste and would drop an Enter that came first; the Enter is left out when the pane stopped being `idle` or the
//! user typed since the paste. The CLI's acknowledgement makes the task `running`: Claude Code's UserPromptSubmit,
//! Codex's rollout `task_started` (the adapter names it). The turn ending (Stop, notify, `task_complete`, or the quiet
//! timeout) makes it `ended`; a Codex Enter that started no turn, no acknowledgement within [`ACK_WAIT`], or the process
//! exiting makes it `failed`, and a task that failed unsubmitted leaves its text in the input, which then counts as
//! typed. The machine never answers a dialog: a task whose pane waits for the user stays `running`.

use std::time::{Duration, Instant};

use ply_agents::StatusSignal;
use ply_proto::pane::{BlockReason, PaneStatus, TaskId, TaskState};

/// How long a pane must stay `idle` before a task is typed into it.
pub const SETTLE: Duration = Duration::from_secs(1);

/// Time between a task's paste and its Enter.
pub const ENTER_DELAY: Duration = Duration::from_millis(50);

/// How long the Enter waits after a paste the CLI reads images from (`Adapter::paste_reads_images`): Claude Code 2.1.283 drops an Enter typed meanwhile, and took 0.41–0.55 s for one to three Retina screenshots on an Apple-silicon Mac, idle or with every core busy.
pub const IMAGE_ENTER_DELAY: Duration = Duration::from_secs(2);

/// How long the CLI has to acknowledge a typed task before it fails as not submitted.
pub const ACK_WAIT: Duration = Duration::from_secs(10);

/// Detail of a task the CLI never acknowledged.
pub const NOT_SUBMITTED: &str = "not submitted: the CLI did not acknowledge the prompt";

/// Detail of a task whose paste the pane's terminal refused, because it would act as keys there (Ruling R21).
pub const PASTE_REFUSED: &str =
    "the pane refused the paste: its CLI takes no pasted text with line breaks";

/// Detail of a task whose pane's process exited while it ran.
pub const PROCESS_EXITED: &str = "the pane's process exited";

/// What the machine asks of the pane task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Ask the registry for the head of the pane's queue and answer with [`Dispatch::offer`].
    Check,
    /// Claim task `id` in the registry (it becomes `sent`) and answer with [`Dispatch::taken`].
    Take(TaskId),
    /// Write the text into the pane as a paste.
    Paste(String),
    /// Press Enter in the pane.
    Enter,
    /// The typed task reached `state`.
    Report {
        /// The task.
        task: TaskId,
        /// `running`, `ended` or `failed`.
        state: TaskState,
        /// Why it failed.
        detail: Option<String>,
    },
    /// The queue is blocked (`Some`) or free again (`None`).
    Block(Option<BlockReason>),
}

/// What the pane's adapter makes of a status signal, for [`Dispatch::on_signal`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sense {
    /// The CLI confirmed it took a prompt (`Adapter::acknowledges_prompt`).
    pub acknowledges: bool,
    /// The process is past its startup screens at its prompt (`Adapter::shows_prompt`).
    pub shows_prompt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Waiting,
    Taking { task: TaskId },
    Pasted { task: TaskId, enter_at: Instant },
    Sent { task: TaskId, deadline: Instant },
    Running { task: TaskId },
}

/// How long a pane must stay quiet after the user pressed Enter in it: their own prompt goes first.
pub const SETTLE_AFTER_ENTER: Duration = Duration::from_secs(3);

/// One agent process's typing machine; see the module docs.
#[derive(Debug)]
pub struct Dispatch {
    phase: Phase,
    status: PaneStatus,
    idle_since: Option<Instant>,
    quiet_until: Option<Instant>,
    check: bool,
    live: bool,
    typed: bool,
    interrupted: bool,
    blocked: Option<BlockReason>,
    force: Option<TaskId>,
}

/// Whether bytes written for one key clear the CLI's input: Ctrl+C or Ctrl+U, in the legacy or the kitty encoding.
fn clears_input(bytes: &[u8]) -> bool {
    bytes == b"\x03"
        || bytes == b"\x15"
        || bytes.starts_with(b"\x1b[99;5u")
        || bytes.starts_with(b"\x1b[117;5u")
}

/// Whether the bytes are Enter alone, which puts no text into an empty input.
fn bare_enter(bytes: &[u8]) -> bool {
    bytes == b"\r" || bytes == b"\x1b[13u"
}

impl Dispatch {
    /// A machine for a new process in `status`; it looks at the queue once the pane has settled.
    pub fn new(status: PaneStatus, now: Instant) -> Self {
        Self {
            phase: Phase::Waiting,
            status,
            idle_since: (status == PaneStatus::Idle).then_some(now),
            quiet_until: None,
            check: true,
            live: false,
            typed: false,
            interrupted: false,
            blocked: None,
            force: None,
        }
    }

    /// Whether the user typed into the pane since their last prompt.
    pub fn typed(&self) -> bool {
        self.typed
    }

    /// Whether the pane may take a pool task: its process showed its prompt and its input holds no typing.
    pub fn may_claim(&self) -> bool {
        self.live && !self.typed()
    }

    /// Whether the process can stop without losing anything: the pane settled `idle` (as before typing a task), no task
    /// in flight or forced, and nothing of the user's in its input.
    pub fn at_rest(&self, now: Instant) -> bool {
        self.settled_at().is_some_and(|at| at <= now) && !self.typed && self.force.is_none()
    }

    /// The pane's queue changed: look at it again once the pane is settled.
    pub fn nudge(&mut self, now: Instant) -> Vec<Action> {
        self.check = true;
        self.poll(now)
    }

    /// `task.send`: type `task` at the pane's next settled moment, even over the user's unsent typing; never before
    /// the process has shown its prompt.
    pub fn send_now(&mut self, task: TaskId, now: Instant) -> Vec<Action> {
        self.force = Some(task);
        self.poll(now)
    }

    /// The pane's status changed without a signal (the process exited).
    pub fn on_status(&mut self, status: PaneStatus, now: Instant) -> Vec<Action> {
        self.set_status(status, now);
        if matches!(status, PaneStatus::Exited | PaneStatus::Lost)
            && let Some(task) = self.in_flight()
        {
            self.phase = Phase::Waiting;
            return vec![failed(task, PROCESS_EXITED)];
        }
        self.poll(now)
    }

    /// A status signal of the process, as the adapter reads it, and the pane's status after the machine applied it.
    pub fn on_signal(
        &mut self,
        signal: &StatusSignal,
        sense: Sense,
        status: PaneStatus,
        now: Instant,
    ) -> Vec<Action> {
        self.set_status(status, now);
        if sense.shows_prompt && !self.live {
            self.live = true;
            self.check = true;
        }
        if sense.acknowledges || matches!(signal, StatusSignal::Ready) {
            self.clear_typed();
        }
        let mut out = Vec::new();
        match (self.phase, signal) {
            (Phase::Pasted { task, .. } | Phase::Sent { task, .. }, _) if sense.acknowledges => {
                self.interrupted = false;
                self.phase = Phase::Running { task };
                out.push(Action::Report {
                    task,
                    state: TaskState::Running,
                    detail: None,
                });
            }
            (Phase::Sent { task, .. }, StatusSignal::NoTurnStarted) => {
                self.not_submitted(now);
                out.push(failed(task, NOT_SUBMITTED));
            }
            (Phase::Running { task }, StatusSignal::TurnComplete | StatusSignal::QuietTimeout) => {
                self.finish(now);
                out.push(Action::Report {
                    task,
                    state: TaskState::Ended,
                    detail: None,
                });
            }
            _ => {}
        }
        out.extend(self.poll(now));
        out
    }

    /// Bytes the user's own key or raw input wrote to the pty.
    pub fn on_user_input(&mut self, bytes: &[u8], now: Instant) -> Vec<Action> {
        self.interrupt();
        if clears_input(bytes) {
            self.clear_typed();
        } else if !self.answering() && !bare_enter(bytes) {
            self.typed = true;
        }
        let quiet = if crate::osc::is_enter(bytes) {
            SETTLE_AFTER_ENTER
        } else {
            SETTLE
        };
        self.quiet(now + quiet);
        self.poll(now)
    }

    /// The user pasted into the pane.
    pub fn on_user_paste(&mut self, now: Instant) -> Vec<Action> {
        self.interrupt();
        if !self.answering() {
            self.typed = true;
        }
        self.quiet(now + SETTLE);
        self.poll(now)
    }

    /// The registry's answer to [`Action::Check`]: the head of the queue, if any.
    pub fn offer(&mut self, head: Option<TaskId>, now: Instant) -> Vec<Action> {
        let _ = now;
        if self.phase != Phase::Waiting {
            return Vec::new();
        }
        match head {
            None => self.block(None),
            Some(_) if !self.live => self.block(Some(BlockReason::Startup)),
            Some(_) if self.typed => self.block(Some(BlockReason::Typing)),
            Some(task) => {
                let mut out = self.block(None);
                self.phase = Phase::Taking { task };
                out.push(Action::Take(task));
                out
            }
        }
    }

    /// The registry's answer to [`Action::Take`]: the task's text, or `None` when it is no longer the head; `reads_images` (the adapter's `paste_reads_images`) makes the Enter wait [`IMAGE_ENTER_DELAY`].
    pub fn taken(
        &mut self,
        task: TaskId,
        text: Option<String>,
        reads_images: bool,
        now: Instant,
    ) -> Vec<Action> {
        if self.phase != (Phase::Taking { task }) {
            return Vec::new();
        }
        match text {
            Some(text) => {
                self.interrupted = false;
                let delay = if reads_images {
                    IMAGE_ENTER_DELAY
                } else {
                    ENTER_DELAY
                };
                self.phase = Phase::Pasted {
                    task,
                    enter_at: now + delay,
                };
                vec![Action::Paste(text)]
            }
            None => {
                self.phase = Phase::Waiting;
                self.check = true;
                self.poll(now)
            }
        }
    }

    /// The terminal refused the paste of the task being typed; it fails and no Enter follows.
    pub fn paste_refused(&mut self, now: Instant) -> Vec<Action> {
        let Phase::Pasted { task, .. } = self.phase else {
            return Vec::new();
        };
        self.finish(now);
        vec![failed(task, PASTE_REFUSED)]
    }

    /// Time passed.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        self.poll(now)
    }

    /// The next moment [`Dispatch::tick`] has work, if any.
    pub fn deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Waiting if self.check || (self.live && self.force.is_some()) => {
                self.settled_at()
            }
            Phase::Pasted { enter_at, .. } => Some(enter_at),
            Phase::Sent { deadline, .. } => Some(deadline),
            Phase::Waiting | Phase::Taking { .. } | Phase::Running { .. } => None,
        }
    }

    fn poll(&mut self, now: Instant) -> Vec<Action> {
        match self.phase {
            Phase::Waiting => {
                if !self.settled_at().is_some_and(|at| at <= now) {
                    return Vec::new();
                }
                if self.live
                    && let Some(task) = self.force.take()
                {
                    self.check = false;
                    let mut out = self.block(None);
                    self.phase = Phase::Taking { task };
                    out.push(Action::Take(task));
                    return out;
                }
                if std::mem::take(&mut self.check) {
                    return vec![Action::Check];
                }
                Vec::new()
            }
            Phase::Pasted { task, enter_at } if enter_at <= now => {
                self.phase = Phase::Sent {
                    task,
                    deadline: now + ACK_WAIT,
                };
                if std::mem::take(&mut self.interrupted) || self.status != PaneStatus::Idle {
                    return Vec::new();
                }
                vec![Action::Enter]
            }
            Phase::Sent { task, deadline } if deadline <= now => {
                self.not_submitted(now);
                vec![failed(task, NOT_SUBMITTED)]
            }
            _ => Vec::new(),
        }
    }

    /// When a waiting machine may type: the pane idle for [`SETTLE`] and past the user's last key's quiet time.
    fn settled_at(&self) -> Option<Instant> {
        if self.status != PaneStatus::Idle || self.phase != Phase::Waiting {
            return None;
        }
        let settled = self.idle_since? + SETTLE;
        Some(self.quiet_until.map_or(settled, |q| q.max(settled)))
    }

    fn set_status(&mut self, status: PaneStatus, now: Instant) {
        let was = self.status;
        self.status = status;
        if status != PaneStatus::Idle {
            self.idle_since = None;
        } else if was != PaneStatus::Idle || self.idle_since.is_none() {
            self.idle_since = Some(now);
            self.check = true;
        }
    }

    /// Keys of the user's that go to the CLI's dialog or question, not into its prompt's input.
    fn answering(&self) -> bool {
        matches!(
            self.status,
            PaneStatus::WaitingPermission | PaneStatus::WaitingInput
        )
    }

    /// The user wrote to the pane between a task's paste and its Enter, which is then left out.
    fn interrupt(&mut self) {
        if matches!(self.phase, Phase::Pasted { .. }) {
            self.interrupted = true;
        }
    }

    /// A typed task is over: look at the queue again once the pane has settled anew.
    fn finish(&mut self, now: Instant) {
        self.phase = Phase::Waiting;
        self.check = true;
        self.quiet(now + SETTLE);
    }

    /// A typed task failed unsubmitted: its text may still be in the CLI's input.
    fn not_submitted(&mut self, now: Instant) {
        self.finish(now);
        self.typed = true;
    }

    fn quiet(&mut self, until: Instant) {
        self.quiet_until = Some(self.quiet_until.map_or(until, |q| q.max(until)));
    }

    fn clear_typed(&mut self) {
        if std::mem::take(&mut self.typed) {
            self.check = true;
        }
    }

    fn block(&mut self, reason: Option<BlockReason>) -> Vec<Action> {
        if self.blocked == reason {
            return Vec::new();
        }
        self.blocked = reason;
        vec![Action::Block(reason)]
    }

    fn in_flight(&self) -> Option<TaskId> {
        match self.phase {
            Phase::Taking { task }
            | Phase::Pasted { task, .. }
            | Phase::Sent { task, .. }
            | Phase::Running { task } => Some(task),
            Phase::Waiting => None,
        }
    }
}

fn failed(task: TaskId, detail: &str) -> Action {
    Action::Report {
        task,
        state: TaskState::Failed,
        detail: Some(detail.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    fn none() -> Sense {
        Sense::default()
    }

    fn ack() -> Sense {
        Sense {
            acknowledges: true,
            shows_prompt: false,
        }
    }

    fn shows() -> Sense {
        Sense {
            acknowledges: false,
            shows_prompt: true,
        }
    }

    /// A machine whose process has shown its prompt and turned idle at `t0`.
    fn live(t0: Instant) -> Dispatch {
        let mut d = Dispatch::new(PaneStatus::Starting, t0);
        d.on_signal(&StatusSignal::Ready, shows(), PaneStatus::Idle, t0);
        d
    }

    /// A machine for an idle pane that has settled and been nudged, answered with head `task`.
    fn ready(task: TaskId) -> (Dispatch, Instant) {
        let t0 = Instant::now();
        let mut d = live(t0);
        d.nudge(t0);
        let t = at(t0, 1000);
        assert_eq!(d.tick(t), [Action::Check]);
        assert_eq!(d.offer(Some(task), t), [Action::Take(task)]);
        (d, t)
    }

    fn report(task: TaskId, state: TaskState, detail: Option<&str>) -> Action {
        Action::Report {
            task,
            state,
            detail: detail.map(str::to_owned),
        }
    }

    #[test]
    fn it_looks_at_the_queue_only_once_the_pane_has_been_idle_for_the_settle_time() {
        let t0 = Instant::now();
        let mut d = Dispatch::new(PaneStatus::Starting, t0);
        assert!(d.nudge(t0).is_empty(), "a starting pane is not ready");
        assert!(
            d.on_signal(&StatusSignal::Ready, shows(), PaneStatus::Idle, at(t0, 100))
                .is_empty()
        );
        assert_eq!(d.deadline(), Some(at(t0, 1100)));
        assert!(d.tick(at(t0, 1099)).is_empty());
        assert_eq!(d.tick(at(t0, 1100)), [Action::Check]);
        assert!(d.offer(None, at(t0, 1100)).is_empty());
        assert!(
            d.tick(at(t0, 5000)).is_empty(),
            "an empty queue is not asked again until something changes"
        );
        assert_eq!(d.deadline(), None);
        assert_eq!(
            d.nudge(at(t0, 6000)),
            [Action::Check],
            "settled already: asked at once"
        );
    }

    #[test]
    fn a_pane_is_at_rest_once_settled_with_nothing_typed_or_in_flight() {
        let t0 = Instant::now();
        let mut d = live(t0);
        assert!(!d.at_rest(at(t0, 999)), "not yet settled");
        assert!(d.at_rest(at(t0, 1000)));
        d.on_user_input(b"draft", at(t0, 2000));
        assert!(!d.at_rest(at(t0, 9000)), "the user's unsent typing");
        d.on_user_input(b"\x15", at(t0, 9000));
        assert!(!d.at_rest(at(t0, 9999)), "quiet after the user's last key");
        assert!(d.at_rest(at(t0, 10_000)), "Ctrl+U cleared the input");
        d.on_signal(
            &StatusSignal::PromptSubmitted,
            ack(),
            PaneStatus::Running,
            at(t0, 11_000),
        );
        assert!(!d.at_rest(at(t0, 20_000)), "a turn runs");

        let (mut d, t) = ready(7);
        assert!(!d.at_rest(t), "a task is being typed");
        d.taken(7, Some("task".to_owned()), false, t);
        assert!(!d.at_rest(at(t0, 60_000)));
    }

    #[test]
    fn no_task_goes_before_the_process_shows_its_prompt() {
        let t0 = Instant::now();
        let mut d = Dispatch::new(PaneStatus::Idle, t0);
        let t = at(t0, 1000);
        assert_eq!(d.tick(t), [Action::Check]);
        assert_eq!(
            d.offer(Some(7), t),
            [Action::Block(Some(BlockReason::Startup))],
            "Codex is idle from its first byte, which a trust, hooks or update screen prints too"
        );
        assert!(!d.may_claim(), "no pool task either");
        assert!(d.send_now(7, t).is_empty(), "not even one sent by hand");
        assert_eq!(d.deadline(), None, "and no busy wait for it");
        d.on_signal(
            &StatusSignal::TurnStarted,
            shows(),
            PaneStatus::Running,
            at(t0, 2000),
        );
        d.on_signal(
            &StatusSignal::TurnComplete,
            shows(),
            PaneStatus::Idle,
            at(t0, 3000),
        );
        assert!(d.may_claim());
        assert_eq!(
            d.tick(at(t0, 4000)),
            [Action::Block(None), Action::Take(7)],
            "the task sent by hand goes first"
        );
    }

    #[test]
    fn a_running_pane_is_never_typed_into() {
        let t0 = Instant::now();
        let mut d = live(t0);
        d.on_status(PaneStatus::Running, t0);
        d.nudge(t0);
        for status in [
            PaneStatus::Running,
            PaneStatus::WaitingPermission,
            PaneStatus::WaitingInput,
            PaneStatus::Lost,
        ] {
            d.on_status(status, t0);
            assert!(d.tick(at(t0, 5000)).is_empty(), "{status:?}");
        }
    }

    #[test]
    fn the_head_is_pasted_then_entered_after_the_delay_and_waits_for_the_acknowledgement() {
        let (mut d, t) = ready(7);
        assert_eq!(
            d.taken(7, Some("/review-pr #1".into()), false, t),
            [Action::Paste("/review-pr #1".into())]
        );
        assert_eq!(d.deadline(), Some(t + ENTER_DELAY));
        assert!(
            d.tick(t + ENTER_DELAY - Duration::from_millis(1))
                .is_empty()
        );
        assert_eq!(d.tick(t + ENTER_DELAY), [Action::Enter]);
        assert_eq!(d.deadline(), Some(t + ENTER_DELAY + ACK_WAIT));
        assert!(
            d.on_signal(
                &StatusSignal::PromptSubmitted,
                none(),
                PaneStatus::Running,
                t
            )
            .is_empty(),
            "Codex's Enter guess is no acknowledgement"
        );
        assert_eq!(
            d.on_signal(&StatusSignal::TurnStarted, ack(), PaneStatus::Running, t),
            [report(7, TaskState::Running, None)]
        );
        assert_eq!(d.deadline(), None, "a running task has no deadline");
    }

    #[test]
    fn a_paste_the_cli_reads_images_from_is_entered_only_after_the_longer_delay() {
        let (mut d, t) = ready(7);
        let text = "Fix this\n/Users/example/Desktop/shot.png";
        assert_eq!(
            d.taken(7, Some(text.into()), true, t),
            [Action::Paste(text.into())]
        );
        assert_eq!(d.deadline(), Some(t + IMAGE_ENTER_DELAY));
        assert!(
            d.tick(t + IMAGE_ENTER_DELAY - Duration::from_millis(1))
                .is_empty()
        );
        assert_eq!(d.tick(t + IMAGE_ENTER_DELAY), [Action::Enter]);
        assert_eq!(d.deadline(), Some(t + IMAGE_ENTER_DELAY + ACK_WAIT));
    }

    #[test]
    fn the_turn_ending_ends_the_task_and_the_next_one_waits_for_the_pane_to_settle_again() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), false, t);
        d.tick(t + ENTER_DELAY);
        d.on_signal(
            &StatusSignal::PromptSubmitted,
            ack(),
            PaneStatus::Running,
            t,
        );
        assert!(
            d.on_status(PaneStatus::WaitingPermission, t).is_empty(),
            "waiting for the user keeps it running"
        );
        d.on_status(PaneStatus::Running, t);
        let end = at(t, 30_000);
        assert_eq!(
            d.on_signal(&StatusSignal::TurnComplete, none(), PaneStatus::Idle, end),
            [report(7, TaskState::Ended, None)]
        );
        assert!(d.tick(at(end, 999)).is_empty());
        assert_eq!(d.tick(at(end, 1000)), [Action::Check]);
    }

    #[test]
    fn the_quiet_timeout_ends_a_claude_task_too() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), false, t);
        d.tick(t + ENTER_DELAY);
        d.on_signal(
            &StatusSignal::PromptSubmitted,
            ack(),
            PaneStatus::Running,
            t,
        );
        assert_eq!(
            d.on_signal(&StatusSignal::QuietTimeout, none(), PaneStatus::Idle, t),
            [report(7, TaskState::Ended, None)]
        );
    }

    #[test]
    fn no_acknowledgement_or_no_codex_turn_fails_the_task() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), false, t);
        d.tick(t + ENTER_DELAY);
        let late = t + ENTER_DELAY + ACK_WAIT;
        assert_eq!(
            d.tick(late),
            [report(7, TaskState::Failed, Some(NOT_SUBMITTED))]
        );
        let (mut d, t) = ready(8);
        d.taken(8, Some("x".into()), false, t);
        d.tick(t + ENTER_DELAY);
        assert_eq!(
            d.on_signal(&StatusSignal::NoTurnStarted, none(), PaneStatus::Idle, t),
            [report(8, TaskState::Failed, Some(NOT_SUBMITTED))]
        );
    }

    #[test]
    fn an_exit_fails_a_typed_task() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), false, t);
        d.tick(t + ENTER_DELAY);
        d.on_signal(
            &StatusSignal::PromptSubmitted,
            ack(),
            PaneStatus::Running,
            t,
        );
        assert_eq!(
            d.on_status(PaneStatus::Exited, t),
            [report(7, TaskState::Failed, Some(PROCESS_EXITED))]
        );
    }

    #[test]
    fn a_submit_by_the_user_between_paste_and_enter_is_the_acknowledgement() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), false, t);
        assert_eq!(
            d.on_signal(
                &StatusSignal::PromptSubmitted,
                ack(),
                PaneStatus::Running,
                t
            ),
            [report(7, TaskState::Running, None)]
        );
        assert!(d.tick(t + ENTER_DELAY).is_empty(), "no second Enter");
    }

    #[test]
    fn a_paste_the_engine_refuses_fails_the_task_without_an_enter() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("two\nlines".into()), false, t);
        assert_eq!(
            d.paste_refused(t),
            [report(7, TaskState::Failed, Some(PASTE_REFUSED))]
        );
        assert!(
            d.tick(t + ENTER_DELAY).is_empty(),
            "no Enter after a refused paste"
        );
        assert!(d.paste_refused(t).is_empty(), "nothing in flight");
    }

    #[test]
    fn a_take_the_registry_refuses_looks_at_the_queue_again() {
        let (mut d, t) = ready(7);
        assert_eq!(d.taken(7, None, false, t), [Action::Check]);
        assert!(d.offer(None, t).is_empty());
    }

    #[test]
    fn unsent_typing_blocks_the_queue_until_ctrl_c_ctrl_u_or_the_cli_taking_a_prompt_clears_it() {
        let t0 = Instant::now();
        let mut d = live(t0);
        d.on_user_input(b"half a thought", t0);
        d.nudge(t0);
        let t = at(t0, 1000);
        assert_eq!(d.tick(t), [Action::Check]);
        assert_eq!(
            d.offer(Some(7), t),
            [Action::Block(Some(BlockReason::Typing))]
        );
        assert!(d.tick(at(t, 5000)).is_empty(), "blocked: no polling");
        d.on_user_input(b"\r", t);
        d.on_signal(
            &StatusSignal::PromptSubmitted,
            ack(),
            PaneStatus::Running,
            at(t, 100),
        );
        d.on_signal(
            &StatusSignal::TurnComplete,
            none(),
            PaneStatus::Idle,
            at(t, 2000),
        );
        assert!(d.tick(at(t, 2999)).is_empty());
        assert_eq!(d.tick(at(t, 3000)), [Action::Check]);
        assert_eq!(
            d.offer(Some(7), at(t, 3000)),
            [Action::Block(None), Action::Take(7)]
        );
        for clear in [&b"\x03"[..], b"\x15", b"\x1b[99;5u", b"\x1b[117;5u"] {
            let mut d = live(t0);
            d.on_user_input(b"x", t0);
            d.on_user_input(clear, t0);
            d.nudge(t0);
            d.tick(t);
            assert_eq!(
                d.offer(Some(7), t),
                [Action::Take(7)],
                "{clear:?} clears the input"
            );
        }
        let mut d = live(t0);
        d.on_user_input(b"\x1b", t0);
        d.nudge(t0);
        d.tick(t);
        assert_eq!(
            d.offer(Some(7), t),
            [Action::Block(Some(BlockReason::Typing))],
            "Esc can bring an interrupted prompt back into the input"
        );
    }

    #[test]
    fn the_users_own_keys_restart_the_settle_time() {
        let t0 = Instant::now();
        let mut d = live(t0);
        d.nudge(t0);
        d.on_user_input(b"\x03", at(t0, 900));
        assert!(d.tick(at(t0, 1000)).is_empty());
        assert_eq!(d.tick(at(t0, 1900)), [Action::Check]);
        let mut d = live(t0);
        d.nudge(t0);
        d.on_user_paste(at(t0, 500));
        d.tick(at(t0, 1500));
        assert_eq!(
            d.offer(Some(7), at(t0, 1500)),
            [Action::Block(Some(BlockReason::Typing))],
            "a paste of the user's own fills the input too"
        );
    }

    #[test]
    fn the_cli_taking_a_prompt_or_starting_a_session_clears_the_input() {
        let t0 = Instant::now();
        for (signal, sense) in [
            (StatusSignal::PromptSubmitted, ack()),
            (StatusSignal::TurnStarted, ack()),
            (StatusSignal::Ready, shows()),
        ] {
            let mut d = live(t0);
            d.on_user_input(b"x", t0);
            d.on_signal(&signal, sense, PaneStatus::Idle, t0);
            d.nudge(t0);
            d.tick(at(t0, 1000));
            assert_eq!(
                d.offer(Some(7), at(t0, 1000)),
                [Action::Take(7)],
                "{signal:?}"
            );
        }
    }

    #[test]
    fn send_now_types_the_task_over_unsent_typing() {
        let t0 = Instant::now();
        let mut d = live(t0);
        d.on_user_input(b"x", t0);
        d.nudge(t0);
        d.tick(at(t0, 1000));
        d.offer(Some(7), at(t0, 1000));
        assert_eq!(
            d.send_now(9, at(t0, 2000)),
            [Action::Block(None), Action::Take(9)],
            "the user's choice, even a task behind the head"
        );
    }

    #[test]
    fn a_users_enter_never_clears_their_typing() {
        let t0 = Instant::now();
        let t = at(t0, 5000);
        for draft in [
            &[&b"first line\\"[..], b"\r"][..],
            &[b"first line", b"\x1b\r"],
            &[b"/model", b"\r"],
            &[b"/model", b"\x1b[13u"],
        ] {
            let mut d = live(t0);
            for keys in draft {
                d.on_user_input(keys, t0);
            }
            d.on_signal(
                &StatusSignal::PromptSubmitted,
                none(),
                PaneStatus::Running,
                t0,
            );
            d.on_signal(&StatusSignal::NoTurnStarted, none(), PaneStatus::Idle, t0);
            d.nudge(t0);
            d.tick(t);
            assert_eq!(
                d.offer(Some(7), t),
                [Action::Block(Some(BlockReason::Typing))],
                "{draft:?}: a newline in a draft or a picker a local command opened"
            );
        }
        let mut d = live(t0);
        d.on_user_input(b"\r", t0);
        d.nudge(t0);
        d.tick(t);
        assert_eq!(
            d.offer(Some(7), t),
            [Action::Take(7)],
            "Enter alone puts nothing into an empty input"
        );
    }

    #[test]
    fn keys_that_answer_a_dialog_are_not_typing() {
        let t0 = Instant::now();
        let mut d = live(t0);
        for status in [PaneStatus::WaitingPermission, PaneStatus::WaitingInput] {
            d.on_status(status, t0);
            d.on_user_input(b"1", t0);
            d.on_user_input(b"\r", t0);
        }
        d.on_status(PaneStatus::Running, t0);
        d.on_status(PaneStatus::Idle, t0);
        d.nudge(t0);
        let t = at(t0, 5000);
        d.tick(t);
        assert_eq!(d.offer(Some(7), t), [Action::Take(7)]);
    }

    #[test]
    fn the_machine_looks_at_the_queue_when_it_starts_whenever_the_pane_turns_idle_and_when_typing_clears()
     {
        let t0 = Instant::now();
        let mut d = Dispatch::new(PaneStatus::Idle, t0);
        assert_eq!(
            d.tick(at(t0, 1000)),
            [Action::Check],
            "a new process, or one resumed"
        );
        d.offer(None, at(t0, 1000));
        d.on_status(PaneStatus::Running, at(t0, 2000));
        d.on_status(PaneStatus::Idle, at(t0, 3000));
        assert_eq!(
            d.tick(at(t0, 4000)),
            [Action::Check],
            "a pool task may wait for this pane"
        );
        d.offer(None, at(t0, 4000));
        d.on_user_input(b"x", at(t0, 5000));
        d.on_user_input(b"\x15", at(t0, 5000));
        assert_eq!(d.tick(at(t0, 6000)), [Action::Check]);
    }

    #[test]
    fn no_enter_once_the_pane_left_idle_or_the_user_typed_after_the_paste() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), false, t);
        d.on_status(PaneStatus::WaitingPermission, t);
        assert!(
            d.tick(t + ENTER_DELAY).is_empty(),
            "an Enter would answer the dialog"
        );
        assert_eq!(
            d.tick(t + ENTER_DELAY + ACK_WAIT),
            [report(7, TaskState::Failed, Some(NOT_SUBMITTED))]
        );
        let (mut d, t) = ready(8);
        d.taken(8, Some("x".into()), false, t);
        d.on_user_input(b"y", t);
        assert!(
            d.tick(t + ENTER_DELAY).is_empty(),
            "the user's keys would be submitted with the task"
        );
    }

    #[test]
    fn a_signal_at_the_settle_deadline_is_judged_by_the_status_it_leads_to() {
        let t0 = Instant::now();
        let mut d = live(t0);
        d.nudge(t0);
        let signal = StatusSignal::PermissionRequested {
            call: None,
            detail: None,
        };
        assert!(
            d.on_signal(&signal, none(), PaneStatus::WaitingPermission, at(t0, 1000))
                .is_empty()
        );
    }

    #[test]
    fn a_task_that_was_not_submitted_leaves_its_text_in_the_input() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), false, t);
        d.tick(t + ENTER_DELAY);
        d.tick(t + ENTER_DELAY + ACK_WAIT);
        assert!(d.typed(), "the paste is still in the CLI's input");
        let (mut d, t) = ready(8);
        d.taken(8, Some("x".into()), false, t);
        d.tick(t + ENTER_DELAY);
        d.on_signal(&StatusSignal::NoTurnStarted, none(), PaneStatus::Idle, t);
        assert!(d.typed());
    }
}
