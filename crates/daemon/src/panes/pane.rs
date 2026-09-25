//! One pane's task: the only owner of its libghostty-vt [`Engine`], its pty channels and its attached C2 clients.
//!
//! Everything that touches the engine runs here, one message at a time, so the engine's no-concurrent-access rule
//! holds without a lock. The task feeds pty output into the engine in batches, queues the engine's answers (DA,
//! DSR, OSC 10/11, size reports) and the clients' encoded input for the pty writer thread without ever waiting on
//! it (a child that stops reading its input cannot stall output), and publishes screen changes to every attached
//! client under the rules of [`crate::publisher`]: the 120 Hz cadence, the per-client Ack window with its forced
//! Snapshot and disconnect, the DEC 2026 hold, and idle-scrollback compression. Title changes and bells are coalesced
//! to the same cadence. OSC 7 updates the pane's directory (`pane.meta`); the process's exit is published after its
//! last output, as C2 EXIT to the clients and `pane.status`/`pane.exit` to C1. OSC 9 notifications and hook-driven
//! status belong to the agent state machine (WP6) and are only logged here.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ply_proto::data::{Exit, Frame};
use ply_proto::pane::{Cli, OptionAsMeta, PaneId, Rgb};
use ply_term::{
    Compression, DeltaBuilder, Encoded, Engine, EngineOutput, Input, Palette, Update, encode_input,
};
use rustix::process::Signal;
use tokio::sync::{mpsc, oneshot};

use crate::daemon::{Shared, unix_now};
use crate::pty::{Geometry, PaneProcess, PtyChannels};
use crate::publisher::{AckOutcome, Cadence, ClientWindow, IdleTimer, SyncHold};

/// Commands a pane task accepts; the channel is bounded ([`COMMAND_CAPACITY`]).
#[derive(Debug)]
pub enum PaneCmd {
    /// A C2 client attached with its view size; the task answers with a Snapshot on `out`.
    Attach {
        /// Unique per connection.
        client: u64,
        /// The client's grid and cell size.
        geometry: Geometry,
        /// Encoded frames for the client's socket.
        out: mpsc::Sender<Vec<u8>>,
    },
    /// The client's connection ended.
    Detach {
        /// The client.
        client: u64,
    },
    /// A client frame after ATTACH: input, RESIZE, FETCH_HISTORY or ACK.
    Frame {
        /// The client.
        client: u64,
        /// The decoded frame.
        frame: Frame,
    },
    /// Bytes for the pty as they are (`pane.answer` digits).
    Write(Vec<u8>),
    /// A new palette from `theme.set`.
    SetPalette(Palette),
    /// A new `option_as_meta` setting.
    SetOptionAsMeta(OptionAsMeta),
    /// SIGHUP to the process group, SIGKILL after [`KILL_GRACE`] (Ruling R7).
    Kill,
    /// A new process for this pane (`pane.resume`).
    Start(Box<Started>),
    /// End the task: the pane was closed or plyd is stopping.
    Stop,
}

/// A spawned process with its pty channels, handed to a pane task.
#[derive(Debug)]
pub struct Started {
    /// The process.
    pub process: PaneProcess,
    /// Its output, input and exit channels.
    pub channels: PtyChannels,
}

/// Capacity of a pane task's command channel.
pub const COMMAND_CAPACITY: usize = 256;

/// Frames queued towards one client's socket before plyd gives up on it.
pub const CLIENT_QUEUE: usize = 32;

/// Time between SIGHUP and SIGKILL for `pane.close {kill:true}` (Ruling R7).
pub const KILL_GRACE: Duration = Duration::from_secs(2);

/// How long after the process's exit plyd waits for the pty's end-of-file before publishing the exit.
pub const EXIT_GRACE: Duration = Duration::from_millis(250);

/// Most output bytes fed to the engine before the task publishes and looks at other work.
pub const BATCH_BYTES: usize = 512 * 1024;

/// Most bytes of pty input queued while the child does not read; beyond it input is dropped and logged.
pub const MAX_PENDING_INPUT: usize = 16 * 1024 * 1024;

/// Time budget of one idle-compression slice before the task yields to other work.
const COMPRESS_SLICE: Duration = Duration::from_millis(5);

/// Minimum interval between two row writes (`last_activity_at`, title) caused by output of one pane.
const ACTIVITY_WRITE_INTERVAL: Duration = Duration::from_secs(5);

/// Everything a pane task starts from.
#[derive(Debug)]
pub struct PaneSeed {
    /// The pane.
    pub id: PaneId,
    /// Its program, for the default title.
    pub cli: Cli,
    /// Title used while the terminal sets none.
    pub default_title: String,
    /// Its engine, already sized and given the palette.
    pub engine: Engine,
    /// The engine's current size.
    pub geometry: Geometry,
    /// A running process, if any.
    pub started: Option<Started>,
    /// The exit code of a process that already ended (a pane restored after a restart).
    pub exited: Option<i32>,
}

/// A neutral palette for engines created before any `theme.set` (restored panes only; spawns wait for the real one).
pub fn fallback_palette() -> Palette {
    let grey = |v: u8| Rgb { r: v, g: v, b: v };
    let mut ansi = [grey(0x80); 16];
    for (i, slot) in ansi.iter_mut().enumerate() {
        let v = u8::try_from(i * 16).unwrap_or(u8::MAX);
        *slot = grey(v);
    }
    Palette {
        ansi,
        fg: grey(0xe6),
        bg: grey(0x0c),
        cursor: grey(0xe6),
        cursor_text: grey(0x0c),
        selection_bg: grey(0x40),
        selection_fg: grey(0xe6),
    }
}

/// Starts the task on the current tokio runtime and returns its command channel.
pub fn spawn_task(shared: Arc<Shared>, seed: PaneSeed) -> mpsc::Sender<PaneCmd> {
    let (tx, rx) = mpsc::channel(COMMAND_CAPACITY);
    let mut task = PaneTask {
        id: seed.id,
        cli: seed.cli,
        default_title: seed.default_title,
        shared,
        engine: seed.engine,
        geometry: seed.geometry,
        clients: Vec::new(),
        process: None,
        output: None,
        input: None,
        exit: None,
        exit_code: seed.exited,
        exit_reported: seed.exited.is_some(),
        exit_grace: None,
        kill_at: None,
        writes: WriteQueue::default(),
        cadence: Cadence::default(),
        sync: SyncHold::default(),
        idle: IdleTimer::default(),
        publish_at: None,
        title: String::new(),
        pending_title: false,
        pending_bell: false,
        last_activity_write: None,
        persist_at: None,
        closing: false,
        stop: false,
    };
    if let Some(started) = seed.started {
        task.adopt(started);
    }
    tokio::spawn(task.run(rx));
    tx
}

#[derive(Debug)]
struct Client {
    id: u64,
    builder: DeltaBuilder,
    out: mpsc::Sender<Vec<u8>>,
    window: ClientWindow,
    dirty: bool,
    closed: bool,
}

#[derive(Debug, Default)]
struct WriteQueue {
    chunks: VecDeque<Vec<u8>>,
    bytes: usize,
}

impl WriteQueue {
    fn push(&mut self, pane_id: PaneId, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if self.bytes + bytes.len() > MAX_PENDING_INPUT {
            tracing::warn!(
                pane_id,
                dropped = bytes.len(),
                "the pane's process is not reading its input; input dropped"
            );
            return;
        }
        self.bytes += bytes.len();
        self.chunks.push_back(bytes);
    }

    fn pop(&mut self) -> Option<Vec<u8>> {
        let chunk = self.chunks.pop_front()?;
        self.bytes -= chunk.len();
        Some(chunk)
    }

    fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    fn clear(&mut self) {
        self.chunks.clear();
        self.bytes = 0;
    }
}

struct PaneTask {
    id: PaneId,
    cli: Cli,
    default_title: String,
    shared: Arc<Shared>,
    engine: Engine,
    geometry: Geometry,
    clients: Vec<Client>,
    process: Option<PaneProcess>,
    output: Option<mpsc::Receiver<Vec<u8>>>,
    input: Option<mpsc::Sender<Vec<u8>>>,
    exit: Option<oneshot::Receiver<i32>>,
    exit_code: Option<i32>,
    exit_reported: bool,
    exit_grace: Option<Instant>,
    kill_at: Option<Instant>,
    writes: WriteQueue,
    cadence: Cadence,
    sync: SyncHold,
    idle: IdleTimer,
    publish_at: Option<Instant>,
    title: String,
    pending_title: bool,
    pending_bell: bool,
    last_activity_write: Option<Instant>,
    persist_at: Option<Instant>,
    closing: bool,
    stop: bool,
}

async fn recv_output(rx: &mut Option<mpsc::Receiver<Vec<u8>>>) -> Option<Vec<u8>> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

async fn wait_exit(rx: &mut Option<oneshot::Receiver<i32>>) -> Option<i32> {
    match rx {
        Some(rx) => rx.await.ok(),
        None => std::future::pending().await,
    }
}

async fn reserve(tx: Option<mpsc::Sender<Vec<u8>>>) -> Option<mpsc::OwnedPermit<Vec<u8>>> {
    match tx {
        Some(tx) => tx.reserve_owned().await.ok(),
        None => std::future::pending().await,
    }
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

impl PaneTask {
    async fn run(mut self, mut cmds: mpsc::Receiver<PaneCmd>) {
        tracing::debug!(pane_id = self.id, "pane task started");
        while !self.stop {
            let deadline = self.next_deadline();
            let writable = !self.writes.is_empty() && self.input.is_some();
            tokio::select! {
                biased;
                cmd = cmds.recv(), if !self.closing => match cmd {
                    Some(PaneCmd::Stop) | None => break,
                    Some(cmd) => self.on_command(cmd),
                },
                code = wait_exit(&mut self.exit), if self.exit.is_some() => {
                    self.exit = None;
                    self.on_exit(code);
                }
                permit = reserve(self.input.clone()), if writable => match permit {
                    Some(permit) => {
                        if let Some(bytes) = self.writes.pop() {
                            permit.send(bytes);
                        }
                    }
                    None => {
                        tracing::debug!(pane_id = self.id, "the pty writer ended; pending input dropped");
                        self.input = None;
                        self.writes.clear();
                    }
                },
                chunk = recv_output(&mut self.output), if self.output.is_some() => match chunk {
                    Some(chunk) => self.on_output(&chunk),
                    None => self.on_output_closed(),
                },
                () = sleep_until(deadline), if deadline.is_some() => {}
            }
            self.on_tick(Instant::now());
        }
        if self.kill_at.take().is_some() {
            self.kill_group();
        }
        tracing::debug!(pane_id = self.id, "pane task stopped");
    }

    fn adopt(&mut self, started: Started) {
        if let Err(e) = started.process.resize(self.geometry) {
            tracing::debug!(pane_id = self.id, error = %e, "cannot size the new pty");
        }
        self.process = Some(started.process);
        self.output = Some(started.channels.output);
        self.input = Some(started.channels.input);
        self.exit = Some(started.channels.exit);
        self.exit_code = None;
        self.exit_reported = false;
        self.exit_grace = None;
        self.kill_at = None;
        self.writes.clear();
    }

    fn on_command(&mut self, cmd: PaneCmd) {
        let now = Instant::now();
        match cmd {
            PaneCmd::Attach {
                client,
                geometry,
                out,
            } => self.attach(client, geometry, out, now),
            PaneCmd::Detach { client } => {
                self.clients.retain(|c| c.id != client);
                self.engine.reset_mouse_buttons();
                tracing::debug!(pane_id = self.id, client, "client detached");
            }
            PaneCmd::Frame { client, frame } => self.on_frame(client, frame, now),
            PaneCmd::Write(bytes) => self.write(bytes),
            PaneCmd::SetPalette(palette) => {
                if let Err(e) = self.engine.set_palette(&palette) {
                    tracing::warn!(pane_id = self.id, error = %e, "cannot apply the new palette");
                }
            }
            PaneCmd::SetOptionAsMeta(option) => self.engine.set_option_as_meta(option),
            PaneCmd::Kill => self.kill(now),
            PaneCmd::Start(started) => {
                self.adopt(*started);
                tracing::info!(pane_id = self.id, "pane process relaunched");
            }
            PaneCmd::Stop => self.stop = true,
        }
    }

    fn kill(&mut self, now: Instant) {
        if self.exit_code.is_some() {
            return;
        }
        let Some(process) = &self.process else {
            return;
        };
        if let Err(e) = process.signal_group(Signal::HUP) {
            tracing::warn!(pane_id = self.id, error = %e, "cannot send SIGHUP to the pane's processes");
        }
        self.kill_at = Some(now + KILL_GRACE);
    }

    /// SIGKILL to the whole process group, even after its leader exited: a member that ignored SIGHUP keeps the pty open.
    fn kill_group(&self) {
        let Some(process) = &self.process else {
            return;
        };
        match process.signal_group(Signal::KILL) {
            Ok(()) => tracing::info!(
                pane_id = self.id,
                leader_exited = self.exit_code.is_some(),
                "SIGHUP grace over; sent SIGKILL to the pane's process group"
            ),
            Err(e) if crate::pty::is_no_such_process(&e) => {
                tracing::debug!(
                    pane_id = self.id,
                    "no process of the pane's group was left to kill"
                );
            }
            Err(e) => {
                tracing::warn!(pane_id = self.id, error = %e, "cannot send SIGKILL to the pane's processes");
            }
        }
    }

    fn write(&mut self, bytes: Vec<u8>) {
        if self.input.is_none() {
            tracing::debug!(
                pane_id = self.id,
                bytes = bytes.len(),
                "input for a pane without a process dropped"
            );
            return;
        }
        self.writes.push(self.id, bytes);
    }

    fn attach(&mut self, id: u64, geometry: Geometry, out: mpsc::Sender<Vec<u8>>, now: Instant) {
        self.apply_geometry(geometry, now);
        let mut client = Client {
            id,
            builder: DeltaBuilder::new(),
            out,
            window: ClientWindow::default(),
            dirty: false,
            closed: false,
        };
        send_snapshot(self.id, &mut self.engine, &mut client, now);
        if !self.title.is_empty() {
            send(self.id, &mut client, &Frame::Title(self.title.clone()));
        }
        if let (true, Some(code)) = (self.exit_reported, self.exit_code) {
            send(self.id, &mut client, &Frame::Exit(Exit { code }));
        }
        tracing::debug!(
            pane_id = self.id,
            client = id,
            cols = geometry.cols,
            rows = geometry.rows,
            "client attached"
        );
        self.clients.push(client);
        self.idle.rearm(now);
    }

    fn apply_geometry(&mut self, geometry: Geometry, now: Instant) {
        if geometry == self.geometry {
            return;
        }
        if !geometry.fits_one_frame() {
            tracing::warn!(
                pane_id = self.id,
                cols = geometry.cols,
                rows = geometry.rows,
                "RESIZE to a grid whose Snapshot cannot fit one C2 frame ignored"
            );
            return;
        }
        match self.engine.resize(
            geometry.cols,
            geometry.rows,
            geometry.cell_width_px,
            geometry.cell_height_px,
        ) {
            Ok(out) => self.on_effects(out),
            Err(e) => {
                tracing::warn!(pane_id = self.id, error = %e, "cannot resize the terminal");
                return;
            }
        }
        if let Some(process) = &self.process
            && self.exit_code.is_none()
            && let Err(e) = process.resize(geometry)
        {
            tracing::debug!(pane_id = self.id, error = %e, "cannot resize the pty");
        }
        self.geometry = geometry;
        self.shared.set_geometry(geometry);
        self.after_change(now);
    }

    fn on_frame(&mut self, client_id: u64, frame: Frame, now: Instant) {
        match frame {
            Frame::InputRaw(bytes) => self.write(bytes),
            Frame::Resize(r) => {
                self.apply_geometry(
                    Geometry {
                        cols: r.cols,
                        rows: r.rows,
                        cell_width_px: r.cell_width_px,
                        cell_height_px: r.cell_height_px,
                    },
                    now,
                );
                if let Some(client) = self.clients.iter_mut().find(|c| c.id == client_id) {
                    send_snapshot(self.id, &mut self.engine, client, now);
                }
            }
            Frame::FetchHistory(f) => {
                if let Some(client) = self.clients.iter_mut().find(|c| c.id == client_id) {
                    match client.builder.fetch_history(&mut self.engine, &f) {
                        Ok(history) => {
                            send(self.id, client, &Frame::History(history));
                        }
                        Err(e) => {
                            tracing::warn!(pane_id = self.id, client = client_id, error = %e, "cannot read the scrollback");
                        }
                    }
                }
                self.idle.rearm(now);
            }
            Frame::Ack(ack) => {
                if let Some(client) = self.clients.iter_mut().find(|c| c.id == client_id)
                    && client.window.ack(ack.seq, now) == AckOutcome::Ignored
                {
                    tracing::debug!(
                        pane_id = self.id,
                        client = client_id,
                        seq = ack.seq,
                        "ack for a frame never sent ignored"
                    );
                }
                self.schedule_publish(now);
            }
            frame @ (Frame::Key(_) | Frame::Mouse(_) | Frame::Paste(_) | Frame::Focus(_)) => {
                let Some(input) = Input::from_frame(&frame) else {
                    return;
                };
                if matches!(frame, Frame::Focus(f) if !f.focused) {
                    self.engine.reset_mouse_buttons();
                }
                match encode_input(&mut self.engine, input) {
                    Ok(Encoded::Bytes(bytes)) => self.write(bytes),
                    Ok(Encoded::Nothing) => {}
                    Ok(Encoded::PasteRejected) => {
                        if let Some(client) = self.clients.iter_mut().find(|c| c.id == client_id) {
                            send(self.id, client, &Frame::PasteRejected);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(pane_id = self.id, error = %e, "cannot encode the input");
                    }
                }
            }
            other => {
                tracing::warn!(
                    pane_id = self.id,
                    kind = other.kind(),
                    "frame kind not accepted after ATTACH"
                );
            }
        }
    }

    fn on_output(&mut self, chunk: &[u8]) {
        let now = Instant::now();
        let out = self.engine.write(chunk);
        self.on_effects(out);
        let mut total = chunk.len();
        while total < BATCH_BYTES {
            let Some(Ok(more)) = self.output.as_mut().map(mpsc::Receiver::try_recv) else {
                break;
            };
            total += more.len();
            let out = self.engine.write(&more);
            self.on_effects(out);
        }
        self.after_change(now);
        match self.last_activity_write {
            Some(t) if now.duration_since(t) < ACTIVITY_WRITE_INTERVAL => {
                self.persist_at.get_or_insert(t + ACTIVITY_WRITE_INTERVAL);
            }
            _ => self.persist(now),
        }
    }

    /// Stores the pane's row (activity time and title) now; output between two writes is stored at most every 5 s.
    fn persist(&mut self, now: Instant) {
        self.persist_at = None;
        self.last_activity_write = Some(now);
        self.shared.registry().touch(self.id, unix_now());
    }

    fn on_effects(&mut self, out: EngineOutput) {
        if out.is_empty() {
            return;
        }
        if !out.reply.is_empty() {
            self.write(out.reply);
        }
        if out.bells > 0 {
            self.pending_bell = true;
        }
        if let Some(title) = out.title {
            self.shared
                .registry()
                .set_title(self.id, &title, &self.default_title);
            self.title = title;
            self.pending_title = true;
        }
        if let Some(uri) = out.pwd {
            match decode_osc7(&uri) {
                Some(cwd) => self.shared.registry().set_cwd(self.id, &cwd),
                None => {
                    tracing::debug!(pane_id = self.id, uri = %uri, "OSC 7 without a local file URI ignored");
                }
            }
        }
        for n in &out.notifications {
            tracing::debug!(pane_id = self.id, cli = ?self.cli, title = %n.title, body = %n.body, "desktop notification");
        }
        if !out.clipboard_writes.is_empty() {
            tracing::debug!(
                pane_id = self.id,
                writes = out.clipboard_writes.len(),
                "OSC 52 clipboard write"
            );
        }
    }

    fn after_change(&mut self, now: Instant) {
        for client in &mut self.clients {
            client.dirty = true;
        }
        self.sync
            .observe(self.engine.is_synchronized_update_open(), now);
        self.idle.observe(self.engine.compression_activity(), now);
        self.schedule_publish(now);
    }

    fn wants_publish(&self) -> bool {
        self.pending_title
            || self.pending_bell
            || self
                .clients
                .iter()
                .any(|c| c.dirty && c.window.can_send_delta())
    }

    fn schedule_publish(&mut self, now: Instant) {
        for client in &mut self.clients {
            if client.dirty && !client.window.can_send_delta() {
                client.window.note_blocked(now);
            }
        }
        if !self.wants_publish() {
            self.publish_at = None;
            return;
        }
        let at = self.cadence.next_at(now);
        self.publish_at = Some(self.sync.release_at().map_or(at, |release| release.max(at)));
    }

    fn publish(&mut self, now: Instant) {
        self.publish_at = None;
        if self.sync.release_at().is_some() {
            if self.engine.is_synchronized_update_open()
                && let Err(e) = self.engine.end_synchronized_update()
            {
                tracing::warn!(pane_id = self.id, error = %e, "cannot end the synchronized update");
            }
            self.sync.observe(false, now);
        }
        for client in &mut self.clients {
            if self.pending_title {
                send(self.id, client, &Frame::Title(self.title.clone()));
            }
            if self.pending_bell {
                send(self.id, client, &Frame::Bell);
            }
            if !client.dirty || client.closed {
                continue;
            }
            if !client.window.can_send_delta() {
                client.window.note_blocked(now);
                continue;
            }
            match client.builder.delta(&mut self.engine) {
                Ok(Some(Update::Snapshot(snapshot))) => {
                    let seq = snapshot.seq;
                    if send(self.id, client, &Frame::Snapshot(snapshot)) {
                        client.window.snapshot_sent(seq, now);
                    }
                }
                Ok(Some(Update::Delta(delta))) => {
                    let seq = delta.seq;
                    if send(self.id, client, &Frame::Delta(delta)) {
                        client.window.delta_sent(seq, now);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(pane_id = self.id, client = client.id, error = %e, "cannot build a Delta");
                }
            }
            client.dirty = false;
        }
        self.pending_title = false;
        self.pending_bell = false;
        self.cadence.sent(now);
        self.clients.retain(|c| !c.closed);
        self.schedule_publish(now);
    }

    fn on_exit(&mut self, code: Option<i32>) {
        let code = match code {
            Some(code) => code,
            None => {
                tracing::error!(
                    pane_id = self.id,
                    "the pane's waiter thread ended without an exit code"
                );
                crate::pty::EXIT_UNKNOWN
            }
        };
        self.exit_code = Some(code);
        if self.output.is_none() {
            self.finish_exit(Instant::now());
        } else {
            self.exit_grace = Some(Instant::now() + EXIT_GRACE);
        }
    }

    fn on_output_closed(&mut self) {
        self.output = None;
        tracing::debug!(pane_id = self.id, "pty output ended");
        if self.exit_code.is_some() {
            self.finish_exit(Instant::now());
        }
    }

    fn finish_exit(&mut self, now: Instant) {
        self.exit_grace = None;
        if self.exit_reported {
            return;
        }
        let Some(code) = self.exit_code else {
            return;
        };
        self.exit_reported = true;
        while let Some(Ok(chunk)) = self.output.as_mut().map(mpsc::Receiver::try_recv) {
            let out = self.engine.write(&chunk);
            self.on_effects(out);
        }
        self.after_change(now);
        self.publish(now);
        for client in &mut self.clients {
            send(self.id, client, &Frame::Exit(Exit { code }));
        }
        self.input = None;
        self.writes.clear();
        let close = self
            .shared
            .registry()
            .mark_exited(self.id, code, unix_now());
        self.shared.update_power();
        if close {
            let closed = self.shared.registry().close_pane(self.id, unix_now());
            match closed {
                Ok(_) => {
                    super::launch::remove_pane_dir(self.id, &self.shared.paths.pane_dir(self.id));
                    self.closing = true;
                    self.stop = self.kill_at.is_none();
                }
                Err(e) => {
                    tracing::warn!(pane_id = self.id, error = %e.msg, "cannot close the pane after its exit");
                }
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let clients = self
            .clients
            .iter()
            .flat_map(|c| [c.window.forced_snapshot_at(), c.window.ack_deadline()]);
        [
            self.publish_at,
            self.exit_grace,
            self.kill_at,
            self.persist_at,
            self.idle.due(),
        ]
        .into_iter()
        .chain(clients)
        .flatten()
        .min()
    }

    fn on_tick(&mut self, now: Instant) {
        if self.exit_grace.is_some_and(|t| t <= now) {
            self.finish_exit(now);
        }
        if self.persist_at.is_some_and(|t| t <= now) {
            self.persist(now);
        }
        if self.kill_at.is_some_and(|t| t <= now) {
            self.kill_at = None;
            self.kill_group();
            if self.closing {
                self.stop = true;
            }
        }
        for client in &mut self.clients {
            if client.window.ack_deadline().is_some_and(|t| t <= now) {
                tracing::warn!(
                    pane_id = self.id,
                    client = client.id,
                    "no Ack for 30 s; disconnecting the client"
                );
                client.closed = true;
            } else if client.window.forced_snapshot_at().is_some_and(|t| t <= now) {
                tracing::info!(
                    pane_id = self.id,
                    client = client.id,
                    "client blocked for 3 s; sending a Snapshot"
                );
                send_snapshot(self.id, &mut self.engine, client, now);
            }
        }
        self.clients.retain(|c| !c.closed);
        if self.publish_at.is_some_and(|t| t <= now) {
            self.publish(now);
        }
        if self.idle.due().is_some_and(|t| t <= now) {
            self.compress(now);
        }
    }

    fn compress(&mut self, now: Instant) {
        let until = now + COMPRESS_SLICE;
        loop {
            match self.engine.compress_idle() {
                Ok(Compression::Pending) if Instant::now() < until => {}
                Ok(Compression::Pending) => {
                    self.idle.continue_now(Instant::now());
                    return;
                }
                Ok(Compression::Complete) => {
                    tracing::debug!(pane_id = self.id, "idle scrollback compressed");
                    self.idle.done();
                    return;
                }
                Ok(Compression::Unsupported) => {
                    self.idle.done();
                    return;
                }
                Err(e) => {
                    tracing::warn!(pane_id = self.id, error = %e, "idle compression failed");
                    self.idle.done();
                    return;
                }
            }
        }
    }
}

fn send(pane_id: PaneId, client: &mut Client, frame: &Frame) -> bool {
    if client.closed {
        return false;
    }
    let mut bytes = Vec::new();
    if let Err(e) = frame.encode(&mut bytes) {
        tracing::warn!(pane_id, client = client.id, kind = frame.kind(), error = %e, "cannot encode a frame; disconnecting the client");
        client.closed = true;
        return false;
    }
    match client.out.try_send(bytes) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            tracing::warn!(
                pane_id,
                client = client.id,
                "client is not reading its frames; disconnecting it"
            );
            client.closed = true;
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            tracing::debug!(
                pane_id,
                client = client.id,
                "the client's connection closed; dropping it"
            );
            client.closed = true;
            false
        }
    }
}

fn send_snapshot(pane_id: PaneId, engine: &mut Engine, client: &mut Client, now: Instant) {
    match client.builder.snapshot(engine) {
        Ok(snapshot) => {
            let seq = snapshot.seq;
            if send(pane_id, client, &Frame::Snapshot(snapshot)) {
                client.window.snapshot_sent(seq, now);
                client.dirty = false;
            }
        }
        Err(e) => {
            tracing::warn!(pane_id, client = client.id, error = %e, "cannot build a Snapshot; disconnecting the client");
            client.closed = true;
        }
    }
}

/// The local path of an OSC 7 `file://host/path` URI, percent-decoded; `None` for other schemes or a relative path.
pub fn decode_osc7(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = path.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc7_uris_decode_to_local_paths() {
        assert_eq!(
            decode_osc7("file://example-host/Users/example/My%20Code"),
            Some("/Users/example/My Code".to_owned())
        );
        assert_eq!(decode_osc7("file:///tmp"), Some("/tmp".to_owned()));
        assert_eq!(decode_osc7("kitty-shell-cwd://host/tmp"), None);
        assert_eq!(decode_osc7("file://host"), None);
    }

    #[test]
    fn the_write_queue_caps_pending_input() {
        let mut q = WriteQueue::default();
        q.push(1, vec![0; MAX_PENDING_INPUT]);
        q.push(1, vec![1]);
        assert_eq!(q.chunks.len(), 1, "input beyond the cap is dropped");
        assert_eq!(q.pop().map(|c| c.len()), Some(MAX_PENDING_INPUT));
        assert!(q.is_empty());
        q.push(1, vec![2]);
        assert_eq!(q.bytes, 1);
    }
}
