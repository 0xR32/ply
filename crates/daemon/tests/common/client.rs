//! A small blocking client for plyd's C1 and C2 sockets, shared by the integration tests and `examples/ply-cli.rs`.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use ply_proto::control::{ErrorBody, Event, ServerMsg};
use ply_proto::data::{Ack, Attach, Frame, HEADER_LEN, MAX_FRAME_LEN};
use ply_term::Replica;
use serde_json::{Value, json};

/// A C1 connection after the handshake; events that arrive while waiting for a response are queued.
pub struct Control {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    partial: Vec<u8>,
    next_id: u64,
    /// Events received and not yet taken.
    pub events: VecDeque<Event>,
}

impl Control {
    /// Connects and completes `hello`/`welcome`.
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        let mut c = Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            partial: Vec::new(),
            next_id: 1,
            events: VecDeque::new(),
        };
        c.send_line(&json!({"t":"hello","v":1,"client":"ply-cli","app_version":"0.1.0"}))?;
        match c.read_msg(Duration::from_secs(10))? {
            Some(ServerMsg::Welcome(_)) => Ok(c),
            other => Err(io::Error::other(format!("expected welcome, got {other:?}"))),
        }
    }

    fn send_line(&mut self, value: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        self.writer.write_all(&line)
    }

    fn read_msg(&mut self, timeout: Duration) -> io::Result<Option<ServerMsg>> {
        self.reader.get_ref().set_read_timeout(Some(timeout))?;
        match self.reader.read_until(b'\n', &mut self.partial) {
            Ok(0) => Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(_) if self.partial.ends_with(b"\n") => {
                let line = std::mem::take(&mut self.partial);
                serde_json::from_slice(&line)
                    .map(Some)
                    .map_err(|e| io::Error::other(format!("bad line from plyd: {e}")))
            }
            Ok(_) => Ok(None),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Calls `method` with `params` and waits up to 15 s for its response.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, ErrorBody> {
        let id = self.next_id;
        self.next_id += 1;
        let io_err = |e: io::Error| ErrorBody {
            code: ply_proto::control::ErrorCode::Internal,
            msg: format!("client I/O: {e}"),
        };
        self.send_line(&json!({"t":"req","id":id,"m":method,"p":params}))
            .map_err(io_err)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            match self.read_msg(Duration::from_millis(200)).map_err(io_err)? {
                Some(ServerMsg::Res(res)) if res.id == id => return res.outcome,
                Some(ServerMsg::Evt(event)) => self.events.push_back(event),
                Some(other) => panic!("unexpected message {other:?}"),
                None => {}
            }
        }
        Err(io_err(io::ErrorKind::TimedOut.into()))
    }

    /// The first queued or arriving event matching `pred` within `timeout`; earlier non-matching events are dropped.
    pub fn wait_event(
        &mut self,
        timeout: Duration,
        pred: impl Fn(&Event) -> bool,
    ) -> Option<Event> {
        while let Some(event) = self.events.pop_front() {
            if pred(&event) {
                return Some(event);
            }
        }
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.read_msg(Duration::from_millis(100)) {
                Ok(Some(ServerMsg::Evt(event))) if pred(&event) => return Some(event),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }
}

/// A C2 connection with the reference replica of what it was sent.
pub struct Data {
    stream: UnixStream,
    buf: Vec<u8>,
    /// The screen as the frames describe it.
    pub replica: Replica,
    /// The last TITLE.
    pub title: Option<String>,
    /// The EXIT code, once sent.
    pub exit: Option<i32>,
    /// Every CLIPBOARD_WRITE text, in order.
    pub clipboard: Vec<String>,
    /// Snapshots received, the attach one included.
    pub snapshots: usize,
    /// Deltas received.
    pub deltas: usize,
}

impl Data {
    /// Connects and sends ATTACH; the first frame (a Snapshot, or ATTACH_REFUSED) is returned unapplied.
    pub fn attach(path: &Path, pane_id: u64, cols: u16, rows: u16) -> io::Result<(Self, Frame)> {
        Self::attach_v(path, ply_proto::C2_VERSION, pane_id, cols, rows)
    }

    /// [`Data::attach`] with an explicit C2 version.
    pub fn attach_v(
        path: &Path,
        v: u16,
        pane_id: u64,
        cols: u16,
        rows: u16,
    ) -> io::Result<(Self, Frame)> {
        let stream = UnixStream::connect(path)?;
        let mut d = Self {
            stream,
            buf: Vec::new(),
            replica: Replica::new(),
            title: None,
            exit: None,
            clipboard: Vec::new(),
            snapshots: 0,
            deltas: 0,
        };
        d.send(&Frame::Attach(Attach {
            v,
            pane_id,
            cols,
            rows,
            cell_width_px: 8,
            cell_height_px: 16,
        }))?;
        let first = d
            .recv(Duration::from_secs(10))?
            .ok_or_else(|| io::Error::other("no answer to ATTACH"))?;
        Ok((d, first))
    }

    /// Sends one frame.
    pub fn send(&mut self, frame: &Frame) -> io::Result<()> {
        let mut bytes = Vec::new();
        frame
            .encode(&mut bytes)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.stream.write_all(&bytes)
    }

    /// Types `bytes` into the pane as INPUT_RAW.
    pub fn input(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.send(&Frame::InputRaw(bytes.to_vec()))
    }

    /// The next frame within `timeout`; `Ok(None)` on timeout, an error once plyd closed the connection.
    pub fn recv(&mut self, timeout: Duration) -> io::Result<Option<Frame>> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.buf.len() >= HEADER_LEN {
                let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]])
                    as usize;
                if len > MAX_FRAME_LEN {
                    return Err(io::Error::other(format!("frame of {len} bytes")));
                }
                if self.buf.len() >= HEADER_LEN + len {
                    let kind = self.buf[4];
                    let frame = Frame::decode(kind, &self.buf[HEADER_LEN..HEADER_LEN + len])
                        .map_err(|e| io::Error::other(e.to_string()))?;
                    self.buf.drain(..HEADER_LEN + len);
                    return Ok(Some(frame));
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            self.stream.set_read_timeout(Some(deadline - now))?;
            let mut chunk = [0u8; 64 * 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Applies a frame to the replica and the recorded title and exit code; ACKs Snapshots and Deltas when `ack`.
    pub fn apply(&mut self, frame: &Frame, ack: bool) -> io::Result<()> {
        match frame {
            Frame::Snapshot(s) => {
                self.snapshots += 1;
                self.replica
                    .apply(frame)
                    .map_err(|e| io::Error::other(e.to_string()))?;
                if ack {
                    self.send(&Frame::Ack(Ack { seq: s.seq }))?;
                }
            }
            Frame::Delta(d) => {
                self.deltas += 1;
                self.replica
                    .apply(frame)
                    .map_err(|e| io::Error::other(e.to_string()))?;
                if ack {
                    self.send(&Frame::Ack(Ack { seq: d.seq }))?;
                }
            }
            Frame::History(_) => {
                self.replica
                    .apply(frame)
                    .map_err(|e| io::Error::other(e.to_string()))?;
            }
            Frame::Title(t) => self.title = Some(t.clone()),
            Frame::Exit(e) => self.exit = Some(e.code),
            Frame::ClipboardWrite(text) => self.clipboard.push(text.clone()),
            _ => {}
        }
        Ok(())
    }

    /// Receives, applies and ACKs frames until `pred` holds (true) or `timeout` passes (false).
    pub fn pump_until(
        &mut self,
        timeout: Duration,
        pred: impl Fn(&Self) -> bool,
    ) -> io::Result<bool> {
        let deadline = Instant::now() + timeout;
        while !pred(self) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(false);
            }
            if let Some(frame) = self.recv(left)? {
                self.apply(&frame, true)?;
            }
        }
        Ok(true)
    }

    /// Receives, applies and ACKs frames until none arrives for `quiet`.
    pub fn settle(&mut self, quiet: Duration) -> io::Result<()> {
        while let Some(frame) = self.recv(quiet)? {
            self.apply(&frame, true)?;
        }
        Ok(())
    }

    /// Every screen row as text.
    pub fn screen(&self) -> Vec<String> {
        self.replica.screen_text()
    }

    /// Whether any screen row contains `needle`.
    pub fn shows(&self, needle: &str) -> bool {
        self.screen().iter().any(|row| row.contains(needle))
    }
}
