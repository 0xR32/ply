//! Client input encoded against the pane's live modes (spec R-R5 to R-R10): KEY, MOUSE, PASTE and FOCUS frames.
//!
//! The app never mirrors terminal modes; it sends what the user did and plyd turns it into pty bytes here, through
//! libghostty-vt's encoders. Rules ply adds on top: ⇧⏎ becomes LF (`\n`) while the pane has not enabled the kitty
//! keyboard protocol (R-R6; the legacy encoder would send `CSI 27;2;13~`); focus reports go out only while the
//! program enabled mode 1004 (R-R8); mouse events are encoded from the pixel position against the engine's grid and
//! cell size, with the pressed-button state tracked per pane for drag reporting (R-R9); unsafe pastes are refused
//! until the user confirms (Ruling R21). INPUT_RAW frames are not encoded: their bytes go to the pty as they are.

use ply_proto::data::{
    Focus, Frame, KeyAction, KeyEvent, Mods, MouseAction, MouseButton, MouseEvent, Paste,
};

use crate::engine::{Engine, KeyInput, MouseInput, PasteResult};
use crate::error::Result;

/// `GhosttyKey` of Return, the key R-R6 rewrites.
const KEY_ENTER: u16 = 58;

/// A client input frame to encode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input<'a> {
    /// KEY (0x15).
    Key(&'a KeyEvent),
    /// MOUSE (0x16).
    Mouse(&'a MouseEvent),
    /// PASTE (0x17).
    Paste(&'a Paste),
    /// FOCUS (0x18).
    Focus(Focus),
}

impl<'a> Input<'a> {
    /// The input carried by a KEY, MOUSE, PASTE or FOCUS frame; `None` for every other kind (INPUT_RAW included).
    pub fn from_frame(frame: &'a Frame) -> Option<Self> {
        match frame {
            Frame::Key(k) => Some(Self::Key(k)),
            Frame::Mouse(m) => Some(Self::Mouse(m)),
            Frame::Paste(p) => Some(Self::Paste(p)),
            Frame::Focus(f) => Some(Self::Focus(*f)),
            _ => None,
        }
    }
}

/// What an input turned into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encoded {
    /// Write these bytes to the pty, in order, after any earlier ones.
    Bytes(Vec<u8>),
    /// The pane's modes report nothing for this input (a key release in legacy mode, a mouse event without tracking, focus without mode 1004).
    Nothing,
    /// The paste could inject commands and was not written; plyd sends PASTE_REJECTED and the app may resend with `allow_unsafe` (Ruling R21).
    PasteRejected,
}

/// Encodes one client input for `engine`'s pane; fails with [`crate::Error::Ghostty`] only when libghostty-vt rejects the call itself.
pub fn encode_input(engine: &mut Engine, input: Input<'_>) -> Result<Encoded> {
    match input {
        Input::Key(key) => encode_key(engine, key),
        Input::Mouse(mouse) => encode_mouse(engine, mouse),
        Input::Paste(paste) => match engine.paste(&paste.text, paste.allow_unsafe)? {
            (PasteResult::Written, bytes) => Ok(bytes_or_nothing(bytes)),
            (PasteResult::Empty, _) => Ok(Encoded::Nothing),
            (PasteResult::Rejected, _) => Ok(Encoded::PasteRejected),
        },
        Input::Focus(focus) => {
            if engine.focus_reporting() {
                engine.encode_focus(focus.focused).map(bytes_or_nothing)
            } else {
                Ok(Encoded::Nothing)
            }
        }
    }
}

fn encode_key(engine: &mut Engine, key: &KeyEvent) -> Result<Encoded> {
    let chord = Mods::SHIFT | Mods::CTRL | Mods::ALT | Mods::SUPER;
    if key.key == KEY_ENTER
        && key.action != KeyAction::Release
        && !key.composing
        && key.mods.bits() & chord.bits() == Mods::SHIFT.bits()
        && engine.kitty_keyboard_flags() == 0
    {
        return Ok(Encoded::Bytes(b"\n".to_vec()));
    }
    let input = KeyInput {
        key: i32::from(key.key),
        mods: key.mods.bits(),
        consumed_mods: key.consumed_mods.bits(),
        action: key.action as i32,
        composing: key.composing,
        unshifted_codepoint: key.unshifted_codepoint,
        text: &key.text,
    };
    engine.encode_key(&input).map(bytes_or_nothing)
}

fn encode_mouse(engine: &mut Engine, mouse: &MouseEvent) -> Result<Encoded> {
    let (cell_width, cell_height) = engine.cell_size();
    if cell_width == 0 || cell_height == 0 {
        tracing::debug!(
            pane_id = engine.pane_id(),
            "mouse event before the cell size is known; nothing encoded"
        );
        return Ok(Encoded::Nothing);
    }
    let held = holds_state(mouse.button).then(|| 1u16 << (mouse.button as u16));
    let buttons = engine.pressed_buttons();
    match (mouse.action, held) {
        (MouseAction::Press, Some(bit)) => *buttons |= bit,
        (MouseAction::Release, Some(bit)) => *buttons &= !bit,
        _ => {}
    }
    let any_pressed = *buttons != 0;
    let input = MouseInput {
        action: mouse.action as i32,
        button: (mouse.button != MouseButton::None).then_some(mouse.button as i32),
        mods: mouse.mods.bits(),
        x: mouse.x,
        y: mouse.y,
        any_pressed,
    };
    engine.encode_mouse(&input).map(bytes_or_nothing)
}

/// Wheel steps (buttons 4–7) are momentary; every other button stays down until its release.
fn holds_state(button: MouseButton) -> bool {
    !matches!(
        button,
        MouseButton::None
            | MouseButton::Four
            | MouseButton::Five
            | MouseButton::Six
            | MouseButton::Seven
    )
}

fn bytes_or_nothing(bytes: Vec<u8>) -> Encoded {
    if bytes.is_empty() {
        Encoded::Nothing
    } else {
        Encoded::Bytes(bytes)
    }
}
