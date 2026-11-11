//! The C1 method table: `METHODS` lists every `Call` variant, in order, under the name it puts on the wire.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ply_proto::control::{
    AccountSwitchParams, Answer, Call, DaemonShutdownParams, Empty, ErrorCode, LayoutSaveParams,
    METHODS, PaneAnswerParams, PaneCloseParams, PaneCreateParams, PaneRef, Request, Response,
    SessionListParams, SettingsSetParams, ThemeSetParams, WorkspaceOpenParams, WorkspaceRef,
};
use ply_proto::pane::{Cli, Layout, Rgb, Settings, TerminalTheme};

fn black_theme() -> TerminalTheme {
    let c = Rgb { r: 0, g: 0, b: 0 };
    TerminalTheme {
        ansi: [c; 16],
        fg: c,
        bg: c,
        cursor: c,
        cursor_text: c,
        selection_bg: c,
        selection_fg: c,
    }
}

#[test]
fn method_table_matches_call() {
    let names: Vec<&str> = METHODS.iter().map(|m| m.name).collect();
    let calls = [
        Call::WorkspaceList(Empty {}),
        Call::WorkspaceOpen(WorkspaceOpenParams {
            path: String::new(),
        }),
        Call::PaneList(WorkspaceRef { workspace_id: 1 }),
        Call::PaneCreate(PaneCreateParams {
            workspace_id: 1,
            tab_id: None,
            cli: Cli::Shell,
            cwd: String::new(),
            worktree: None,
            prompt: None,
        }),
        Call::PaneClose(PaneCloseParams {
            pane_id: 1,
            kill: false,
        }),
        Call::PaneAnswer(PaneAnswerParams {
            pane_id: 1,
            answer: Answer::No,
        }),
        Call::PaneResume(PaneRef { pane_id: 1 }),
        Call::SessionList(SessionListParams {
            workspace_id: 1,
            include_closed: true,
        }),
        Call::ThemeSet(ThemeSetParams {
            palette: black_theme(),
        }),
        Call::LayoutGet(WorkspaceRef { workspace_id: 1 }),
        Call::LayoutSave(LayoutSaveParams {
            workspace_id: 1,
            layout: Layout {
                tabs: vec![],
                active_tab_id: None,
            },
        }),
        Call::SettingsGet(Empty {}),
        Call::SettingsSet(SettingsSetParams {
            settings: Settings::default(),
        }),
        Call::DaemonShutdown(DaemonShutdownParams { kill_panes: false }),
        Call::UsageGet(Empty {}),
        Call::AccountList(Empty {}),
        Call::AccountSwitch(AccountSwitchParams {
            workspace_id: 1,
            account: None,
        }),
        Call::CodexAccount(Empty {}),
        Call::CodexSignIn(WorkspaceRef { workspace_id: 1 }),
    ];
    let from_calls: Vec<&str> = calls.iter().map(Call::method).collect();
    assert_eq!(names, from_calls);
    for call in &calls {
        let v = serde_json::to_value(call).unwrap();
        assert_eq!(v["m"], call.method());
    }
    let _ = Request {
        id: 1,
        call: Call::SettingsGet(Empty {}),
    };
    let _ = Response::err(1, ErrorCode::Internal, "x");
}
