//! The Engine's contract with plyd: query answers from the palette (R-R4), effect events (C8, R-R11), DEC 2026
//! (R-R18), idle compression (R-R22), search, and state that survives a save and restore.

#![cfg(feature = "engine")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{engine, palette, screen};
use ply_proto::data::Modes;
use ply_term::{ClipboardContent, Compression, Engine, Notification, ProgressState, XTVERSION};

fn answer(e: &mut Engine, query: &[u8]) -> String {
    String::from_utf8(e.write(query).reply).unwrap()
}

#[test]
fn colour_and_device_queries_are_answered_from_the_palette() {
    let mut e = engine(120, 40);
    assert_eq!(
        answer(&mut e, b"\x1b]10;?\x1b\\"),
        "\x1b]10;rgb:d4d4/d4d4/d8d8\x1b\\"
    );
    assert_eq!(
        answer(&mut e, b"\x1b]11;?\x07"),
        "\x1b]11;rgb:1818/1818/1b1b\x07"
    );
    assert_eq!(
        answer(&mut e, b"\x1b]4;1;?\x07"),
        "\x1b]4;1;rgb:e0e0/6c6c/7575\x07"
    );
    assert_eq!(
        answer(&mut e, b"\x1b]12;?\x07"),
        "\x1b]12;rgb:fafa/fafa/fafa\x07"
    );
    assert_eq!(
        answer(&mut e, b"\x1b]4;1;#ff0000\x07\x1b]4;1;?\x07"),
        "\x1b]4;1;rgb:ffff/0000/0000\x07"
    );
    assert_eq!(
        answer(&mut e, b"\x1b]104;1\x07\x1b]4;1;?\x07"),
        "\x1b]4;1;rgb:e0e0/6c6c/7575\x07",
        "OSC 104 restores ply's value"
    );
    assert_eq!(answer(&mut e, b"\x1b[c"), "\x1b[?62;22c");
    assert_eq!(answer(&mut e, b"\x1b[5;7H\x1b[6n"), "\x1b[5;7R");
    assert_eq!(answer(&mut e, b"\x1b[5n"), "\x1b[0n");
    assert_eq!(answer(&mut e, b"\x1b[?u"), "\x1b[?0u");
    assert_eq!(answer(&mut e, b"\x1b[>1u\x1b[?u\x1b[<u"), "\x1b[?1u");
    assert_eq!(answer(&mut e, b"\x1b[18t"), "\x1b[8;40;120t");
    assert_eq!(answer(&mut e, b"\x1b[16t"), "\x1b[6;16;8t");
    assert_eq!(
        answer(&mut e, b"\x1b[?996n"),
        "\x1b[?997;1n",
        "dark scheme from the palette"
    );
    assert_eq!(
        answer(&mut e, b"\x1b[>q"),
        format!("\x1bP>|{XTVERSION}\x1b\\")
    );
    assert_eq!(XTVERSION, concat!("ply ", env!("CARGO_PKG_VERSION")));
    assert_eq!(
        answer(&mut e, b"\x1b[?2027$p"),
        "\x1b[?2027;1$y",
        "grapheme clustering is on (R22)"
    );
    assert_eq!(answer(&mut e, b"\x1b[?2026$p"), "\x1b[?2026;2$y");
    assert_eq!(
        answer(&mut e, b"\x1bP+q544e\x1b\\"),
        "\x1bP1+r544E=787465726D2D323536636F6C6F72\x1b\\",
        "TN = xterm-256color"
    );
    assert_eq!(
        answer(&mut e, b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\"),
        "",
        "Kitty graphics are off"
    );
    assert_eq!(answer(&mut e, b"\x05"), "", "ENQ is unanswered");
    assert_eq!(
        answer(&mut e, b"\x1b]2;x\x07\x1b[21t"),
        "",
        "title reports stay off"
    );
}

#[test]
fn the_codex_startup_probe_is_answered_at_once() {
    let mut e = engine(120, 40);
    let probe = b"\x1b[?2004h\x1b[>4;0m\x1b[>5u\x1b[?1004h\x1b[6n\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[?u\x1b[c\x1b[?2026h";
    let reply = answer(&mut e, probe);
    for part in [
        "\x1b[1;1R",
        "\x1b]10;rgb:d4d4/d4d4/d8d8\x1b\\",
        "\x1b]11;rgb:1818/1818/1b1b\x1b\\",
        "\x1b[?5u",
        "\x1b[?62;22c",
    ] {
        assert!(reply.contains(part), "{part:?} missing from {reply:?}");
    }
}

#[test]
fn size_reports_wait_for_a_cell_size() {
    let mut e = Engine::new(3, 80, 24, 1000, &palette()).unwrap();
    assert_eq!(e.cell_size(), (0, 0));
    assert_eq!(
        answer(&mut e, b"\x1b[14t"),
        "",
        "no cell size yet, so no answer"
    );
    let out = e.resize(100, 30, 9, 18).unwrap();
    assert!(out.reply.is_empty());
    assert_eq!(answer(&mut e, b"\x1b[14t"), "\x1b[4;540;900t");
    e.write(b"\x1b[?2048h");
    let report = String::from_utf8(e.resize(90, 30, 9, 18).unwrap().reply).unwrap();
    assert_eq!(
        report, "\x1b[48;30;90;540;810t",
        "mode 2048 reports the resize in band"
    );
}

#[test]
fn effects_arrive_as_output_events() {
    let mut e = engine(80, 24);
    let out = e.write(b"\x1b]0;first\x07\x1b]2;second\x1b\\\x07ding\x07");
    assert_eq!(
        out.title.as_deref(),
        Some("second"),
        "titles are coalesced to the last one"
    );
    assert_eq!(out.bells, 2);
    assert_eq!(e.title(), "second");
    let out = e.write(b"\x1b]7;file://example-host/Users/example/my%20dir\x1b\\");
    assert_eq!(
        out.pwd.as_deref(),
        Some("file://example-host/Users/example/my%20dir"),
        "OSC 7 stays a raw URI"
    );
    assert!(e.write(b"plain").is_empty());

    let out =
        e.write(b"\x1b]9;Codex wants to edit a.txt\x07\x1b]777;notify;Title here;Body here\x1b\\");
    assert_eq!(
        out.notifications,
        vec![
            Notification {
                title: String::new(),
                body: "Codex wants to edit a.txt".to_owned()
            },
            Notification {
                title: "Title here".to_owned(),
                body: "Body here".to_owned()
            },
        ]
    );
    for swallowed in ["12 tests pass", "5 files changed", "1;500", "4;1;50"] {
        let out = e.write(format!("\x1b]9;{swallowed}\x07").as_bytes());
        assert!(
            out.notifications.is_empty(),
            "{swallowed:?} is a ConEmu command (ADR-0005)"
        );
    }
    let out = e.write(b"\x1b]9;4;1;50\x07");
    let progress = out.progress.unwrap();
    assert_eq!(
        (progress.state, progress.percent),
        (ProgressState::Set, Some(50))
    );
    assert!(e.write(b"\x1b]9;split across").notifications.is_empty());
    assert_eq!(
        e.write(b" two writes\x07").notifications[0].body,
        "split across two writes"
    );

    let out = e.write(b"\x1b]52;c;aGVsbG8gcGx5\x07");
    assert_eq!(
        out.clipboard_writes[0].contents,
        vec![ClipboardContent {
            mime: "text/plain".to_owned(),
            data: b"hello ply".to_vec()
        }]
    );
    assert_eq!(
        e.write(b"\x1b]52;c;?\x07").reply,
        b"",
        "clipboard reads are refused (R-R11)"
    );
}

#[test]
fn synchronized_output_is_polled_and_capped() {
    let mut e = engine(40, 5);
    assert!(!e.is_synchronized_update_open());
    e.write(b"\x1b[?2026h");
    assert!(e.is_synchronized_update_open());
    e.write(b"\x1b[?2026l");
    assert!(!e.is_synchronized_update_open());
    e.write(b"\x1b[?2026h");
    e.end_synchronized_update().unwrap();
    assert!(!e.is_synchronized_update_open(), "plyd's 150 ms cap");
    e.write(b"\x1b[?2026h");
    e.resize(41, 5, 8, 16).unwrap();
    assert!(!e.is_synchronized_update_open(), "a resize ends the update");
}

#[test]
fn modes_report_what_the_view_needs() {
    let mut e = engine(40, 5);
    assert_eq!(e.modes(), Modes::CURSOR_VISIBLE);
    e.write(b"\x1b[?1049h\x1b[?25l\x1b[?1002h\x1b[?2004h");
    assert_eq!(
        e.modes(),
        Modes::ALT_SCREEN | Modes::MOUSE_REPORTING | Modes::BRACKETED_PASTE
    );
}

#[test]
fn scrollback_is_capped_by_lines_not_bytes() {
    let mut e = Engine::new(1, 168, 50, 10_000, &palette()).unwrap();
    for i in 0..12_000 {
        e.write(format!("{i:08} {}\r\n", "x".repeat(150)).as_bytes());
    }
    let rows = e.scrollback_rows();
    assert!(
        (10_000..=10_300).contains(&rows),
        "the 10 kB byte default is gone and the cap keeps >= 10 000 rows: {rows}"
    );
}

#[test]
fn idle_compression_runs_to_completion() {
    let mut e = Engine::new(1, 168, 50, 10_000, &palette()).unwrap();
    let before = e.compression_activity();
    for i in 0..3000 {
        e.write(format!("\x1b[3{}m{i:08}\x1b[0m {}\r\n", i % 8, "y".repeat(150)).as_bytes());
    }
    let active = e.compression_activity();
    assert_ne!(before, active, "writes move the activity token");
    let mut steps = 0;
    let result = loop {
        steps += 1;
        match e.compress_idle().unwrap() {
            Compression::Pending if steps < 100_000 => continue,
            other => break other,
        }
    };
    assert_eq!(result, Compression::Complete, "macOS builds compress");
    assert_eq!(
        e.compression_activity(),
        active,
        "compressing does not move the token"
    );
    let top = e.scrollback_base() + u64::from(e.scrollback_rows());
    assert_eq!(
        e.scroll_history(top - 2000, 1).unwrap().lines.len(),
        1,
        "compressed history still reads"
    );
}

#[test]
fn search_finds_matches_in_scrollback_newest_first() {
    let mut e = Engine::new(1, 80, 24, 10_000, &palette()).unwrap();
    for i in 0..5000 {
        e.write(
            format!(
                "line {i} {}\r\n",
                if i % 1000 == 7 { "NEEDLE here" } else { "hay" }
            )
            .as_bytes(),
        );
    }
    let matches = e.search("needle").unwrap();
    assert_eq!(matches.len(), 5);
    let newest = matches[0];
    let history = e.scroll_history(newest.start_line, 1).unwrap();
    let text = ply_term::cells_text(&history.lines[0].cells);
    assert_eq!(text, "line 4007 NEEDLE here");
    assert_eq!((newest.start_col, newest.end_col), (10, 15));
    assert!(
        matches
            .windows(2)
            .all(|w| w[0].start_line > w[1].start_line)
    );
    assert!(e.search("").unwrap().is_empty());
    assert!(e.search("absent").unwrap().is_empty());
}

#[test]
fn saved_state_restores_screen_modes_and_an_unfinished_sequence() {
    let mut e = engine(120, 40);
    for i in 0..3000 {
        e.write(format!("\x1b[3{}mrow {i}\x1b[0m\r\n", i % 8).as_bytes());
    }
    e.write(b"\x1b]2;saved title\x07\x1b]7;file://example-host/tmp\x07\x1b[?2004h\x1b[>1u\x1b]4;1;#123456\x07\x1b[5;5Hcursor");
    e.write(b"\x1b]9;half an O");
    let before = screen(&mut e);
    let history = e.scroll_history(0, 5).unwrap();
    let state = e.save().unwrap();
    drop(e);

    let mut r = Engine::restore(9, &state, 10_000, &common::palette()).unwrap();
    let out = r.write(b"SC completes\x07");
    assert_eq!(
        out.notifications[0].body, "half an OSC completes",
        "the unfinished sequence survives"
    );
    assert_eq!(screen(&mut r), before);
    assert_eq!(r.scroll_history(0, 5).unwrap().lines, history.lines);
    assert_eq!(r.title(), "saved title");
    assert_eq!(r.pwd(), "file://example-host/tmp");
    assert!(r.modes().contains(Modes::BRACKETED_PASTE));
    assert_eq!(answer(&mut r, b"\x1b[?u"), "\x1b[?1u");
    assert_eq!(
        answer(&mut r, b"\x1b]4;1;?\x07"),
        "\x1b]4;1;rgb:1212/3434/5656\x07",
        "an OSC 4 override survives"
    );
    assert_eq!(
        answer(&mut r, b"\x1b]10;?\x07"),
        "\x1b]10;rgb:d4d4/d4d4/d8d8\x07"
    );
    assert!(Engine::restore(9, b"not a snapshot", 10_000, &common::palette()).is_err());
}

#[test]
fn zero_sizes_are_refused() {
    assert!(Engine::new(1, 0, 24, 100, &palette()).is_err());
    let mut e = engine(10, 2);
    assert!(e.resize(10, 0, 8, 16).is_err());
    assert_eq!(e.size(), (10, 2));
}
