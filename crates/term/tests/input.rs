//! Encoder tables (spec R-R5 to R-R10): KEY, MOUSE, PASTE and FOCUS against the pane's live modes, with the
//! expected bytes ADR-0005 recorded for the S2b/S3b agents (legacy and kitty keys, ⌥ handling, SGR and pixel mouse).

#![cfg(feature = "engine")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::engine;
use ply_proto::data::{
    Focus, Frame, KeyAction, KeyEvent, Mods, MouseAction, MouseButton, MouseEvent, Paste,
};
use ply_proto::pane::OptionAsMeta;
use ply_term::{Encoded, Engine, Input, encode_input};

const A: u16 = 20;
const C: u16 = 22;
const J: u16 = 29;
const X: u16 = 43;
const ENTER: u16 = 58;
const TAB: u16 = 64;
const ARROW_UP: u16 = 78;
const ESCAPE: u16 = 120;
const F1: u16 = 121;

fn key(code: u16, mods: Mods, text: &str, unshifted: char) -> KeyEvent {
    KeyEvent {
        key: code,
        mods,
        consumed_mods: Mods::empty(),
        action: KeyAction::Press,
        composing: false,
        unshifted_codepoint: if unshifted == '\0' {
            0
        } else {
            unshifted as u32
        },
        text: text.to_owned(),
    }
}

fn send(e: &mut Engine, event: &KeyEvent) -> Encoded {
    encode_input(e, Input::Key(event)).unwrap()
}

fn bytes(b: &[u8]) -> Encoded {
    Encoded::Bytes(b.to_vec())
}

#[test]
fn legacy_keys_follow_the_live_modes() {
    let mut e = engine(80, 24);
    let none = Mods::empty();
    assert_eq!(send(&mut e, &key(A, none, "a", 'a')), bytes(b"a"));
    assert_eq!(send(&mut e, &key(C, Mods::CTRL, "c", 'c')), bytes(b"\x03"));
    assert_eq!(send(&mut e, &key(ENTER, none, "", '\0')), bytes(b"\r"));
    assert_eq!(send(&mut e, &key(J, Mods::CTRL, "j", 'j')), bytes(b"\n"));
    assert_eq!(send(&mut e, &key(TAB, none, "", '\0')), bytes(b"\t"));
    assert_eq!(
        send(&mut e, &key(TAB, Mods::SHIFT, "", '\0')),
        bytes(b"\x1b[Z")
    );
    assert_eq!(
        send(&mut e, &key(ARROW_UP, none, "", '\0')),
        bytes(b"\x1b[A")
    );
    assert_eq!(send(&mut e, &key(F1, none, "", '\0')), bytes(b"\x1bOP"));
    e.write(b"\x1b[?1h");
    assert_eq!(
        send(&mut e, &key(ARROW_UP, none, "", '\0')),
        bytes(b"\x1bOA"),
        "DECCKM switches the arrows"
    );
    let release = KeyEvent {
        action: KeyAction::Release,
        ..key(A, none, "a", 'a')
    };
    assert_eq!(send(&mut e, &release), Encoded::Nothing);
    assert_eq!(
        send(&mut e, &key(0, none, "こ", '\0')),
        bytes("こ".as_bytes()),
        "committed IME text goes out as is"
    );
}

#[test]
fn shift_enter_is_lf_until_the_pane_enables_kitty() {
    let mut e = engine(80, 24);
    let shift_enter = key(ENTER, Mods::SHIFT, "", '\0');
    assert_eq!(
        send(&mut e, &shift_enter),
        bytes(b"\n"),
        "R-R6: the legacy encoder alone would send CSI 27;2;13~"
    );
    let caps = key(ENTER, Mods::SHIFT | Mods::CAPS_LOCK, "", '\0');
    assert_eq!(send(&mut e, &caps), bytes(b"\n"));
    assert_eq!(
        send(&mut e, &key(ENTER, Mods::SHIFT | Mods::CTRL, "", '\0')),
        bytes(b"\x1b[27;6;13~")
    );
    e.write(b"\x1b[>1u");
    assert_eq!(send(&mut e, &shift_enter), bytes(b"\x1b[13;2u"));
    assert_eq!(
        send(&mut e, &key(C, Mods::CTRL, "c", 'c')),
        bytes(b"\x1b[99;5u")
    );
    assert_eq!(
        send(&mut e, &key(ESCAPE, Mods::empty(), "", '\0')),
        bytes(b"\x1b[27u")
    );
    e.write(b"\x1b[=31u");
    assert_eq!(
        send(&mut e, &key(A, Mods::empty(), "a", 'a')),
        bytes(b"\x1b[97;;97u")
    );
    let release = KeyEvent {
        action: KeyAction::Release,
        ..key(A, Mods::empty(), "a", 'a')
    };
    assert_eq!(send(&mut e, &release), bytes(b"\x1b[97;1:3u"));
    e.write(b"\x1b[<u\x1b[<u");
    assert_eq!(
        send(&mut e, &shift_enter),
        bytes(b"\n"),
        "popping the kitty flags restores the LF rule"
    );
}

#[test]
fn option_follows_option_as_meta_and_the_consumed_modifiers() {
    let mut e = engine(80, 24);
    let left = key(X, Mods::ALT, "\u{2248}", 'x');
    let right = key(X, Mods::ALT | Mods::ALT_SIDE, "\u{2248}", 'x');
    let consumed = KeyEvent {
        consumed_mods: Mods::ALT,
        ..left.clone()
    };
    assert_eq!(
        send(&mut e, &left),
        bytes("\u{2248}".as_bytes()),
        "off: ⌥ types layout characters"
    );
    e.set_option_as_meta(OptionAsMeta::Both);
    assert_eq!(send(&mut e, &left), bytes(b"\x1bx"));
    assert_eq!(
        send(&mut e, &consumed),
        bytes("\u{2248}".as_bytes()),
        "a consumed ⌥ still types"
    );
    e.set_option_as_meta(OptionAsMeta::Right);
    assert_eq!(send(&mut e, &left), bytes("\u{2248}".as_bytes()));
    assert_eq!(send(&mut e, &right), bytes(b"\x1bx"));
    e.set_option_as_meta(OptionAsMeta::Left);
    assert_eq!(send(&mut e, &left), bytes(b"\x1bx"));
    e.set_option_as_meta(OptionAsMeta::Off);
    e.write(b"\x1b[>31u");
    assert_eq!(send(&mut e, &consumed), bytes(b"\x1b[120;3;8776u"));
}

#[test]
fn composing_keys_send_nothing() {
    let mut e = engine(80, 24);
    let composing = KeyEvent {
        composing: true,
        ..key(A, Mods::empty(), "a", 'a')
    };
    assert_eq!(send(&mut e, &composing), Encoded::Nothing);
}

fn mouse(action: MouseAction, button: MouseButton, mods: Mods, x: f32, y: f32) -> MouseEvent {
    MouseEvent {
        action,
        button,
        mods,
        col: (x / 10.0) as u16,
        row: (y / 20.0) as u16,
        x,
        y,
    }
}

fn click(e: &mut Engine, event: MouseEvent) -> Encoded {
    encode_input(e, Input::Mouse(&event)).unwrap()
}

#[test]
fn mouse_events_follow_the_tracking_mode_and_format() {
    let mut e = engine(80, 24);
    e.resize(80, 24, 10, 20).unwrap();
    let none = Mods::empty();
    let (x, y) = (45.0, 50.0);
    assert_eq!(
        click(
            &mut e,
            mouse(MouseAction::Press, MouseButton::Left, none, x, y)
        ),
        Encoded::Nothing,
        "no tracking, no report"
    );
    click(
        &mut e,
        mouse(MouseAction::Release, MouseButton::Left, none, x, y),
    );
    e.write(b"\x1b[?1000h");
    assert_eq!(
        click(
            &mut e,
            mouse(MouseAction::Press, MouseButton::Left, none, x, y)
        ),
        bytes(b"\x1b[M %#"),
        "X10 format"
    );
    click(
        &mut e,
        mouse(MouseAction::Release, MouseButton::Left, none, x, y),
    );
    e.write(b"\x1b[?1006h");
    assert_eq!(
        click(
            &mut e,
            mouse(MouseAction::Press, MouseButton::Left, none, x, y)
        ),
        bytes(b"\x1b[<0;5;3M")
    );
    assert_eq!(
        click(
            &mut e,
            mouse(MouseAction::Release, MouseButton::Left, none, x, y)
        ),
        bytes(b"\x1b[<0;5;3m")
    );
    assert_eq!(
        click(
            &mut e,
            mouse(MouseAction::Motion, MouseButton::None, none, x + 10.0, y)
        ),
        Encoded::Nothing,
        "1000 reports no motion"
    );
    assert_eq!(
        click(
            &mut e,
            mouse(MouseAction::Press, MouseButton::Four, none, x, y)
        ),
        bytes(b"\x1b[<64;5;3M"),
        "wheel up"
    );
    assert_eq!(
        click(
            &mut e,
            mouse(MouseAction::Press, MouseButton::Left, Mods::CTRL, x, y)
        ),
        bytes(b"\x1b[<16;5;3M")
    );
    e.write(b"\x1b[?1002h");
    assert_eq!(
        click(
            &mut e,
            mouse(MouseAction::Motion, MouseButton::Left, none, x + 20.0, y)
        ),
        bytes(b"\x1b[<32;7;3M"),
        "drag while held"
    );
    click(
        &mut e,
        mouse(MouseAction::Release, MouseButton::Left, none, x + 20.0, y),
    );
    e.write(b"\x1b[?1003h");
    assert_eq!(
        click(
            &mut e,
            mouse(MouseAction::Motion, MouseButton::None, none, x + 30.0, y)
        ),
        bytes(b"\x1b[<35;8;3M"),
        "hover"
    );
    e.write(b"\x1b[?1016h");
    assert_eq!(
        click(
            &mut e,
            mouse(MouseAction::Press, MouseButton::Left, none, 47.5, 51.25)
        ),
        bytes(b"\x1b[<0;48;51M"),
        "SGR pixels"
    );
}

#[test]
fn focus_reports_only_with_mode_1004() {
    let mut e = engine(20, 4);
    let gained = Frame::Focus(Focus { focused: true });
    let lost = Frame::Focus(Focus { focused: false });
    assert_eq!(
        encode_input(&mut e, Input::from_frame(&gained).unwrap()).unwrap(),
        Encoded::Nothing
    );
    e.write(b"\x1b[?1004h");
    assert_eq!(
        encode_input(&mut e, Input::from_frame(&gained).unwrap()).unwrap(),
        bytes(b"\x1b[I")
    );
    assert_eq!(
        encode_input(&mut e, Input::from_frame(&lost).unwrap()).unwrap(),
        bytes(b"\x1b[O")
    );
    assert!(
        Input::from_frame(&Frame::InputRaw(b"x".to_vec())).is_none(),
        "INPUT_RAW is not encoded"
    );
}

fn paste(e: &mut Engine, text: &str, allow_unsafe: bool) -> Encoded {
    let p = Paste {
        allow_unsafe,
        text: text.to_owned(),
    };
    encode_input(e, Input::Paste(&p)).unwrap()
}

#[test]
fn pastes_are_validated_and_bracketed() {
    let mut e = engine(40, 5);
    assert_eq!(paste(&mut e, "echo hi", false), bytes(b"echo hi"));
    assert_eq!(
        paste(&mut e, "a\nb", false),
        Encoded::PasteRejected,
        "a newline could run a command (R21)"
    );
    assert_eq!(paste(&mut e, "a\nb", true), bytes(b"a\rb"));
    assert_eq!(
        paste(&mut e, "x\x1by", false),
        bytes(b"x y"),
        "control bytes become spaces"
    );
    assert_eq!(paste(&mut e, "", false), Encoded::Nothing);
    e.write(b"\x1b[?2004h");
    assert_eq!(
        paste(&mut e, "a\nb", false),
        bytes(b"\x1b[200~a\nb\x1b[201~")
    );
    assert_eq!(paste(&mut e, "a\x1b[201~b", false), Encoded::PasteRejected);
    assert!(
        e.write(b"").reply.is_empty(),
        "paste bytes are returned, not left behind as replies"
    );
}

#[test]
fn undefined_key_codes_encode_nothing_instead_of_reaching_the_library() {
    let mut e = engine(80, 24);
    for code in [176, 200, u16::MAX] {
        assert_eq!(
            send(&mut e, &key(code, Mods::empty(), "a", 'a')),
            Encoded::Nothing,
            "key {code}"
        );
    }
    assert_eq!(
        send(&mut e, &key(175, Mods::empty(), "", '\0')),
        Encoded::Nothing,
        "the last defined key is accepted"
    );
    assert_eq!(
        send(&mut e, &key(A, Mods::empty(), "a", 'a')),
        bytes(b"a"),
        "the encoder still works afterwards"
    );
    for bad in [0xD800, 0x11_0000, u32::MAX] {
        let odd = KeyEvent {
            unshifted_codepoint: bad,
            ..key(A, Mods::empty(), "a", 'a')
        };
        assert_eq!(
            send(&mut e, &odd),
            bytes(b"a"),
            "an invalid unshifted codepoint {bad:#x} is sent as unknown"
        );
    }
}

#[test]
fn huge_or_non_finite_pointer_positions_are_clamped_or_refused() {
    let mut e = engine(80, 24);
    e.resize(80, 24, 10, 20).unwrap();
    e.write(b"\x1b[?1000h\x1b[?1006h");
    let none = Mods::empty();
    let far = |action| mouse(action, MouseButton::Left, none, 1e30, 1e30);
    assert_eq!(
        click(&mut e, far(MouseAction::Press)),
        Encoded::Nothing,
        "a press outside the surface is not reported"
    );
    assert_eq!(
        click(&mut e, far(MouseAction::Release)),
        bytes(b"\x1b[<0;80;24m"),
        "a release anywhere is, at the last cell"
    );
    let near = mouse(MouseAction::Release, MouseButton::Left, none, -1e30, -1e30);
    assert_eq!(click(&mut e, near), bytes(b"\x1b[<0;1;1m"));
    e.write(b"\x1b[?1016h");
    let pixels = mouse(MouseAction::Release, MouseButton::Left, none, 1e30, 5.0);
    assert_eq!(
        click(&mut e, pixels),
        bytes(b"\x1b[<0;1600;5m"),
        "SGR pixels stop at twice the surface width"
    );
    for (x, y) in [
        (f32::NAN, 5.0),
        (5.0, f32::INFINITY),
        (f32::NEG_INFINITY, f32::NAN),
    ] {
        assert_eq!(
            click(
                &mut e,
                mouse(MouseAction::Release, MouseButton::Left, none, x, y)
            ),
            Encoded::Nothing
        );
    }
}

#[test]
fn resetting_the_buttons_ends_a_drag_the_view_lost() {
    let mut e = engine(80, 24);
    e.resize(80, 24, 10, 20).unwrap();
    e.write(b"\x1b[?1002h\x1b[?1006h");
    let none = Mods::empty();
    click(
        &mut e,
        mouse(MouseAction::Press, MouseButton::Left, none, 45.0, 50.0),
    );
    let outside = mouse(MouseAction::Motion, MouseButton::Left, none, 900.0, 50.0);
    assert_eq!(
        click(&mut e, outside),
        bytes(b"\x1b[<32;80;3M"),
        "held: dragging out of the surface is reported"
    );
    e.reset_mouse_buttons();
    let outside = mouse(MouseAction::Motion, MouseButton::Left, none, 950.0, 60.0);
    assert_eq!(
        click(&mut e, outside),
        Encoded::Nothing,
        "after the reset no button is held"
    );
}
