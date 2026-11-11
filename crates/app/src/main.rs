//! ply's window: tabs of up to four terminal panes, drawn with GPUI and fed by plyd over C1 (control) and C2 (screen
//! rows). It owns no pty and emulates no terminal; it never makes a network request and never writes a CLI's files.
#![forbid(unsafe_code)]

mod chrome;
mod conn;
mod control;
mod dirs;
mod drop;
mod field;
mod forms;
mod keys;
mod launcher;
mod paint;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use gpui::{
    AnyView, App, AppContext, Application, Bounds, Context, Div, Entity, FocusHandle, Font,
    FontFeatures, FontStyle, FontWeight, KeyDownEvent, MouseButton, Render, ScrollDelta,
    ScrollWheelEvent, SharedString, StyleRefinement, Task, TitlebarOptions, Window, WindowBounds,
    WindowOptions, canvas, div, prelude::*, px, size,
};
use ply_proto::control::Event;
use ply_proto::data::{
    Frame, Modes, Mods, MouseAction, MouseButton as WheelButton, MouseEvent, Paste,
};
use ply_proto::pane::{ClaudeAccounts, Cli, Layout, Pane, PaneStatus, SignInOutcome, Tab};
use ply_term::Palette;

use chrome::Scale;
use conn::{Conn, Grid};
use paint::{FONT_FAMILY, Metrics};

static WAKES: AtomicU32 = AtomicU32::new(0);

struct Args {
    stats: bool,
}

fn parse_args() -> Args {
    Args {
        stats: std::env::args().any(|a| a == "--stats"),
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn pct(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((q * sorted.len() as f64) as usize).min(sorted.len() - 1)]
}

#[derive(Default)]
struct Stats {
    frame_start: Option<Instant>,
    frames: u32,
    frame_ms: Vec<f64>,
    gaps: Vec<f64>,
    last_present: Option<Instant>,
    intervals: Vec<f64>,
}

impl Stats {
    fn report(&mut self) -> String {
        let mut f = std::mem::take(&mut self.frame_ms);
        let mut g = std::mem::take(&mut self.gaps);
        let mut iv = std::mem::take(&mut self.intervals);
        f.sort_by(f64::total_cmp);
        g.sort_by(f64::total_cmp);
        iv.sort_by(f64::total_cmp);
        let frames = std::mem::take(&mut self.frames);
        format!(
            "{{\"frames\":{frames},\"pane_updates\":{},\"frame_p50_ms\":{:.3},\"frame_p99_ms\":{:.3},\"frame_max_ms\":{:.3},\"interval_p50_ms\":{:.2},\"interval_p99_ms\":{:.2},\"stall_p99_ms\":{:.2},\"stall_max_ms\":{:.2},\"stalls_over_16ms\":{},\"stalls_over_33ms\":{}}}",
            WAKES.swap(0, Ordering::Relaxed),
            pct(&f, 0.5),
            pct(&f, 0.99),
            f.last().copied().unwrap_or(0.0),
            pct(&iv, 0.5),
            pct(&iv, 0.99),
            pct(&g, 0.99),
            g.last().copied().unwrap_or(0.0),
            g.iter().filter(|&&x| x > 1000.0 / 60.0).count(),
            g.iter().filter(|&&x| x > 2000.0 / 60.0).count(),
        )
    }
}

struct PaneView {
    conn: Arc<Conn>,
    info: Option<Pane>,
    /// Absolute line at the top of a scrolled-back view; `None` follows the live screen.
    view_top: Option<u64>,
    scroll_accum: f32,
    grid_origin: Rc<std::cell::Cell<gpui::Point<gpui::Pixels>>>,
    mouse_cell: Option<(u16, u16)>,
    position: usize,
    scale: f32,
    shell: String,
    focused: bool,
    cursor_on: bool,
    metrics: Metrics,
    palette: Rc<Palette>,
    _wake: Task<()>,
    _blink: Task<()>,
}

impl PaneView {
    fn new(
        pane_id: u64,
        position: usize,
        info: Option<Pane>,
        socket: PathBuf,
        metrics: Metrics,
        palette: Rc<Palette>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, mut rx) = unbounded();
        let conn = Conn::new(pane_id, socket, tx);
        let wake = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                WAKES.fetch_add(1, Ordering::Relaxed);
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let blink = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(530))
                    .await;
                let alive = this.update(cx, |v, cx| {
                    let cursor = v
                        .conn
                        .shared
                        .replica
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .cursor();
                    if v.focused && cursor.visible && cursor.blinking {
                        v.cursor_on = !v.cursor_on;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        Self {
            conn,
            info,
            view_top: None,
            scroll_accum: 0.0,
            grid_origin: Rc::default(),
            mouse_cell: None,
            position,
            scale: f32::from(metrics.font_size) / 12.5,
            shell: chrome::shell_name(),
            focused: false,
            cursor_on: true,
            metrics,
            palette,
            _wake: wake,
            _blink: blink,
        }
    }
}

impl PaneView {
    /// The wheel, as ply does it: the program's mouse report, arrow keys on the alternate screen, else ply's own scrollback view.
    /// The cell under `pos`, and its centre in plyd's whole-pixel cells, which encode the report.
    fn pointer(&self, pos: gpui::Point<gpui::Pixels>) -> (u16, u16, f32, f32) {
        let origin = self.grid_origin.get();
        let col = (f32::from(pos.x - origin.x) / f32::from(self.metrics.cell_w)).max(0.0) as u16;
        let row = (f32::from(pos.y - origin.y) / f32::from(self.metrics.cell_h)).max(0.0) as u16;
        let (w, h) = (
            f32::from(self.metrics.cell_w).round(),
            f32::from(self.metrics.cell_h).round(),
        );
        (
            col,
            row,
            (f32::from(col) + 0.5) * w,
            (f32::from(row) + 0.5) * h,
        )
    }

    /// Clicks, releases and drags go to a program that reports the mouse; a move only when the cell changes.
    fn report_mouse(
        &mut self,
        action: MouseAction,
        button: Option<MouseButton>,
        pos: gpui::Point<gpui::Pixels>,
        mods: &gpui::Modifiers,
    ) {
        let modes = self
            .conn
            .shared
            .replica
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .modes();
        if !modes.contains(Modes::MOUSE_REPORTING) {
            return;
        }
        let (col, row, x, y) = self.pointer(pos);
        if action == MouseAction::Motion && self.mouse_cell == Some((col, row)) {
            return;
        }
        self.mouse_cell = Some((col, row));
        let button = match button {
            Some(MouseButton::Left) => WheelButton::Left,
            Some(MouseButton::Right) => WheelButton::Right,
            Some(MouseButton::Middle) => WheelButton::Middle,
            _ => WheelButton::None,
        };
        self.conn.send(&Frame::Mouse(MouseEvent {
            action,
            button,
            mods: mouse_mods(mods),
            col,
            row,
            x,
            y,
        }));
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let lines = match ev.delta {
            ScrollDelta::Lines(p) => p.y * 3.0,
            ScrollDelta::Pixels(p) => f32::from(p.y) / f32::from(self.metrics.cell_h),
        };
        self.scroll_accum += lines;
        let steps = self.scroll_accum.trunc() as i64;
        self.scroll_accum -= steps as f32;
        if steps == 0 {
            return;
        }
        let (modes, live_top, base) = {
            let r = self
                .conn
                .shared
                .replica
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                r.modes(),
                r.scrollback_base() + u64::from(r.scrollback_rows()),
                r.scrollback_base(),
            )
        };
        let n = steps.unsigned_abs().min(10);
        if modes.contains(Modes::MOUSE_REPORTING) {
            let (col, row, x, y) = self.pointer(ev.position);
            let mouse = MouseEvent {
                action: MouseAction::Press,
                button: if steps > 0 {
                    WheelButton::Four
                } else {
                    WheelButton::Five
                },
                mods: mouse_mods(&ev.modifiers),
                col,
                row,
                x,
                y,
            };
            for _ in 0..n {
                self.conn.send(&Frame::Mouse(mouse));
            }
        } else if modes.contains(Modes::ALT_SCREEN) {
            let key = keys::arrow(steps > 0);
            for _ in 0..n {
                self.conn.send(&Frame::Key(key.clone()));
            }
        } else {
            let top = (self.view_top.unwrap_or(live_top) as i64 - steps)
                .clamp(base as i64, live_top as i64) as u64;
            self.view_top = (top < live_top).then_some(top);
            cx.notify();
        }
    }
}

impl Render for PaneView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.conn.shared.dirty.store(false, Ordering::SeqCst);
        let view_top = self.view_top;
        let scrolled_by = view_top.map(|top| {
            let r = self
                .conn
                .shared
                .replica
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (r.scrollback_base() + u64::from(r.scrollback_rows())).saturating_sub(top)
        });
        let origin_cell = self.grid_origin.clone();
        let m = self.metrics;
        let pal = self.palette.clone();
        let focused = self.focused;
        let cursor_on = self.cursor_on;
        let title = match &self.info {
            Some(p) if !p.title.is_empty() => p.title.clone(),
            _ => {
                let t = self
                    .conn
                    .shared
                    .title
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                if t.is_empty() {
                    format!("pane {}", self.conn.pane_id)
                } else {
                    t
                }
            }
        };
        let exit = *self
            .conn
            .shared
            .exit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut info = self.info.clone();
        if let (Some(p), Some(code)) = (info.as_mut(), exit)
            && p.exit_code.is_none()
        {
            p.status = PaneStatus::Exited;
            p.exit_code = Some(code);
        }
        let sc = Scale(self.scale);
        let waiting = info.as_ref().is_some_and(chrome::needs_you);
        let (layout_conn, paint_conn) = (self.conn.clone(), self.conn.clone());
        chrome::pane_frame(sc, focused, waiting)
            .child(chrome::pane_header(
                sc,
                self.position,
                info.as_ref(),
                &title,
                focused,
                scrolled_by,
                &self.shell,
            ))
            .child(
                chrome::term_body(sc)
                    .on_scroll_wheel(cx.listener(Self::on_scroll))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|v, e: &gpui::MouseDownEvent, _, _| {
                            v.report_mouse(
                                MouseAction::Press,
                                Some(e.button),
                                e.position,
                                &e.modifiers,
                            )
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|v, e: &gpui::MouseDownEvent, _, _| {
                            v.report_mouse(
                                MouseAction::Press,
                                Some(e.button),
                                e.position,
                                &e.modifiers,
                            )
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(|v, e: &gpui::MouseDownEvent, _, _| {
                            v.report_mouse(
                                MouseAction::Press,
                                Some(e.button),
                                e.position,
                                &e.modifiers,
                            )
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|v, e: &gpui::MouseUpEvent, _, _| {
                            v.report_mouse(
                                MouseAction::Release,
                                Some(e.button),
                                e.position,
                                &e.modifiers,
                            )
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(|v, e: &gpui::MouseUpEvent, _, _| {
                            v.report_mouse(
                                MouseAction::Release,
                                Some(e.button),
                                e.position,
                                &e.modifiers,
                            )
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(|v, e: &gpui::MouseUpEvent, _, _| {
                            v.report_mouse(
                                MouseAction::Release,
                                Some(e.button),
                                e.position,
                                &e.modifiers,
                            )
                        }),
                    )
                    .on_mouse_move(cx.listener(|v, e: &gpui::MouseMoveEvent, _, _| {
                        v.report_mouse(
                            MouseAction::Motion,
                            e.pressed_button,
                            e.position,
                            &e.modifiers,
                        )
                    }))
                    .child(
                        canvas(
                            move |bounds, _window, _cx| {
                                origin_cell.set(bounds.origin);
                                let cols = (f32::from(bounds.size.width) / f32::from(m.cell_w))
                                    .floor()
                                    .max(1.0) as u16;
                                let rows = (f32::from(bounds.size.height) / f32::from(m.cell_h))
                                    .floor()
                                    .max(1.0) as u16;
                                layout_conn.ensure_grid(Grid {
                                    cols,
                                    rows,
                                    cell_w: f32::from(m.cell_w).round() as u16,
                                    cell_h: f32::from(m.cell_h).round() as u16,
                                });
                            },
                            move |bounds, (), window, cx| {
                                let replica = paint_conn
                                    .shared
                                    .replica
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                if replica.last_seq().is_none() {
                                    return;
                                }
                                let missing = paint::paint_screen(
                                    &replica,
                                    &pal,
                                    m,
                                    paint::View {
                                        bounds,
                                        view_top,
                                        focused,
                                        cursor_on,
                                    },
                                    window,
                                    cx,
                                );
                                drop(replica);
                                if let Some(line) = missing {
                                    paint_conn.fetch_history(line);
                                }
                            },
                        )
                        .size_full(),
                    ),
            )
    }
}

struct Workspace {
    tabs: Vec<Vec<u64>>,
    tab_ids: Vec<u64>,
    workspace_id: Option<u64>,
    workspace_path: String,
    overlay: Option<forms::Overlay>,
    recents: Vec<(String, u64)>,
    repos: Option<Vec<String>>,
    scanning: bool,
    accounts: Option<ClaudeAccounts>,
    codex: Option<ply_proto::pane::CodexAccounts>,
    notice: Option<String>,
    notice_task: Option<Task<()>>,
    pending_focus: Option<u64>,
    turn_started: HashMap<u64, u64>,
    tab_names: Vec<String>,
    tab_focus: Vec<usize>,
    infos: HashMap<u64, Pane>,
    control: Option<PathBuf>,
    active: usize,
    panes: Vec<Entity<PaneView>>,
    focused: usize,
    tab_zoomed: Vec<bool>,
    font_size: f32,
    save_task: Option<Task<()>>,
    socket: PathBuf,
    metrics: Metrics,
    palette: Rc<Palette>,
    focus: FocusHandle,
    stats: Option<Rc<RefCell<Stats>>>,
    _tasks: Vec<Task<()>>,
}

impl Workspace {
    fn new(
        args: &Args,
        home: &std::path::Path,
        metrics: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let sock = home.join("run").join("plyd.sock");
        let mut this = Self {
            tabs: Vec::new(),
            tab_ids: Vec::new(),
            workspace_id: None,
            workspace_path: String::new(),
            overlay: None,
            recents: Vec::new(),
            repos: None,
            scanning: false,
            accounts: None,
            codex: None,
            notice: None,
            notice_task: None,
            pending_focus: None,
            turn_started: HashMap::new(),
            tab_names: Vec::new(),
            tab_focus: Vec::new(),
            infos: HashMap::new(),
            control: Some(sock.clone()),
            active: 0,
            panes: Vec::new(),
            focused: 0,
            tab_zoomed: Vec::new(),
            font_size: f32::from(metrics.font_size),
            save_task: None,
            socket: home.join("run").join("data.sock"),
            metrics,
            palette: Rc::new(Palette::from_theme(&paint::theme_for(&Default::default()))),
            focus,
            stats: args.stats.then(Rc::default),
            _tasks: Vec::new(),
        };
        if let Some(stats) = this.stats.clone() {
            let probe = stats.clone();
            this._tasks.push(cx.spawn(async move |_, cx| {
                let mut last = Instant::now();
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(1))
                        .await;
                    let now = Instant::now();
                    probe.borrow_mut().gaps.push(ms(now - last));
                    last = now;
                }
            }));
            this._tasks.push(cx.spawn(async move |_, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    tracing::info!(target: "frame_stats", "frame stats {}", stats.borrow_mut().report());
                }
            }));
        }
        let (tx, mut rx) = unbounded();
        control::watch(&sock, tx);
        match control::load(&sock, paint::theme_for) {
            Ok(loaded) => {
                let first = this.apply(loaded);
                this.show_tab(first, cx);
            }
            Err(e) => {
                tracing::error!(error = %e, "cannot load ply's tabs");
                this.notice(format!("plyd: {e}"), cx);
            }
        }
        this._tasks.push(cx.spawn(async move |this, cx| {
            while let Some(msg) = rx.next().await {
                let reload = match msg {
                    control::Watch::Connected => true,
                    control::Watch::Event(e) => {
                        this.update(cx, |w, cx| w.on_event(e, cx)).unwrap_or(false)
                    }
                };
                if reload {
                    let sock = sock.clone();
                    match cx
                        .background_executor()
                        .spawn(async move { control::load(&sock, paint::theme_for) })
                        .await
                    {
                        Ok(l) => {
                            let _ = this.update(cx, |w, cx| w.reload(l, cx));
                        }
                        Err(e) => tracing::warn!(error = %e, "reloading the tabs failed"),
                    }
                }
            }
        }));
        this.load_accounts(cx);
        this
    }

    /// `layout.save` 250 ms after the last change of tab, focus or zoom, as the app does.
    fn save_layout_soon(&mut self, cx: &mut Context<Self>) {
        let (Some(sock), Some(ws)) = (self.control.clone(), self.workspace_id) else {
            return;
        };
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let Ok(layout) = this.update(cx, |w, _| w.layout_now()) else {
                return;
            };
            let saved = cx
                .background_executor()
                .spawn(async move { control::save_layout(&sock, ws, layout) })
                .await;
            if let Err(e) = saved {
                tracing::warn!(error = %e, "layout.save failed");
            }
        }));
    }

    fn layout_now(&self) -> Layout {
        let tabs = (0..self.tabs.len().min(self.tab_ids.len()))
            .map(|i| Tab {
                id: self.tab_ids[i],
                name: self.tab_names.get(i).cloned().unwrap_or_default(),
                position: i as u32,
                pane_ids: self.tabs[i].clone(),
                focus_pane_id: self
                    .tab_focus
                    .get(i)
                    .and_then(|&f| self.tabs[i].get(f).copied()),
                zoomed: self.tab_zoomed.get(i).copied().unwrap_or(false),
            })
            .collect();
        Layout {
            tabs,
            active_tab_id: self.tab_ids.get(self.active).copied(),
        }
    }

    fn zoomed(&self) -> bool {
        self.tab_zoomed.get(self.active).copied().unwrap_or(false)
    }

    /// A status-bar notice for 5 s, as the app shows them.
    fn notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.notice = Some(text.clone());
        self.notice_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let _ = this.update(cx, |w, cx| {
                if w.notice.as_deref() == Some(text.as_str()) {
                    w.notice = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    /// Shows pane `id`'s tab and focuses it, now or once a reload brings the pane in.
    fn focus_pane(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some((t, i)) = self
            .tabs
            .iter()
            .enumerate()
            .find_map(|(t, ids)| ids.iter().position(|&p| p == id).map(|i| (t, i)))
        else {
            self.pending_focus = Some(id);
            return;
        };
        self.pending_focus = None;
        if t != self.active || self.panes.len() != self.tabs[t].len() {
            self.show_tab(t, cx);
        }
        self.set_focus(i, cx);
    }

    /// Takes a C1 load; returns the tab to show first.
    fn apply(&mut self, loaded: control::Loaded) -> usize {
        self.palette = Rc::new(Palette::from_theme(&paint::theme_for(&loaded.settings)));
        self.tab_zoomed = loaded.zoomed;
        self.workspace_id = Some(loaded.workspace_id);
        self.workspace_path = loaded.workspace_path;
        self.tab_ids = loaded.tab_ids;
        self.tab_names = loaded.tabs.iter().map(|(n, _)| n.clone()).collect();
        self.tabs = loaded.tabs.into_iter().map(|(_, ids)| ids).collect();
        self.tab_focus = loaded.focus;
        self.infos = loaded.panes;
        loaded.active_tab
    }

    fn reload(&mut self, loaded: control::Loaded, cx: &mut Context<Self>) {
        let shown: Vec<u64> = self.panes.iter().map(|p| p.read(cx).conn.pane_id).collect();
        self.apply(loaded);
        let still = self.tabs.iter().position(|t| *t == shown);
        match still {
            Some(i) => self.active = i,
            None => self.show_tab(self.active.min(self.tabs.len().saturating_sub(1)), cx),
        }
        if let Some(id) = self.pending_focus {
            self.focus_pane(id, cx);
        }
        cx.notify();
    }

    /// Applies a pane event; returns whether the tabs must be loaded again.
    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) -> bool {
        let id = match &event {
            Event::PaneAdded(_) => return true,
            Event::PaneRemoved(e) => {
                drop::release(e.pane_id, u64::MAX);
                self.turn_started.remove(&e.pane_id);
                return true;
            }
            Event::CodexChanged(e) => {
                let l = forms::codex_who(e.account.as_ref());
                let r = match e.panes {
                    0 => String::new(),
                    1 => " · 1 Codex pane restarts on it once idle".into(),
                    n => format!(" · {n} Codex panes restart on it once idle"),
                };
                let words = match e.outcome {
                    SignInOutcome::SignedIn | SignInOutcome::OtherAccount => {
                        format!("Signed in to Codex as {l}{r}")
                    }
                    SignInOutcome::Unchanged => format!("Still signed in to Codex as {l}"),
                    SignInOutcome::Failed => {
                        "The Codex sign-in did not finish; its tab shows why".into()
                    }
                    SignInOutcome::Cancelled => {
                        "The Codex sign-in was closed before it finished".into()
                    }
                };
                self.notice(words, cx);
                self.load_accounts(cx);
                return false;
            }
            Event::AccountChanged(e) => {
                let l = forms::who(e.account.as_ref());
                let r = match e.panes {
                    0 => String::new(),
                    1 => " · 1 Claude pane restarts on it once idle".into(),
                    n => format!(" · {n} Claude panes restart on it once idle"),
                };
                let words = match e.outcome {
                    SignInOutcome::SignedIn => format!("Signed in to Claude as {l}{r}"),
                    SignInOutcome::OtherAccount => {
                        format!("Signed in to Claude as {l}, not the account picked{r}")
                    }
                    SignInOutcome::Unchanged => format!("Still signed in to Claude as {l}"),
                    SignInOutcome::Failed => {
                        "The Claude sign-in did not finish; its tab shows why".into()
                    }
                    SignInOutcome::Cancelled => {
                        "The Claude sign-in was closed before it finished".into()
                    }
                };
                self.notice(words, cx);
                self.load_accounts(cx);
                return false;
            }
            Event::PaneStatus(e) => {
                let was_running = self
                    .infos
                    .get(&e.pane_id)
                    .is_some_and(|p| p.status == PaneStatus::Running);
                if e.status == PaneStatus::Running
                    && !was_running
                    && let Some(last) = self.turn_started.insert(e.pane_id, drop::stamp())
                {
                    drop::release(e.pane_id, last);
                }
                if let Some(p) = self.infos.get_mut(&e.pane_id) {
                    p.status = e.status;
                    p.detail = e.detail.clone();
                    p.exit_code = e.exit_code;
                }
                e.pane_id
            }
            Event::PaneMeta(e) => {
                if let Some(p) = self.infos.get_mut(&e.pane_id) {
                    p.model_seen = e.model.clone();
                }
                e.pane_id
            }
            _ => return false,
        };
        let info = self.infos.get(&id).cloned();
        for v in &self.panes {
            if v.read(cx).conn.pane_id == id {
                v.update(cx, |v, cx| {
                    v.info = info.clone();
                    cx.notify();
                });
            }
        }
        cx.notify();
        false
    }

    fn show_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(ids) = self.tabs.get(idx).cloned() else {
            return;
        };
        let mut old: HashMap<u64, Entity<PaneView>> = self
            .panes
            .drain(..)
            .map(|p| (p.read(cx).conn.pane_id, p))
            .collect();
        self.active = idx;
        self.panes = ids
            .iter()
            .enumerate()
            .map(|(pos, &id)| match old.remove(&id) {
                Some(view) => {
                    let info = self.infos.get(&id).cloned();
                    view.update(cx, |v, cx| {
                        v.position = pos;
                        v.info = info;
                        cx.notify();
                    });
                    view
                }
                None => {
                    let (socket, m, pal) =
                        (self.socket.clone(), self.metrics, self.palette.clone());
                    let info = self.infos.get(&id).cloned();
                    cx.new(|cx| PaneView::new(id, pos, info, socket, m, pal, cx))
                }
            })
            .collect();
        for gone in old.into_values() {
            gone.read(cx).conn.close();
        }
        let focus = self
            .tab_focus
            .get(idx)
            .copied()
            .unwrap_or(0)
            .min(self.panes.len().saturating_sub(1));
        self.set_focus(focus, cx);
    }

    fn set_focus(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.focused = idx;
        if let Some(f) = self.tab_focus.get_mut(self.active) {
            *f = idx;
        }
        self.save_layout_soon(cx);
        for (i, p) in self.panes.iter().enumerate() {
            p.update(cx, |v, cx| {
                v.focused = i == idx;
                v.cursor_on = true;
                cx.notify();
            });
        }
        cx.notify();
    }

    /// ply's ⌘ chords that a viewer can honour (`app/src/keymap/keymap.ts`); every other key goes to the focused pane.
    fn key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.is_some() {
            self.overlay_key(ev, cx);
            cx.stop_propagation();
            return;
        }
        let ks = &ev.keystroke;
        if ks.modifiers.platform {
            let n = self.panes.len();
            let shift = ks.modifiers.shift;
            match ks.key.as_str() {
                "q" => cx.quit(),
                "{" => self.step_tab(-1, cx),
                "}" => self.step_tab(1, cx),
                "[" if shift => self.step_tab(-1, cx),
                "]" if shift => self.step_tab(1, cx),
                "[" if n > 0 => self.set_focus((self.focused + n - 1) % n, cx),
                "]" if n > 0 => self.set_focus((self.focused + 1) % n, cx),
                "left" | "right" | "up" | "down" => {
                    if let Some(i) = neighbour(self.focused, n, &ks.key) {
                        self.set_focus(i, cx);
                    }
                }
                "j" => self.next_waiting(cx),
                "n" => self.open_new_pane(forms::Target::Pane, cx),
                "t" => self.open_new_pane(forms::Target::Tab, cx),
                "a" if shift => self.open_accounts(cx),
                "enter" => {
                    if let Some(z) = self.tab_zoomed.get_mut(self.active) {
                        *z = !*z;
                    }
                    self.save_layout_soon(cx);
                    cx.notify();
                }
                "=" | "+" => self.set_font(self.font_size + 1.0, cx),
                "-" => self.set_font(self.font_size - 1.0, cx),
                "0" => self.set_font(12.5, cx),
                "v" => {
                    let item = cx.read_from_clipboard();
                    let id = self
                        .panes
                        .get(self.focused)
                        .map(|p| p.read(cx).conn.pane_id);
                    let text = item.as_ref().and_then(|c| c.text()).or_else(|| {
                        let image = item.as_ref()?.entries().iter().find_map(|e| match e {
                            gpui::ClipboardEntry::Image(i) => Some(i),
                            _ => None,
                        })?;
                        drop::save_pasted_image(image, id?).map(|path| drop::typed(&[path]))
                    });
                    if let (Some(text), Some(pane)) = (text, self.panes.get(self.focused)) {
                        pane.update(cx, |v, cx| {
                            v.view_top = None;
                            v.conn.send(&Frame::Paste(Paste {
                                allow_unsafe: false,
                                text,
                            }));
                            cx.notify();
                        });
                    }
                }
                k => {
                    if let Ok(d) = k.parse::<usize>()
                        && (1..=self.tabs.len()).contains(&d)
                    {
                        self.show_tab(d - 1, cx);
                    }
                }
            }
            return;
        }
        let key = keys::key_event(ks, ev.is_held);
        if let (Some(key), Some(pane)) = (key, self.panes.get(self.focused)) {
            pane.update(cx, |v, cx| {
                if v.view_top.take().is_some() {
                    cx.notify();
                }
                v.conn.send(&Frame::Key(key));
            });
            cx.stop_propagation();
        }
    }

    fn step_tab(&mut self, by: isize, cx: &mut Context<Self>) {
        let n = self.tabs.len() as isize;
        if n > 0 {
            self.show_tab((self.active as isize + by).rem_euclid(n) as usize, cx);
        }
    }

    /// ⌘J: the next pane that needs you after the focused one, across every tab in tab order.
    fn next_waiting(&mut self, cx: &mut Context<Self>) {
        let order: Vec<(usize, usize)> = self
            .tabs
            .iter()
            .enumerate()
            .flat_map(|(t, ids)| (0..ids.len()).map(move |i| (t, i)))
            .collect();
        let here = order
            .iter()
            .position(|&(t, i)| t == self.active && i == self.focused)
            .unwrap_or(0);
        let next = (1..=order.len())
            .map(|k| order[(here + k) % order.len()])
            .find(|&(t, i)| {
                self.infos
                    .get(&self.tabs[t][i])
                    .is_some_and(chrome::needs_you)
            });
        if let Some((t, i)) = next {
            if t != self.active {
                self.show_tab(t, cx);
            }
            self.set_focus(i, cx);
        }
    }

    fn set_font(&mut self, size: f32, cx: &mut Context<Self>) {
        self.font_size = size.clamp(9.5, 24.5);
        self.metrics = metrics_for(self.font_size, cx);
        let m = self.metrics;
        for p in &self.panes {
            p.update(cx, |v, cx| {
                v.metrics = m;
                v.scale = f32::from(m.font_size) / 12.5;
                cx.notify();
            });
        }
        cx.notify();
    }

    fn pane_cell(&self, i: usize, view: &Entity<PaneView>, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |w, _, _, cx| w.set_focus(i, cx)),
            )
            .on_drop(
                cx.listener(move |w, paths: &gpui::ExternalPaths, _, cx| {
                    w.drop_files(i, paths, cx)
                }),
            )
            .child(AnyView::from(view.clone()).cached(StyleRefinement::default().size_full()))
    }

    /// Files dropped on pane `i` are typed as one paste of their escaped paths, and the pane takes focus.
    fn drop_files(&mut self, i: usize, paths: &gpui::ExternalPaths, cx: &mut Context<Self>) {
        let Some(pane) = self.panes.get(i).cloned() else {
            return;
        };
        let id = pane.read(cx).conn.pane_id;
        let kept: Vec<std::path::PathBuf> = paths
            .paths()
            .iter()
            .map(|p| drop::keep_dropped(p, id))
            .collect();
        if kept.is_empty() {
            return;
        }
        self.set_focus(i, cx);
        pane.update(cx, |v, cx| {
            v.view_top = None;
            v.conn.send(&Frame::Paste(Paste {
                allow_unsafe: false,
                text: drop::typed(&kept),
            }));
            cx.notify();
        });
    }

    fn grid(&self, cx: &mut Context<Self>) -> Div {
        let cells: Vec<Div> = self
            .panes
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.zoomed() || *i == self.focused)
            .map(|(i, v)| self.pane_cell(i, v, cx))
            .collect();
        let sc = Scale(self.font_size / 12.5);
        let row = move |cells: Vec<Div>| {
            div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_row()
                .gap(sc.z(10.0))
                .children(cells)
        };
        let grid = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(sc.z(10.0))
            .pt(sc.z(2.0))
            .pb(sc.z(6.0))
            .px(sc.z(6.0));
        if cells.len() == 4 {
            let mut it = cells.into_iter();
            let top: Vec<Div> = it.by_ref().take(2).collect();
            let bottom: Vec<Div> = it.collect();
            grid.child(row(top)).child(row(bottom))
        } else {
            grid.child(row(cells))
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(s) = &self.stats {
            let mut s = s.borrow_mut();
            s.frame_start = Some(Instant::now());
            s.frames += 1;
        }
        let stats = self.stats.clone();
        let sc = Scale(self.font_size / 12.5);
        let tabs: Vec<Div> = (0..self.tabs.len())
            .map(|i| {
                let name = self
                    .tab_names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("tab {}", i + 1));
                let infos: Vec<&Pane> = self.tabs[i]
                    .iter()
                    .filter_map(|id| self.infos.get(id))
                    .collect();
                chrome::tab(
                    sc,
                    i,
                    &name,
                    i == self.active,
                    chrome::tab_dot(&infos),
                    self.tabs[i].len(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |w, _, _, cx| w.show_tab(i, cx)),
                )
            })
            .collect();
        for (i, pane) in self.panes.iter().enumerate() {
            pane.read(cx)
                .conn
                .set_focus(i == self.focused && self.overlay.is_none());
        }
        let all: Vec<&Pane> = self
            .tabs
            .iter()
            .flatten()
            .filter_map(|id| self.infos.get(id))
            .collect();
        let count = |cli: Cli| all.iter().filter(|p| p.cli == cli).count();
        let counts = [
            (count(Cli::Claude), "claude".to_string()),
            (count(Cli::Codex), "codex".to_string()),
            (count(Cli::Shell), chrome::shell_name()),
        ];
        let waiting = all.iter().filter(|p| chrome::needs_you(p)).count();
        let top_bar = div()
            .flex_none()
            .h(sc.z(40.0))
            .pl(px(84.0))
            .pr(sc.z(16.0))
            .flex()
            .items_center()
            .gap(sc.z(14.0))
            .on_mouse_down(MouseButton::Left, |ev: &gpui::MouseDownEvent, window, _| {
                if ev.click_count == 2 {
                    window.titlebar_double_click()
                } else {
                    window.start_window_move()
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sc.z(8.0))
                    .child(
                        div()
                            .size(sc.z(8.0))
                            .rounded(sc.z(2.0))
                            .bg(chrome::c(chrome::ACCENT))
                            .shadow(vec![chrome::shadow(
                                chrome::ca(chrome::ACCENT, 0.7),
                                0.0,
                                12.0,
                                0.0,
                                sc,
                            )]),
                    )
                    .child(chrome::text(
                        sc,
                        chrome::Type::Wordmark,
                        600.0,
                        chrome::c(chrome::TEXT),
                        false,
                        "ply",
                    )),
            )
            .child(
                div()
                    .p(sc.z(3.0))
                    .rounded(sc.z(11.0))
                    .bg(chrome::white(0.03))
                    .border_1()
                    .border_color(chrome::white(0.06))
                    .flex()
                    .items_center()
                    .gap(sc.z(4.0))
                    .children(tabs),
            )
            .child(div().flex_1())
            .children(self.notice.clone().map(|n| {
                div().min_w(px(0.0)).overflow_hidden().child(chrome::text(
                    sc,
                    chrome::Type::Small,
                    400.0,
                    chrome::c(chrome::AMBER),
                    false,
                    n,
                ))
            }))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(sc.z(6.0))
                    .children(counts.into_iter().enumerate().flat_map(|(i, (n, what))| {
                        let sep = (i > 0).then(|| {
                            chrome::text(
                                sc,
                                chrome::Type::Small,
                                400.0,
                                chrome::c(chrome::HINT),
                                false,
                                "·",
                            )
                        });
                        sep.into_iter().chain([div()
                            .flex()
                            .gap(sc.z(4.0))
                            .child(chrome::text(
                                sc,
                                chrome::Type::Small,
                                600.0,
                                chrome::c(chrome::TEXT),
                                false,
                                n.to_string(),
                            ))
                            .child(chrome::text(
                                sc,
                                chrome::Type::Small,
                                500.0,
                                chrome::c(chrome::TEXT2),
                                false,
                                what,
                            ))])
                    })),
            )
            .when(waiting > 0, |d| {
                d.child(chrome::needs_you_pill(sc, waiting).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|w, _, _, cx| w.next_waiting(cx)),
                ))
            })
            .children(self.account_chip(sc, cx))
            .children(self.codex_chip(sc, cx));
        let overlay = self.render_overlay(sc, cx);
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(chrome::ca(chrome::GLASS.0, chrome::GLASS.1))
            .font_family(chrome::UI)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(gpui::linear_gradient(
                        135.0,
                        gpui::linear_color_stop(chrome::white(0.07), 0.0),
                        gpui::linear_color_stop(chrome::white(0.0), 0.45),
                    )),
            )
            .child(div().absolute().top_0().left_0().w_full().h(px(420.0)).bg(
                gpui::linear_gradient(
                    180.0,
                    gpui::linear_color_stop(chrome::ca(chrome::ACCENT, 0.09), 0.0),
                    gpui::linear_color_stop(chrome::ca(chrome::ACCENT, 0.0), 1.0),
                ),
            ))
            .child(top_bar)
            .child(self.grid(cx))
            .children(overlay)
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, (), _, _| {
                        let now = Instant::now();
                        if let Some(s) = &stats {
                            let mut s = s.borrow_mut();
                            if let Some(start) = s.frame_start.take() {
                                s.frame_ms.push(ms(now - start));
                            }
                            if let Some(prev) = s.last_present.replace(now) {
                                s.intervals.push(ms(now - prev));
                            }
                        }
                    },
                )
                .absolute()
                .size(px(0.0)),
            )
    }
}

/// GPUI's modifiers as libghostty-vt's mouse mods: shift, ctrl, alt, super.
fn mouse_mods(m: &gpui::Modifiers) -> Mods {
    let bits = u16::from(m.shift)
        | u16::from(m.control) << 1
        | u16::from(m.alt) << 2
        | u16::from(m.platform) << 3;
    Mods::from_bits(bits).unwrap_or(Mods::empty())
}

/// R58: columns move left and right only; the four-pane quadrants move within the row and the column; no wrap.
fn neighbour(i: usize, n: usize, dir: &str) -> Option<usize> {
    match (n, dir) {
        (4, "left") if i % 2 == 1 => Some(i - 1),
        (4, "right") if i.is_multiple_of(2) => Some(i + 1),
        (4, "up") if i >= 2 => Some(i - 2),
        (4, "down") if i < 2 => Some(i + 2),
        (2..=3, "left") if i > 0 => Some(i - 1),
        (2..=3, "right") if i + 1 < n => Some(i + 1),
        _ => None,
    }
}

/// Geist Mono's cell at `size` points: the advance of `m`, and ascent + descent rounded to whole pixels.
fn metrics_for(size: f32, cx: &App) -> Metrics {
    let font = Font {
        family: SharedString::new_static(FONT_FAMILY),
        features: FontFeatures::disable_ligatures(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    };
    let font_size = px(size);
    let id = cx.text_system().resolve_font(&font);
    let cell_w = cx
        .text_system()
        .advance(id, font_size, 'm')
        .map_or(px(size * 0.6), |s| s.width);
    Metrics {
        font_size,
        cell_w,
        cell_h: px((size * 1.3).round()),
    }
}

fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let dir = match std::env::var_os("PLY_HOME") {
        Some(home) => std::path::PathBuf::from(home).join("logs"),
        None => std::path::PathBuf::from(std::env::var_os("HOME")?).join("Library/Logs/ply"),
    };
    let (writer, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(dir, "app"));
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Some(guard)
}

mod menu {
    gpui::actions!(ply, [Quit, Hide, HideOthers, Minimize, CloseWindow]);
}
use menu::{CloseWindow, Hide, HideOthers, Minimize, Quit};

fn main() {
    let args = parse_args();
    let _log = init_logging();
    let home = launcher::data_dir();
    let sock = home.join("run").join("plyd.sock");
    if let Err(e) = launcher::ensure_plyd(&sock) {
        tracing::error!(error = %e, "plyd is not available");
    }
    let font_size = control::request::<ply_proto::pane::Settings>(
        &sock,
        ply_proto::control::Call::SettingsGet(Default::default()),
    )
    .map_or(12.5, |s| s.font_size);
    Application::new().run(move |cx: &mut App| {
        let fonts: Vec<Cow<'static, [u8]>> = vec![
            Cow::Borrowed(include_bytes!(
                "../../../app/assets/fonts/GeistMono-Regular.ttf"
            )),
            Cow::Borrowed(include_bytes!(
                "../../../app/assets/fonts/GeistMono-Medium.ttf"
            )),
            Cow::Borrowed(include_bytes!(
                "../../../app/assets/fonts/GeistMono-SemiBold.ttf"
            )),
            Cow::Borrowed(include_bytes!(
                "../../../app/assets/fonts/Geist-Regular.ttf"
            )),
            Cow::Borrowed(include_bytes!("../../../app/assets/fonts/Geist-Medium.ttf")),
            Cow::Borrowed(include_bytes!(
                "../../../app/assets/fonts/Geist-SemiBold.ttf"
            )),
        ];
        if let Err(e) = cx.text_system().add_fonts(fonts) {
            tracing::warn!(error = %e, "loading Geist failed; the system fonts stand in");
        }
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.bind_keys([
            gpui::KeyBinding::new("cmd-q", Quit, None),
            gpui::KeyBinding::new("cmd-h", Hide, None),
            gpui::KeyBinding::new("alt-cmd-h", HideOthers, None),
            gpui::KeyBinding::new("cmd-m", Minimize, None),
            gpui::KeyBinding::new("cmd-w", CloseWindow, None),
        ]);
        cx.set_menus(vec![
            gpui::Menu {
                name: "ply".into(),
                items: vec![
                    gpui::MenuItem::action("Hide ply", Hide),
                    gpui::MenuItem::action("Hide Others", HideOthers),
                    gpui::MenuItem::separator(),
                    gpui::MenuItem::action("Quit ply", Quit),
                ],
            },
            gpui::Menu {
                name: "Window".into(),
                items: vec![
                    gpui::MenuItem::action("Minimize", Minimize),
                    gpui::MenuItem::action("Close Window", CloseWindow),
                ],
            },
        ]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let metrics = metrics_for(font_size, cx);
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        let zoom = std::env::var("PLY_WINDOW_ZOOM").as_deref() != Ok("0");
        let focus = std::env::var("PLY_WINDOW_FOCUS").as_deref() != Ok("0");
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(if zoom {
                    WindowBounds::Maximized(bounds)
                } else {
                    WindowBounds::Windowed(bounds)
                }),
                titlebar: Some(TitlebarOptions {
                    title: Some("ply".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(gpui::point(px(16.0), px(13.0))),
                }),
                window_background: gpui::WindowBackgroundAppearance::Blurred,
                focus,
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Workspace::new(&args, &home, metrics, window, cx)),
        );
        if let Err(e) = opened {
            tracing::error!(error = %e, "cannot open the window");
            cx.quit();
            return;
        }
        if focus {
            cx.activate(true);
        }
    });
}
