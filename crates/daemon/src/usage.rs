//! `usage.get` (Ruling R59): the plan usage Claude Code and Codex recorded in their own local files, read-only.
//!
//! Claude Code caches its usage in `.claude.json` (in `$CLAUDE_CONFIG_DIR` when the login shell sets it, else in the
//! home directory); Codex records its rate limits with every `token_count` event of its rollouts under
//! `$CODEX_HOME/sessions/` (default `~/.codex`). plyd reads both on a blocking thread, never writes either (INV-8) and
//! makes no request (INV-1), so the numbers are as old as the CLIs' last report and [`CliUsage::as_of`] says how old.
//! Rollouts are read from their ends: at most the [`MAX_ROLLOUTS`] most recently written files, at most
//! [`MAX_BYTES_PER_ROLLOUT`] of each and [`MAX_ROLLOUT_BYTES`] in all. An answer is reused for [`CACHE_FOR`], so a held
//! ⌘U polling every 5 s reads the files at most once per interval. A missing, unreadable or unexpected file means no
//! usage for that CLI (logged), never an error.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use ply_agents::claude::usage::parse_usage_cache;
use ply_agents::codex::usage::{FileScan, RolloutUsage};
use ply_proto::pane::{CliUsage, Usage};
use tokio::sync::Mutex;

/// How long one reading answers `usage.get`.
pub const CACHE_FOR: Duration = Duration::from_secs(5);

/// Rollouts read per `usage.get`, the most recently written first.
pub const MAX_ROLLOUTS: usize = 20;

/// Rollout bytes read per `usage.get`, over all files.
pub const MAX_ROLLOUT_BYTES: u64 = 8 << 20;

/// Rollout bytes read from the end of one file, so one file without rate limits cannot use up the whole budget.
pub const MAX_BYTES_PER_ROLLOUT: u64 = 2 << 20;

/// Largest `.claude.json` read; a bigger one is skipped and logged.
pub const MAX_CLAUDE_JSON_BYTES: u64 = 64 << 20;

/// Bytes read per step when reading a rollout backwards.
const CHUNK: usize = 64 * 1024;

/// Longest line kept while reading backwards; longer ones (inline images) are skipped whole.
const MAX_KEPT_LINE: usize = 1 << 20;

/// Rollout files listed, newest day directories first, before the most recently written [`MAX_ROLLOUTS`] are taken.
const CANDIDATES: usize = 2 * MAX_ROLLOUTS;

/// Where the CLIs keep what `usage.get` reads; `None` when the environment names no home for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSources {
    /// Claude Code's `.claude.json`.
    pub claude_json: Option<PathBuf>,
    /// Codex's `sessions/` directory.
    pub codex_sessions: Option<PathBuf>,
}

impl UsageSources {
    /// The files the CLIs use in `env` (plyd's login environment, R52): `CLAUDE_CONFIG_DIR` and `CODEX_HOME`, else `HOME`.
    pub fn from_env(env: &BTreeMap<String, String>) -> Self {
        let dir = |key: &str| env.get(key).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = dir("HOME");
        Self {
            claude_json: dir("CLAUDE_CONFIG_DIR")
                .or_else(|| home.clone())
                .map(|d| d.join(".claude.json")),
            codex_sessions: dir("CODEX_HOME")
                .or_else(|| home.map(|h| h.join(".codex")))
                .map(|d| d.join("sessions")),
        }
    }
}

/// Both CLIs' usage from `sources`; blocks on file I/O, so plyd calls it through [`UsageCache::get`].
pub fn read_usage(sources: &UsageSources) -> Usage {
    Usage {
        claude: sources.claude_json.as_deref().and_then(claude_usage),
        codex: sources.codex_sessions.as_deref().and_then(codex_usage),
    }
}

/// Claude Code's usage from its `.claude.json` at `path`; blocks on file I/O.
pub fn claude_usage(path: &Path) -> Option<CliUsage> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(file = %path.display(), error = %e, "cannot open Claude Code's .claude.json for its usage");
            return None;
        }
    };
    let len = file.metadata().map_or(0, |m| m.len());
    if len > MAX_CLAUDE_JSON_BYTES {
        tracing::warn!(file = %path.display(), len, "Claude Code's .claude.json is too large to read for its usage");
        return None;
    }
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    if let Err(e) = file.take(MAX_CLAUDE_JSON_BYTES).read_to_end(&mut bytes) {
        tracing::warn!(file = %path.display(), error = %e, "cannot read Claude Code's .claude.json for its usage");
        return None;
    }
    match parse_usage_cache(&bytes) {
        Ok(usage) => usage,
        Err(e) => {
            tracing::warn!(file = %path.display(), error = %e, "Claude Code's .claude.json is not readable JSON; no usage");
            None
        }
    }
}

/// Codex's usage from the newest rollouts under `sessions`; blocks on file I/O.
pub fn codex_usage(sessions: &Path) -> Option<CliUsage> {
    let mut budget = MAX_ROLLOUT_BYTES;
    let mut usage = RolloutUsage::default();
    for path in newest_rollouts(sessions, MAX_ROLLOUTS) {
        if budget == 0 {
            break;
        }
        usage.add(scan_backwards(&path, &mut budget));
    }
    usage.finish()
}

/// The `limit` most recently written `rollout-*.jsonl` files under `sessions/YYYY/MM/DD/`, newest first.
pub fn newest_rollouts(sessions: &Path, limit: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    'days: for year in numbered_dirs_desc(sessions) {
        for month in numbered_dirs_desc(&year) {
            for day in numbered_dirs_desc(&month) {
                let Some(entries) = read_dir_logged(&day) else {
                    continue;
                };
                files.extend(
                    entries
                        .filter_map(Result::ok)
                        .filter(|e| {
                            e.file_name()
                                .to_str()
                                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
                        })
                        .map(|e| e.path()),
                );
                if files.len() >= CANDIDATES.max(limit) {
                    break 'days;
                }
            }
        }
    }
    let mut dated: Vec<(SystemTime, PathBuf)> = files
        .into_iter()
        .filter_map(|p| match std::fs::metadata(&p).and_then(|m| m.modified()) {
            Ok(modified) => Some((modified, p)),
            Err(e) => {
                tracing::debug!(rollout = %p.display(), error = %e, "cannot stat a Codex rollout for its usage");
                None
            }
        })
        .collect();
    dated.sort_unstable_by(|a, b| b.cmp(a));
    dated.into_iter().take(limit).map(|(_, p)| p).collect()
}

/// Reads `path` backwards from its end, line by line, until the scan has what it needs, the file starts, or the budget (shared and per file) runs out.
pub fn scan_backwards(path: &Path, budget: &mut u64) -> FileScan {
    let mut scan = FileScan::default();
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) => {
            tracing::debug!(rollout = %path.display(), error = %e, "cannot open a Codex rollout for its usage");
            return scan;
        }
    };
    let mut end = match file.metadata() {
        Ok(meta) => meta.len(),
        Err(e) => {
            tracing::debug!(rollout = %path.display(), error = %e, "cannot stat a Codex rollout for its usage");
            return scan;
        }
    };
    let mut allowance = MAX_BYTES_PER_ROLLOUT.min(*budget);
    let mut chunk = vec![0u8; CHUNK];
    let mut carry: Vec<u8> = Vec::new();
    let mut oversized = false;
    while end > 0 && allowance > 0 {
        let take = (CHUNK as u64).min(end).min(allowance);
        let start = end - take;
        let buf = &mut chunk[..take as usize];
        if let Err(e) = file
            .seek(SeekFrom::Start(start))
            .and_then(|_| file.read_exact(buf))
        {
            tracing::debug!(rollout = %path.display(), error = %e, "cannot read a Codex rollout for its usage");
            return scan;
        }
        allowance -= take;
        *budget -= take;
        end = start;
        let mut high = buf.len();
        while let Some(newline) = buf[..high].iter().rposition(|&b| b == b'\n') {
            if !oversized {
                let tail = &buf[newline + 1..high];
                let done = if carry.is_empty() {
                    !tail.is_empty() && scan.push(tail)
                } else {
                    let mut line = tail.to_vec();
                    line.extend_from_slice(&carry);
                    scan.push(&line)
                };
                if done {
                    return scan;
                }
            }
            carry.clear();
            oversized = false;
            high = newline;
        }
        if !oversized {
            if carry.len() + high > MAX_KEPT_LINE {
                carry = Vec::new();
                oversized = true;
            } else {
                let mut joined = buf[..high].to_vec();
                joined.extend_from_slice(&carry);
                carry = joined;
            }
        }
    }
    if end == 0 && !oversized && !carry.is_empty() {
        scan.push(&carry);
    }
    scan
}

fn read_dir_logged(dir: &Path) -> Option<std::fs::ReadDir> {
    match std::fs::read_dir(dir) {
        Ok(entries) => Some(entries),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => {
            tracing::debug!(dir = %dir.display(), error = %e, "cannot list a Codex sessions directory for its usage");
            None
        }
    }
}

fn numbered_dirs_desc(dir: &Path) -> Vec<PathBuf> {
    let Some(entries) = read_dir_logged(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort_unstable_by(|a, b| b.cmp(a));
    dirs
}

/// The last `usage.get` answer and when it was read; concurrent callers wait for one reading instead of starting several.
#[derive(Debug, Default)]
pub struct UsageCache {
    last: Mutex<Option<(Instant, Usage)>>,
}

impl UsageCache {
    /// The usage in `sources`, read on a blocking thread unless the last reading is younger than [`CACHE_FOR`].
    pub async fn get(&self, sources: UsageSources) -> Usage {
        let mut last = self.last.lock().await;
        if let Some((at, usage)) = last.as_ref()
            && at.elapsed() < CACHE_FOR
        {
            return usage.clone();
        }
        match tokio::task::spawn_blocking(move || read_usage(&sources)).await {
            Ok(usage) => {
                *last = Some((Instant::now(), usage.clone()));
                usage
            }
            Err(e) => {
                tracing::error!(error = %e, "the usage reader failed; answering no usage");
                Usage::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!("ply-usage-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn token_count(time: &str, used: f64) -> String {
        format!(
            r#"{{"timestamp":"{time}","type":"event_msg","payload":{{"type":"token_count","info":null,"rate_limits":{{"limit_id":"codex","limit_name":null,"primary":{{"used_percent":{used:?},"window_minutes":300,"resets_at":1790341200}},"secondary":{{"used_percent":40.0,"window_minutes":10080,"resets_at":1790926095}},"plan_type":"plus"}}}}}}"#
        )
    }

    fn turn_context(model: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-09-25T09:59:01.000Z","type":"turn_context","payload":{{"cwd":"/Users/example/project","model":"{model}"}}}}"#
        )
    }

    fn filler(len: usize) -> String {
        let text = "x".repeat(len);
        format!(
            r#"{{"timestamp":"2026-09-25T10:00:00.000Z","type":"response_item","payload":{{"type":"message","text":"{text}"}}}}"#
        )
    }

    fn write_lines(path: &Path, lines: &[String]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = File::create(path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
    }

    #[test]
    fn the_sources_follow_the_clis_own_variables_and_fall_back_to_home() {
        let env = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        let home = UsageSources::from_env(&env(&[("HOME", "/Users/example")]));
        assert_eq!(
            home.claude_json,
            Some(PathBuf::from("/Users/example/.claude.json"))
        );
        assert_eq!(
            home.codex_sessions,
            Some(PathBuf::from("/Users/example/.codex/sessions"))
        );
        let own = UsageSources::from_env(&env(&[
            ("HOME", "/Users/example"),
            ("CLAUDE_CONFIG_DIR", "/example/claude"),
            ("CODEX_HOME", "/example/codex"),
        ]));
        assert_eq!(
            own.claude_json,
            Some(PathBuf::from("/example/claude/.claude.json"))
        );
        assert_eq!(
            own.codex_sessions,
            Some(PathBuf::from("/example/codex/sessions"))
        );
        let none = UsageSources::from_env(&env(&[("CLAUDE_CONFIG_DIR", "")]));
        assert_eq!((none.claude_json, none.codex_sessions), (None, None));
    }

    #[test]
    fn missing_and_malformed_files_mean_no_usage() {
        let dir = Dir::new("missing");
        let sources = UsageSources {
            claude_json: Some(dir.0.join(".claude.json")),
            codex_sessions: Some(dir.0.join(".codex/sessions")),
        };
        assert_eq!(read_usage(&sources), Usage::default());
        std::fs::write(dir.0.join(".claude.json"), "{\"cachedUsageUtilization\":").unwrap();
        write_lines(
            &dir.0
                .join(".codex/sessions/2026/09/25/rollout-2026-09-25T10-00-00-a.jsonl"),
            &["not json".to_owned(), "{\"type\":\"event_msg\"}".to_owned()],
        );
        assert_eq!(read_usage(&sources), Usage::default());
        std::fs::remove_file(dir.0.join(".claude.json")).unwrap();
        std::fs::create_dir(dir.0.join(".claude.json")).unwrap();
        assert_eq!(
            claude_usage(&dir.0.join(".claude.json")),
            None,
            "a directory in its place"
        );
    }

    #[test]
    fn both_clis_usage_is_read_from_their_files() {
        let dir = Dir::new("both");
        std::fs::write(
            dir.0.join(".claude.json"),
            r#"{"projects":{},"cachedUsageUtilization":{"fetchedAtMs":1790350000123,"accountUuid":"00000000-0000-4000-8000-000000000000","utilization":{"five_hour":{"utilization":81,"resets_at":"2026-09-25T18:50:00.474957+00:00"},"seven_day":{"utilization":30,"resets_at":"2026-09-26T08:00:00+00:00"},"seven_day_opus":null}}}"#,
        )
        .unwrap();
        write_lines(
            &dir.0
                .join(".codex/sessions/2026/09/25/rollout-2026-09-25T10-00-00-a.jsonl"),
            &[
                turn_context("gpt-6-sol"),
                token_count("2026-09-25T10:10:00.000Z", 12.0),
            ],
        );
        let usage = read_usage(&UsageSources {
            claude_json: Some(dir.0.join(".claude.json")),
            codex_sessions: Some(dir.0.join(".codex/sessions")),
        });
        let claude = usage.claude.unwrap();
        assert_eq!(claude.as_of, 1_790_350_000);
        assert_eq!(claude.windows.len(), 2);
        let codex = usage.codex.unwrap();
        assert_eq!(
            (codex.as_of, codex.plan.as_deref()),
            (1_790_331_000, Some("plus"))
        );
        let rows: Vec<(&str, f64, &[String])> = codex
            .windows
            .iter()
            .map(|w| (w.label.as_str(), w.used_percent, w.models.as_slice()))
            .collect();
        let sol = ["gpt-6-sol".to_owned()];
        assert_eq!(
            rows,
            [("Session · 5h", 12.0, &sol[..]), ("Week", 40.0, &sol[..])]
        );
    }

    #[test]
    fn only_the_newest_rollouts_are_listed_across_day_directories() {
        let dir = Dir::new("newest");
        let sessions = dir.0.join("sessions");
        let mut names = Vec::new();
        for i in 0..25 {
            let day = if i < 20 { "2026/09/24" } else { "2026/09/25" };
            let name = format!(
                "rollout-2026-09-2{}T10-00-{i:02}-x.jsonl",
                if i < 20 { 4 } else { 5 }
            );
            let path = sessions.join(day).join(&name);
            write_lines(&path, &[token_count("2026-09-25T10:00:00.000Z", 1.0)]);
            let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000 + i * 60);
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(when)
                .unwrap();
            names.push(path);
        }
        std::fs::write(sessions.join("2026/09/25/notes.txt"), "").unwrap();
        std::fs::create_dir_all(sessions.join("archive")).unwrap();
        let newest = newest_rollouts(&sessions, MAX_ROLLOUTS);
        assert_eq!(newest.len(), MAX_ROLLOUTS);
        names.reverse();
        assert_eq!(newest, names[..MAX_ROLLOUTS]);
        assert!(newest_rollouts(&dir.0.join("nothing"), MAX_ROLLOUTS).is_empty());
    }

    #[test]
    fn a_rollout_is_read_from_its_end_across_chunks_and_past_an_oversized_line() {
        let dir = Dir::new("backwards");
        let path = dir.0.join("rollout-a.jsonl");
        write_lines(
            &path,
            &[
                filler(2 * CHUNK),
                turn_context("gpt-6-astra"),
                token_count("2026-09-25T09:00:00.000Z", 3.0),
                turn_context("gpt-6-sol"),
                filler(MAX_KEPT_LINE + 10),
                filler(3 * CHUNK),
                token_count("2026-09-25T10:10:00.000Z", 12.0),
                filler(CHUNK / 2),
            ],
        );
        let mut budget = MAX_ROLLOUT_BYTES;
        let scan = scan_backwards(&path, &mut budget);
        assert_eq!(scan.latest().map(|r| r.at), Some(1_790_331_000));
        assert_eq!(
            scan.model(),
            Some("gpt-6-sol"),
            "the turn behind the record, beyond the oversized line"
        );
        let read = MAX_ROLLOUT_BYTES - budget;
        let len = std::fs::metadata(&path).unwrap().len();
        assert!(
            read < len,
            "stops at the turn_context: read {read} of {len}"
        );
    }

    #[test]
    fn the_budgets_bound_what_is_read() {
        let dir = Dir::new("budget");
        let path = dir.0.join("rollout-a.jsonl");
        write_lines(
            &path,
            &[
                turn_context("gpt-6-sol"),
                token_count("2026-09-25T10:10:00.000Z", 12.0),
                filler(3 * CHUNK),
            ],
        );
        let mut small = (2 * CHUNK) as u64;
        let scan = scan_backwards(&path, &mut small);
        assert_eq!(
            (scan.latest(), small),
            (None, 0),
            "the record lies beyond the shared budget"
        );
        let big = dir.0.join("rollout-b.jsonl");
        let fill = filler(MAX_KEPT_LINE / 2);
        let lines: Vec<String> = std::iter::once(token_count("2026-09-25T10:10:00.000Z", 12.0))
            .chain(std::iter::repeat_n(fill, 6))
            .collect();
        write_lines(&big, &lines);
        let mut budget = MAX_ROLLOUT_BYTES;
        let scan = scan_backwards(&big, &mut budget);
        assert_eq!(scan.latest(), None);
        assert_eq!(
            MAX_ROLLOUT_BYTES - budget,
            MAX_BYTES_PER_ROLLOUT,
            "one file reads at most its own allowance"
        );
        let empty = dir.0.join("rollout-c.jsonl");
        std::fs::write(&empty, "").unwrap();
        let mut budget = MAX_ROLLOUT_BYTES;
        assert_eq!(scan_backwards(&empty, &mut budget), FileScan::default());
        assert_eq!(
            scan_backwards(&dir.0.join("gone.jsonl"), &mut budget),
            FileScan::default()
        );
    }

    #[test]
    fn a_first_line_without_a_newline_before_it_is_read_at_the_start_of_the_file() {
        let dir = Dir::new("first");
        let path = dir.0.join("rollout-a.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n{}",
                turn_context("gpt-6-sol"),
                token_count("2026-09-25T10:10:00.000Z", 12.0)
            ),
        )
        .unwrap();
        let mut budget = MAX_ROLLOUT_BYTES;
        let scan = scan_backwards(&path, &mut budget);
        assert_eq!(
            scan.latest().map(|r| r.at),
            Some(1_790_331_000),
            "an unterminated last line still parses when whole"
        );
        assert_eq!(
            scan.model(),
            Some("gpt-6-sol"),
            "the file's first line has no newline before it"
        );
    }

    #[tokio::test]
    async fn an_answer_is_reused_for_five_seconds() {
        let dir = Dir::new("cache");
        let json = dir.0.join(".claude.json");
        let sources = UsageSources {
            claude_json: Some(json.clone()),
            codex_sessions: None,
        };
        let cache = UsageCache::default();
        assert_eq!(cache.get(sources.clone()).await, Usage::default());
        std::fs::write(
            &json,
            r#"{"cachedUsageUtilization":{"fetchedAtMs":1790350000123,"utilization":{"five_hour":{"utilization":5}}}}"#,
        )
        .unwrap();
        assert_eq!(
            cache.get(sources.clone()).await,
            Usage::default(),
            "still the cached answer"
        );
        *cache.last.lock().await = None;
        assert!(cache.get(sources).await.claude.is_some());
    }
}
