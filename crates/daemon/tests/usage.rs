//! `usage.get` against a real plyd (Ruling R59): the answer comes from the CLIs' own files in a sandboxed home, honours
//! `CLAUDE_CONFIG_DIR`, never carries the account id, and leaves every file it read byte for byte and time for time.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use common::{Control, Sandbox};
use serde_json::{Value, json};

fn agents_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../agents/tests/fixtures")
        .join(name)
}

fn place(from: &str, to: &Path) {
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::copy(agents_fixture(from), to).unwrap();
}

fn snapshot(files: &[&Path]) -> Vec<(Vec<u8>, SystemTime)> {
    files
        .iter()
        .map(|f| {
            let meta = std::fs::metadata(f).unwrap();
            (std::fs::read(f).unwrap(), meta.modified().unwrap())
        })
        .collect()
}

fn labels(usage: &Value, cli: &str) -> Vec<String> {
    usage[cli]["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["label"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn usage_comes_from_the_clis_files_which_stay_untouched() {
    let sb = Sandbox::new("usage");
    let claude_json = sb.home.join(".claude.json");
    let rollout = sb
        .home
        .join(".codex/sessions/2026/09/25/rollout-2026-09-25T09-59-00-00000000-0000-7000-8000-000000000031.jsonl");
    place("claude/dot-claude.json", &claude_json);
    place("codex/rollout-usage-two-limits.jsonl", &rollout);
    let before = snapshot(&[&claude_json, &rollout]);

    let _plyd = sb.start();
    let mut c = Control::connect(&sb.control_socket()).unwrap();
    let usage = c.call("usage.get", json!({})).unwrap();
    assert_eq!(
        labels(&usage, "claude"),
        ["Session · 5h", "Week · all models", "Week · Sonnet"]
    );
    assert_eq!(usage["claude"]["as_of"], 1_790_350_000);
    assert_eq!(labels(&usage, "codex"), ["Session · 5h", "Week"]);
    assert_eq!(usage["codex"]["windows"][1]["models"], json!(["gpt-6-sol"]));
    assert_eq!(usage["codex"]["plan"], "plus");
    assert!(
        !usage
            .to_string()
            .contains("00000000-0000-4000-8000-00000000c1a0"),
        "the account id never reaches a client"
    );
    assert_eq!(
        c.call("usage.get", json!({})).unwrap(),
        usage,
        "the cached answer"
    );
    assert_eq!(
        snapshot(&[&claude_json, &rollout]),
        before,
        "INV-8: read-only"
    );
}

#[test]
fn claude_config_dir_moves_the_cache_and_nothing_is_no_usage() {
    let sb = Sandbox::new("usage-dir");
    let mut plyd = sb.start();
    let mut c = Control::connect(&sb.control_socket()).unwrap();
    assert_eq!(c.call("usage.get", json!({})).unwrap(), json!({}));
    drop(c);
    plyd.stop();

    let config = sb.root.join("claude-config");
    place("claude/dot-claude.json", &config.join(".claude.json"));
    let _plyd = sb.start_with(&[("CLAUDE_CONFIG_DIR", config.to_str().unwrap())]);
    let mut c = Control::connect(&sb.control_socket()).unwrap();
    let usage = c.call("usage.get", json!({})).unwrap();
    assert_eq!(labels(&usage, "claude").len(), 3);
    assert!(usage.get("codex").is_none());
}
