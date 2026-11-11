//! The new-pane form's folder sources, as `app/src/ipc/dirs.ts` and `app/src/state/dir-match.ts` define them.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// `path` with the home directory written as `~`.
pub fn tilde(path: &str) -> String {
    let home = home();
    let home = home.to_string_lossy();
    if path == home {
        "~".into()
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
        format!("~/{rest}")
    } else {
        path.to_string()
    }
}

/// `~` and `~/…` expanded; anything else as typed.
pub fn expand(path: &str) -> String {
    if path == "~" {
        home().to_string_lossy().into_owned()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home().join(rest).to_string_lossy().into_owned()
    } else {
        path.to_string()
    }
}

/// Breadth-first over `$HOME` to depth 4 for folders holding `.git`; hidden folders, `node_modules`, `target`,
/// `~/Library`, symlinks and a repository's own tree are skipped; stops at 5 000 folders or 2 s.
pub fn scan_repos() -> Vec<String> {
    let root = home();
    let library = root.join("Library");
    let started = Instant::now();
    let mut found = Vec::new();
    let mut listed = 0usize;
    let mut queue = VecDeque::from([(root, 0usize)]);
    while let Some((dir, depth)) = queue.pop_front() {
        if listed >= 5_000 || started.elapsed() > Duration::from_millis(2_000) {
            break;
        }
        listed += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() || !kind.is_dir() {
                continue;
            }
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            let path = entry.path();
            if path == library {
                continue;
            }
            children.push(path);
        }
        for path in children {
            if path.join(".git").exists() {
                found.push(path.to_string_lossy().into_owned());
            } else if depth + 1 < 4 {
                queue.push_back((path, depth + 1));
            }
        }
    }
    found.sort();
    found
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Recent,
    Repo,
    Completion,
}

#[derive(Clone)]
pub struct Suggestion {
    pub path: String,
    pub kind: Kind,
    score: i32,
    recency: u64,
}

impl Suggestion {
    /// The parent shown dim and the folder name shown bright.
    pub fn split(&self) -> (String, String) {
        match self.path.rfind('/') {
            Some(i) if i + 1 < self.path.len() => {
                (self.path[..=i].to_string(), self.path[i + 1..].to_string())
            }
            _ => (String::new(), self.path.clone()),
        }
    }
}

/// 3 equal, 2 prefix, 1 substring, 0 subsequence, `None` no match; case-insensitive.
fn quality(needle: &str, hay: &str) -> Option<i32> {
    let (n, h) = (needle.to_lowercase(), hay.to_lowercase());
    if n == h {
        return Some(3);
    }
    if h.starts_with(&n) {
        return Some(2);
    }
    if h.contains(&n) {
        return Some(1);
    }
    let mut it = h.chars();
    n.chars().all(|c| it.any(|x| x == c)).then_some(0)
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// Sub-folders of the deepest existing ancestor of the folder before the query's last `/`, matched on the segment after it.
fn completions(query: &str, start: &str) -> Vec<Suggestion> {
    let expanded = expand(query);
    let (dir, segment) = match expanded.rfind('/') {
        Some(i) => (expanded[..=i].to_string(), expanded[i + 1..].to_string()),
        None if query == "~" => (format!("{}/", home().to_string_lossy()), String::new()),
        None => (String::new(), expanded.clone()),
    };
    let mut base = if Path::new(&dir).is_absolute() {
        PathBuf::from(&dir)
    } else {
        Path::new(start).join(&dir)
    };
    let mut segment = segment;
    while !base.is_dir() {
        let Some(parent) = base.parent() else {
            return Vec::new();
        };
        let lost = base
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        segment = format!("{lost}/{segment}");
        base = parent.to_path_buf();
    }
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') || segment.starts_with('.'))
        .collect();
    names.sort();
    names.truncate(1_000);
    names
        .into_iter()
        .filter_map(|name| {
            let q = if segment.is_empty() {
                0
            } else {
                quality(&segment, &name)?
            };
            let path = tilde(&base.join(&name).to_string_lossy());
            Some(Suggestion {
                path,
                kind: Kind::Completion,
                score: 100 + q * 10,
                recency: 0,
            })
        })
        .collect()
}

/// At most eight rows for `query`: recents only when it is empty, else recents, repositories and path completion ranked
/// by match quality on the folder name (100 + q×10) or the path (q×10), recents +10.
pub fn suggest(
    query: &str,
    start: &str,
    recents: &[(String, u64)],
    repos: &[String],
) -> Vec<Suggestion> {
    let query = query.trim();
    if query.is_empty() {
        return recents
            .iter()
            .take(8)
            .map(|(p, t)| Suggestion {
                path: tilde(p),
                kind: Kind::Recent,
                score: 0,
                recency: *t,
            })
            .collect();
    }
    let last = query
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(query);
    let score = |path: &str| -> Option<i32> {
        let shown = tilde(path);
        if let Some(q) = quality(last, basename(&shown)) {
            Some(100 + q * 10)
        } else {
            quality(query, &shown).map(|q| q * 10)
        }
    };
    let mut rows: Vec<Suggestion> = Vec::new();
    for (p, t) in recents {
        if let Some(s) = score(p) {
            rows.push(Suggestion {
                path: tilde(p),
                kind: Kind::Recent,
                score: s + 10,
                recency: *t,
            });
        }
    }
    for p in repos {
        if let Some(s) = score(p) {
            rows.push(Suggestion {
                path: tilde(p),
                kind: Kind::Repo,
                score: s,
                recency: 0,
            });
        }
    }
    if query.starts_with('/') || query.starts_with('~') || query.starts_with('.') {
        rows.extend(completions(query, start));
    }
    rows.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.kind.cmp(&b.kind))
            .then(b.recency.cmp(&a.recency))
            .then(a.path.len().cmp(&b.path.len()))
            .then(a.path.cmp(&b.path))
    });
    let mut seen = HashSet::new();
    rows.retain(|r| seen.insert(r.path.clone()));
    rows.truncate(8);
    rows
}
