//! C3 hook ingress over `run/hook.sock` (spec 4.3): Claude Code hooks and Codex notify, forwarded by `ply-hook`.
//!
//! Each `ply-hook` run connects, writes one envelope line (`{"v":1,"pane_id","cli","event","payload"}`) and closes;
//! plyd reads at most [`MAX_LINE_BYTES`] per line and gives a connection [`READ_TIMEOUT`] per line, so a stuck
//! writer never holds a task for long. A line is decoded strictly (`deny_unknown_fields`, `v` checked, INV-10), then
//! handed to its pane's task, which feeds it to the pane's agent session and state machine. A `StatusLine` envelope is
//! plan usage, not a hook: it goes to [`crate::usage::UsageCache::status_line`] and never reaches a pane, so a status
//! line refreshing every few seconds cannot hold R17's quiet timer open. It is taken from any pane id, 0 included, which
//! is a Claude Code session outside ply reporting through `ply-hook statusline`. An envelope for an unknown pane, a
//! shell pane or a pane of the other CLI is logged and dropped. Nothing is ever answered: the hook
//! never waits on plyd and never decides anything for the CLI (INV-12, INV-14). The socket lives in the 0700 run
//! directory, so only the user's own processes can reach it.

use std::sync::Arc;
use std::time::Duration;

use ply_agents::claude::settings::STATUS_LINE_EVENT;
use ply_proto::hook::{HookEnvelope, MAX_LINE_BYTES};
use ply_proto::pane::{AgentCli, Cli};
use tokio::io::BufReader;
use tokio::net::{UnixListener, UnixStream};

use crate::daemon::{Shared, unix_now};
use crate::panes::pane::PaneCmd;
use crate::server::control::read_line_max;

/// Longest plyd waits for the next line on a hook connection before closing it.
pub const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Accepts C3 connections until the task is aborted.
pub async fn serve(listener: UnixListener, shared: Arc<Shared>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                // Tasks are unordered, which is fine: a CLI starts its next hook only after ply-hook wrote this line and exited, milliseconds apart.
                tokio::spawn(connection(stream, Arc::clone(&shared)));
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot accept a C3 connection");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn connection(stream: UnixStream, shared: Arc<Shared>) {
    let mut rd = BufReader::new(stream);
    let mut line = Vec::new();
    loop {
        line.clear();
        match tokio::time::timeout(
            READ_TIMEOUT,
            read_line_max(&mut rd, &mut line, MAX_LINE_BYTES),
        )
        .await
        {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => return,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "unreadable C3 connection closed");
                return;
            }
            Err(_) => {
                tracing::debug!("C3 connection sent nothing for {READ_TIMEOUT:?}; closed");
                return;
            }
        }
        match HookEnvelope::decode_line(&line) {
            Ok(envelope) => deliver(&shared, envelope).await,
            Err(e) => tracing::warn!(error = %e, "invalid C3 envelope dropped"),
        }
    }
}

async fn deliver(shared: &Shared, envelope: HookEnvelope) {
    let pane_id = envelope.pane_id;
    if envelope.cli == AgentCli::Claude && envelope.event.as_deref() == Some(STATUS_LINE_EVENT) {
        shared
            .usage
            .status_line(pane_id, &envelope.payload, unix_now())
            .await;
        return;
    }
    let handle = {
        let reg = shared.registry();
        match reg.entry(pane_id) {
            Some(entry) if cli_matches(entry.pane.cli, envelope.cli) => entry.handle.clone(),
            Some(entry) => {
                tracing::warn!(pane_id, pane_cli = ?entry.pane.cli, envelope_cli = ?envelope.cli, "C3 envelope for a pane of another program dropped");
                return;
            }
            None => {
                tracing::info!(pane_id, event = ?envelope.event, "C3 envelope for an unknown pane dropped");
                return;
            }
        }
    };
    let Some(handle) = handle else {
        tracing::info!(pane_id, "C3 envelope for a pane without a task dropped");
        return;
    };
    if handle
        .send(PaneCmd::Hook(Box::new(envelope)))
        .await
        .is_err()
    {
        tracing::debug!(pane_id, "the pane task stopped before its hook arrived");
    }
}

fn cli_matches(pane: Cli, envelope: AgentCli) -> bool {
    matches!(
        (pane, envelope),
        (Cli::Claude, AgentCli::Claude) | (Cli::Codex, AgentCli::Codex)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_reach_only_panes_of_their_cli() {
        assert!(cli_matches(Cli::Claude, AgentCli::Claude));
        assert!(cli_matches(Cli::Codex, AgentCli::Codex));
        assert!(!cli_matches(Cli::Claude, AgentCli::Codex));
        assert!(!cli_matches(Cli::Shell, AgentCli::Claude));
    }
}
