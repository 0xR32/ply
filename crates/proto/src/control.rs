//! C1 control protocol (spec 4.1): UTF-8 JSON, one object per line, at most [`MAX_LINE_BYTES`].
//!
//! The app sends [`ClientMsg`] (`hello` first, then `req`); plyd answers with [`ServerMsg`] (`welcome`, `res`,
//! `evt`). Every struct rejects unknown fields (INV-10). Requests carry a method name in `m` and its params in
//! `p`; [`Call`] lists every method of the 4.1 table plus ADR-0009's additions, and [`METHODS`] names the result
//! type of each. No type here carries pty bytes (INV-2).
//!
//! ```
//! use ply_proto::control::{Call, ClientMsg, PaneCloseParams, Request};
//!
//! let line = br#"{"t":"req","id":7,"m":"pane.close","p":{"pane_id":2,"kill":false}}"#;
//! let msg = ClientMsg::decode(line).expect("valid request");
//! assert_eq!(
//!     msg,
//!     ClientMsg::Req(Request { id: 7, call: Call::PaneClose(PaneCloseParams { pane_id: 2, kill: false }) })
//! );
//! ```

use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{Error, Result};
use crate::pane::{
    Cli, Layout, Pane, PaneId, PaneStatus, Progress, Settings, TerminalTheme, UnixSeconds,
};

/// Longest accepted C1 line in bytes, newline included; a reader must stop buffering at this size.
pub const MAX_LINE_BYTES: usize = 1 << 20;

/// Request id reserved for plyd's reply to a rejected `hello`; clients number their requests from 1.
pub const HANDSHAKE_ID: u64 = 0;

/// Messages from the app to plyd, tagged by `"t"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// First line on every connection.
    Hello(Hello),
    /// A method call.
    Req(Request),
}

/// Messages from plyd to the app, tagged by `"t"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Answer to `hello` when the versions match.
    Welcome(Welcome),
    /// Answer to one request (or to a rejected `hello`, with id [`HANDSHAKE_ID`]).
    Res(Response),
    /// A daemon event; events have no id.
    Evt(Event),
}

/// Handshake from the app; `v` must equal [`crate::PROTOCOL_VERSION`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    /// Protocol version the client speaks.
    pub v: u16,
    /// Client name, `"ply-app"` for the app.
    pub client: String,
    /// Client build version (semver).
    pub app_version: String,
}

impl Hello {
    /// Checks `v` against [`crate::PROTOCOL_VERSION`]; fails with [`Error::VersionMismatch`] when they differ.
    pub fn check_version(&self) -> Result<()> {
        crate::version::check_version("C1", crate::PROTOCOL_VERSION, self.v)
    }
}

/// Handshake answer from plyd.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Welcome {
    /// Protocol version plyd speaks.
    pub v: u16,
    /// plyd's build id, `<semver>+<commit>`: the 12-character commit hash it was built from (`t<unix seconds>` outside git).
    pub daemon_version: String,
}

/// A method call: `{"t":"req","id":…,"m":"<method>","p":{…}}`; `p` is required, `{}` for methods without params.
#[derive(Debug, Clone, PartialEq, Deserialize, TS)]
#[serde(try_from = "RequestWire")]
pub struct Request {
    /// Caller-chosen id, echoed by the response; unique per connection, starting at 1.
    pub id: u64,
    /// The method and its params.
    #[ts(flatten)]
    pub call: Call,
}

impl Serialize for Request {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Flat<'a> {
            id: u64,
            #[serde(flatten)]
            call: &'a Call,
        }
        Flat {
            id: self.id,
            call: &self.call,
        }
        .serialize(s)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestWire {
    id: u64,
    m: String,
    p: serde_json::Value,
}

/// Why a request line was refused, carrying the id when one could be read so plyd can answer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    /// The request's id, or `None` when the line had none (then plyd logs and skips it).
    pub id: Option<u64>,
    /// The error to send back.
    pub error: ErrorBody,
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.error.code, self.error.msg)
    }
}

impl TryFrom<RequestWire> for Request {
    type Error = Rejection;

    fn try_from(w: RequestWire) -> std::result::Result<Self, Rejection> {
        let reject = |code, msg: String| Rejection {
            id: Some(w.id),
            error: ErrorBody { code, msg },
        };
        if !METHODS.iter().any(|m| m.name == w.m) {
            return Err(reject(
                ErrorCode::UnknownMethod,
                format!("unknown method {:?}", w.m),
            ));
        }
        let tagged = serde_json::json!({ "m": w.m, "p": w.p });
        serde_json::from_value::<Call>(tagged)
            .map(|call| Self { id: w.id, call })
            .map_err(|e| {
                reject(
                    ErrorCode::BadRequest,
                    format!("bad params for {}: {e}", w.m),
                )
            })
    }
}

impl ClientMsg {
    /// Parses one line (trailing newline allowed); a refused request keeps its id so plyd can answer it.
    /// Fails with a [`Rejection`] of `bad_request` or `unknown_method`; lines over [`MAX_LINE_BYTES`] are `bad_request`.
    pub fn decode(line: &[u8]) -> std::result::Result<Self, Rejection> {
        let bad = |id, msg: String| Rejection {
            id,
            error: ErrorBody {
                code: ErrorCode::BadRequest,
                msg,
            },
        };
        if line.len() > MAX_LINE_BYTES {
            return Err(bad(
                None,
                format!("line of {} bytes exceeds the 1 MiB cap", line.len()),
            ));
        }
        serde_json::from_slice::<Self>(line).map_err(|e| {
            let req = serde_json::from_slice::<serde_json::Value>(line)
                .ok()
                .filter(|v| v.get("t").and_then(serde_json::Value::as_str) == Some("req"));
            let id = req
                .as_ref()
                .and_then(|v| v.get("id"))
                .and_then(serde_json::Value::as_u64);
            let precise = req
                .and_then(|mut v| {
                    v.as_object_mut().map(|o| o.remove("t"));
                    serde_json::from_value::<RequestWire>(v).ok()
                })
                .and_then(|w| Request::try_from(w).err());
            precise.unwrap_or_else(|| bad(id, format!("invalid C1 message: {e}")))
        })
    }
}

/// Serialises a C1 message as one line ending in `\n`; fails with [`Error::LineTooLong`] above [`MAX_LINE_BYTES`].
pub fn encode_line<T: Serialize>(msg: &T) -> Result<Vec<u8>> {
    let mut out = serde_json::to_vec(msg)?;
    out.push(b'\n');
    if out.len() > MAX_LINE_BYTES {
        return Err(Error::LineTooLong {
            len: out.len(),
            max: MAX_LINE_BYTES,
        });
    }
    Ok(out)
}

/// Parses one C1 line of any message type; fails with [`Error::LineTooLong`] or [`Error::Json`].
pub fn decode_line<T: DeserializeOwned>(line: &[u8]) -> Result<T> {
    if line.len() > MAX_LINE_BYTES {
        return Err(Error::LineTooLong {
            len: line.len(),
            max: MAX_LINE_BYTES,
        });
    }
    Ok(serde_json::from_slice(line)?)
}

/// Params of methods that take none: the empty object `{}`; also the result of methods that return nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// `workspace.open` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceOpenParams {
    /// Absolute directory path; opening a known path returns the existing workspace.
    pub path: String,
}

/// Params naming one workspace (`pane.list`, `layout.get`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRef {
    /// Workspace id.
    pub workspace_id: u64,
}

/// Params naming one pane (`pane.resume`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PaneRef {
    /// Pane id.
    pub pane_id: PaneId,
}

/// The worktree option of `pane.create`, passed through to `claude --worktree <name>` (INV-7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct WorktreeOption {
    /// Worktree name as Claude Code expects it.
    pub name: String,
}

/// `pane.create` params; without `tab_id` the pane opens a new tab, and `worktree` is valid for Claude only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct PaneCreateParams {
    /// Workspace to create the pane in.
    pub workspace_id: u64,
    /// Existing tab to add the pane to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<u64>,
    /// Program to run.
    pub cli: Cli,
    /// Absolute working directory.
    pub cwd: String,
    /// Claude Code worktree to start in; `bad_request` for other CLIs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeOption>,
    /// First prompt, passed as the CLI's prompt argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

/// `pane.close` params: `kill: false` on a live pane fails with `pane_alive`; `kill: true` sends SIGHUP, then SIGKILL after 2 s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PaneCloseParams {
    /// Pane to close.
    pub pane_id: PaneId,
    /// Stop a live process first.
    pub kill: bool,
}

/// A dialog choice for `pane.answer`: 1, 2 or 3, written to the pty as that digit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(try_from = "u8", into = "u8")]
#[ts(type = "1 | 2 | 3")]
pub struct Choice(u8);

impl Choice {
    /// The digit, always 1, 2 or 3.
    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Choice {
    type Error = String;

    fn try_from(v: u8) -> std::result::Result<Self, String> {
        if (1..=3).contains(&v) {
            Ok(Self(v))
        } else {
            Err(format!("choice must be 1, 2 or 3, got {v}"))
        }
    }
}

impl From<Choice> for u8 {
    fn from(c: Choice) -> Self {
        c.0
    }
}

/// `pane.answer` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PaneAnswerParams {
    /// Pane showing the CLI's dialog.
    pub pane_id: PaneId,
    /// Option to pick.
    pub choice: Choice,
}

/// `session.list` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SessionListParams {
    /// Workspace whose sessions to list.
    pub workspace_id: u64,
    /// Include panes that were closed.
    pub include_closed: bool,
}

/// `theme.set` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ThemeSetParams {
    /// Palette every pane's terminal answers colour queries from; stored across restarts.
    pub palette: TerminalTheme,
}

/// `layout.save` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LayoutSaveParams {
    /// Workspace the layout belongs to.
    pub workspace_id: u64,
    /// The layout to store, replacing the previous one.
    pub layout: Layout,
}

/// `settings.set` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SettingsSetParams {
    /// The complete settings, replacing the stored ones.
    pub settings: Settings,
}

/// `daemon.shutdown` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct DaemonShutdownParams {
    /// Stop every pane's process before exiting ("Quit ply and stop sessions").
    pub kill_panes: bool,
}

/// Every C1 method with its params, tagged `"m"` with params in `"p"`; [`METHODS`] gives each result type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "m", content = "p", deny_unknown_fields)]
pub enum Call {
    /// `workspace.list` → `Workspace[]`.
    #[serde(rename = "workspace.list")]
    WorkspaceList(Empty),
    /// `workspace.open` → `Workspace`.
    #[serde(rename = "workspace.open")]
    WorkspaceOpen(WorkspaceOpenParams),
    /// `pane.list` → `Pane[]` (open panes of the workspace).
    #[serde(rename = "pane.list")]
    PaneList(WorkspaceRef),
    /// `pane.create` → `Pane`; `cli_not_found` / `cli_too_old` when the CLI cannot be launched.
    #[serde(rename = "pane.create")]
    PaneCreate(PaneCreateParams),
    /// `pane.close` → `{}`.
    #[serde(rename = "pane.close")]
    PaneClose(PaneCloseParams),
    /// `pane.answer` → `{}`.
    #[serde(rename = "pane.answer")]
    PaneAnswer(PaneAnswerParams),
    /// `pane.resume` → `Pane`: relaunches a `lost` pane from its launch spec and session id (ADR-0009).
    #[serde(rename = "pane.resume")]
    PaneResume(PaneRef),
    /// `session.list` → `Session[]`.
    #[serde(rename = "session.list")]
    SessionList(SessionListParams),
    /// `theme.set` → `{}`.
    #[serde(rename = "theme.set")]
    ThemeSet(ThemeSetParams),
    /// `layout.get` → `Layout`.
    #[serde(rename = "layout.get")]
    LayoutGet(WorkspaceRef),
    /// `layout.save` → `{}`.
    #[serde(rename = "layout.save")]
    LayoutSave(LayoutSaveParams),
    /// `settings.get` → `Settings` (ADR-0009).
    #[serde(rename = "settings.get")]
    SettingsGet(Empty),
    /// `settings.set` → `{}` (ADR-0009).
    #[serde(rename = "settings.set")]
    SettingsSet(SettingsSetParams),
    /// `daemon.shutdown` → `{}`, sent before plyd emits `daemon.stopping` and exits.
    #[serde(rename = "daemon.shutdown")]
    DaemonShutdown(DaemonShutdownParams),
}

impl Call {
    /// The wire method name (`"pane.create"`, …), always an entry of [`METHODS`].
    pub fn method(&self) -> &'static str {
        match self {
            Self::WorkspaceList(_) => "workspace.list",
            Self::WorkspaceOpen(_) => "workspace.open",
            Self::PaneList(_) => "pane.list",
            Self::PaneCreate(_) => "pane.create",
            Self::PaneClose(_) => "pane.close",
            Self::PaneAnswer(_) => "pane.answer",
            Self::PaneResume(_) => "pane.resume",
            Self::SessionList(_) => "session.list",
            Self::ThemeSet(_) => "theme.set",
            Self::LayoutGet(_) => "layout.get",
            Self::LayoutSave(_) => "layout.save",
            Self::SettingsGet(_) => "settings.get",
            Self::SettingsSet(_) => "settings.set",
            Self::DaemonShutdown(_) => "daemon.shutdown",
        }
    }
}

/// One row of the method table: the wire name and the TypeScript names of its params and result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodInfo {
    /// Wire name, the value of `"m"`.
    pub name: &'static str,
    /// Params type name in `proto.gen.ts`.
    pub params: &'static str,
    /// Result type in `proto.gen.ts` (the value of a successful response's `"r"`).
    pub result: &'static str,
}

/// The C1 method table in [`Call`] order; `proto.gen.ts` exports it as the `Methods` type map.
pub const METHODS: &[MethodInfo] = &[
    MethodInfo {
        name: "workspace.list",
        params: "Empty",
        result: "Array<Workspace>",
    },
    MethodInfo {
        name: "workspace.open",
        params: "WorkspaceOpenParams",
        result: "Workspace",
    },
    MethodInfo {
        name: "pane.list",
        params: "WorkspaceRef",
        result: "Array<Pane>",
    },
    MethodInfo {
        name: "pane.create",
        params: "PaneCreateParams",
        result: "Pane",
    },
    MethodInfo {
        name: "pane.close",
        params: "PaneCloseParams",
        result: "Empty",
    },
    MethodInfo {
        name: "pane.answer",
        params: "PaneAnswerParams",
        result: "Empty",
    },
    MethodInfo {
        name: "pane.resume",
        params: "PaneRef",
        result: "Pane",
    },
    MethodInfo {
        name: "session.list",
        params: "SessionListParams",
        result: "Array<Session>",
    },
    MethodInfo {
        name: "theme.set",
        params: "ThemeSetParams",
        result: "Empty",
    },
    MethodInfo {
        name: "layout.get",
        params: "WorkspaceRef",
        result: "Layout",
    },
    MethodInfo {
        name: "layout.save",
        params: "LayoutSaveParams",
        result: "Empty",
    },
    MethodInfo {
        name: "settings.get",
        params: "Empty",
        result: "Settings",
    },
    MethodInfo {
        name: "settings.set",
        params: "SettingsSetParams",
        result: "Empty",
    },
    MethodInfo {
        name: "daemon.shutdown",
        params: "DaemonShutdownParams",
        result: "Empty",
    },
];

/// Closed set of C1 error codes (snake_case on the wire); clients match on these, never on `msg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The CLI is not on the login-shell `PATH`.
    CliNotFound,
    /// The CLI is older than the supported minimum (C7).
    CliTooOld,
    /// Malformed line, unknown field or invalid params.
    BadRequest,
    /// `m` names no method of the table.
    UnknownMethod,
    /// `hello.v` differs from plyd's protocol version; answered on request id 0, then the connection closes.
    VersionMismatch,
    /// The workspace, tab or pane does not exist.
    NotFound,
    /// `pane.close {kill:false}` on a pane whose process is alive.
    PaneAlive,
    /// The pane is in a state that does not allow the request (e.g. `pane.resume` on a live pane).
    InvalidState,
    /// The process could not be started (pty, exec or launch spec failure).
    SpawnFailed,
    /// plyd is shutting down and accepts no new work.
    ShuttingDown,
    /// Anything else; details are in plyd's log.
    Internal,
}

/// The error of a failed request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    /// Machine-readable code.
    pub code: ErrorCode,
    /// Human-readable message, shown to the user as is.
    pub msg: String,
}

/// The answer to a request: `{"t":"res","id","ok":true,"r":…}` or `{"t":"res","id","ok":false,"err":{…}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(try_from = "ResponseWire", into = "ResponseWire")]
#[ts(as = "ResponseTs")]
pub struct Response {
    /// Id of the request answered.
    pub id: u64,
    /// The result value (typed per [`METHODS`]) or the error.
    pub outcome: std::result::Result<serde_json::Value, ErrorBody>,
}

impl Response {
    /// A success carrying `result` serialised to JSON; fails with [`Error::Json`] if `result` cannot be serialised.
    pub fn ok<T: Serialize>(id: u64, result: &T) -> Result<Self> {
        Ok(Self {
            id,
            outcome: Ok(serde_json::to_value(result)?),
        })
    }

    /// A failure with `code` and a human-readable `msg`.
    pub fn err(id: u64, code: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            id,
            outcome: Err(ErrorBody {
                code,
                msg: msg.into(),
            }),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseWire {
    id: u64,
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    r: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    err: Option<ErrorBody>,
}

impl TryFrom<ResponseWire> for Response {
    type Error = String;

    fn try_from(w: ResponseWire) -> std::result::Result<Self, String> {
        match (w.ok, w.r, w.err) {
            (true, Some(r), None) => Ok(Self {
                id: w.id,
                outcome: Ok(r),
            }),
            (false, None, Some(err)) => Ok(Self {
                id: w.id,
                outcome: Err(err),
            }),
            _ => Err("a response has `ok:true` with `r`, or `ok:false` with `err`".to_owned()),
        }
    }
}

impl From<Response> for ResponseWire {
    fn from(r: Response) -> Self {
        match r.outcome {
            Ok(v) => Self {
                id: r.id,
                ok: true,
                r: Some(v),
                err: None,
            },
            Err(e) => Self {
                id: r.id,
                ok: false,
                r: None,
                err: Some(e),
            },
        }
    }
}

#[derive(TS)]
#[ts(rename = "Response", untagged)]
#[allow(dead_code)]
enum ResponseTs {
    Ok {
        id: u64,
        #[ts(type = "true")]
        ok: bool,
        r: serde_json::Value,
    },
    Err {
        id: u64,
        #[ts(type = "false")]
        ok: bool,
        err: ErrorBody,
    },
}

/// `pane.removed` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PaneRemoved {
    /// The pane that was closed.
    pub pane_id: PaneId,
}

/// `pane.status` payload; `exit_code` is present exactly when `status` is `exited`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct PaneStatusChanged {
    /// The pane.
    pub pane_id: PaneId,
    /// Its new state.
    pub status: PaneStatus,
    /// One line on the state, e.g. the tool awaiting permission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Exit code for `exited`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// When plyd observed the signal behind the change.
    pub at: UnixSeconds,
}

/// `pane.progress` payload, sent at most 4 times a second per pane; no `progress` hides the bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct PaneProgress {
    /// The pane.
    pub pane_id: PaneId,
    /// The agent's plan progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
}

/// `pane.meta` payload (Ruling R4): what the session reports about itself; absent fields are unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
pub struct PaneMeta {
    /// The pane.
    pub pane_id: PaneId,
    /// Model as the session reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Worktree name the CLI reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// Current working directory, absolute.
    pub cwd: String,
    /// Git branch of `cwd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// `pane.exit` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PaneExit {
    /// The pane.
    pub pane_id: PaneId,
    /// Exit code; a signal death is 128 + signal.
    pub code: i32,
    /// When the process ended.
    pub at: UnixSeconds,
}

/// `daemon.stopping` payload, sent to every client before plyd exits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct DaemonStopping {
    /// Whether the panes' processes are being stopped too.
    pub kill_panes: bool,
}

/// Daemon events, tagged `"e"` with the payload in `"p"`; broadcast to every connected client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "e", content = "p", deny_unknown_fields)]
pub enum Event {
    /// A pane was created (by any client).
    #[serde(rename = "pane.added")]
    PaneAdded(Box<Pane>),
    /// A pane was closed.
    #[serde(rename = "pane.removed")]
    PaneRemoved(PaneRemoved),
    /// A pane changed state.
    #[serde(rename = "pane.status")]
    PaneStatus(PaneStatusChanged),
    /// A pane's plan progress changed.
    #[serde(rename = "pane.progress")]
    PaneProgress(PaneProgress),
    /// A pane's reported model, worktree, directory or branch changed.
    #[serde(rename = "pane.meta")]
    PaneMeta(PaneMeta),
    /// A pane's process ended.
    #[serde(rename = "pane.exit")]
    PaneExit(PaneExit),
    /// plyd is about to exit.
    #[serde(rename = "daemon.stopping")]
    DaemonStopping(DaemonStopping),
}
