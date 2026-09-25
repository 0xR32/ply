//! DeltaBuilder and Replica against a live Engine (spec 9.3): a Replica fed any sequence of Snapshots and Deltas
//! equals the Engine's grid, and each Delta carries exactly what changed.

#![cfg(feature = "engine")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Rng, attach, engine, pump, truth, view};
use ply_proto::data::{CellFlags, Color, Frame, Modes, Style};
use ply_term::{DeltaBuilder, Replica, Update};

/// One random burst of terminal output: text, SGR, cursor motion, erases, scrolling, wide and combined characters.
fn burst(rng: &mut Rng, cols: u16, rows: u16) -> Vec<u8> {
    const WORDS: &[&str] = &[
        "the",
        "render",
        "state",
        "delta",
        "pane",
        "日本語",
        "e\u{301}",
        "👨\u{200d}👩\u{200d}👧",
        "🇩🇪",
        "│",
        "─",
    ];
    let mut out = Vec::new();
    for _ in 0..rng.below(12) + 1 {
        match rng.below(16) {
            0..=4 => {
                out.extend_from_slice(WORDS[rng.below(WORDS.len() as u64) as usize].as_bytes())
            }
            5 => out.extend_from_slice(b" "),
            6 => out.extend_from_slice(b"\r\n"),
            7 => {
                let sgr = match rng.below(6) {
                    0 => format!("\x1b[3{}m", rng.below(8)),
                    1 => format!("\x1b[48;5;{}m", rng.below(256)),
                    2 => format!(
                        "\x1b[38;2;{};{};{}m",
                        rng.below(256),
                        rng.below(256),
                        rng.below(256)
                    ),
                    3 => format!("\x1b[{}m", [1, 2, 3, 4, 5, 7, 9, 53][rng.below(8) as usize]),
                    4 => "\x1b[4:3;58;5;99m".to_owned(),
                    _ => "\x1b[0m".to_owned(),
                };
                out.extend_from_slice(sgr.as_bytes());
            }
            8 => out.extend_from_slice(
                format!(
                    "\x1b[{};{}H",
                    rng.below(u64::from(rows)) + 1,
                    rng.below(u64::from(cols)) + 1
                )
                .as_bytes(),
            ),
            9 => out.extend_from_slice(
                [&b"\x1b[K"[..], b"\x1b[1K", b"\x1b[2K", b"\x1b[J"][rng.below(4) as usize],
            ),
            10 => out.extend_from_slice(b"\x1b[44m\x1b[K\x1b[0m"),
            11 => out.extend_from_slice(
                [&b"\x1b[L"[..], b"\x1b[M", b"\x1b[2S", b"\x1b[T"][rng.below(4) as usize],
            ),
            12 => out.extend_from_slice(b"\x08\t"),
            13 => out.extend_from_slice(
                [
                    &b"\x1b[?25l"[..],
                    b"\x1b[?25h",
                    b"\x1b[5 q",
                    b"\x1b[?2004h",
                    b"\x1b[?1000h",
                ][rng.below(5) as usize],
            ),
            14 => {
                let line: String = (0..cols + rng.below(8) as u16)
                    .map(|i| char::from(b'a' + (i % 26) as u8))
                    .collect();
                out.extend_from_slice(line.as_bytes());
            }
            _ => {
                out.extend_from_slice([&b"\x1b[?1049h"[..], b"\x1b[?1049l"][rng.below(2) as usize])
            }
        }
    }
    out
}

#[test]
fn a_replica_fed_random_snapshots_and_deltas_equals_the_engine() {
    for seed in 1..=24u64 {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ seed);
        let (mut cols, mut rows) = (20 + rng.below(40) as u16, 5 + rng.below(15) as u16);
        let mut e = engine(cols, rows);
        let (mut a, mut ra) = attach(&mut e);
        let (mut b, mut rb) = attach(&mut e);
        for step in 0..300 {
            e.write(&burst(&mut rng, cols, rows));
            if rng.chance(3) {
                cols = 20 + rng.below(40) as u16;
                rows = 5 + rng.below(15) as u16;
                e.resize(cols, rows, 8, 16).unwrap();
            }
            // Client A takes most updates, client B far fewer, so B's Deltas accumulate many generations.
            if rng.chance(70) {
                pump(&mut e, &mut a, &mut ra);
            }
            if rng.chance(15) {
                pump(&mut e, &mut b, &mut rb);
            }
            if rng.chance(2) {
                ra.apply_snapshot(&a.snapshot(&mut e).unwrap()).unwrap();
            }
            if step % 25 == 24 {
                pump(&mut e, &mut a, &mut ra);
                pump(&mut e, &mut b, &mut rb);
                let want = view(&truth(&mut e));
                assert_eq!(view(&ra), want, "seed {seed} step {step}: client A");
                assert_eq!(view(&rb), want, "seed {seed} step {step}: client B");
                common::assert_matches_formatter(&mut e, &ra, &format!("seed {seed} step {step}"));
            }
        }
    }
}

#[test]
fn a_delta_carries_only_the_rows_that_changed() {
    let mut e = engine(40, 10);
    let (mut client, mut replica) = attach(&mut e);
    assert!(
        client.delta(&mut e).unwrap().is_none(),
        "no change, no Delta (R-R21)"
    );

    e.write(b"line zero\r\nline one\r\nline two");
    let Some(Update::Delta(d)) = client.delta(&mut e).unwrap() else {
        panic!("expected a Delta")
    };
    assert_eq!(
        d.lines.iter().map(|r| r.index).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    replica.apply_delta(&d).unwrap();
    assert_eq!(replica.row_text(1), "line one");

    e.write(b"\x1b[8;5Hhello");
    let Some(Update::Delta(d)) = client.delta(&mut e).unwrap() else {
        panic!("expected a Delta")
    };
    assert_eq!(
        d.lines.iter().map(|r| r.index).collect::<Vec<_>>(),
        vec![2, 7],
        "the row the cursor left is dirty too"
    );
    assert_eq!((d.cursor.col, d.cursor.row), (9, 7));
    replica.apply_delta(&d).unwrap();

    e.write(b"\x1b[?25l");
    let Some(Update::Delta(d)) = client.delta(&mut e).unwrap() else {
        panic!("a hidden cursor is a change")
    };
    assert!(!d.cursor.visible);
    assert!(!d.modes.contains(Modes::CURSOR_VISIBLE));
    replica.apply_delta(&d).unwrap();
    assert!(client.delta(&mut e).unwrap().is_none());

    e.write(b"\x1b[10;1H\n");
    let Some(Update::Delta(d)) = client.delta(&mut e).unwrap() else {
        panic!("expected a Delta")
    };
    assert_eq!(d.lines.len(), 10, "a scroll redraws every row");
    assert_eq!(d.scrollback_rows, 1);
}

#[test]
fn styles_are_interned_per_client_and_sent_once() {
    let mut e = engine(30, 4);
    let (mut client, _) = attach(&mut e);
    e.write(b"\x1b[31mred\x1b[0m plain \x1b[31mred\x1b[1;38;2;1;2;3mX\x1b[0m");
    let Some(Update::Delta(d)) = client.delta(&mut e).unwrap() else {
        panic!()
    };
    let red = Style {
        fg: Color::Indexed(1),
        ..Style::default()
    };
    assert_eq!(d.styles_added.len(), 2);
    assert_eq!(d.styles_added[0].style, red);
    assert_eq!(d.styles_added[1].style.fg, Color::Rgb(1, 2, 3));
    let row = &d.lines[0];
    assert_eq!(row.cells[0].style, d.styles_added[0].id);
    assert_eq!(
        row.cells[3].style, 0,
        "the default style is id 0 and never sent"
    );
    assert_eq!(row.cells[10].style, d.styles_added[0].id);

    e.write(b"\r\n\x1b[31magain\x1b[0m");
    let Some(Update::Delta(d)) = client.delta(&mut e).unwrap() else {
        panic!()
    };
    assert!(d.styles_added.is_empty(), "a known style is not sent again");

    let mut fresh = DeltaBuilder::new();
    let snapshot = fresh.snapshot(&mut e).unwrap();
    assert_eq!(
        snapshot.styles.len(),
        2,
        "a Snapshot carries the complete table of its client"
    );
}

#[test]
fn cells_keep_symbolic_colours_wide_spacers_and_graphemes() {
    let mut e = engine(40, 8);
    e.write("A\u{4e2d}B\r\n".as_bytes());
    e.write("e\u{301}\r\n".as_bytes());
    e.write("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}|\u{1f1e9}\u{1f1ea}|\r\n".as_bytes());
    e.write(b"\x1b[1;3;4;9;53m\x1b[31;48;5;200;58;2;1;2;3mX\x1b[0m\x1b[4:3mY\x1b[0m\r\n");
    e.write(b"\x1b[44m\x1b[K\x1b[0m\r\n");
    e.write(b"\x1b[2;5;7;8mI\x1b[0m");
    let r = truth(&mut e);

    let row0 = r.resolved_row(0).unwrap();
    assert_eq!(row0[1].codepoint, 0x4e2d);
    assert!(row0[1].flags.contains(CellFlags::WIDE));
    assert!(row0[2].flags.contains(CellFlags::SPACER));
    assert_eq!(r.row_text(0), "A\u{4e2d}B");

    let row1 = r.resolved_row(1).unwrap();
    assert_eq!(
        (row1[0].codepoint, row1[0].extra.clone()),
        ('e' as u32, vec![0x301])
    );
    assert!(row1[0].flags.contains(CellFlags::GRAPHEME));

    let row2 = r.resolved_row(2).unwrap();
    assert_eq!(
        row2[0].codepoint, 0x1f468,
        "mode 2027 keeps the family emoji in one cell (R22)"
    );
    assert_eq!(row2[0].extra, vec![0x200d, 0x1f469, 0x200d, 0x1f467]);
    assert!(row2[0].flags.contains(CellFlags::WIDE));
    assert_eq!(
        r.row_text(2),
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}|\u{1f1e9}\u{1f1ea}|"
    );

    let x = r.resolved_row(3).unwrap()[0].style;
    assert_eq!(
        (x.fg, x.bg, x.underline_color),
        (Color::Indexed(1), Color::Indexed(200), Color::Rgb(1, 2, 3))
    );
    let attrs = ply_proto::data::Attrs::BOLD
        | ply_proto::data::Attrs::ITALIC
        | ply_proto::data::Attrs::STRIKETHROUGH
        | ply_proto::data::Attrs::OVERLINE;
    assert!(x.attrs.contains(attrs));
    assert_eq!(x.attrs.underline(), ply_proto::data::Underline::Single);
    assert_eq!(
        r.resolved_row(3).unwrap()[1].style.attrs.underline(),
        ply_proto::data::Underline::Curly
    );

    let erased = r.resolved_row(4).unwrap();
    assert_eq!(
        erased.len(),
        40,
        "a background-only erase is not a trailing default blank"
    );
    assert_eq!(
        erased[20].style.bg,
        Color::Indexed(4),
        "the erase keeps its palette background symbolically"
    );

    let i = r.resolved_row(5).unwrap()[0].style.attrs;
    use ply_proto::data::Attrs;
    assert!(i.contains(Attrs::FAINT | Attrs::BLINK | Attrs::INVERSE | Attrs::INVISIBLE));
}

#[test]
fn a_resize_or_a_new_client_gets_a_snapshot() {
    let mut e = engine(20, 5);
    let mut client = DeltaBuilder::new();
    assert!(
        matches!(client.delta(&mut e).unwrap(), Some(Update::Snapshot(_))),
        "the first frame is a Snapshot"
    );
    e.write(b"0123456789abcdefghijKLMNO");
    let mut replica = Replica::new();
    replica
        .apply_snapshot(&client.snapshot(&mut e).unwrap())
        .unwrap();
    assert_eq!(
        replica.screen_text()[..2],
        ["0123456789abcdefghij".to_owned(), "KLMNO".to_owned()]
    );
    assert!(replica.row(0).unwrap().wrapped);

    e.resize(40, 5, 8, 16).unwrap();
    let Some(Update::Snapshot(s)) = client.delta(&mut e).unwrap() else {
        panic!("a resize needs a Snapshot")
    };
    replica.apply_snapshot(&s).unwrap();
    assert_eq!(replica.size(), (40, 5));
    assert_eq!(
        replica.row_text(0),
        "0123456789abcdefghijKLMNO",
        "the primary screen reflows (R-R12)"
    );
    assert!(
        s.seq > 1,
        "sequence numbers keep increasing across Snapshots"
    );
}

#[test]
fn history_pages_come_from_scrollback_with_negative_indexes() {
    let mut e = engine(30, 5);
    for i in 0..40 {
        e.write(format!("\x1b[3{}mline {i:02}\x1b[0m\r\n", i % 8).as_bytes());
    }
    let (mut client, mut replica) = attach(&mut e);
    assert_eq!(replica.scrollback_rows(), 36);
    let h = client.history(&mut e, -36, 10).unwrap();
    assert_eq!(h.start, -36);
    assert_eq!(h.lines.len(), 10);
    replica.apply(&Frame::History(h)).unwrap();
    assert_eq!(
        ply_term::cells_text(&replica.history_row(-36).unwrap().cells),
        "line 00"
    );
    assert_eq!(
        ply_term::cells_text(&replica.history_row(-27).unwrap().cells),
        "line 09"
    );
    assert_eq!(replica.resolved_row(0).map(|r| r.len()), Some(7));

    let tail = client.history(&mut e, -3, 1000).unwrap();
    assert_eq!(
        (tail.start, tail.lines.len()),
        (-3, 3),
        "clipped to what exists"
    );
    assert_eq!(ply_term::cells_text(&tail.lines[2].cells), "line 35");
    let none = client.history(&mut e, -100, 10).unwrap();
    assert!(none.lines.is_empty());
    assert!(none.styles_added.is_empty());
}

#[test]
fn a_history_page_never_exceeds_one_frame() {
    let mut e = engine(168, 5);
    let row: String = (0..168)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    for _ in 0..1100 {
        e.write(format!("{row}\r\n").as_bytes());
    }
    let mut client = DeltaBuilder::new();
    let h = client.history(&mut e, -1050, 1000).unwrap();
    assert!(
        !h.lines.is_empty() && h.lines.len() < 1000,
        "1000 full rows of 168 cells exceed 1 MiB: {}",
        h.lines.len()
    );
    let mut wire = Vec::new();
    Frame::History(h).encode(&mut wire).unwrap();
    assert!(wire.len() <= ply_proto::data::MAX_FRAME_LEN + ply_proto::data::HEADER_LEN);
}

#[test]
fn a_styled_history_page_never_exceeds_one_frame() {
    let mut e = engine(168, 5);
    for i in 0..1100u32 {
        let mut line = String::new();
        for w in 0..16u32 {
            let (r, g, b) = (
                (i * 7 + w) % 256,
                (i * 13 + w * 17) % 256,
                (i * 3 + w * 29) % 256,
            );
            line.push_str(&format!("\x1b[38;2;{r};{g};{b}mword{w:02}{i:04}\x1b[0m"));
        }
        line.push_str("\x1b[48;5;4m........\x1b[0m");
        e.write(format!("{line}\r\n").as_bytes());
    }
    let (mut client, mut replica) = attach(&mut e);
    let page = client.history(&mut e, -1050, 1000).unwrap();
    assert!(
        page.styles_added.len() > 1000,
        "a style per word: {}",
        page.styles_added.len()
    );
    assert!(
        !page.lines.is_empty() && page.lines.len() < 1000,
        "cut to fit: {} rows",
        page.lines.len()
    );
    let frame = Frame::History(page);
    let mut wire = Vec::new();
    frame.encode(&mut wire).unwrap();
    assert!(wire.len() - ply_proto::data::HEADER_LEN <= ply_proto::data::MAX_FRAME_LEN);
    replica.apply(&frame).unwrap();
    let Frame::History(page) = frame else {
        unreachable!()
    };
    let next = client
        .history(&mut e, -1050 + page.lines.len() as i64, 1000)
        .unwrap();
    assert_eq!(
        next.start,
        -1050 + page.lines.len() as i64,
        "the next page continues where the cut one stopped"
    );
    replica.apply(&Frame::History(next)).unwrap();
}
