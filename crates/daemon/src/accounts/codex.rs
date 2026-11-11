//! Codex's sign-in (`codex.account`, `codex.sign_in`): the ChatGPT account or API key Codex uses, read from its own
//! `$CODEX_HOME/auth.json`, and a new sign-in through its own `codex login`.
//!
//! Codex cannot be pointed at one account, so moving to another is signing in again. plyd only reads `auth.json`
//! (read-only, at most [`MAX_AUTH_BYTES`]), keeps nothing of it but the email, the plan and whether it is an API key,
//! and writes nothing under `CODEX_HOME` (INV-8). The sign-in runs in a pane of its own, a new [`SIGN_IN_TAB`] tab,
//! where the user finishes it and Codex stores the result itself. When that pane's process exits 0, plyd reads
//! `auth.json` again: another account than before closes the pane and tells every live Codex pane to restart once at
//! rest ([`crate::panes::pane::PaneCmd::ReloadAtRest`]), resuming its thread; the same account only closes the pane,
//! and a failed sign-in leaves it open with what `codex login` printed. `codex.changed` says how it ended. One Codex
//! sign-in runs at a time, beside at most one Claude account switch.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ply_agents::codex::account::{AUTH_FILE, parse_auth, same_account, sign_in_argv};
use ply_proto::control::{CodexChanged, ErrorCode, Event, WorkspaceRef};
use ply_proto::pane::{AgentCli, Cli, CodexAccount, CodexAccounts, Pane, PaneId, SignInOutcome};
use tokio::sync::broadcast;

use super::{
    Ended, Slot, announce, cli_program, close_sign_in, open_tab, reload_panes, wait_sign_in,
    workspace_dir,
};
use crate::daemon::Shared;
use crate::panes::registry::{MethodResult, refuse};

/// Most bytes of `auth.json` read; a larger file is not one Codex wrote and counts as unreadable.
pub const MAX_AUTH_BYTES: u64 = 64 * 1024;

/// The name of the tab `codex login` runs in.
pub const SIGN_IN_TAB: &str = "Codex sign-in";

/// The sign-in pane's title until the terminal sets one.
pub const SIGN_IN_TITLE: &str = "codex login";

/// `codex.account`: Codex's sign-in now and the sign-in pane, if one is open; never fails, since an unreadable `auth.json` is no sign-in.
pub async fn account(shared: &Shared) -> CodexAccounts {
    CodexAccounts {
        current: current(shared).await,
        signing_in: shared.accounts.signing_in(AgentCli::Codex),
    }
}

/// `codex.sign_in`: opens the sign-in tab running `codex login` and follows it (module docs).
/// Fails with `invalid_state` while another Codex sign-in is open, `not_found`, `cli_not_found`, `bad_request`, `spawn_failed`, `shutting_down`.
pub async fn sign_in(shared: &Arc<Shared>, p: WorkspaceRef) -> MethodResult<Pane> {
    if shared.is_stopping() {
        return Err(refuse(ErrorCode::ShuttingDown, "plyd is shutting down"));
    }
    shared.accounts.begin(AgentCli::Codex)?;
    let opened = open(shared, p.workspace_id).await;
    shared.accounts.set_slot(
        AgentCli::Codex,
        match &opened {
            Ok((pane, _, _)) => Slot::Open(pane.id),
            Err(_) => Slot::Free,
        },
    );
    let (pane, before, events) = opened?;
    tracing::info!(pane_id = pane.id, "a Codex sign-in opened");
    tokio::spawn(follow(Arc::clone(shared), pane.id, before, events));
    Ok(pane)
}

type Opened = (Pane, Option<CodexAccount>, broadcast::Receiver<Event>);

async fn open(shared: &Arc<Shared>, workspace_id: u64) -> MethodResult<Opened> {
    let cwd = workspace_dir(shared, workspace_id)?;
    let program = cli_program(shared, AgentCli::Codex).await?;
    let argv = sign_in_argv(&program).map_err(|e| {
        tracing::warn!(error = %e, "a Codex sign-in was refused");
        refuse(ErrorCode::BadRequest, e.to_string())
    })?;
    let before = current(shared).await;
    let (pane, events) =
        open_tab(shared, workspace_id, &cwd, SIGN_IN_TAB, SIGN_IN_TITLE, argv).await?;
    Ok((pane, before, events))
}

/// Waits for the sign-in pane's process to end, then restarts the Codex panes on a new account and announces it (module docs).
async fn follow(
    shared: Arc<Shared>,
    pane_id: PaneId,
    before: Option<CodexAccount>,
    events: broadcast::Receiver<Event>,
) {
    let ended = wait_sign_in(&shared, pane_id, events).await;
    let (outcome, account) = match ended {
        Ended::Stopping => {
            tracing::info!(pane_id, "plyd stops while a Codex sign-in is open");
            shared.accounts.set_slot(AgentCli::Codex, Slot::Free);
            return;
        }
        Ended::Cancelled => (SignInOutcome::Cancelled, None),
        Ended::Failed => (SignInOutcome::Failed, None),
        Ended::Succeeded => match current(&shared).await {
            Some(account) => (decide(before.as_ref(), &account), Some(account)),
            None => {
                tracing::warn!(pane_id, "codex login exited 0 but Codex is not signed in");
                (SignInOutcome::Failed, None)
            }
        },
    };
    let mut panes = 0;
    if account.is_some() {
        if outcome != SignInOutcome::Unchanged {
            panes = reload_panes(&shared, Cli::Codex, pane_id).await;
        }
        close_sign_in(&shared, pane_id).await;
    }
    shared.accounts.set_slot(AgentCli::Codex, Slot::Free);
    tracing::info!(pane_id, ?ended, ?outcome, account = ?account.as_ref().map(|a| (&a.email, a.api_key)), panes, "the Codex sign-in ended");
    announce(
        &shared,
        Event::CodexChanged(CodexChanged {
            pane_id,
            outcome,
            account,
            panes,
        }),
    );
}

fn decide(before: Option<&CodexAccount>, account: &CodexAccount) -> SignInOutcome {
    if before.is_some_and(|b| same_account(b, account)) {
        SignInOutcome::Unchanged
    } else {
        SignInOutcome::SignedIn
    }
}

/// Codex's sign-in from `auth.json` under the `CODEX_HOME` its panes run with (else `~/.codex`); `None` when signed out, or when the file is missing or unreadable.
async fn current(shared: &Shared) -> Option<CodexAccount> {
    let env = shared.login.settled().await.env_for(&BTreeMap::new());
    let dir = |key: &str| env.get(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    let Some(home) = dir("CODEX_HOME").or_else(|| dir("HOME").map(|h| h.join(".codex"))) else {
        tracing::warn!("no CODEX_HOME or HOME; Codex's sign-in is unknown");
        return None;
    };
    let path = home.join(AUTH_FILE);
    match tokio::task::spawn_blocking(move || read_auth(&path)).await {
        Ok(account) => account,
        Err(e) => {
            tracing::error!(error = %e, "the auth.json reader failed");
            None
        }
    }
}

/// Reads and parses `path` without keeping its bytes; blocks on file I/O.
fn read_auth(path: &Path) -> Option<CodexAccount> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(file = %path.display(), error = %e, "cannot open Codex's auth.json");
            return None;
        }
    };
    let mut bytes = Vec::new();
    if let Err(e) = file.take(MAX_AUTH_BYTES + 1).read_to_end(&mut bytes) {
        tracing::warn!(file = %path.display(), error = %e, "cannot read Codex's auth.json");
        return None;
    }
    if bytes.len() > usize::try_from(MAX_AUTH_BYTES).unwrap_or(usize::MAX) {
        tracing::warn!(file = %path.display(), max = MAX_AUTH_BYTES, "Codex's auth.json is too large to be Codex's");
        return None;
    }
    match parse_auth(&bytes) {
        Ok(account) => account,
        Err(e) => {
            tracing::warn!(file = %path.display(), error = %e, "Codex's auth.json is unreadable; its sign-in is unknown");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(email: Option<&str>, api_key: bool) -> CodexAccount {
        CodexAccount {
            email: email.map(str::to_owned),
            plan: None,
            api_key,
        }
    }

    #[test]
    fn a_new_email_or_a_first_sign_in_is_signed_in_and_the_same_one_unchanged() {
        let personal = account(Some("someone@example.com"), false);
        let other = account(Some("other@example.com"), false);
        assert_eq!(decide(None, &personal), SignInOutcome::SignedIn);
        assert_eq!(decide(Some(&other), &personal), SignInOutcome::SignedIn);
        assert_eq!(
            decide(Some(&account(None, true)), &personal),
            SignInOutcome::SignedIn
        );
        assert_eq!(
            decide(
                Some(&account(Some("Someone@Example.com"), false)),
                &personal
            ),
            SignInOutcome::Unchanged
        );
    }

    #[test]
    fn auth_json_is_read_within_its_cap_and_never_when_larger() {
        let dir = std::env::temp_dir().join(format!("ply-codex-auth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(AUTH_FILE);
        assert_eq!(read_auth(&path), None, "a missing file is no sign-in");
        std::fs::write(
            &path,
            r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-example"}"#,
        )
        .unwrap();
        assert_eq!(read_auth(&path), Some(account(None, true)));
        let padded = format!(
            r#"{{"OPENAI_API_KEY":"sk-example","pad":"{}"}}"#,
            "x".repeat(usize::try_from(MAX_AUTH_BYTES).unwrap())
        );
        std::fs::write(&path, padded).unwrap();
        assert_eq!(read_auth(&path), None);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_auth(&path), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
