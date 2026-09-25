//! The terminal's effect callbacks and the state they write into.
//!
//! libghostty-vt runs every callback synchronously inside `vt_write`, `resize` or `paste` on the calling thread
//! (`terminal.h`, "Effects"). Each callback receives the [`Effects`] pointer installed as `OPT_USERDATA`; the
//! [`super::Engine`] owns that allocation for the terminal's whole life and never holds a Rust reference to it while
//! a libghostty-vt call runs, so a callback's `&mut Effects` is the only live reference. Callbacks never block and
//! never call back into the terminal except for read-only `terminal_get`.

use std::ffi::c_void;
use std::ptr;

use ghostty_sys as sys;

use super::{
    ClipboardContent, ClipboardWrite, Notification, ProgressReport, ProgressState, XTVERSION,
};

/// What the callbacks collected since the engine last drained them.
#[derive(Debug, Default)]
pub(super) struct Effects {
    pub(super) reply: Vec<u8>,
    pub(super) bells: u32,
    pub(super) title_changed: bool,
    pub(super) pwd_changed: bool,
    pub(super) notifications: Vec<Notification>,
    pub(super) progress: Option<ProgressReport>,
    pub(super) clipboard_writes: Vec<ClipboardWrite>,
    pub(super) size: sys::GhosttySizeReportSize,
    pub(super) dark: bool,
}

/// Installs ply's callbacks with `effects` as userdata; safety: `term` is live and `effects` stays valid and unborrowed during library calls until `term` is freed.
pub(super) unsafe fn install(
    term: sys::GhosttyTerminal,
    effects: *mut Effects,
) -> Result<(), (&'static str, i32)> {
    let callbacks: [(sys::GhosttyTerminalOption, *const c_void); 11] = [
        (sys::GHOSTTY_TERMINAL_OPT_USERDATA, effects.cast()),
        (
            sys::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
            on_write_pty as sys::GhosttyTerminalWritePtyFn as *const c_void,
        ),
        (
            sys::GHOSTTY_TERMINAL_OPT_BELL,
            on_bell as sys::GhosttyTerminalBellFn as *const c_void,
        ),
        (
            sys::GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
            on_title as sys::GhosttyTerminalTitleChangedFn as *const c_void,
        ),
        (
            sys::GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
            on_pwd as sys::GhosttyTerminalPwdChangedFn as *const c_void,
        ),
        (
            sys::GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION,
            on_notification as sys::GhosttyTerminalDesktopNotificationFn as *const c_void,
        ),
        (
            sys::GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT,
            on_progress as sys::GhosttyTerminalProgressReportFn as *const c_void,
        ),
        (
            sys::GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
            on_clipboard_write as sys::GhosttyTerminalClipboardWriteFn as *const c_void,
        ),
        (
            sys::GHOSTTY_TERMINAL_OPT_SIZE,
            on_size as sys::GhosttyTerminalSizeFn as *const c_void,
        ),
        (
            sys::GHOSTTY_TERMINAL_OPT_COLOR_SCHEME,
            on_color_scheme as sys::GhosttyTerminalColorSchemeFn as *const c_void,
        ),
        (
            sys::GHOSTTY_TERMINAL_OPT_XTVERSION,
            on_xtversion as sys::GhosttyTerminalXtversionFn as *const c_void,
        ),
    ];
    for (option, value) in callbacks {
        // SAFETY: the caller guarantees `term` is live; each value has the type its option documents.
        let code = unsafe { sys::ghostty_terminal_set(term, option, value) };
        if code != sys::GHOSTTY_SUCCESS {
            return Err(("ghostty_terminal_set(callback)", code));
        }
    }
    Ok(())
}

/// Safety: `userdata` is the pointer [`install`] set, and no other reference to it is live.
unsafe fn effects<'a>(userdata: *mut c_void) -> &'a mut Effects {
    // SAFETY: guaranteed by the caller (see the module docs).
    unsafe { &mut *userdata.cast::<Effects>() }
}

/// Safety: `s` describes memory valid for the current callback.
unsafe fn bytes<'a>(s: &sys::GhosttyString) -> &'a [u8] {
    if s.ptr.is_null() || s.len == 0 {
        &[]
    } else {
        // SAFETY: guaranteed by the caller.
        unsafe { std::slice::from_raw_parts(s.ptr, s.len) }
    }
}

unsafe extern "C" fn on_write_pty(
    _t: sys::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if data.is_null() || len == 0 {
        return;
    }
    // SAFETY: libghostty-vt passes our userdata and `len` readable bytes at `data` for this call.
    unsafe {
        effects(userdata)
            .reply
            .extend_from_slice(std::slice::from_raw_parts(data, len))
    };
}

unsafe extern "C" fn on_bell(_t: sys::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: libghostty-vt passes our userdata.
    let fx = unsafe { effects(userdata) };
    fx.bells = fx.bells.saturating_add(1);
}

unsafe extern "C" fn on_title(_t: sys::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: libghostty-vt passes our userdata.
    unsafe { effects(userdata).title_changed = true };
}

unsafe extern "C" fn on_pwd(_t: sys::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: libghostty-vt passes our userdata.
    unsafe { effects(userdata).pwd_changed = true };
}

unsafe extern "C" fn on_notification(
    _t: sys::GhosttyTerminal,
    userdata: *mut c_void,
    notification: *const sys::GhosttyTerminalDesktopNotification,
) {
    if notification.is_null() {
        return;
    }
    // SAFETY: libghostty-vt passes our userdata and a notification borrowed for this call.
    let (fx, n) = unsafe { (effects(userdata), &*notification) };
    // SAFETY: the strings are borrowed for this call.
    let (title, body) = unsafe { (bytes(&n.title), bytes(&n.body)) };
    fx.notifications.push(Notification {
        title: String::from_utf8_lossy(title).into_owned(),
        body: String::from_utf8_lossy(body).into_owned(),
    });
}

unsafe extern "C" fn on_progress(
    _t: sys::GhosttyTerminal,
    userdata: *mut c_void,
    report: *const sys::GhosttyTerminalProgressReport,
) {
    if report.is_null() {
        return;
    }
    // SAFETY: libghostty-vt passes our userdata and a report borrowed for this call.
    let (fx, r) = unsafe { (effects(userdata), &*report) };
    let state = match r.state {
        0 => ProgressState::Remove,
        1 => ProgressState::Set,
        2 => ProgressState::Error,
        3 => ProgressState::Indeterminate,
        4 => ProgressState::Pause,
        other => {
            tracing::debug!(
                state = other,
                "libghostty-vt reported an unknown OSC 9;4 state"
            );
            return;
        }
    };
    fx.progress = Some(ProgressReport {
        state,
        percent: u8::try_from(r.progress).ok().filter(|p| *p <= 100),
    });
}

unsafe extern "C" fn on_clipboard_write(
    _t: sys::GhosttyTerminal,
    userdata: *mut c_void,
    write: *const sys::GhosttyClipboardWrite,
) {
    if write.is_null() {
        return;
    }
    // SAFETY: libghostty-vt passes our userdata and a request borrowed for this call.
    let (fx, w) = unsafe { (effects(userdata), &*write) };
    let mut contents = Vec::with_capacity(w.contents_len);
    for i in 0..w.contents_len {
        // SAFETY: `contents` holds `contents_len` entries borrowed for this call.
        let part = unsafe { &*w.contents.add(i) };
        // SAFETY: the strings are borrowed for this call.
        let (mime, data) = unsafe { (bytes(&part.mime), bytes(&part.data)) };
        contents.push(ClipboardContent {
            mime: String::from_utf8_lossy(mime).into_owned(),
            data: data.to_vec(),
        });
    }
    fx.clipboard_writes.push(ClipboardWrite { contents });
    let reply = sys::GhosttyClipboardWriteReply {
        size: size_of::<sys::GhosttyClipboardWriteReply>(),
        result: sys::GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS,
        remember: false,
    };
    if let Some(answer) = w.reply {
        // SAFETY: the reply function must be called inside this callback with the request it came with.
        unsafe { answer(write, ptr::from_ref(&reply)) };
    }
}

unsafe extern "C" fn on_size(
    _t: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut sys::GhosttySizeReportSize,
) -> bool {
    // SAFETY: libghostty-vt passes our userdata.
    let size = unsafe { effects(userdata).size };
    if out.is_null() || size.cell_width == 0 || size.cell_height == 0 {
        return false;
    }
    // SAFETY: `out` is a writable size struct for this call.
    unsafe { out.write(size) };
    true
}

unsafe extern "C" fn on_color_scheme(
    _t: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut sys::GhosttyColorScheme,
) -> bool {
    if out.is_null() {
        return false;
    }
    // SAFETY: libghostty-vt passes our userdata and a writable scheme for this call.
    unsafe {
        out.write(if effects(userdata).dark {
            sys::GHOSTTY_COLOR_SCHEME_DARK
        } else {
            sys::GHOSTTY_COLOR_SCHEME_LIGHT
        });
    }
    true
}

unsafe extern "C" fn on_xtversion(
    _t: sys::GhosttyTerminal,
    _userdata: *mut c_void,
) -> sys::GhosttyString {
    sys::GhosttyString {
        ptr: XTVERSION.as_ptr(),
        len: XTVERSION.len(),
    }
}
