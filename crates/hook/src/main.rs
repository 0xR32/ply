//! `ply-hook`, the forwarder Claude Code hooks and Codex's notify program run (C3, spec 4.3).
//!
//! `ply-hook claude <Event>` reads the hook's JSON from stdin; `ply-hook codex <json>` takes Codex's notify JSON from
//! its last argument. Either payload (at most `MAX_PAYLOAD_BYTES`) is wrapped verbatim in the envelope
//! `{"v":1,"pane_id":…,"cli":…,"event":…,"payload":…}` with the pane id from `PLY_PANE_ID`, and written as one line to
//! the Unix socket named by `PLY_HOOK_SOCK`.
//!
//! It must never block or fail the CLI (INV-12) and never decide anything for it (INV-14): it prints nothing to stdout
//! or stderr, exits 0 on every path, and gives up once `DEADLINE` has passed since start, dropping the payload. With
//! nowhere to log, a failure is simply a missing envelope; plyd falls back to its other signals (C3). It depends on std
//! and serde_json only (spec 8.2), so it builds the line by hand; `ply_proto::hook::HookEnvelope` decodes it.
//!
//! `ply-hook statusline [<command>]` is Claude Code's status line in a ply pane. It sends the status line payload to plyd
//! as a `StatusLine` envelope, for the plan usage in its `rate_limits`, within the same `DEADLINE`. Outside a ply pane
//! (no `PLY_PANE_ID`) it reports as pane 0 to `PLY_HOOK_SOCK`, else to plyd's socket under `PLY_HOME` or
//! `~/Library/Application Support/ply`, so any Claude Code status line can hand plyd its numbers. Meanwhile it runs the
//! user's own status line command, when given, through `/bin/sh -c` with the same payload on stdin. It prints only what
//! that command prints and exits with its code, so the pane shows the status line it would show without ply.

#![forbid(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::value::RawValue;

/// Total time from start after which `ply-hook` exits 0 whatever it is doing (spec 4.3).
const DEADLINE: Duration = Duration::from_millis(200);

/// Largest payload forwarded; equals `ply_proto::hook::MAX_PAYLOAD_BYTES` (1 MiB less 4 KiB of envelope room).
const MAX_PAYLOAD_BYTES: usize = (1 << 20) - 4096;

/// Longest hook event name accepted; real names are under 32 bytes.
const MAX_EVENT_BYTES: usize = 64;

/// Longest the status line waits for Claude Code to finish writing its payload; it closes stdin once written.
const STDIN_WAIT: Duration = Duration::from_secs(1);

/// The status line's envelope event, `ply_agents::claude::settings::STATUS_LINE_EVENT`.
const STATUS_LINE_EVENT: &str = "StatusLine";

/// The C3 envelope version, `ply_proto::HOOK_VERSION`.
const HOOK_VERSION: u8 = 1;

const ENV_PANE_ID: &str = "PLY_PANE_ID";
const ENV_HOOK_SOCK: &str = "PLY_HOOK_SOCK";

fn main() {
    let started = Instant::now();
    std::panic::set_hook(Box::new(|_| {}));
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "statusline") {
        std::process::exit(status_line(args.get(1).map(OsString::as_os_str), started));
    }
    let (done_tx, done_rx) = mpsc::sync_channel::<()>(1);
    let worker = thread::Builder::new()
        .name("ply-hook".to_owned())
        .spawn(move || {
            let _ = forward(&args, started);
            let _ = done_tx.send(());
        });
    if worker.is_ok() {
        let _ = done_rx.recv_timeout(DEADLINE.saturating_sub(started.elapsed()));
    }
    // Exiting here also ends a worker still blocked on stdin, connect or write.
    std::process::exit(0);
}

/// Reports the status line payload to plyd, runs the user's `command` on it, and returns that command's exit code (0 without one, or when Claude Code never finished writing).
fn status_line(command: Option<&OsStr>, started: Instant) -> i32 {
    let (read_tx, read_rx) = mpsc::sync_channel::<Option<Vec<u8>>>(1);
    let reader = thread::Builder::new()
        .name("ply-hook-stdin".to_owned())
        .spawn(move || {
            let _ = read_tx.send(read_stdin());
        });
    let payload = match reader
        .ok()
        .and_then(|_| read_rx.recv_timeout(STDIN_WAIT).ok())
    {
        Some(Some(payload)) => payload,
        _ => return 0,
    };
    let (done_tx, done_rx) = mpsc::sync_channel::<()>(1);
    let report = payload.clone();
    let reporter = thread::Builder::new()
        .name("ply-hook".to_owned())
        .spawn(move || {
            let _ = report_status_line(report, started);
            let _ = done_tx.send(());
        });
    let code = command.map_or(0, |command| run_own(command, payload));
    if reporter.is_ok() {
        let _ = done_rx.recv_timeout(DEADLINE.saturating_sub(started.elapsed()));
    }
    code
}

/// Sends one `StatusLine` envelope, as pane 0 outside a ply pane; `None` on any reason to drop it.
fn report_status_line(payload: Vec<u8>, started: Instant) -> Option<()> {
    let (pane_id, socket) = target().or_else(outside_target)?;
    let line = envelope(pane_id, "claude", Some(STATUS_LINE_EVENT), payload)?;
    send(&socket, &line, started)
}

/// Runs the user's status line `command` through `/bin/sh -c` with `payload` on stdin; its exit code, 0 when it cannot start or was killed.
fn run_own(command: &OsStr, payload: Vec<u8>) -> i32 {
    let Ok(mut child) = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .spawn()
    else {
        return 0;
    };
    // A command that never reads its stdin must not block this write, so it runs on a thread of its own.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = thread::Builder::new()
            .name("ply-hook-feed".to_owned())
            .spawn(move || {
                let _ = stdin.write_all(&payload);
            });
    }
    child.wait().ok().and_then(|s| s.code()).unwrap_or(0)
}

/// The pane id and C3 socket from the environment plyd gave the pane.
fn target() -> Option<(u64, PathBuf)> {
    let pane_id: u64 = std::env::var(ENV_PANE_ID).ok()?.trim().parse().ok()?;
    let socket = PathBuf::from(std::env::var_os(ENV_HOOK_SOCK).filter(|s| !s.is_empty())?);
    Some((pane_id, socket))
}

/// Pane 0 and plyd's C3 socket, for a status line running outside ply.
fn outside_target() -> Option<(u64, PathBuf)> {
    if let Some(socket) = std::env::var_os(ENV_HOOK_SOCK).filter(|s| !s.is_empty()) {
        return Some((0, PathBuf::from(socket)));
    }
    let data = match std::env::var_os("PLY_HOME").filter(|s| !s.is_empty()) {
        Some(home) => PathBuf::from(home),
        None => PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/ply"),
    };
    Some((0, data.join("run/hook.sock")))
}

/// Writes `line` to `socket` within what is left of `DEADLINE`.
fn send(socket: &Path, line: &[u8], started: Instant) -> Option<()> {
    let remaining = DEADLINE.checked_sub(started.elapsed())?;
    let mut stream = UnixStream::connect(socket).ok()?;
    stream
        .set_write_timeout(Some(remaining.max(Duration::from_millis(1))))
        .ok()?;
    stream.write_all(line).ok()?;
    stream.flush().ok()
}

/// Builds and sends the envelope; `None` on any reason to drop it, which the caller ignores.
fn forward(args: &[OsString], started: Instant) -> Option<()> {
    let cli = args.first()?.to_str()?;
    let (pane_id, socket) = target()?;
    let (event, payload) = match cli {
        "claude" => {
            let event = args.get(1).and_then(|e| e.to_str()).map(str::to_owned);
            (event, read_stdin()?)
        }
        "codex" => (None, args.get(1..)?.last()?.to_str()?.as_bytes().to_vec()),
        _ => return None,
    };
    let line = envelope(pane_id, cli, event.as_deref(), payload)?;
    send(&socket, &line, started)
}

/// Stdin up to the cap; an oversized payload is drained (so the CLI's write does not fail) and dropped.
fn read_stdin() -> Option<Vec<u8>> {
    let mut stdin = io::stdin().lock();
    let mut payload = Vec::new();
    let cap = u64::try_from(MAX_PAYLOAD_BYTES).ok()?;
    (&mut stdin).take(cap + 1).read_to_end(&mut payload).ok()?;
    if payload.len() > MAX_PAYLOAD_BYTES {
        let _ = io::copy(&mut stdin, &mut io::sink());
        return None;
    }
    Some(payload)
}

/// The envelope line, or `None` when the payload is not one JSON value or a field is out of range.
fn envelope(pane_id: u64, cli: &str, event: Option<&str>, mut payload: Vec<u8>) -> Option<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD_BYTES || event.is_some_and(|e| e.len() > MAX_EVENT_BYTES) {
        return None;
    }
    // JSON strings cannot hold a raw CR or LF, so these bytes are whitespace and the payload stays one line.
    for b in &mut payload {
        if matches!(*b, b'\n' | b'\r') {
            *b = b' ';
        }
    }
    let text = std::str::from_utf8(&payload).ok()?.trim();
    let raw: &RawValue = serde_json::from_str(text).ok()?;
    let mut line = format!(r#"{{"v":{HOOK_VERSION},"pane_id":{pane_id},"cli":"{cli}""#);
    if let Some(event) = event {
        line.push_str(r#","event":"#);
        line.push_str(&serde_json::to_string(event).ok()?);
    }
    line.push_str(r#","payload":"#);
    line.push_str(raw.get());
    line.push_str("}\n");
    Some(line.into_bytes())
}
