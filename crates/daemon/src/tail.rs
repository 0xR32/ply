//! C4: following a Codex pane's rollout file (spec 3.3 C4, 6.2 discovery, Ruling R28).
//!
//! Codex writes one rollout per thread, `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<thread_uuid>.jsonl`, one
//! `{timestamp, ordinal?, type, payload}` record per line. A [`Tailer`] runs on its own std thread for as long as the
//! pane's Codex process lives. Until it is bound to a file it looks for one: a file named after a thread it was asked
//! for (the resumed thread, or the `thread-id` of a notify, R28), else the newest rollout created after the spawn whose
//! `session_meta.cwd` is the pane's directory and that no other pane follows. Bound, it reads the file from the start
//! by offset and hands every complete line to the pane task as [`PaneCmd::Rollout`]; the pane's `CodexSession` binds
//! itself to the first `session_meta` it is fed, so it only ever sees one thread's records. A resumed pane binds only
//! to its own thread's file, found anywhere under `sessions/`.
//!
//! It wakes on filesystem events for `sessions/` (FSEvents through `notify`, once the directory exists; plyd never
//! creates it) and polls besides ([`POLL`] until it is bound or watching, [`IDLE_POLL`] after), so a missed event
//! only delays a record. It reads files and never writes anything under `$CODEX_HOME` (INV-8). Lines over
//! [`ply_agents::codex::rollout::MAX_LINE_BYTES`] are skipped and logged; unknown record types are the session's to count.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant, SystemTime};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ply_agents::codex::rollout::{
    LineBuffer, MAX_LINE_BYTES, RolloutCandidate, RolloutRecord, parse_record, pick_rollout_by_cwd,
    rollout_thread_id,
};
use ply_proto::pane::PaneId;
use tokio::sync::mpsc::WeakSender;

use crate::daemon::Shared;
use crate::error::{Error, Result};
use crate::panes::pane::PaneCmd;

/// Poll interval while the tailer has neither a file nor a watcher.
pub const POLL: Duration = Duration::from_millis(250);

/// Poll interval once filesystem events wake the tailer.
pub const IDLE_POLL: Duration = Duration::from_secs(1);

/// Bytes read from the rollout per read call.
pub const READ_CHUNK: usize = 64 * 1024;

/// Lines handed to the pane task per [`PaneCmd::Rollout`].
pub const LINES_PER_BATCH: usize = 256;

/// Messages queued towards a tailer thread; wakes beyond it are dropped, since one pending wake suffices.
const QUEUE: usize = 16;

/// Day directories searched for new rollouts, newest first; a spawn just before midnight needs two.
const RECENT_DAYS: usize = 2;

/// How much earlier than the spawn a rollout may claim to be created, for filesystems with coarse birth times.
const SPAWN_SLACK: Duration = Duration::from_secs(1);

/// Shortest interval between two full walks of `sessions/` for a resumed thread's older file.
const WALK_INTERVAL: Duration = Duration::from_secs(5);

/// What a Codex pane's tailer needs to find its rollout.
#[derive(Debug, Clone)]
pub struct TailOptions {
    /// The pane.
    pub pane_id: PaneId,
    /// The `CODEX_HOME` the pane's process uses (its `CODEX_HOME`, else `$HOME/.codex`).
    pub codex_home: PathBuf,
    /// The directory Codex was started in.
    pub cwd: String,
    /// When the process was spawned; rollouts created before it belong to other sessions.
    pub spawned_at: SystemTime,
    /// The thread the pane resumed, if any: only that thread's rollout is followed.
    pub thread: Option<String>,
}

#[derive(Debug)]
enum Msg {
    Find(String),
    Wake,
    Stop,
}

/// The pane task's handle on its tailer thread; dropping it stops the thread within one poll interval.
#[derive(Debug)]
pub struct Tailer {
    tx: SyncSender<Msg>,
    stop: Arc<AtomicBool>,
}

impl Tailer {
    /// Starts the tailer thread; lines go to `pane` until that channel closes. Fails with [`Error::Io`] if no thread starts.
    pub fn spawn(
        shared: Arc<Shared>,
        options: TailOptions,
        pane: WeakSender<PaneCmd>,
    ) -> Result<Self> {
        let (tx, rx) = sync_channel(QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let state = TailState {
            sessions: options.codex_home.join("sessions"),
            cwd: canonical(&options.cwd),
            spawned_at: options
                .spawned_at
                .checked_sub(SPAWN_SLACK)
                .unwrap_or(options.spawned_at),
            resumed: options.thread,
            wanted: Vec::new(),
            first_lines: HashMap::new(),
            last_walk: None,
            bound: None,
            pane_id: options.pane_id,
            shared,
            pane,
        };
        let wake = tx.clone();
        let flag = Arc::clone(&stop);
        std::thread::Builder::new()
            .name(format!("rollout-{}", options.pane_id))
            .spawn(move || state.run(&rx, &wake, &flag))
            .map_err(|source| Error::Io {
                what: "cannot start the rollout tailer for",
                path: options.codex_home,
                source,
            })?;
        Ok(Self { tx, stop })
    }

    /// Asks the tailer to follow `rollout-*-<thread_id>.jsonl` once it exists (a notify's thread, R28).
    pub fn find(&self, thread_id: String) {
        match self.tx.try_send(Msg::Find(thread_id)) {
            Ok(()) => {}
            Err(TrySendError::Full(Msg::Find(thread))) => {
                tracing::warn!(thread, "the rollout tailer is busy; thread request dropped");
            }
            Err(e) => tracing::debug!(error = %e, "the rollout tailer has stopped"),
        }
    }
}

impl Drop for Tailer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Err(TrySendError::Disconnected(_)) = self.tx.try_send(Msg::Stop) {
            tracing::trace!("the rollout tailer had already stopped");
        }
    }
}

struct Bound {
    path: PathBuf,
    file: File,
    lines: LineBuffer,
}

struct TailState {
    sessions: PathBuf,
    cwd: String,
    spawned_at: SystemTime,
    resumed: Option<String>,
    wanted: Vec<String>,
    first_lines: HashMap<PathBuf, Option<String>>,
    last_walk: Option<Instant>,
    bound: Option<Bound>,
    pane_id: PaneId,
    shared: Arc<Shared>,
    pane: WeakSender<PaneCmd>,
}

impl TailState {
    fn run(mut self, rx: &Receiver<Msg>, wake: &SyncSender<Msg>, stop: &AtomicBool) {
        let pane_id = self.pane_id;
        tracing::debug!(pane_id, sessions = %self.sessions.display(), "rollout tailer started");
        let mut watcher: Option<RecommendedWatcher> = None;
        while !stop.load(Ordering::Relaxed) {
            if watcher.is_none() && self.sessions.is_dir() {
                watcher = watch(pane_id, &self.sessions, wake.clone());
            }
            if self.bound.is_none() {
                self.bind();
            }
            if !self.pump() {
                break;
            }
            let wait = if watcher.is_some() && self.bound.is_some() {
                IDLE_POLL
            } else {
                POLL
            };
            match rx.recv_timeout(wait) {
                Ok(Msg::Find(thread)) => {
                    if !self.wanted.contains(&thread) {
                        self.wanted.push(thread);
                    }
                }
                Ok(Msg::Wake) | Err(RecvTimeoutError::Timeout) => {}
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if let Some(bound) = self.bound.take() {
            self.shared.release_rollout(&bound.path);
        }
        tracing::debug!(pane_id, "rollout tailer stopped");
    }

    fn bind(&mut self) {
        let Some(path) = self.discover() else {
            return;
        };
        if !self.shared.claim_rollout(&path) {
            return;
        }
        match File::open(&path) {
            Ok(file) => {
                tracing::info!(pane_id = self.pane_id, rollout = %path.display(), "following the Codex rollout");
                self.bound = Some(Bound {
                    path,
                    file,
                    lines: LineBuffer::default(),
                });
            }
            Err(e) => {
                tracing::warn!(pane_id = self.pane_id, rollout = %path.display(), error = %e, "cannot open the rollout");
                self.shared.release_rollout(&path);
            }
        }
    }

    fn discover(&mut self) -> Option<PathBuf> {
        let recent = rollouts_in(&recent_day_dirs(&self.sessions, RECENT_DAYS));
        let named = |threads: &[String], files: &[(PathBuf, String)]| {
            files
                .iter()
                .find(|(_, thread)| threads.contains(thread))
                .map(|(path, _)| path.clone())
        };
        if let Some(thread) = self.resumed.clone() {
            let threads = [thread];
            if let Some(path) = named(&threads, &recent) {
                return Some(path);
            }
            if self.last_walk.is_none_or(|t| t.elapsed() >= WALK_INTERVAL) {
                self.last_walk = Some(Instant::now());
                return named(&threads, &rollouts_in(&all_day_dirs(&self.sessions)));
            }
            return None;
        }
        if let Some(path) = named(&self.wanted, &recent) {
            return Some(path);
        }
        let mut candidates = Vec::new();
        for (path, _) in recent {
            let created = match std::fs::metadata(&path).and_then(|m| m.created().or(m.modified()))
            {
                Ok(created) => created,
                Err(e) => {
                    tracing::debug!(pane_id = self.pane_id, rollout = %path.display(), error = %e, "cannot stat a rollout");
                    continue;
                }
            };
            if created < self.spawned_at || self.shared.rollout_claimed(&path) {
                continue;
            }
            let Some(cwd) = self.first_line_cwd(&path) else {
                continue;
            };
            candidates.push(RolloutCandidate { path, created, cwd });
        }
        pick_rollout_by_cwd(&candidates, self.spawned_at, &self.cwd).map(|c| c.path.clone())
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

    /// Hands every complete new line of the bound file to the pane task; false once the pane task is gone.
    fn pump(&mut self) -> bool {
        let Some(bound) = self.bound.as_mut() else {
            return true;
        };
        let mut buf = vec![0u8; READ_CHUNK];
        let mut batch = Vec::new();
        loop {
            match bound.file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    for framed in bound.lines.push(&buf[..n]) {
                        match framed.into_line() {
                            Ok(line) => batch.push(line),
                            Err(e) => {
                                tracing::warn!(pane_id = self.pane_id, error = %e, "rollout line skipped");
                            }
                        }
                    }
                    if batch.len() >= LINES_PER_BATCH
                        && !send(&self.pane, std::mem::take(&mut batch))
                    {
                        return false;
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => {
                    tracing::warn!(pane_id = self.pane_id, rollout = %bound.path.display(), error = %e, "cannot read the rollout");
                    break;
                }
            }
        }
        batch.is_empty() || send(&self.pane, batch)
    }
}

fn send(pane: &WeakSender<PaneCmd>, lines: Vec<Vec<u8>>) -> bool {
    pane.upgrade()
        .is_some_and(|tx| tx.blocking_send(PaneCmd::Rollout(lines)).is_ok())
}

fn watch(pane_id: PaneId, sessions: &Path, wake: SyncSender<Msg>) -> Option<RecommendedWatcher> {
    let handler = move |event: notify::Result<notify::Event>| {
        if let Err(e) = event {
            tracing::debug!(pane_id, error = %e, "rollout watcher error");
        }
        match wake.try_send(Msg::Wake) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                tracing::trace!(pane_id, "the rollout tailer has stopped");
            }
        }
    };
    let mut watcher = match notify::recommended_watcher(handler) {
        Ok(watcher) => watcher,
        Err(e) => {
            tracing::warn!(pane_id, error = %e, "cannot watch Codex's sessions; polling instead");
            return None;
        }
    };
    match watcher.watch(sessions, RecursiveMode::Recursive) {
        Ok(()) => Some(watcher),
        Err(e) => {
            tracing::warn!(pane_id, sessions = %sessions.display(), error = %e, "cannot watch Codex's sessions; polling instead");
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

fn numbered_dirs_desc(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
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
fn recent_day_dirs(sessions: &Path, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for year in numbered_dirs_desc(sessions) {
        for month in numbered_dirs_desc(&year) {
            for day in numbered_dirs_desc(&month) {
                out.push(day);
                if out.len() == limit {
                    return out;
                }
            }
        }
    }
    out
}

fn all_day_dirs(sessions: &Path) -> Vec<PathBuf> {
    recent_day_dirs(sessions, usize::MAX)
}

/// The rollout files in `dirs` with their thread ids, in directory order.
fn rollouts_in(dirs: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
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
        let recent = recent_day_dirs(&root, 2);
        assert_eq!(recent, [root.join("2026/02/03"), root.join("2026/01/02")]);
        assert_eq!(all_day_dirs(&root).len(), 4);
        let id = "00000000-0000-7000-8000-000000000001";
        std::fs::write(
            root.join(format!("2025/12/31/rollout-2025-12-31T23-59-59-{id}.jsonl")),
            "",
        )
        .unwrap();
        std::fs::write(root.join("2025/12/31/notes.txt"), "").unwrap();
        let files = rollouts_in(&all_day_dirs(&root));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1, id);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
