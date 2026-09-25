//! Engine + DeltaBuilder throughput (WP3 exit criterion, Ruling R18): at least 100 MB/s on the recorded agent corpus
//! and 300 MB/s on plain ASCII, on a 168 × 50 pane with 10 000 lines of scrollback, written in 64 KiB pty-sized chunks
//! with a Delta built at most every 8.333 ms (the 120 Hz cap of spec 4.2).
//!
//! The recorded agent corpus is ADR-0005's: the eleven S2b Claude Code captures and four S3b Codex captures,
//! concatenated in byte order of their names (188 223 bytes), repeated to at least 32 MiB. They live in the owner's
//! git-ignored `.superpowers/plan/evidence` (override with `PLY_AGENT_CORPUS_DIR`); without them the bench uses the
//! committed, scrubbed `tests/fixtures/*-session.bytes` instead and says so.
//!
//! Run: `cargo bench -p ply-term --features engine --bench throughput` (best and median of seven runs each).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ply_proto::data::Frame;
use ply_proto::pane::Rgb;
use ply_term::{DeltaBuilder, Engine, Palette};

const CLAUDE: &[&str] = &[
    "run10_raw.bin",
    "run1_raw.bin",
    "run2_raw.bin",
    "run3_raw.bin",
    "run4_raw.bin",
    "run5_raw.bin",
    "run6_raw.bin",
    "run6a_raw.bin",
    "run7_raw.bin",
    "run8_raw.bin",
    "run9_raw.bin",
];
const CODEX: &[&str] = &[
    "raw_approval.bin",
    "raw_approval2.bin",
    "raw_approve.bin",
    "raw_observe.bin",
];
const TARGET: usize = 32 << 20;
const CHUNK: usize = 64 * 1024;
const FRAME: Duration = Duration::from_micros(8_333);
const RUNS: usize = 7;

#[derive(Clone, Copy, PartialEq)]
enum Deltas {
    None,
    At120Hz,
    EveryChunkEncoded,
}

fn palette() -> Palette {
    let grey = |v: u8| Rgb { r: v, g: v, b: v };
    Palette {
        ansi: [grey(128); 16],
        fg: grey(212),
        bg: grey(24),
        cursor: grey(250),
        cursor_text: grey(24),
        selection_bg: grey(64),
        selection_fg: grey(212),
    }
}

fn agent_corpus() -> (Vec<u8>, String) {
    let root = std::env::var_os("PLY_AGENT_CORPUS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.superpowers/plan/evidence")
        });
    let files: Vec<PathBuf> = CLAUDE
        .iter()
        .map(|f| root.join("s2b/logs").join(f))
        .chain(CODEX.iter().map(|f| root.join("s3b").join(f)))
        .collect();
    if files.iter().all(|f| f.is_file()) {
        let one: Vec<u8> = files
            .iter()
            .flat_map(|f| std::fs::read(f).unwrap())
            .collect();
        let label = format!(
            "recorded agent corpus (15 captures, {} bytes per copy)",
            one.len()
        );
        return (repeat(&one), label);
    }
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let one: Vec<u8> = ["claude-session.bytes", "codex-session.bytes"]
        .iter()
        .flat_map(|f| std::fs::read(fixtures.join(f)).unwrap())
        .collect();
    let label = format!(
        "committed fixtures instead of the agent corpus ({} bytes per copy)",
        one.len()
    );
    (repeat(&one), label)
}

fn repeat(one: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(TARGET + one.len());
    while data.len() < TARGET {
        data.extend_from_slice(one);
    }
    data
}

fn plain_ascii() -> Vec<u8> {
    let mut data = Vec::with_capacity(TARGET + 256);
    let mut i = 0usize;
    while data.len() < TARGET {
        let head = format!("{i:08} ");
        data.extend_from_slice(head.as_bytes());
        data.extend((head.len()..168).map(|j| b'a' + ((i + j) % 26) as u8));
        data.extend_from_slice(b"\r\n");
        i += 1;
    }
    data
}

/// Seconds to feed `data`, and the Deltas built.
fn run(data: &[u8], deltas: Deltas) -> (f64, usize) {
    let palette = palette();
    let mut engine = Engine::new(1, 168, 50, 10_000, &palette).unwrap();
    engine.resize(168, 50, 8, 16).unwrap();
    let mut client = DeltaBuilder::new();
    client.snapshot(&mut engine).unwrap();
    let mut built = 0;
    let mut wire = Vec::with_capacity(1 << 20);
    let start = Instant::now();
    let mut last = start;
    for chunk in data.chunks(CHUNK) {
        let out = engine.write(chunk);
        std::hint::black_box(&out);
        let due = match deltas {
            Deltas::None => false,
            Deltas::At120Hz => last.elapsed() >= FRAME,
            Deltas::EveryChunkEncoded => true,
        };
        if due {
            if let Some(update) = client.delta(&mut engine).unwrap() {
                built += 1;
                if deltas == Deltas::EveryChunkEncoded {
                    wire.clear();
                    Frame::from(update).encode(&mut wire).unwrap();
                } else {
                    std::hint::black_box(update);
                }
            }
            last = Instant::now();
        }
    }
    if deltas != Deltas::None {
        client.delta(&mut engine).unwrap();
    }
    (start.elapsed().as_secs_f64(), built)
}

fn measure(name: &str, data: &[u8], deltas: Deltas, mode: &str) -> f64 {
    let mut rates = Vec::with_capacity(RUNS);
    let mut built = 0;
    for _ in 0..RUNS {
        let (secs, n) = run(data, deltas);
        rates.push(data.len() as f64 / secs / 1e6);
        built = n;
    }
    rates.sort_by(|a, b| b.total_cmp(a));
    println!(
        "{name:13} {:5.1} MB  {mode:36} best {:7.1} MB/s  median {:7.1} MB/s  ({built} Deltas)",
        data.len() as f64 / 1e6,
        rates[0],
        rates[RUNS / 2]
    );
    rates[0]
}

fn main() {
    let (agents, label) = agent_corpus();
    let plain = plain_ascii();
    println!("168 x 50 pane, 10 000 lines of scrollback, 64 KiB writes; {label}");
    measure("agent corpus", &agents, Deltas::None, "engine only");
    let agents_rate = measure(
        "agent corpus",
        &agents,
        Deltas::At120Hz,
        "engine + DeltaBuilder at <= 120 Hz",
    );
    measure(
        "agent corpus",
        &agents,
        Deltas::EveryChunkEncoded,
        "engine + Delta + encode every 64 KiB",
    );
    measure("plain ASCII", &plain, Deltas::None, "engine only");
    let plain_rate = measure(
        "plain ASCII",
        &plain,
        Deltas::At120Hz,
        "engine + DeltaBuilder at <= 120 Hz",
    );
    let verdict = |ok: bool| if ok { "PASS" } else { "FAIL" };
    println!(
        "R18 agent corpus >= 100 MB/s: {} ({agents_rate:.1})",
        verdict(agents_rate >= 100.0)
    );
    println!(
        "R18 plain ASCII  >= 300 MB/s: {} ({plain_rate:.1})",
        verdict(plain_rate >= 300.0)
    );
}
