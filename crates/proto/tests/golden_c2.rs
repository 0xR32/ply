//! Golden C2 frames: one binary per frame kind in `tests/golden/c2/`, written by this crate's encoder.
//!
//! `app/src/terminal/frames.ts` decodes every file and re-encodes it byte for byte (`frames.test.ts`), so the two
//! hand-written codecs cannot drift apart. The values sit inside JavaScript's safe-integer range, which the
//! TypeScript codec enforces for `u64`/`i64` fields. After an intended layout change, regenerate the files with
//! `PLY_BLESS=1 cargo test -p ply-proto --test golden_c2` and update the TypeScript expectations with them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ply_proto::C2_VERSION;
use ply_proto::data::{
    Ack, Attach, AttachRefused, Attrs, Cell, CellFlags, Color, Cursor, CursorShape, Delta, Exit,
    FetchHistory, Focus, Frame, FrameReader, History, KeyAction, KeyEvent, Modes, Mods,
    MouseAction, MouseButton, MouseEvent, Paste, RefuseReason, Resize, Row, Snapshot, Style,
    StyleEntry, Underline,
};

const MAX_SAFE: u64 = (1 << 53) - 1;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/c2")
}

fn cell(codepoint: u32, style: u16, flags: CellFlags, extra: Vec<u32>) -> Cell {
    Cell {
        codepoint,
        style,
        flags,
        extra,
    }
}

fn styles() -> Vec<StyleEntry> {
    vec![
        StyleEntry {
            id: 1,
            style: Style {
                fg: Color::Indexed(1),
                bg: Color::Indexed(200),
                underline_color: Color::Rgb(1, 2, 3),
                attrs: (Attrs::BOLD | Attrs::ITALIC | Attrs::STRIKETHROUGH | Attrs::OVERLINE)
                    .with_underline(Underline::Curly),
            },
        },
        StyleEntry {
            id: 2,
            style: Style {
                fg: Color::Rgb(0xE6, 0xE8, 0xEF),
                bg: Color::Default,
                underline_color: Color::Default,
                attrs: (Attrs::FAINT | Attrs::BLINK | Attrs::INVERSE | Attrs::INVISIBLE)
                    .with_underline(Underline::Dashed),
            },
        },
        StyleEntry {
            id: 65535,
            style: Style::default(),
        },
    ]
}

fn rows(first: i32) -> Vec<Row> {
    vec![
        Row {
            index: first,
            wrapped: true,
            cells: vec![
                cell('A' as u32, 1, CellFlags::empty(), vec![]),
                cell(0x4E2D, 0, CellFlags::WIDE, vec![]),
                cell(0, 0, CellFlags::SPACER, vec![]),
                cell(
                    0x1F468,
                    2,
                    CellFlags::WIDE | CellFlags::GRAPHEME,
                    vec![0x200D, 0x1F469, 0x200D, 0x1F467],
                ),
                cell(0, 0, CellFlags::SPACER, vec![]),
                cell('e' as u32, 65535, CellFlags::GRAPHEME, vec![0x0301]),
                cell(0x10_FFFF, 0, CellFlags::empty(), vec![]),
                cell(0, 0, CellFlags::SPACER_HEAD, vec![]),
            ],
        },
        Row {
            index: first + 1,
            wrapped: false,
            cells: vec![],
        },
    ]
}

/// Every frame kind once (KEY twice, for the composing flag), with values that exercise each field's range.
fn goldens() -> Vec<(&'static str, Frame)> {
    vec![
        (
            "attach",
            Frame::Attach(Attach {
                v: C2_VERSION,
                pane_id: MAX_SAFE,
                cols: 168,
                rows: 50,
                cell_width_px: 8,
                cell_height_px: 19,
            }),
        ),
        ("input-raw", Frame::InputRaw(vec![b'1', 0x00, 0x1B, 0xFF])),
        (
            "resize",
            Frame::Resize(Resize {
                cols: 65535,
                rows: 1,
                cell_width_px: 7,
                cell_height_px: 16,
            }),
        ),
        (
            "fetch-history",
            Frame::FetchHistory(FetchHistory {
                start: MAX_SAFE,
                count: 1000,
            }),
        ),
        ("ack", Frame::Ack(Ack { seq: MAX_SAFE })),
        (
            "key",
            Frame::Key(KeyEvent {
                key: 175,
                mods: Mods::ALT | Mods::ALT_SIDE | Mods::SHIFT | Mods::SUPER_SIDE,
                consumed_mods: Mods::ALT,
                action: KeyAction::Repeat,
                composing: false,
                unshifted_codepoint: 'x' as u32,
                text: "≈".to_owned(),
            }),
        ),
        (
            "key-composing",
            Frame::Key(KeyEvent {
                key: 0,
                mods: Mods::empty(),
                consumed_mods: Mods::empty(),
                action: KeyAction::Release,
                composing: true,
                unshifted_codepoint: 0x10_FFFF,
                text: String::new(),
            }),
        ),
        (
            "mouse",
            Frame::Mouse(MouseEvent {
                action: MouseAction::Motion,
                button: MouseButton::Eleven,
                mods: Mods::CTRL | Mods::CTRL_SIDE | Mods::CAPS_LOCK | Mods::NUM_LOCK,
                col: 5,
                row: 3,
                x: 47.5,
                y: -0.25,
            }),
        ),
        (
            "paste",
            Frame::Paste(Paste {
                allow_unsafe: true,
                text: "a\nb\u{1b}[201~ü".to_owned(),
            }),
        ),
        ("focus", Frame::Focus(Focus { focused: true })),
        (
            "snapshot",
            Frame::Snapshot(Snapshot {
                seq: 1,
                cols: 168,
                rows: 2,
                cursor: Cursor {
                    col: 7,
                    row: 1,
                    shape: CursorShape::Bar,
                    visible: true,
                    blinking: true,
                },
                modes: Modes::ALT_SCREEN
                    | Modes::CURSOR_VISIBLE
                    | Modes::MOUSE_REPORTING
                    | Modes::BRACKETED_PASTE,
                scrollback_rows: 10_300,
                scrollback_base: 123_456_789,
                styles: styles(),
                lines: rows(0),
            }),
        ),
        (
            "delta",
            Frame::Delta(Delta {
                seq: MAX_SAFE,
                cursor: Cursor {
                    col: 0,
                    row: 0,
                    shape: CursorShape::BlockHollow,
                    visible: false,
                    blinking: false,
                },
                modes: Modes::CURSOR_VISIBLE,
                scrollback_rows: u32::MAX,
                scrollback_base: MAX_SAFE,
                styles_added: vec![StyleEntry {
                    id: 3,
                    style: Style {
                        fg: Color::Default,
                        bg: Color::Rgb(0x0C, 0x0E, 0x14),
                        underline_color: Color::Indexed(255),
                        attrs: Attrs::empty().with_underline(Underline::Double),
                    },
                }],
                lines: rows(1),
            }),
        ),
        (
            "history",
            Frame::History(History {
                start: 123_466_787,
                lines: rows(0),
                styles_added: styles(),
            }),
        ),
        ("title", Frame::Title("claude — example ✳".to_owned())),
        ("bell", Frame::Bell),
        ("exit", Frame::Exit(Exit { code: -129 })),
        ("paste-rejected", Frame::PasteRejected),
        (
            "attach-refused",
            Frame::AttachRefused(AttachRefused {
                reason: RefuseReason::UnknownPane,
                message: "no pane 9".to_owned(),
            }),
        ),
    ]
}

fn encode(frame: &Frame) -> Vec<u8> {
    let mut wire = Vec::new();
    frame.encode(&mut wire).unwrap();
    wire
}

#[test]
fn c2_goldens_match_the_encoder_and_decode_back() {
    let bless = std::env::var_os("PLY_BLESS").is_some();
    if bless {
        std::fs::create_dir_all(dir()).unwrap();
    }
    for (name, frame) in goldens() {
        let path = dir().join(format!("{name}.bin"));
        let wire = encode(&frame);
        if bless {
            std::fs::write(&path, &wire).unwrap();
        }
        let golden = std::fs::read(&path).unwrap_or_else(|e| {
            panic!("{name}: {e}; run PLY_BLESS=1 cargo test -p ply-proto --test golden_c2")
        });
        assert_eq!(
            golden, wire,
            "{name}: the encoder no longer writes the golden bytes"
        );
        let mut reader = FrameReader::new(golden.as_slice());
        assert_eq!(
            reader.read_frame().unwrap().as_ref(),
            Some(&frame),
            "{name}"
        );
        assert_eq!(reader.read_frame().unwrap(), None, "{name}: trailing bytes");
    }
}

#[test]
fn c2_goldens_cover_every_kind_and_nothing_else() {
    let kinds: BTreeSet<u8> = goldens().iter().map(|(_, f)| f.kind()).collect();
    let expected: BTreeSet<u8> = (0x10..=0x18).chain(0x20..=0x27).collect();
    assert_eq!(kinds, expected);
    let names: BTreeSet<String> = goldens().iter().map(|(n, _)| format!("{n}.bin")).collect();
    let on_disk: BTreeSet<String> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        on_disk, names,
        "tests/golden/c2 holds a file no golden writes"
    );
}
