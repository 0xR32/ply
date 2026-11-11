//! The C1 client: the app's load, one-shot requests, and the event stream that re-connects after plyd restarts.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use ply_proto::PROTOCOL_VERSION;
use ply_proto::control::{
    Call, ClientMsg, Empty, Event, Hello, LayoutSaveParams, MAX_LINE_BYTES, Request, ServerMsg,
    ThemeSetParams, WorkspaceRef, encode_line,
};
use ply_proto::pane::{Layout, Pane, Settings, TerminalTheme, Workspace};

/// plyd moves idle agent panes onto their CLI's update when this client connects, so only the event stream says it.
const APP_CLIENT: &str = "ply-app";
const REQUEST_CLIENT: &str = "ply-app-request";
const BUILD_ID: &str = match option_env!("PLY_BUILD_ID") {
    Some(id) => id,
    None => env!("CARGO_PKG_VERSION"),
};

pub struct Loaded {
    pub workspace_id: u64,
    pub workspace_path: String,
    pub tab_ids: Vec<u64>,
    pub tabs: Vec<(String, Vec<u64>)>,
    pub zoomed: Vec<bool>,
    pub active_tab: usize,
    pub focus: Vec<usize>,
    pub panes: HashMap<u64, Pane>,
    pub settings: Settings,
}

struct Conn {
    reader: BufReader<std::io::Take<UnixStream>>,
    writer: UnixStream,
    next_id: u64,
}

impl Conn {
    fn open(sock: &Path, client: &str) -> Result<Self, String> {
        let stream =
            UnixStream::connect(sock).map_err(|e| format!("connect {}: {e}", sock.display()))?;
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        let mut conn = Self {
            reader: BufReader::new(stream.take(u64::MAX)),
            writer,
            next_id: 1,
        };
        conn.send(&ClientMsg::Hello(Hello {
            v: PROTOCOL_VERSION,
            client: client.into(),
            app_version: BUILD_ID.into(),
        }))?;
        match conn.read()? {
            ServerMsg::Welcome(_) => Ok(conn),
            other => Err(format!("plyd refused the handshake: {other:?}")),
        }
    }

    fn send(&mut self, msg: &ClientMsg) -> Result<(), String> {
        let line = encode_line(msg).map_err(|e| e.to_string())?;
        self.writer.write_all(&line).map_err(|e| e.to_string())
    }

    /// One line, at most `MAX_LINE_BYTES` as plyd's own cap.
    fn read(&mut self) -> Result<ServerMsg, String> {
        let mut line = Vec::new();
        self.reader.get_mut().set_limit(MAX_LINE_BYTES as u64);
        let n = self
            .reader
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("plyd closed the control connection".into());
        }
        if !line.ends_with(b"\n") {
            return Err("a line from plyd passed the 1 MiB cap".into());
        }
        serde_json::from_slice(&line).map_err(|e| format!("bad line from plyd: {e}"))
    }

    fn call<T: serde::de::DeserializeOwned>(&mut self, call: Call) -> Result<T, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&ClientMsg::Req(Request { id, call }))?;
        loop {
            if let ServerMsg::Res(r) = self.read()?
                && r.id == id
            {
                let value = r.outcome.map_err(|e| e.msg)?;
                return serde_json::from_value(value).map_err(|e| e.to_string());
            }
        }
    }
}

/// One request on its own connection; the answer's `msg` is the error, as the app shows it.
pub fn request<T: serde::de::DeserializeOwned>(sock: &Path, call: Call) -> Result<T, String> {
    Conn::open(sock, REQUEST_CLIENT)?.call(call)
}

/// The app's load: settings, the theme for its accent, the home workspace (else the latest opened), its layout and panes.
pub fn load(sock: &Path, theme: impl Fn(&Settings) -> TerminalTheme) -> Result<Loaded, String> {
    let mut conn = Conn::open(sock, REQUEST_CLIENT)?;
    let settings: Settings = conn.call(Call::SettingsGet(Empty {}))?;
    if let Err(e) = conn.call::<serde_json::Value>(Call::ThemeSet(ThemeSetParams {
        palette: theme(&settings),
    })) {
        tracing::warn!(error = %e, "theme.set failed; plyd keeps its palette");
    }
    let workspaces: Vec<Workspace> = conn.call(Call::WorkspaceList(Empty {}))?;
    let home = std::env::var("HOME").unwrap_or_default();
    let ws = workspaces
        .iter()
        .find(|w| w.path == home)
        .or_else(|| workspaces.iter().max_by_key(|w| w.opened_at))
        .ok_or("plyd has no workspace")?;
    let layout: Layout = conn.call(Call::LayoutGet(WorkspaceRef {
        workspace_id: ws.id,
    }))?;
    let panes: Vec<Pane> = conn.call(Call::PaneList(WorkspaceRef {
        workspace_id: ws.id,
    }))?;
    let mut tabs = layout.tabs.clone();
    tabs.sort_by_key(|t| t.position);
    let active_tab = layout
        .active_tab_id
        .and_then(|id| tabs.iter().position(|t| t.id == id))
        .unwrap_or(0);
    let focus = tabs
        .iter()
        .map(|t| {
            t.focus_pane_id
                .and_then(|f| t.pane_ids.iter().position(|&p| p == f))
                .unwrap_or(0)
        })
        .collect();
    Ok(Loaded {
        workspace_id: ws.id,
        workspace_path: ws.path.clone(),
        tab_ids: tabs.iter().map(|t| t.id).collect(),
        zoomed: tabs.iter().map(|t| t.zoomed).collect(),
        tabs: tabs.into_iter().map(|t| (t.name, t.pane_ids)).collect(),
        active_tab,
        focus,
        panes: panes.into_iter().map(|p| (p.id, p)).collect(),
        settings,
    })
}

/// `layout.save` of the tabs as shown: their order, focused panes, zoom and the active tab.
pub fn save_layout(sock: &Path, workspace_id: u64, layout: Layout) -> Result<(), String> {
    request::<serde_json::Value>(
        sock,
        Call::LayoutSave(LayoutSaveParams {
            workspace_id,
            layout,
        }),
    )
    .map(|_| ())
}

/// What the event stream reports: a fresh connection (the caller loads again) or one event.
pub enum Watch {
    Connected,
    Event(Event),
}

/// Streams events on a connection of its own, re-connecting 100 ms doubling to 2 s after plyd goes away; subscribes
/// before returning when plyd is up, so no event between the caller's load and the stream is lost.
pub fn watch(sock: &Path, events: UnboundedSender<Watch>) {
    let sock: PathBuf = sock.to_path_buf();
    let first = Conn::open(&sock, APP_CLIENT);
    let spawned = thread::Builder::new()
        .name("c1-events".into())
        .spawn(move || {
            let mut conn = first.ok();
            let mut backoff = Duration::from_millis(100);
            loop {
                if let Some(mut c) = conn.take() {
                    backoff = Duration::from_millis(100);
                    loop {
                        match c.read() {
                            Ok(ServerMsg::Evt(e)) => {
                                if events.unbounded_send(Watch::Event(e)).is_err() {
                                    return;
                                }
                            }
                            Ok(_) => {}
                            Err(e) => {
                                tracing::info!(error = %e, "C1 event stream ended; re-connecting");
                                break;
                            }
                        }
                    }
                }
                thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(2));
                match Conn::open(&sock, APP_CLIENT) {
                    Ok(c) => {
                        if events.unbounded_send(Watch::Connected).is_err() {
                            return;
                        }
                        conn = Some(c);
                    }
                    Err(e) => tracing::debug!(error = %e, "plyd not answering yet"),
                }
            }
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "cannot start the C1 event thread");
    }
}
