//! C2 screen data over `run/data.sock` (spec 4.2): one connection per attached pane view.
//!
//! The first frame must be ATTACH with [`ply_proto::C2_VERSION`], the id of an open pane and a grid whose Snapshot
//! fits one frame ([`crate::pty::Geometry::fits_one_frame`]); anything else is answered with ATTACH_REFUSED (reason
//! and a readable message) and the connection closes. After ATTACH the
//! connection has two halves: a reader that decodes client frames (length-capped at
//! [`ply_proto::data::MAX_FRAME_LEN`] before any allocation) and hands them to the pane task, and a writer that
//! drains the frames the pane task encoded for this client. The pane task decides what is sent and when; a client
//! that sends a server kind or a second ATTACH, or a malformed frame, is disconnected. Either half ending ends both,
//! and the pane task forgets the client.

use std::sync::Arc;

use ply_proto::data::{AttachRefused, Frame, HEADER_LEN, MAX_FRAME_LEN, RefuseReason};
use ply_proto::pane::PaneId;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::daemon::Shared;
use crate::panes::pane::{CLIENT_QUEUE, PaneCmd};
use crate::pty::Geometry;

/// Accepts C2 connections until the task is aborted.
pub async fn serve(listener: UnixListener, shared: Arc<Shared>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(connection(stream, Arc::clone(&shared)));
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot accept a C2 connection");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// Reads one frame; `Ok(None)` at a clean end of stream between frames.
/// Fails with [`ply_proto::Error::FrameTooLarge`] before allocating for an oversized header, else a decode or I/O error.
pub async fn read_frame<R: AsyncRead + Unpin>(
    r: &mut R,
    buf: &mut Vec<u8>,
) -> Result<Option<Frame>, ply_proto::Error> {
    let mut header = [0u8; HEADER_LEN];
    if r.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    r.read_exact(&mut header[1..]).await?;
    let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let len = usize::try_from(len).unwrap_or(usize::MAX);
    if len > MAX_FRAME_LEN {
        return Err(ply_proto::Error::FrameTooLarge { len: len as u64 });
    }
    buf.resize(len, 0);
    r.read_exact(buf).await?;
    Frame::decode(header[4], buf).map(Some)
}

async fn connection(stream: UnixStream, shared: Arc<Shared>) {
    let client = shared.next_client_id();
    let (mut rd, mut wr) = stream.into_split();
    let mut buf = Vec::new();
    let attach = match read_frame(&mut rd, &mut buf).await {
        Ok(Some(Frame::Attach(attach))) => attach,
        Ok(Some(other)) => {
            tracing::warn!(
                client,
                kind = other.kind(),
                "C2 connection did not start with ATTACH"
            );
            refuse(
                &mut wr,
                None,
                RefuseReason::NotAttached,
                "the first frame must be ATTACH",
            )
            .await;
            return;
        }
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(client, error = %e, "invalid first C2 frame");
            refuse(
                &mut wr,
                None,
                RefuseReason::NotAttached,
                &format!("invalid ATTACH: {e}"),
            )
            .await;
            return;
        }
    };
    let pane_id = attach.pane_id;
    if let Err(e) = ply_proto::version::check_version("C2", ply_proto::C2_VERSION, attach.v) {
        tracing::warn!(client, pane_id, error = %e, "C2 version mismatch");
        refuse(
            &mut wr,
            Some(pane_id),
            RefuseReason::VersionMismatch,
            &e.to_string(),
        )
        .await;
        return;
    }
    let geometry = Geometry {
        cols: attach.cols,
        rows: attach.rows,
        cell_width_px: attach.cell_width_px,
        cell_height_px: attach.cell_height_px,
    };
    if !geometry.fits_one_frame() {
        tracing::warn!(
            client,
            pane_id,
            cols = geometry.cols,
            rows = geometry.rows,
            "ATTACH with a grid whose Snapshot cannot fit one frame"
        );
        refuse(
            &mut wr,
            Some(pane_id),
            RefuseReason::NotAttached,
            &format!(
                "a {} x {} grid is too large for one screen frame",
                geometry.cols, geometry.rows
            ),
        )
        .await;
        return;
    }
    let handle = shared
        .registry()
        .entry(pane_id)
        .and_then(|e| e.handle.clone());
    let Some(handle) = handle else {
        tracing::info!(client, pane_id, "ATTACH to an unknown pane");
        refuse(
            &mut wr,
            Some(pane_id),
            RefuseReason::UnknownPane,
            &format!("no pane {pane_id}"),
        )
        .await;
        return;
    };
    let (out, mut frames) = mpsc::channel(CLIENT_QUEUE);
    if handle
        .send(PaneCmd::Attach {
            client,
            geometry,
            out,
        })
        .await
        .is_err()
    {
        tracing::info!(client, pane_id, "ATTACH to a pane that just closed");
        refuse(
            &mut wr,
            Some(pane_id),
            RefuseReason::UnknownPane,
            &format!("pane {pane_id} closed"),
        )
        .await;
        return;
    }
    let reader = tokio::spawn(read_loop(rd, handle.clone(), client, pane_id));
    while let Some(bytes) = frames.recv().await {
        if let Err(e) = wr.write_all(&bytes).await {
            tracing::debug!(client, pane_id, error = %e, "C2 write failed");
            break;
        }
    }
    reader.abort();
    if handle.try_send(PaneCmd::Detach { client }).is_err() {
        tracing::trace!(
            client,
            pane_id,
            "detach not delivered; the pane task is gone or busy"
        );
    }
    tracing::debug!(client, pane_id, "C2 connection closed");
}

async fn read_loop(
    mut rd: OwnedReadHalf,
    handle: mpsc::Sender<PaneCmd>,
    client: u64,
    pane_id: PaneId,
) {
    let mut buf = Vec::new();
    loop {
        match read_frame(&mut rd, &mut buf).await {
            Ok(Some(frame)) if frame.is_from_client() && !matches!(frame, Frame::Attach(_)) => {
                if handle.send(PaneCmd::Frame { client, frame }).await.is_err() {
                    break;
                }
            }
            Ok(Some(frame)) => {
                tracing::warn!(
                    client,
                    pane_id,
                    kind = frame.kind(),
                    "C2 client sent a frame it may not send; closing"
                );
                break;
            }
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(client, pane_id, error = %e, "invalid C2 frame; closing");
                break;
            }
        }
    }
    if handle.send(PaneCmd::Detach { client }).await.is_err() {
        tracing::trace!(client, pane_id, "the pane task is gone");
    }
}

async fn refuse(
    wr: &mut OwnedWriteHalf,
    pane_id: Option<PaneId>,
    reason: RefuseReason,
    message: &str,
) {
    let mut bytes = Vec::new();
    let frame = Frame::AttachRefused(AttachRefused {
        reason,
        message: message.to_owned(),
    });
    if let Err(e) = frame.encode(&mut bytes) {
        tracing::error!(?pane_id, error = %e, "cannot encode ATTACH_REFUSED");
        return;
    }
    if let Err(e) = wr.write_all(&bytes).await {
        tracing::debug!(?pane_id, error = %e, "cannot send ATTACH_REFUSED");
        return;
    }
    if let Err(e) = wr.shutdown().await {
        tracing::debug!(?pane_id, error = %e, "cannot shut the C2 connection down");
    }
}
