//! The git labels of a pane header (spec 6.3): the branch, the project and the linked worktree of the pane's directory.
//!
//! plyd asks git on a tokio task, with the pane's own base environment, stdin from `/dev/null` and a [`GIT_TIMEOUT`],
//! whenever a pane's directory changes (its spawn, OSC 7, a hook's or rollout's `cwd`) and whenever the repository's
//! `HEAD` file changes: [`watch_heads`] looks at each pane's `HEAD` once every [`HEAD_POLL`], a file stat with no git
//! run, so a `git switch` in the pane shows at once. It runs `git rev-parse --show-toplevel --absolute-git-dir
//! --git-common-dir` and `git symbolic-ref --short -q HEAD` (a detached `HEAD` shows as its `git rev-parse --short
//! HEAD`), and nothing else (INV-7). The answers are display only: they go out in `pane.meta` and live in memory, never
//! in the database. Outside a repository, without git, or on any failure there is no branch and no worktree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ply_proto::pane::PaneId;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::daemon::Shared;

/// Longest one git run may take.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(2);

/// How often [`watch_heads`] looks at each pane's `HEAD` file.
pub const HEAD_POLL: Duration = Duration::from_secs(1);

/// What git says about one directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitInfo {
    /// The branch checked out, or the short commit of a detached `HEAD`.
    pub branch: Option<String>,
    /// The folder name of the repository, the main one for a linked worktree.
    pub project: Option<String>,
    /// The folder name of the linked worktree the directory is in; `None` in the main one.
    pub worktree: Option<String>,
    /// The `HEAD` file a branch switch rewrites.
    pub head: Option<PathBuf>,
}

/// One pane's `HEAD` under watch: the directory it was looked up for and the file's last modification time.
#[derive(Debug, Clone)]
struct HeadWatch {
    cwd: String,
    head: PathBuf,
    modified: Option<SystemTime>,
}

/// The `HEAD` files [`watch_heads`] looks at, one per pane.
#[derive(Debug, Default)]
pub struct HeadWatches {
    panes: Mutex<BTreeMap<PaneId, HeadWatch>>,
}

impl HeadWatches {
    async fn set(&self, pane_id: PaneId, cwd: &str, head: Option<PathBuf>) {
        let mut panes = self.panes.lock().await;
        match head {
            Some(head) => {
                let modified = modified(&head);
                panes.insert(
                    pane_id,
                    HeadWatch {
                        cwd: cwd.to_owned(),
                        head,
                        modified,
                    },
                );
            }
            None => {
                panes.remove(&pane_id);
            }
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Looks the git labels of `cwd` up in the background, records them for `pane_id` if the pane is still there, and watches its `HEAD`.
pub fn lookup(shared: &Arc<Shared>, pane_id: PaneId, cwd: String) {
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        let info = git_info(&shared, pane_id, &cwd).await;
        shared.registry().set_git(pane_id, &cwd, &info);
        shared.heads.set(pane_id, &cwd, info.head).await;
    });
}

/// Every [`HEAD_POLL`], looks the labels up again for each pane whose `HEAD` changed, and forgets the panes that are gone.
pub async fn watch_heads(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(HEAD_POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let watched: Vec<(PaneId, HeadWatch)> = {
            let panes = shared.heads.panes.lock().await;
            panes.iter().map(|(id, w)| (*id, w.clone())).collect()
        };
        for (pane_id, watch) in watched {
            if shared.registry().entry(pane_id).is_none() {
                shared.heads.panes.lock().await.remove(&pane_id);
                continue;
            }
            let now = modified(&watch.head);
            if now == watch.modified {
                continue;
            }
            if let Some(w) = shared.heads.panes.lock().await.get_mut(&pane_id) {
                w.modified = now;
            }
            tracing::debug!(pane_id, head = %watch.head.display(), "HEAD changed; looking the branch up again");
            lookup(&shared, pane_id, watch.cwd);
        }
    }
}

async fn git_info(shared: &Shared, pane_id: PaneId, cwd: &str) -> GitInfo {
    if !Path::new(cwd).is_dir() {
        return GitInfo::default();
    }
    let Some(dirs) = git(
        shared,
        pane_id,
        cwd,
        &[
            "rev-parse",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
        ],
    )
    .await
    else {
        return GitInfo::default();
    };
    let mut lines = dirs.lines();
    let (Some(top), Some(git_dir), Some(common)) = (lines.next(), lines.next(), lines.next())
    else {
        tracing::debug!(pane_id, cwd, "git rev-parse answered too few lines");
        return GitInfo::default();
    };
    let git_dir = PathBuf::from(git_dir);
    let common = canonical(&Path::new(cwd).join(common));
    let linked = canonical(&git_dir) != common;
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    let project = if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().and_then(name)
    } else {
        name(Path::new(top))
    };
    let branch = match git(
        shared,
        pane_id,
        cwd,
        &["symbolic-ref", "--short", "-q", "HEAD"],
    )
    .await
    {
        Some(branch) => Some(branch),
        None => git(shared, pane_id, cwd, &["rev-parse", "--short", "HEAD"]).await,
    };
    GitInfo {
        branch,
        project,
        worktree: linked.then(|| name(Path::new(top))).flatten(),
        head: Some(git_dir.join("HEAD")),
    }
}

fn canonical(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(e) => {
            tracing::debug!(path = %path.display(), error = %e, "cannot resolve a git directory; comparing it as given");
            path.to_path_buf()
        }
    }
}

/// The trimmed output of one git run in `cwd`, `None` when it fails, times out or prints nothing.
async fn git(shared: &Shared, pane_id: PaneId, cwd: &str, args: &[&str]) -> Option<String> {
    let login = shared.login.get();
    let git = login.which("git")?;
    let mut cmd = Command::new(&git);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(&login.base)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(GIT_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            (!text.is_empty()).then_some(text)
        }
        Ok(Ok(_)) => None,
        Ok(Err(e)) => {
            tracing::debug!(pane_id, error = %e, "cannot run git for the pane header");
            None
        }
        Err(_) => {
            tracing::debug!(pane_id, cwd, "git took too long for the pane header");
            None
        }
    }
}
