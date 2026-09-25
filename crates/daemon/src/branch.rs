//! The branch label of a pane header (spec 6.3): `git rev-parse --abbrev-ref HEAD` in the pane's directory.
//!
//! plyd asks git once per directory change (a pane's spawn, OSC 7, a hook's or rollout's `cwd`) on a tokio task, with
//! the pane's own base environment, stdin from `/dev/null` and a [`GIT_TIMEOUT`]. The answer is display only: it goes
//! out in `pane.meta` and lives in memory, never in the database. Outside a repository, without git, or on any
//! failure there is simply no branch. plyd never runs any other git command here (INV-7).

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use ply_proto::pane::PaneId;
use tokio::process::Command;

use crate::daemon::Shared;

/// Longest one branch lookup may take.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Looks the branch of `cwd` up in the background and records it for `pane_id` if the pane is still there.
pub fn lookup(shared: &Arc<Shared>, pane_id: PaneId, cwd: String) {
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        let branch = branch_of(&shared, pane_id, &cwd).await;
        shared.registry().set_branch(pane_id, &cwd, branch);
    });
}

async fn branch_of(shared: &Shared, pane_id: PaneId, cwd: &str) -> Option<String> {
    let git = shared.login.which("git")?;
    if !Path::new(cwd).is_dir() {
        return None;
    }
    let mut cmd = Command::new(&git);
    cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .env_clear()
        .envs(&shared.login.base)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(GIT_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            let branch = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            (!branch.is_empty()).then_some(branch)
        }
        Ok(Ok(_)) => None,
        Ok(Err(e)) => {
            tracing::debug!(pane_id, error = %e, "cannot run git for the branch label");
            None
        }
        Err(_) => {
            tracing::debug!(pane_id, cwd, "git took too long for the branch label");
            None
        }
    }
}
