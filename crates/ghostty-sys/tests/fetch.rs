//! `build.rs`'s source download (`fetch.rs`) against local `file://` archives: a verified archive lands whole, and a
//! wrong hash, a failed download or a missing top directory caches nothing and leaves no staging files behind.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../fetch.rs"]
mod fetch;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use fetch::Pin;

struct Fixture {
    archive: PathBuf,
    url: String,
    sha256: String,
    cache: PathBuf,
}

fn fixture(test: &str) -> Fixture {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ghostty-fetch-{test}"));
    let _ = fs::remove_dir_all(&dir);
    let tree = dir.join("src/ghostty-abc");
    fs::create_dir_all(tree.join("include/ghostty")).unwrap();
    fs::write(tree.join("build.zig"), "// build\n").unwrap();
    fs::write(tree.join("include/ghostty/vt.h"), "// vt\n").unwrap();
    let archive = dir.join("ghostty.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(dir.join("src"))
        .arg("ghostty-abc")
        .status()
        .unwrap();
    assert!(status.success());
    let sha256 = fetch::sha256(&archive).unwrap();
    Fixture {
        url: format!("file://{}", archive.display()),
        archive,
        sha256,
        cache: dir.join("cache/ply/ghostty"),
    }
}

fn cache_entries(cache: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(cache)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_verified_archive_is_renamed_into_place_whole() {
    let f = fixture("ok");
    let dest = f.cache.join("abc");
    let pin = Pin {
        url: &f.url,
        sha256: &f.sha256,
        top_dir: "ghostty-abc",
    };
    fetch::fetch(&pin, &dest).unwrap();
    assert_eq!(
        fs::read_to_string(dest.join("include/ghostty/vt.h")).unwrap(),
        "// vt\n"
    );
    assert_eq!(cache_entries(&f.cache), ["abc"]);
}

#[test]
fn a_wrong_hash_names_both_hashes_and_caches_nothing() {
    let f = fixture("mismatch");
    let wrong = "0".repeat(64);
    let pin = Pin {
        url: &f.url,
        sha256: &wrong,
        top_dir: "ghostty-abc",
    };
    let err = fetch::fetch(&pin, &f.cache.join("abc")).unwrap_err();
    assert!(err.contains(&f.sha256), "{err}");
    assert!(err.contains(&wrong), "{err}");
    assert!(err.contains(&f.url), "{err}");
    assert!(cache_entries(&f.cache).is_empty());
}

#[test]
fn a_failed_download_caches_nothing() {
    let f = fixture("missing");
    fs::remove_file(&f.archive).unwrap();
    let pin = Pin {
        url: &f.url,
        sha256: &f.sha256,
        top_dir: "ghostty-abc",
    };
    let err = fetch::fetch(&pin, &f.cache.join("abc")).unwrap_err();
    assert!(err.contains(&f.url), "{err}");
    assert!(cache_entries(&f.cache).is_empty());
}

#[test]
fn an_archive_without_the_top_directory_caches_nothing() {
    let f = fixture("layout");
    let pin = Pin {
        url: &f.url,
        sha256: &f.sha256,
        top_dir: "ghostty-other",
    };
    let err = fetch::fetch(&pin, &f.cache.join("abc")).unwrap_err();
    assert!(err.contains("ghostty-other"), "{err}");
    assert!(cache_entries(&f.cache).is_empty());
}

#[test]
fn a_tree_another_build_put_in_place_first_is_kept() {
    let f = fixture("race");
    let dest = f.cache.join("abc");
    fs::create_dir_all(dest.join("include/ghostty")).unwrap();
    fs::write(dest.join("include/ghostty/vt.h"), "// first\n").unwrap();
    let pin = Pin {
        url: &f.url,
        sha256: &f.sha256,
        top_dir: "ghostty-abc",
    };
    fetch::fetch(&pin, &dest).unwrap();
    assert_eq!(
        fs::read_to_string(dest.join("include/ghostty/vt.h")).unwrap(),
        "// first\n"
    );
    assert_eq!(cache_entries(&f.cache), ["abc"]);
}
