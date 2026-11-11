//! The ⌘N / ⌘T new-pane form and the ⌘⇧A account palette, as `features/new-pane` and `features/palette` do them.

use std::path::Path;

use gpui::{
    Context, Div, MouseButton, MouseDownEvent, PathPromptOptions, deferred, div, prelude::*, px,
};
use ply_proto::control::{
    AccountSwitchParams, Call, PaneCreateParams, SessionListParams, WorkspaceRef, WorktreeOption,
};
use ply_proto::pane::{
    ClaudeAccount, ClaudeAccounts, Cli, CodexAccount, CodexAccounts, Pane, Session,
};

use crate::chrome::{
    self, ACCENT, AMBER, HINT, MINT, RED, Scale, TERM, TEXT, TEXT2, TEXT3, Type, c, ca, text, white,
};
use crate::dirs::{self, Kind, Suggestion};
use crate::field::Field;
use crate::{Workspace, control};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Pane,
    Tab,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Agent,
    Directory,
    Browse,
    Switch,
    Name,
    Prompt,
    Cancel,
    Open,
}

pub struct NewPane {
    pub target: Target,
    pub cli: Cli,
    pub dir: Field,
    pub start: String,
    pub worktree: bool,
    pub name: Field,
    pub prompt: Field,
    pub focus: Slot,
    pub list_open: bool,
    pub highlight: usize,
    pub suggestions: Vec<Suggestion>,
    pub error: Option<String>,
    pub pending: bool,
}

impl NewPane {
    fn slots(&self) -> Vec<Slot> {
        let mut v = vec![Slot::Agent, Slot::Directory, Slot::Browse];
        if self.cli == Cli::Claude {
            v.push(Slot::Switch);
            if self.worktree {
                v.push(Slot::Name);
            }
        }
        if self.cli != Cli::Shell {
            v.push(Slot::Prompt);
        }
        v.extend([Slot::Cancel, Slot::Open]);
        v
    }

    fn step(&mut self, by: isize) {
        let slots = self.slots();
        let i = slots.iter().position(|s| *s == self.focus).unwrap_or(0) as isize;
        self.focus = slots[(i + by).rem_euclid(slots.len() as isize) as usize];
        self.list_open = false;
    }

    fn take_row(&mut self, i: usize) {
        if let Some(row) = self.suggestions.get(i) {
            self.dir.set(row.path.clone());
            self.list_open = false;
        }
    }
}

pub struct Palette {
    pub query: Field,
    pub selected: usize,
}

pub enum Overlay {
    NewPane(NewPane),
    Palette(Palette),
}

enum Run {
    Switch(ClaudeAccount),
    SignIn,
    CodexSignIn,
    GoTo(u64),
}

/// Codex's plan as its sign-in names it (`chatgpt_plan_type`), in words.
pub fn codex_plan(plan: &str) -> String {
    match plan {
        "free" => "Free".into(),
        "plus" => "Plus".into(),
        "pro" => "Pro".into(),
        "team" => "Team".into(),
        "business" => "Business".into(),
        "enterprise" => "Enterprise".into(),
        "edu" => "Edu".into(),
        other => {
            let words = other.replace('_', " ");
            let mut chars = words.chars();
            chars
                .next()
                .map_or(String::new(), |c| c.to_uppercase().chain(chars).collect())
        }
    }
}

/// "email · Plan", or "API key" for a Codex that authenticates with a key.
pub fn codex_who(a: Option<&CodexAccount>) -> String {
    match a {
        None => "another account".into(),
        Some(a) if a.api_key => "API key".into(),
        Some(a) => match (&a.email, &a.plan) {
            (Some(e), Some(p)) => format!("{e} · {}", codex_plan(p)),
            (Some(e), None) => e.clone(),
            (None, Some(p)) => codex_plan(p),
            (None, None) => "ChatGPT".into(),
        },
    }
}

fn cli_name(cli: Cli) -> &'static str {
    match cli {
        Cli::Claude => "Claude Code",
        Cli::Codex => "Codex",
        Cli::Shell => "Shell",
    }
}

/// "Max · personal" or "Team · `org`"; "claude.ai" stands in for an unknown plan.
pub fn kind(a: &ClaudeAccount) -> String {
    let plan = a.plan.as_deref().unwrap_or("");
    let name = match plan {
        "free" => "Free",
        "pro" => "Pro",
        "max" => "Max",
        "team" => "Team",
        "enterprise" => "Enterprise",
        "" => "claude.ai",
        other => other,
    };
    match (plan, &a.org_name) {
        ("team" | "enterprise", Some(org)) => format!("{name} · {org}"),
        _ => format!("{name} · personal"),
    }
}

pub fn who(a: Option<&ClaudeAccount>) -> String {
    a.map_or_else(
        || "another account".into(),
        |a| format!("{} · {}", a.email, kind(a)),
    )
}

fn printable(ks: &gpui::Keystroke) -> Option<String> {
    if ks.modifiers.control || ks.modifiers.platform {
        return None;
    }
    ks.key_char
        .clone()
        .filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
}

impl Workspace {
    pub fn open_new_pane(&mut self, target: Target, cx: &mut Context<Self>) {
        let target = if self.tabs.is_empty() {
            Target::Tab
        } else {
            target
        };
        if target == Target::Pane && self.tabs.get(self.active).is_some_and(|t| t.len() >= 4) {
            self.notice("This tab has 4 panes — ⌘T opens a new tab".into(), cx);
            return;
        }
        let focused_cwd = self
            .panes
            .get(self.focused)
            .and_then(|p| self.infos.get(&p.read(cx).conn.pane_id))
            .map(|p| p.cwd.clone())
            .filter(|c| c.starts_with('/'));
        let start = focused_cwd
            .or_else(|| (!self.workspace_path.is_empty()).then(|| self.workspace_path.clone()))
            .unwrap_or_else(|| dirs::home().to_string_lossy().into_owned());
        self.overlay = Some(Overlay::NewPane(NewPane {
            target,
            cli: Cli::Claude,
            dir: Field::new(dirs::tilde(&start)),
            start,
            worktree: false,
            name: Field::default(),
            prompt: Field::default(),
            focus: Slot::Agent,
            list_open: false,
            highlight: 0,
            suggestions: Vec::new(),
            error: None,
            pending: false,
        }));
        self.load_recents(cx);
        self.scan_repos(cx);
        self.refresh_suggestions();
        cx.notify();
    }

    fn refresh_suggestions(&mut self) {
        let repos = self.repos.clone().unwrap_or_default();
        if let Some(Overlay::NewPane(f)) = &mut self.overlay {
            f.suggestions = dirs::suggest(&f.dir.text, &f.start, &self.recents, &repos);
            f.highlight = f.highlight.min(f.suggestions.len().saturating_sub(1));
        }
    }

    /// Open panes first, then by last activity: the session store plus the live panes, folders that still exist, at most 50.
    fn load_recents(&mut self, cx: &mut Context<Self>) {
        let (Some(sock), Some(ws)) = (self.control.clone(), self.workspace_id) else {
            return;
        };
        let live: Vec<(String, bool, u64)> = self
            .infos
            .values()
            .map(|p| {
                (
                    p.cwd.clone(),
                    true,
                    p.last_activity_at.unwrap_or(p.created_at),
                )
            })
            .collect();
        cx.spawn(async move |this, cx| {
            let sessions: Vec<Session> = cx
                .background_executor()
                .spawn(async move {
                    control::request(
                        &sock,
                        Call::SessionList(SessionListParams {
                            workspace_id: ws,
                            include_closed: true,
                        }),
                    )
                })
                .await
                .unwrap_or_default();
            let mut all = live;
            all.extend(sessions.into_iter().map(|s| {
                let at = s.last_activity_at.or(s.closed_at).unwrap_or(s.created_at);
                (s.cwd, s.closed_at.is_none(), at)
            }));
            all.retain(|(p, _, _)| p.starts_with('/'));
            all.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
            let mut seen = std::collections::HashSet::new();
            let recents: Vec<(String, u64)> = all
                .into_iter()
                .filter(|(p, _, _)| seen.insert(p.clone()) && Path::new(p).is_dir())
                .take(50)
                .map(|(p, _, t)| (p, t))
                .collect();
            let _ = this.update(cx, |w, cx| {
                w.recents = recents;
                w.refresh_suggestions();
                cx.notify();
            });
        })
        .detach();
    }

    fn scan_repos(&mut self, cx: &mut Context<Self>) {
        if self.repos.is_some() || self.scanning {
            return;
        }
        self.scanning = true;
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { dirs::scan_repos() })
                .await;
            let _ = this.update(cx, |w, cx| {
                w.repos = Some(found);
                w.scanning = false;
                w.refresh_suggestions();
                cx.notify();
            });
        })
        .detach();
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.first()
            {
                let chosen = dirs::tilde(&p.to_string_lossy());
                let _ = this.update(cx, |w, cx| {
                    if let Some(Overlay::NewPane(f)) = &mut w.overlay {
                        f.dir.set(chosen);
                        f.list_open = false;
                    }
                    w.refresh_suggestions();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn submit_new_pane(&mut self, cx: &mut Context<Self>) {
        let tab_id = self.tab_ids.get(self.active).copied();
        let active_full = self.tabs.get(self.active).is_some_and(|t| t.len() >= 4);
        let (ws, sock) = (self.workspace_id, self.control.clone());
        let Some(Overlay::NewPane(f)) = &mut self.overlay else {
            return;
        };
        if f.pending {
            return;
        }
        let cwd = dirs::expand(f.dir.text.trim());
        let error = if !Path::new(&cwd).is_absolute() {
            Some("The directory must be an absolute path or start with ~")
        } else if f.cli == Cli::Claude && f.worktree && f.name.text.trim().is_empty() {
            Some("Name the worktree, or turn it off")
        } else if f.target == Target::Pane && active_full {
            Some("This tab has 4 panes — ⌘T opens a new tab")
        } else if ws.is_none() || sock.is_none() {
            Some("plyd has not loaded a workspace yet")
        } else {
            None
        };
        if let Some(e) = error {
            f.error = Some(e.into());
            cx.notify();
            return;
        }
        let (Some(ws), Some(sock)) = (ws, sock) else {
            return;
        };
        let prompt = f.prompt.text.trim().to_string();
        let params = PaneCreateParams {
            workspace_id: ws,
            tab_id: if f.target == Target::Pane {
                tab_id
            } else {
                None
            },
            cli: f.cli,
            cwd,
            worktree: (f.cli == Cli::Claude && f.worktree).then(|| WorktreeOption {
                name: f.name.text.trim().to_string(),
            }),
            prompt: (f.cli != Cli::Shell && !prompt.is_empty()).then_some(prompt),
        };
        f.pending = true;
        f.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r: Result<Pane, String> = cx
                .background_executor()
                .spawn(async move { control::request(&sock, Call::PaneCreate(params)) })
                .await;
            let _ = this.update(cx, |w, cx| {
                match r {
                    Ok(pane) => {
                        w.overlay = None;
                        w.focus_pane(pane.id, cx);
                    }
                    Err(msg) => {
                        if let Some(Overlay::NewPane(f)) = &mut w.overlay {
                            f.pending = false;
                            f.error = Some(msg);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// ⌘⇧A: the palette on the Claude and Codex account rows.
    pub fn open_accounts(&mut self, cx: &mut Context<Self>) {
        if self.accounts.is_none() && self.codex.is_none() {
            self.notice(
                "The account switch needs claude auth status or Codex's sign-in, which plyd could not read".into(),
                cx,
            );
            return;
        }
        self.overlay = Some(Overlay::Palette(Palette {
            query: Field::new("account"),
            selected: 0,
        }));
        cx.notify();
    }

    /// `account.list` and `codex.account`; a failure hides that CLI's chip and rows, as the app does.
    pub fn load_accounts(&mut self, cx: &mut Context<Self>) {
        let Some(sock) = self.control.clone() else {
            return;
        };
        let codex_sock = sock.clone();
        cx.spawn(async move |this, cx| {
            let claude: Result<ClaudeAccounts, String> = cx
                .background_executor()
                .spawn(
                    async move { control::request(&sock, Call::AccountList(Default::default())) },
                )
                .await;
            let codex: Result<CodexAccounts, String> = cx
                .background_executor()
                .spawn(async move {
                    control::request(&codex_sock, Call::CodexAccount(Default::default()))
                })
                .await;
            let _ = this.update(cx, |w, cx| {
                w.accounts = claude.ok();
                w.codex = codex.ok();
                cx.notify();
            });
        })
        .detach();
    }

    fn sign_in_row(&self, cli: &str, id: u64) -> (String, String, u32, Run) {
        let tab = self
            .tabs
            .iter()
            .position(|t| t.contains(&id))
            .map_or(0, |t| t + 1);
        (
            format!("Go to the {cli} sign-in"),
            format!("tab {tab} · the {cli} account switch waits for the browser"),
            AMBER,
            Run::GoTo(id),
        )
    }

    fn palette_rows(&self) -> Vec<(String, String, u32, Run)> {
        let mut rows: Vec<(String, String, u32, Run)> = Vec::new();
        if let Some(a) = &self.accounts {
            if let Some(id) = a.signing_in {
                rows.push(self.sign_in_row("Claude", id));
            } else {
                rows.extend(
                    a.known
                        .iter()
                        .filter(|k| {
                            a.current
                                .as_ref()
                                .is_none_or(|c| c.email != k.email || c.org_id != k.org_id)
                        })
                        .map(|k| {
                            (
                                format!("Switch Claude account to {}", k.email),
                                format!("{} · signs in, then Claude panes restart on it", kind(k)),
                                ACCENT,
                                Run::Switch(k.clone()),
                            )
                        }),
                );
                let now = match &a.current {
                    Some(cur) => format!("now {}", who(Some(cur))),
                    None => "signed out now".into(),
                };
                rows.push((
                    "Sign in to another Claude account".into(),
                    format!("{now} · claude auth login in a new tab"),
                    chrome::DOT_IDLE,
                    Run::SignIn,
                ));
            }
        }
        if let Some(c) = &self.codex {
            if let Some(id) = c.signing_in {
                rows.push(self.sign_in_row("Codex", id));
            } else {
                let now = match &c.current {
                    Some(cur) => format!("now {}", codex_who(Some(cur))),
                    None => "signed out now".into(),
                };
                rows.push((
                    "Sign in to another Codex account".into(),
                    format!("{now} · codex login in a new tab"),
                    chrome::DOT_IDLE,
                    Run::CodexSignIn,
                ));
            }
        }
        let query = self.palette_query().to_lowercase();
        rows.retain(|(label, hint, _, _)| {
            let hay = format!("{label} {hint}").to_lowercase();
            query.split_whitespace().all(|w| hay.contains(w))
        });
        rows
    }

    fn palette_query(&self) -> String {
        match &self.overlay {
            Some(Overlay::Palette(p)) => p.query.text.clone(),
            _ => String::new(),
        }
    }

    fn run_palette(&mut self, cx: &mut Context<Self>) {
        let selected = match &self.overlay {
            Some(Overlay::Palette(p)) => p.selected,
            _ => return,
        };
        let Some((_, _, _, run)) = self.palette_rows().into_iter().nth(selected) else {
            return;
        };
        self.overlay = None;
        cx.notify();
        let (Some(ws), Some(sock)) = (self.workspace_id, self.control.clone()) else {
            return;
        };
        let (call, cli) = match run {
            Run::GoTo(id) => return self.focus_pane(id, cx),
            Run::Switch(a) => (
                Call::AccountSwitch(AccountSwitchParams {
                    workspace_id: ws,
                    account: Some(a),
                }),
                "Claude",
            ),
            Run::SignIn => (
                Call::AccountSwitch(AccountSwitchParams {
                    workspace_id: ws,
                    account: None,
                }),
                "Claude",
            ),
            Run::CodexSignIn => (
                Call::CodexSignIn(WorkspaceRef { workspace_id: ws }),
                "Codex",
            ),
        };
        cx.spawn(async move |this, cx| {
            let r: Result<Pane, String> = cx
                .background_executor()
                .spawn(async move { control::request(&sock, call) })
                .await;
            let _ = this.update(cx, |w, cx| match r {
                Ok(pane) => {
                    w.focus_pane(pane.id, cx);
                    w.load_accounts(cx);
                }
                Err(msg) => w.notice(format!("Switching the {cli} account failed: {msg}"), cx),
            });
        })
        .detach();
    }

    /// Keys while an overlay is open; the global chords are off, as in the app.
    pub fn overlay_key(&mut self, ev: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let key = ks.key.as_str();
        match &mut self.overlay {
            Some(Overlay::NewPane(f)) => {
                if m.platform && key == "enter" {
                    return self.submit_new_pane(cx);
                }
                if m.platform && key == "v" {
                    if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        match f.focus {
                            Slot::Directory => f.dir.insert(t.trim()),
                            Slot::Name => f.name.insert(t.trim()),
                            Slot::Prompt => f.prompt.insert(&t),
                            _ => {}
                        }
                    }
                } else if key == "escape" {
                    if f.list_open {
                        f.list_open = false;
                    } else {
                        self.overlay = None;
                    }
                } else if key == "tab" {
                    f.step(if m.shift { -1 } else { 1 });
                } else {
                    match f.focus {
                        Slot::Agent => {
                            let order = [Cli::Claude, Cli::Codex, Cli::Shell];
                            let i = order.iter().position(|c| *c == f.cli).unwrap_or(0);
                            match (key, printable(ks).as_deref()) {
                                ("left", _) => f.cli = order[(i + 2) % 3],
                                ("right", _) => f.cli = order[(i + 1) % 3],
                                (_, Some("1")) => f.cli = Cli::Claude,
                                (_, Some("2")) => f.cli = Cli::Codex,
                                (_, Some("3")) => f.cli = Cli::Shell,
                                _ => {}
                            }
                        }
                        Slot::Directory => {
                            let n = f.suggestions.len();
                            if f.list_open && n > 0 && (key == "up" || key == "down") {
                                f.highlight = if key == "up" {
                                    (f.highlight + n - 1) % n
                                } else {
                                    (f.highlight + 1) % n
                                };
                            } else if key == "enter" && f.list_open && n > 0 {
                                let h = f.highlight;
                                f.take_row(h);
                            } else if key == "down" && !f.list_open {
                                f.list_open = true;
                            } else if let Some(t) = printable(ks) {
                                f.dir.insert(&t);
                                f.list_open = true;
                                f.highlight = 0;
                            } else if f.dir.key(key, m.alt) {
                                f.list_open = key == "backspace" || key == "delete" || f.list_open;
                            }
                        }
                        Slot::Switch if key == "space" || key == "enter" => {
                            f.worktree = !f.worktree
                        }
                        Slot::Name => {
                            if let Some(t) = printable(ks) {
                                f.name.insert(&t);
                            } else {
                                f.name.key(key, m.alt);
                            }
                        }
                        Slot::Prompt => {
                            if key == "enter" {
                                f.prompt.insert("\n");
                            } else if let Some(t) = printable(ks) {
                                f.prompt.insert(&t);
                            } else {
                                f.prompt.key(key, m.alt);
                            }
                        }
                        Slot::Browse if key == "space" || key == "enter" => return self.browse(cx),
                        Slot::Cancel if key == "space" || key == "enter" => self.overlay = None,
                        Slot::Open if key == "space" || key == "enter" => {
                            return self.submit_new_pane(cx);
                        }
                        _ => {}
                    }
                }
                self.refresh_suggestions();
            }
            Some(Overlay::Palette(p)) => {
                if key == "escape" {
                    self.overlay = None;
                } else if key == "enter" {
                    return self.run_palette(cx);
                } else if key == "up" || key == "down" {
                    let n = self.palette_rows().len().max(1);
                    if let Some(Overlay::Palette(p)) = &mut self.overlay {
                        p.selected = if key == "up" {
                            (p.selected + n - 1) % n
                        } else {
                            (p.selected + 1) % n
                        };
                    }
                } else if let Some(t) = printable(ks) {
                    p.query.insert(&t);
                    p.selected = 0;
                } else {
                    p.query.key(key, m.alt);
                    p.selected = 0;
                }
            }
            None => {}
        }
        cx.notify();
    }

    pub fn render_overlay(&self, s: Scale, cx: &mut Context<Self>) -> Option<Div> {
        let card = match self.overlay.as_ref()? {
            Overlay::NewPane(f) => self.new_pane_card(s, f, cx),
            Overlay::Palette(p) => self.palette_card(s, p, cx),
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(ca(0x06070B, 0.55))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|w, _: &MouseDownEvent, _, cx| {
                        w.overlay = None;
                        cx.notify();
                    }),
                )
                .flex()
                .flex_col()
                .items_center()
                .pt(s.z(110.0))
                .child(card.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())),
        )
    }

    fn card(s: Scale, width: f32) -> Div {
        div()
            .w(s.z(width))
            .flex()
            .flex_col()
            .rounded(s.z(14.0))
            .bg(ca(0x141721, 0.95))
            .border_1()
            .border_color(white(0.09))
            .shadow(vec![chrome::shadow(
                ca(0x000000, 0.8),
                30.0,
                80.0,
                -20.0,
                s,
            )])
            .overflow_hidden()
    }

    fn esc_cap(s: Scale) -> Div {
        div()
            .h(s.z(22.0))
            .px(s.z(7.0))
            .rounded(s.z(6.0))
            .bg(white(0.05))
            .border_1()
            .border_color(white(0.10))
            .flex()
            .items_center()
            .child(text(s, Type::Label, 400.0, c(TEXT2), true, "esc"))
    }

    fn input(s: Scale, field: &Field, placeholder: &str, focused: bool) -> Div {
        let (a, b) = field.halves();
        let caret = div().w(px(1.5)).h(s.z(16.0)).bg(c(ACCENT));
        let mut row = div().flex().items_center().overflow_hidden();
        if field.text.is_empty() {
            row = row.when(focused, |r| r.child(caret)).child(text(
                s,
                Type::Body,
                400.0,
                c(HINT),
                true,
                placeholder.to_string(),
            ));
        } else {
            row = row
                .child(text(s, Type::Body, 400.0, c(TEXT), true, a.to_string()))
                .when(focused, |r| r.child(caret))
                .child(text(s, Type::Body, 400.0, c(TEXT), true, b.to_string()));
        }
        div()
            .h(s.z(36.0))
            .px(s.z(12.0))
            .rounded(s.z(8.0))
            .bg(c(TERM))
            .border_1()
            .border_color(if focused {
                ca(ACCENT, 0.45)
            } else {
                white(0.09)
            })
            .flex()
            .items_center()
            .overflow_hidden()
            .child(row)
    }

    fn textarea(s: Scale, field: &Field, placeholder: &str, focused: bool) -> Div {
        let (a, b) = field.halves();
        let line = |t: &str, color: u32| text(s, Type::Body, 400.0, c(color), false, t.to_string());
        let mut lines = div().flex().flex_col();
        if field.text.is_empty() {
            lines = lines.child(
                div()
                    .flex()
                    .items_center()
                    .when(focused, |r| {
                        r.child(div().w(px(1.5)).h(s.z(16.0)).bg(c(ACCENT)))
                    })
                    .child(line(placeholder, HINT)),
            );
        } else {
            let before: Vec<&str> = a.split('\n').collect();
            let after: Vec<&str> = b.split('\n').collect();
            for l in &before[..before.len() - 1] {
                lines = lines.child(line(l, TEXT));
            }
            lines = lines.child(
                div()
                    .flex()
                    .items_center()
                    .child(line(before[before.len() - 1], TEXT))
                    .when(focused, |r| {
                        r.child(div().w(px(1.5)).h(s.z(16.0)).bg(c(ACCENT)))
                    })
                    .child(line(after[0], TEXT)),
            );
            for l in &after[1..] {
                lines = lines.child(line(l, TEXT));
            }
        }
        div()
            .min_h(s.z(72.0))
            .px(s.z(12.0))
            .py(s.z(8.0))
            .rounded(s.z(8.0))
            .bg(c(TERM))
            .border_1()
            .border_color(if focused {
                ca(ACCENT, 0.45)
            } else {
                white(0.09)
            })
            .overflow_hidden()
            .child(lines)
    }

    fn label(s: Scale, words: &str) -> Div {
        text(s, Type::Small, 500.0, c(TEXT2), false, words.to_string())
    }

    fn focus_slot(
        slot: Slot,
    ) -> impl Fn(&mut Workspace, &MouseDownEvent, &mut gpui::Window, &mut Context<Workspace>) + 'static
    {
        move |w, _, _, cx| {
            if let Some(Overlay::NewPane(f)) = &mut w.overlay {
                f.focus = slot;
                f.list_open = false;
            }
            cx.notify();
        }
    }

    fn button(s: Scale, words: &str, focused: bool, primary: bool) -> Div {
        let b = div()
            .h(s.z(34.0))
            .px(s.z(14.0))
            .rounded(s.z(8.0))
            .flex()
            .items_center()
            .gap(s.z(8.0))
            .border_1()
            .border_color(if focused {
                ca(ACCENT, 0.80)
            } else if primary {
                ca(ACCENT, 0.0)
            } else {
                white(0.08)
            });
        if primary {
            b.bg(c(ACCENT))
                .child(text(
                    s,
                    Type::Body,
                    600.0,
                    c(0x0A0B10),
                    false,
                    words.to_string(),
                ))
                .child(
                    div()
                        .h(s.z(18.0))
                        .px(s.z(5.0))
                        .rounded(s.z(5.0))
                        .bg(ca(0x0A0B10, 0.14))
                        .flex()
                        .items_center()
                        .child(text(s, Type::Key, 500.0, c(0x0A0B10), true, "⌘⏎")),
                )
        } else {
            b.bg(white(0.04)).child(text(
                s,
                Type::Body,
                500.0,
                c(TEXT),
                false,
                words.to_string(),
            ))
        }
    }

    fn new_pane_card(&self, s: Scale, f: &NewPane, cx: &mut Context<Self>) -> Div {
        let title = if f.target == Target::Pane {
            "New pane"
        } else {
            "New tab"
        };
        let header = div()
            .h(s.z(56.0))
            .pl(s.z(20.0))
            .pr(s.z(14.0))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(white(0.07))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(text(s, Type::Wordmark, 600.0, c(TEXT), false, title))
                    .child(text(
                        s,
                        Type::Small,
                        400.0,
                        c(TEXT3),
                        false,
                        "Model and effort are chosen inside the session.",
                    )),
            )
            .child(Self::esc_cap(s));

        let seg = |cli: Cli, digit: &str, cx: &mut Context<Self>| {
            let on = f.cli == cli;
            div()
                .flex_1()
                .h(s.z(32.0))
                .rounded(s.z(7.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(s.z(8.0))
                .border_1()
                .border_color(if on {
                    ca(ACCENT, 0.45)
                } else {
                    ca(ACCENT, 0.0)
                })
                .when(on, |d| d.bg(c(chrome::SEGMENT_ACTIVE)))
                .child(text(
                    s,
                    Type::Label,
                    400.0,
                    c(HINT),
                    true,
                    digit.to_string(),
                ))
                .child(text(
                    s,
                    Type::Body,
                    if on { 500.0 } else { 400.0 },
                    c(if on { TEXT } else { TEXT2 }),
                    false,
                    cli_name(cli),
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |w, _: &MouseDownEvent, _, cx| {
                        if let Some(Overlay::NewPane(f)) = &mut w.overlay {
                            f.cli = cli;
                            f.focus = Slot::Agent;
                        }
                        cx.notify();
                    }),
                )
        };
        let agent = div()
            .flex()
            .flex_col()
            .gap(s.z(8.0))
            .child(Self::label(s, "Agent"))
            .child(
                div()
                    .p(s.z(3.0))
                    .rounded(s.z(10.0))
                    .bg(white(0.03))
                    .border_1()
                    .border_color(if f.focus == Slot::Agent {
                        ca(ACCENT, 0.45)
                    } else {
                        white(0.07)
                    })
                    .flex()
                    .gap(s.z(4.0))
                    .child(seg(Cli::Claude, "1", cx))
                    .child(seg(Cli::Codex, "2", cx))
                    .child(seg(Cli::Shell, "3", cx)),
            );

        let list = (f.list_open && !f.suggestions.is_empty()).then(|| {
            let rows: Vec<Div> = f
                .suggestions
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    let (parent, name) = row.split();
                    let hl = i == f.highlight;
                    let tag = match row.kind {
                        Kind::Recent => Some("recent"),
                        Kind::Repo => Some("repo"),
                        Kind::Completion => None,
                    };
                    div()
                        .h(s.z(30.0))
                        .px(s.z(10.0))
                        .rounded(s.z(7.0))
                        .flex()
                        .items_center()
                        .border_1()
                        .border_color(if hl {
                            ca(ACCENT, 0.28)
                        } else {
                            ca(ACCENT, 0.0)
                        })
                        .when(hl, |d| d.bg(ca(ACCENT, 0.10)))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .flex()
                                .child(text(s, Type::Body, 400.0, c(TEXT3), true, parent))
                                .child(text(s, Type::Body, 600.0, c(TEXT), true, name)),
                        )
                        .children(tag.map(|t| {
                            div()
                                .h(s.z(18.0))
                                .px(s.z(6.0))
                                .rounded(s.z(5.0))
                                .bg(white(0.05))
                                .flex()
                                .items_center()
                                .child(text(s, Type::Label, 400.0, c(TEXT3), true, t))
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |w, _: &MouseDownEvent, _, cx| {
                                if let Some(Overlay::NewPane(f)) = &mut w.overlay {
                                    f.take_row(i);
                                    f.focus = Slot::Directory;
                                }
                                w.refresh_suggestions();
                                cx.notify();
                            }),
                        )
                })
                .collect();
            deferred(
                div()
                    .absolute()
                    .top(s.z(42.0))
                    .left_0()
                    .w(s.z(520.0))
                    .p(s.z(4.0))
                    .rounded(s.z(10.0))
                    .bg(c(0x181B27))
                    .border_1()
                    .border_color(white(0.10))
                    .shadow(vec![chrome::shadow(
                        ca(0x000000, 0.6),
                        16.0,
                        40.0,
                        -12.0,
                        s,
                    )])
                    .occlude()
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .with_priority(1)
        });
        let directory = div()
            .flex()
            .flex_col()
            .gap(s.z(8.0))
            .child(Self::label(s, "Directory"))
            .child(
                div()
                    .flex()
                    .gap(s.z(8.0))
                    .child(
                        div()
                            .flex_1()
                            .relative()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(Self::focus_slot(Slot::Directory)),
                            )
                            .child(Self::input(
                                s,
                                &f.dir,
                                "Search folders and repositories, or type a path",
                                f.focus == Slot::Directory,
                            ))
                            .children(list),
                    )
                    .child(
                        Self::button(s, "Browse…", f.focus == Slot::Browse, false)
                            .h(s.z(36.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|w, _: &MouseDownEvent, _, cx| w.browse(cx)),
                            ),
                    ),
            );

        let worktree = (f.cli == Cli::Claude).then(|| {
            let sub = if f.worktree {
                let n = f.name.text.trim();
                format!(
                    "claude --worktree {}",
                    if n.is_empty() { "<name>" } else { n }
                )
            } else {
                "Claude Code creates and tracks it".into()
            };
            let knob_x = if f.worktree { 17.0 } else { 3.0 };
            div()
                .p(s.z(12.0))
                .rounded(s.z(10.0))
                .bg(white(0.02))
                .border_1()
                .border_color(if f.focus == Slot::Switch {
                    ca(ACCENT, 0.45)
                } else {
                    white(0.06)
                })
                .flex()
                .flex_col()
                .gap(s.z(12.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(s.z(12.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|w, _: &MouseDownEvent, _, cx| {
                                if let Some(Overlay::NewPane(f)) = &mut w.overlay {
                                    f.worktree = !f.worktree;
                                    f.focus = Slot::Switch;
                                }
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_none()
                                .w(s.z(34.0))
                                .h(s.z(20.0))
                                .rounded(s.z(10.0))
                                .bg(if f.worktree { c(ACCENT) } else { white(0.10) })
                                .child(
                                    div()
                                        .absolute()
                                        .top(s.z(3.0))
                                        .left(s.z(knob_x))
                                        .size(s.z(14.0))
                                        .rounded(s.z(7.0))
                                        .bg(c(TEXT)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(text(
                                    s,
                                    Type::Body,
                                    500.0,
                                    c(TEXT),
                                    false,
                                    "Run in a Claude Code worktree",
                                ))
                                .child(text(s, Type::Label, 400.0, c(HINT), true, sub)),
                        ),
                )
                .when(f.worktree, |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(s.z(8.0))
                            .child(Self::label(s, "Worktree name"))
                            .child(
                                div()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(Self::focus_slot(Slot::Name)),
                                    )
                                    .child(Self::input(
                                        s,
                                        &f.name,
                                        "my-feature",
                                        f.focus == Slot::Name,
                                    )),
                            ),
                    )
                })
        });

        let prompt = (f.cli != Cli::Shell).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(s.z(8.0))
                .child(
                    div()
                        .flex()
                        .gap(s.z(6.0))
                        .child(Self::label(s, "First prompt"))
                        .child(text(s, Type::Small, 400.0, c(HINT), false, "(optional)")),
                )
                .child(
                    div()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(Self::focus_slot(Slot::Prompt)),
                        )
                        .child(Self::textarea(
                            s,
                            &f.prompt,
                            "What should it work on?",
                            f.focus == Slot::Prompt,
                        )),
                )
        });

        let info = match &f.error {
            Some(e) => text(s, Type::Small, 400.0, c(RED), false, e.clone()),
            None => {
                let place = if f.target == Target::Pane {
                    "in this tab"
                } else {
                    "in a new tab"
                };
                let real = if f.cli == Cli::Shell {
                    ""
                } else {
                    " · a real CLI session"
                };
                text(
                    s,
                    Type::Small,
                    400.0,
                    c(TEXT3),
                    false,
                    format!("{} {place}{real}", cli_name(f.cli)),
                )
            }
        };
        let open_words = if f.pending {
            "Opening…"
        } else if f.target == Target::Pane {
            "Open pane"
        } else {
            "Open tab"
        };
        let footer = div()
            .h(s.z(60.0))
            .pl(s.z(20.0))
            .pr(s.z(14.0))
            .flex()
            .items_center()
            .gap(s.z(8.0))
            .border_t_1()
            .border_color(white(0.07))
            .child(div().flex_1().min_w(px(0.0)).overflow_hidden().child(info))
            .child(
                Self::button(s, "Cancel", f.focus == Slot::Cancel, false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|w, _: &MouseDownEvent, _, cx| {
                        w.overlay = None;
                        cx.notify();
                    }),
                ),
            )
            .child(
                Self::button(s, open_words, f.focus == Slot::Open, true).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|w, _: &MouseDownEvent, _, cx| w.submit_new_pane(cx)),
                ),
            );

        Self::card(s, 560.0)
            .child(header)
            .child(
                div()
                    .px(s.z(20.0))
                    .py(s.z(18.0))
                    .flex()
                    .flex_col()
                    .gap(s.z(18.0))
                    .child(agent)
                    .child(directory)
                    .children(worktree)
                    .children(prompt),
            )
            .child(footer)
    }

    fn palette_card(&self, s: Scale, p: &Palette, cx: &mut Context<Self>) -> Div {
        let rows: Vec<Div> = self
            .palette_rows()
            .into_iter()
            .enumerate()
            .map(|(i, (label, hint, dot, _))| {
                let sel = i == p.selected;
                div()
                    .h(s.z(40.0))
                    .px(s.z(10.0))
                    .rounded(s.z(9.0))
                    .flex()
                    .items_center()
                    .gap(s.z(12.0))
                    .border_1()
                    .border_color(if sel {
                        ca(ACCENT, 0.28)
                    } else {
                        ca(ACCENT, 0.0)
                    })
                    .when(sel, |d| d.bg(ca(ACCENT, 0.10)))
                    .child(chrome::dot(s, 6.0, c(dot)))
                    .child(text(s, Type::Body, 500.0, c(TEXT), false, label))
                    .child(div().flex_1().min_w(px(0.0)).overflow_hidden().child(text(
                        s,
                        Type::Body,
                        400.0,
                        c(TEXT3),
                        false,
                        hint,
                    )))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |w, _: &MouseDownEvent, _, cx| {
                            if let Some(Overlay::Palette(p)) = &mut w.overlay {
                                p.selected = i;
                            }
                            w.run_palette(cx);
                        }),
                    )
            })
            .collect();
        let empty = rows.is_empty();
        let (a, b) = p.query.halves();
        Self::card(s, 640.0)
            .child(
                div()
                    .h(s.z(56.0))
                    .pl(s.z(18.0))
                    .pr(s.z(14.0))
                    .flex()
                    .items_center()
                    .gap(s.z(12.0))
                    .border_b_1()
                    .border_color(white(0.07))
                    .child(
                        div()
                            .flex_1()
                            .h(s.z(36.0))
                            .flex()
                            .items_center()
                            .text_size(s.z(15.0))
                            .child(div().text_color(c(TEXT)).child(a.to_string()))
                            .child(div().w(px(1.5)).h(s.z(18.0)).bg(c(ACCENT)))
                            .child(div().text_color(c(TEXT)).child(b.to_string())),
                    )
                    .child(Self::esc_cap(s)),
            )
            .child(
                div()
                    .px(s.z(8.0))
                    .pb(s.z(6.0))
                    .flex()
                    .flex_col()
                    .gap(s.z(2.0))
                    .children(rows)
                    .when(empty, |d| {
                        d.child(div().p(s.z(10.0)).child(text(
                            s,
                            Type::Body,
                            400.0,
                            c(TEXT3),
                            false,
                            "No matching command, tab or pane",
                        )))
                    }),
            )
            .child(
                div()
                    .h(s.z(40.0))
                    .px(s.z(18.0))
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(white(0.07))
                    .child(text(
                        s,
                        Type::Label,
                        400.0,
                        c(HINT),
                        true,
                        "↑↓ move · ⏎ run",
                    )),
            )
    }

    /// The top bar's Codex chip: who Codex is signed in as, or its sign-in in progress.
    pub fn codex_chip(&self, s: Scale, cx: &mut Context<Self>) -> Option<Div> {
        let a = self.codex.as_ref()?;
        let signing = a.signing_in.is_some();
        let (dot, body): (u32, Div) = if signing {
            (
                AMBER,
                text(
                    s,
                    Type::Body,
                    500.0,
                    c(AMBER),
                    false,
                    "Signing in to Codex…",
                ),
            )
        } else if let Some(cur) = &a.current {
            let plan = if cur.api_key {
                String::new()
            } else {
                cur.plan.as_deref().map(codex_plan).unwrap_or_default()
            };
            let who = if cur.api_key {
                "API key".to_string()
            } else {
                cur.email.clone().unwrap_or_else(|| "ChatGPT".into())
            };
            (
                MINT,
                div()
                    .flex()
                    .gap(s.z(6.0))
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(text(
                        s,
                        Type::Body,
                        500.0,
                        c(TEXT2),
                        false,
                        format!("Codex · {who}"),
                    ))
                    .child(text(s, Type::Body, 400.0, c(HINT), false, plan)),
            )
        } else {
            (
                TEXT3,
                text(s, Type::Body, 500.0, c(TEXT2), false, "Codex signed out"),
            )
        };
        Some(
            div()
                .flex_none()
                .max_w(s.z(340.0))
                .h(s.z(26.0))
                .pl(s.z(10.0))
                .pr(s.z(12.0))
                .rounded(s.z(13.0))
                .bg(if signing {
                    ca(AMBER, 0.10)
                } else {
                    white(0.04)
                })
                .border_1()
                .border_color(if signing {
                    ca(AMBER, 0.35)
                } else {
                    white(0.08)
                })
                .flex()
                .items_center()
                .gap(s.z(8.0))
                .overflow_hidden()
                .child(chrome::dot(s, 7.0, c(dot)))
                .child(body)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|w, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        w.open_accounts(cx);
                    }),
                ),
        )
    }

    /// The top bar's account chip: who Claude Code is signed in to, or the sign-in in progress.
    pub fn account_chip(&self, s: Scale, cx: &mut Context<Self>) -> Option<Div> {
        let a = self.accounts.as_ref()?;
        let signing = a.signing_in.is_some();
        let (dot, body): (u32, Div) = if signing {
            (
                AMBER,
                text(
                    s,
                    Type::Body,
                    500.0,
                    c(AMBER),
                    false,
                    "Signing in to Claude…",
                ),
            )
        } else if let Some(cur) = &a.current {
            let org = matches!(cur.plan.as_deref(), Some("team" | "enterprise"));
            (
                if org { ACCENT } else { MINT },
                div()
                    .flex()
                    .gap(s.z(6.0))
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(text(
                        s,
                        Type::Body,
                        500.0,
                        c(TEXT2),
                        false,
                        cur.email.clone(),
                    ))
                    .child(text(s, Type::Body, 400.0, c(HINT), false, kind(cur))),
            )
        } else {
            (
                TEXT3,
                text(s, Type::Body, 500.0, c(TEXT2), false, "Claude signed out"),
            )
        };
        Some(
            div()
                .flex_none()
                .max_w(s.z(380.0))
                .h(s.z(26.0))
                .pl(s.z(10.0))
                .pr(s.z(12.0))
                .rounded(s.z(13.0))
                .bg(if signing {
                    ca(AMBER, 0.10)
                } else {
                    white(0.04)
                })
                .border_1()
                .border_color(if signing {
                    ca(AMBER, 0.35)
                } else {
                    white(0.08)
                })
                .flex()
                .items_center()
                .gap(s.z(8.0))
                .overflow_hidden()
                .child(chrome::dot(s, 7.0, c(dot)))
                .child(body)
                .child(text(s, Type::Label, 400.0, c(TEXT3), true, "⌘⇧A"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|w, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        w.open_accounts(cx);
                    }),
                ),
        )
    }
}
