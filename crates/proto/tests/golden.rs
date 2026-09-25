//! Protocol conformance: C1 and C3 golden files round-tripped both ways, C2 frames of every kind round-tripped
//! through the encoder and the reader, and the strictness rules (caps, unknown fields, versions) enforced.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ply_proto::control::{
    self, ClientMsg, ErrorCode, MAX_LINE_BYTES, METHODS, Response, ServerMsg, decode_line,
    encode_line,
};
use ply_proto::data::{
    self, Ack, Attach, AttachRefused, Attrs, Cell, CellFlags, Color, Cursor, CursorShape, Delta,
    Exit, FetchHistory, Focus, Frame, FrameReader, History, KeyAction, KeyEvent, MAX_FRAME_LEN,
    Modes, Mods, MouseAction, MouseButton, MouseEvent, Paste, RefuseReason, Resize, Row, Snapshot,
    Style, StyleEntry, Underline, kind,
};
use ply_proto::hook::HookEnvelope;
use ply_proto::pane::{Layout, Pane, Session, Settings, Workspace};
use ply_proto::{C2_VERSION, Error, PROTOCOL_VERSION, version};
use serde_json::Value;

fn golden_dir(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(sub)
}

fn goldens(sub: &str) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(golden_dir(sub))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

fn is_client(name: &str) -> bool {
    name.starts_with("hello") || name.starts_with("req.")
}

fn with_extra_field(v: &Value, nested: bool) -> Value {
    let mut v = v.clone();
    let key = if v.get("p").is_some() { "p" } else { "r" };
    let target = if nested { v.get_mut(key) } else { Some(&mut v) };
    if let Some(Value::Object(map)) = target {
        map.insert("unexpected".to_owned(), Value::Bool(true));
    }
    v
}

#[test]
fn c1_goldens_round_trip_both_ways() {
    let files = goldens("c1");
    assert!(
        files.len() >= 30,
        "expected a golden per C1 message, found {}",
        files.len()
    );
    for (name, bytes) in &files {
        let golden: Value = serde_json::from_slice(bytes).unwrap();
        let reserialized = if is_client(name) {
            let msg = ClientMsg::decode(bytes).unwrap_or_else(|r| panic!("{name}: {r}"));
            let line = encode_line(&msg).unwrap();
            assert_eq!(
                ClientMsg::decode(&line).unwrap(),
                msg,
                "{name}: decode(encode(x)) != x"
            );
            serde_json::to_value(&msg).unwrap()
        } else {
            let msg: ServerMsg = decode_line(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            let line = encode_line(&msg).unwrap();
            assert_eq!(
                decode_line::<ServerMsg>(&line).unwrap(),
                msg,
                "{name}: decode(encode(x)) != x"
            );
            serde_json::to_value(&msg).unwrap()
        };
        assert_eq!(
            reserialized, golden,
            "{name}: encode(decode(golden)) != golden"
        );
    }
}

#[test]
fn c1_goldens_cover_every_method_and_event() {
    let names: BTreeSet<String> = goldens("c1").into_iter().map(|(n, _)| n).collect();
    for m in METHODS {
        assert!(
            names.contains(&format!("req.{}.json", m.name)),
            "no golden for method {}",
            m.name
        );
    }
    for e in [
        "pane.added",
        "pane.removed",
        "pane.status",
        "pane.progress",
        "pane.meta",
        "pane.exit",
        "daemon.stopping",
    ] {
        assert!(
            names.contains(&format!("evt.{e}.json")),
            "no golden for event {e}"
        );
    }
    for n in [
        "hello.json",
        "welcome.json",
        "res.ok.pane.json",
        "res.err.json",
    ] {
        assert!(names.contains(n), "no golden {n}");
    }
}

fn result_of<T: serde::de::DeserializeOwned + serde::Serialize>(bytes: &[u8]) -> (Value, Value) {
    let ServerMsg::Res(Response { outcome: Ok(r), .. }) = decode_line::<ServerMsg>(bytes).unwrap()
    else {
        panic!("not a successful response");
    };
    let typed: T = serde_json::from_value(r.clone()).unwrap();
    (serde_json::to_value(typed).unwrap(), r)
}

#[test]
fn c1_results_decode_into_their_method_types() {
    let dir = golden_dir("c1");
    let read = |n: &str| std::fs::read(dir.join(n)).unwrap();
    let pairs = [
        result_of::<Pane>(&read("res.ok.pane.json")),
        result_of::<Vec<Workspace>>(&read("res.ok.workspaces.json")),
        result_of::<Vec<Session>>(&read("res.ok.sessions.json")),
        result_of::<Layout>(&read("res.ok.layout.json")),
        result_of::<Settings>(&read("res.ok.settings.json")),
        result_of::<control::Empty>(&read("res.ok.empty.json")),
    ];
    for (typed, raw) in pairs {
        assert_eq!(typed, raw);
    }
}

#[test]
fn c1_unknown_fields_are_rejected_everywhere() {
    for (name, bytes) in goldens("c1") {
        let golden: Value = serde_json::from_slice(&bytes).unwrap();
        for nested in [false, true] {
            if nested
                && golden
                    .get("p")
                    .or_else(|| golden.get("r"))
                    .is_none_or(|v| !v.is_object())
            {
                continue;
            }
            let line = serde_json::to_vec(&with_extra_field(&golden, nested)).unwrap();
            if is_client(&name) {
                let rejection = ClientMsg::decode(&line).expect_err(&name);
                assert_eq!(rejection.error.code, ErrorCode::BadRequest, "{name}");
                if name.starts_with("req.") {
                    assert_eq!(
                        rejection.id,
                        golden["id"].as_u64(),
                        "{name}: the id must survive"
                    );
                }
            } else if !(nested && golden["t"] == "res") {
                assert!(
                    decode_line::<ServerMsg>(&line).is_err(),
                    "{name} (nested: {nested}) accepted"
                );
            }
        }
    }
}

#[test]
fn c1_unknown_method_and_bad_params_keep_the_id() {
    let r = ClientMsg::decode(br#"{"t":"req","id":9,"m":"pane.explode","p":{}}"#).unwrap_err();
    assert_eq!((r.id, r.error.code), (Some(9), ErrorCode::UnknownMethod));
    let r =
        ClientMsg::decode(br#"{"t":"req","id":10,"m":"pane.answer","p":{"pane_id":1,"choice":4}}"#)
            .unwrap_err();
    assert_eq!((r.id, r.error.code), (Some(10), ErrorCode::BadRequest));
    let r = ClientMsg::decode(br#"{"t":"req","id":11,"m":"pane.close"}"#).unwrap_err();
    assert_eq!((r.id, r.error.code), (Some(11), ErrorCode::BadRequest));
    let r = ClientMsg::decode(b"not json").unwrap_err();
    assert_eq!((r.id, r.error.code), (None, ErrorCode::BadRequest));
}

#[test]
fn c1_invalid_values_are_rejected() {
    let theme = std::fs::read_to_string(golden_dir("c1").join("req.theme.set.json")).unwrap();
    let short = theme.replacen("\"#0A0B10\",", "", 1);
    assert!(
        ClientMsg::decode(short.as_bytes()).is_err(),
        "15 ANSI colours accepted"
    );
    let bad_hex = theme.replacen("#0A0B10", "#0A0B1G", 1);
    assert!(
        ClientMsg::decode(bad_hex.as_bytes()).is_err(),
        "a non-hex colour accepted"
    );
    let lower = theme.replacen("#0A0B10", "#0a0b10", 1);
    assert!(
        ClientMsg::decode(lower.as_bytes()).is_ok(),
        "lower-case hex rejected"
    );
    let both = br#"{"t":"res","id":1,"ok":true,"r":{},"err":{"code":"internal","msg":"x"}}"#;
    assert!(
        decode_line::<ServerMsg>(both).is_err(),
        "ok with err accepted"
    );
    let neither = br#"{"t":"res","id":1,"ok":false}"#;
    assert!(
        decode_line::<ServerMsg>(neither).is_err(),
        "failure without err accepted"
    );
}

#[test]
fn c1_line_cap_is_enforced() {
    let mut line = br#"{"t":"hello","v":1,"client":""#.to_vec();
    line.resize(MAX_LINE_BYTES + 1, b'a');
    assert_eq!(
        ClientMsg::decode(&line).unwrap_err().error.code,
        ErrorCode::BadRequest
    );
    assert!(matches!(
        decode_line::<ServerMsg>(&line),
        Err(Error::LineTooLong { .. })
    ));
    let huge = control::Hello {
        v: 1,
        client: "a".repeat(MAX_LINE_BYTES),
        app_version: String::new(),
    };
    assert!(matches!(
        encode_line(&ClientMsg::Hello(huge)),
        Err(Error::LineTooLong { .. })
    ));
}

#[test]
fn c1_version_mismatch_is_detectable() {
    let ClientMsg::Hello(ok) =
        ClientMsg::decode(br#"{"t":"hello","v":1,"client":"ply-app","app_version":"0.1.0"}"#)
            .unwrap()
    else {
        panic!("not a hello");
    };
    assert!(ok.check_version().is_ok());
    let ClientMsg::Hello(newer) =
        ClientMsg::decode(br#"{"t":"hello","v":2,"client":"ply-app","app_version":"9.0.0"}"#)
            .unwrap()
    else {
        panic!("not a hello");
    };
    assert!(matches!(
        newer.check_version(),
        Err(Error::VersionMismatch {
            protocol: "C1",
            expected: PROTOCOL_VERSION,
            found: 2
        })
    ));
}

#[test]
fn c3_goldens_round_trip_and_stay_strict() {
    let files = goldens("c3");
    assert_eq!(files.len(), 2);
    for (name, bytes) in files {
        let golden: Value = serde_json::from_slice(&bytes).unwrap();
        let env = HookEnvelope::decode_line(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let line = env.encode_line().unwrap();
        assert_eq!(HookEnvelope::decode_line(&line).unwrap(), env, "{name}");
        assert_eq!(serde_json::to_value(&env).unwrap(), golden, "{name}");
        let mut extra = golden.clone();
        extra["unexpected"] = Value::Bool(true);
        assert!(
            HookEnvelope::decode_line(&serde_json::to_vec(&extra).unwrap()).is_err(),
            "{name}"
        );
        let mut newer = golden.clone();
        newer["v"] = Value::from(2);
        assert!(matches!(
            HookEnvelope::decode_line(&serde_json::to_vec(&newer).unwrap()),
            Err(Error::VersionMismatch { protocol: "C3", .. })
        ));
    }
    let mut line = br#"{"v":1,"pane_id":1,"cli":"claude","payload":""#.to_vec();
    line.resize(ply_proto::hook::MAX_LINE_BYTES + 1, b'a');
    assert!(matches!(
        HookEnvelope::decode_line(&line),
        Err(Error::LineTooLong { .. })
    ));
}

fn cursor() -> Cursor {
    Cursor {
        col: 7,
        row: 2,
        shape: CursorShape::Bar,
        visible: true,
        blinking: true,
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
                    .with_underline(Underline::Single),
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
    ]
}

fn rows(first: i32) -> Vec<Row> {
    vec![
        Row {
            index: first,
            wrapped: true,
            cells: vec![
                Cell {
                    codepoint: 'A' as u32,
                    style: 1,
                    flags: CellFlags::empty(),
                    extra: vec![],
                },
                Cell {
                    codepoint: 0x4E2D,
                    style: 0,
                    flags: CellFlags::WIDE,
                    extra: vec![],
                },
                Cell {
                    codepoint: 0,
                    style: 0,
                    flags: CellFlags::SPACER,
                    extra: vec![],
                },
                Cell {
                    codepoint: 0x1F468,
                    style: 2,
                    flags: CellFlags::WIDE | CellFlags::GRAPHEME,
                    extra: vec![0x200D, 0x1F469, 0x200D, 0x1F467],
                },
                Cell {
                    codepoint: 0,
                    style: 0,
                    flags: CellFlags::SPACER,
                    extra: vec![],
                },
                Cell {
                    codepoint: 0,
                    style: 0,
                    flags: CellFlags::SPACER_HEAD,
                    extra: vec![],
                },
            ],
        },
        Row {
            index: first + 1,
            wrapped: false,
            cells: vec![],
        },
    ]
}

fn every_frame() -> Vec<Frame> {
    vec![
        Frame::Attach(Attach {
            v: C2_VERSION,
            pane_id: 42,
            cols: 168,
            rows: 50,
            cell_width_px: 8,
            cell_height_px: 19,
        }),
        Frame::InputRaw(b"1".to_vec()),
        Frame::Resize(Resize {
            cols: 80,
            rows: 24,
            cell_width_px: 8,
            cell_height_px: 19,
        }),
        Frame::FetchHistory(FetchHistory {
            start: -1000,
            count: 1000,
        }),
        Frame::Ack(Ack { seq: u64::MAX }),
        Frame::Key(KeyEvent {
            key: 175,
            mods: Mods::ALT | Mods::ALT_SIDE | Mods::SHIFT,
            consumed_mods: Mods::ALT,
            action: KeyAction::Repeat,
            composing: false,
            unshifted_codepoint: 'x' as u32,
            text: "≈".to_owned(),
        }),
        Frame::Key(KeyEvent {
            key: 0,
            mods: Mods::empty(),
            consumed_mods: Mods::empty(),
            action: KeyAction::Press,
            composing: true,
            unshifted_codepoint: 0,
            text: String::new(),
        }),
        Frame::Mouse(MouseEvent {
            action: MouseAction::Motion,
            button: MouseButton::Eleven,
            mods: Mods::CTRL | Mods::SUPER_SIDE | Mods::SUPER,
            col: 5,
            row: 3,
            x: 47.5,
            y: 51.25,
        }),
        Frame::Paste(Paste {
            allow_unsafe: true,
            text: "a\nb".to_owned(),
        }),
        Frame::Focus(Focus { focused: true }),
        Frame::Snapshot(Snapshot {
            seq: 1,
            cols: 168,
            rows: 2,
            cursor: cursor(),
            modes: Modes::ALT_SCREEN
                | Modes::CURSOR_VISIBLE
                | Modes::MOUSE_REPORTING
                | Modes::BRACKETED_PASTE,
            scrollback_rows: 10_300,
            styles: styles(),
            lines: rows(0),
        }),
        Frame::Delta(Delta {
            seq: 2,
            cursor: Cursor {
                shape: CursorShape::BlockHollow,
                visible: false,
                blinking: false,
                ..cursor()
            },
            modes: Modes::empty(),
            scrollback_rows: 0,
            styles_added: vec![],
            lines: rows(0),
        }),
        Frame::History(History {
            start: -2,
            lines: rows(-2),
            styles_added: styles(),
        }),
        Frame::Title("claude — example".to_owned()),
        Frame::Bell,
        Frame::Exit(Exit { code: -1 }),
        Frame::PasteRejected,
        Frame::AttachRefused(AttachRefused {
            reason: RefuseReason::VersionMismatch,
            message: "plyd speaks C2 version 1".to_owned(),
        }),
    ]
}

#[test]
fn c2_every_kind_round_trips_through_encode_and_the_reader() {
    let frames = every_frame();
    let kinds: BTreeSet<u8> = frames.iter().map(Frame::kind).collect();
    let all: BTreeSet<u8> = [
        kind::ATTACH,
        kind::INPUT_RAW,
        kind::RESIZE,
        kind::FETCH_HISTORY,
        kind::ACK,
        kind::KEY,
        kind::MOUSE,
        kind::PASTE,
        kind::FOCUS,
        kind::SNAPSHOT,
        kind::DELTA,
        kind::HISTORY,
        kind::TITLE,
        kind::BELL,
        kind::EXIT,
        kind::PASTE_REJECTED,
        kind::ATTACH_REFUSED,
    ]
    .into();
    assert_eq!(kinds, all, "a frame kind has no round-trip case");

    let mut wire = Vec::new();
    for f in &frames {
        f.encode(&mut wire).unwrap();
    }
    let mut reader = FrameReader::new(wire.as_slice());
    for f in &frames {
        assert_eq!(reader.read_frame().unwrap().as_ref(), Some(f));
    }
    assert_eq!(reader.read_frame().unwrap(), None);

    for f in &frames {
        let mut one = Vec::new();
        f.encode(&mut one).unwrap();
        let len = u32::from_le_bytes([one[0], one[1], one[2], one[3]]) as usize;
        assert_eq!(
            len,
            one.len() - data::HEADER_LEN,
            "{:#04x}: len is the payload length",
            f.kind()
        );
        assert_eq!(one[4], f.kind());
        assert_eq!(&Frame::decode(one[4], &one[data::HEADER_LEN..]).unwrap(), f);
        assert_eq!(f.is_from_client(), f.kind() < 0x20);
    }
}

#[test]
fn c2_layouts_are_the_documented_byte_counts() {
    let size = |f: Frame| {
        let mut v = Vec::new();
        f.encode(&mut v).unwrap();
        v.len() - data::HEADER_LEN
    };
    assert_eq!(
        size(Frame::Attach(Attach {
            v: 1,
            pane_id: 1,
            cols: 1,
            rows: 1,
            cell_width_px: 1,
            cell_height_px: 1
        })),
        18
    );
    assert_eq!(
        size(Frame::Resize(Resize {
            cols: 1,
            rows: 1,
            cell_width_px: 1,
            cell_height_px: 1
        })),
        8
    );
    assert_eq!(
        size(Frame::FetchHistory(FetchHistory {
            start: -1,
            count: 1
        })),
        10
    );
    assert_eq!(size(Frame::Ack(Ack { seq: 1 })), 8);
    assert_eq!(size(Frame::Focus(Focus { focused: false })), 1);
    assert_eq!(size(Frame::Exit(Exit { code: 0 })), 4);
    assert_eq!(size(Frame::Bell), 0);
    let key = KeyEvent {
        key: 1,
        mods: Mods::empty(),
        consumed_mods: Mods::empty(),
        action: KeyAction::Press,
        composing: false,
        unshifted_codepoint: 0,
        text: "ab".to_owned(),
    };
    assert_eq!(size(Frame::Key(key)), 12 + 2);
    let mouse = MouseEvent {
        action: MouseAction::Press,
        button: MouseButton::Left,
        mods: Mods::empty(),
        col: 0,
        row: 0,
        x: 0.0,
        y: 0.0,
    };
    assert_eq!(size(Frame::Mouse(mouse)), 16);
    let one_cell = Row {
        index: 0,
        wrapped: false,
        cells: vec![Cell::default()],
    };
    let delta = |lines| {
        Frame::Delta(Delta {
            seq: 0,
            cursor: Cursor::default(),
            modes: Modes::empty(),
            scrollback_rows: 0,
            styles_added: vec![],
            lines,
        })
    };
    assert_eq!(
        size(delta(vec![one_cell])) - size(delta(vec![Row::default()])),
        7,
        "a plain cell is 7 bytes"
    );
}

#[test]
fn c2_max_size_frame_passes_and_oversized_frames_are_rejected() {
    let max = Frame::InputRaw(vec![b'x'; MAX_FRAME_LEN]);
    let mut wire = Vec::new();
    max.encode(&mut wire).unwrap();
    assert_eq!(wire.len(), MAX_FRAME_LEN + data::HEADER_LEN);
    assert_eq!(
        FrameReader::new(wire.as_slice()).read_frame().unwrap(),
        Some(max)
    );

    let mut out = vec![0xAA];
    let err = Frame::InputRaw(vec![b'x'; MAX_FRAME_LEN + 1])
        .encode(&mut out)
        .unwrap_err();
    assert!(matches!(err, Error::FrameTooLarge { len } if len == (MAX_FRAME_LEN + 1) as u64));
    assert_eq!(
        out,
        vec![0xAA],
        "a failed encode leaves the buffer untouched"
    );

    let mut header = ((MAX_FRAME_LEN + 1) as u32).to_le_bytes().to_vec();
    header.push(kind::INPUT_RAW);
    let err = FrameReader::new(header.as_slice())
        .read_frame()
        .unwrap_err();
    assert!(
        matches!(err, Error::FrameTooLarge { .. }),
        "the reader must refuse before reading the payload"
    );
    let mut header = u32::MAX.to_le_bytes().to_vec();
    header.push(kind::SNAPSHOT);
    assert!(matches!(
        FrameReader::new(header.as_slice()).read_frame(),
        Err(Error::FrameTooLarge { .. })
    ));
}

#[test]
fn c2_malformed_payloads_are_rejected() {
    let mut ack = Vec::new();
    Frame::Ack(Ack { seq: 1 }).encode(&mut ack).unwrap();
    let payload = &ack[data::HEADER_LEN..];
    assert!(matches!(
        Frame::decode(kind::ACK, &payload[..7]),
        Err(Error::Truncated { kind: 0x14 })
    ));
    let mut long = payload.to_vec();
    long.push(0);
    assert!(matches!(
        Frame::decode(kind::ACK, &long),
        Err(Error::TrailingBytes { extra: 1, .. })
    ));
    assert!(matches!(
        Frame::decode(0x30, &[]),
        Err(Error::UnknownFrameKind(0x30))
    ));
    assert!(matches!(
        Frame::decode(kind::FOCUS, &[2]),
        Err(Error::InvalidValue { field: "in", .. })
    ));
    assert!(matches!(
        Frame::decode(kind::FETCH_HISTORY, &[0, 0, 0, 0, 0, 0, 0, 0, 0xE9, 0x03]),
        Err(Error::InvalidValue {
            field: "history count",
            value: 1001,
            ..
        })
    ));
    let mut key = vec![176, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0];
    assert!(matches!(
        Frame::decode(kind::KEY, &key),
        Err(Error::InvalidValue { field: "key", .. })
    ));
    key[0] = 1;
    key[3] = 0b100;
    assert!(matches!(
        Frame::decode(kind::KEY, &key),
        Err(Error::InvalidValue { field: "mods", .. })
    ));
    key[3] = 0;
    key.extend_from_slice(&[0xFF, 0xFE]);
    assert!(matches!(
        Frame::decode(kind::KEY, &key),
        Err(Error::InvalidUtf8 { .. })
    ));
    let mut mouse = vec![0, 1, 0, 0, 0, 0, 0, 0];
    mouse.extend_from_slice(&f32::NAN.to_le_bytes());
    mouse.extend_from_slice(&0f32.to_le_bytes());
    assert!(matches!(
        Frame::decode(kind::MOUSE, &mouse),
        Err(Error::InvalidValue { field: "x", .. })
    ));
    assert!(
        Attrs::from_bits(6 << 3).is_none(),
        "underline kind 6 accepted"
    );
    assert!(Attrs::from_bits(1 << 11).is_none(), "attrs bit 11 accepted");
    assert!(
        CellFlags::from_bits(1 << 4).is_none(),
        "cell flag bit 4 accepted"
    );
    let snapshot_with = |mutate: &dyn Fn(&mut Vec<u8>)| {
        let mut wire = Vec::new();
        Frame::Snapshot(Snapshot {
            seq: 0,
            cols: 1,
            rows: 1,
            cursor: Cursor::default(),
            modes: Modes::empty(),
            scrollback_rows: 0,
            styles: vec![],
            lines: vec![Row {
                index: 0,
                wrapped: false,
                cells: vec![Cell::default()],
            }],
        })
        .encode(&mut wire)
        .unwrap();
        let mut payload = wire[data::HEADER_LEN..].to_vec();
        mutate(&mut payload);
        Frame::decode(kind::SNAPSHOT, &payload)
    };
    let cell_flags_at = 8 + 2 + 2 + 6 + 2 + 4 + 2 + 2 + 4 + 1 + 2 + 4 + 2;
    assert!(snapshot_with(&|_| {}).is_ok());
    assert!(matches!(
        snapshot_with(&|p| p[cell_flags_at] = 1 << 4),
        Err(Error::InvalidValue {
            field: "cell flags",
            ..
        })
    ));
    assert!(matches!(
        snapshot_with(&|p| p[cell_flags_at] = CellFlags::GRAPHEME.bits()),
        Err(Error::Truncated { .. })
    ));
    assert!(matches!(
        snapshot_with(&|p| p[16] = 9),
        Err(Error::InvalidValue {
            field: "cursor shape",
            ..
        })
    ));
}

#[test]
fn c2_encoding_refuses_inconsistent_graphemes() {
    let bad = Frame::History(History {
        start: -1,
        lines: vec![Row {
            index: -1,
            wrapped: false,
            cells: vec![Cell {
                codepoint: 'e' as u32,
                style: 0,
                flags: CellFlags::empty(),
                extra: vec![0x301],
            }],
        }],
        styles_added: vec![],
    });
    assert!(matches!(
        bad.encode(&mut Vec::new()),
        Err(Error::InvalidValue {
            field: "grapheme length",
            ..
        })
    ));
}

#[test]
fn c2_version_mismatch_is_detectable() {
    let mut wire = Vec::new();
    Frame::Attach(Attach {
        v: 2,
        pane_id: 1,
        cols: 80,
        rows: 24,
        cell_width_px: 8,
        cell_height_px: 19,
    })
    .encode(&mut wire)
    .unwrap();
    let Some(Frame::Attach(a)) = FrameReader::new(wire.as_slice()).read_frame().unwrap() else {
        panic!("not an attach");
    };
    assert!(matches!(
        version::check_version("C2", C2_VERSION, a.v),
        Err(Error::VersionMismatch {
            protocol: "C2",
            expected: 1,
            found: 2
        })
    ));
}

#[test]
fn c2_reader_reports_a_frame_cut_short() {
    let mut wire = Vec::new();
    Frame::Title("abc".to_owned()).encode(&mut wire).unwrap();
    wire.pop();
    assert!(matches!(
        FrameReader::new(wire.as_slice()).read_frame(),
        Err(Error::Io(_))
    ));
    assert!(matches!(
        FrameReader::new(&wire[..3]).read_frame(),
        Err(Error::Io(_))
    ));
}

#[test]
fn c1_types_carry_no_pty_bytes() {
    let byte_types = [
        "Vec<u8>",
        "&[u8]",
        "[u8;",
        "Bytes",
        "ByteBuf",
        "serde_bytes",
    ];
    for file in ["src/control.rs", "src/pane.rs"] {
        let text =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
        let mut rest = text.as_str();
        while let Some(at) = rest
            .find("struct ")
            .into_iter()
            .chain(rest.find("enum "))
            .min()
        {
            let after = &rest[at..];
            let Some(open) = after.find(['{', ';']) else {
                break;
            };
            if after.as_bytes()[open] == b';' {
                rest = &after[open + 1..];
                continue;
            }
            let mut depth = 0;
            let mut end = after.len();
            for (i, ch) in after[open..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let body = &after[..end];
            for t in byte_types {
                assert!(
                    !body.contains(t),
                    "{file}: a C1 type holds {t} (INV-2): {}",
                    &body[..body.len().min(80)]
                );
            }
            rest = &after[end..];
        }
    }
}
