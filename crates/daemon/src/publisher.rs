//! C2 delivery rules (spec 4.2, R-R18, R-R21, R-R22) as small clocked state machines the pane task drives.
//!
//! Each takes the current [`Instant`] from its caller, so the rules are tested without sleeping:
//!
//! - [`Cadence`]: at most 120 frames a second per pane, and the first change after a quiet spell goes out at once.
//! - [`ClientWindow`]: per attached client, at most [`MAX_UNACKED_DELTAS`] Deltas without an Ack. A client that
//!   stays blocked for [`FORCED_SNAPSHOT_AFTER`] gets a Snapshot, which supersedes its unacknowledged Deltas and
//!   so reopens the window; a client with anything unacknowledged and no Ack for [`ACK_TIMEOUT`] is disconnected.
//!   Sequence numbers are per client (the pane's `DeltaBuilder` numbers them).
//! - [`SyncHold`]: no Delta while the program holds DEC 2026 synchronized output open, for at most [`SYNC_CAP`];
//!   after that plyd ends the update itself.
//! - [`IdleTimer`]: scrollback is compressed once the pane's compression-activity token has not moved for
//!   [`IDLE_COMPRESS_AFTER`] (Ruling R19 makes this mandatory; P4 is read after 10 minutes idle).
//!
//! "No Delta without dirty rows" needs no state here: the pane's `DeltaBuilder` returns nothing for an idle pane.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Shortest interval between two frames to one pane's clients: 1/120 s.
pub const MIN_FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 120);

/// Deltas a client may have unacknowledged before plyd stops sending it more.
pub const MAX_UNACKED_DELTAS: usize = 4;

/// How long a client may stay blocked on its window before it gets a forced Snapshot.
pub const FORCED_SNAPSHOT_AFTER: Duration = Duration::from_secs(3);

/// How long a client with unacknowledged frames may go without any Ack before plyd disconnects it.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(30);

/// Longest a DEC 2026 synchronized update may hold Deltas back.
pub const SYNC_CAP: Duration = Duration::from_millis(150);

/// Quiet time after which a pane's scrollback is compressed.
pub const IDLE_COMPRESS_AFTER: Duration = Duration::from_secs(10);

/// The 120 Hz cap of one pane.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cadence {
    last: Option<Instant>,
}

impl Cadence {
    /// The earliest moment the next frame may go out: `now` after a quiet spell, else one interval after the last.
    pub fn next_at(&self, now: Instant) -> Instant {
        match self.last {
            Some(last) => (last + MIN_FRAME_INTERVAL).max(now),
            None => now,
        }
    }

    /// Records that frames went out at `now`.
    pub fn sent(&mut self, now: Instant) {
        self.last = Some(now);
    }
}

/// One attached client's acknowledgement window.
#[derive(Debug, Clone, Default)]
pub struct ClientWindow {
    last_sent: u64,
    acked: u64,
    deltas: VecDeque<u64>,
    waiting_since: Option<Instant>,
    blocked_since: Option<Instant>,
}

/// What an Ack did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckOutcome {
    /// The Ack was applied.
    Applied,
    /// The Ack named a frame never sent (or regressed); it was ignored.
    Ignored,
}

impl ClientWindow {
    /// Whether another Delta may go out now.
    pub fn can_send_delta(&self) -> bool {
        self.deltas.len() < MAX_UNACKED_DELTAS
    }

    /// Records a Snapshot with `seq` sent at `now`: earlier Deltas no longer count against the window.
    pub fn snapshot_sent(&mut self, seq: u64, now: Instant) {
        self.deltas.clear();
        self.blocked_since = None;
        self.sent(seq, now);
    }

    /// Records a Delta with `seq` sent at `now`.
    pub fn delta_sent(&mut self, seq: u64, now: Instant) {
        self.deltas.push_back(seq);
        self.sent(seq, now);
    }

    fn sent(&mut self, seq: u64, now: Instant) {
        self.last_sent = seq;
        self.waiting_since.get_or_insert(now);
    }

    /// Applies `Ack {seq}` received at `now`; any progress restarts the Ack timeout.
    pub fn ack(&mut self, seq: u64, now: Instant) -> AckOutcome {
        if seq > self.last_sent || seq < self.acked {
            return AckOutcome::Ignored;
        }
        self.acked = seq;
        while self.deltas.front().is_some_and(|&d| d <= seq) {
            self.deltas.pop_front();
        }
        self.waiting_since = (seq < self.last_sent).then_some(now);
        if self.can_send_delta() {
            self.blocked_since = None;
        }
        AckOutcome::Applied
    }

    /// Notes that the client has changes to receive but a full window, starting its blocked clock.
    pub fn note_blocked(&mut self, now: Instant) {
        self.blocked_since.get_or_insert(now);
    }

    /// When a forced Snapshot is due, if the client is blocked.
    pub fn forced_snapshot_at(&self) -> Option<Instant> {
        self.blocked_since.map(|t| t + FORCED_SNAPSHOT_AFTER)
    }

    /// When the client is to be disconnected for not acknowledging, if it has anything unacknowledged.
    pub fn ack_deadline(&self) -> Option<Instant> {
        self.waiting_since.map(|t| t + ACK_TIMEOUT)
    }

    /// The newest sequence number the client acknowledged.
    pub fn acked(&self) -> u64 {
        self.acked
    }
}

/// The DEC 2026 hold of one pane.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncHold {
    since: Option<Instant>,
}

impl SyncHold {
    /// Records the mode after a write batch: `open` starts the cap's clock once, `!open` clears it.
    pub fn observe(&mut self, open: bool, now: Instant) {
        if open {
            self.since.get_or_insert(now);
        } else {
            self.since = None;
        }
    }

    /// When plyd must end the update itself; `None` while no update is open.
    pub fn release_at(&self) -> Option<Instant> {
        self.since.map(|t| t + SYNC_CAP)
    }
}

/// The idle-compression clock of one pane.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdleTimer {
    token: Option<u64>,
    due: Option<Instant>,
}

impl IdleTimer {
    /// Records the engine's compression-activity token; a new value restarts the quiet period.
    pub fn observe(&mut self, token: u64, now: Instant) {
        if self.token != Some(token) {
            self.token = Some(token);
            self.due = Some(now + IDLE_COMPRESS_AFTER);
        }
    }

    /// Restarts the quiet period, e.g. after a history read decompressed pages.
    pub fn rearm(&mut self, now: Instant) {
        self.due = Some(now + IDLE_COMPRESS_AFTER);
    }

    /// Asks for another compression step at `now` (the previous one reported more work).
    pub fn continue_now(&mut self, now: Instant) {
        self.due = Some(now);
    }

    /// When the next compression step is due; `None` once everything is compressed.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// Records that compression finished until the token moves again.
    pub fn done(&mut self) {
        self.due = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_first_change_goes_out_at_once_then_at_most_120_hz() {
        let t0 = Instant::now();
        let mut c = Cadence::default();
        assert_eq!(c.next_at(t0), t0);
        c.sent(t0);
        assert_eq!(c.next_at(t0 + ms(1)), t0 + MIN_FRAME_INTERVAL);
        assert_eq!(c.next_at(t0 + ms(20)), t0 + ms(20));
    }

    #[test]
    fn four_unacked_deltas_close_the_window_and_an_ack_reopens_it() {
        let t0 = Instant::now();
        let mut w = ClientWindow::default();
        w.snapshot_sent(1, t0);
        for seq in 2..=5 {
            assert!(w.can_send_delta());
            w.delta_sent(seq, t0);
        }
        assert!(!w.can_send_delta());
        w.note_blocked(t0 + ms(10));
        assert_eq!(
            w.forced_snapshot_at(),
            Some(t0 + ms(10) + FORCED_SNAPSHOT_AFTER)
        );
        assert_eq!(w.ack(3, t0 + ms(20)), AckOutcome::Applied);
        assert!(w.can_send_delta());
        assert_eq!(w.forced_snapshot_at(), None);
        assert_eq!(w.ack_deadline(), Some(t0 + ms(20) + ACK_TIMEOUT));
        assert_eq!(w.ack(5, t0 + ms(30)), AckOutcome::Applied);
        assert_eq!(w.ack_deadline(), None, "nothing outstanding, no timeout");
    }

    #[test]
    fn a_forced_snapshot_supersedes_the_blocked_deltas() {
        let t0 = Instant::now();
        let mut w = ClientWindow::default();
        w.snapshot_sent(1, t0);
        for seq in 2..=5 {
            w.delta_sent(seq, t0);
        }
        w.note_blocked(t0);
        w.snapshot_sent(6, t0 + FORCED_SNAPSHOT_AFTER);
        assert!(w.can_send_delta());
        assert_eq!(w.forced_snapshot_at(), None);
        assert_eq!(
            w.ack_deadline(),
            Some(t0 + ACK_TIMEOUT),
            "no Ack yet since t0"
        );
    }

    #[test]
    fn acks_for_unsent_or_older_frames_are_ignored() {
        let t0 = Instant::now();
        let mut w = ClientWindow::default();
        w.snapshot_sent(1, t0);
        w.delta_sent(2, t0);
        assert_eq!(w.ack(9, t0), AckOutcome::Ignored);
        assert_eq!(w.ack(2, t0), AckOutcome::Applied);
        assert_eq!(w.ack(1, t0), AckOutcome::Ignored);
        assert_eq!(w.acked(), 2);
    }

    #[test]
    fn a_synchronized_update_is_capped_at_150_ms() {
        let t0 = Instant::now();
        let mut s = SyncHold::default();
        assert_eq!(s.release_at(), None);
        s.observe(true, t0);
        s.observe(true, t0 + ms(100));
        assert_eq!(s.release_at(), Some(t0 + SYNC_CAP));
        s.observe(false, t0 + ms(120));
        assert_eq!(s.release_at(), None);
    }

    #[test]
    fn compression_waits_for_a_quiet_token() {
        let t0 = Instant::now();
        let mut i = IdleTimer::default();
        i.observe(7, t0);
        assert_eq!(i.due(), Some(t0 + IDLE_COMPRESS_AFTER));
        i.observe(7, t0 + ms(500));
        assert_eq!(
            i.due(),
            Some(t0 + IDLE_COMPRESS_AFTER),
            "same token, same deadline"
        );
        i.observe(8, t0 + ms(600));
        assert_eq!(i.due(), Some(t0 + ms(600) + IDLE_COMPRESS_AFTER));
        i.done();
        assert_eq!(i.due(), None);
        i.observe(8, t0 + ms(700));
        assert_eq!(i.due(), None, "an unchanged token does not re-arm");
        i.rearm(t0 + ms(800));
        assert_eq!(i.due(), Some(t0 + ms(800) + IDLE_COMPRESS_AFTER));
    }
}
