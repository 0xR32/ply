//! One pane's task: the only owner of its libghostty-vt [`Engine`], its pty channels and its attached C2 clients.
//!
//! Everything that touches the engine runs here, one message at a time, so the engine's no-concurrent-access rule
//! holds without a lock. The task feeds pty output into the engine in batches, queues the engine's answers (DA,
//! DSR, OSC 10/11, size reports) and the clients' encoded input for the pty writer thread without ever waiting on
//! it (a child that stops reading its input cannot stall output), and publishes screen changes to every attached
//! client under the rules of [`crate::publisher`]: the 120 Hz cadence, the per-client Ack window with its forced
//! Snapshot and disconnect, the DEC 2026 hold, and idle-scrollback compression. Title changes and bells are coalesced
//! to the same cadence. OSC 7 updates the pane's directory (`pane.meta`); an OSC 52 clipboard write goes to every
//! attached client at once as CLIPBOARD_WRITE (the app sets the pasteboard; a write with nobody attached is dropped);
//! the process's exit is published after its last output, as C2 EXIT to the clients and `pane.status`/`pane.exit` to
//! C1.
//!
//! An agent pane's task also owns its [`Agent`]: it is prepared from the launch spec before the process spawns (so a
//! hook the new process fires at once already finds it), and the task feeds it the C3 envelopes other tasks hand in,
//! the OSC 9 bodies and first output byte the engine reports, every key typed (a KEY frame that encoded to bytes,
//! INPUT_RAW, a `pane.answer` digit) and the rollout tailer's batches, which it reads only when no command, exit,
//! input write, pty output or timer is ready, so JSON never delays the terminal. The task publishes the agent's state
//! when it adopts the process and acknowledges the adoption to whoever spawned it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ply_agents::LaunchSpec;
use ply_proto::data::{
    Exit, FetchHistory, Frame, History, KeyAction, KeyEvent, MAX_FRAME_LEN, Mods, Paste,
};
use ply_proto::hook::HookEnvelope;
use ply_proto::pane::{AgentCli, Cli, OptionAsMeta, PaneId, PaneStatus, Rgb, TaskId};
use ply_term::{
    ClipboardWrite, Compression, DeltaBuilder, Encoded, Engine, EngineOutput, Input, KEY_ENTER,
    Palette, Update, encode_input,
};
use rustix::process::Signal;
use tokio::sync::{mpsc, oneshot};

use crate::branch;
use crate::daemon::{Shared, unix_now};
use crate::osc::{decode_osc7, is_enter, osc9_bodies};
use crate::panes::agent::{Agent, Typed};
use crate::pty::{Geometry, PaneProcess, PtyChannels, PtyWrite};
use crate::publisher::{AckOutcome, Cadence, ClientWindow, IdleTimer, SyncHold};
use crate::tail::TailMsg;

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
        /// The id the connection attached with; an unknown id is ignored.
        client: u64,
    },
    /// A client frame after ATTACH: input, RESIZE, FETCH_HISTORY or ACK.
    Frame {
        /// The id the connection attached with; frames of a detached client are dropped.
        client: u64,
        /// A client kind other than ATTACH (the data server refuses the rest before this).
        frame: Frame,
        /// When the data server decoded it; a KEY's bytes carry it to the pty writer for the P2 probe.
        received: Instant,
    },
    /// Bytes for the pty as they are (`pane.answer` digits).
    Write(Vec<u8>),
    /// The pane's task queue changed (Ruling R60): look at it again once the pane has settled.
    Queue,
    /// `task.send`: type this queued task at the pane's next settled moment, over the user's unsent typing.
    SendNow(TaskId),
    /// A new palette from `theme.set`.
    SetPalette(Palette),
    /// A new `option_as_meta` setting.
    SetOptionAsMeta(OptionAsMeta),
    /// SIGHUP to the process group, SIGKILL after [`KILL_GRACE`] (Ruling R7).
    Kill,
    /// Stop an agent process at rest ([`Agent::may_reload`]) for a relaunch onto its CLI's update: the pane turns
    /// `starting`, and `stopped` is answered once the process exited and the terminal was reset, with no exit reported.
    /// A busy pane drops `stopped` at once; one closed meanwhile drops it at the exit, which it reports as usual.
    Reload(oneshot::Sender<()>),
    /// The process about to be spawned from this spec (`pane.resume`): prepares the agent integration for it.
    Launch(Box<LaunchSpec>),
    /// The spawn prepared by [`PaneCmd::Launch`] failed; the prepared agent integration is dropped.
    LaunchFailed,
    /// A new process for this pane (`pane.create`, `pane.resume`).
    Start(Box<Started>),
    /// A C3 envelope for this pane (hook or notify, spec 4.3).
    Hook(Box<HookEnvelope>),
    /// End the task: the pane was closed or plyd is stopping.
    Stop,
}

/// A spawned process with its pty channels, handed to a pane task.
#[derive(Debug)]
pub struct Started {
    /// The spawned child, the leader of its own process group, which `Kill` signals.
    pub process: PaneProcess,
    /// Its output, input and exit channels.
    pub channels: PtyChannels,
    /// Answered once the task has adopted the process and published the pane's state.
    pub adopted: Option<oneshot::Sender<()>>,
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
    /// The pane's id, which is also its C2 `pane_id` and its row in `panes`.
    pub id: PaneId,
    /// Its program, for the default title.
    pub cli: Cli,
    /// Title used while the terminal sets none.
    pub default_title: String,
    /// Its engine, already sized and given the palette.
    pub engine: Engine,
    /// The engine's current size.
    pub geometry: Geometry,
    /// The launch spec of the process about to be started with [`PaneCmd::Start`] (`pane.create`), if any.
    pub launch: Option<LaunchSpec>,
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
        agent: None,
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
        kill_pending: false,
        reloading: None,
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
    if let Some(spec) = seed.launch {
        task.prepare(&spec);
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
    chunks: VecDeque<PtyWrite>,
    bytes: usize,
}

impl WriteQueue {
    fn push(&mut self, pane_id: PaneId, bytes: Vec<u8>, key_at: Option<Instant>) {
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
        self.chunks.push_back(PtyWrite { bytes, key_at });
    }

    fn pop(&mut self) -> Option<PtyWrite> {
        let chunk = self.chunks.pop_front()?;
        self.bytes -= chunk.bytes.len();
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
    agent: Option<Agent>,
    id: PaneId,
    cli: Cli,
    default_title: String,
    shared: Arc<Shared>,
    engine: Engine,
    geometry: Geometry,
    clients: Vec<Client>,
    process: Option<PaneProcess>,
    output: Option<mpsc::Receiver<Vec<u8>>>,
    input: Option<mpsc::Sender<PtyWrite>>,
    exit: Option<oneshot::Receiver<i32>>,
    exit_code: Option<i32>,
    exit_reported: bool,
    exit_grace: Option<Instant>,
    kill_at: Option<Instant>,
    kill_pending: bool,
    reloading: Option<oneshot::Sender<()>>,
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

async fn reserve(tx: Option<mpsc::Sender<PtyWrite>>) -> Option<mpsc::OwnedPermit<PtyWrite>> {
    match tx {
        Some(tx) => tx.reserve_owned().await.ok(),
        None => std::future::pending().await,
    }
}

async fn next_tail(agent: &mut Option<Agent>) -> Option<TailMsg> {
    match agent {
        Some(agent) => agent.next_tail().await,
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
            let tailing = self.agent.as_ref().is_some_and(Agent::tailing);
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
                msg = next_tail(&mut self.agent), if tailing => {
                    if let (Some(msg), Some(agent)) = (msg, self.agent.as_mut()) {
                        agent.on_tail(&self.shared, msg, Instant::now());
                    }
                }
            }
            self.on_tick(Instant::now());
        }
        if self.kill_at.take().is_some() {
            self.kill_group();
        }
        tracing::debug!(pane_id = self.id, "pane task stopped");
    }

    /// Prepares the agent integration for the process about to start from `spec`; shells have none.
    fn prepare(&mut self, spec: &LaunchSpec) {
        if spec.cli != self.cli {
            self.cli = spec.cli;
            self.default_title = match spec.cli {
                Cli::Claude => "claude".to_owned(),
                Cli::Codex => "codex".to_owned(),
                Cli::Shell => spec
                    .argv
                    .first()
                    .and_then(|p| std::path::Path::new(p).file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("shell")
                    .to_owned(),
            };
        }
        let cli = match spec.cli {
            Cli::Claude => AgentCli::Claude,
            Cli::Codex => AgentCli::Codex,
            Cli::Shell => {
                self.agent = None;
                return;
            }
        };
        self.agent = Some(Agent::new(&self.shared, self.id, cli, spec));
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
        let (status, detail) = self
            .agent
            .as_ref()
            .map_or((PaneStatus::Idle, None), Agent::status);
        self.shared.set_status(self.id, status, detail);
        if let Some(adopted) = started.adopted
            && adopted.send(()).is_err()
        {
            tracing::debug!(pane_id = self.id, "nobody waited for the adoption");
        }
        let cwd = self
            .shared
            .registry()
            .entry(self.id)
            .map(|e| e.pane.cwd.clone());
        if let Some(cwd) = cwd {
            branch::lookup(&self.shared, self.id, cwd);
        }
        if std::mem::take(&mut self.kill_pending) {
            tracing::info!(
                pane_id = self.id,
                "stopping the resumed process the pane was closed for"
            );
            self.kill(Instant::now());
        }
    }

    fn key_typed(&mut self, enter: bool, now: Instant) {
        if let Some(agent) = self.agent.as_mut() {
            agent.on_key(&self.shared, enter, now);
        }
    }

    /// The user's own key or raw input, which the task queue must not type over.
    fn user_typed(&mut self, bytes: &[u8], now: Instant) {
        if let Some(agent) = self.agent.as_mut() {
            agent.on_user_input(&self.shared, bytes, now);
        }
    }

    /// Writes what the agent's dispatch decided to type: a queued task's paste, then its Enter as a real key press.
    fn type_queued(&mut self, now: Instant) {
        let Some(writes) = self.agent.as_mut().map(Agent::take_writes) else {
            return;
        };
        for write in writes {
            match write {
                Typed::Paste(text) => {
                    let paste = Paste {
                        allow_unsafe: false,
                        text,
                    };
                    match encode_input(&mut self.engine, Input::Paste(&paste)) {
                        Ok(Encoded::Bytes(bytes)) => {
                            tracing::debug!(
                                pane_id = self.id,
                                bytes = bytes.len(),
                                "pasting a queued task"
                            );
                            self.write(bytes);
                        }
                        Ok(Encoded::Nothing) => {}
                        Ok(Encoded::PasteRejected) => {
                            tracing::warn!(
                                pane_id = self.id,
                                "the terminal refused a queued task's paste"
                            );
                            if let Some(agent) = self.agent.as_mut() {
                                agent.paste_refused(&self.shared, now);
                            }
                        }
                        Err(e) => {
                            tracing::warn!(pane_id = self.id, error = %e, "cannot encode a queued task's paste");
                            if let Some(agent) = self.agent.as_mut() {
                                agent.paste_refused(&self.shared, now);
                            }
                        }
                    }
                }
                Typed::Enter => {
                    for action in [KeyAction::Press, KeyAction::Release] {
                        let enter = KeyEvent {
                            key: KEY_ENTER,
                            mods: Mods::empty(),
                            consumed_mods: Mods::empty(),
                            action,
                            composing: false,
                            unshifted_codepoint: 0,
                            text: String::new(),
                        };
                        match encode_input(&mut self.engine, Input::Key(&enter)) {
                            Ok(Encoded::Bytes(bytes)) => {
                                tracing::debug!(
                                    pane_id = self.id,
                                    ?bytes,
                                    "pressing Enter for a queued task"
                                );
                                self.write(bytes);
                            }
                            Ok(Encoded::Nothing | Encoded::PasteRejected) => {}
                            Err(e) => {
                                tracing::warn!(pane_id = self.id, error = %e, "cannot encode Enter for a queued task");
                            }
                        }
                    }
                    self.key_typed(true, now);
                }
            }
        }
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
            PaneCmd::Frame {
                client,
                frame,
                received,
            } => self.on_frame(client, frame, received, now),
            PaneCmd::Write(bytes) => {
                let enter = is_enter(&bytes);
                self.write(bytes);
                self.key_typed(enter, now);
            }
            PaneCmd::Queue => {
                if let Some(agent) = self.agent.as_mut() {
                    agent.nudge(&self.shared, now);
                }
            }
            PaneCmd::SendNow(task) => match self.agent.as_mut() {
                Some(agent) => agent.send_now(&self.shared, task, now),
                None => tracing::warn!(
                    pane_id = self.id,
                    task_id = task,
                    "task.send for a pane without an agent ignored"
                ),
            },
            PaneCmd::SetPalette(palette) => {
                if let Err(e) = self.engine.set_palette(&palette) {
                    tracing::warn!(pane_id = self.id, error = %e, "cannot apply the new palette");
                }
            }
            PaneCmd::SetOptionAsMeta(option) => self.engine.set_option_as_meta(option),
            PaneCmd::Kill => self.kill(now),
            PaneCmd::Reload(stopped) => self.reload(stopped, now),
            PaneCmd::Launch(spec) => self.prepare(&spec),
            PaneCmd::LaunchFailed => {
                self.agent = None;
                self.kill_pending = false;
            }
            PaneCmd::Start(started) => {
                self.adopt(*started);
                tracing::info!(pane_id = self.id, "pane process adopted");
            }
            PaneCmd::Hook(envelope) => match self.agent.as_mut() {
                Some(agent) if self.exit_code.is_none() => {
                    agent.on_hook(&self.shared, &envelope, now);
                }
                _ => {
                    tracing::debug!(pane_id = self.id, event = ?envelope.event, "hook for a pane without a running agent dropped");
                }
            },
            PaneCmd::Stop => self.stop = true,
        }
    }

    fn reload(&mut self, stopped: oneshot::Sender<()>, now: Instant) {
        let at_rest = self.process.is_some()
            && self.exit_code.is_none()
            && self.reloading.is_none()
            && self
                .agent
                .as_ref()
                .is_some_and(|a| a.may_reload(&self.shared, now));
        if !at_rest {
            tracing::info!(
                pane_id = self.id,
                "the pane is busy or has no session to resume; its process stays on the CLI version it runs"
            );
            return;
        }
        if let Some(mut agent) = self.agent.take() {
            agent.exit(&self.shared, now);
        }
        tracing::info!(
            pane_id = self.id,
            "stopping the pane's process to resume it on its CLI's update"
        );
        self.shared.set_status(self.id, PaneStatus::Starting, None);
        self.reloading = Some(stopped);
        self.kill(now);
    }

    /// The reloading process exited: what is left of its group is killed, its pty dropped and the terminal reset (RIS), so the resumed CLI starts as on a new terminal.
    fn finish_reload(&mut self, stopped: oneshot::Sender<()>, now: Instant) {
        if self.kill_at.take().is_some() {
            self.kill_group();
        }
        self.process = None;
        self.output = None;
        self.input = None;
        self.writes.clear();
        self.exit_code = None;
        let out = self.engine.write(b"\x1bc");
        self.on_effects(out);
        self.after_change(now);
        self.publish(now);
        if stopped.send(()).is_err() {
            tracing::warn!(
                pane_id = self.id,
                "nobody waits to resume the reloaded pane; it stays starting"
            );
        }
    }

    fn kill(&mut self, now: Instant) {
        if self.exit_code.is_some() {
            return;
        }
        let Some(process) = &self.process else {
            // A `pane.resume` is starting the process this close is meant for; adopt() applies the kill.
            self.kill_pending = true;
            tracing::debug!(
                pane_id = self.id,
                "kill before the process is adopted; kept for it"
            );
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
        self.write_key(bytes, None);
    }

    /// Queues `bytes` for the pty; `key_at` is set for a KEY frame's bytes (the P2 probe).
    fn write_key(&mut self, bytes: Vec<u8>, key_at: Option<Instant>) {
        if self.input.is_none() {
            tracing::debug!(
                pane_id = self.id,
                bytes = bytes.len(),
                "input for a pane without a process dropped"
            );
            return;
        }
        self.writes.push(self.id, bytes, key_at);
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

    fn on_frame(&mut self, client_id: u64, frame: Frame, received: Instant, now: Instant) {
        match frame {
            Frame::InputRaw(bytes) => {
                let enter = is_enter(&bytes);
                self.user_typed(&bytes, now);
                self.write(bytes);
                self.key_typed(enter, now);
            }
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
                    let built = client.builder.fetch_history(&mut self.engine, &f);
                    let history = history_answer(self.id, client_id, &f, built);
                    send(self.id, client, &Frame::History(history));
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
                let key = matches!(frame, Frame::Key(_));
                let paste = matches!(frame, Frame::Paste(_));
                match encode_input(&mut self.engine, input) {
                    Ok(Encoded::Bytes(bytes)) => {
                        let enter = is_enter(&bytes);
                        if key {
                            self.user_typed(&bytes, now);
                        } else if paste && let Some(agent) = self.agent.as_mut() {
                            agent.on_user_paste(&self.shared, now);
                        }
                        self.write_key(bytes, key.then_some(received));
                        if key {
                            self.key_typed(enter, now);
                        }
                    }
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
        if let Some(agent) = self.agent.as_mut() {
            agent.on_output(&self.shared, now);
        }
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
                Some(cwd) => {
                    if self.shared.registry().set_cwd(self.id, &cwd) {
                        branch::lookup(&self.shared, self.id, cwd);
                    }
                }
                None => {
                    tracing::debug!(pane_id = self.id, uri = %uri, "OSC 7 without a local file URI ignored");
                }
            }
        }
        for n in &out.notifications {
            tracing::debug!(pane_id = self.id, cli = ?self.cli, title = %n.title, body = %n.body, "desktop notification");
        }
        if let Some(agent) = self.agent.as_mut() {
            let now = Instant::now();
            for body in osc9_bodies(&out.notifications) {
                agent.on_osc9(&self.shared, body, now);
            }
        }
        for write in &out.clipboard_writes {
            self.forward_clipboard(write);
        }
    }

    fn forward_clipboard(&mut self, write: &ClipboardWrite) {
        let Some(text) = clipboard_text(write) else {
            tracing::debug!(
                pane_id = self.id,
                mimes = ?write.contents.iter().map(|c| c.mime.as_str()).collect::<Vec<_>>(),
                "OSC 52 write without UTF-8 text/plain ignored"
            );
            return;
        };
        if text.len() > MAX_FRAME_LEN {
            tracing::warn!(
                pane_id = self.id,
                bytes = text.len(),
                "OSC 52 write larger than one C2 frame dropped"
            );
            return;
        }
        if self.clients.is_empty() {
            tracing::debug!(
                pane_id = self.id,
                "OSC 52 write with no client attached dropped"
            );
            return;
        }
        let frame = Frame::ClipboardWrite(text);
        for client in &mut self.clients {
            send(self.id, client, &frame);
        }
        tracing::debug!(
            pane_id = self.id,
            clients = self.clients.len(),
            "OSC 52 clipboard write forwarded"
        );
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
        if let Some(stopped) = self.reloading.take()
            && !self.shared.registry().closes_on_exit(self.id)
        {
            tracing::info!(pane_id = self.id, code, "the reloading process exited");
            self.finish_reload(stopped, now);
            return;
        }
        self.after_change(now);
        self.publish(now);
        for client in &mut self.clients {
            send(self.id, client, &Frame::Exit(Exit { code }));
        }
        self.input = None;
        self.writes.clear();
        if let Some(agent) = self.agent.as_mut() {
            agent.exit(&self.shared, Instant::now());
        }
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
            self.agent.as_ref().and_then(Agent::deadline),
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
        if let Some(agent) = self.agent.as_mut() {
            agent.on_tick(&self.shared, now);
        }
        self.type_queued(now);
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

/// The HISTORY answering `request`: the page, or an empty one at its `start` when it could not be built, so the client stops waiting for it (it marks that start exhausted until its next Snapshot).
fn history_answer(
    pane_id: PaneId,
    client: u64,
    request: &FetchHistory,
    built: ply_term::Result<History>,
) -> History {
    built.unwrap_or_else(|e| {
        tracing::warn!(pane_id, client, start = request.start, error = %e, "cannot read the scrollback; answering an empty page");
        History {
            start: request.start,
            lines: Vec::new(),
            styles_added: Vec::new(),
        }
    })
}

/// The text an OSC 52 write sets: its `text/plain` part as UTF-8, or empty for a write that clears the clipboard.
fn clipboard_text(write: &ClipboardWrite) -> Option<String> {
    if write.contents.is_empty() {
        return Some(String::new());
    }
    let part = write
        .contents
        .iter()
        .find(|c| c.mime == "text/plain" || c.mime.starts_with("text/plain;"))?;
    String::from_utf8(part.data.clone()).ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clipboard_write_forwards_its_utf8_text_and_a_clear_as_empty() {
        let part = |mime: &str, data: &[u8]| ply_term::ClipboardContent {
            mime: mime.to_owned(),
            data: data.to_vec(),
        };
        let write = |contents| ClipboardWrite { contents };
        assert_eq!(
            clipboard_text(&write(vec![
                part("image/png", b"\x89PNG"),
                part("text/plain", b"hi")
            ])),
            Some("hi".to_owned())
        );
        assert_eq!(
            clipboard_text(&write(vec![part(
                "text/plain;charset=utf-8",
                "ü".as_bytes()
            )])),
            Some("ü".to_owned())
        );
        assert_eq!(clipboard_text(&write(vec![])), Some(String::new()));
        assert_eq!(
            clipboard_text(&write(vec![part("text/plain", &[0xff])])),
            None
        );
        assert_eq!(clipboard_text(&write(vec![part("image/png", b"x")])), None);
    }

    #[test]
    fn a_history_page_that_cannot_be_built_is_answered_empty_at_its_start() {
        let request = FetchHistory {
            start: 1_000,
            count: 50,
        };
        let failed = Err(ply_term::Error::Ghostty {
            call: "ghostty_terminal_grid_ref",
            code: -4,
        });
        assert_eq!(
            history_answer(1, 2, &request, failed),
            History {
                start: 1_000,
                lines: Vec::new(),
                styles_added: Vec::new()
            }
        );
        let page = History {
            start: 1_010,
            lines: Vec::new(),
            styles_added: Vec::new(),
        };
        assert_eq!(history_answer(1, 2, &request, Ok(page.clone())), page);
    }

    #[test]
    fn the_write_queue_caps_pending_input() {
        let mut q = WriteQueue::default();
        q.push(1, vec![0; MAX_PENDING_INPUT], None);
        q.push(1, vec![1], None);
        assert_eq!(q.chunks.len(), 1, "input beyond the cap is dropped");
        assert_eq!(q.pop().map(|c| c.bytes.len()), Some(MAX_PENDING_INPUT));
        assert!(q.is_empty());
        q.push(1, vec![2], Some(Instant::now()));
        assert_eq!(q.bytes, 1);
    }
}
