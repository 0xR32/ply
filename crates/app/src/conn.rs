//! One pane's C2 connection: a reader thread decodes frames into a shared replica, off the UI thread, and re-attaches
//! after a drop (100 ms doubling to 2 s, as the app's data client does).

use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use ply_proto::C2_VERSION;
use ply_proto::data::{
    Ack, Attach, FetchHistory, Focus, Frame, FrameReader, MAX_HISTORY_ROWS, Resize,
};
use ply_term::Replica;

/// Locks `m`; a panicked holder leaves plain data behind, so the guard is taken anyway.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
}

#[derive(Default)]
pub struct Shared {
    pub replica: Mutex<Replica>,
    pub title: Mutex<String>,
    pub exit: Mutex<Option<i32>>,
    pub dirty: AtomicBool,
    /// A FETCH_HISTORY is in flight; at most one at a time, like the app.
    pub history_pending: AtomicBool,
    /// A start plyd answered with no rows, not asked again until the next Snapshot.
    pub history_exhausted: Mutex<Option<u64>>,
}

pub struct Conn {
    pub pane_id: u64,
    pub shared: Arc<Shared>,
    socket: PathBuf,
    writer: Mutex<Option<UnixStream>>,
    grid: Mutex<Option<Grid>>,
    wake: UnboundedSender<()>,
    closed: AtomicBool,
    want_focus: AtomicBool,
    /// The focus last reported on this attachment; `None` until its Snapshot, so a new attachment reports it again.
    focus_sent: Mutex<Option<bool>>,
}

impl Conn {
    pub fn new(pane_id: u64, socket: PathBuf, wake: UnboundedSender<()>) -> Arc<Self> {
        Arc::new(Self {
            pane_id,
            shared: Arc::default(),
            socket,
            writer: Mutex::new(None),
            grid: Mutex::new(None),
            wake,
            closed: AtomicBool::new(false),
            want_focus: AtomicBool::new(false),
            focus_sent: Mutex::new(None),
        })
    }

    /// Attaches on the first call, sends RESIZE when the grid changes after that.
    pub fn ensure_grid(self: &Arc<Self>, grid: Grid) {
        let mut current = lock(&self.grid);
        if *current == Some(grid) {
            return;
        }
        let first = current.is_none();
        *current = Some(grid);
        drop(current);
        if first {
            let conn = self.clone();
            let spawned = thread::Builder::new()
                .name(format!("c2-pane-{}", self.pane_id))
                .spawn(move || conn.run());
            if let Err(e) = spawned {
                tracing::error!(pane_id = self.pane_id, error = %e, "cannot start the pane's reader thread");
            }
        } else {
            self.send(&Frame::Resize(Resize {
                cols: grid.cols,
                rows: grid.rows,
                cell_width_px: grid.cell_w,
                cell_height_px: grid.cell_h,
            }));
        }
    }

    pub fn send(&self, frame: &Frame) {
        let mut buf = Vec::with_capacity(64);
        if let Err(e) = frame.encode(&mut buf) {
            tracing::warn!(pane_id = self.pane_id, error = %e, "cannot encode a C2 frame");
            return;
        }
        if let Some(w) = lock(&self.writer).as_mut()
            && let Err(e) = w.write_all(&buf)
        {
            tracing::debug!(pane_id = self.pane_id, error = %e, "C2 write failed; the reader re-attaches");
        }
    }

    /// Asks for the page of scrollback ending at `line`, unless one is in flight or `line`'s page came back empty.
    pub fn fetch_history(&self, line: u64) {
        let base = lock(&self.shared.replica).scrollback_base();
        let start = line
            .saturating_sub(u64::from(MAX_HISTORY_ROWS) / 2)
            .max(base);
        if *lock(&self.shared.history_exhausted) == Some(start)
            || self.shared.history_pending.swap(true, Ordering::SeqCst)
        {
            return;
        }
        self.send(&Frame::FetchHistory(FetchHistory {
            start,
            count: MAX_HISTORY_ROWS,
        }));
    }

    /// Detaches for good: the reader thread ends and does not re-attach.
    /// Reports focus in or out (plyd passes it on under mode 1004) when it changed or the attachment is new.
    pub fn set_focus(&self, focused: bool) {
        self.want_focus.store(focused, Ordering::SeqCst);
        let mut sent = lock(&self.focus_sent);
        if sent.is_some_and(|s| s != focused) {
            self.send(&Frame::Focus(Focus { focused }));
            *sent = Some(focused);
        }
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(w) = lock(&self.writer).take()
            && let Err(e) = w.shutdown(std::net::Shutdown::Both)
        {
            tracing::debug!(pane_id = self.pane_id, error = %e, "C2 shutdown");
        }
    }

    fn wake(&self) {
        if !self.shared.dirty.swap(true, Ordering::SeqCst) {
            let _ = self.wake.unbounded_send(());
        }
    }

    fn run(self: Arc<Self>) {
        let mut backoff = Duration::from_millis(100);
        while !self.closed.load(Ordering::SeqCst) {
            match self.attach_once() {
                Ok(true) => backoff = Duration::from_millis(100),
                Ok(false) => return,
                Err(e) => {
                    tracing::debug!(pane_id = self.pane_id, error = %e, "C2 connection down; re-attaching")
                }
            }
            if self.closed.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(2));
        }
    }

    /// One attachment; `Ok(false)` when plyd refused it for good, `Ok(true)` after a Snapshot then a clean end.
    fn attach_once(&self) -> std::io::Result<bool> {
        let stream = UnixStream::connect(&self.socket)?;
        *lock(&self.writer) = Some(stream.try_clone()?);
        if self.closed.load(Ordering::SeqCst) {
            self.close();
            return Ok(false);
        }
        let Some(grid) = *lock(&self.grid) else {
            return Ok(false);
        };
        self.send(&Frame::Attach(Attach {
            v: C2_VERSION,
            pane_id: self.pane_id,
            cols: grid.cols,
            rows: grid.rows,
            cell_width_px: grid.cell_w,
            cell_height_px: grid.cell_h,
        }));
        let mut reader = FrameReader::new(BufReader::with_capacity(1 << 18, stream));
        *lock(&self.focus_sent) = None;
        let mut attached = false;
        loop {
            let frame = match reader.read_frame() {
                Ok(Some(f)) => f,
                Ok(None) => return Ok(attached),
                Err(e) => {
                    tracing::debug!(pane_id = self.pane_id, error = %e, "C2 read ended");
                    return Ok(attached);
                }
            };
            let seq = match &frame {
                Frame::Snapshot(s) => Some(s.seq),
                Frame::Delta(d) => Some(d.seq),
                _ => None,
            };
            match &frame {
                Frame::Snapshot(_) => {
                    *lock(&self.shared.replica) = Replica::new();
                    if let Err(e) = lock(&self.shared.replica).apply(&frame) {
                        tracing::warn!(pane_id = self.pane_id, error = %e, "bad Snapshot");
                        return Ok(attached);
                    }
                    *lock(&self.shared.history_exhausted) = None;
                    attached = true;
                    let mut sent = lock(&self.focus_sent);
                    if sent.is_none() {
                        let focused = self.want_focus.load(Ordering::SeqCst);
                        self.send(&Frame::Focus(Focus { focused }));
                        *sent = Some(focused);
                    }
                }
                Frame::Delta(_) | Frame::History(_) => {
                    if let Err(e) = lock(&self.shared.replica).apply(&frame) {
                        tracing::warn!(pane_id = self.pane_id, error = %e, "bad frame; re-attaching");
                        return Ok(attached);
                    }
                    if let Frame::History(h) = &frame {
                        if h.lines.is_empty() {
                            *lock(&self.shared.history_exhausted) = Some(h.start);
                        }
                        self.shared.history_pending.store(false, Ordering::SeqCst);
                    }
                }
                Frame::Title(t) => *lock(&self.shared.title) = t.clone(),
                Frame::Exit(e) => *lock(&self.shared.exit) = Some(e.code),
                Frame::AttachRefused(r) => {
                    tracing::warn!(pane_id = self.pane_id, message = %r.message, "attach refused");
                    return Ok(false);
                }
                _ => {}
            }
            if let Some(seq) = seq {
                self.send(&Frame::Ack(Ack { seq }));
            }
            self.wake();
        }
    }
}
