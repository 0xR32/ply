//! C4: following a Codex pane's rollout file (spec 3.3 C4, 6.2 discovery, Rulings R28, R48, R49).
//!
//! Codex writes one rollout per thread, `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<thread_uuid>.jsonl`, one
//! `{timestamp, ordinal?, type, payload}` record per line. A [`Tailer`] runs on its own std thread for as long as the
//! pane's Codex process lives. Until it is bound to a file it looks for one: a file named after a thread it was asked
//! for (the resumed thread, or the `thread-id` of a notify, R28), else the newest rollout created after the spawn whose
//! `session_meta.cwd` is the pane's directory and that no other pane follows. Bound, it reads the file from the start
//! by offset. It switches to another file when a notify names another thread whose rollout exists, or when, within
//! [`REDISCOVER_WINDOW`] of an Enter the pane asked about ([`Tailer::rediscover`]), a new unclaimed rollout of the
//! pane's directory appears: that is `/new` or `/clear` starting a thread (R49). A file that shrinks or is replaced is
//! read again from the start.
//!
//! Lines go to the pane's agent on a channel of [`TAIL_QUEUE`] messages that the pane task reads last, after its pty
//! output, commands and timers (I4): [`TailMsg::Switched`] when another file starts, then [`TailMsg::Lines`] batches
//! of at most [`LINES_PER_BATCH`] lines and [`MAX_BATCH_BYTES`], holding only the lines the session reads
//! ([`ply_agents::codex::rollout::plyd_reads`], checked on this thread so the pane task never parses the rest). The
//! lines a file already held when it was bound, up to the first end of file, are [`TailMsg::History`] instead when the
//! file is the resumed thread's or was created before the process, and so are those of a file read again from its
//! start: they are the thread's past, whose turns must not replay through the status machine.
//!
//! It wakes on filesystem events for `sessions/` (FSEvents through `notify`, once the directory exists; plyd never
//! creates it; a failed watch is retried every [`WATCH_RETRY`]) and polls besides ([`POLL`] until it is bound or
//! watching, [`IDLE_POLL`] after), so a missed event only delays a record. Requests from the pane task sit in a shared
//! list, never in the wake queue, so a burst of events cannot drop them. It reads files and never writes anything under
//! `$CODEX_HOME` (INV-8). Lines over [`ply_agents::codex::rollout::MAX_LINE_BYTES`] are skipped and logged; unknown
//! record types are the session's to count.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ply_agents::codex::rollout::{
    LineBuffer, MAX_LINE_BYTES, RolloutCandidate, RolloutRecord, parse_record, pick_rollout_by_cwd,
    plyd_reads, rollout_thread_id,
};
use ply_proto::pane::PaneId;
use tokio::sync::mpsc;

use crate::daemon::Shared;
use crate::error::{Error, Result};

/// Poll interval while the tailer has neither a file nor a watcher.
pub const POLL: Duration = Duration::from_millis(250);

/// Poll interval once filesystem events wake the tailer.
pub const IDLE_POLL: Duration = Duration::from_secs(1);

/// Bytes read from the rollout per read call.
pub const READ_CHUNK: usize = 64 * 1024;

/// Most lines in one [`TailMsg::Lines`].
pub const LINES_PER_BATCH: usize = 256;

/// Most line bytes in one [`TailMsg::Lines`].
pub const MAX_BATCH_BYTES: usize = 1 << 20;

/// Messages queued towards the pane's agent; the tailer thread waits when they are all unread.
pub const TAIL_QUEUE: usize = 4;

/// How long after an Enter a new rollout of the pane's directory counts as the pane's new thread (R49).
pub const REDISCOVER_WINDOW: Duration = Duration::from_secs(5);

/// Interval between attempts to watch `sessions/` after one failed.
pub const WATCH_RETRY: Duration = Duration::from_secs(30);

/// Threads a notify asked for that the tailer remembers; the oldest goes first.
const MAX_WANTED: usize = 8;

/// Day directories searched for new rollouts, newest first; a spawn just before midnight needs two.
const RECENT_DAYS: usize = 2;

/// How much earlier than the spawn (or an Enter) a rollout may claim to be created, for coarse birth times.
const SPAWN_SLACK: Duration = Duration::from_secs(1);

/// Shortest interval between two full walks of `sessions/` for a resumed thread's older file.
const WALK_INTERVAL: Duration = Duration::from_secs(5);

/// What a Codex pane's tailer needs to find its rollout.
#[derive(Debug, Clone)]
pub struct TailOptions {
    /// The pane, for log fields.
    pub pane_id: PaneId,
    /// The `CODEX_HOME` the pane's process uses (its `CODEX_HOME`, else `$HOME/.codex`).
    pub codex_home: PathBuf,
    /// The directory Codex was started in.
    pub cwd: String,
    /// When the process was spawned; rollouts created before it belong to other sessions.
    pub spawned_at: SystemTime,
    /// The thread the pane resumed, if any: until it switches threads, only that thread's rollout is followed.
    pub thread: Option<String>,
}

/// What the tailer hands the pane's agent, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailMsg {
    /// The tailer started following a file; its `session_meta` may rebind the session to another thread (R49).
    Switched,
    /// Complete lines the session reads, without newlines, in file order.
    Lines(Vec<Vec<u8>>),
    /// Like [`TailMsg::Lines`], but written before this process (see the module docs): they start and end no turn.
    History(Vec<Vec<u8>>),
}

#[derive(Debug, Default)]
struct Requests {
    wanted: Vec<String>,
    since: Option<(SystemTime, Instant)>,
}

/// The pane task's handle on its tailer thread; dropping it stops the thread within one poll interval.
#[derive(Debug)]
pub struct Tailer {
    pane_id: PaneId,
    requests: Arc<Mutex<Requests>>,
    wake: SyncSender<()>,
    stop: Arc<AtomicBool>,
}

impl Tailer {
    /// Starts the tailer thread and returns it with the channel its lines arrive on; errors: [`Error::Io`] if no thread starts.
    pub fn spawn(
        shared: Arc<Shared>,
        options: TailOptions,
    ) -> Result<(Self, mpsc::Receiver<TailMsg>)> {
        let (wake, woken) = sync_channel(1);
        let (out, lines) = mpsc::channel(TAIL_QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Requests::default()));
        let state = TailState {
            sessions: options.codex_home.join("sessions"),
            cwd: canonical(&options.cwd),
            spawned_at: options
                .spawned_at
                .checked_sub(SPAWN_SLACK)
                .unwrap_or(options.spawned_at),
            started_at: options.spawned_at,
            resumed: options.thread,
            wanted: Vec::new(),
            since: None,
            requests: Arc::clone(&requests),
            first_lines: HashMap::new(),
            last_walk: None,
            bound: None,
            pane_id: options.pane_id,
            shared,
            out,
        };
        let wake_for_watcher = wake.clone();
        let flag = Arc::clone(&stop);
        std::thread::Builder::new()
            .name(format!("rollout-{}", options.pane_id))
            .spawn(move || state.run(&woken, &wake_for_watcher, &flag))
            .map_err(|source| Error::Io {
                what: "cannot start the rollout tailer for",
                path: options.codex_home,
                source,
            })?;
        let tailer = Self {
            pane_id: options.pane_id,
            requests,
            wake,
            stop,
        };
        Ok((tailer, lines))
    }

    /// Asks the tailer to follow `rollout-*-<thread_id>.jsonl` once it exists (a notify's thread, R28, R49).
    pub fn find(&self, thread_id: String) {
        self.request(|r| {
            if !r.wanted.contains(&thread_id) {
                if r.wanted.len() == MAX_WANTED {
                    r.wanted.remove(0);
                }
                r.wanted.push(thread_id);
            }
        });
    }

    /// Asks the tailer to switch to a new rollout of the pane's directory created from `since` on, for a while (R49).
    pub fn rediscover(&self, since: SystemTime) {
        self.request(|r| r.since = Some((since, Instant::now())));
    }

    fn request(&self, change: impl FnOnce(&mut Requests)) {
        change(&mut self.requests.lock().unwrap_or_else(PoisonError::into_inner));
        match self.wake.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => {
                tracing::debug!(pane_id = self.pane_id, "the rollout tailer has stopped");
            }
        }
    }
}

impl Drop for Tailer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Err(TrySendError::Disconnected(())) = self.wake.try_send(()) {
            tracing::trace!(
                pane_id = self.pane_id,
                "the rollout tailer had already stopped"
            );
        }
    }
}

struct Bound {
    path: PathBuf,
    file: File,
    offset: u64,
    lines: LineBuffer,
    history: bool,
}

struct TailState {
    sessions: PathBuf,
    cwd: String,
    spawned_at: SystemTime,
    started_at: SystemTime,
    resumed: Option<String>,
    wanted: Vec<String>,
    since: Option<(SystemTime, Instant)>,
    requests: Arc<Mutex<Requests>>,
    first_lines: HashMap<PathBuf, Option<String>>,
    last_walk: Option<Instant>,
    bound: Option<Bound>,
    pane_id: PaneId,
    shared: Arc<Shared>,
    out: mpsc::Sender<TailMsg>,
}

impl TailState {
    fn run(mut self, woken: &Receiver<()>, wake: &SyncSender<()>, stop: &AtomicBool) {
        let pane_id = self.pane_id;
        tracing::debug!(pane_id, sessions = %self.sessions.display(), "rollout tailer started");
        let mut watcher: Option<RecommendedWatcher> = None;
        let mut watch_failed: Option<Instant> = None;
        while !stop.load(Ordering::Relaxed) {
            if watcher.is_none()
                && watch_failed.is_none_or(|t| t.elapsed() >= WATCH_RETRY)
                && self.sessions.is_dir()
            {
                watcher = watch(
                    pane_id,
                    &self.sessions,
                    wake.clone(),
                    watch_failed.is_none(),
                );
                watch_failed = watcher.is_none().then(Instant::now);
            }
            self.take_requests();
            let next = match self.bound {
                None => self.discover(),
                Some(_) => self.newer_file(),
            };
            if let Some(path) = next
                && !self.follow(path)
            {
                break;
            }
            self.reopen_if_replaced();
            if !self.pump() {
                break;
            }
            let wait = if watcher.is_some() && self.bound.is_some() {
                IDLE_POLL
            } else {
                POLL
            };
            match woken.recv_timeout(wait) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if let Some(bound) = self.bound.take() {
            self.shared.release_rollout(&bound.path);
        }
        tracing::debug!(pane_id, "rollout tailer stopped");
    }

    fn take_requests(&mut self) {
        let mut requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
        for thread in requests.wanted.drain(..) {
            if !self.wanted.contains(&thread) {
                if self.wanted.len() == MAX_WANTED {
                    self.wanted.remove(0);
                }
                self.wanted.push(thread);
            }
        }
        if let Some(since) = requests.since.take() {
            self.since = Some(since);
        }
    }

    /// Starts following `path` (claimed for this pane) from its first byte; false once the pane's agent is gone.
    fn follow(&mut self, path: PathBuf) -> bool {
        if !self.shared.claim_rollout(&path) {
            return true;
        }
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!(pane_id = self.pane_id, rollout = %path.display(), error = %e, "cannot open the rollout");
                self.shared.release_rollout(&path);
                return true;
            }
        };
        let thread = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(rollout_thread_id)
            .map(str::to_owned);
        let history = (thread.is_some() && thread == self.resumed) || self.predates_spawn(&path);
        if let Some(old) = self.bound.take() {
            tracing::info!(pane_id = self.pane_id, from = %old.path.display(), to = %path.display(), "Codex started another thread; following its rollout");
            self.shared.release_rollout(&old.path);
            self.resumed = None;
        } else {
            tracing::info!(pane_id = self.pane_id, rollout = %path.display(), history, "following the Codex rollout");
        }
        if let Some(thread) = &thread {
            self.wanted.retain(|t| t != thread);
        }
        self.since = None;
        self.bound = Some(Bound {
            path,
            file,
            offset: 0,
            lines: LineBuffer::default(),
            history,
        });
        self.send(TailMsg::Switched)
    }

    /// Whether `path` was created before the pane's process started, so what it holds now is the thread's past.
    fn predates_spawn(&self, path: &Path) -> bool {
        match std::fs::metadata(path).and_then(|m| m.created().or(m.modified())) {
            Ok(created) => created < self.started_at,
            Err(e) => {
                tracing::debug!(pane_id = self.pane_id, rollout = %path.display(), error = %e, "cannot stat the rollout; reading all of it as new");
                false
            }
        }
    }

    fn discover(&mut self) -> Option<PathBuf> {
        let recent = rollouts_in(
            self.pane_id,
            &recent_day_dirs(self.pane_id, &self.sessions, RECENT_DAYS),
        );
        if let Some(thread) = self.resumed.clone() {
            let threads = [thread];
            if let Some(path) = named(&threads, &recent) {
                return Some(path);
            }
            if self.last_walk.is_none_or(|t| t.elapsed() >= WALK_INTERVAL) {
                self.last_walk = Some(Instant::now());
                let all = all_day_dirs(self.pane_id, &self.sessions);
                return named(&threads, &rollouts_in(self.pane_id, &all));
            }
            return None;
        }
        if let Some(path) = named(&self.wanted, &recent) {
            return Some(path);
        }
        self.newest_by_cwd(&recent, self.spawned_at)
    }

    /// While bound: a notified thread's file, or a new file of the pane's directory shortly after an Enter (R49).
    fn newer_file(&mut self) -> Option<PathBuf> {
        let bound = self.bound.as_ref().map(|b| b.path.clone())?;
        let expired = self
            .since
            .is_some_and(|(_, asked)| asked.elapsed() >= REDISCOVER_WINDOW);
        if expired {
            self.since = None;
        }
        if self.wanted.is_empty() && self.since.is_none() {
            return None;
        }
        let recent: Vec<(PathBuf, String)> = rollouts_in(
            self.pane_id,
            &recent_day_dirs(self.pane_id, &self.sessions, RECENT_DAYS),
        )
        .into_iter()
        .filter(|(path, _)| *path != bound)
        .collect();
        if let Some(path) = named(&self.wanted, &recent) {
            return Some(path);
        }
        let (since, _) = self.since?;
        let since = since.checked_sub(SPAWN_SLACK).unwrap_or(since);
        self.newest_by_cwd(&recent, since)
    }

    fn newest_by_cwd(&mut self, files: &[(PathBuf, String)], after: SystemTime) -> Option<PathBuf> {
        let mut candidates = Vec::new();
        for (path, _) in files {
            let created = match std::fs::metadata(path).and_then(|m| m.created().or(m.modified())) {
                Ok(created) => created,
                Err(e) => {
                    tracing::debug!(pane_id = self.pane_id, rollout = %path.display(), error = %e, "cannot stat a rollout");
                    continue;
                }
            };
            if created < after || self.shared.rollout_claimed(path) {
                continue;
            }
            let Some(cwd) = self.first_line_cwd(path) else {
                continue;
            };
            candidates.push(RolloutCandidate {
                path: path.clone(),
                created,
                cwd,
            });
        }
        pick_rollout_by_cwd(&candidates, after, &self.cwd).map(|c| c.path.clone())
    }

    /// The canonical `session_meta.cwd` of the file's first line; files without a complete first line are retried later.
    fn first_line_cwd(&mut self, path: &Path) -> Option<String> {
        if let Some(known) = self.first_lines.get(path) {
            return known.clone();
        }
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) => {
                tracing::debug!(pane_id = self.pane_id, rollout = %path.display(), error = %e, "cannot open a rollout");
                return None;
            }
        };
        let mut line = Vec::new();
        let limit = u64::try_from(MAX_LINE_BYTES).unwrap_or(u64::MAX);
        if let Err(e) = BufReader::new(file)
            .take(limit)
            .read_until(b'\n', &mut line)
        {
            tracing::debug!(pane_id = self.pane_id, rollout = %path.display(), error = %e, "cannot read a rollout");
            return None;
        }
        if line.pop() != Some(b'\n') {
            return None;
        }
        let cwd = match parse_record(&line) {
            Ok(RolloutRecord::SessionMeta(meta)) => Some(canonical(&meta.cwd)),
            Ok(_) => None,
            Err(e) => {
                tracing::debug!(pane_id = self.pane_id, rollout = %path.display(), error = %e, "a rollout without a readable session_meta");
                None
            }
        };
        self.first_lines.insert(path.to_path_buf(), cwd.clone());
        cwd
    }

    /// Reads the bound file from its start again when it shrank below what was read or another file replaced it.
    fn reopen_if_replaced(&mut self) {
        let Some(bound) = self.bound.as_mut() else {
            return;
        };
        let now = match std::fs::metadata(&bound.path) {
            Ok(meta) => meta,
            Err(e) => {
                tracing::debug!(pane_id = self.pane_id, rollout = %bound.path.display(), error = %e, "cannot stat the followed rollout");
                return;
            }
        };
        let replaced = bound
            .file
            .metadata()
            .is_ok_and(|open| open.ino() != now.ino());
        if !replaced && now.len() >= bound.offset {
            return;
        }
        match File::open(&bound.path) {
            Ok(file) => {
                tracing::info!(pane_id = self.pane_id, rollout = %bound.path.display(), replaced, "the rollout was truncated or replaced; reading it again");
                bound.file = file;
                bound.offset = 0;
                bound.lines = LineBuffer::default();
                bound.history = true;
            }
            Err(e) => {
                tracing::warn!(pane_id = self.pane_id, rollout = %bound.path.display(), error = %e, "cannot reopen the rollout");
            }
        }
    }

    /// Hands every complete new line the session reads to the pane's agent, in batches, history ending at the first end of file; false once the agent is gone.
    fn pump(&mut self) -> bool {
        let Some(bound) = self.bound.as_mut() else {
            return true;
        };
        let pane_id = self.pane_id;
        let mut buf = vec![0u8; READ_CHUNK];
        let mut batch = Vec::new();
        let mut bytes = 0;
        let message = |history: bool, lines| {
            if history {
                TailMsg::History(lines)
            } else {
                TailMsg::Lines(lines)
            }
        };
        let mut at_end = false;
        loop {
            match bound.file.read(&mut buf) {
                Ok(0) => {
                    at_end = true;
                    break;
                }
                Ok(n) => {
                    bound.offset += n as u64;
                    for framed in bound.lines.push(&buf[..n]) {
                        match framed.into_line() {
                            Ok(line) if plyd_reads(&line) => {
                                bytes += line.len();
                                batch.push(line);
                            }
                            Ok(_) => {}
                            Err(e) => {
                                tracing::warn!(pane_id, error = %e, "rollout line skipped");
                            }
                        }
                        if batch.len() >= LINES_PER_BATCH || bytes >= MAX_BATCH_BYTES {
                            bytes = 0;
                            let full = message(bound.history, std::mem::take(&mut batch));
                            if self.out.blocking_send(full).is_err() {
                                return false;
                            }
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => {
                    tracing::warn!(pane_id, rollout = %bound.path.display(), error = %e, "cannot read the rollout");
                    break;
                }
            }
        }
        let history = bound.history;
        if at_end && history {
            bound.history = false;
            tracing::debug!(pane_id, rollout = %bound.path.display(), "read the thread's past; following its new lines");
        }
        batch.is_empty() || self.send(message(history, batch))
    }

    fn send(&self, msg: TailMsg) -> bool {
        self.out.blocking_send(msg).is_ok()
    }
}

/// The first file in `files` whose thread is one of `threads`.
fn named(threads: &[String], files: &[(PathBuf, String)]) -> Option<PathBuf> {
    files
        .iter()
        .find(|(_, thread)| threads.contains(thread))
        .map(|(path, _)| path.clone())
}

fn watch(
    pane_id: PaneId,
    sessions: &Path,
    wake: SyncSender<()>,
    first: bool,
) -> Option<RecommendedWatcher> {
    let handler = move |event: notify::Result<notify::Event>| {
        if let Err(e) = event {
            tracing::debug!(pane_id, error = %e, "rollout watcher error");
        }
        match wake.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => {
                tracing::trace!(pane_id, "the rollout tailer has stopped");
            }
        }
    };
    let failed = |error: &dyn std::fmt::Display| {
        if first {
            tracing::warn!(pane_id, sessions = %sessions.display(), error = %error, "cannot watch Codex's sessions; polling and retrying every 30 s");
        } else {
            tracing::debug!(pane_id, sessions = %sessions.display(), error = %error, "still cannot watch Codex's sessions");
        }
    };
    let mut watcher = match notify::recommended_watcher(handler) {
        Ok(watcher) => watcher,
        Err(e) => {
            failed(&e);
            return None;
        }
    };
    match watcher.watch(sessions, RecursiveMode::Recursive) {
        Ok(()) => Some(watcher),
        Err(e) => {
            failed(&e);
            None
        }
    }
}

/// `path` with symlinks resolved (`/var` is `/private/var` on macOS, and Codex records the resolved directory).
fn canonical(path: &str) -> String {
    std::fs::canonicalize(path)
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| path.to_owned())
}

fn read_dir_logged(pane_id: PaneId, dir: &Path) -> Option<std::fs::ReadDir> {
    match std::fs::read_dir(dir) {
        Ok(entries) => Some(entries),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => {
            tracing::debug!(pane_id, dir = %dir.display(), error = %e, "cannot list a Codex sessions directory");
            None
        }
    }
}

fn numbered_dirs_desc(pane_id: PaneId, dir: &Path) -> Vec<PathBuf> {
    let Some(entries) = read_dir_logged(pane_id, dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort_unstable_by(|a, b| b.cmp(a));
    dirs
}

/// The newest `limit` `YYYY/MM/DD` directories under `sessions`, newest first.
fn recent_day_dirs(pane_id: PaneId, sessions: &Path, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for year in numbered_dirs_desc(pane_id, sessions) {
        for month in numbered_dirs_desc(pane_id, &year) {
            for day in numbered_dirs_desc(pane_id, &month) {
                out.push(day);
                if out.len() == limit {
                    return out;
                }
            }
        }
    }
    out
}

fn all_day_dirs(pane_id: PaneId, sessions: &Path) -> Vec<PathBuf> {
    recent_day_dirs(pane_id, sessions, usize::MAX)
}

/// The rollout files in `dirs` with their thread ids, in directory order.
fn rollouts_in(pane_id: PaneId, dirs: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for dir in dirs {
        let Some(entries) = read_dir_logged(pane_id, dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            if let Some(thread) = name.to_str().and_then(rollout_thread_id) {
                out.push((entry.path(), thread.to_owned()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_directories_are_found_newest_first_across_months_and_years() {
        let root = std::env::temp_dir().join(format!("ply-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["2025/12/31", "2026/01/01", "2026/01/02", "2026/02/03"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::create_dir_all(root.join("archive")).unwrap();
        let recent = recent_day_dirs(1, &root, 2);
        assert_eq!(recent, [root.join("2026/02/03"), root.join("2026/01/02")]);
        assert_eq!(all_day_dirs(1, &root).len(), 4);
        let id = "00000000-0000-7000-8000-000000000001";
        std::fs::write(
            root.join(format!("2025/12/31/rollout-2025-12-31T23-59-59-{id}.jsonl")),
            "",
        )
        .unwrap();
        std::fs::write(root.join("2025/12/31/notes.txt"), "").unwrap();
        let files = rollouts_in(1, &all_day_dirs(1, &root));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1, id);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
