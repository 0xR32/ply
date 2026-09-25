//! Recorded Claude Code and Codex byte streams replay through the Engine against stored row snapshots (spec 9.3).
//!
//! `fixtures/claude-session.bytes` is S2b run 3 (claude 2.1.282, approve a Write, a TodoWrite plan, `/exit`),
//! recorded at 80 × 24; `fixtures/codex-session.bytes` is S3b `raw_approve` (codex 0.156.1, approve an edit),
//! recorded at 120 × 40. Both are the raw pty output of `.superpowers/plan/evidence/{s2b/logs,s3b}` scrubbed for
//! INV-11: the capture machine's scratch-directory path (its user name, organisation directory and session UUID)
//! was replaced, byte for byte, by the same-length neutral `/private/tmp/example-01/-Users-example-Work-Examples/
//! 00000000-0000-4000-8000-000000000000/scratchpad`, including the fragments Codex's redraw writes one cursor-
//! positioned piece at a time; nothing else changed, so every escape sequence and cursor movement is the agents'
//! own. The scrub script is kept with the WP3 report outside the repository.
//!
//! `fixtures/*.screen` hold the expected final screen: cursor, modes, every row's text and its style runs, and the
//! scrollback. After an intended change in libghostty-vt or ply-term, regenerate them with
//! `PLY_BLESS=1 cargo test -p ply-term --features engine --test replay` and review the diff.

#![cfg(feature = "engine")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fmt::Write as _;
use std::path::PathBuf;

use common::{assert_matches_formatter, attach, palette, pump, view};
use ply_proto::data::{Color, Style};
use ply_term::{Engine, EngineOutput, Replica};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn colour(c: Color) -> String {
    match c {
        Color::Default => "-".to_owned(),
        Color::Indexed(i) => format!("{i}"),
        Color::Rgb(r, g, b) => format!("{r:02x}{g:02x}{b:02x}"),
    }
}

fn describe(s: &Style) -> String {
    format!(
        "fg={} bg={} ul={} attrs={:#06x}",
        colour(s.fg),
        colour(s.bg),
        colour(s.underline_color),
        s.attrs.bits()
    )
}

/// A stable, reviewable dump of what a client shows: cursor, modes, rows with style runs, scrollback text.
fn dump(engine: &mut Engine, replica: &Replica) -> String {
    let mut out = String::new();
    let (cols, rows) = replica.size();
    let c = replica.cursor();
    writeln!(out, "size {cols}x{rows}").unwrap();
    writeln!(
        out,
        "cursor {},{} {:?} visible={} blinking={}",
        c.col, c.row, c.shape, c.visible, c.blinking
    )
    .unwrap();
    writeln!(out, "modes {:#06x}", replica.modes().bits()).unwrap();
    writeln!(out, "title {:?}", engine.title()).unwrap();
    for y in 0..rows {
        let row = replica.row(y).unwrap();
        writeln!(
            out,
            "{y:02}{}{}",
            if row.wrapped { '+' } else { '|' },
            replica.row_text(y)
        )
        .unwrap();
        let cells = replica.resolved_row(y).unwrap();
        let mut runs = Vec::new();
        let mut x = 0;
        while x < cells.len() {
            let style = cells[x].style;
            let start = x;
            while x < cells.len() && cells[x].style == style {
                x += 1;
            }
            if style != Style::default() {
                runs.push(format!("{start}+{} {}", x - start, describe(&style)));
            }
        }
        if !runs.is_empty() {
            writeln!(out, "{y:02}~ {}", runs.join("; ")).unwrap();
        }
    }
    let scrollback = engine.scrollback_rows();
    writeln!(out, "scrollback {scrollback}").unwrap();
    let history = engine
        .scroll_history(engine.scrollback_base(), 1000)
        .unwrap();
    for (i, row) in history.lines.iter().enumerate() {
        writeln!(
            out,
            "{:+05}|{}",
            i as i64 - i64::from(scrollback),
            ply_term::cells_text(&row.cells)
        )
        .unwrap();
    }
    out
}

struct Replayed {
    engine: Engine,
    replica: Replica,
    output: Vec<EngineOutput>,
    deltas: usize,
    oracle_checks: usize,
}

fn replay(bytes: &[u8], cols: u16, rows: u16, chunk: usize) -> Replayed {
    let mut engine = Engine::new(11, cols, rows, 10_000, &palette()).unwrap();
    engine.resize(cols, rows, 8, 16).unwrap();
    let (mut client, mut replica) = attach(&mut engine);
    let mut output = Vec::new();
    let mut deltas = 0;
    let mut oracle_checks = 0;
    for (i, piece) in bytes.chunks(chunk).enumerate() {
        output.push(engine.write(piece));
        if pump(&mut engine, &mut client, &mut replica) {
            deltas += 1;
        }
        if i % 4 == 3 {
            assert_matches_formatter(&mut engine, &replica, &format!("after chunk {i}"));
            oracle_checks += 1;
        }
    }
    pump(&mut engine, &mut client, &mut replica);
    assert_matches_formatter(&mut engine, &replica, "at the end");
    Replayed {
        engine,
        replica,
        output,
        deltas,
        oracle_checks: oracle_checks + 1,
    }
}

fn check_against_stored(name: &str, cols: u16, rows: u16) -> Replayed {
    let bytes = std::fs::read(fixture(name)).unwrap();
    let mut run = replay(&bytes, cols, rows, 1024);
    assert!(
        run.deltas > 10,
        "{name}: the replay drove {} Deltas",
        run.deltas
    );
    assert!(
        run.oracle_checks > 4,
        "{name}: {} formatter checkpoints",
        run.oracle_checks
    );
    assert_eq!(
        view(&run.replica),
        view(&common::truth(&mut run.engine)),
        "{name}: the Delta-fed replica equals the engine"
    );

    let mut whole = replay(&bytes, cols, rows, bytes.len());
    assert_eq!(
        view(&whole.replica),
        view(&run.replica),
        "{name}: the final screen does not depend on the write size"
    );
    let got = dump(&mut run.engine, &run.replica);
    assert_eq!(got, dump(&mut whole.engine, &whole.replica));

    let stored = fixture(&name.replace(".bytes", ".screen"));
    if std::env::var_os("PLY_BLESS").is_some() {
        std::fs::write(&stored, &got).unwrap();
    }
    let want = std::fs::read_to_string(&stored)
        .unwrap_or_else(|e| panic!("{}: {e}; bless it with PLY_BLESS=1", stored.display()));
    assert_eq!(
        got,
        want,
        "{name}: the screen differs from {}",
        stored.display()
    );
    run
}

fn replies(run: &Replayed) -> String {
    String::from_utf8(run.output.iter().flat_map(|o| o.reply.clone()).collect()).unwrap()
}

#[test]
fn the_claude_session_replays_to_its_stored_screen() {
    let run = check_against_stored("claude-session.bytes", 80, 24);
    let answered = replies(&run);
    assert!(
        answered.contains("\x1bP>|ply "),
        "Claude Code's XTVERSION probe gets ply's name: {answered:?}"
    );
    assert!(
        answered.contains("\x1b[?5u"),
        "the kitty keyboard query reports the flags Claude Code pushed"
    );
    assert!(answered.contains("\x1b[?62;22c"), "DA1 is answered");
    assert!(
        run.output.iter().any(|o| o.title.is_some()),
        "Claude Code sets the title"
    );
}

#[test]
fn the_codex_session_replays_to_its_stored_screen() {
    let run = check_against_stored("codex-session.bytes", 120, 40);
    let answered = replies(&run);
    for probe in [
        "\x1b[1;1R",
        "\x1b]10;rgb:d4d4/d4d4/d8d8\x1b\\",
        "\x1b]11;rgb:1818/1818/1b1b\x1b\\",
        "\x1b[?62;22c",
    ] {
        assert!(
            answered.contains(probe),
            "Codex's startup probe {probe:?} is answered: {answered:?}"
        );
    }
    let bodies: Vec<&str> = run
        .output
        .iter()
        .flat_map(|o| &o.notifications)
        .map(|n| n.body.as_str())
        .collect();
    assert!(
        bodies.iter().any(|b| b.starts_with("Codex wants to edit")),
        "the approval OSC 9 arrives (C8): {bodies:?}"
    );
    assert!(
        bodies.iter().any(|b| b.starts_with("Created [a.txt]")),
        "the turn-complete OSC 9 arrives: {bodies:?}"
    );
    let titles = run.output.iter().filter(|o| o.title.is_some()).count();
    assert!(titles > 10, "Codex animates its title ({titles} changes)");
}
