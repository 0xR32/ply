//! Whether an agent process is at rest: it can be stopped and its session resumed without losing a turn, a dialog or
//! the user's unsent typing, which a reload onto the CLI's update or a new sign-in waits for.
//!
//! A pane is at rest once it has been `idle` for [`SETTLE`], [`SETTLE`] has passed since the user's last key or paste
//! ([`SETTLE_AFTER_ENTER`] after an Enter, so their own prompt goes first), and its input holds nothing the user typed
//! since their last prompt: any key, raw input or paste of theirs marks the input as typed, except a bare Enter and keys
//! that answer the CLI's dialog or question. Only Ctrl+C, Ctrl+U, the CLI acknowledging a prompt
//! (`Adapter::acknowledges_prompt`) or starting a new session clears it; the user's Enter does not, since it may add a
//! line to a draft or open a local command's picker. Plain data, driven by one pane task.

use std::time::{Duration, Instant};

use ply_agents::StatusSignal;
use ply_proto::pane::PaneStatus;

/// How long a pane must stay `idle`, and quiet after the user's last key, before it is at rest.
pub const SETTLE: Duration = Duration::from_secs(1);

/// How long a pane must stay quiet after the user pressed Enter in it.
pub const SETTLE_AFTER_ENTER: Duration = Duration::from_secs(3);

/// One agent process's rest; see the module docs.
#[derive(Debug)]
pub struct Rest {
    status: PaneStatus,
    idle_since: Option<Instant>,
    quiet_until: Option<Instant>,
    typed: bool,
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

impl Rest {
    /// The rest of a new process in `status`.
    pub fn new(status: PaneStatus, now: Instant) -> Self {
        Self {
            status,
            idle_since: (status == PaneStatus::Idle).then_some(now),
            quiet_until: None,
            typed: false,
        }
    }

    /// Whether the process can stop without losing anything; see the module docs.
    pub fn at_rest(&self, now: Instant) -> bool {
        self.rest_at().is_some_and(|at| at <= now)
    }

    /// When [`Rest::at_rest`] turns true if nothing else happens; `None` while the pane is not idle or the user's typing is in its input.
    pub fn rest_at(&self) -> Option<Instant> {
        if self.typed {
            return None;
        }
        let settled = self.idle_since? + SETTLE;
        Some(self.quiet_until.map_or(settled, |q| q.max(settled)))
    }

    /// A status signal of the process, whether the adapter reads it as the CLI taking a prompt, and the pane's status after it.
    pub fn on_signal(
        &mut self,
        signal: &StatusSignal,
        acknowledges: bool,
        status: PaneStatus,
        now: Instant,
    ) {
        self.status = status;
        self.idle_since = (status == PaneStatus::Idle).then(|| self.idle_since.unwrap_or(now));
        if acknowledges || matches!(signal, StatusSignal::Ready) {
            self.typed = false;
        }
    }

    /// Bytes the user's own key or raw input wrote to the pty.
    pub fn on_user_input(&mut self, bytes: &[u8], now: Instant) {
        if clears_input(bytes) {
            self.typed = false;
        } else if !self.answering() && !bare_enter(bytes) {
            self.typed = true;
        }
        let quiet = if crate::osc::is_enter(bytes) {
            SETTLE_AFTER_ENTER
        } else {
            SETTLE
        };
        self.quiet(now + quiet);
    }

    /// The user pasted into the pane.
    pub fn on_user_paste(&mut self, now: Instant) {
        if !self.answering() {
            self.typed = true;
        }
        self.quiet(now + SETTLE);
    }

    /// Keys of the user's that go to the CLI's dialog or question, not into its prompt's input.
    fn answering(&self) -> bool {
        matches!(
            self.status,
            PaneStatus::WaitingPermission | PaneStatus::WaitingInput
        )
    }

    fn quiet(&mut self, until: Instant) {
        self.quiet_until = Some(self.quiet_until.map_or(until, |q| q.max(until)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    fn signal(
        r: &mut Rest,
        s: &StatusSignal,
        acknowledges: bool,
        status: PaneStatus,
        now: Instant,
    ) {
        r.on_signal(s, acknowledges, status, now);
    }

    /// A process that has shown its prompt and turned idle at `t0`.
    fn idle(t0: Instant) -> Rest {
        let mut r = Rest::new(PaneStatus::Starting, t0);
        signal(&mut r, &StatusSignal::Ready, false, PaneStatus::Idle, t0);
        r
    }

    #[test]
    fn a_pane_is_at_rest_once_settled_with_nothing_typed() {
        let t0 = Instant::now();
        let mut r = idle(t0);
        assert!(!r.at_rest(at(t0, 999)), "not yet settled");
        assert!(r.at_rest(at(t0, 1000)));
        r.on_user_input(b"draft", at(t0, 2000));
        assert!(!r.at_rest(at(t0, 9000)), "the user's unsent typing");
        assert_eq!(r.rest_at(), None);
        r.on_user_input(b"\x15", at(t0, 9000));
        assert!(!r.at_rest(at(t0, 9999)), "quiet after the user's last key");
        assert!(r.at_rest(at(t0, 10_000)), "Ctrl+U cleared the input");
        signal(
            &mut r,
            &StatusSignal::PromptSubmitted,
            true,
            PaneStatus::Running,
            at(t0, 11_000),
        );
        assert!(!r.at_rest(at(t0, 20_000)), "a turn runs");
        signal(
            &mut r,
            &StatusSignal::TurnComplete,
            false,
            PaneStatus::Idle,
            at(t0, 30_000),
        );
        assert_eq!(r.rest_at(), Some(at(t0, 31_000)), "settled anew");
    }

    #[test]
    fn ctrl_c_and_ctrl_u_clear_the_input_and_esc_does_not() {
        let t0 = Instant::now();
        let t = at(t0, 1000);
        for clear in [&b"\x03"[..], b"\x15", b"\x1b[99;5u", b"\x1b[117;5u"] {
            let mut r = idle(t0);
            r.on_user_input(b"x", t0);
            r.on_user_input(clear, t0);
            assert!(r.at_rest(t), "{clear:?} clears the input");
        }
        let mut r = idle(t0);
        r.on_user_input(b"\x1b", t0);
        assert!(
            !r.at_rest(at(t0, 5000)),
            "Esc can bring an interrupted prompt back into the input"
        );
    }

    #[test]
    fn a_users_enter_never_clears_their_typing_and_waits_longer() {
        let t0 = Instant::now();
        let t = at(t0, 5000);
        for draft in [
            &[&b"first line\\"[..], b"\r"][..],
            &[b"first line", b"\x1b\r"],
            &[b"/model", b"\r"],
            &[b"/model", b"\x1b[13u"],
        ] {
            let mut r = idle(t0);
            for keys in draft {
                r.on_user_input(keys, t0);
            }
            signal(
                &mut r,
                &StatusSignal::PromptSubmitted,
                false,
                PaneStatus::Running,
                t0,
            );
            signal(
                &mut r,
                &StatusSignal::NoTurnStarted,
                false,
                PaneStatus::Idle,
                t0,
            );
            assert!(
                !r.at_rest(t),
                "{draft:?}: a newline in a draft or a picker a local command opened"
            );
        }
        let mut r = idle(t0);
        r.on_user_input(b"\r", t0);
        assert!(!r.at_rest(at(t0, 2999)), "the user's own prompt goes first");
        assert!(
            r.at_rest(at(t0, 3000)),
            "Enter alone puts nothing into an empty input"
        );
    }

    #[test]
    fn keys_that_answer_a_dialog_are_not_typing_but_a_paste_is() {
        let t0 = Instant::now();
        let mut r = idle(t0);
        for (s, status) in [
            (
                StatusSignal::PermissionRequested {
                    call: None,
                    detail: None,
                },
                PaneStatus::WaitingPermission,
            ),
            (
                StatusSignal::InputRequested { detail: None },
                PaneStatus::WaitingInput,
            ),
        ] {
            signal(&mut r, &s, false, status, t0);
            r.on_user_input(b"1", t0);
            r.on_user_input(b"\r", t0);
        }
        signal(
            &mut r,
            &StatusSignal::TurnComplete,
            false,
            PaneStatus::Idle,
            t0,
        );
        assert!(r.at_rest(at(t0, 5000)));
        r.on_user_paste(at(t0, 5000));
        assert!(
            !r.at_rest(at(t0, 9000)),
            "a paste of the user's own fills the input"
        );
    }

    #[test]
    fn the_cli_taking_a_prompt_or_starting_a_session_clears_the_input() {
        let t0 = Instant::now();
        for (s, acknowledges) in [
            (StatusSignal::PromptSubmitted, true),
            (StatusSignal::TurnStarted, true),
            (StatusSignal::Ready, false),
        ] {
            let mut r = idle(t0);
            r.on_user_input(b"x", t0);
            signal(&mut r, &s, acknowledges, PaneStatus::Idle, t0);
            assert!(r.at_rest(at(t0, 1000)), "{s:?}");
        }
    }
}
