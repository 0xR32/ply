//! The pane registry: workspaces, tabs (order, focus, zoom), panes and their tasks, mirrored to SQLite.
//!
//! Every C1 method reads or changes state here, under one lock held only for in-memory work and a few short SQLite
//! writes. Pane ids are the `panes` row ids, so they never repeat and double as the C2 `pane_id` (spec 4.2). A pane's
//! `position` is its index in its tab (0 is the main pane); positions are renumbered whenever a pane leaves a tab,
//! and a tab disappears with its last open pane. Tabs are named after the basename of their first pane's directory
//! (Ruling R3); one default workspace, the home directory, exists after the first start. Events for C1
//! (`pane.added`, `pane.removed`, `pane.status`, `pane.meta`, `pane.exit`) are broadcast from here, after the change
//! is stored. `layout.save`'s `active_tab_id` has no column in schema v1, so it is kept in memory only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use ply_proto::control::{
    ErrorBody, ErrorCode, Event, PaneExit, PaneMeta, PaneRemoved, PaneStatusChanged,
};
use ply_proto::pane::{
    Cli, Layout, Pane, PaneId, PaneStatus, Session, Settings, Tab, TerminalTheme, UnixSeconds,
    Workspace,
};
use tokio::sync::{broadcast, mpsc};

use crate::config::Config;
use crate::db::{Db, TabRow};
use crate::error::Result;
use crate::panes::pane::PaneCmd;

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

    /// Replaces the settings and writes `config.toml`; fails with `internal` if the file cannot be written.
    pub fn set_settings(&mut self, settings: Settings) -> MethodResult<()> {
        self.config.settings = settings;
        self.save_config()
    }

    /// The last `theme.set` palette.
    pub fn palette(&self) -> Option<&TerminalTheme> {
        self.config.palette.as_ref()
    }

    /// Stores the palette in `config.toml`; fails with `internal` if the file cannot be written.
    pub fn set_palette(&mut self, theme: TerminalTheme) -> MethodResult<()> {
        self.config.palette = Some(theme);
        self.save_config()
    }

    fn save_config(&self) -> MethodResult<()> {
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
    /// Fails with `not_found` for an unknown workspace or tab, `internal` if SQLite fails.
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

    /// Ends a `pane.resume`; a failed one puts the pane back to `lost`.
    pub fn end_resume(&mut self, id: PaneId, started: bool, now: UnixSeconds) {
        if let Some(entry) = self.panes.get_mut(&id) {
            entry.resuming = false;
        }
        if !started {
            self.set_status(id, PaneStatus::Lost, None, now);
        }
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

    /// Records a reported working directory and broadcasts `pane.meta`.
    pub fn set_cwd(&mut self, id: PaneId, cwd: &str) {
        let Some(entry) = self.panes.get_mut(&id) else {
            return;
        };
        if entry.pane.cwd == cwd {
            return;
        }
        entry.pane.cwd = cwd.to_owned();
        let pane = entry.pane.clone();
        self.store(&pane);
        self.emit(Event::PaneMeta(PaneMeta {
            pane_id: id,
            model: pane.model_seen,
            worktree: pane.worktree_seen,
            cwd: pane.cwd,
            branch: None,
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
        self.emit(Event::PaneRemoved(PaneRemoved { pane_id: id }));
        Ok(entry.handle)
    }

    /// The workspace's tabs in bar order with their panes in position order.
    /// Fails with `not_found` for an unknown workspace.
    pub fn layout(&self, workspace_id: u64) -> MethodResult<Layout> {
        self.require_workspace(workspace_id)?;
        let tabs = self
            .tabs_of(workspace_id)
            .into_iter()
            .map(|t| Tab {
                id: t.row.id,
                name: t.row.name.clone(),
                position: t.row.position,
                pane_ids: t.panes.clone(),
                focus_pane_id: t.row.focus_pane_id,
                zoomed: t.row.zoomed,
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
    /// Fails with `not_found` for an unknown workspace, `bad_request` for a tab or pane outside it or a repeated pane.
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
        for id in &ids {
            let wanted = layout.tabs.iter().find(|t| t.id == *id);
            let Some(tab) = self.tabs.get_mut(id) else {
                continue;
            };
            let mut panes: Vec<PaneId> = wanted.map(|t| t.pane_ids.clone()).unwrap_or_default();
            panes.extend(tab.panes.iter().filter(|p| !named.contains(p)));
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

    fn store(&self, pane: &Pane) {
        if let Err(e) = self.db.update_pane(pane) {
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

    fn shell(reg: &mut Registry, tab: Option<u64>, cwd: &str) -> Pane {
        reg.insert_pane(&new_pane(tab, cwd), 10).unwrap()
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
    fn a_pane_is_listed_only_once_announced() {
        let (mut reg, _) = registry();
        let a = shell(&mut reg, None, "/Users/example");
        assert!(reg.panes_of(1).unwrap().is_empty(), "not yet announced");
        assert!(reg.entry(a.id).is_some(), "but known to plyd itself");
        let (tx, _rx) = mpsc::channel(1);
        reg.announce(a.id, tx);
        assert_eq!(reg.panes_of(1).unwrap().len(), 1);
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
