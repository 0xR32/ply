//! ply's window chrome in GPUI, from the app's tokens and components (top bar, tab bar, pane frame and header).

use gpui::{
    BoxShadow, Div, FontWeight, Hsla, Pixels, SharedString, div, point, prelude::*, px, rgb,
};
use ply_proto::pane::{Cli, Pane, PaneStatus};

pub const UI: &str = "Geist";
pub const MONO: &str = "Geist Mono";

pub const GLASS: (u32, f32) = (0x0A0B10, 0.62);
pub const PANE: u32 = 0x11131A;
pub const PANE_FOCUS: u32 = 0x141722;
pub const TERM: u32 = 0x0C0E14;
pub const TEXT: u32 = 0xE6E8EF;
pub const TEXT_SOFT: u32 = 0xC3C7D2;
pub const TEXT2: u32 = 0xA4A9B8;
pub const TEXT3: u32 = 0x8D93A4;
pub const HINT: u32 = 0x7C8294;
pub const ACCENT: u32 = 0x8AB4FF;
pub const AMBER: u32 = 0xF2B35B;
pub const MINT: u32 = 0x7FD4B0;
pub const RED: u32 = 0xF07A6A;
pub const TAB_ACTIVE: u32 = 0x1A1E2A;
pub const DOT_IDLE: u32 = 0x5C6273;
pub const SEGMENT_ACTIVE: u32 = 0x1E2330;

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn ca(hex: u32, alpha: f32) -> Hsla {
    c(hex).opacity(alpha)
}

pub fn white(alpha: f32) -> Hsla {
    ca(0xFFFFFF, alpha)
}

/// The chrome's scale, `font_size / 12.5`; `z` rounds a scaled length to half pixels as the app's `z()` does.
#[derive(Clone, Copy)]
pub struct Scale(pub f32);

impl Scale {
    pub fn z(self, x: f32) -> Pixels {
        px((x * self.0 * 2.0).round() / 2.0)
    }
}

pub enum Type {
    Wordmark,
    Body,
    Small,
    Label,
    Key,
}

/// One line of chrome text in the type scale.
pub fn text(
    s: Scale,
    ty: Type,
    weight: f32,
    color: Hsla,
    mono: bool,
    content: impl Into<SharedString>,
) -> Div {
    let (size, line) = match ty {
        Type::Wordmark => (15.0, 20.0),
        Type::Body => (13.0, 20.0),
        Type::Small => (12.0, 16.0),
        Type::Label => (11.0, 14.0),
        Type::Key => (10.5, 14.0),
    };
    div()
        .font_family(if mono { MONO } else { UI })
        .text_size(s.z(size))
        .line_height(s.z(line))
        .font_weight(FontWeight(weight))
        .text_color(color)
        .whitespace_nowrap()
        .child(content.into())
}

pub fn dot(s: Scale, size: f32, color: Hsla) -> Div {
    div()
        .flex_none()
        .size(s.z(size))
        .rounded(s.z(size / 2.0))
        .bg(color)
}

/// A key cap: `fg` text on `bg` with a `ring`.
pub fn kbd(
    s: Scale,
    label: impl Into<SharedString>,
    h: f32,
    radius: f32,
    (fg, bg, ring): (Hsla, Hsla, Hsla),
    weight: f32,
) -> Div {
    div()
        .flex_none()
        .h(s.z(h))
        .min_w(s.z(h))
        .px(s.z(4.0))
        .rounded(s.z(radius))
        .bg(bg)
        .border_1()
        .border_color(ring)
        .flex()
        .items_center()
        .justify_center()
        .child(text(
            s,
            if h <= 18.0 { Type::Key } else { Type::Label },
            weight,
            fg,
            true,
            label,
        ))
}

pub fn shadow(color: Hsla, y: f32, blur: f32, spread: f32, s: Scale) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(px(0.0), s.z(y)),
        blur_radius: s.z(blur),
        spread_radius: s.z(spread),
    }
}

pub fn needs_you(p: &Pane) -> bool {
    matches!(
        p.status,
        PaneStatus::WaitingPermission | PaneStatus::WaitingInput
    )
}

fn plan_done(p: &Pane) -> bool {
    p.status == PaneStatus::Idle
        && p.progress
            .as_ref()
            .is_some_and(|g| g.total > 0 && g.done >= g.total)
}

/// The shell's name for a shell pane's chip, from the login shell plyd starts.
pub fn shell_name() -> String {
    std::env::var("SHELL")
        .ok()
        .and_then(|s| s.rsplit('/').next().map(str::to_string))
        .unwrap_or_else(|| "sh".into())
}

/// The status chip: label, text colour and fill, as the app's header shows it.
pub fn status_chip(s: Scale, p: &Pane, shell: &str) -> Div {
    let (label, fg, bg) = match p.status {
        PaneStatus::Starting => ("Starting".to_string(), c(TEXT2), white(0.05)),
        PaneStatus::Running if p.cli != Cli::Shell => {
            ("Running".into(), c(ACCENT), ca(ACCENT, 0.10))
        }
        PaneStatus::WaitingPermission | PaneStatus::WaitingInput => {
            ("Needs you".into(), c(AMBER), ca(AMBER, 0.12))
        }
        PaneStatus::Idle if plan_done(p) => ("Done".into(), c(MINT), ca(MINT, 0.10)),
        PaneStatus::Exited => {
            let code = p.exit_code.unwrap_or(0);
            let (fg, bg) = if code == 0 {
                (c(TEXT3), white(0.04))
            } else {
                (c(RED), ca(RED, 0.12))
            };
            (format!("Exited {code}"), fg, bg)
        }
        PaneStatus::Lost => ("Lost".into(), c(TEXT2), white(0.05)),
        _ if p.cli == Cli::Shell => (shell.to_string(), c(TEXT3), white(0.04)),
        _ => ("Your turn".into(), c(TEXT_SOFT), white(0.05)),
    };
    div()
        .flex_none()
        .h(s.z(22.0))
        .pl(s.z(8.0))
        .pr(s.z(9.0))
        .rounded(s.z(11.0))
        .bg(bg)
        .flex()
        .items_center()
        .gap(s.z(7.0))
        .child(dot(s, 6.0, fg))
        .child(text(s, Type::Small, 500.0, fg, false, label))
}

/// The tab's dot: amber if a pane needs you, accent if one runs, mint if all are done or shells, else grey.
pub fn tab_dot(panes: &[&Pane]) -> u32 {
    if panes.iter().any(|p| needs_you(p)) {
        AMBER
    } else if panes
        .iter()
        .any(|p| p.status == PaneStatus::Running && p.cli != Cli::Shell)
    {
        ACCENT
    } else if !panes.is_empty() && panes.iter().all(|p| plan_done(p) || p.cli == Cli::Shell) {
        MINT
    } else {
        DOT_IDLE
    }
}

/// The pane header: position key, project, branch, plan progress, CLI and model chip, status chip.
pub fn pane_header(
    s: Scale,
    position: usize,
    p: Option<&Pane>,
    title: &str,
    focused: bool,
    scrolled: Option<u64>,
    shell: &str,
) -> Div {
    let waiting = p.is_some_and(needs_you);
    let (kfg, kbg, kring) = if focused {
        (c(ACCENT), ca(ACCENT, 0.14), ca(ACCENT, 0.45))
    } else if waiting {
        (c(AMBER), white(0.03), white(0.10))
    } else {
        (c(TEXT2), white(0.03), white(0.10))
    };
    let project = p
        .and_then(|p| p.project.clone())
        .unwrap_or_else(|| title.to_string());
    let branch = p.and_then(|p| {
        let tree = p.git_worktree.clone().or_else(|| p.worktree_seen.clone());
        match (&p.branch, tree) {
            (Some(b), Some(t)) => Some(format!("{b} · worktree {t}")),
            (Some(b), None) => Some(b.clone()),
            (None, Some(t)) => Some(format!("worktree {t}")),
            (None, None) => None,
        }
    });
    let progress = p.and_then(|p| {
        p.progress
            .as_ref()
            .filter(|g| g.total > 0)
            .map(|g| (g.done, g.total))
    });
    let fill = if waiting {
        AMBER
    } else if p.is_some_and(plan_done) {
        MINT
    } else {
        ACCENT
    };
    let agent = p.filter(|p| p.cli != Cli::Shell);
    div()
        .flex_none()
        .h(s.z(42.0))
        .pl(s.z(8.0))
        .pr(s.z(10.0))
        .flex()
        .items_center()
        .gap(s.z(10.0))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .flex()
                .items_center()
                .gap(s.z(10.0))
                .child(kbd(
                    s,
                    (position + 1).to_string(),
                    20.0,
                    6.0,
                    (kfg, kbg, kring),
                    500.0,
                ))
                .child(
                    div()
                        .min_w(px(0.0))
                        .max_w(s.z(200.0))
                        .overflow_hidden()
                        .child(text(s, Type::Body, 600.0, c(TEXT), false, project)),
                )
                .children(branch.map(|b| {
                    div().min_w(px(0.0)).overflow_hidden().child(text(
                        s,
                        Type::Label,
                        400.0,
                        c(HINT),
                        true,
                        b,
                    ))
                }))
                .children(scrolled.map(|n| {
                    text(
                        s,
                        Type::Label,
                        500.0,
                        c(AMBER),
                        true,
                        format!("↑ {n} lines"),
                    )
                })),
        )
        .children(progress.map(|(done, total)| {
            let w = 56.0;
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(s.z(6.0))
                .child(
                    div()
                        .w(s.z(w))
                        .h(s.z(4.0))
                        .rounded(s.z(2.0))
                        .bg(white(0.07))
                        .child(
                            div()
                                .h_full()
                                .w(s.z(w * done.min(total) as f32 / total as f32))
                                .rounded(s.z(2.0))
                                .bg(c(fill)),
                        ),
                )
                .child(text(
                    s,
                    Type::Label,
                    400.0,
                    c(HINT),
                    true,
                    format!("{done}/{total}"),
                ))
        }))
        .children(agent.map(|p| {
            let cli = if p.cli == Cli::Claude {
                "claude"
            } else {
                "codex"
            };
            div()
                .flex_none()
                .h(s.z(22.0))
                .px(s.z(8.0))
                .rounded(s.z(6.0))
                .bg(white(0.04))
                .flex()
                .items_center()
                .gap(s.z(6.0))
                .child(text(s, Type::Label, 400.0, c(TEXT), true, cli))
                .children(
                    p.model_seen
                        .clone()
                        .map(|m| text(s, Type::Label, 400.0, c(TEXT2), true, m)),
                )
        }))
        .children(p.map(|p| status_chip(s, p, shell)))
}

/// The pane's frame: ring, fill and shadow for focused, waiting and other panes.
pub fn pane_frame(s: Scale, focused: bool, waiting: bool) -> Div {
    let frame = div()
        .size_full()
        .flex()
        .flex_col()
        .rounded(s.z(12.0))
        .overflow_hidden()
        .border_1();
    if focused {
        frame
            .bg(c(PANE_FOCUS))
            .border_color(ca(ACCENT, 0.55))
            .shadow(vec![shadow(ca(ACCENT, 0.35), 18.0, 48.0, -18.0, s)])
    } else if waiting {
        frame
            .bg(c(PANE))
            .border_color(ca(AMBER, 0.35))
            .shadow(vec![shadow(ca(AMBER, 0.25), 18.0, 48.0, -22.0, s)])
    } else {
        frame.bg(c(PANE)).border_color(white(0.06))
    }
}

/// The inset box the terminal draws in.
pub fn term_body(s: Scale) -> Div {
    div()
        .flex_1()
        .min_h(px(0.0))
        .ml(s.z(6.0))
        .mr(s.z(6.0))
        .mb(s.z(6.0))
        .pt(s.z(12.0))
        .pb(s.z(12.0))
        .pl(s.z(14.0))
        .pr(s.z(14.0))
        .rounded(s.z(8.0))
        .bg(c(TERM))
        .border_1()
        .border_color(white(0.04))
}

/// One tab pill: number key, name, status dot, pane count.
pub fn tab(s: Scale, index: usize, name: &str, active: bool, dot_color: u32, panes: usize) -> Div {
    let pill = div()
        .flex_none()
        .h(s.z(26.0))
        .pl(s.z(6.0))
        .pr(s.z(10.0))
        .rounded(s.z(8.0))
        .flex()
        .items_center()
        .gap(s.z(8.0))
        .border_1();
    let pill = if active {
        pill.bg(c(TAB_ACTIVE))
            .border_color(white(0.08))
            .shadow(vec![shadow(ca(0x000000, 0.6), 4.0, 14.0, -6.0, s)])
    } else {
        pill.border_color(ca(0xFFFFFF, 0.0))
    };
    let (kfg, kbg, kring) = if active {
        (c(ACCENT), ca(ACCENT, 0.12), ca(ACCENT, 0.40))
    } else {
        (c(TEXT3), white(0.02), white(0.10))
    };
    pill.when(index < 9, |d| {
        d.child(kbd(
            s,
            (index + 1).to_string(),
            18.0,
            5.0,
            (kfg, kbg, kring),
            400.0,
        ))
    })
    .child(text(
        s,
        Type::Body,
        if active { 500.0 } else { 400.0 },
        c(if active { TEXT } else { TEXT2 }),
        false,
        name.to_string(),
    ))
    .child(dot(s, 6.0, c(dot_color)))
    .child(text(
        s,
        Type::Label,
        400.0,
        c(HINT),
        true,
        panes.to_string(),
    ))
}

/// The amber "N need you" pill with its ⌘J hint.
pub fn needs_you_pill(s: Scale, count: usize) -> Div {
    let words = if count == 1 {
        "1 needs you".to_string()
    } else {
        format!("{count} need you")
    };
    div()
        .flex_none()
        .h(s.z(26.0))
        .pl(s.z(10.0))
        .pr(s.z(12.0))
        .rounded(s.z(13.0))
        .bg(ca(AMBER, 0.10))
        .border_1()
        .border_color(ca(AMBER, 0.35))
        .flex()
        .items_center()
        .gap(s.z(8.0))
        .child(dot(s, 7.0, c(AMBER)))
        .child(text(s, Type::Body, 500.0, c(AMBER), false, words))
        .child(text(s, Type::Label, 400.0, ca(AMBER, 0.75), true, "⌘J"))
}
