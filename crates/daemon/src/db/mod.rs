//! The session store (spec 11.2): SQLite schema v2 with forward-only migrations.
//!
//! Migrations are the numbered files in `db/migrations/`, compiled in; [`Db::open`] applies every one above the
//! stored `schema_version` in a transaction and records the new version. A database whose version is newer than
//! [`SCHEMA_VERSION`] was written by a newer plyd, and opening it fails with [`Error::SchemaTooNew`] so plyd refuses
//! to start instead of downgrading it (spec 15). Schema v1 is exactly 11.2's; `worktree_seen` is the only worktree
//! column and holds what the CLI reported (Ruling R5, INV-7). Pane rows are kept after the pane closes (`closed_at`),
//! so `session.list {include_closed:true}` returns finished sessions (F3). Schema v2 adds `tasks`, the task queue of
//! Ruling R60: every task with its target, text, state and times; a queue's pause is not stored. A [`Db`] is not `Sync`; plyd keeps it
//! behind the registry's lock, and every call blocks briefly on the local file.

use std::path::Path;

use ply_proto::pane::{
    AgentCli, Cli, Pane, PaneId, PaneStatus, Session, Task, TaskId, TaskPool, TaskState,
    UnixSeconds, Workspace,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::error::{Error, Result};

/// The schema version this build writes and the newest it opens.
pub const SCHEMA_VERSION: i64 = 2;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("migrations/0001_init.sql")),
    (2, include_str!("migrations/0002_tasks.sql")),
];

const TASK_COLUMNS: &str = "id, workspace_id, pane_id, pool_cli, pool_cwd, text, skill, state, position, detail, \
     created_at, sent_at, started_at, ended_at";

const PANE_COLUMNS: &str = "id, workspace_id, tab_id, position, cli, cwd, model_seen, worktree_seen, exit_code, \
     last_activity_at, session_ref, title, status, created_at, closed_at";

/// One `tabs` row as stored; C1 `Tab` is built from it and the tab's open panes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabRow {
    /// SQLite rowid, the C1 `Tab.id`; ignored on insert, which assigns it.
    pub id: u64,
    /// The `workspaces.id` the tab belongs to; a tab never moves between workspaces.
    pub workspace_id: u64,
    /// Label in the tab bar; ply names a new tab after its first pane's directory.
    pub name: String,
    /// 0-based order in the tab bar.
    pub position: u32,
    /// The focused pane as `layout.save` last stored it (else the tab's first pane); `None` while the tab has none.
    pub focus_pane_id: Option<PaneId>,
    /// Whether the focused pane fills the tab.
    pub zoomed: bool,
}

/// An open SQLite connection with the schema migrated to [`SCHEMA_VERSION`].
#[derive(Debug)]
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Opens or creates the database at `path` (WAL journal, `synchronous=NORMAL`) and migrates it; errors: [`Error::SchemaTooNew`] for a newer database, else [`Error::Db`].
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // In WAL mode NORMAL syncs at checkpoints, not per commit; a crash can lose the last commits, never corrupt.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    /// A migrated database in memory, for tests; errors: [`Error::Db`] only if SQLite does.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<()> {
        let current = self.schema_version()?;
        if current > SCHEMA_VERSION {
            return Err(Error::SchemaTooNew {
                found: current,
                supported: SCHEMA_VERSION,
            });
        }
        for (version, sql) in MIGRATIONS.iter().filter(|(v, _)| *v > current) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute("DELETE FROM schema_version", [])?;
            tx.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                [version],
            )?;
            tx.commit()?;
            tracing::info!(version, "database migrated");
        }
        Ok(())
    }

    /// The stored `schema_version`; 0 for a database without the table (a new file).
    /// Fails with [`Error::Db`].
    pub fn schema_version(&self) -> Result<i64> {
        let exists: Option<String> = self
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Ok(0);
        }
        let version: Option<i64> =
            self.conn
                .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))?;
        Ok(version.unwrap_or(0))
    }

    /// Every workspace, by id.
    /// Fails with [`Error::Db`].
    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, name, opened_at FROM workspaces ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(Workspace {
                id: r.get(0)?,
                path: r.get(1)?,
                name: r.get(2)?,
                opened_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Inserts a workspace; the path must be new.
    /// Fails with [`Error::Db`] (a duplicate path violates the UNIQUE constraint).
    pub fn insert_workspace(
        &self,
        path: &str,
        name: &str,
        opened_at: UnixSeconds,
    ) -> Result<Workspace> {
        self.conn.execute(
            "INSERT INTO workspaces (path, name, opened_at) VALUES (?1, ?2, ?3)",
            params![path, name, opened_at],
        )?;
        Ok(Workspace {
            id: self.last_id()?,
            path: path.to_owned(),
            name: name.to_owned(),
            opened_at,
        })
    }

    /// Records that a workspace was opened again.
    /// Fails with [`Error::Db`].
    pub fn touch_workspace(&self, id: u64, opened_at: UnixSeconds) -> Result<()> {
        self.conn.execute(
            "UPDATE workspaces SET opened_at = ?2 WHERE id = ?1",
            params![id, opened_at],
        )?;
        Ok(())
    }

    /// Every tab, by workspace and position.
    /// Fails with [`Error::Db`].
    pub fn tabs(&self) -> Result<Vec<TabRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, workspace_id, name, position, focus_pane_id, zoomed FROM tabs \
             ORDER BY workspace_id, position, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TabRow {
                id: r.get(0)?,
                workspace_id: r.get(1)?,
                name: r.get(2)?,
                position: r.get(3)?,
                focus_pane_id: r.get(4)?,
                zoomed: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Inserts a tab and returns its id; `tab.id` is ignored.
    /// Fails with [`Error::Db`] (an unknown workspace violates the foreign key).
    pub fn insert_tab(&self, tab: &TabRow) -> Result<u64> {
        self.conn.execute(
            "INSERT INTO tabs (workspace_id, name, position, focus_pane_id, zoomed) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                tab.workspace_id,
                tab.name,
                tab.position,
                tab.focus_pane_id,
                tab.zoomed
            ],
        )?;
        self.last_id()
    }

    /// Writes every column of an existing tab.
    /// Fails with [`Error::Db`].
    pub fn update_tab(&self, tab: &TabRow) -> Result<()> {
        self.conn.execute(
            "UPDATE tabs SET name = ?2, position = ?3, focus_pane_id = ?4, zoomed = ?5 WHERE id = ?1",
            params![
                tab.id,
                tab.name,
                tab.position,
                tab.focus_pane_id,
                tab.zoomed
            ],
        )?;
        Ok(())
    }

    /// Deletes a tab that no open pane uses any more.
    /// Fails with [`Error::Db`].
    pub fn delete_tab(&self, id: u64) -> Result<()> {
        self.conn.execute("DELETE FROM tabs WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Every pane without `closed_at`, by id.
    /// Fails with [`Error::Db`] or [`Error::BadRow`].
    pub fn open_panes(&self) -> Result<Vec<Pane>> {
        let sql = format!("SELECT {PANE_COLUMNS} FROM panes WHERE closed_at IS NULL ORDER BY id");
        self.panes_where(&sql, [])
    }

    /// The stored record of every pane of a workspace, open ones first by id, then closed ones when asked for.
    /// Fails with [`Error::Db`] or [`Error::BadRow`].
    pub fn sessions(&self, workspace_id: u64, include_closed: bool) -> Result<Vec<Session>> {
        let filter = if include_closed {
            ""
        } else {
            " AND closed_at IS NULL"
        };
        let sql = format!(
            "SELECT {PANE_COLUMNS} FROM panes WHERE workspace_id = ?1{filter} \
             ORDER BY closed_at IS NOT NULL, id"
        );
        Ok(self
            .panes_where(&sql, [workspace_id])?
            .into_iter()
            .map(session_of)
            .collect())
    }

    fn panes_where<P: rusqlite::Params>(&self, sql: &str, params: P) -> Result<Vec<Pane>> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(pane_of(row)?);
        }
        Ok(out)
    }

    /// Inserts a pane (its `id` is ignored) and returns the new id, which is also its C2 `pane_id`.
    /// Fails with [`Error::Db`].
    pub fn insert_pane(&self, pane: &Pane) -> Result<PaneId> {
        self.conn.execute(
            "INSERT INTO panes (workspace_id, tab_id, position, cli, cwd, model_seen, worktree_seen, exit_code, \
             last_activity_at, session_ref, title, status, created_at, closed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                pane.workspace_id,
                pane.tab_id,
                pane.position,
                cli_str(pane.cli),
                pane.cwd,
                pane.model_seen,
                pane.worktree_seen,
                pane.exit_code,
                pane.last_activity_at,
                pane.session_ref,
                pane.title,
                status_str(pane.status),
                pane.created_at,
                pane.closed_at,
            ],
        )?;
        self.last_id()
    }

    /// Writes every mutable column of an existing pane (`cli` too: a resumed pane without a session becomes a shell).
    /// Fails with [`Error::Db`].
    pub fn update_pane(&self, pane: &Pane) -> Result<()> {
        self.conn.execute(
            "UPDATE panes SET tab_id = ?2, position = ?3, cwd = ?4, model_seen = ?5, worktree_seen = ?6, \
             exit_code = ?7, last_activity_at = ?8, session_ref = ?9, title = ?10, status = ?11, closed_at = ?12, \
             cli = ?13 WHERE id = ?1",
            params![
                pane.id,
                pane.tab_id,
                pane.position,
                pane.cwd,
                pane.model_seen,
                pane.worktree_seen,
                pane.exit_code,
                pane.last_activity_at,
                pane.session_ref,
                pane.title,
                status_str(pane.status),
                pane.closed_at,
                cli_str(pane.cli),
            ],
        )?;
        Ok(())
    }

    /// Deletes a pane row outright; only for a pane whose process never started.
    /// Fails with [`Error::Db`].
    pub fn delete_pane(&self, id: PaneId) -> Result<()> {
        self.conn.execute("DELETE FROM panes WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Every stored task, by id; plyd loads them once at startup.
    /// Fails with [`Error::Db`] or [`Error::BadRow`].
    pub fn tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {TASK_COLUMNS} FROM tasks ORDER BY id"))?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(task_of(row)?);
        }
        Ok(out)
    }

    /// Inserts a task (its `id` is ignored) and returns the new id.
    /// Fails with [`Error::Db`].
    pub fn insert_task(&self, task: &Task) -> Result<TaskId> {
        let (pool_cli, pool_cwd) = pool_columns(task);
        self.conn.execute(
            "INSERT INTO tasks (workspace_id, pane_id, pool_cli, pool_cwd, text, skill, state, position, detail, \
             created_at, sent_at, started_at, ended_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                task.workspace_id,
                task.pane_id,
                pool_cli,
                pool_cwd,
                task.text,
                task.skill,
                task_state_str(task.state),
                task.position,
                task.detail,
                task.created_at,
                task.sent_at,
                task.started_at,
                task.ended_at,
            ],
        )?;
        self.last_id()
    }

    /// Writes every mutable column of an existing task: its pane (a pool task taken by a pane), state, place, detail and times.
    /// Fails with [`Error::Db`].
    pub fn update_task(&self, task: &Task) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET pane_id = ?2, state = ?3, position = ?4, detail = ?5, sent_at = ?6, started_at = ?7, \
             ended_at = ?8 WHERE id = ?1",
            params![
                task.id,
                task.pane_id,
                task_state_str(task.state),
                task.position,
                task.detail,
                task.sent_at,
                task.started_at,
                task.ended_at,
            ],
        )?;
        Ok(())
    }

    /// Deletes the tasks `ids`, the finished ones past the history plyd keeps.
    /// Fails with [`Error::Db`].
    pub fn delete_tasks(&self, ids: &[TaskId]) -> Result<()> {
        for id in ids {
            self.conn.execute("DELETE FROM tasks WHERE id = ?1", [id])?;
        }
        Ok(())
    }

    fn last_id(&self) -> Result<u64> {
        u64::try_from(self.conn.last_insert_rowid()).map_err(|_| Error::BadRow {
            what: "row id",
            value: self.conn.last_insert_rowid().to_string(),
        })
    }
}

fn pane_of(r: &Row<'_>) -> Result<Pane> {
    let cli: String = r.get(4)?;
    let status: String = r.get(12)?;
    Ok(Pane {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        tab_id: r.get(2)?,
        position: r.get(3)?,
        cli: parse_cli(&cli)?,
        cwd: r.get(5)?,
        model_seen: r.get(6)?,
        worktree_seen: r.get(7)?,
        exit_code: r.get(8)?,
        last_activity_at: r.get(9)?,
        session_ref: r.get(10)?,
        title: r.get(11)?,
        status: parse_status(&status)?,
        created_at: r.get(13)?,
        closed_at: r.get(14)?,
        detail: None,
        progress: None,
        branch: None,
        project: None,
        git_worktree: None,
    })
}

fn pool_columns(task: &Task) -> (Option<&'static str>, Option<&str>) {
    match &task.pool {
        Some(pool) => (Some(agent_cli_str(pool.cli)), Some(pool.cwd.as_str())),
        None => (None, None),
    }
}

fn task_of(r: &Row<'_>) -> Result<Task> {
    let pool_cli: Option<String> = r.get(3)?;
    let pool_cwd: Option<String> = r.get(4)?;
    let pool = match (pool_cli, pool_cwd) {
        (Some(cli), Some(cwd)) => Some(TaskPool {
            cli: parse_agent_cli(&cli)?,
            cwd,
        }),
        _ => None,
    };
    let state: String = r.get(7)?;
    Ok(Task {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        pane_id: r.get(2)?,
        pool,
        text: r.get(5)?,
        skill: r.get(6)?,
        state: parse_task_state(&state)?,
        position: r.get(8)?,
        detail: r.get(9)?,
        created_at: r.get(10)?,
        sent_at: r.get(11)?,
        started_at: r.get(12)?,
        ended_at: r.get(13)?,
    })
}

fn agent_cli_str(cli: AgentCli) -> &'static str {
    match cli {
        AgentCli::Claude => "claude",
        AgentCli::Codex => "codex",
    }
}

fn parse_agent_cli(s: &str) -> Result<AgentCli> {
    match s {
        "claude" => Ok(AgentCli::Claude),
        "codex" => Ok(AgentCli::Codex),
        _ => Err(Error::BadRow {
            what: "pool_cli",
            value: s.to_owned(),
        }),
    }
}

/// The `tasks.state` value of a state, its C1 wire name.
pub fn task_state_str(state: TaskState) -> &'static str {
    match state {
        TaskState::Queued => "queued",
        TaskState::Sent => "sent",
        TaskState::Running => "running",
        TaskState::Ended => "ended",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
    }
}

fn parse_task_state(s: &str) -> Result<TaskState> {
    Ok(match s {
        "queued" => TaskState::Queued,
        "sent" => TaskState::Sent,
        "running" => TaskState::Running,
        "ended" => TaskState::Ended,
        "failed" => TaskState::Failed,
        "cancelled" => TaskState::Cancelled,
        _ => {
            return Err(Error::BadRow {
                what: "task state",
                value: s.to_owned(),
            });
        }
    })
}

/// A pane as `session.list` reports it: the stored columns, without the live-only progress and branch.
pub fn session_of(p: Pane) -> Session {
    Session {
        pane_id: p.id,
        workspace_id: p.workspace_id,
        cli: p.cli,
        cwd: p.cwd,
        title: p.title,
        status: p.status,
        session_ref: p.session_ref,
        model_seen: p.model_seen,
        worktree_seen: p.worktree_seen,
        exit_code: p.exit_code,
        created_at: p.created_at,
        closed_at: p.closed_at,
        last_activity_at: p.last_activity_at,
    }
}

/// The `panes.cli` column value, which is also the CLI's C1 wire name.
pub fn cli_str(cli: Cli) -> &'static str {
    match cli {
        Cli::Claude => "claude",
        Cli::Codex => "codex",
        Cli::Shell => "shell",
    }
}

fn parse_cli(s: &str) -> Result<Cli> {
    match s {
        "claude" => Ok(Cli::Claude),
        "codex" => Ok(Cli::Codex),
        "shell" => Ok(Cli::Shell),
        _ => Err(Error::BadRow {
            what: "cli",
            value: s.to_owned(),
        }),
    }
}

/// The `panes.status` value of a state, its C1 wire name.
pub fn status_str(status: PaneStatus) -> &'static str {
    match status {
        PaneStatus::Starting => "starting",
        PaneStatus::Idle => "idle",
        PaneStatus::Running => "running",
        PaneStatus::WaitingPermission => "waiting_permission",
        PaneStatus::WaitingInput => "waiting_input",
        PaneStatus::Exited => "exited",
        PaneStatus::Lost => "lost",
    }
}

fn parse_status(s: &str) -> Result<PaneStatus> {
    Ok(match s {
        "starting" => PaneStatus::Starting,
        "idle" => PaneStatus::Idle,
        "running" => PaneStatus::Running,
        "waiting_permission" => PaneStatus::WaitingPermission,
        "waiting_input" => PaneStatus::WaitingInput,
        "exited" => PaneStatus::Exited,
        "lost" => PaneStatus::Lost,
        _ => {
            return Err(Error::BadRow {
                what: "status",
                value: s.to_owned(),
            });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(workspace_id: u64, tab_id: u64) -> Pane {
        Pane {
            id: 0,
            workspace_id,
            tab_id,
            position: 0,
            cli: Cli::Shell,
            cwd: "/Users/example".into(),
            title: "zsh".into(),
            status: PaneStatus::Idle,
            detail: None,
            progress: None,
            model_seen: None,
            worktree_seen: None,
            branch: None,
            project: None,
            git_worktree: None,
            session_ref: None,
            exit_code: None,
            created_at: 100,
            closed_at: None,
            last_activity_at: None,
        }
    }

    #[test]
    fn a_new_database_is_at_schema_v2_with_the_11_2_tables_and_tasks() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), 2);
        assert!(db.tasks().unwrap().is_empty());
        let ws = db.insert_workspace("/Users/example", "example", 1).unwrap();
        let tab = TabRow {
            id: 0,
            workspace_id: ws.id,
            name: "example".into(),
            position: 0,
            focus_pane_id: None,
            zoomed: false,
        };
        let tab_id = db.insert_tab(&tab).unwrap();
        let id = db.insert_pane(&pane(ws.id, tab_id)).unwrap();
        let mut stored = db.open_panes().unwrap().remove(0);
        assert_eq!(stored.id, id);
        stored.status = PaneStatus::Exited;
        stored.exit_code = Some(3);
        stored.closed_at = Some(200);
        db.update_pane(&stored).unwrap();
        assert!(db.open_panes().unwrap().is_empty());
        assert!(db.sessions(ws.id, false).unwrap().is_empty());
        let sessions = db.sessions(ws.id, true).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].exit_code, Some(3));
        assert_eq!(sessions[0].status, PaneStatus::Exited);
    }

    #[test]
    fn the_cli_column_rejects_unknown_programs() {
        let db = Db::open_in_memory().unwrap();
        let ws = db.insert_workspace("/Users/example", "example", 1).unwrap();
        let bad = db.conn.execute(
            "INSERT INTO panes (workspace_id, tab_id, position, cli, cwd, title, status, created_at) \
             VALUES (?1, 1, 0, 'vim', '/', 'vim', 'idle', 1)",
            [ws.id],
        );
        assert!(bad.is_err());
    }

    #[test]
    fn a_newer_schema_version_is_refused() {
        let dir = std::env::temp_dir().join(format!("ply-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ply.db");
        let db = Db::open(&path).unwrap();
        db.conn
            .execute("UPDATE schema_version SET version = 3", [])
            .unwrap();
        drop(db);
        match Db::open(&path) {
            Err(Error::SchemaTooNew { found, supported }) => {
                assert_eq!((found, supported), (3, SCHEMA_VERSION));
            }
            other => panic!("expected SchemaTooNew, got {other:?}"),
        }
        let reopened = Connection::open(&path).unwrap();
        let v: i64 = reopened
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 3, "a refused database is left untouched");
        drop(reopened);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_v1_database_migrates_to_v2_and_keeps_its_panes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0].1).unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO workspaces (path, name, opened_at) VALUES ('/Users/example', 'example', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO panes (workspace_id, tab_id, position, cli, cwd, title, status, created_at) \
             VALUES (1, 1, 0, 'claude', '/Users/example', 'claude', 'idle', 5)",
            [],
        )
        .unwrap();
        let db = Db::init(conn).unwrap();
        assert_eq!(db.schema_version().unwrap(), 2);
        let panes = db.open_panes().unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].cli, Cli::Claude);
        assert!(db.tasks().unwrap().is_empty());
    }

    fn task(workspace_id: u64, pane_id: Option<PaneId>) -> Task {
        Task {
            id: 0,
            workspace_id,
            pane_id,
            pool: None,
            text: "/review-pr #212\nthen tell me".into(),
            skill: Some("/review-pr".into()),
            state: TaskState::Queued,
            position: 2,
            detail: None,
            created_at: 100,
            sent_at: None,
            started_at: None,
            ended_at: None,
        }
    }

    #[test]
    fn tasks_round_trip_through_their_row() {
        let db = Db::open_in_memory().unwrap();
        let ws = db.insert_workspace("/Users/example", "example", 1).unwrap();
        let pane_id = db.insert_pane(&pane(ws.id, 1)).unwrap();
        let queued = task(ws.id, Some(pane_id));
        let id = db.insert_task(&queued).unwrap();
        let pooled = Task {
            pane_id: None,
            pool: Some(TaskPool {
                cli: AgentCli::Codex,
                cwd: "/Users/example/project".into(),
            }),
            skill: None,
            ..task(ws.id, None)
        };
        let pool_id = db.insert_task(&pooled).unwrap();
        let mut stored = db.tasks().unwrap();
        stored.sort_by_key(|t| t.id);
        assert_eq!(
            stored[0],
            Task {
                id,
                ..queued.clone()
            }
        );
        assert_eq!(
            stored[1],
            Task {
                id: pool_id,
                ..pooled
            }
        );
        let ended = Task {
            id,
            state: TaskState::Ended,
            sent_at: Some(110),
            started_at: Some(111),
            ended_at: Some(150),
            detail: Some("done".into()),
            ..queued
        };
        db.update_task(&ended).unwrap();
        assert!(db.tasks().unwrap().contains(&ended));
        db.delete_tasks(&[pool_id]).unwrap();
        assert_eq!(db.tasks().unwrap(), vec![ended]);
    }

    #[test]
    fn task_states_round_trip_and_the_column_rejects_others() {
        for s in [
            TaskState::Queued,
            TaskState::Sent,
            TaskState::Running,
            TaskState::Ended,
            TaskState::Failed,
            TaskState::Cancelled,
        ] {
            assert_eq!(parse_task_state(task_state_str(s)).unwrap(), s);
            assert_eq!(
                serde_json::to_value(s).unwrap(),
                serde_json::Value::from(task_state_str(s))
            );
        }
        let db = Db::open_in_memory().unwrap();
        let ws = db.insert_workspace("/Users/example", "example", 1).unwrap();
        let bad = db.conn.execute(
            "INSERT INTO tasks (workspace_id, text, state, position, created_at) VALUES (?1, 'x', 'paused', 0, 1)",
            [ws.id],
        );
        assert!(bad.is_err());
    }

    #[test]
    fn statuses_and_clis_round_trip_through_their_column_values() {
        for s in [
            PaneStatus::Starting,
            PaneStatus::Idle,
            PaneStatus::Running,
            PaneStatus::WaitingPermission,
            PaneStatus::WaitingInput,
            PaneStatus::Exited,
            PaneStatus::Lost,
        ] {
            assert_eq!(parse_status(status_str(s)).unwrap(), s);
            assert_eq!(
                serde_json::to_value(s).unwrap(),
                serde_json::Value::from(status_str(s))
            );
        }
        for c in [Cli::Claude, Cli::Codex, Cli::Shell] {
            assert_eq!(parse_cli(cli_str(c)).unwrap(), c);
        }
    }
}
