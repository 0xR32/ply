//! What both CLIs' plan-usage readers share (Ruling R59): the window lengths, their labels and the timestamp format.
//!
//! The readers themselves are [`crate::claude::usage`] and [`crate::codex::usage`]. They parse bytes plyd read from the
//! CLIs' own files and never touch the filesystem; what they cannot read is left out, never an error.

use ply_proto::pane::UnixSeconds;

/// Minutes of the 5-hour session window both CLIs report.
pub const SESSION_MINUTES: u32 = 300;

/// Minutes of the weekly window both CLIs report.
pub const WEEK_MINUTES: u32 = 10_080;

/// The label of a window of `minutes`: `"Session · 5h"`, `"Week"`, else its length (`"2 d"`, `"12 h"`, `"90 min"`).
pub fn window_label(minutes: u32) -> String {
    match minutes {
        SESSION_MINUTES => "Session · 5h".to_owned(),
        WEEK_MINUTES => "Week".to_owned(),
        m if m > 0 && m % 1440 == 0 => format!("{} d", m / 1440),
        m if m > 0 && m % 60 == 0 => format!("{} h", m / 60),
        m => format!("{m} min"),
    }
}

/// Unix seconds of an RFC 3339 time as the CLIs write it (`…T07:34:20.105Z`, `…T18:50:00.474957+00:00`), fraction dropped; `None` for other text and times before 1970.
pub fn parse_timestamp(s: &str) -> Option<UnixSeconds> {
    let digits = |from: usize, len: usize| -> Option<i64> {
        let part = s.get(from..from + len)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())
            .flatten()
    };
    let at = |i: usize, allowed: &[u8]| s.as_bytes().get(i).is_some_and(|b| allowed.contains(b));
    if !(at(4, b"-") && at(7, b"-") && at(10, b"Tt ") && at(13, b":") && at(16, b":")) {
        return None;
    }
    let (year, month, day) = (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?);
    let (hour, minute, second) = (digits(11, 2)?, digits(14, 2)?, digits(17, 2)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = s.get(19..)?;
    if let Some(fraction) = rest.strip_prefix('.') {
        let len = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if len == 0 {
            return None;
        }
        rest = &fraction[len..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let zone = &rest[1..];
            let (h, m) = match zone.len() {
                5 if zone.as_bytes()[2] == b':' => (zone.get(0..2)?, zone.get(3..5)?),
                4 => (zone.get(0..2)?, zone.get(2..4)?),
                _ => return None,
            };
            let number = |t: &str| {
                t.bytes()
                    .all(|b| b.is_ascii_digit())
                    .then(|| t.parse::<i64>().ok())
                    .flatten()
            };
            sign * (number(h)? * 3600 + number(m)? * 60)
        }
    };
    let days = days_from_civil(year, month, day);
    u64::try_from(days * 86_400 + hour * 3600 + minute * 60 + second - offset).ok()
}

/// Days from 1970-01-01 to the proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Whether `needle` occurs in `haystack`: the cheap test that keeps most rollout lines from being parsed at all.
pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_in_both_clis_shapes_parse_to_unix_seconds() {
        for (text, want) in [
            ("2026-09-25T18:50:00.474957+00:00", Some(1_790_362_200)),
            ("2026-09-25T20:50:00+02:00", Some(1_790_362_200)),
            ("2026-09-25T20:50:00+0200", Some(1_790_362_200)),
            ("2026-09-25T07:34:20.105Z", Some(1_790_321_660)),
            ("2026-09-26T08:00:00+00:00", Some(1_790_409_600)),
            ("2000-02-29T00:00:00Z", Some(951_782_400)),
            ("2024-12-31T23:59:59Z", Some(1_735_689_599)),
            ("1970-01-01T00:00:00Z", Some(0)),
            ("1969-12-31T23:59:59Z", None),
            ("2026-09-25", None),
            ("2026-09-25T18:50:00", None),
            ("2026-13-25T18:50:00Z", None),
            ("2026-09-25T18:50:00.Z", None),
            ("2026-09-25T18:50:00+00", None),
            ("tomorrow", None),
            ("", None),
        ] {
            assert_eq!(parse_timestamp(text), want, "{text}");
        }
    }

    #[test]
    fn windows_are_named_by_their_length() {
        assert_eq!(window_label(300), "Session · 5h");
        assert_eq!(window_label(10_080), "Week");
        assert_eq!(window_label(2880), "2 d");
        assert_eq!(window_label(720), "12 h");
        assert_eq!(window_label(90), "90 min");
        assert_eq!(window_label(0), "0 min");
    }

    #[test]
    fn contains_finds_a_needle_anywhere() {
        assert!(contains(br#"{"type":"token_count"}"#, b"token_count"));
        assert!(!contains(b"token", b"token_count"));
        assert!(!contains(b"anything", b""));
    }
}
