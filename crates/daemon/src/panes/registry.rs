//! The pane registry: workspaces, tabs (order, focus, zoom), panes and their tasks, mirrored to SQLite.
//!
//! Every C1 method reads or changes state here, under one lock held only for in-memory work and a few short SQLite
//! writes. Pane ids are the `panes` row ids, so they never repeat and double as the C2 `pane_id` (spec 4.2). A pane's
//! `position` is its index in its tab, at most [`MAX_PANES_PER_TAB`] of them; positions are renumbered whenever a pane leaves a tab,
//! and a tab disappears with its last open pane. Tabs are named after the basename of their first pane's directory
//! (Ruling R3); one default workspace, the home directory, exists after the first start. Events for C1
//! (`pane.added`, `pane.removed`, `pane.status`, `pane.meta`, `pane.exit`) are broadcast from here, after the change
//! is stored. `layout.save`'s `active_tab_id` has no column in schema v1, so it is kept in memory only. The task
//! queue (Ruling R60) lives here too, in [`Queues`]: every task change is stored in `tasks` and then announced with
//! `task.changed`; a queue's pause and block are kept in memory and announced with `queue.changed`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use ply_agents::SessionMeta;
use ply_proto::control::{
    ErrorBody, ErrorCode, Event, PaneExit, PaneMeta, PaneProgress, PaneRemoved, PaneStatusChanged,
    TaskAddParams,
};
use ply_proto::pane::{
    AgentCli, BlockReason, Cli, Layout, MAX_OPEN_TASK_TEXT_BYTES, MAX_PANES_PER_TAB,
    MAX_QUEUED_TASKS, Pane, PaneId, PaneStatus, PauseReason, Progress, Session, Settings, Tab,
    Task, TaskId, TaskList, TaskState, TaskTarget, TerminalTheme, UnixSeconds, Workspace,
};
use tokio::sync::{broadcast, mpsc};

use crate::config::Config;
use crate::db::{Db, TabRow};
use crate::error::Result;
use crate::panes::pane::PaneCmd;
use crate::panes::queue::{MAX_SKILL_BYTES, QueueKey, Queues, REOPENED_AS_SHELL, check_text};

/// The result of a registry change requested over C1: the value, or the error to answer with.
pub type MethodResult<T> = std::result::Result<T, ErrorBody>;

/// A C1 error body with `code` and `msg`.
pub fn refuse(code: ErrorCode, msg: impl Into<String>) -> ErrorBody {
    ErrorBody {
        code,
        msg: msg.into(),
    }
}

/// Logs an internal failure and turns it into the `internal` error the client sees.
pub fn internal(context: &str, e: &crate::Error) -> ErrorBody {
    tracing::error!(error = %e, "{context}");
    refuse(ErrorCode::Internal, format!("{context}: {e}"))
}

/// [`internal`] for a failure of one pane, logged with its `pane_id`.
pub fn internal_for(pane_id: PaneId, context: &str, e: &crate::Error) -> ErrorBody {
    tracing::error!(pane_id, error = %e, "{context}");
    refuse(ErrorCode::Internal, format!("{context}: {e}"))
}

/// Whether a pane in `status` has (or should have) a process.
pub fn is_live(status: PaneStatus) -> bool {
    !matches!(status, PaneStatus::Exited | PaneStatus::Lost)
}

/// One open pane: its record and the channel to its task.
#[derive(Debug)]
pub struct PaneEntry {
    /// The record C1 returns.
    pub pane: Pane,
    /// The pane task; `None` only between insertion and the spawn finishing.
    pub handle: Option<mpsc::Sender<PaneCmd>>,
    /// `pane.close {kill:true}` asked for the pane to close once its process has exited.
    pub close_on_exit: bool,
    /// Clients know the pane: it was announced with `pane.added` or restored at startup; `pane.list` shows only these.
    pub announced: bool,
    /// A `pane.resume` is relaunching the pane's process; a second one is refused.
    pub resuming: bool,
    /// The last worktree the CLI reported, which the stored session record keeps after the live label cleared.
    pub recorded_worktree: Option<String>,
}

/// What `pane.create` asks the registry to record.
#[derive(Debug, Clone, Copy)]
pub struct NewPane<'a> {
    /// Workspace of the pane.
    pub workspace_id: u64,
    /// Existing tab to append to; `None` opens a new tab named after `cwd`.
    pub tab_id: Option<u64>,
    /// Program the pane runs.
    pub cli: Cli,
    /// Absolute working directory.
    pub cwd: &'a str,
    /// Initial header title.
    pub title: &'a str,
    /// Initial state.
    pub status: PaneStatus,
}

#[derive(Debug, Clone)]
struct TabState {
    row: TabRow,
    panes: Vec<PaneId>,
}

/// See the module docs; not `Sync` (it owns the database connection), so it lives behind a mutex.
#[derive(Debug)]
pub struct Registry {
    db: Db,
    config: Config,
    config_path: std::path::PathBuf,
    events: broadcast::Sender<Event>,
    workspaces: BTreeMap<u64, Workspace>,
    tabs: BTreeMap<u64, TabState>,
    panes: BTreeMap<PaneId, PaneEntry>,
    active_tabs: HashMap<u64, u64>,
    queues: Queues,
}

impl Registry {
    /// Loads `db`, marks panes live at the last stop `lost` (spec 6.3), drops empty tabs, creates the `home` workspace if none.
    /// Fails with [`crate::Error::Db`] or [`crate::Error::BadRow`].
    pub fn load(
        db: Db,
        config: Config,
        config_path: &Path,
        events: broadcast::Sender<Event>,
        home: &str,
        now: UnixSeconds,
    ) -> Result<Self> {
        let mut reg = Self {
            workspaces: db.workspaces()?.into_iter().map(|w| (w.id, w)).collect(),
            tabs: BTreeMap::new(),
            panes: BTreeMap::new(),
            active_tabs: HashMap::new(),
            queues: Queues::default(),
            db,
            config,
            config_path: config_path.to_path_buf(),
            events,
        };
        if reg.workspaces.is_empty() {
            let ws = reg.db.insert_workspace(home, &basename(home), now)?;
            tracing::info!(path = %ws.path, "created the default workspace");
            reg.workspaces.insert(ws.id, ws);
        }
        for row in reg.db.tabs()? {
            reg.tabs.insert(
                row.id,
                TabState {
                    row,
                    panes: Vec::new(),
                },
            );
        }
        let mut panes = reg.db.open_panes()?;
        panes.sort_by_key(|p| (p.tab_id, p.position, p.id));
        for mut pane in panes {
            if is_live(pane.status) {
                pane.status = PaneStatus::Lost;
                pane.detail = None;
                reg.db.update_pane(&pane)?;
                tracing::info!(
                    pane_id = pane.id,
                    "the pane's process did not survive the restart; marked lost"
                );
            }
            if !reg.tabs.contains_key(&pane.tab_id) {
                let row = TabRow {
                    id: 0,
                    workspace_id: pane.workspace_id,
                    name: basename(&pane.cwd),
                    position: u32::try_from(reg.tabs.len()).unwrap_or(u32::MAX),
                    focus_pane_id: Some(pane.id),
                    zoomed: false,
                };
                let id = reg.db.insert_tab(&row)?;
                tracing::warn!(
                    pane_id = pane.id,
                    tab_id = id,
                    "pane referred to a missing tab; recreated it"
                );
                pane.tab_id = id;
                reg.tabs.insert(
                    id,
                    TabState {
                        row: TabRow { id, ..row },
                        panes: Vec::new(),
                    },
                );
            }
            if let Some(tab) = reg.tabs.get_mut(&pane.tab_id) {
                tab.panes.push(pane.id);
            }
            reg.panes.insert(
                pane.id,
                PaneEntry {
                    recorded_worktree: pane.worktree_seen.clone(),
                    pane,
                    handle: None,
                    close_on_exit: false,
                    announced: true,
                    resuming: false,
                },
            );
        }
        let empty: Vec<u64> = reg
            .tabs
            .values()
            .filter(|t| t.panes.is_empty())
            .map(|t| t.row.id)
            .collect();
        for id in empty {
            reg.tabs.remove(&id);
            reg.db.delete_tab(id)?;
        }
        let open: HashSet<PaneId> = reg.panes.keys().copied().collect();
        let (queues, changed) = Queues::restore(reg.db.tasks()?, &open, now);
        reg.queues = queues;
        for task in &changed {
            reg.db.update_task(task)?;
        }
        let ws_ids: Vec<u64> = reg.workspaces.keys().copied().collect();
        for ws in ws_ids {
            let pruned = reg.queues.prune(ws);
            reg.db.delete_tasks(&pruned)?;
        }
        let tab_ids: Vec<u64> = reg.tabs.keys().copied().collect();
        for id in tab_ids {
            reg.renumber_tab(id)?;
        }
        let ws_ids: Vec<u64> = reg.workspaces.keys().copied().collect();
        for ws in ws_ids {
            reg.renumber_tabs(ws)?;
        }
        Ok(reg)
    }

    /// The stored settings.
    pub fn settings(&self) -> Settings {
        self.config.settings.clone()
    }

    /// Replaces the settings in memory, where they apply at once; [`Registry::save_config`] writes them.
    pub fn set_settings(&mut self, settings: Settings) {
        self.config.settings = settings;
    }

    /// The last `theme.set` palette.
    pub fn palette(&self) -> Option<&TerminalTheme> {
        self.config.palette.as_ref()
    }

    /// Keeps the palette in memory, where it applies at once; [`Registry::save_config`] writes it.
    pub fn set_palette(&mut self, theme: TerminalTheme) {
        self.config.palette = Some(theme);
    }

    /// Writes the settings and the palette to `config.toml`; fails with `internal` (logged) when the file cannot be written.
    pub fn save_config(&self) -> MethodResult<()> {
        self.config
            .save(&self.config_path)
            .map_err(|e| internal("cannot write config.toml", &e))
    }

    /// Every workspace, by id.
    pub fn workspaces(&self) -> Vec<Workspace> {
        self.workspaces.values().cloned().collect()
    }

    /// The workspace at `path`, created when new; `opened_at` becomes `now`.
    /// Fails with `bad_request` for a relative path or one that is not a directory.
    pub fn open_workspace(&mut self, path: &str, now: UnixSeconds) -> MethodResult<Workspace> {
        if !Path::new(path).is_absolute() {
            return Err(refuse(ErrorCode::BadRequest, "path must be absolute"));
        }
        if !Path::new(path).is_dir() {
            return Err(refuse(
                ErrorCode::BadRequest,
                format!("{path} is not a directory"),
            ));
        }
        if let Some(ws) = self.workspaces.values_mut().find(|w| w.path == path) {
            ws.opened_at = now;
            let ws = ws.clone();
            self.db
                .touch_workspace(ws.id, now)
                .map_err(|e| internal("cannot store the workspace", &e))?;
            return Ok(ws);
        }
        let ws = self
            .db
            .insert_workspace(path, &basename(path), now)
            .map_err(|e| internal("cannot store the workspace", &e))?;
        self.workspaces.insert(ws.id, ws.clone());
        Ok(ws)
    }

    fn require_workspace(&self, id: u64) -> MethodResult<&Workspace> {
        self.workspaces
            .get(&id)
            .ok_or_else(|| refuse(ErrorCode::NotFound, format!("no workspace {id}")))
    }

    /// Open panes of a workspace, by id; `not_found` for an unknown workspace.
    pub fn panes_of(&self, workspace_id: u64) -> MethodResult<Vec<Pane>> {
        self.require_workspace(workspace_id)?;
        Ok(self
            .panes
            .values()
            .filter(|e| e.announced && e.pane.workspace_id == workspace_id)
            .map(|e| e.pane.clone())
            .collect())
    }

    /// Stored session records of a workspace (closed ones when asked); `not_found` for an unknown workspace.
    pub fn sessions(&self, workspace_id: u64, include_closed: bool) -> MethodResult<Vec<Session>> {
        self.require_workspace(workspace_id)?;
        self.db
            .sessions(workspace_id, include_closed)
            .map_err(|e| internal("cannot read the sessions", &e))
    }

    /// The open pane `id`.
    pub fn entry(&self, id: PaneId) -> Option<&PaneEntry> {
        self.panes.get(&id)
    }

    /// The open pane `id`, or `not_found`.
    pub fn require(&self, id: PaneId) -> MethodResult<&PaneEntry> {
        self.panes
            .get(&id)
            .ok_or_else(|| refuse(ErrorCode::NotFound, format!("no pane {id}")))
    }

    /// Every open pane's task channel, for broadcasts such as a new palette.
    pub fn handles(&self) -> Vec<(PaneId, mpsc::Sender<PaneCmd>)> {
        self.panes
            .iter()
            .filter_map(|(id, e)| e.handle.clone().map(|h| (*id, h)))
            .collect()
    }

    /// Open panes whose process runs, i.e. that are not `exited` or `lost`.
    pub fn live_panes(&self) -> Vec<PaneId> {
        self.panes
            .values()
            .filter(|e| is_live(e.pane.status))
            .map(|e| e.pane.id)
            .collect()
    }

    /// Panes in `running`, which keep the Mac awake (spec 11.3).
    pub fn running_count(&self) -> usize {
        self.panes
            .values()
            .filter(|e| e.pane.status == PaneStatus::Running)
            .count()
    }

    /// Adds a pane record (no event yet) in tab `tab_id`, at its end, or in a new tab named after `cwd`.
    /// Fails with `not_found` for an unknown workspace or tab, `tab_full` for a tab of [`MAX_PANES_PER_TAB`], `internal` if SQLite fails.
    pub fn insert_pane(&mut self, new: &NewPane<'_>, now: UnixSeconds) -> MethodResult<Pane> {
        let NewPane {
            workspace_id,
            tab_id,
            cli,
            cwd,
            title,
            status,
        } = *new;
        self.require_workspace(workspace_id)?;
        let store = |e: crate::Error| internal("cannot store the pane", &e);
        let (tab_id, position, new_tab) = match tab_id {
            Some(id) => {
                let tab = self
                    .tabs
                    .get(&id)
                    .filter(|t| t.row.workspace_id == workspace_id)
                    .ok_or_else(|| {
                        refuse(
                            ErrorCode::NotFound,
                            format!("no tab {id} in this workspace"),
                        )
                    })?;
                if tab.panes.len() >= MAX_PANES_PER_TAB {
                    return Err(refuse(
                        ErrorCode::TabFull,
                        format!("tab {id} already holds {MAX_PANES_PER_TAB} panes"),
                    ));
                }
                (id, count(tab.panes.len()), false)
            }
            None => {
                let row = TabRow {
                    id: 0,
                    workspace_id,
                    name: basename(cwd),
                    position: count(self.tabs_of(workspace_id).len()),
                    focus_pane_id: None,
                    zoomed: false,
                };
                let id = self.db.insert_tab(&row).map_err(store)?;
                self.tabs.insert(
                    id,
                    TabState {
                        row: TabRow { id, ..row },
                        panes: Vec::new(),
                    },
                );
                (id, 0, true)
            }
        };
        let mut pane = Pane {
            id: 0,
            workspace_id,
            tab_id,
            position,
            cli,
            cwd: cwd.to_owned(),
            title: title.to_owned(),
            status,
            detail: None,
            progress: None,
            model_seen: None,
            worktree_seen: None,
            branch: None,
            project: None,
            git_worktree: None,
            session_ref: None,
            exit_code: None,
            created_at: now,
            closed_at: None,
            last_activity_at: None,
        };
        pane.id = match self.db.insert_pane(&pane) {
            Ok(id) => id,
            Err(e) => {
                if new_tab {
                    self.drop_tab(tab_id);
                }
                return Err(store(e));
            }
        };
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.panes.push(pane.id);
            if tab.row.focus_pane_id.is_none() {
                tab.row.focus_pane_id = Some(pane.id);
                self.db.update_tab(&tab.row).map_err(store)?;
            }
        }
        self.active_tabs.entry(workspace_id).or_insert(tab_id);
        self.panes.insert(
            pane.id,
            PaneEntry {
                pane: pane.clone(),
                handle: None,
                close_on_exit: false,
                announced: false,
                resuming: false,
                recorded_worktree: None,
            },
        );
        Ok(pane)
    }

    /// Removes a pane whose process never started, as if it had never been created (no event).
    pub fn discard_pane(&mut self, id: PaneId) {
        let Some(entry) = self.panes.remove(&id) else {
            return;
        };
        if let Err(e) = self.db.delete_pane(id) {
            tracing::error!(pane_id = id, error = %e, "cannot delete the pane that failed to start");
        }
        if let Err(e) = self.leave_tab(&entry.pane) {
            tracing::error!(pane_id = id, error = %e, "cannot update the tab of the pane that failed to start");
        }
    }

    /// Records the pane's task and announces the pane with `pane.added`.
    pub fn announce(&mut self, id: PaneId, handle: mpsc::Sender<PaneCmd>) {
        let Some(entry) = self.panes.get_mut(&id) else {
            return;
        };
        entry.handle = Some(handle);
        entry.announced = true;
        let pane = entry.pane.clone();
        self.emit(Event::PaneAdded(Box::new(pane)));
    }

    /// Records a restored pane's task without an event (clients learn restored panes from `pane.list`).
    pub fn set_handle(&mut self, id: PaneId, handle: mpsc::Sender<PaneCmd>) {
        if let Some(entry) = self.panes.get_mut(&id) {
            entry.handle = Some(handle);
        }
    }

    /// Starts a `pane.resume` of the `lost` pane `id`: moves it to `status` (`pane.status`) and marks it resuming.
    /// Fails with `not_found`, or `invalid_state` when the pane is not lost or another resume runs.
    pub fn begin_resume(
        &mut self,
        id: PaneId,
        status: PaneStatus,
        now: UnixSeconds,
    ) -> MethodResult<Pane> {
        let entry = self.require(id)?;
        if entry.resuming {
            return Err(refuse(
                ErrorCode::InvalidState,
                "the pane is already resuming",
            ));
        }
        if entry.pane.status != PaneStatus::Lost {
            return Err(refuse(ErrorCode::InvalidState, "the pane is not lost"));
        }
        self.set_status(id, status, None, now);
        let entry = self
            .panes
            .get_mut(&id)
            .ok_or_else(|| refuse(ErrorCode::NotFound, format!("no pane {id}")))?;
        entry.resuming = true;
        Ok(entry.pane.clone())
    }

    /// Ends a `pane.resume`; a failed one is `lost` again, and `true` means a `pane.close {kill:true}` during it wants the pane closed now (no process will exit to close it).
    pub fn end_resume(&mut self, id: PaneId, started: bool, now: UnixSeconds) -> bool {
        let Some(entry) = self.panes.get_mut(&id) else {
            return false;
        };
        entry.resuming = false;
        let close = !started && entry.close_on_exit;
        if !started {
            self.set_status(id, PaneStatus::Lost, None, now);
        }
        close
    }

    /// Marks that the pane closes once its process has exited (`pane.close {kill:true}`).
    pub fn close_on_exit(&mut self, id: PaneId) {
        if let Some(entry) = self.panes.get_mut(&id) {
            entry.close_on_exit = true;
        }
    }

    /// Changes a pane's state and broadcasts `pane.status`; unchanged state and detail send nothing.
    pub fn set_status(
        &mut self,
        id: PaneId,
        status: PaneStatus,
        detail: Option<String>,
        now: UnixSeconds,
    ) {
        let Some(entry) = self.panes.get_mut(&id) else {
            return;
        };
        if entry.pane.status == status && entry.pane.detail == detail {
            return;
        }
        entry.pane.status = status;
        entry.pane.detail.clone_from(&detail);
        if status != PaneStatus::Exited {
            entry.pane.exit_code = None;
        }
        let pane = entry.pane.clone();
        self.store(&pane);
        self.emit(Event::PaneStatus(PaneStatusChanged {
            pane_id: id,
            status,
            detail,
            exit_code: pane.exit_code,
            at: now,
        }));
    }

    /// Records the process's exit: `pane.status exited` then `pane.exit`. Returns whether the pane is to close now.
    pub fn mark_exited(&mut self, id: PaneId, code: i32, now: UnixSeconds) -> bool {
        let Some(entry) = self.panes.get_mut(&id) else {
            return false;
        };
        entry.pane.status = PaneStatus::Exited;
        entry.pane.detail = None;
        entry.pane.exit_code = Some(code);
        entry.pane.last_activity_at = Some(now);
        let close = entry.close_on_exit;
        let pane = entry.pane.clone();
        self.store(&pane);
        self.emit(Event::PaneStatus(PaneStatusChanged {
            pane_id: id,
            status: PaneStatus::Exited,
            detail: None,
            exit_code: Some(code),
            at: now,
        }));
        self.emit(Event::PaneExit(PaneExit {
            pane_id: id,
            code,
            at: now,
        }));
        close
    }

    /// Sets the header title in memory (an empty one falls back to `fallback`); the next row write stores it. No event.
    pub fn set_title(&mut self, id: PaneId, title: &str, fallback: &str) {
        let Some(entry) = self.panes.get_mut(&id) else {
            return;
        };
        let title = if title.is_empty() { fallback } else { title };
        if entry.pane.title != title {
            entry.pane.title = title.to_owned();
        }
    }

    /// Records a reported working directory (OSC 7) and broadcasts `pane.meta`; returns whether it changed.
    pub fn set_cwd(&mut self, id: PaneId, cwd: &str) -> bool {
        let Some(entry) = self.panes.get_mut(&id) else {
            return false;
        };
        if entry.pane.cwd == cwd {
            return false;
        }
        entry.pane.cwd = cwd.to_owned();
        entry.pane.branch = None;
        let pane = entry.pane.clone();
        self.store(&pane);
        self.emit_meta(&pane);
        true
    }

    /// Records what the session reports (spec 6.5, `pane.meta` per Ruling R4); returns whether the directory changed.
    pub fn set_meta(&mut self, id: PaneId, meta: &SessionMeta) -> bool {
        let Some(entry) = self.panes.get_mut(&id) else {
            return false;
        };
        let before = entry.pane.clone();
        let p = &mut entry.pane;
        if meta.session_ref.is_some() {
            p.session_ref.clone_from(&meta.session_ref);
        }
        if meta.model.is_some() {
            p.model_seen.clone_from(&meta.model);
        }
        let moved = meta.cwd.as_ref().is_some_and(|cwd| *cwd != p.cwd);
        if let Some(cwd) = &meta.cwd {
            p.cwd.clone_from(cwd);
            p.worktree_seen.clone_from(&meta.worktree);
            if meta.worktree.is_some() {
                entry.recorded_worktree.clone_from(&meta.worktree);
            }
        }
        if moved {
            p.branch = None;
        }
        if *p == before {
            return false;
        }
        let pane = p.clone();
        self.store(&pane);
        self.emit_meta(&pane);
        moved
    }

    /// Records git's branch, project and linked worktree for `cwd` (display only, never stored) and broadcasts `pane.meta` when they changed, unless the pane moved on; without a repository the project is `cwd`'s folder name.
    pub fn set_git(&mut self, id: PaneId, cwd: &str, git: &crate::branch::GitInfo) {
        let Some(entry) = self.panes.get_mut(&id) else {
            return;
        };
        let project = git.project.clone().or_else(|| {
            std::path::Path::new(cwd)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        });
        let pane = &mut entry.pane;
        if pane.cwd != cwd
            || (pane.branch == git.branch
                && pane.project == project
                && pane.git_worktree == git.worktree)
        {
            return;
        }
        pane.branch.clone_from(&git.branch);
        pane.project = project;
        pane.git_worktree.clone_from(&git.worktree);
        let pane = pane.clone();
        self.emit_meta(&pane);
    }

    /// Records the plan progress (kept in memory only; schema v1 has no column) and broadcasts `pane.progress`.
    pub fn set_progress(&mut self, id: PaneId, progress: Option<Progress>) {
        let Some(entry) = self.panes.get_mut(&id) else {
            return;
        };
        if entry.pane.progress == progress {
            return;
        }
        entry.pane.progress.clone_from(&progress);
        self.emit(Event::PaneProgress(PaneProgress {
            pane_id: id,
            progress,
        }));
    }

    /// Makes the pane run `cli` from now on, titled `title`, and announces the whole record again with `pane.added` (no other event carries `cli`); `pane.resume` reopens a pane without a session as a shell, which ends its queued tasks.
    pub fn set_cli(&mut self, id: PaneId, cli: Cli, title: &str, now: UnixSeconds) {
        let Some(entry) = self.panes.get_mut(&id) else {
            return;
        };
        entry.pane.cli = cli;
        entry.pane.title = title.to_owned();
        entry.pane.progress = None;
        entry.pane.model_seen = None;
        let pane = entry.pane.clone();
        let announced = entry.announced;
        self.store(&pane);
        if cli == Cli::Shell {
            let ended = self.queues.end_pane(id, REOPENED_AS_SHELL, now);
            self.apply_tasks(ended);
        }
        if announced {
            self.emit(Event::PaneAdded(Box::new(pane)));
        }
    }

    fn emit_meta(&self, pane: &Pane) {
        self.emit(Event::PaneMeta(PaneMeta {
            pane_id: pane.id,
            model: pane.model_seen.clone(),
            worktree: pane.worktree_seen.clone(),
            cwd: pane.cwd.clone(),
            branch: pane.branch.clone(),
            project: pane.project.clone(),
            git_worktree: pane.git_worktree.clone(),
        }));
    }

    /// Records pty activity at `now` and stores the row, title included (the caller throttles it).
    pub fn touch(&mut self, id: PaneId, now: UnixSeconds) {
        let Some(entry) = self.panes.get_mut(&id) else {
            return;
        };
        entry.pane.last_activity_at = Some(now);
        let pane = entry.pane.clone();
        self.store(&pane);
    }

    /// Closes the open pane `id`: stores `closed_at`, removes it from its tab and broadcasts `pane.removed`.
    /// Returns the pane's task so the caller can stop it; `not_found` for an unknown pane.
    pub fn close_pane(
        &mut self,
        id: PaneId,
        now: UnixSeconds,
    ) -> MethodResult<Option<mpsc::Sender<PaneCmd>>> {
        let mut entry = self
            .panes
            .remove(&id)
            .ok_or_else(|| refuse(ErrorCode::NotFound, format!("no pane {id}")))?;
        entry.pane.closed_at = Some(now);
        self.store(&entry.pane);
        if let Err(e) = self.leave_tab(&entry.pane) {
            tracing::error!(pane_id = id, error = %e, "cannot update the tab of a closed pane");
        }
        let cancelled = self.queues.close_pane(id, now);
        self.apply_tasks(cancelled);
        self.emit(Event::PaneRemoved(PaneRemoved { pane_id: id }));
        Ok(entry.handle)
    }

    /// The workspace's tasks and flagged queues (`task.list`); `not_found` for an unknown workspace.
    pub fn tasks_of(&self, workspace_id: u64) -> MethodResult<TaskList> {
        if !self.workspaces.contains_key(&workspace_id) {
            return Err(refuse(
                ErrorCode::NotFound,
                format!("no workspace {workspace_id}"),
            ));
        }
        let panes: HashSet<PaneId> = self
            .panes
            .values()
            .filter(|e| e.pane.workspace_id == workspace_id)
            .map(|e| e.pane.id)
            .collect();
        Ok(self.queues.list(workspace_id, &panes))
    }

    /// Queues a task (`task.add`), stores it and announces it with `task.changed`.
    /// Fails with `not_found` (workspace, pane), `bad_request` (text, skill, a shell pane, a relative pool directory), `invalid_state` (an exited pane, a full queue), `internal` if SQLite fails.
    pub fn add_task(&mut self, p: &TaskAddParams, now: UnixSeconds) -> MethodResult<Task> {
        if !self.workspaces.contains_key(&p.workspace_id) {
            return Err(refuse(
                ErrorCode::NotFound,
                format!("no workspace {}", p.workspace_id),
            ));
        }
        check_text(&p.text)?;
        if p.skill
            .as_deref()
            .is_some_and(|s| s.len() > MAX_SKILL_BYTES || s.chars().any(char::is_control))
        {
            return Err(refuse(
                ErrorCode::BadRequest,
                "the skill is not a skill invocation",
            ));
        }
        let (pane_id, pool) = match &p.target {
            TaskTarget::Pane(id) => {
                let entry = self
                    .panes
                    .get(id)
                    .filter(|e| e.announced && e.pane.workspace_id == p.workspace_id)
                    .ok_or_else(|| {
                        refuse(
                            ErrorCode::NotFound,
                            format!("no pane {id} in this workspace"),
                        )
                    })?;
                if entry.pane.cli == Cli::Shell {
                    return Err(refuse(ErrorCode::BadRequest, "a shell pane takes no tasks"));
                }
                if entry.pane.status == PaneStatus::Exited {
                    return Err(refuse(
                        ErrorCode::InvalidState,
                        "the pane's process has exited",
                    ));
                }
                (Some(*id), None)
            }
            TaskTarget::Pool(pool) => {
                if !Path::new(&pool.cwd).is_absolute() {
                    return Err(refuse(
                        ErrorCode::BadRequest,
                        "the pool's directory must be absolute",
                    ));
                }
                (None, Some(pool.clone()))
            }
        };
        let mut task = Task {
            id: 0,
            workspace_id: p.workspace_id,
            pane_id,
            pool,
            text: p.text.clone(),
            skill: p.skill.clone(),
            state: TaskState::Queued,
            position: 0,
            detail: None,
            created_at: now,
            sent_at: None,
            started_at: None,
            ended_at: None,
        };
        let key = QueueKey::of(&task)
            .ok_or_else(|| refuse(ErrorCode::Internal, "the task has no queue"))?;
        let queued = self.queues.queued_len(&key);
        if queued >= MAX_QUEUED_TASKS {
            return Err(refuse(
                ErrorCode::InvalidState,
                format!("the queue already holds {MAX_QUEUED_TASKS} tasks"),
            ));
        }
        let open = self.queues.open_text_bytes(task.workspace_id);
        if open + task.text.len() > MAX_OPEN_TASK_TEXT_BYTES {
            return Err(refuse(
                ErrorCode::InvalidState,
                format!(
                    "the workspace's waiting tasks already hold {} KiB of text; the limit is {} KiB",
                    open / 1024,
                    MAX_OPEN_TASK_TEXT_BYTES / 1024
                ),
            ));
        }
        task.position = count(queued);
        task.id = self
            .db
            .insert_task(&task)
            .map_err(|e| internal("cannot store the task", &e))?;
        self.queues.insert(task.clone());
        self.emit(Event::TaskChanged(Box::new(task.clone())));
        Ok(task)
    }

    /// Cancels a queued task (`task.cancel`); `not_found` for an unknown task, `invalid_state` for one no longer queued.
    pub fn cancel_task(&mut self, id: TaskId, now: UnixSeconds) -> MethodResult<()> {
        let changed = self.queues.cancel(id, "cancelled", now)?;
        self.apply_tasks(changed);
        Ok(())
    }

    /// Moves a queued task within its queue (`task.move`); errors as [`Registry::cancel_task`].
    pub fn move_task(&mut self, id: TaskId, position: u32) -> MethodResult<()> {
        let changed = self.queues.move_to(id, position)?;
        self.apply_tasks(changed);
        Ok(())
    }

    /// Pauses or resumes a pane's queue (`queue.pause`) and announces a change; `not_found` for an unknown pane.
    pub fn pause_queue(&mut self, pane: PaneId, paused: bool) -> MethodResult<()> {
        self.require(pane)?;
        if let Some(state) = self.queues.pause(pane, paused.then_some(PauseReason::User)) {
            self.emit(Event::QueueChanged(state));
        }
        Ok(())
    }

    /// The task `pane`'s queue offers now: its head while the queue runs and no task is typed there; none for a shell.
    /// The pane's dispatch holds the head until its process shows its prompt, and [`Registry::take_task`] refuses a pane
    /// whose CLI has reported no session.
    pub fn queue_head(&self, pane: PaneId) -> Option<TaskId> {
        let entry = self.panes.get(&pane)?;
        if entry.pane.cli == Cli::Shell || self.queues.active(pane).is_some() {
            return None;
        }
        self.queues.head(pane).map(|t| t.id)
    }

    /// The task `pane` may type next: its own head, else, with `claim_pool`, the oldest pool task of its CLI for its
    /// directory, which it takes onto its queue (announced). None while the queue is paused or a task is typed there.
    pub fn next_task(&mut self, pane: PaneId, claim_pool: bool) -> Option<TaskId> {
        if let Some(head) = self.queue_head(pane) {
            return Some(head);
        }
        let entry = self.panes.get(&pane)?;
        let cli = match entry.pane.cli {
            Cli::Claude => AgentCli::Claude,
            Cli::Codex => AgentCli::Codex,
            Cli::Shell => return None,
        };
        if !claim_pool
            || entry.pane.session_ref.is_none()
            || entry.pane.status != PaneStatus::Idle
            || self.queues.active(pane).is_some()
            || self.queues.state_of(pane).paused.is_some()
        {
            return None;
        }
        let (workspace_id, cwd) = (entry.pane.workspace_id, entry.pane.cwd.clone());
        let claimed = self.queues.claim(pane, workspace_id, cli, &cwd);
        if claimed.is_empty() {
            return None;
        }
        tracing::info!(
            pane_id = pane,
            task_id = claimed[0].id,
            "the pane took a pool task"
        );
        self.apply_tasks(claimed);
        self.queue_head(pane)
    }

    /// The agent panes of `workspace_id` running `cli`, which a new pool task may go to.
    pub fn pool_panes(&self, workspace_id: u64, cli: AgentCli) -> Vec<PaneId> {
        let want = match cli {
            AgentCli::Claude => Cli::Claude,
            AgentCli::Codex => Cli::Codex,
        };
        self.panes
            .values()
            .filter(|e| {
                e.pane.workspace_id == workspace_id && e.pane.cli == want && is_live(e.pane.status)
            })
            .map(|e| e.pane.id)
            .collect()
    }

    /// Claims `task` for typing: marks it `sent` and returns its text when `pane` is idle, has no typed task and offers
    /// it as its head; `forced` (`task.send`) takes any queued task of the pane, past a pause. `None` otherwise.
    pub fn take_task(
        &mut self,
        pane: PaneId,
        task: TaskId,
        forced: bool,
        now: UnixSeconds,
    ) -> Option<String> {
        let entry = self.panes.get(&pane)?;
        if entry.pane.status != PaneStatus::Idle
            || entry.pane.cli == Cli::Shell
            || entry.pane.session_ref.is_none()
            || self.queues.active(pane).is_some()
        {
            return None;
        }
        let queued = self.queues.get(task)?;
        if queued.state != TaskState::Queued || queued.pane_id != Some(pane) {
            return None;
        }
        if !forced && self.queues.head(pane).map(|h| h.id) != Some(task) {
            return None;
        }
        let text = queued.text.clone();
        let changed = self.queues.set_state(task, TaskState::Sent, None, now);
        self.apply_tasks(changed);
        Some(text)
    }

    /// Records a typed task's progress (`running`, `ended` or `failed`); a failure pauses its pane's queue. A task that is
    /// no longer typed (its pane closed meanwhile) is left as it is.
    pub fn task_progress(
        &mut self,
        task: TaskId,
        state: TaskState,
        detail: Option<String>,
        now: UnixSeconds,
    ) {
        let Some(current) = self.queues.get(task) else {
            return;
        };
        if !matches!(current.state, TaskState::Sent | TaskState::Running) {
            tracing::debug!(
                task_id = task,
                ?state,
                "progress for a task that is no longer typed ignored"
            );
            return;
        }
        let pane = current.pane_id;
        let changed = self.queues.set_state(task, state, detail, now);
        self.apply_tasks(changed);
        if state == TaskState::Failed
            && let Some(pane) = pane
            && let Some(q) = self.queues.pause(pane, Some(PauseReason::Failed))
        {
            self.emit(Event::QueueChanged(q));
        }
    }

    /// Blocks (`Some`) or unblocks (`None`) a pane's queue and announces a change.
    pub fn block_queue(&mut self, pane: PaneId, reason: Option<BlockReason>) {
        if let Some(q) = self.queues.block(pane, reason) {
            self.emit(Event::QueueChanged(q));
        }
    }

    /// `task.send`'s checks: a queued task on a pane that is idle with no task typed; returns the pane and its task.
    /// Fails with `not_found` for an unknown task and `invalid_state` otherwise.
    pub fn sendable(&self, task: TaskId) -> MethodResult<(PaneId, Option<mpsc::Sender<PaneCmd>>)> {
        let t = self
            .queues
            .get(task)
            .ok_or_else(|| refuse(ErrorCode::NotFound, format!("no task {task}")))?;
        if t.state != TaskState::Queued {
            return Err(refuse(
                ErrorCode::InvalidState,
                "the task is no longer queued",
            ));
        }
        let pane = t
            .pane_id
            .ok_or_else(|| refuse(ErrorCode::InvalidState, "a pool task waits for a free pane"))?;
        let entry = self.require(pane)?;
        if entry.pane.session_ref.is_none() {
            return Err(refuse(
                ErrorCode::InvalidState,
                "the pane's CLI has not started its session yet",
            ));
        }
        if entry.pane.status != PaneStatus::Idle || self.queues.active(pane).is_some() {
            return Err(refuse(
                ErrorCode::InvalidState,
                "the pane is not idle; the task goes when it is your turn there",
            ));
        }
        Ok((pane, entry.handle.clone()))
    }

    /// The pane a queued task waits on, for the caller to nudge after a change.
    pub fn task_pane(&self, task: TaskId) -> Option<PaneId> {
        self.queues.get(task).and_then(|t| t.pane_id)
    }

    /// The task channel of `pane`, when it has one.
    pub fn handle_of(&self, pane: PaneId) -> Option<mpsc::Sender<PaneCmd>> {
        self.panes.get(&pane).and_then(|e| e.handle.clone())
    }

    /// Stores and announces changed tasks, then drops finished history past what each workspace keeps.
    fn apply_tasks(&mut self, changed: Vec<Task>) {
        let mut finished = HashSet::new();
        for task in changed {
            if let Err(e) = self.db.update_task(&task) {
                tracing::error!(task_id = task.id, error = %e, "cannot store the task");
            }
            if matches!(
                task.state,
                TaskState::Ended | TaskState::Failed | TaskState::Cancelled
            ) {
                finished.insert(task.workspace_id);
            }
            self.emit(Event::TaskChanged(Box::new(task)));
        }
        for ws in finished {
            let pruned = self.queues.prune(ws);
            if let Err(e) = self.db.delete_tasks(&pruned) {
                tracing::error!(workspace_id = ws, error = %e, "cannot delete old finished tasks");
            }
        }
    }

    /// The workspace's tabs in bar order with their announced panes (as `pane.list` has them) in position order; a tab holding none is left out; `not_found` for an unknown workspace.
    pub fn layout(&self, workspace_id: u64) -> MethodResult<Layout> {
        self.require_workspace(workspace_id)?;
        let tabs = self
            .tabs_of(workspace_id)
            .into_iter()
            .filter_map(|t| {
                let pane_ids: Vec<PaneId> = t
                    .panes
                    .iter()
                    .copied()
                    .filter(|id| self.panes.get(id).is_some_and(|e| e.announced))
                    .collect();
                let focus_pane_id = t
                    .row
                    .focus_pane_id
                    .filter(|f| pane_ids.contains(f))
                    .or_else(|| pane_ids.first().copied());
                (!pane_ids.is_empty()).then(|| Tab {
                    id: t.row.id,
                    name: t.row.name.clone(),
                    position: t.row.position,
                    pane_ids,
                    focus_pane_id,
                    zoomed: t.row.zoomed,
                })
            })
            .collect::<Vec<_>>();
        let active_tab_id = self
            .active_tabs
            .get(&workspace_id)
            .copied()
            .filter(|id| tabs.iter().any(|t| t.id == *id));
        Ok(Layout {
            tabs,
            active_tab_id,
        })
    }

    /// Applies tab names, order, focus, zoom and pane placement; unnamed panes stay put after the named, empty tabs go.
    /// Fails with `not_found` for an unknown workspace, `bad_request` for a tab or pane outside it, a repeated pane or a tab of more than [`MAX_PANES_PER_TAB`]; a refusal changes nothing.
    pub fn save_layout(&mut self, workspace_id: u64, layout: &Layout) -> MethodResult<()> {
        self.require_workspace(workspace_id)?;
        let mut named = HashSet::new();
        for tab in &layout.tabs {
            if !self
                .tabs
                .get(&tab.id)
                .is_some_and(|t| t.row.workspace_id == workspace_id)
            {
                return Err(refuse(
                    ErrorCode::BadRequest,
                    format!("no tab {} in this workspace", tab.id),
                ));
            }
            for id in &tab.pane_ids {
                if !self
                    .panes
                    .get(id)
                    .is_some_and(|e| e.pane.workspace_id == workspace_id)
                {
                    return Err(refuse(
                        ErrorCode::BadRequest,
                        format!("no open pane {id} in this workspace"),
                    ));
                }
                if !named.insert(*id) {
                    return Err(refuse(
                        ErrorCode::BadRequest,
                        format!("pane {id} appears twice"),
                    ));
                }
            }
        }
        let store = |e: crate::Error| internal("cannot store the layout", &e);
        let mut order: Vec<&Tab> = layout.tabs.iter().collect();
        order.sort_by_key(|t| t.position);
        let mut ids: Vec<u64> = order.iter().map(|t| t.id).collect();
        for t in self.tabs_of(workspace_id) {
            if !ids.contains(&t.row.id) {
                ids.push(t.row.id);
            }
        }
        let mut lists: HashMap<u64, Vec<PaneId>> = HashMap::new();
        for id in &ids {
            let Some(tab) = self.tabs.get(id) else {
                continue;
            };
            let wanted = layout.tabs.iter().find(|t| t.id == *id);
            let mut panes: Vec<PaneId> = wanted.map(|t| t.pane_ids.clone()).unwrap_or_default();
            panes.extend(tab.panes.iter().filter(|p| !named.contains(p)));
            if panes.len() > MAX_PANES_PER_TAB {
                return Err(refuse(
                    ErrorCode::BadRequest,
                    format!(
                        "tab {id} would hold {} panes; at most {MAX_PANES_PER_TAB}",
                        panes.len()
                    ),
                ));
            }
            lists.insert(*id, panes);
        }
        for id in &ids {
            let wanted = layout.tabs.iter().find(|t| t.id == *id);
            let (Some(tab), Some(panes)) = (self.tabs.get_mut(id), lists.remove(id)) else {
                continue;
            };
            tab.panes = panes;
            if let Some(t) = wanted {
                tab.row.name.clone_from(&t.name);
                tab.row.zoomed = t.zoomed;
                if t.focus_pane_id.is_some() {
                    tab.row.focus_pane_id = t.focus_pane_id;
                }
            }
            if !tab
                .row
                .focus_pane_id
                .is_some_and(|f| tab.panes.contains(&f))
            {
                tab.row.focus_pane_id = tab.panes.first().copied();
                tab.row.zoomed = false;
            }
        }
        let mut position = 0u32;
        for id in &ids {
            if self.tabs.get(id).is_some_and(|t| t.panes.is_empty()) {
                self.drop_tab(*id);
                continue;
            }
            if let Some(tab) = self.tabs.get_mut(id) {
                tab.row.position = position;
                position += 1;
                self.db.update_tab(&tab.row).map_err(store)?;
            }
            self.renumber_tab(*id).map_err(store)?;
        }
        match layout.active_tab_id.filter(|id| self.tabs.contains_key(id)) {
            Some(active) => {
                self.active_tabs.insert(workspace_id, active);
            }
            None => {
                self.active_tabs.remove(&workspace_id);
            }
        }
        Ok(())
    }

    fn tabs_of(&self, workspace_id: u64) -> Vec<&TabState> {
        let mut tabs: Vec<&TabState> = self
            .tabs
            .values()
            .filter(|t| t.row.workspace_id == workspace_id)
            .collect();
        tabs.sort_by_key(|t| (t.row.position, t.row.id));
        tabs
    }

    fn leave_tab(&mut self, pane: &Pane) -> Result<()> {
        let Some(tab) = self.tabs.get_mut(&pane.tab_id) else {
            return Ok(());
        };
        let Some(at) = tab.panes.iter().position(|p| *p == pane.id) else {
            return Ok(());
        };
        tab.panes.remove(at);
        if tab.panes.is_empty() {
            self.drop_tab(pane.tab_id);
            return self.renumber_tabs(pane.workspace_id);
        }
        if tab.row.focus_pane_id == Some(pane.id) {
            tab.row.focus_pane_id = tab.panes.get(at.min(tab.panes.len() - 1)).copied();
            tab.row.zoomed = false;
        }
        self.db.update_tab(&tab.row)?;
        self.renumber_tab(pane.tab_id)
    }

    fn drop_tab(&mut self, id: u64) {
        if let Some(tab) = self.tabs.remove(&id) {
            if let Err(e) = self.db.delete_tab(id) {
                tracing::error!(tab_id = id, error = %e, "cannot delete an empty tab");
            }
            if self.active_tabs.get(&tab.row.workspace_id) == Some(&id) {
                self.active_tabs.remove(&tab.row.workspace_id);
            }
        }
    }

    fn renumber_tab(&mut self, tab_id: u64) -> Result<()> {
        let Some(tab) = self.tabs.get(&tab_id) else {
            return Ok(());
        };
        for (i, id) in tab.panes.clone().into_iter().enumerate() {
            let Some(entry) = self.panes.get_mut(&id) else {
                continue;
            };
            let position = count(i);
            if entry.pane.tab_id != tab_id || entry.pane.position != position {
                entry.pane.tab_id = tab_id;
                entry.pane.position = position;
                self.db.update_pane(&entry.pane)?;
            }
        }
        Ok(())
    }

    fn renumber_tabs(&mut self, workspace_id: u64) -> Result<()> {
        let ids: Vec<u64> = self
            .tabs_of(workspace_id)
            .iter()
            .map(|t| t.row.id)
            .collect();
        for (i, id) in ids.into_iter().enumerate() {
            if let Some(tab) = self.tabs.get_mut(&id)
                && tab.row.position != count(i)
            {
                tab.row.position = count(i);
                self.db.update_tab(&tab.row)?;
            }
        }
        Ok(())
    }

    /// Writes the pane's row; its `worktree_seen` keeps the last worktree reported even after the live label cleared.
    fn store(&self, pane: &Pane) {
        let recorded = self
            .panes
            .get(&pane.id)
            .and_then(|e| e.recorded_worktree.as_ref());
        let written = match (recorded, &pane.worktree_seen) {
            (Some(worktree), None) => self.db.update_pane(&Pane {
                worktree_seen: Some(worktree.clone()),
                ..pane.clone()
            }),
            _ => self.db.update_pane(pane),
        };
        if let Err(e) = written {
            tracing::error!(pane_id = pane.id, error = %e, "cannot store the pane");
        }
    }

    fn emit(&self, event: Event) {
        if self.events.send(event).is_err() {
            tracing::trace!("no C1 client is connected; event dropped");
        }
    }
}

/// The last component of a path, or the path itself for `/`; tabs and workspaces are named with it.
pub fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map_or_else(|| path.to_owned(), str::to_owned)
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use ply_proto::pane::{MAX_TASK_TEXT_BYTES, TaskPool};

    use super::*;

    fn registry() -> (Registry, broadcast::Receiver<Event>) {
        let (tx, rx) = broadcast::channel(64);
        let path = std::env::temp_dir().join(format!("ply-reg-{}.toml", std::process::id()));
        let reg = Registry::load(
            Db::open_in_memory().unwrap(),
            Config::default(),
            &path,
            tx,
            "/Users/example",
            1,
        )
        .unwrap();
        (reg, rx)
    }

    fn new_pane(tab_id: Option<u64>, cwd: &str) -> NewPane<'_> {
        NewPane {
            workspace_id: 1,
            tab_id,
            cli: Cli::Shell,
            cwd,
            title: "zsh",
            status: PaneStatus::Idle,
        }
    }

    /// A shell pane clients know, as a restored one is (no event).
    fn shell(reg: &mut Registry, tab: Option<u64>, cwd: &str) -> Pane {
        let pane = reg.insert_pane(&new_pane(tab, cwd), 10).unwrap();
        reg.panes.get_mut(&pane.id).unwrap().announced = true;
        pane
    }

    #[test]
    fn leaving_a_worktree_clears_the_live_label_but_the_session_record_keeps_it() {
        let (mut reg, _) = registry();
        let pane = shell(&mut reg, None, "/Users/example/repo");
        let meta = |cwd: &str, worktree: Option<&str>| SessionMeta {
            cwd: Some(cwd.to_owned()),
            worktree: worktree.map(str::to_owned),
            ..SessionMeta::default()
        };
        reg.set_meta(
            pane.id,
            &meta(
                "/Users/example/repo/.claude/worktrees/feat-x",
                Some("feat-x"),
            ),
        );
        reg.set_meta(pane.id, &meta("/Users/example/repo", None));
        assert_eq!(
            reg.entry(pane.id).unwrap().pane.worktree_seen,
            None,
            "the live label clears"
        );
        let stored = reg.sessions(1, true).unwrap();
        assert_eq!(stored[0].worktree_seen.as_deref(), Some("feat-x"));
    }

    #[test]
    fn the_first_start_creates_the_home_workspace() {
        let (reg, _) = registry();
        let ws = reg.workspaces();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].path, "/Users/example");
        assert_eq!(ws[0].name, "example");
    }

    #[test]
    fn panes_fill_tabs_in_order_and_tabs_are_named_after_their_first_pane() {
        let (mut reg, _) = registry();
        let a = shell(&mut reg, None, "/Users/example/code/ply");
        let b = shell(&mut reg, Some(a.tab_id), "/Users/example");
        let c = shell(&mut reg, None, "/Users/example/notes");
        assert_eq!((a.position, b.position), (0, 1));
        assert_ne!(a.tab_id, c.tab_id);
        let layout = reg.layout(1).unwrap();
        assert_eq!(layout.tabs.len(), 2);
        assert_eq!(layout.tabs[0].name, "ply");
        assert_eq!(layout.tabs[0].pane_ids, [a.id, b.id]);
        assert_eq!(layout.tabs[0].focus_pane_id, Some(a.id));
        assert_eq!(layout.tabs[1].name, "notes");
        assert_eq!(layout.active_tab_id, Some(a.tab_id));
        assert!(matches!(
            reg.insert_pane(&new_pane(Some(999), "/"), 1),
            Err(ErrorBody {
                code: ErrorCode::NotFound,
                ..
            })
        ));
    }

    #[test]
    fn closing_renumbers_moves_focus_and_drops_empty_tabs() {
        let (mut reg, mut rx) = registry();
        let a = shell(&mut reg, None, "/Users/example/ply");
        let b = shell(&mut reg, Some(a.tab_id), "/Users/example/ply");
        reg.close_pane(a.id, 20).unwrap();
        assert!(matches!(
            rx.try_recv(),
            Ok(Event::PaneRemoved(PaneRemoved { pane_id })) if pane_id == a.id
        ));
        assert_eq!(reg.entry(b.id).unwrap().pane.position, 0);
        let layout = reg.layout(1).unwrap();
        assert_eq!(layout.tabs[0].focus_pane_id, Some(b.id));
        reg.close_pane(b.id, 21).unwrap();
        assert!(reg.layout(1).unwrap().tabs.is_empty());
        let sessions = reg.sessions(1, true).unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().all(|s| s.closed_at.is_some()));
        assert!(reg.sessions(1, false).unwrap().is_empty());
    }

    #[test]
    fn a_tab_holds_at_most_four_panes() {
        let (mut reg, _) = registry();
        let a = shell(&mut reg, None, "/Users/example/one");
        for _ in 1..MAX_PANES_PER_TAB {
            shell(&mut reg, Some(a.tab_id), "/Users/example/one");
        }
        let refused = reg
            .insert_pane(&new_pane(Some(a.tab_id), "/Users/example/one"), 10)
            .unwrap_err();
        assert_eq!(refused.code, ErrorCode::TabFull);
        assert_eq!(
            reg.layout(1).unwrap().tabs[0].pane_ids.len(),
            MAX_PANES_PER_TAB
        );
        let other = shell(&mut reg, None, "/Users/example/two");
        let mut layout = reg.layout(1).unwrap();
        layout.tabs[0].pane_ids.push(other.id);
        layout.tabs.pop();
        let before = reg.layout(1).unwrap();
        assert_eq!(
            reg.save_layout(1, &layout).unwrap_err().code,
            ErrorCode::BadRequest,
            "a fifth pane moved into a full tab"
        );
        assert_eq!(reg.layout(1).unwrap(), before, "nothing changed");
        assert_eq!(reg.entry(other.id).unwrap().pane.tab_id, other.tab_id);
    }

    #[test]
    fn a_saved_layout_moves_panes_and_reorders_tabs() {
        let (mut reg, _) = registry();
        let a = shell(&mut reg, None, "/Users/example/one");
        let b = shell(&mut reg, None, "/Users/example/two");
        let mut layout = reg.layout(1).unwrap();
        layout.tabs[0].position = 1;
        layout.tabs[1].position = 0;
        layout.tabs[1].pane_ids = vec![b.id, a.id];
        layout.tabs[1].zoomed = true;
        layout.tabs[0].pane_ids = vec![];
        layout.active_tab_id = Some(b.tab_id);
        reg.save_layout(1, &layout).unwrap();
        let saved = reg.layout(1).unwrap();
        assert_eq!(saved.tabs.len(), 1, "tab one lost its only pane");
        assert_eq!(saved.tabs[0].id, b.tab_id);
        assert_eq!(saved.tabs[0].pane_ids, [b.id, a.id]);
        assert!(saved.tabs[0].zoomed);
        assert_eq!(saved.active_tab_id, Some(b.tab_id));
        assert_eq!(reg.entry(a.id).unwrap().pane.tab_id, b.tab_id);
        assert_eq!(reg.entry(a.id).unwrap().pane.position, 1);
        let mut bad = saved.clone();
        bad.tabs[0].pane_ids = vec![a.id, a.id];
        assert_eq!(
            reg.save_layout(1, &bad).unwrap_err().code,
            ErrorCode::BadRequest
        );
    }

    #[test]
    fn a_pane_is_listed_and_laid_out_only_once_announced() {
        let (mut reg, _) = registry();
        let a = reg
            .insert_pane(&new_pane(None, "/Users/example"), 10)
            .unwrap();
        assert!(reg.panes_of(1).unwrap().is_empty(), "not yet announced");
        assert!(
            reg.layout(1).unwrap().tabs.is_empty(),
            "its new tab neither"
        );
        assert!(reg.entry(a.id).is_some(), "but known to plyd itself");
        let (tx, _rx) = mpsc::channel(1);
        reg.announce(a.id, tx.clone());
        assert_eq!(reg.panes_of(1).unwrap().len(), 1);
        let b = reg
            .insert_pane(&new_pane(Some(a.tab_id), "/Users/example"), 10)
            .unwrap();
        let layout = reg.layout(1).unwrap();
        assert_eq!(layout.tabs.len(), 1);
        assert_eq!(layout.tabs[0].pane_ids, [a.id], "b is still spawning");
        assert_eq!(layout.active_tab_id, Some(a.tab_id));
        reg.announce(b.id, tx);
        assert_eq!(reg.layout(1).unwrap().tabs[0].pane_ids, [a.id, b.id]);
    }

    #[test]
    fn a_pane_reopened_as_a_shell_is_announced_again_whole() {
        let (mut reg, mut rx) = registry();
        let mut new = new_pane(None, "/Users/example");
        new.cli = Cli::Claude;
        let a = reg.insert_pane(&new, 10).unwrap();
        reg.set_cli(a.id, Cli::Shell, "zsh", 40);
        assert!(
            rx.try_recv().is_err(),
            "an unannounced pane is not announced by it"
        );
        let (tx, _handle) = mpsc::channel(1);
        reg.announce(a.id, tx);
        let _added = rx.try_recv().unwrap();
        reg.set_cli(a.id, Cli::Shell, "zsh", 40);
        assert!(matches!(
            rx.try_recv(),
            Ok(Event::PaneAdded(p)) if p.id == a.id && p.cli == Cli::Shell && p.title == "zsh"
        ));
        assert_eq!(reg.sessions(1, false).unwrap()[0].cli, Cli::Shell);
    }

    #[test]
    fn a_second_concurrent_resume_is_refused_and_a_failed_one_is_lost_again() {
        let (mut reg, _) = registry();
        let a = shell(&mut reg, None, "/Users/example");
        assert_eq!(
            reg.begin_resume(a.id, PaneStatus::Starting, 5)
                .unwrap_err()
                .code,
            ErrorCode::InvalidState,
            "a live pane is not resumed"
        );
        reg.set_status(a.id, PaneStatus::Lost, None, 6);
        let resuming = reg.begin_resume(a.id, PaneStatus::Starting, 7).unwrap();
        assert_eq!(resuming.status, PaneStatus::Starting);
        reg.set_status(a.id, PaneStatus::Lost, None, 8);
        assert_eq!(
            reg.begin_resume(a.id, PaneStatus::Starting, 9)
                .unwrap_err()
                .code,
            ErrorCode::InvalidState,
            "one resume at a time"
        );
        reg.end_resume(a.id, false, 10);
        assert_eq!(reg.entry(a.id).unwrap().pane.status, PaneStatus::Lost);
        assert!(reg.begin_resume(a.id, PaneStatus::Idle, 11).is_ok());
    }

    #[test]
    fn titles_stay_in_memory_until_the_next_row_write() {
        let (mut reg, _) = registry();
        let a = shell(&mut reg, None, "/Users/example");
        reg.set_title(a.id, "vim notes.txt", "zsh");
        assert_eq!(reg.entry(a.id).unwrap().pane.title, "vim notes.txt");
        let stored = |reg: &Registry| reg.db.open_panes().unwrap()[0].title.clone();
        assert_eq!(stored(&reg), "zsh", "a title change alone writes nothing");
        reg.touch(a.id, 20);
        assert_eq!(stored(&reg), "vim notes.txt");
    }

    /// An agent pane whose CLI has started its session (it reported a session id).
    fn agent(reg: &mut Registry, cwd: &str) -> Pane {
        let pane = fresh_agent(reg, cwd);
        reg.panes.get_mut(&pane.id).unwrap().pane.session_ref = Some("s".into());
        pane
    }

    /// An agent pane whose CLI has not reported a session yet, as during Codex's startup screens.
    fn fresh_agent(reg: &mut Registry, cwd: &str) -> Pane {
        let pane = reg
            .insert_pane(
                &NewPane {
                    cli: Cli::Claude,
                    title: "claude",
                    ..new_pane(None, cwd)
                },
                10,
            )
            .unwrap();
        reg.panes.get_mut(&pane.id).unwrap().announced = true;
        pane
    }

    #[test]
    fn a_pane_whose_cli_has_not_started_a_session_takes_no_task() {
        let (mut reg, _) = registry();
        let pane = fresh_agent(&mut reg, "/Users/example");
        let task = reg.add_task(&add(pane.id, "not yet"), 20).unwrap();
        assert_eq!(
            reg.queue_head(pane.id),
            Some(task.id),
            "offered, so the pane's dispatch can say why it waits"
        );
        assert_eq!(
            reg.take_task(pane.id, task.id, true, 21),
            None,
            "a startup screen must never get a queued Enter"
        );
        assert_eq!(
            reg.sendable(task.id).unwrap_err().code,
            ErrorCode::InvalidState
        );
    }

    fn add(pane: PaneId, text: &str) -> TaskAddParams {
        TaskAddParams {
            workspace_id: 1,
            target: TaskTarget::Pane(pane),
            text: text.to_owned(),
            skill: None,
        }
    }

    fn events(rx: &mut broadcast::Receiver<Event>) -> Vec<Event> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[test]
    fn a_task_joins_an_agent_panes_queue_and_is_stored_and_announced() {
        let (mut reg, mut rx) = registry();
        let pane = agent(&mut reg, "/Users/example");
        let first = reg.add_task(&add(pane.id, "/review-pr #1"), 20).unwrap();
        let second = reg.add_task(&add(pane.id, "then this"), 21).unwrap();
        assert_eq!(
            (first.state, first.position, first.pane_id),
            (TaskState::Queued, 0, Some(pane.id))
        );
        assert_eq!(second.position, 1);
        assert!(
            matches!(&events(&mut rx)[..], [Event::TaskChanged(a), Event::TaskChanged(b)] if a.id == first.id && b.id == second.id)
        );
        let list = reg.tasks_of(1).unwrap();
        assert_eq!(
            list.tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
            [first.id, second.id]
        );
        assert_eq!(reg.db.tasks().unwrap().len(), 2, "stored before announced");
        assert_eq!(reg.tasks_of(9).unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn task_add_refuses_what_cannot_be_queued() {
        let (mut reg, _) = registry();
        let shell_pane = shell(&mut reg, None, "/Users/example");
        let pane = agent(&mut reg, "/Users/example");
        let exited = agent(&mut reg, "/Users/example");
        reg.mark_exited(exited.id, 0, 15);
        let code = |reg: &mut Registry, p: TaskAddParams| reg.add_task(&p, 20).unwrap_err().code;
        assert_eq!(
            code(&mut reg, add(shell_pane.id, "ls")),
            ErrorCode::BadRequest,
            "a shell takes no tasks"
        );
        assert_eq!(code(&mut reg, add(exited.id, "x")), ErrorCode::InvalidState);
        assert_eq!(code(&mut reg, add(999, "x")), ErrorCode::NotFound);
        assert_eq!(
            code(
                &mut reg,
                TaskAddParams {
                    workspace_id: 9,
                    ..add(pane.id, "x")
                }
            ),
            ErrorCode::NotFound
        );
        for bad in [
            "",
            "   \n",
            "a\rb",
            "esc \u{1b}[31m",
            "nul \0",
            &"x".repeat(MAX_TASK_TEXT_BYTES + 1),
        ] {
            assert_eq!(
                code(&mut reg, add(pane.id, bad)),
                ErrorCode::BadRequest,
                "{bad:?}"
            );
        }
        assert!(
            reg.add_task(&add(pane.id, "two\nlines\twith a tab"), 20)
                .is_ok()
        );
        let pool = |cwd: &str| TaskAddParams {
            target: TaskTarget::Pool(TaskPool {
                cli: AgentCli::Codex,
                cwd: cwd.to_owned(),
            }),
            ..add(pane.id, "x")
        };
        assert_eq!(code(&mut reg, pool("relative")), ErrorCode::BadRequest);
        assert!(reg.add_task(&pool("/Users/example/project"), 20).is_ok());
        for i in 1..MAX_QUEUED_TASKS {
            reg.add_task(&add(pane.id, &format!("t{i}")), 20).unwrap();
        }
        assert_eq!(
            code(&mut reg, add(pane.id, "one too many")),
            ErrorCode::InvalidState
        );
    }

    #[test]
    fn a_workspaces_open_tasks_hold_at_most_the_text_limit() {
        use ply_proto::pane::MAX_OPEN_TASK_TEXT_BYTES;
        let (mut reg, _) = registry();
        let one = agent(&mut reg, "/Users/example");
        let two = agent(&mut reg, "/Users/example/other");
        let text = "x".repeat(MAX_TASK_TEXT_BYTES);
        for i in 0..MAX_OPEN_TASK_TEXT_BYTES / MAX_TASK_TEXT_BYTES {
            let pane = if i % 2 == 0 { one.id } else { two.id };
            reg.add_task(&add(pane, &text), 20).unwrap();
        }
        assert_eq!(
            reg.add_task(&add(two.id, "one byte too many"), 20)
                .unwrap_err()
                .code,
            ErrorCode::InvalidState
        );
    }

    #[test]
    fn cancel_move_and_pause_change_the_queue_and_announce_it() {
        let (mut reg, mut rx) = registry();
        let pane = agent(&mut reg, "/Users/example");
        let a = reg.add_task(&add(pane.id, "a"), 20).unwrap();
        let b = reg.add_task(&add(pane.id, "b"), 20).unwrap();
        events(&mut rx);
        reg.move_task(b.id, 0).unwrap();
        assert_eq!(events(&mut rx).len(), 2, "both positions changed");
        reg.cancel_task(b.id, 30).unwrap();
        let got = events(&mut rx);
        assert!(
            matches!(&got[0], Event::TaskChanged(t) if t.id == b.id && t.state == TaskState::Cancelled)
        );
        assert!(matches!(&got[1], Event::TaskChanged(t) if t.id == a.id && t.position == 0));
        reg.pause_queue(pane.id, true).unwrap();
        reg.pause_queue(pane.id, true).unwrap();
        let got = events(&mut rx);
        assert_eq!(got.len(), 1, "an unchanged pause sends nothing");
        assert!(matches!(&got[0], Event::QueueChanged(q) if q.paused == Some(PauseReason::User)));
        assert_eq!(reg.tasks_of(1).unwrap().queues.len(), 1);
        reg.pause_queue(pane.id, false).unwrap();
        assert!(reg.tasks_of(1).unwrap().queues.is_empty());
        assert_eq!(
            reg.pause_queue(999, true).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn a_pane_task_takes_its_head_only_while_idle_and_its_queue_runs() {
        let (mut reg, mut rx) = registry();
        let pane = agent(&mut reg, "/Users/example");
        let a = reg.add_task(&add(pane.id, "first"), 20).unwrap();
        let b = reg.add_task(&add(pane.id, "second"), 20).unwrap();
        assert_eq!(reg.queue_head(pane.id), Some(a.id));
        reg.set_status(pane.id, PaneStatus::Running, None, 21);
        assert_eq!(
            reg.take_task(pane.id, a.id, false, 21),
            None,
            "never into a busy pane"
        );
        reg.set_status(pane.id, PaneStatus::Idle, None, 22);
        assert_eq!(
            reg.take_task(pane.id, b.id, false, 22),
            None,
            "only the head, unless sent by the user"
        );
        events(&mut rx);
        assert_eq!(
            reg.take_task(pane.id, a.id, false, 23).as_deref(),
            Some("first")
        );
        let got = events(&mut rx);
        assert!(
            matches!(&got[0], Event::TaskChanged(t) if t.id == a.id && t.state == TaskState::Sent && t.sent_at == Some(23))
        );
        assert_eq!(reg.queue_head(pane.id), None, "one typed task at a time");
        assert_eq!(reg.take_task(pane.id, b.id, true, 24), None);
        reg.task_progress(a.id, TaskState::Running, None, 25);
        reg.task_progress(a.id, TaskState::Ended, None, 30);
        assert_eq!(reg.queue_head(pane.id), Some(b.id));
        reg.pause_queue(pane.id, true).unwrap();
        assert_eq!(reg.queue_head(pane.id), None);
        assert_eq!(reg.take_task(pane.id, b.id, false, 31), None);
        assert_eq!(
            reg.take_task(pane.id, b.id, true, 31).as_deref(),
            Some("second"),
            "task.send passes a pause"
        );
    }

    #[test]
    fn an_idle_pane_with_an_empty_running_queue_takes_a_pool_task() {
        let (mut reg, mut rx) = registry();
        let pane = agent(&mut reg, "/Users/example/project/sub");
        let pool = |cli: AgentCli, cwd: &str| TaskAddParams {
            target: TaskTarget::Pool(TaskPool {
                cli,
                cwd: cwd.to_owned(),
            }),
            ..add(pane.id, "pooled")
        };
        let codex = reg
            .add_task(&pool(AgentCli::Codex, "/Users/example"), 20)
            .unwrap();
        let task = reg
            .add_task(&pool(AgentCli::Claude, "/Users/example/project"), 20)
            .unwrap();
        events(&mut rx);
        assert_eq!(
            reg.next_task(pane.id, false),
            None,
            "no claim while the user's input is typed"
        );
        reg.pause_queue(pane.id, true).unwrap();
        assert_eq!(
            reg.next_task(pane.id, true),
            None,
            "a paused queue takes nothing"
        );
        reg.pause_queue(pane.id, false).unwrap();
        events(&mut rx);
        assert_eq!(reg.next_task(pane.id, true), Some(task.id));
        let got = events(&mut rx);
        assert!(
            matches!(&got[0], Event::TaskChanged(t) if t.id == task.id && t.pane_id == Some(pane.id))
        );
        let stored = reg.db.tasks().unwrap();
        assert_eq!(
            stored.iter().find(|t| t.id == task.id).unwrap().pane_id,
            Some(pane.id)
        );
        assert_eq!(
            reg.next_task(pane.id, true),
            Some(task.id),
            "its own head first"
        );
        let list = reg.tasks_of(1).unwrap();
        assert_eq!(
            list.tasks
                .iter()
                .find(|t| t.id == codex.id)
                .unwrap()
                .pane_id,
            None
        );
        assert_eq!(reg.pool_panes(1, AgentCli::Claude), vec![pane.id]);
        assert!(reg.pool_panes(1, AgentCli::Codex).is_empty());
    }

    #[test]
    fn a_failed_task_pauses_its_queue_and_a_block_is_announced() {
        let (mut reg, mut rx) = registry();
        let pane = agent(&mut reg, "/Users/example");
        let a = reg.add_task(&add(pane.id, "first"), 20).unwrap();
        reg.add_task(&add(pane.id, "second"), 20).unwrap();
        reg.take_task(pane.id, a.id, false, 21).unwrap();
        events(&mut rx);
        reg.task_progress(a.id, TaskState::Failed, Some("not submitted".into()), 31);
        let got = events(&mut rx);
        assert!(
            matches!(&got[0], Event::TaskChanged(t) if t.state == TaskState::Failed && t.detail.as_deref() == Some("not submitted"))
        );
        assert!(matches!(&got[1], Event::QueueChanged(q) if q.paused == Some(PauseReason::Failed)));
        assert_eq!(
            reg.queue_head(pane.id),
            None,
            "nothing more is typed until the user resumes"
        );
        reg.block_queue(pane.id, Some(BlockReason::Typing));
        reg.block_queue(pane.id, Some(BlockReason::Typing));
        let got = events(&mut rx);
        assert_eq!(got.len(), 1);
        assert!(
            matches!(&got[0], Event::QueueChanged(q) if q.blocked == Some(BlockReason::Typing))
        );
    }

    #[test]
    fn task_send_needs_a_queued_task_of_an_idle_pane() {
        let (mut reg, _) = registry();
        let pane = agent(&mut reg, "/Users/example");
        let a = reg.add_task(&add(pane.id, "first"), 20).unwrap();
        assert_eq!(reg.sendable(a.id).map(|(p, _)| p), Ok(pane.id));
        reg.set_status(pane.id, PaneStatus::Running, None, 21);
        assert_eq!(
            reg.sendable(a.id).unwrap_err().code,
            ErrorCode::InvalidState
        );
        reg.set_status(pane.id, PaneStatus::Idle, None, 22);
        reg.take_task(pane.id, a.id, false, 22).unwrap();
        assert_eq!(
            reg.sendable(a.id).unwrap_err().code,
            ErrorCode::InvalidState,
            "already typed"
        );
        assert_eq!(reg.sendable(999).unwrap_err().code, ErrorCode::NotFound);
        let pooled = reg
            .add_task(
                &TaskAddParams {
                    target: TaskTarget::Pool(TaskPool {
                        cli: AgentCli::Claude,
                        cwd: "/Users/example".into(),
                    }),
                    ..add(pane.id, "x")
                },
                23,
            )
            .unwrap();
        assert_eq!(
            reg.sendable(pooled.id).unwrap_err().code,
            ErrorCode::InvalidState
        );
    }

    #[test]
    fn a_pane_reopened_as_a_shell_cancels_its_queued_tasks() {
        let (mut reg, mut rx) = registry();
        let pane = agent(&mut reg, "/Users/example");
        let task = reg.add_task(&add(pane.id, "never typed"), 11).unwrap();
        events(&mut rx);
        reg.set_cli(pane.id, Cli::Shell, "zsh", 40);
        let list = reg.tasks_of(1).unwrap();
        assert_eq!(list.tasks[0].id, task.id);
        assert_eq!(list.tasks[0].state, TaskState::Cancelled);
        assert_eq!(
            list.tasks[0].detail.as_deref(),
            Some(crate::panes::queue::REOPENED_AS_SHELL)
        );
    }

    #[test]
    fn closing_a_pane_cancels_its_queued_tasks() {
        let (mut reg, mut rx) = registry();
        let pane = agent(&mut reg, "/Users/example");
        reg.add_task(&add(pane.id, "never typed"), 11).unwrap();
        reg.mark_exited(pane.id, 0, 12);
        events(&mut rx);
        reg.close_pane(pane.id, 30).unwrap();
        let got = events(&mut rx);
        assert!(
            got.iter()
                .any(|e| matches!(e, Event::TaskChanged(t) if t.state == TaskState::Cancelled))
        );
        assert!(
            reg.db
                .tasks()
                .unwrap()
                .iter()
                .all(|t| t.state == TaskState::Cancelled)
        );
    }

    #[test]
    fn a_restart_holds_the_queues_of_the_panes_that_come_back() {
        let db = Db::open_in_memory().unwrap();
        let ws = db.insert_workspace("/Users/example", "example", 1).unwrap();
        let tab = db
            .insert_tab(&TabRow {
                id: 0,
                workspace_id: ws.id,
                name: "example".into(),
                position: 0,
                focus_pane_id: None,
                zoomed: false,
            })
            .unwrap();
        let pane_id = db
            .insert_pane(&Pane {
                id: 0,
                workspace_id: ws.id,
                tab_id: tab,
                position: 0,
                cli: Cli::Claude,
                cwd: "/Users/example".into(),
                title: "claude".into(),
                status: PaneStatus::Idle,
                detail: None,
                progress: None,
                model_seen: None,
                worktree_seen: None,
                branch: None,
                project: None,
                git_worktree: None,
                session_ref: Some("s".into()),
                exit_code: None,
                created_at: 1,
                closed_at: None,
                last_activity_at: None,
            })
            .unwrap();
        let base = Task {
            id: 0,
            workspace_id: ws.id,
            pane_id: Some(pane_id),
            pool: None,
            text: "x".into(),
            skill: None,
            state: TaskState::Queued,
            position: 0,
            detail: None,
            created_at: 2,
            sent_at: None,
            started_at: None,
            ended_at: None,
        };
        let running = db
            .insert_task(&Task {
                state: TaskState::Running,
                ..base.clone()
            })
            .unwrap();
        let waiting = db
            .insert_task(&Task {
                position: 1,
                ..base
            })
            .unwrap();
        let (tx, _rx) = broadcast::channel(16);
        let path =
            std::env::temp_dir().join(format!("ply-reg-restart-{}.toml", std::process::id()));
        let reg = Registry::load(db, Config::default(), &path, tx, "/Users/example", 50).unwrap();
        let list = reg.tasks_of(ws.id).unwrap();
        let state = |id| list.tasks.iter().find(|t| t.id == id).unwrap().clone();
        assert_eq!(state(running).state, TaskState::Failed);
        assert_eq!(
            (state(waiting).state, state(waiting).position),
            (TaskState::Queued, 0)
        );
        assert_eq!(list.queues[0].paused, Some(PauseReason::Restored));
        assert_eq!(
            reg.db
                .tasks()
                .unwrap()
                .iter()
                .find(|t| t.id == running)
                .unwrap()
                .state,
            TaskState::Failed
        );
    }

    #[test]
    fn exit_and_status_changes_are_broadcast_in_order() {
        let (mut reg, mut rx) = registry();
        let a = shell(&mut reg, None, "/Users/example");
        reg.set_status(a.id, PaneStatus::Running, None, 30);
        reg.set_status(a.id, PaneStatus::Running, None, 31);
        assert!(!reg.mark_exited(a.id, 3, 32));
        let got: Vec<Event> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(got.len(), 3, "{got:?}");
        assert!(
            matches!(&got[1], Event::PaneStatus(s) if s.status == PaneStatus::Exited && s.exit_code == Some(3))
        );
        assert!(matches!(&got[2], Event::PaneExit(e) if e.code == 3));
        assert_eq!(reg.running_count(), 0);
    }
}
