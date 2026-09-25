//! libghostty-vt's key and mouse encoders, focus encoding and terminal paste, behind safe calls.
//!
//! Each encode first copies the pane's live modes from the terminal (`setopt_from_terminal`), so the bytes always
//! match what the program asked for (spec R-R5, R-R9). Key encoding re-applies option-as-alt after that copy,
//! because the copy resets it (`key/encoder.h`, ADR-0005).
//!
//! libghostty-vt's ReleaseFast build does not range-check enum arguments or float-to-int conversions, so an
//! undefined key, action or button, a non-finite or huge pointer position, or a zero cell size would be undefined
//! behaviour inside the library. Every encode here refuses such values (encoding nothing) before the unsafe call;
//! `crate::input` rejects them earlier with a log line, so these checks are the last line of defence.

use std::ffi::{c_char, c_void};
use std::ptr;

use ghostty_sys as sys;

use super::check;
use crate::error::Result;

/// The modifier bits libghostty-vt defines (`key/event.h`); bits 10–15 are padding it assumes zero.
const MODS_DEFINED: sys::GhosttyMods = 0x03FF;

/// Largest pointer coordinate, in pixels, passed to the mouse encoder, so its `i32` pixel and cell math cannot overflow.
const POSITION_LIMIT: f32 = 1.0e9;

/// A key encoder and a reusable key event.
pub(super) struct KeyEncoder {
    encoder: sys::GhosttyKeyEncoder,
    event: sys::GhosttyKeyEvent,
}

/// The parameters of one key event, numbered as libghostty-vt's.
pub(crate) struct KeyInput<'a> {
    pub(crate) key: sys::GhosttyKey,
    pub(crate) mods: sys::GhosttyMods,
    pub(crate) consumed_mods: sys::GhosttyMods,
    pub(crate) action: sys::GhosttyKeyAction,
    pub(crate) composing: bool,
    pub(crate) unshifted_codepoint: u32,
    pub(crate) text: &'a str,
}

impl KeyEncoder {
    pub(super) fn new() -> Result<Self> {
        let mut this = Self {
            encoder: ptr::null_mut(),
            event: ptr::null_mut(),
        };
        // SAFETY: the out pointers receive new handles; `Drop` frees any that were created.
        unsafe {
            check(
                "ghostty_key_encoder_new",
                sys::ghostty_key_encoder_new(ptr::null(), &raw mut this.encoder),
            )?;
            check(
                "ghostty_key_event_new",
                sys::ghostty_key_event_new(ptr::null(), &raw mut this.event),
            )?;
        }
        Ok(this)
    }

    /// Encodes `input` against `term`'s live modes with `option_as_alt` applied.
    pub(super) fn encode(
        &mut self,
        term: sys::GhosttyTerminal,
        option_as_alt: sys::GhosttyOptionAsAlt,
        input: &KeyInput<'_>,
    ) -> Result<Vec<u8>> {
        if !(0..=sys::GHOSTTY_KEY_MAX).contains(&input.key) || !(0..=2).contains(&input.action) {
            return Ok(Vec::new());
        }
        // SAFETY: live handles, serialized `term`; key and action range-checked above, mods masked, so every enum argument is defined; `input.text` outlives the call.
        unsafe {
            sys::ghostty_key_encoder_setopt_from_terminal(self.encoder, term);
            sys::ghostty_key_encoder_setopt(
                self.encoder,
                sys::GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
                (&raw const option_as_alt).cast::<c_void>(),
            );
            sys::ghostty_key_event_set_key(self.event, input.key);
            sys::ghostty_key_event_set_mods(self.event, input.mods & MODS_DEFINED);
            sys::ghostty_key_event_set_consumed_mods(
                self.event,
                input.consumed_mods & MODS_DEFINED,
            );
            sys::ghostty_key_event_set_action(self.event, input.action);
            sys::ghostty_key_event_set_composing(self.event, input.composing);
            sys::ghostty_key_event_set_unshifted_codepoint(self.event, input.unshifted_codepoint);
            sys::ghostty_key_event_set_utf8(
                self.event,
                input.text.as_ptr().cast::<c_char>(),
                input.text.len(),
            );
        }
        encode_with("ghostty_key_encoder_encode", |buf, cap, len| {
            // SAFETY: `buf` has `cap` writable bytes (or is null with `cap` 0 for a size query).
            unsafe { sys::ghostty_key_encoder_encode(self.encoder, self.event, buf, cap, len) }
        })
    }
}

impl Drop for KeyEncoder {
    fn drop(&mut self) {
        // SAFETY: each handle is null or owned by this value; the free functions accept null.
        unsafe {
            sys::ghostty_key_event_free(self.event);
            sys::ghostty_key_encoder_free(self.encoder);
        }
    }
}

/// A mouse encoder and a reusable mouse event.
pub(super) struct MouseEncoder {
    encoder: sys::GhosttyMouseEncoder,
    event: sys::GhosttyMouseEvent,
}

/// The parameters of one mouse event, numbered as libghostty-vt's.
pub(crate) struct MouseInput {
    pub(crate) action: sys::GhosttyMouseAction,
    pub(crate) button: Option<sys::GhosttyMouseButton>,
    pub(crate) mods: sys::GhosttyMods,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) any_pressed: bool,
}

/// Surface geometry in pixels: grid size times cell size, no padding.
#[derive(Debug, Clone, Copy)]
pub(super) struct Surface {
    pub(super) cols: u16,
    pub(super) rows: u16,
    pub(super) cell_width: u16,
    pub(super) cell_height: u16,
}

impl MouseEncoder {
    pub(super) fn new() -> Result<Self> {
        let mut this = Self {
            encoder: ptr::null_mut(),
            event: ptr::null_mut(),
        };
        // SAFETY: the out pointers receive new handles; `Drop` frees any that were created.
        unsafe {
            check(
                "ghostty_mouse_encoder_new",
                sys::ghostty_mouse_encoder_new(ptr::null(), &raw mut this.encoder),
            )?;
            check(
                "ghostty_mouse_event_new",
                sys::ghostty_mouse_event_new(ptr::null(), &raw mut this.event),
            )?;
        }
        Ok(this)
    }

    /// Encodes `input` against `term`'s live tracking mode and format; empty when the mode reports nothing.
    pub(super) fn encode(
        &mut self,
        term: sys::GhosttyTerminal,
        surface: Surface,
        input: &MouseInput,
    ) -> Result<Vec<u8>> {
        let button_ok = input.button.is_none_or(|b| (1..=11).contains(&b));
        if surface.cell_width == 0
            || surface.cell_height == 0
            || !(0..=2).contains(&input.action)
            || !button_ok
            || !input.x.is_finite()
            || !input.y.is_finite()
        {
            return Ok(Vec::new());
        }
        let width = u32::from(surface.cols) * u32::from(surface.cell_width);
        let height = u32::from(surface.rows) * u32::from(surface.cell_height);
        let clamp = |v: f32, extent: u32| {
            let extent = (extent as f32).min(POSITION_LIMIT);
            v.clamp(-extent, 2.0 * extent)
        };
        let position = sys::GhosttyMousePosition {
            x: clamp(input.x, width),
            y: clamp(input.y, height),
        };
        let size = sys::GhosttyMouseEncoderSize {
            size: size_of::<sys::GhosttyMouseEncoderSize>(),
            screen_width: width,
            screen_height: height,
            cell_width: u32::from(surface.cell_width),
            cell_height: u32::from(surface.cell_height),
            ..Default::default()
        };
        let track_last_cell = true;
        // SAFETY: live handles, serialized `term`, documented option types; action/button defined, cells non-zero, position finite and clamped above, so no Zig conversion overflows.
        unsafe {
            sys::ghostty_mouse_encoder_setopt_from_terminal(self.encoder, term);
            sys::ghostty_mouse_encoder_setopt(
                self.encoder,
                sys::GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                (&raw const size).cast(),
            );
            sys::ghostty_mouse_encoder_setopt(
                self.encoder,
                sys::GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                (&raw const input.any_pressed).cast(),
            );
            sys::ghostty_mouse_encoder_setopt(
                self.encoder,
                sys::GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL,
                (&raw const track_last_cell).cast(),
            );
            sys::ghostty_mouse_event_set_action(self.event, input.action);
            match input.button {
                Some(button) => sys::ghostty_mouse_event_set_button(self.event, button),
                None => sys::ghostty_mouse_event_clear_button(self.event),
            }
            sys::ghostty_mouse_event_set_mods(self.event, input.mods & MODS_DEFINED);
            sys::ghostty_mouse_event_set_position(self.event, position);
        }
        encode_with("ghostty_mouse_encoder_encode", |buf, cap, len| {
            // SAFETY: `buf` has `cap` writable bytes (or is null with `cap` 0 for a size query).
            unsafe { sys::ghostty_mouse_encoder_encode(self.encoder, self.event, buf, cap, len) }
        })
    }
}

impl MouseEncoder {
    /// Forgets the last reported cell, so the next motion is reported even inside it.
    pub(super) fn reset(&mut self) {
        // SAFETY: the encoder handle is live and owned by this value.
        unsafe { sys::ghostty_mouse_encoder_reset(self.encoder) };
    }
}

impl Drop for MouseEncoder {
    fn drop(&mut self) {
        // SAFETY: each handle is null or owned by this value; the free functions accept null.
        unsafe {
            sys::ghostty_mouse_event_free(self.event);
            sys::ghostty_mouse_encoder_free(self.encoder);
        }
    }
}

/// `CSI I` or `CSI O`.
pub(super) fn focus(gained: bool) -> Result<Vec<u8>> {
    let event = if gained {
        sys::GHOSTTY_FOCUS_GAINED
    } else {
        sys::GHOSTTY_FOCUS_LOST
    };
    encode_with("ghostty_focus_encode", |buf, cap, len| {
        // SAFETY: `buf` has `cap` writable bytes (or is null with `cap` 0 for a size query).
        unsafe { sys::ghostty_focus_encode(event, buf, cap, len) }
    })
}

/// What `ghostty_terminal_paste` did; its bytes went through the write-pty callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PasteResult {
    Written,
    Empty,
    Rejected,
}

/// Pastes `text` as a user clipboard paste; its encoded bytes arrive through the write-pty callback.
pub(super) fn paste(
    term: sys::GhosttyTerminal,
    text: &str,
    allow_unsafe: bool,
) -> Result<PasteResult> {
    let mime = sys::GhosttyString {
        ptr: b"text/plain".as_ptr(),
        len: b"text/plain".len(),
    };
    let mut source = text.as_bytes();
    let request = sys::GhosttyPaste {
        size: size_of::<sys::GhosttyPaste>(),
        location: sys::GHOSTTY_CLIPBOARD_LOCATION_STANDARD,
        source: sys::GHOSTTY_PASTE_SOURCE_CLIPBOARD,
        mimes: &raw const mime,
        mimes_len: 1,
        reader: sys::GhosttyMimeReader {
            read: Some(read_text),
            userdata: (&raw mut source).cast(),
        },
        allow_unsafe,
    };
    let mut written = false;
    // SAFETY: `request`, `mime` and `source` outlive the call; `term` is serialized by the engine.
    let code = unsafe { sys::ghostty_terminal_paste(term, &raw const request, &raw mut written) };
    match code {
        sys::GHOSTTY_SUCCESS if written => Ok(PasteResult::Written),
        sys::GHOSTTY_SUCCESS => Ok(PasteResult::Empty),
        sys::GHOSTTY_REJECTED => Ok(PasteResult::Rejected),
        other => check("ghostty_terminal_paste", other).map(|()| PasteResult::Empty),
    }
}

unsafe extern "C" fn read_text(
    userdata: *mut c_void,
    _mime: sys::GhosttyString,
    writer: sys::GhosttyWriter,
) -> bool {
    // SAFETY: `userdata` is the `&[u8]` `paste` passed, alive for the paste call.
    let text = unsafe { *userdata.cast::<&[u8]>() };
    match writer.write {
        // SAFETY: the writer is valid for this callback and `text` is readable.
        Some(write) => unsafe { write(writer.userdata, text.as_ptr(), text.len()) },
        None => false,
    }
}

/// Runs an encoder that reports OUT_OF_SPACE with the size it needs, retrying once with a buffer that large.
fn encode_with(
    call: &'static str,
    mut encode: impl FnMut(*mut c_char, usize, *mut usize) -> sys::GhosttyResult,
) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; 64];
    let mut len = 0usize;
    let mut code = encode(buf.as_mut_ptr().cast(), buf.len(), &raw mut len);
    if code == sys::GHOSTTY_OUT_OF_SPACE {
        buf.resize(len, 0);
        code = encode(buf.as_mut_ptr().cast(), buf.len(), &raw mut len);
    }
    check(call, code)?;
    buf.truncate(len);
    Ok(buf)
}
