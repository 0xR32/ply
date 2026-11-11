//! GPUI keystrokes → C2 KEY frames that plyd encodes against the pane's live modes (option-as-meta off).

use gpui::Keystroke;
use ply_proto::data::{KeyAction, KeyEvent, Mods};

const SHIFT: u16 = 1;
const CTRL: u16 = 1 << 1;
const ALT: u16 = 1 << 2;

/// The GhosttyKey wire value and the key's unshifted codepoint.
fn key_code(key: &str) -> (u16, u32) {
    let named = match key {
        "enter" => 58,
        "backspace" => 53,
        "tab" => 64,
        "space" => return (63, ' ' as u32),
        "escape" => 120,
        "delete" => 68,
        "home" => 71,
        "end" => 69,
        "pageup" => 74,
        "pagedown" => 73,
        "up" => 78,
        "down" => 75,
        "left" => 76,
        "right" => 77,
        _ => 0,
    };
    if named != 0 {
        return (named, 0);
    }
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.clone().next()) {
        let code = match c {
            'a'..='z' => 20 + (c as u16 - 'a' as u16),
            '0'..='9' => 6 + (c as u16 - '0' as u16),
            '-' => 46,
            '=' => 16,
            '[' => 3,
            ']' => 4,
            '\\' => 2,
            ';' => 49,
            '\'' => 48,
            ',' => 5,
            '.' => 47,
            '/' => 50,
            '`' => 1,
            _ => 0,
        };
        return (code, c as u32);
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u16>().ok())
        && (1..=25).contains(&n)
    {
        return (120 + n, 0);
    }
    (0, 0)
}

fn printable(text: Option<&str>) -> String {
    match text {
        Some(t) if !t.chars().any(char::is_control) => t.to_string(),
        _ => String::new(),
    }
}

/// An arrow key press, which the wheel sends on the alternate screen when the program does not report the mouse.
pub fn arrow(up: bool) -> KeyEvent {
    KeyEvent {
        key: if up { 78 } else { 75 },
        mods: Mods::empty(),
        consumed_mods: Mods::empty(),
        action: KeyAction::Press,
        composing: false,
        unshifted_codepoint: 0,
        text: String::new(),
    }
}

pub fn key_event(ks: &Keystroke, repeat: bool) -> Option<KeyEvent> {
    let m = &ks.modifiers;
    if m.platform {
        return None;
    }
    let (key, unshifted) = key_code(&ks.key);
    let mut mods = 0;
    if m.shift {
        mods |= SHIFT;
    }
    if m.control {
        mods |= CTRL;
    }
    if m.alt {
        mods |= ALT;
    }
    let mut consumed = 0;
    let mut text = String::new();
    if !m.control {
        text = printable(ks.key_char.as_deref());
        let base = char::from_u32(unshifted)
            .map(String::from)
            .unwrap_or_default();
        let base_shifted = if m.shift {
            base.to_uppercase()
        } else {
            base.clone()
        };
        if !text.is_empty() && m.alt && text != base_shifted {
            consumed |= ALT;
        }
        if !text.is_empty() && m.shift && text != base {
            consumed |= SHIFT;
        }
    }
    if key == 0 && text.is_empty() {
        return None;
    }
    Some(KeyEvent {
        key,
        mods: Mods::from_bits(mods)?,
        consumed_mods: Mods::from_bits(consumed)?,
        action: if repeat {
            KeyAction::Repeat
        } else {
            KeyAction::Press
        },
        composing: false,
        unshifted_codepoint: unshifted,
        text,
    })
}
