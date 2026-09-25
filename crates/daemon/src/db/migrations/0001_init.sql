CREATE TABLE schema_version (version INTEGER NOT NULL);

CREATE TABLE workspaces (
  id INTEGER PRIMARY KEY,
  path TEXT UNIQUE NOT NULL,
  name TEXT NOT NULL,
  opened_at INTEGER NOT NULL
);

CREATE TABLE panes (
  id INTEGER PRIMARY KEY,
  workspace_id INTEGER NOT NULL REFERENCES workspaces,
  tab_id INTEGER NOT NULL,
  position INTEGER NOT NULL,
  cli TEXT NOT NULL CHECK (cli IN ('claude', 'codex', 'shell')),
  cwd TEXT NOT NULL,
  model_seen TEXT,
  worktree_seen TEXT,
  exit_code INTEGER,
  last_activity_at INTEGER,
  session_ref TEXT,
  title TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  closed_at INTEGER
);

CREATE TABLE tabs (
  id INTEGER PRIMARY KEY,
  workspace_id INTEGER NOT NULL REFERENCES workspaces,
  name TEXT NOT NULL,
  position INTEGER NOT NULL,
  focus_pane_id INTEGER,
  zoomed INTEGER NOT NULL DEFAULT 0
);
