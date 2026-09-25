//! Helpers shared by ply-term's integration tests.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use ply_proto::data::{Cursor, Frame, Modes, Snapshot};
use ply_proto::pane::Rgb;
use ply_term::{DeltaBuilder, Engine, Palette, Replica, ResolvedCell};

pub const FG: Rgb = Rgb {
    r: 0xd4,
    g: 0xd4,
    b: 0xd8,
};
pub const BG: Rgb = Rgb {
    r: 0x18,
    g: 0x18,
    b: 0x1b,
};

/// A neutral example palette: ANSI 1 is `e06c75`, the rest a grey ramp.
pub fn palette() -> Palette {
    let mut ansi = [Rgb { r: 0, g: 0, b: 0 }; 16];
    for (i, c) in ansi.iter_mut().enumerate() {
        let v = (i as u8) * 16;
        *c = Rgb { r: v, g: v, b: v };
    }
    ansi[1] = Rgb {
        r: 0xe0,
        g: 0x6c,
        b: 0x75,
    };
    Palette {
        ansi,
        fg: FG,
        bg: BG,
        cursor: Rgb {
            r: 0xfa,
            g: 0xfa,
            b: 0xfa,
        },
        cursor_text: BG,
        selection_bg: Rgb {
            r: 0x30,
            g: 0x40,
            b: 0x60,
        },
        selection_fg: FG,
    }
}

pub fn engine(cols: u16, rows: u16) -> Engine {
    let mut e = Engine::new(7, cols, rows, 10_000, &palette()).unwrap();
    e.resize(cols, rows, 8, 16).unwrap();
    e
}

/// A replica built from one fresh Snapshot of `engine`: the engine's grid as a client would see it.
pub fn truth(engine: &mut Engine) -> Replica {
    let mut r = Replica::new();
    r.apply_snapshot(&engine.snapshot().unwrap()).unwrap();
    r
}

/// Everything a client shows, with styles resolved, for comparing replicas whose style ids differ.
#[derive(Debug, PartialEq, Eq)]
pub struct View {
    pub size: (u16, u16),
    pub rows: Vec<(bool, Vec<ResolvedCell>)>,
    pub cursor: Cursor,
    pub modes: Modes,
    pub scrollback_rows: u32,
}

pub fn view(r: &Replica) -> View {
    let (_, rows) = r.size();
    View {
        size: r.size(),
        rows: (0..rows)
            .map(|y| (r.row(y).unwrap().wrapped, r.resolved_row(y).unwrap()))
            .collect(),
        cursor: r.cursor(),
        modes: r.modes(),
        scrollback_rows: r.scrollback_rows(),
    }
}

/// Applies one update from `client` to `replica`, if there is one.
pub fn pump(engine: &mut Engine, client: &mut DeltaBuilder, replica: &mut Replica) -> bool {
    match client.delta(engine).unwrap() {
        Some(update) => {
            let frame: Frame = update.into();
            let mut wire = Vec::new();
            frame.encode(&mut wire).unwrap();
            let decoded = Frame::decode(wire[4], &wire[5..]).unwrap();
            assert_eq!(decoded, frame, "C2 round trip");
            replica.apply(&decoded).unwrap();
            true
        }
        None => false,
    }
}

pub fn attach(engine: &mut Engine) -> (DeltaBuilder, Replica) {
    let mut client = DeltaBuilder::new();
    let mut replica = Replica::new();
    let snapshot: Snapshot = client.snapshot(engine).unwrap();
    replica.apply_snapshot(&snapshot).unwrap();
    (client, replica)
}

/// Screen text of `engine`, trailing empty rows dropped.
pub fn screen(engine: &mut Engine) -> Vec<String> {
    let mut rows = truth(engine).screen_text();
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows
}

/// A seeded xorshift64* generator: deterministic, dependency-free.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// Compares the client's view with libghostty-vt's own plain-text formatter, which shares no code with the C2 path:
/// scrollback (read through History pages) plus the Delta-fed screen, trailing empty lines dropped on both sides
/// (the formatter keeps a row of background-only cells as an empty line).
pub fn assert_matches_formatter(engine: &mut Engine, replica: &Replica, context: &str) {
    let text = engine.plain_text().unwrap();
    let mut got: Vec<&str> = if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').collect()
    };
    while got.last().is_some_and(|l| l.is_empty()) {
        got.pop();
    }
    let scrollback = i64::from(engine.scrollback_rows());
    let mut want: Vec<String> = Vec::new();
    let mut next = -scrollback;
    while next < 0 {
        let page = engine.scroll_history(next, 1000).unwrap();
        assert!(
            !page.lines.is_empty(),
            "{context}: history page at {next} is empty"
        );
        next += page.lines.len() as i64;
        want.extend(page.lines.iter().map(|r| ply_term::cells_text(&r.cells)));
    }
    want.extend(replica.screen_text());
    while want.last().is_some_and(String::is_empty) {
        want.pop();
    }
    assert_eq!(
        got, want,
        "{context}: the replica disagrees with libghostty-vt's formatter"
    );
}
