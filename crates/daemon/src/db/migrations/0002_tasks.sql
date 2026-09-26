CREATE TABLE tasks (
  id INTEGER PRIMARY KEY,
  workspace_id INTEGER NOT NULL REFERENCES workspaces,
  pane_id INTEGER REFERENCES panes,
  pool_cli TEXT CHECK (pool_cli IN ('claude', 'codex')),
  pool_cwd TEXT,
  text TEXT NOT NULL,
  skill TEXT,
  state TEXT NOT NULL CHECK (state IN ('queued', 'sent', 'running', 'ended', 'failed', 'cancelled')),
  position INTEGER NOT NULL,
  detail TEXT,
  created_at INTEGER NOT NULL,
  sent_at INTEGER,
  started_at INTEGER,
  ended_at INTEGER
);

CREATE INDEX tasks_by_queue ON tasks (pane_id, state, position);
CREATE INDEX tasks_by_end ON tasks (workspace_id, ended_at);
