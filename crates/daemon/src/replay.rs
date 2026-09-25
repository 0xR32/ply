//! Debug-build replay for the P1 bench (spec WP5, WP11): recorded agent output streamed into live panes.
//!
//! `plyd --replay <dir> [--speed N] [--panes K]` connects to the running plyd of `PLY_HOME`, opens K shell panes in
//! `<dir>` and makes each one `exec` a feeder, `plyd --replay-feed <file> [--speed N]`, which writes one of the
//! `*.bytes` streams in `<dir>` to its pty at N × [`BASE_BYTES_PER_SECOND`] and starts over at the end. The bytes
//! take the real path (pty reader thread, libghostty-vt, publisher, C2), so a view attached to those panes measures
//! what P1 asks for. The pane ids are printed one per line, ready for `PLY_DEMO_ATTACH`. The recorded streams carry
//! no timestamps, so "real speed" is the nominal agent output rate [`BASE_BYTES_PER_SECOND`], not the capture's own
//! pacing. The feeder attaches only to type the `exec` line and detaches again, so the viewer that attaches next
//! decides the pane size.
//!
//! Only debug builds contain this module (`lib.rs`); it writes nothing but the panes' own input.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ply_proto::C2_VERSION;
use ply_proto::data::{Attach, Frame, FrameReader};
use ply_proto::pane::PaneId;
use serde_json::{Value, json};

use crate::error::{Error, Result, io};
use crate::paths::Paths;

/// Bytes per second a replayed pane receives at speed 1: roughly what a streaming Claude Code or Codex TUI writes.
pub const BASE_BYTES_PER_SECOND: u64 = 4096;

/// Replay tick: the feeder writes one slice of its stream every 16 ms.
const TICK: Duration = Duration::from_millis(16);

/// How long a C1 or C2 read may wait before the replay gives up.
const WAIT: Duration = Duration::from_secs(15);

/// What `plyd --replay` sets up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayOptions {
    /// Directory holding the recorded `*.bytes` streams; the panes start in it.
    pub dir: PathBuf,
    /// Multiple of [`BASE_BYTES_PER_SECOND`], at least 1.
    pub speed: u32,
    /// Panes to open; they take the streams in name order, round robin.
    pub panes: usize,
}

struct Control {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    socket: PathBuf,
    next_id: u64,
}

impl Control {
    fn connect(socket: &Path) -> Result<Self> {
        let writer = UnixStream::connect(socket).map_err(io("cannot connect to", socket))?;
        writer
            .set_read_timeout(Some(WAIT))
            .map_err(io("cannot set a timeout on", socket))?;
        let reader = BufReader::new(writer.try_clone().map_err(io("cannot clone", socket))?);
        let mut c = Self {
            reader,
            writer,
            socket: socket.to_owned(),
            next_id: 1,
        };
        c.send(&json!({"t": "hello", "v": ply_proto::PROTOCOL_VERSION, "client": "plyd-replay", "app_version": env!("CARGO_PKG_VERSION")}))?;
        let welcome = c.read()?;
        if welcome["t"] != "welcome" {
            return Err(c.refused("hello", &welcome.to_string()));
        }
        Ok(c)
    }

    fn refused(&self, method: &str, why: &str) -> Error {
        tracing::warn!(method, why, "plyd refused a replay request");
        Error::Io {
            what: "plyd refused a replay request on",
            path: self.socket.clone(),
            source: std::io::Error::other(format!("{method}: {why}")),
        }
    }

    fn send(&mut self, value: &Value) -> Result<()> {
        let mut line = value.to_string().into_bytes();
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .map_err(io("cannot write to", &self.socket))
    }

    fn read(&mut self) -> Result<Value> {
        let mut line = Vec::new();
        let n = self
            .reader
            .read_until(b'\n', &mut line)
            .map_err(io("cannot read from", &self.socket))?;
        if n == 0 {
            return Err(self.refused("read", "plyd closed the connection"));
        }
        serde_json::from_slice(&line).map_err(|e| self.refused("read", &e.to_string()))
    }

    /// Calls `method`; `Err(code)` carries plyd's error code when it answered with an error.
    fn call(&mut self, method: &str, params: Value) -> Result<std::result::Result<Value, String>> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"t": "req", "id": id, "m": method, "p": params}))?;
        loop {
            let msg = self.read()?;
            if msg["t"] != "res" || msg["id"] != id {
                continue;
            }
            return Ok(if msg["ok"] == true {
                Ok(msg["r"].clone())
            } else {
                Err(msg["err"]["code"].as_str().unwrap_or("internal").to_owned())
            });
        }
    }

    fn require(&mut self, method: &str, params: Value) -> Result<Value> {
        match self.call(method, params)? {
            Ok(v) => Ok(v),
            Err(code) => Err(self.refused(method, &code)),
        }
    }
}

fn streams(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(io("cannot list", dir))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "bytes"))
        .collect();
    out.sort();
    if out.is_empty() {
        return Err(Error::Io {
            what: "no recorded *.bytes stream in",
            path: dir.to_owned(),
            source: ErrorKind::NotFound.into(),
        });
    }
    Ok(out)
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

fn grey(v: u8) -> String {
    format!("#{v:02X}{v:02X}{v:02X}")
}

fn type_into(data_socket: &Path, pane_id: PaneId, line: &str) -> Result<()> {
    let mut stream =
        UnixStream::connect(data_socket).map_err(io("cannot connect to", data_socket))?;
    stream
        .set_read_timeout(Some(WAIT))
        .map_err(io("cannot set a timeout on", data_socket))?;
    let mut wire = Vec::new();
    Frame::Attach(Attach {
        v: C2_VERSION,
        pane_id,
        cols: 80,
        rows: 24,
        cell_width_px: 8,
        cell_height_px: 16,
    })
    .encode(&mut wire)?;
    stream
        .write_all(&wire)
        .map_err(io("cannot write to", data_socket))?;
    let mut reader = FrameReader::new(
        stream
            .try_clone()
            .map_err(io("cannot clone", data_socket))?,
    );
    loop {
        match reader.read_frame()? {
            Some(Frame::Snapshot(_)) => break,
            Some(Frame::AttachRefused(r)) => {
                tracing::warn!(pane_id, message = %r.message, "the replay attach was refused");
                return Err(Error::Io {
                    what: "plyd refused the replay attach on",
                    path: data_socket.to_owned(),
                    source: std::io::Error::other(r.message),
                });
            }
            Some(_) => {}
            None => {
                return Err(Error::Io {
                    what: "plyd closed the replay connection on",
                    path: data_socket.to_owned(),
                    source: ErrorKind::UnexpectedEof.into(),
                });
            }
        }
    }
    wire.clear();
    Frame::InputRaw(format!("{line}\r").into_bytes()).encode(&mut wire)?;
    stream
        .write_all(&wire)
        .map_err(io("cannot write to", data_socket))?;
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

/// Opens the replay panes on the running plyd, starts a feeder in each and returns their ids; errors: [`Error::Io`] when plyd is unreachable, refuses a request or `options.dir` has no `*.bytes`, [`Error::Proto`] for C2 codec failures.
pub fn start(paths: &Paths, plyd: &Path, options: &ReplayOptions) -> Result<Vec<PaneId>> {
    let files = streams(&options.dir)?;
    let mut c = Control::connect(&paths.control_socket())?;
    let dir = options.dir.display().to_string();
    let workspace = c.require("workspace.open", json!({"path": dir}))?;
    let workspace_id = workspace["id"].clone();
    let mut ids = Vec::new();
    for i in 0..options.panes.max(1) {
        let params = json!({"workspace_id": workspace_id, "cli": "shell", "cwd": dir});
        let pane = match c.call("pane.create", params.clone())? {
            Ok(p) => p,
            Err(code) if code == "invalid_state" => {
                tracing::info!("plyd has no palette yet; the replay sends a neutral one");
                let ansi: Vec<String> = (0..16u8).map(|i| grey(i * 16)).collect();
                c.require(
                    "theme.set",
                    json!({"palette": {"ansi": ansi, "fg": grey(220), "bg": grey(16), "cursor": grey(240),
                        "cursorText": grey(16), "selectionBg": grey(64), "selectionFg": grey(220)}}),
                )?;
                c.require("pane.create", params)?
            }
            Err(code) => return Err(c.refused("pane.create", &code)),
        };
        let Some(id) = pane["id"].as_u64() else {
            return Err(c.refused("pane.create", "the answer has no pane id"));
        };
        let file = &files[i % files.len()];
        let line = format!(
            "exec {} --replay-feed {} --speed {}",
            quote(plyd),
            quote(file),
            options.speed.max(1)
        );
        type_into(&paths.data_socket(), id, &line)?;
        ids.push(id);
    }
    Ok(ids)
}

/// Writes `file` to stdout (the pane's pty) at `speed` × [`BASE_BYTES_PER_SECOND`], looping with a screen clear, blocking until the reader goes away; errors: [`Error::Io`] when `file` is unreadable or stdout fails other than by a closed pipe.
pub fn feed(file: &Path, speed: u32) -> Result<()> {
    let bytes = std::fs::read(file).map_err(io("cannot read", file))?;
    let per_tick = (BASE_BYTES_PER_SECOND * u64::from(speed.max(1)) * 16 / 1000).max(1);
    let per_tick = usize::try_from(per_tick).unwrap_or(usize::MAX);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    loop {
        for chunk in bytes
            .chunks(per_tick)
            .chain(std::iter::once(&b"\x1b[0m\x1b[H\x1b[2J"[..]))
        {
            if let Err(e) = out.write_all(chunk).and_then(|()| out.flush()) {
                if e.kind() == ErrorKind::BrokenPipe {
                    tracing::debug!("the replayed pane closed");
                    return Ok(());
                }
                tracing::warn!(error = %e, "replay feed failed");
                return Err(Error::Io {
                    what: "cannot write the replay to",
                    path: PathBuf::from("stdout"),
                    source: e,
                });
            }
            std::thread::sleep(TICK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quoting_survives_quotes_and_spaces() {
        assert_eq!(quote(Path::new("/tmp/a b/it's")), r"'/tmp/a b/it'\''s'");
    }

    #[test]
    fn only_bytes_files_are_streams_and_an_empty_dir_is_an_error() {
        let dir = std::env::temp_dir().join(format!("ply-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert!(streams(&dir).is_err());
        std::fs::write(dir.join("b.bytes"), b"x").expect("write");
        std::fs::write(dir.join("a.bytes"), b"x").expect("write");
        std::fs::write(dir.join("notes.txt"), b"x").expect("write");
        let found = streams(&dir).expect("streams");
        assert_eq!(found, vec![dir.join("a.bytes"), dir.join("b.bytes")]);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
