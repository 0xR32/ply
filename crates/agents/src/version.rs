//! CLI versions for the spawn-time check of C7: parsing, ordering and the minimum gate.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result};

/// A `major.minor.patch[-pre]` version; build metadata (`+…`) is dropped, and a pre-release sorts before its release.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CliVersion {
    /// Major number.
    pub major: u64,
    /// Minor number.
    pub minor: u64,
    /// Patch number.
    pub patch: u64,
    /// Pre-release identifiers after `-` (e.g. `"alpha.3"`), compared per SemVer 2.0 §11.
    pub pre: Option<String>,
}

impl CliVersion {
    /// A release version with no pre-release part.
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: None,
        }
    }

    /// Finds the first whitespace-separated token of `--version` output that is a version, e.g. `2.1.282 (Claude Code)`.
    pub fn from_version_output(text: &str) -> Result<Self> {
        text.split_whitespace()
            .find_map(|token| token.parse().ok())
            .ok_or_else(|| Error::BadVersion(text.trim().chars().take(80).collect()))
    }
}

impl FromStr for CliVersion {
    type Err = Error;

    /// Accepts an optional leading `v`; fails with [`Error::BadVersion`] on anything but three numeric parts.
    fn from_str(s: &str) -> Result<Self> {
        let bad = || Error::BadVersion(s.chars().take(80).collect());
        let core = s.strip_prefix('v').unwrap_or(s);
        let core = core.split_once('+').map_or(core, |(c, _)| c);
        let (numbers, pre) = match core.split_once('-') {
            Some((n, p)) if !p.is_empty() && p.split('.').all(valid_identifier) => {
                (n, Some(p.to_owned()))
            }
            Some(_) => return Err(bad()),
            None => (core, None),
        };
        let mut parts = numbers.split('.');
        let mut next = || -> Result<u64> {
            let part = parts.next().ok_or_else(bad)?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            part.parse().map_err(|_| bad())
        };
        let (major, minor, patch) = (next()?, next()?, next()?);
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

fn valid_identifier(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl Ord for CliVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => compare_pre(a, b),
            })
    }
}

impl PartialOrd for CliVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

impl fmt::Display for CliVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> CliVersion {
        s.parse().unwrap()
    }

    #[test]
    fn parses_releases_prereleases_and_build_metadata() {
        assert_eq!(v("2.1.282"), CliVersion::new(2, 1, 282));
        assert_eq!(v("v0.156.1"), CliVersion::new(0, 156, 1));
        assert_eq!(v("0.157.0-alpha.3").pre.as_deref(), Some("alpha.3"));
        assert_eq!(v("1.2.3+build.9"), CliVersion::new(1, 2, 3));
    }

    #[test]
    fn rejects_anything_but_three_numbers() {
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "1..3",
            "a.b.c",
            "1.2.3-",
            "1.2.x",
            " 1.2.3",
            "1.2.3-a..b",
        ] {
            assert!(bad.parse::<CliVersion>().is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn orders_by_numbers_then_prerelease() {
        assert!(v("0.156.1") < v("0.157.0"));
        assert!(v("2.1.282") > v("2.1.99"));
        assert!(v("1.0.0-alpha") < v("1.0.0"));
        assert!(v("1.0.0-alpha") < v("1.0.0-alpha.1"));
        assert!(v("1.0.0-alpha.2") < v("1.0.0-alpha.10"));
        assert!(v("1.0.0-1") < v("1.0.0-alpha"));
        assert_eq!(v("1.0.0+a").cmp(&v("1.0.0+b")), Ordering::Equal);
    }

    #[test]
    fn reads_version_output_of_both_clis() {
        assert_eq!(
            CliVersion::from_version_output("2.1.282 (Claude Code)\n").unwrap(),
            v("2.1.282")
        );
        assert_eq!(
            CliVersion::from_version_output("codex-cli 0.156.1\n").unwrap(),
            v("0.156.1")
        );
        assert!(CliVersion::from_version_output("command not found").is_err());
    }

    #[test]
    fn displays_round_trip() {
        for s in ["2.1.282", "0.157.0-alpha.3"] {
            assert_eq!(v(s).to_string(), s);
        }
    }
}
