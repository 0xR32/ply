//! The Claude account switch (`account.list`, `account.switch`): which claude.ai account Claude Code is signed in to,
//! from its own `claude auth status --json`, and a move to another through its own `claude auth login`.
//!
//! plyd holds no credential and writes none of Claude Code's files (INV-8). The sign-in runs in a pane of its own, a
//! new [`SIGN_IN_TAB`] tab, where the user finishes Claude's browser sign-in and Claude Code stores the result itself.
//! plyd remembers only the accounts it saw signed in (email, organization and plan, in `accounts.json`, at most
//! [`MAX_KNOWN`]), so the app can offer them again; a personal plan and a Team organization under one email are two
//! accounts. When the sign-in pane's process exits, plyd asks `claude auth status` again. On another account than
//! before it records it, forgets the plan usage Claude Code reported for the old one, closes the sign-in pane and
//! tells every live Claude pane to restart once at rest ([`PaneCmd::ReloadAtRest`]), resuming its session; the same
//! account only closes the pane, and a failed sign-in leaves it open with what `claude auth login` printed.
//! `account.changed` says how it ended. One switch runs at a time.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ply_agents::claude::account::{parse_auth_status, same_account, sign_in_argv, status_argv};
use ply_proto::control::{AccountChanged, AccountSwitchParams, ErrorBody, ErrorCode, Event};
use ply_proto::pane::{
    AgentCli, ClaudeAccount, ClaudeAccounts, Cli, Pane, PaneId, PaneStatus, SignInOutcome,
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::broadcast;

use crate::daemon::{Shared, unix_now};
use crate::panes::launch;
use crate::panes::pane::PaneCmd;
use crate::panes::registry::{MethodResult, refuse};

/// Longest `claude auth status` may take before plyd gives up on it.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// Most bytes of `claude auth status` output read.
pub const MAX_STATUS_BYTES: u64 = 64 * 1024;

/// Most accounts remembered; the one signed in to longest ago goes first.
pub const MAX_KNOWN: usize = 16;

/// The name of the tab `claude auth login` runs in.
pub const SIGN_IN_TAB: &str = "Claude sign-in";

/// The sign-in pane's title until the terminal sets one.
pub const SIGN_IN_TITLE: &str = "claude auth login";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Free,
    Opening,
    Open(PaneId),
}

#[derive(Debug)]
struct State {
    known: Vec<ClaudeAccount>,
    slot: Slot,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AccountsFile {
    #[serde(default)]
    claude: Vec<ClaudeAccount>,
}

/// The accounts seen signed in and the switch under way; one per plyd, in [`Shared`].
#[derive(Debug)]
pub struct Accounts {
    path: PathBuf,
    state: Mutex<State>,
}

impl Accounts {
    /// Reads `path` (`accounts.json`); a missing file is no account, and an unreadable or invalid one is logged and ignored until the next write replaces it.
    pub fn load(path: &Path) -> Self {
        let known = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<AccountsFile>(&bytes) {
                Ok(file) => file.claude,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "accounts.json is invalid; starting with no known account");
                    Vec::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot read accounts.json; starting with no known account");
                Vec::new()
            }
        };
        Self {
            path: path.to_path_buf(),
            state: Mutex::new(State {
                known,
                slot: Slot::Free,
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("the accounts lock was poisoned; continuing with its state");
                poisoned.into_inner()
            }
        }
    }

    /// The pane of the switch under way, if one is.
    pub fn signing_in(&self) -> Option<PaneId> {
        match self.state().slot {
            Slot::Open(pane) => Some(pane),
            Slot::Free | Slot::Opening => None,
        }
    }

    /// What `account.list` answers with `current` as Claude Code's sign-in now.
    pub fn snapshot(&self, current: Option<ClaudeAccount>) -> ClaudeAccounts {
        let state = self.state();
        ClaudeAccounts {
            current,
            known: state.known.clone(),
            signing_in: match state.slot {
                Slot::Open(pane) => Some(pane),
                Slot::Free | Slot::Opening => None,
            },
        }
    }

    /// Puts `account` first among the known ones, replacing what was known of it, and writes `accounts.json` when that changed anything; a failed write is logged.
    pub fn remember(&self, account: &ClaudeAccount) {
        let known = {
            let mut state = self.state();
            let mut known: Vec<ClaudeAccount> = state
                .known
                .iter()
                .filter(|k| !same_account(k, account))
                .cloned()
                .collect();
            known.insert(0, account.clone());
            known.truncate(MAX_KNOWN);
            if known == state.known {
                return;
            }
            state.known.clone_from(&known);
            known
        };
        if let Err(e) = write_file(&self.path, &AccountsFile { claude: known }) {
            tracing::warn!(path = %self.path.display(), error = %e, "cannot write accounts.json");
        }
    }

    fn begin(&self) -> MethodResult<()> {
        let mut state = self.state();
        match state.slot {
            Slot::Free => {
                state.slot = Slot::Opening;
                Ok(())
            }
            Slot::Opening => Err(refuse(
                ErrorCode::InvalidState,
                "a Claude sign-in is opening",
            )),
            Slot::Open(pane) => Err(refuse(
                ErrorCode::InvalidState,
                format!("a Claude sign-in is open in pane {pane}"),
            )),
        }
    }

    fn set_slot(&self, slot: Slot) {
        self.state().slot = slot;
    }
}

fn write_file(path: &Path, file: &AccountsFile) -> std::io::Result<()> {
    let text = serde_json::to_vec_pretty(file).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    out.write_all(&text)?;
    out.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// `account.list`: Claude Code's sign-in now, remembered among the known accounts.
/// Fails with `cli_not_found`, or `internal` when `claude auth status` fails or prints something unreadable.
pub async fn list(shared: &Shared) -> MethodResult<ClaudeAccounts> {
    let current = auth_status(shared).await?;
    if let Some(account) = &current {
        shared.accounts.remember(account);
    }
    Ok(shared.accounts.snapshot(current))
}

/// `account.switch`: opens the sign-in tab running `claude auth login` and follows it (module docs).
/// Fails with `invalid_state` while another switch is open, `bad_request` for an email `claude auth login` cannot take, `not_found`, `cli_not_found`, `spawn_failed`, `shutting_down`.
pub async fn switch(shared: &Arc<Shared>, p: AccountSwitchParams) -> MethodResult<Pane> {
    if shared.is_stopping() {
        return Err(refuse(ErrorCode::ShuttingDown, "plyd is shutting down"));
    }
    shared.accounts.begin()?;
    let opened = open(shared, &p).await;
    shared.accounts.set_slot(match &opened {
        Ok((pane, _, _)) => Slot::Open(pane.id),
        Err(_) => Slot::Free,
    });
    let (pane, before, events) = opened?;
    tracing::info!(pane_id = pane.id, asked = ?p.account.as_ref().map(|a| &a.email), "a Claude sign-in opened");
    let sign_in = SignIn {
        pane_id: pane.id,
        asked: p.account,
        before,
    };
    tokio::spawn(follow(Arc::clone(shared), sign_in, events));
    Ok(pane)
}

type Opened = (Pane, Option<ClaudeAccount>, broadcast::Receiver<Event>);

async fn open(shared: &Arc<Shared>, p: &AccountSwitchParams) -> MethodResult<Opened> {
    let cwd = {
        let reg = shared.registry();
        let workspace = reg
            .workspaces()
            .into_iter()
            .find(|w| w.id == p.workspace_id)
            .ok_or_else(|| {
                refuse(
                    ErrorCode::NotFound,
                    format!("no workspace {}", p.workspace_id),
                )
            })?;
        PathBuf::from(workspace.path)
    };
    let program = claude_program(shared).await?;
    let email = p.account.as_ref().map(|a| a.email.as_str());
    let argv = sign_in_argv(&program, email).map_err(|e| {
        tracing::warn!(error = %e, "an account switch was refused");
        refuse(ErrorCode::BadRequest, e.to_string())
    })?;
    let before = match auth_status(shared).await {
        Ok(account) => account,
        Err(e) => {
            tracing::warn!(error = %e.msg, "cannot tell the sign-in before the switch; any account after it counts as new");
            None
        }
    };
    // Subscribed before the process starts, so its exit cannot be missed.
    let events = shared.events.subscribe();
    let pane = launch::create_command(
        shared,
        p.workspace_id,
        &cwd,
        SIGN_IN_TAB,
        SIGN_IN_TITLE,
        argv,
    )
    .await?;
    Ok((pane, before, events))
}

struct SignIn {
    pane_id: PaneId,
    asked: Option<ClaudeAccount>,
    before: Option<ClaudeAccount>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Exited(i32),
    Closed,
    Stopping,
}

/// Waits for the sign-in pane's process to end, then carries out and announces the switch (module docs).
async fn follow(shared: Arc<Shared>, sign_in: SignIn, mut events: broadcast::Receiver<Event>) {
    let pane_id = sign_in.pane_id;
    let end = wait_end(&shared, pane_id, &mut events).await;
    drop(events);
    if end == End::Stopping {
        tracing::info!(pane_id, "plyd stops while a Claude sign-in is open");
        shared.accounts.set_slot(Slot::Free);
        return;
    }
    let closing = shared
        .registry()
        .entry(pane_id)
        .is_none_or(|e| e.close_on_exit);
    let (outcome, account) = match end {
        End::Exited(_) | End::Closed if closing => (SignInOutcome::Cancelled, None),
        End::Exited(0) => match auth_status(&shared).await {
            Ok(Some(account)) => (decide(&sign_in, &account), Some(account)),
            Ok(None) => (SignInOutcome::Failed, None),
            Err(e) => {
                tracing::warn!(pane_id, error = %e.msg, "cannot tell the sign-in after claude auth login");
                (SignInOutcome::Failed, None)
            }
        },
        End::Exited(_) | End::Closed | End::Stopping => (SignInOutcome::Failed, None),
    };
    let mut panes = 0;
    if let Some(account) = &account {
        shared.accounts.remember(account);
        if outcome != SignInOutcome::Unchanged {
            shared.usage.forget_claude().await;
            panes = reload_claude_panes(&shared, pane_id).await;
        }
        close_sign_in(&shared, pane_id).await;
    }
    shared.accounts.set_slot(Slot::Free);
    tracing::info!(pane_id, ?end, ?outcome, account = ?account.as_ref().map(|a| &a.email), panes, "the Claude sign-in ended");
    let event = Event::AccountChanged(AccountChanged {
        pane_id,
        outcome,
        account,
        panes,
    });
    if shared.events.send(event).is_err() {
        tracing::debug!("no C1 client to tell about the account switch");
    }
}

fn decide(sign_in: &SignIn, account: &ClaudeAccount) -> SignInOutcome {
    if sign_in
        .before
        .as_ref()
        .is_some_and(|b| same_account(b, account))
    {
        SignInOutcome::Unchanged
    } else if sign_in
        .asked
        .as_ref()
        .is_some_and(|a| !same_account(a, account))
    {
        SignInOutcome::OtherAccount
    } else {
        SignInOutcome::SignedIn
    }
}

async fn wait_end(
    shared: &Shared,
    pane_id: PaneId,
    events: &mut broadcast::Receiver<Event>,
) -> End {
    loop {
        match events.recv().await {
            Ok(Event::PaneExit(exit)) if exit.pane_id == pane_id => return End::Exited(exit.code),
            Ok(Event::PaneRemoved(removed)) if removed.pane_id == pane_id => return End::Closed,
            Ok(Event::DaemonStopping(_)) | Err(broadcast::error::RecvError::Closed) => {
                return End::Stopping;
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                tracing::debug!(
                    pane_id,
                    missed,
                    "the sign-in's follower fell behind; asking the registry"
                );
                let reg = shared.registry();
                match reg.entry(pane_id) {
                    None => return End::Closed,
                    Some(e) if e.pane.status == PaneStatus::Exited => {
                        return End::Exited(e.pane.exit_code.unwrap_or(crate::pty::EXIT_UNKNOWN));
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

/// Tells every live Claude pane but the sign-in's to restart on the new sign-in once at rest; returns how many were told.
async fn reload_claude_panes(shared: &Shared, sign_in: PaneId) -> u32 {
    let handles: Vec<(PaneId, tokio::sync::mpsc::Sender<PaneCmd>)> = {
        let reg = shared.registry();
        reg.live_panes()
            .into_iter()
            .filter(|&id| id != sign_in)
            .filter_map(|id| {
                let entry = reg.entry(id)?;
                let handle = entry.handle.clone()?;
                (entry.pane.cli == Cli::Claude).then_some((id, handle))
            })
            .collect()
    };
    let mut told = 0;
    for (pane_id, handle) in handles {
        if handle.send(PaneCmd::ReloadAtRest).await.is_ok() {
            told += 1;
        } else {
            tracing::debug!(pane_id, "the pane task had stopped before the switch");
        }
    }
    told
}

/// Closes the sign-in pane once its process exited, as `pane.close` does; a failure is logged.
async fn close_sign_in(shared: &Shared, pane_id: PaneId) {
    let closed = shared.registry().close_pane(pane_id, unix_now());
    match closed {
        Ok(handle) => {
            if let Some(handle) = handle
                && handle.send(PaneCmd::Stop).await.is_err()
            {
                tracing::debug!(pane_id, "the sign-in pane's task had stopped");
            }
            launch::remove_pane_dir(pane_id, &shared.paths.pane_dir(pane_id));
        }
        Err(e) => tracing::warn!(pane_id, error = %e.msg, "cannot close the sign-in pane"),
    }
}

async fn claude_program(shared: &Shared) -> MethodResult<PathBuf> {
    let login = shared.login.settled().await;
    let name = ply_agents::cli_name(AgentCli::Claude);
    login.which(name).ok_or_else(|| {
        tracing::warn!(
            from_shell = login.from_shell,
            "claude is not on the login shell's PATH"
        );
        refuse(
            ErrorCode::CliNotFound,
            format!("{name} is not on the login shell's PATH"),
        )
    })
}

/// Runs `claude auth status --json` in the login environment, bounded by [`STATUS_TIMEOUT`] and [`MAX_STATUS_BYTES`]; its exit status is not used, since it may report a signed-out state with a failure.
async fn auth_status(shared: &Shared) -> MethodResult<Option<ClaudeAccount>> {
    let program = claude_program(shared).await?;
    let fail = |what: String| -> ErrorBody {
        tracing::warn!(error = %what, "claude auth status failed");
        refuse(ErrorCode::Internal, what)
    };
    let argv = status_argv(&program).map_err(|e| fail(e.to_string()))?;
    let env = shared.login.get().env_for(&BTreeMap::new());
    let home = env
        .get("HOME")
        .map_or_else(|| PathBuf::from("/"), PathBuf::from);
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .env_clear()
        .envs(&env)
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let run = async {
        let mut child = command.spawn()?;
        let mut out = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            stdout.take(MAX_STATUS_BYTES).read_to_end(&mut out).await?;
        }
        child.wait().await?;
        Ok::<_, std::io::Error>(out)
    };
    let out = match tokio::time::timeout(STATUS_TIMEOUT, run).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(fail(format!("cannot run claude auth status: {e}"))),
        Err(_) => {
            return Err(fail(format!(
                "claude auth status did not answer within {STATUS_TIMEOUT:?}"
            )));
        }
    };
    parse_auth_status(&out).map_err(|e| fail(format!("cannot read claude auth status: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn account(email: &str, org: &str, plan: &str) -> ClaudeAccount {
        ClaudeAccount {
            email: email.to_owned(),
            org_id: Some(org.to_owned()),
            org_name: None,
            plan: Some(plan.to_owned()),
        }
    }

    #[test]
    fn the_last_account_signed_in_to_comes_first_and_survives_a_restart() {
        let dir = Dir(std::env::temp_dir().join(format!("ply-accounts-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let path = dir.0.join("accounts.json");
        let personal = account("someone@example.com", "org-a", "max");
        let team = account("someone@example.com", "org-b", "team");
        let accounts = Accounts::load(&path);
        assert!(accounts.snapshot(None).known.is_empty());
        accounts.remember(&personal);
        accounts.remember(&team);
        accounts.remember(&ClaudeAccount {
            plan: Some("pro".to_owned()),
            ..personal.clone()
        });
        let known = Accounts::load(&path).snapshot(None).known;
        assert_eq!(known.len(), 2, "one email, two organizations: {known:?}");
        assert_eq!(known[0].plan.as_deref(), Some("pro"), "replaced and first");
        assert_eq!(known[1], team);
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&path).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);

        for i in 0..MAX_KNOWN + 3 {
            accounts.remember(&account(&format!("a{i}@example.com"), "org", "pro"));
        }
        assert_eq!(accounts.snapshot(None).known.len(), MAX_KNOWN);

        std::fs::write(&path, "not json").unwrap();
        assert!(Accounts::load(&path).snapshot(None).known.is_empty());
    }

    #[test]
    fn one_switch_runs_at_a_time() {
        let accounts = Accounts::load(Path::new("/nonexistent/accounts.json"));
        accounts.begin().unwrap();
        assert_eq!(accounts.begin().unwrap_err().code, ErrorCode::InvalidState);
        accounts.set_slot(Slot::Open(7));
        assert_eq!(accounts.signing_in(), Some(7));
        assert!(accounts.begin().unwrap_err().msg.contains("pane 7"));
        accounts.set_slot(Slot::Free);
        accounts.begin().unwrap();
    }

    #[test]
    fn the_outcome_compares_the_sign_in_with_the_one_before_and_the_one_asked_for() {
        let personal = account("someone@example.com", "org-a", "max");
        let team = account("someone@example.com", "org-b", "team");
        let sign_in = |before: Option<&ClaudeAccount>, asked: Option<&ClaudeAccount>| SignIn {
            pane_id: 1,
            before: before.cloned(),
            asked: asked.cloned(),
        };
        assert_eq!(
            decide(&sign_in(Some(&personal), Some(&team)), &team),
            SignInOutcome::SignedIn
        );
        assert_eq!(
            decide(&sign_in(Some(&personal), None), &team),
            SignInOutcome::SignedIn
        );
        assert_eq!(
            decide(&sign_in(None, Some(&team)), &personal),
            SignInOutcome::OtherAccount
        );
        assert_eq!(
            decide(&sign_in(Some(&personal), Some(&team)), &personal),
            SignInOutcome::Unchanged
        );
    }
}
