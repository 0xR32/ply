//! Memory per pane at 10 000 lines × 168 columns (spec P4: ≤ 15 MB per pane, read after 10 minutes idle, so after
//! plyd's idle compressor ran; Ruling R19). Six panes are filled at once and the process footprint (`phys_footprint`,
//! what `vmmap` and `footprint` report) is divided by six; each pane also holds a DeltaBuilder after one Snapshot.
//! Run with `--nocapture` to see the numbers; the test skips when macOS's `footprint` tool is missing.

#![cfg(feature = "engine")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::process::Command;

use ply_term::{Compression, DeltaBuilder, Engine};

const PANES: usize = 6;
const LINES: usize = 10_000;
const COLS: usize = 168;
const P4_BYTES: u64 = 15 * 1000 * 1000;

fn footprint() -> Option<u64> {
    let out = Command::new("footprint")
        .args(["-f", "bytes", "--noCategories"])
        .arg(std::process::id().to_string())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| l.trim().strip_prefix("phys_footprint: "))
        .and_then(|v| v.trim_end_matches(" B").parse().ok())
}

fn plain(lines: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(lines * (COLS + 2));
    for i in 0..lines {
        let head = format!("{i:08} ");
        out.extend_from_slice(head.as_bytes());
        out.extend((head.len()..COLS).map(|j| b'a' + ((i + j) % 26) as u8));
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// A worst case for page style tables: an SGR change on every word, true colour, CJK and emoji.
fn dense(lines: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(lines * (COLS * 3));
    for i in 0..lines {
        let mut width = 0;
        let mut k = 0;
        while width + 14 <= COLS {
            let sgr = match k % 4 {
                0 => format!("\x1b[38;5;{}m", (i * 7 + k) % 256),
                1 => format!("\x1b[1;38;2;{};{};128m", (i * 13) % 256, (k * 29) % 256),
                2 => "\x1b[3;4m".to_owned(),
                _ => format!("\x1b[48;5;{}m", 232 + i % 24),
            };
            out.extend_from_slice(sgr.as_bytes());
            out.extend_from_slice(format!("word{:08}\x1b[0m  ", i * 31 + k).as_bytes());
            width += 14;
            k += 1;
        }
        if i % 10 == 0 {
            out.extend_from_slice("日本".as_bytes());
        }
        if i % 50 == 0 {
            out.extend_from_slice("👩\u{200d}💻".as_bytes());
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[test]
fn memory_per_pane_at_10000_lines_of_168_columns() {
    if footprint().is_none() {
        eprintln!("skipped: macOS `footprint` is not available");
        return;
    }
    for (kind, payload) in [
        ("plain ASCII", plain(LINES + 50)),
        ("SGR-dense, CJK, emoji", dense(LINES + 50)),
    ] {
        let base = footprint().unwrap();
        let mut panes: Vec<(Engine, DeltaBuilder)> = (0..PANES)
            .map(|i| {
                (
                    Engine::new(i as u64, COLS as u16, 50, 10_000, &common::palette()).unwrap(),
                    DeltaBuilder::new(),
                )
            })
            .collect();
        for (engine, client) in &mut panes {
            engine.write(&payload);
            client.snapshot(engine).unwrap();
        }
        let filled = footprint().unwrap();
        let rows = panes[0].0.scrollback_rows();
        for (engine, _) in &mut panes {
            while engine.compress_idle().unwrap() == Compression::Pending {}
        }
        let idle = footprint().unwrap();
        let live = filled.saturating_sub(base) / PANES as u64;
        let compressed = idle.saturating_sub(base) / PANES as u64;
        eprintln!(
            "{kind}: {rows} scrollback rows × {COLS} cols per pane: live {:.2} MiB, after idle compression {:.2} MiB (P4 {:.2} MiB)",
            mib(live),
            mib(compressed),
            mib(P4_BYTES)
        );
        assert!(rows >= LINES as u32, "{kind}: {rows} rows kept");
        assert!(
            compressed <= P4_BYTES,
            "{kind}: {compressed} bytes per idle pane exceeds P4"
        );
        drop(panes);
    }
}
