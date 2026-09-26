//! When plyd types a queued task into its pane (Ruling R60), as a small clocked state machine per agent process.
//!
//! The machine owns no queue: the registry holds the tasks, and the machine asks for the queue's head when typing may
//! be possible ([`Action::Check`]), claims it ([`Action::Take`]; the registry marks it `sent` if it is still the head)
//! and follows it. It types only into a pane that has been `idle` for [`SETTLE`] and whose input holds nothing the
//! user typed since their last prompt: any key, raw input or paste of theirs marks the input as typed, and Enter,
//! Ctrl+C, Ctrl+U or the CLI reporting a prompt or a new session clears it. While typed, the head waits and the queue
//! is blocked ([`BlockReason::Typing`]) until the input clears or the user sends the task anyway (`task.send`).
//!
//! A task is written as one paste (bracketed when the CLI enabled it) and, [`ENTER_DELAY`] later, one Enter, so an
//! autocomplete pop-over the paste opens has settled. The CLI's acknowledgement makes the task `running`: Claude
//! Code's UserPromptSubmit, Codex's rollout `task_started` (the adapter names it). The turn ending (Stop, notify,
//! `task_complete`, or the quiet timeout) makes it `ended`; a Codex Enter that started no turn, no acknowledgement
//! within [`ACK_WAIT`], or the process exiting makes it `failed`. The machine never answers a dialog: a task whose
//! pane waits for the user stays `running`.

use std::time::{Duration, Instant};

use ply_agents::StatusSignal;
use ply_proto::pane::{BlockReason, PaneStatus, TaskId, TaskState};

/// How long a pane must stay `idle` before a task is typed into it.
pub const SETTLE: Duration = Duration::from_secs(1);

/// Time between a task's paste and its Enter.
pub const ENTER_DELAY: Duration = Duration::from_millis(50);

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
    typed: bool,
    blocked: bool,
    force: Option<TaskId>,
}

/// Whether bytes written for one key clear the CLI's input: Ctrl+C or Ctrl+U, in the legacy or the kitty encoding.
fn clears_input(bytes: &[u8]) -> bool {
    bytes == b"\x03"
        || bytes == b"\x15"
        || bytes.starts_with(b"\x1b[99;5u")
        || bytes.starts_with(b"\x1b[117;5u")
}

impl Dispatch {
    /// A machine for a process in `status`; it looks at the queue once the pane has settled.
    pub fn new(status: PaneStatus, now: Instant) -> Self {
        Self {
            phase: Phase::Waiting,
            status,
            idle_since: (status == PaneStatus::Idle).then_some(now),
            quiet_until: None,
            check: false,
            typed: false,
            blocked: false,
            force: None,
        }
    }

    /// The pane's queue changed: look at it again once the pane is settled.
    pub fn nudge(&mut self, now: Instant) -> Vec<Action> {
        self.check = true;
        self.poll(now)
    }

    /// `task.send`: type `task` at the pane's next settled moment, even over the user's unsent typing.
    pub fn send_now(&mut self, task: TaskId, now: Instant) -> Vec<Action> {
        self.force = Some(task);
        self.poll(now)
    }

    /// The pane's status changed.
    pub fn on_status(&mut self, status: PaneStatus, now: Instant) -> Vec<Action> {
        let was = self.status;
        self.status = status;
        if status != PaneStatus::Idle {
            self.idle_since = None;
        } else if was != PaneStatus::Idle || self.idle_since.is_none() {
            self.idle_since = Some(now);
        }
        if matches!(status, PaneStatus::Exited | PaneStatus::Lost)
            && let Some(task) = self.in_flight()
        {
            self.phase = Phase::Waiting;
            return vec![failed(task, PROCESS_EXITED)];
        }
        self.poll(now)
    }

    /// A status signal the machine applied; `acknowledges` when the adapter counts it as the CLI taking a prompt.
    pub fn on_signal(
        &mut self,
        signal: &StatusSignal,
        acknowledges: bool,
        now: Instant,
    ) -> Vec<Action> {
        if matches!(
            signal,
            StatusSignal::PromptSubmitted | StatusSignal::TurnStarted | StatusSignal::Ready
        ) {
            self.clear_typed();
        }
        let mut out = Vec::new();
        match (self.phase, signal) {
            (Phase::Pasted { task, .. } | Phase::Sent { task, .. }, _) if acknowledges => {
                self.phase = Phase::Running { task };
                out.push(Action::Report {
                    task,
                    state: TaskState::Running,
                    detail: None,
                });
            }
            (Phase::Sent { task, .. }, StatusSignal::NoTurnStarted) => {
                self.finish(now);
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
        if crate::osc::is_enter(bytes) {
            self.clear_typed();
            self.quiet(now + SETTLE_AFTER_ENTER);
        } else {
            if clears_input(bytes) {
                self.clear_typed();
            } else {
                self.typed = true;
            }
            self.quiet(now + SETTLE);
        }
        self.poll(now)
    }

    /// The user pasted into the pane.
    pub fn on_user_paste(&mut self, now: Instant) -> Vec<Action> {
        self.typed = true;
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
            None => self.unblock(),
            Some(_) if self.typed => {
                if self.blocked {
                    Vec::new()
                } else {
                    self.blocked = true;
                    vec![Action::Block(Some(BlockReason::Typing))]
                }
            }
            Some(task) => {
                let mut out = self.unblock();
                self.phase = Phase::Taking { task };
                out.push(Action::Take(task));
                out
            }
        }
    }

    /// The registry's answer to [`Action::Take`]: the task's text, or `None` when it is no longer the head.
    pub fn taken(&mut self, task: TaskId, text: Option<String>, now: Instant) -> Vec<Action> {
        if self.phase != (Phase::Taking { task }) {
            return Vec::new();
        }
        match text {
            Some(text) => {
                self.phase = Phase::Pasted {
                    task,
                    enter_at: now + ENTER_DELAY,
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
            Phase::Waiting if self.check || self.force.is_some() => self.settled_at(),
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
                if let Some(task) = self.force.take() {
                    self.check = false;
                    let mut out = self.unblock();
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
                vec![Action::Enter]
            }
            Phase::Sent { task, deadline } if deadline <= now => {
                self.finish(now);
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

    /// A typed task is over: look at the queue again once the pane has settled anew.
    fn finish(&mut self, now: Instant) {
        self.phase = Phase::Waiting;
        self.check = true;
        self.quiet(now + SETTLE);
    }

    fn quiet(&mut self, until: Instant) {
        self.quiet_until = Some(self.quiet_until.map_or(until, |q| q.max(until)));
    }

    fn clear_typed(&mut self) {
        if std::mem::take(&mut self.typed) && self.blocked {
            self.check = true;
        }
    }

    fn unblock(&mut self) -> Vec<Action> {
        if std::mem::take(&mut self.blocked) {
            vec![Action::Block(None)]
        } else {
            Vec::new()
        }
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

    /// A machine for an idle pane that has settled and been nudged, answered with head `task`.
    fn ready(task: TaskId) -> (Dispatch, Instant) {
        let t0 = Instant::now();
        let mut d = Dispatch::new(PaneStatus::Starting, t0);
        d.on_status(PaneStatus::Idle, t0);
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
        assert!(d.on_status(PaneStatus::Idle, at(t0, 100)).is_empty());
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
    fn a_running_pane_is_never_typed_into() {
        let t0 = Instant::now();
        let mut d = Dispatch::new(PaneStatus::Idle, t0);
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
            d.taken(7, Some("/review-pr #1".into()), t),
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
            d.on_signal(&StatusSignal::PromptSubmitted, false, t)
                .is_empty(),
            "Codex's Enter guess is no acknowledgement"
        );
        assert_eq!(
            d.on_signal(&StatusSignal::TurnStarted, true, t),
            [report(7, TaskState::Running, None)]
        );
        assert_eq!(d.deadline(), None, "a running task has no deadline");
    }

    #[test]
    fn the_turn_ending_ends_the_task_and_the_next_one_waits_for_the_pane_to_settle_again() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), t);
        d.tick(t + ENTER_DELAY);
        d.on_signal(&StatusSignal::PromptSubmitted, true, t);
        d.on_status(PaneStatus::Running, t);
        assert!(
            d.on_status(PaneStatus::WaitingPermission, t).is_empty(),
            "waiting for the user keeps it running"
        );
        d.on_status(PaneStatus::Running, t);
        let end = at(t, 30_000);
        assert_eq!(
            d.on_signal(&StatusSignal::TurnComplete, false, end),
            [report(7, TaskState::Ended, None)]
        );
        d.on_status(PaneStatus::Idle, end);
        assert!(d.tick(at(end, 999)).is_empty());
        assert_eq!(d.tick(at(end, 1000)), [Action::Check]);
    }

    #[test]
    fn the_quiet_timeout_ends_a_claude_task_too() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), t);
        d.tick(t + ENTER_DELAY);
        d.on_signal(&StatusSignal::PromptSubmitted, true, t);
        assert_eq!(
            d.on_signal(&StatusSignal::QuietTimeout, false, t),
            [report(7, TaskState::Ended, None)]
        );
    }

    #[test]
    fn no_acknowledgement_or_no_codex_turn_fails_the_task() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), t);
        d.tick(t + ENTER_DELAY);
        let late = t + ENTER_DELAY + ACK_WAIT;
        assert_eq!(
            d.tick(late),
            [report(7, TaskState::Failed, Some(NOT_SUBMITTED))]
        );
        let (mut d, t) = ready(8);
        d.taken(8, Some("x".into()), t);
        d.tick(t + ENTER_DELAY);
        assert_eq!(
            d.on_signal(&StatusSignal::NoTurnStarted, false, t),
            [report(8, TaskState::Failed, Some(NOT_SUBMITTED))]
        );
    }

    #[test]
    fn an_exit_fails_a_typed_task() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), t);
        d.tick(t + ENTER_DELAY);
        d.on_signal(&StatusSignal::PromptSubmitted, true, t);
        assert_eq!(
            d.on_status(PaneStatus::Exited, t),
            [report(7, TaskState::Failed, Some(PROCESS_EXITED))]
        );
    }

    #[test]
    fn a_submit_by_the_user_between_paste_and_enter_is_the_acknowledgement() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("x".into()), t);
        assert_eq!(
            d.on_signal(&StatusSignal::PromptSubmitted, true, t),
            [report(7, TaskState::Running, None)]
        );
        assert!(d.tick(t + ENTER_DELAY).is_empty(), "no second Enter");
    }

    #[test]
    fn a_paste_the_engine_refuses_fails_the_task_without_an_enter() {
        let (mut d, t) = ready(7);
        d.taken(7, Some("two\nlines".into()), t);
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
        assert_eq!(d.taken(7, None, t), [Action::Check]);
        assert!(d.offer(None, t).is_empty());
    }

    #[test]
    fn unsent_typing_blocks_the_queue_until_enter_or_ctrl_c_clears_it() {
        let t0 = Instant::now();
        let mut d = Dispatch::new(PaneStatus::Idle, t0);
        d.on_status(PaneStatus::Idle, t0);
        d.on_user_input(b"half a thought", t0);
        d.nudge(t0);
        let t = at(t0, 1000);
        assert_eq!(d.tick(t), [Action::Check]);
        assert_eq!(
            d.offer(Some(7), t),
            [Action::Block(Some(BlockReason::Typing))]
        );
        assert!(d.tick(at(t, 5000)).is_empty(), "blocked: no polling");
        assert!(
            d.on_user_input(b"\r", t).is_empty(),
            "Enter submits the user's prompt, which the CLI takes first"
        );
        assert!(d.tick(at(t, 2999)).is_empty());
        assert_eq!(d.tick(at(t, 3000)), [Action::Check]);
        assert_eq!(
            d.offer(Some(7), at(t, 3000)),
            [Action::Block(None), Action::Take(7)]
        );
        for clear in [
            &b"\x03"[..],
            b"\x15",
            b"\x1b[99;5u",
            b"\x1b[117;5u",
            b"\x1b[13u",
        ] {
            let mut d = Dispatch::new(PaneStatus::Idle, t0);
            d.on_status(PaneStatus::Idle, t0);
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
        let mut d = Dispatch::new(PaneStatus::Idle, t0);
        d.on_status(PaneStatus::Idle, t0);
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
        let mut d = Dispatch::new(PaneStatus::Idle, t0);
        d.on_status(PaneStatus::Idle, t0);
        d.nudge(t0);
        d.on_user_input(b"\x03", at(t0, 900));
        assert!(d.tick(at(t0, 1000)).is_empty());
        assert_eq!(d.tick(at(t0, 1900)), [Action::Check]);
        let mut d = Dispatch::new(PaneStatus::Idle, t0);
        d.on_status(PaneStatus::Idle, t0);
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
        for signal in [
            StatusSignal::PromptSubmitted,
            StatusSignal::TurnStarted,
            StatusSignal::Ready,
        ] {
            let mut d = Dispatch::new(PaneStatus::Idle, t0);
            d.on_status(PaneStatus::Idle, t0);
            d.on_user_input(b"x", t0);
            d.on_signal(&signal, false, t0);
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
        let mut d = Dispatch::new(PaneStatus::Idle, t0);
        d.on_status(PaneStatus::Idle, t0);
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
}
