//! The status state machine of spec 6.3 (with Rulings R17 and R27), and the `pane.progress` rate limit (spec 6.4).
//!
//! plyd owns the table; the adapters in `ply-agents` only name the signal column ([`StatusSignal`]). One
//! [`StatusMachine`] runs per agent process: it starts in `starting` at spawn, and every signal either moves it along a
//! row of the table or is ignored and counted ("transitions not in the table are ignored and counted"). The rows:
//!
//! | From | Signal | To |
//! |---|---|---|
//! | every live state | Ready (Claude SessionStart but a compaction's · Codex first output byte) | `idle` |
//! | `idle`, `waiting_input` | PromptSubmitted (Claude UserPromptSubmit · Codex Enter typed) | `running` |
//! | `starting`, `idle`, `running` | TurnStarted (Codex rollout `task_started`, R48) | `running` |
//! | `running` (Codex) | NoTurnStarted (plyd: an Enter started no turn within 3 s, R48) | `idle` |
//! | every live state but `waiting_permission` | ToolUse (Claude PreToolUse · PostToolUse) | `running` |
//! | `running` (Codex also `idle`, R48) | PermissionRequested (Claude PermissionRequest or `permission_prompt` Notification · Codex OSC 9 approval) | `waiting_permission` |
//! | `running`, `idle` | InputRequested (Codex OSC 9 question or plan · Claude Notification but `permission_prompt` and `idle_prompt`, R46) | `waiting_input` |
//! | `waiting_permission`, `waiting_input` | KeyTyped (any key typed in the pane, R17) | `running` |
//! | `running` (Claude) | QuietTimeout (silent pty, no hook for 5 s, R17) | `idle` |
//! | `waiting_permission` | CallSettled for the pending call (PostToolUse, PostToolUseFailure, PermissionDenied) | `running` |
//! | `running`, `waiting_permission` without a pending call | TurnComplete (Claude Stop · StopFailure · Codex notify · OSC 9 turn complete, R27 · rollout `task_complete` or `turn_aborted`) | `idle` |
//! | any | the process exits (pty end-of-file) | `exited(code)` |
//!
//! SessionEnd changes nothing ([`Step::SessionEnded`], Ruling R47): Claude fires it for `/clear` and an in-session
//! `/resume` while the process keeps running, and the SessionStart that follows makes the pane `idle`. Only the
//! process's exit ends the machine ([`StatusMachine::exit`]). `lost` is set by the registry when plyd starts, never by
//! a signal. Shell panes have no machine.

use std::time::{Duration, Instant};

use ply_agents::{StatusSignal, ToolCall};
use ply_proto::pane::{AgentCli, PaneStatus, Progress};

/// What one signal did to a [`StatusMachine`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A row of the table applied: the pane is now in `status` with `detail` (possibly the state it was already in).
    To {
        /// The new state.
        status: PaneStatus,
        /// One line for the header; set only for the waiting states.
        detail: Option<String>,
    },
    /// Claude ended a session but its process runs on (`/clear`, `/resume`, or on its way out); nothing changed.
    SessionEnded,
    /// No row matches the current state and signal; nothing changed and the ignored count grew by one.
    Ignored,
}

/// One agent process's spec 6.3 machine; see the module docs for the table. Plain data, driven by one pane task.
#[derive(Debug, Clone)]
pub struct StatusMachine {
    cli: AgentCli,
    status: PaneStatus,
    detail: Option<String>,
    pending: Option<ToolCall>,
    ended: bool,
    ignored: u64,
}

impl StatusMachine {
    /// A machine for a freshly spawned process of `cli`, in `starting`.
    pub fn new(cli: AgentCli) -> Self {
        Self {
            cli,
            status: PaneStatus::Starting,
            detail: None,
            pending: None,
            ended: false,
            ignored: 0,
        }
    }

    /// The current state.
    pub fn status(&self) -> PaneStatus {
        self.status
    }

    /// The detail line of the current state (the tool or question a waiting state is about).
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Signals ignored so far because no row of the table matched.
    pub fn ignored(&self) -> u64 {
        self.ignored
    }

    /// Whether the process exited; an ended machine ignores every signal.
    pub fn has_ended(&self) -> bool {
        self.ended
    }

    /// Applies one signal (see the table in the module docs).
    pub fn apply(&mut self, signal: &StatusSignal) -> Step {
        use PaneStatus::{Idle, Running, Starting, WaitingInput, WaitingPermission};
        if self.ended {
            return self.ignore();
        }
        let s = self.status;
        match signal {
            StatusSignal::Ready => self.to(Idle, None),
            StatusSignal::PromptSubmitted if matches!(s, Idle | WaitingInput) => {
                self.to(Running, None)
            }
            StatusSignal::ToolUse(_) if matches!(s, Starting | Idle | Running | WaitingInput) => {
                self.to(Running, None)
            }
            StatusSignal::PermissionRequested { call, detail }
                if s == Running || (s == Idle && self.cli == AgentCli::Codex) =>
            {
                let step = self.to(WaitingPermission, detail.clone());
                self.pending.clone_from(call);
                step
            }
            StatusSignal::InputRequested { detail } if matches!(s, Running | Idle) => {
                self.to(WaitingInput, detail.clone())
            }
            StatusSignal::TurnStarted if matches!(s, Starting | Idle | Running) => {
                self.to(Running, None)
            }
            StatusSignal::NoTurnStarted if s == Running => self.to(Idle, None),
            StatusSignal::KeyTyped if matches!(s, WaitingPermission | WaitingInput) => {
                self.to(Running, None)
            }
            StatusSignal::QuietTimeout if s == Running && self.cli == AgentCli::Claude => {
                self.to(Idle, None)
            }
            StatusSignal::CallSettled(call)
                if s == WaitingPermission
                    && self.pending.as_ref().is_some_and(|p| p.same_call(call)) =>
            {
                self.to(Running, None)
            }
            // A late `permission_prompt` Notification re-enters waiting_permission with no call; the turn's end still counts.
            StatusSignal::TurnComplete
                if s == Running || (s == WaitingPermission && self.pending.is_none()) =>
            {
                self.to(Idle, None)
            }
            StatusSignal::SessionEnded { .. } => Step::SessionEnded,
            _ => self.ignore(),
        }
    }

    /// The process ended: the machine is `exited` from now on and ignores every later signal.
    pub fn exit(&mut self) {
        self.ended = true;
        self.status = PaneStatus::Exited;
        self.detail = None;
        self.pending = None;
    }

    fn to(&mut self, status: PaneStatus, detail: Option<String>) -> Step {
        self.status = status;
        self.detail.clone_from(&detail);
        if status != PaneStatus::WaitingPermission {
            self.pending = None;
        }
        Step::To { status, detail }
    }

    fn ignore(&mut self) -> Step {
        self.ignored += 1;
        Step::Ignored
    }
}

/// Shortest interval between two `pane.progress` events of one pane: at most 4 a second (spec 6.4).
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// `pane.progress` rate limit, clocked by the caller: a change after a quiet interval goes out at once, a burst's last value when the interval ends.
#[derive(Debug, Clone, Default)]
pub struct ProgressGate {
    last_sent: Option<Instant>,
    pending: Option<Option<Progress>>,
}

impl ProgressGate {
    /// Offers a new value at `now`; returns it when it may be sent now, else keeps it for [`ProgressGate::due`].
    pub fn offer(&mut self, progress: Option<Progress>, now: Instant) -> Option<Option<Progress>> {
        match self.last_sent {
            Some(at) if now.duration_since(at) < PROGRESS_INTERVAL => {
                self.pending = Some(progress);
                None
            }
            _ => {
                self.pending = None;
                self.last_sent = Some(now);
                Some(progress)
            }
        }
    }

    /// When the kept value may go out; `None` while nothing is kept.
    pub fn due(&self) -> Option<Instant> {
        self.pending.as_ref()?;
        self.last_sent.map(|at| at + PROGRESS_INTERVAL)
    }

    /// The kept value once its time has come at `now`, marking it sent.
    pub fn take_due(&mut self, now: Instant) -> Option<Option<Progress>> {
        if self.due().is_some_and(|at| at <= now) {
            self.last_sent = Some(now);
            return self.pending.take();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    use PaneStatus::{Exited, Idle, Running, Starting, WaitingInput, WaitingPermission};

    const LIVE: [PaneStatus; 5] = [Starting, Idle, Running, WaitingPermission, WaitingInput];

    fn call(name: &str) -> ToolCall {
        ToolCall::new(
            None,
            name,
            Some(&json!({"file_path": "/Users/example/a.txt"})),
        )
    }

    fn permission() -> StatusSignal {
        StatusSignal::PermissionRequested {
            call: Some(call("Write")),
            detail: Some("Write".into()),
        }
    }

    fn input() -> StatusSignal {
        StatusSignal::InputRequested {
            detail: Some("Claude is waiting for your input".into()),
        }
    }

    /// A machine of `cli` driven into `status` along rows of the table.
    fn at(cli: AgentCli, status: PaneStatus) -> StatusMachine {
        let mut m = StatusMachine::new(cli);
        let path: &[StatusSignal] = match status {
            Starting => &[],
            Idle => &[StatusSignal::Ready],
            Running => &[StatusSignal::Ready, StatusSignal::PromptSubmitted],
            WaitingPermission => &[
                StatusSignal::Ready,
                StatusSignal::PromptSubmitted,
                permission(),
            ],
            WaitingInput => &[StatusSignal::Ready, StatusSignal::PromptSubmitted, input()],
            _ => unreachable!("not a live state"),
        };
        for s in path {
            assert!(
                matches!(m.apply(s), Step::To { .. }),
                "{s:?} from {:?}",
                m.status()
            );
        }
        assert_eq!(m.status(), status);
        m
    }

    /// Asserts `signal` moves `cli` from every state in `from` to `to`, and is ignored from every other live state.
    fn row(cli: AgentCli, signal: &StatusSignal, from: &[PaneStatus], to: PaneStatus) {
        for s in LIVE {
            let mut m = at(cli, s);
            let before = m.ignored();
            let step = m.apply(signal);
            if from.contains(&s) {
                assert!(
                    matches!(&step, Step::To { status, .. } if *status == to),
                    "{cli:?} {signal:?} from {s:?}: {step:?}"
                );
                assert_eq!(m.status(), to);
            } else {
                assert_eq!(step, Step::Ignored, "{cli:?} {signal:?} from {s:?}");
                assert_eq!(m.status(), s);
                assert_eq!(m.ignored(), before + 1, "ignored transitions are counted");
            }
        }
    }

    const BOTH: [AgentCli; 2] = [AgentCli::Claude, AgentCli::Codex];

    #[test]
    fn spawn_starts_in_starting() {
        for cli in BOTH {
            assert_eq!(StatusMachine::new(cli).status(), Starting);
        }
    }

    #[test]
    fn ready_makes_every_live_state_idle() {
        for cli in BOTH {
            row(cli, &StatusSignal::Ready, &LIVE, Idle);
        }
    }

    #[test]
    fn a_submitted_prompt_moves_idle_and_waiting_input_to_running() {
        for cli in BOTH {
            row(
                cli,
                &StatusSignal::PromptSubmitted,
                &[Idle, WaitingInput],
                Running,
            );
        }
    }

    #[test]
    fn tool_use_runs_from_every_live_state_but_waiting_permission() {
        row(
            AgentCli::Claude,
            &StatusSignal::ToolUse(call("Read")),
            &[Starting, Idle, Running, WaitingInput],
            Running,
        );
    }

    #[test]
    fn a_permission_request_while_running_waits_for_permission() {
        row(
            AgentCli::Claude,
            &permission(),
            &[Running],
            WaitingPermission,
        );
        row(
            AgentCli::Codex,
            &permission(),
            &[Running, Idle],
            WaitingPermission,
        );
        let mut m = at(AgentCli::Codex, Running);
        let step = m.apply(&StatusSignal::PermissionRequested {
            call: None,
            detail: Some("Approval requested: rm -rf build".into()),
        });
        assert_eq!(
            step,
            Step::To {
                status: WaitingPermission,
                detail: Some("Approval requested: rm -rf build".into())
            }
        );
        assert_eq!(m.detail(), Some("Approval requested: rm -rf build"));
    }

    #[test]
    fn a_codex_question_waits_for_input_while_running_or_idle() {
        row(AgentCli::Codex, &input(), &[Running, Idle], WaitingInput);
    }

    #[test]
    fn a_rollout_turn_start_runs_the_pane_and_a_turnless_enter_goes_back_to_idle() {
        for cli in BOTH {
            row(
                cli,
                &StatusSignal::TurnStarted,
                &[Starting, Idle, Running],
                Running,
            );
            row(cli, &StatusSignal::NoTurnStarted, &[Running], Idle);
        }
    }

    #[test]
    fn a_claude_notification_waits_for_input_from_running_or_idle() {
        row(AgentCli::Claude, &input(), &[Running, Idle], WaitingInput);
    }

    #[test]
    fn any_key_typed_in_a_waiting_pane_runs_it() {
        for cli in BOTH {
            row(
                cli,
                &StatusSignal::KeyTyped,
                &[WaitingPermission, WaitingInput],
                Running,
            );
        }
    }

    #[test]
    fn a_quiet_claude_pane_goes_idle_and_codex_never_times_out() {
        row(
            AgentCli::Claude,
            &StatusSignal::QuietTimeout,
            &[Running],
            Idle,
        );
        row(AgentCli::Codex, &StatusSignal::QuietTimeout, &[], Idle);
    }

    #[test]
    fn only_the_pending_call_settling_leaves_waiting_permission() {
        let mut m = at(AgentCli::Claude, WaitingPermission);
        assert_eq!(
            m.apply(&StatusSignal::CallSettled(call("Edit"))),
            Step::Ignored
        );
        let other_input = ToolCall::new(None, "Write", Some(&json!({"file_path": "/x"})));
        assert_eq!(
            m.apply(&StatusSignal::CallSettled(other_input)),
            Step::Ignored
        );
        assert_eq!(m.status(), WaitingPermission);
        let settled = ToolCall::new(
            Some("toolu_example1".into()),
            "Write",
            Some(&json!({"file_path": "/Users/example/a.txt"})),
        );
        assert_eq!(
            m.apply(&StatusSignal::CallSettled(settled)),
            Step::To {
                status: Running,
                detail: None
            }
        );
        row(
            AgentCli::Claude,
            &StatusSignal::CallSettled(call("Write")),
            &[WaitingPermission],
            Running,
        );
        let mut codex = at(AgentCli::Codex, Running);
        codex.apply(&StatusSignal::PermissionRequested {
            call: None,
            detail: Some("Codex wants to edit a.txt".into()),
        });
        assert_eq!(
            codex.apply(&StatusSignal::CallSettled(call("Write"))),
            Step::Ignored,
            "an OSC 9 approval has no call to settle"
        );
    }

    #[test]
    fn a_turn_complete_moves_running_to_idle() {
        for cli in BOTH {
            row(cli, &StatusSignal::TurnComplete, &[Running], Idle);
        }
    }

    #[test]
    fn a_turn_ends_a_wait_for_permission_that_has_no_pending_call() {
        let late_prompt = StatusSignal::PermissionRequested {
            call: None,
            detail: Some("Claude needs your permission to use Write".into()),
        };
        let mut m = at(AgentCli::Claude, WaitingPermission);
        assert!(matches!(
            m.apply(&StatusSignal::CallSettled(call("Write"))),
            Step::To {
                status: Running,
                ..
            }
        ));
        assert!(matches!(
            m.apply(&late_prompt),
            Step::To {
                status: WaitingPermission,
                ..
            }
        ));
        assert_eq!(
            m.apply(&StatusSignal::TurnComplete),
            Step::To {
                status: Idle,
                detail: None
            },
            "Stop after a late permission_prompt leaves the pane your turn, not amber"
        );
        let mut pending = at(AgentCli::Claude, WaitingPermission);
        assert_eq!(
            pending.apply(&StatusSignal::TurnComplete),
            Step::Ignored,
            "a dialog whose call is still pending keeps waiting"
        );
    }

    #[test]
    fn session_end_changes_nothing_and_the_next_session_start_makes_the_pane_idle() {
        for s in LIVE {
            let mut m = at(AgentCli::Claude, s);
            let ignored = m.ignored();
            assert_eq!(
                m.apply(&StatusSignal::SessionEnded {
                    reason: Some("clear".into())
                }),
                Step::SessionEnded
            );
            assert!(!m.has_ended(), "only the process's exit ends the machine");
            assert_eq!((m.status(), m.ignored()), (s, ignored));
            assert!(matches!(
                m.apply(&StatusSignal::Ready),
                Step::To { status: Idle, .. }
            ));
            assert!(matches!(
                m.apply(&StatusSignal::PromptSubmitted),
                Step::To {
                    status: Running,
                    ..
                }
            ));
            assert!(matches!(
                m.apply(&permission()),
                Step::To {
                    status: WaitingPermission,
                    ..
                }
            ));
            m.exit();
            assert_eq!(m.status(), Exited);
        }
    }

    #[test]
    fn an_exited_machine_ignores_everything() {
        let mut m = at(AgentCli::Codex, Running);
        m.exit();
        assert_eq!(m.status(), Exited);
        for s in [
            StatusSignal::Ready,
            StatusSignal::TurnComplete,
            StatusSignal::KeyTyped,
            permission(),
        ] {
            assert_eq!(m.apply(&s), Step::Ignored);
        }
        assert_eq!(m.status(), Exited);
    }

    #[test]
    fn leaving_waiting_permission_forgets_the_pending_call() {
        let mut m = at(AgentCli::Claude, WaitingPermission);
        assert!(matches!(m.apply(&StatusSignal::KeyTyped), Step::To { .. }));
        assert_eq!(m.detail(), None);
        assert!(matches!(m.apply(&permission()), Step::To { .. }));
        assert!(matches!(
            m.apply(&StatusSignal::CallSettled(call("Write"))),
            Step::To {
                status: Running,
                ..
            }
        ));
    }

    #[test]
    fn progress_goes_out_at_most_four_times_a_second_and_keeps_the_last_value() {
        let t0 = Instant::now();
        let p = |done| {
            Some(Progress {
                done,
                total: 5,
                current: None,
            })
        };
        let mut gate = ProgressGate::default();
        assert_eq!(
            gate.offer(p(1), t0),
            Some(p(1)),
            "the first change is immediate"
        );
        assert_eq!(gate.offer(p(2), t0 + Duration::from_millis(50)), None);
        assert_eq!(gate.offer(p(3), t0 + Duration::from_millis(100)), None);
        assert_eq!(gate.due(), Some(t0 + PROGRESS_INTERVAL));
        assert_eq!(gate.take_due(t0 + Duration::from_millis(200)), None);
        assert_eq!(gate.take_due(t0 + PROGRESS_INTERVAL), Some(p(3)));
        assert_eq!(gate.due(), None);
        assert_eq!(
            gate.offer(None, t0 + Duration::from_millis(600)),
            Some(None),
            "after a quiet interval a change is immediate, hiding included"
        );
    }
}
