//! C1 control over `run/plyd.sock` (spec 4.1): the handshake, every method, and the event broadcast.
//!
//! A connection starts with `hello`; a version other than [`ply_proto::PROTOCOL_VERSION`] is answered with a
//! `version_mismatch` response on request id 0 and the connection closes (INV-10). After `welcome` the client sends
//! requests, one JSON object per line of at most [`MAX_LINE_BYTES`]; a longer line closes the connection. Requests on
//! one connection are answered in order. Every daemon event is written to every connection that completed the
//! handshake; a connection that falls [`crate::daemon::EVENT_CAPACITY`] events behind is closed so its client
//! reconnects and reloads. When a response and an event are ready together the response goes first, so a client
//! always sees the answer to `daemon.shutdown` before `daemon.stopping`.

use std::sync::Arc;

use ply_proto::control::{
    Answer, Call, ClientMsg, DaemonShutdownParams, Empty, ErrorBody, ErrorCode, Event,
    HANDSHAKE_ID, MAX_LINE_BYTES, PaneAnswerParams, PaneCloseParams, Response, ServerMsg,
    ThemeSetParams, Welcome, encode_line,
};
use ply_proto::pane::{PaneStatus, Settings};
use ply_term::Palette;
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

use crate::daemon::{Shared, unix_now};
use crate::panes::launch;
use crate::panes::pane::PaneCmd;
use crate::panes::registry::{internal, is_live, refuse};
use crate::skills::SkillSources;
use crate::usage::UsageSources;

/// Responses queued towards one connection's writer.
pub const RESPONSE_CAPACITY: usize = 64;

/// Accepts C1 connections until the task is aborted.
pub async fn serve(listener: UnixListener, shared: Arc<Shared>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(connection(stream, Arc::clone(&shared)));
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot accept a C1 connection");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn connection(stream: UnixStream, shared: Arc<Shared>) {
    let conn = shared.next_client_id();
    let (rd, mut wr) = stream.into_split();
    let mut rd = BufReader::new(rd);
    let mut line = Vec::new();
    match read_line(&mut rd, &mut line).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            tracing::debug!(conn, error = %e, "C1 connection ended before hello");
            return;
        }
    }
    let hello = match ClientMsg::decode(&line) {
        Ok(ClientMsg::Hello(hello)) => hello,
        Ok(ClientMsg::Req(req)) => {
            tracing::warn!(conn, "C1 request before hello");
            let res = Response::err(req.id, ErrorCode::BadRequest, "send hello first");
            write_now(&mut wr, &ServerMsg::Res(res)).await;
            return;
        }
        Err(rejection) => {
            tracing::warn!(conn, error = %rejection, "invalid C1 hello");
            let res = Response {
                id: rejection.id.unwrap_or(HANDSHAKE_ID),
                outcome: Err(rejection.error),
            };
            write_now(&mut wr, &ServerMsg::Res(res)).await;
            return;
        }
    };
    if let Err(e) = hello.check_version() {
        tracing::warn!(conn, client = %hello.client, error = %e, "C1 version mismatch");
        let res = Response::err(HANDSHAKE_ID, ErrorCode::VersionMismatch, e.to_string());
        write_now(&mut wr, &ServerMsg::Res(res)).await;
        return;
    }
    let events = shared.events.subscribe();
    let welcome = ServerMsg::Welcome(Welcome {
        v: ply_proto::PROTOCOL_VERSION,
        daemon_version: crate::daemon::BUILD_ID.to_owned(),
    });
    if !write_now(&mut wr, &welcome).await {
        return;
    }
    tracing::debug!(conn, client = %hello.client, version = %hello.app_version, "C1 client connected");
    let (tx, rx) = mpsc::channel(RESPONSE_CAPACITY);
    let writer = tokio::spawn(write_loop(wr, rx, events, conn));
    loop {
        line.clear();
        match read_line(&mut rd, &mut line).await {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                tracing::warn!(conn, error = %e, "closing the C1 connection");
                break;
            }
        }
        let (response, stop) = match ClientMsg::decode(&line) {
            Ok(ClientMsg::Req(req)) => {
                let stop = match &req.call {
                    Call::DaemonShutdown(DaemonShutdownParams { kill_panes }) => Some(*kill_panes),
                    _ => None,
                };
                let method = req.call.method();
                let outcome = dispatch(&shared, req.call).await;
                if let Err(e) = &outcome {
                    tracing::info!(conn, method, code = ?e.code, msg = %e.msg, "request refused");
                }
                (
                    Response {
                        id: req.id,
                        outcome,
                    },
                    stop,
                )
            }
            Ok(ClientMsg::Hello(_)) => {
                tracing::warn!(conn, "a second hello on one connection ignored");
                continue;
            }
            Err(rejection) => match rejection.id {
                Some(id) => {
                    tracing::info!(conn, error = %rejection, "request rejected");
                    (
                        Response {
                            id,
                            outcome: Err(rejection.error),
                        },
                        None,
                    )
                }
                None => {
                    tracing::warn!(conn, error = %rejection, "unreadable C1 line skipped");
                    continue;
                }
            },
        };
        let bytes = match encode_line(&ServerMsg::Res(response)) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::error!(conn, error = %e, "cannot encode a response");
                continue;
            }
        };
        if tx.send(bytes).await.is_err() {
            break;
        }
        if let Some(kill_panes) = stop {
            shared.request_shutdown(kill_panes);
        }
    }
    drop(tx);
    if let Err(e) = writer.await {
        tracing::debug!(conn, error = %e, "C1 writer ended abnormally");
    }
    tracing::debug!(conn, "C1 client disconnected");
}

async fn write_loop(
    mut wr: OwnedWriteHalf,
    mut responses: mpsc::Receiver<Vec<u8>>,
    mut events: broadcast::Receiver<Event>,
    conn: u64,
) {
    loop {
        let bytes = tokio::select! {
            biased;
            msg = responses.recv() => match msg {
                Some(bytes) => bytes,
                None => break,
            },
            event = events.recv() => match event {
                Ok(event) => match encode_line(&ServerMsg::Evt(event)) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::error!(conn, error = %e, "cannot encode an event");
                        continue;
                    }
                },
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(conn, missed, "C1 client fell behind the events; closing it so it reloads");
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        };
        if let Err(e) = wr.write_all(&bytes).await {
            tracing::debug!(conn, error = %e, "C1 write failed");
            break;
        }
    }
}

async fn write_now(wr: &mut OwnedWriteHalf, msg: &ServerMsg) -> bool {
    let bytes = match encode_line(msg) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!(error = %e, "cannot encode a C1 message");
            return false;
        }
    };
    match wr.write_all(&bytes).await {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!(error = %e, "C1 write failed");
            false
        }
    }
}

/// Reads one line including its `\n` into `out` (appending); `Ok(false)` at a clean end of stream.
/// Fails with `InvalidData` once the line passes [`MAX_LINE_BYTES`] and `UnexpectedEof` inside a line.
pub async fn read_line<R: AsyncBufRead + Unpin>(
    r: &mut R,
    out: &mut Vec<u8>,
) -> std::io::Result<bool> {
    read_line_max(r, out, MAX_LINE_BYTES).await
}

/// [`read_line`] with the cap `max` bytes (newline included) instead of C1's.
pub async fn read_line_max<R: AsyncBufRead + Unpin>(
    r: &mut R,
    out: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<bool> {
    loop {
        let buf = r.fill_buf().await?;
        if buf.is_empty() {
            return if out.is_empty() {
                Ok(false)
            } else {
                Err(std::io::ErrorKind::UnexpectedEof.into())
            };
        }
        let (take, done) = match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        out.extend_from_slice(&buf[..take]);
        r.consume(take);
        if out.len() > max {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("a line passed the {max}-byte cap"),
            ));
        }
        if done {
            return Ok(true);
        }
    }
}

fn ok<T: Serialize>(value: &T) -> Result<Value, ErrorBody> {
    serde_json::to_value(value).map_err(|e| {
        internal(
            "cannot encode a result",
            &crate::Error::Json {
                what: "result",
                source: e,
            },
        )
    })
}

async fn dispatch(shared: &Arc<Shared>, call: Call) -> Result<Value, ErrorBody> {
    match call {
        Call::WorkspaceList(Empty {}) => ok(&shared.registry().workspaces()),
        Call::WorkspaceOpen(p) => {
            let ws = shared.registry().open_workspace(&p.path, unix_now())?;
            ok(&ws)
        }
        Call::PaneList(p) => {
            let panes = shared.registry().panes_of(p.workspace_id)?;
            ok(&panes)
        }
        Call::PaneCreate(p) => ok(&launch::create(shared, p).await?),
        Call::PaneClose(p) => close(shared, p).await,
        Call::PaneAnswer(p) => answer(shared, p).await,
        Call::PaneResume(p) => ok(&launch::resume(shared, p.pane_id).await?),
        Call::SessionList(p) => {
            let sessions = shared
                .registry()
                .sessions(p.workspace_id, p.include_closed)?;
            ok(&sessions)
        }
        Call::ThemeSet(p) => theme_set(shared, p).await,
        Call::LayoutGet(p) => {
            let layout = shared.registry().layout(p.workspace_id)?;
            ok(&layout)
        }
        Call::LayoutSave(p) => {
            shared.registry().save_layout(p.workspace_id, &p.layout)?;
            ok(&Empty {})
        }
        Call::SettingsGet(Empty {}) => ok(&shared.registry().settings()),
        Call::SettingsSet(p) => settings_set(shared, p.settings).await,
        Call::DaemonShutdown(_) => ok(&Empty {}),
        Call::UsageGet(Empty {}) => {
            let sources = UsageSources::from_env(&shared.login.base);
            ok(&shared.usage.get(sources).await)
        }
        Call::SkillList(p) => {
            let cwd = std::path::PathBuf::from(&p.cwd);
            if !cwd.is_absolute() {
                return Err(refuse(
                    ErrorCode::BadRequest,
                    "cwd must be an absolute path",
                ));
            }
            let sources = SkillSources::from_env(&shared.login.base);
            ok(&shared.skills.get(sources, p.cli, cwd).await)
        }
        Call::TaskList(p) => ok(&shared.registry().tasks_of(p.workspace_id)?),
        Call::TaskAdd(p) => {
            if shared.is_stopping() {
                return Err(refuse(ErrorCode::ShuttingDown, "plyd is shutting down"));
            }
            let task = shared.registry().add_task(&p, unix_now())?;
            ok(&task)
        }
        Call::TaskCancel(p) => {
            shared.registry().cancel_task(p.task_id, unix_now())?;
            ok(&Empty {})
        }
        Call::TaskMove(p) => {
            shared.registry().move_task(p.task_id, p.position)?;
            ok(&Empty {})
        }
        Call::QueuePause(p) => {
            shared.registry().pause_queue(p.pane_id, p.paused)?;
            ok(&Empty {})
        }
        Call::TaskSend(_) => Err(refuse(ErrorCode::Internal, "not in this build of plyd yet")),
    }
}

async fn close(shared: &Shared, p: PaneCloseParams) -> Result<Value, ErrorBody> {
    let (handle, killing) = {
        let mut reg = shared.registry();
        let entry = reg.require(p.pane_id)?;
        let handle = entry.handle.clone();
        if is_live(entry.pane.status) {
            if !p.kill {
                return Err(refuse(
                    ErrorCode::PaneAlive,
                    "the pane's process is still running",
                ));
            }
            reg.close_on_exit(p.pane_id);
            (handle, true)
        } else {
            (reg.close_pane(p.pane_id, unix_now())?, false)
        }
    };
    let cmd = if killing {
        PaneCmd::Kill
    } else {
        PaneCmd::Stop
    };
    if let Some(handle) = handle
        && handle.send(cmd).await.is_err()
    {
        tracing::debug!(pane_id = p.pane_id, "the pane task had already stopped");
    }
    if !killing {
        launch::remove_pane_dir(p.pane_id, &shared.paths.pane_dir(p.pane_id));
    }
    ok(&Empty {})
}

/// The keys for `answer` (R55): `1` is the first option, "Yes" in every dialog seen; ESC cancels, so No never approves.
pub fn answer_keys(answer: Answer) -> &'static [u8] {
    match answer {
        Answer::Yes => b"1",
        Answer::No => b"\x1b",
    }
}

async fn answer(shared: &Shared, p: PaneAnswerParams) -> Result<Value, ErrorBody> {
    let handle = {
        let reg = shared.registry();
        let entry = reg.require(p.pane_id)?;
        if !matches!(
            entry.pane.status,
            PaneStatus::WaitingPermission | PaneStatus::WaitingInput
        ) {
            return Err(refuse(ErrorCode::InvalidState, "the pane shows no dialog"));
        }
        entry.handle.clone()
    };
    let keys = answer_keys(p.answer).to_vec();
    tracing::info!(pane_id = p.pane_id, answer = ?p.answer, "answering the pane's dialog");
    match handle {
        Some(handle) if handle.send(PaneCmd::Write(keys)).await.is_ok() => ok(&Empty {}),
        _ => {
            tracing::warn!(pane_id = p.pane_id, "the pane task is gone; answer dropped");
            Err(refuse(ErrorCode::Internal, "the pane task is gone"))
        }
    }
}

/// Applies the palette everywhere before writing `config.toml`, so an unwritable file never keeps spawns waiting; its failure is answered `internal` afterwards.
async fn theme_set(shared: &Shared, p: ThemeSetParams) -> Result<Value, ErrorBody> {
    let palette = Palette::from_theme(&p.palette);
    let (handles, saved) = {
        let mut reg = shared.registry();
        reg.set_palette(p.palette);
        (reg.handles(), reg.save_config())
    };
    shared.palette.send_replace(Some(palette.clone()));
    for (pane_id, handle) in handles {
        if handle
            .send(PaneCmd::SetPalette(palette.clone()))
            .await
            .is_err()
        {
            tracing::debug!(pane_id, "the pane task had stopped before the new palette");
        }
    }
    saved?;
    ok(&Empty {})
}

/// Like [`theme_set`]: the settings apply at once, and a failed `config.toml` write is answered `internal` afterwards.
async fn settings_set(shared: &Shared, settings: Settings) -> Result<Value, ErrorBody> {
    let (old, handles, saved) = {
        let mut reg = shared.registry();
        let old = reg.settings();
        reg.set_settings(settings.clone());
        (old, reg.handles(), reg.save_config())
    };
    if old.option_as_meta != settings.option_as_meta {
        for (pane_id, handle) in handles {
            if handle
                .send(PaneCmd::SetOptionAsMeta(settings.option_as_meta))
                .await
                .is_err()
            {
                tracing::debug!(pane_id, "the pane task had stopped before the new setting");
            }
        }
    }
    shared.update_power();
    saved?;
    ok(&Empty {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lines_are_capped_and_split_at_newlines() {
        let input: &[u8] = b"{\"a\":1}\n{\"b\":2}\npartial";
        let mut r = BufReader::new(input);
        let mut line = Vec::new();
        assert!(read_line(&mut r, &mut line).await.unwrap());
        assert_eq!(line, b"{\"a\":1}\n");
        line.clear();
        assert!(read_line(&mut r, &mut line).await.unwrap());
        assert_eq!(line, b"{\"b\":2}\n");
        line.clear();
        assert!(
            read_line(&mut r, &mut line).await.is_err(),
            "EOF inside a line"
        );
        let big = vec![b'x'; MAX_LINE_BYTES + 10];
        let mut r = BufReader::new(big.as_slice());
        let mut line = Vec::new();
        let err = read_line(&mut r, &mut line).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let mut r = BufReader::new(&b""[..]);
        assert!(!read_line(&mut r, &mut Vec::new()).await.unwrap());
    }

    #[test]
    fn a_no_is_a_cancel_and_a_yes_the_first_option() {
        assert_eq!(answer_keys(Answer::Yes), b"1");
        assert_eq!(answer_keys(Answer::No), [0x1b], "ESC, never a digit");
    }

    #[test]
    fn the_welcome_names_the_commit_plyd_was_built_from() {
        let (version, commit) = crate::daemon::BUILD_ID.split_once('+').unwrap();
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
        let hash = commit.len() == 12 && commit.bytes().all(|b| b.is_ascii_hexdigit());
        let time = commit
            .strip_prefix('t')
            .is_some_and(|t| t.parse::<u64>().is_ok());
        assert!(hash || time, "{commit}");
    }
}
