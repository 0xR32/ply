//! Puts a pinned source archive into a cache directory for `build.rs`: curl downloads it, `shasum -a 256` checks it,
//! tar extracts it into a staging directory beside the destination, and one `rename` moves the tree into place, so a
//! concurrent or interrupted build never sees a half-extracted tree. Nothing is left behind when a step fails.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

/// Staging files older than this in the cache directory are leftovers of an interrupted build.
const STALE_AFTER: Duration = Duration::from_secs(3600);

/// A source archive and what it must hash to.
pub struct Pin<'a> {
    /// Where curl downloads the archive from (any URL curl accepts; tests use `file://`).
    pub url: &'a str,
    /// The archive's SHA-256, lowercase hex; a download that hashes differently is rejected.
    pub sha256: &'a str,
    /// The archive's single top-level directory, which becomes the destination.
    pub top_dir: &'a str,
}

/// Blocks while `dest` becomes the verified archive's top directory (a tree another build moved there first is kept); errors: a failing tool, a SHA-256 mismatch naming both hashes and the URL, a missing top directory, cache I/O — `dest` is then untouched.
pub fn fetch(pin: &Pin<'_>, dest: &Path) -> Result<(), String> {
    let parent = dest
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", dest.display()))?;
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} does not end in a UTF-8 name", dest.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    sweep_stale(parent, name);

    let tag = format!(".{name}.{}", std::process::id());
    let archive = Staged(parent.join(format!("{tag}.tar.gz")));
    let staging = Staged(parent.join(format!("{tag}.tmp")));
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(&archive.0)
        .arg(pin.url)
        .status()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if !status.success() {
        return Err(format!("curl could not download {} ({status})", pin.url));
    }
    let actual = sha256(&archive.0)?;
    if !actual.eq_ignore_ascii_case(pin.sha256) {
        return Err(format!(
            "the archive downloaded from {} has SHA-256 {actual}, but the pin expects {}; nothing was cached",
            pin.url, pin.sha256
        ));
    }

    fs::create_dir(&staging.0)
        .map_err(|e| format!("cannot create {}: {e}", staging.0.display()))?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive.0)
        .arg("-C")
        .arg(&staging.0)
        .status()
        .map_err(|e| format!("cannot run tar: {e}"))?;
    if !status.success() {
        return Err(format!(
            "tar could not extract the archive from {} ({status})",
            pin.url
        ));
    }
    let tree = staging.0.join(pin.top_dir);
    if !tree.is_dir() {
        return Err(format!(
            "the archive from {} has no top directory {}/",
            pin.url, pin.top_dir
        ));
    }
    match fs::rename(&tree, dest) {
        Ok(()) => Ok(()),
        Err(_) if dest.is_dir() => Ok(()),
        Err(e) => Err(format!(
            "cannot move {} to {}: {e}",
            tree.display(),
            dest.display()
        )),
    }
}

/// The SHA-256 of `file` as lowercase hex, from `shasum -a 256`; errors when shasum cannot run or prints no hash.
pub fn sha256(file: &Path) -> Result<String, String> {
    let output = Command::new("shasum")
        .args(["-a", "256"])
        .arg(file)
        .output()
        .map_err(|e| format!("cannot run shasum: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    match text.split_whitespace().next() {
        Some(hash) if output.status.success() && hash.len() == 64 => Ok(hash.to_ascii_lowercase()),
        _ => Err(format!(
            "`shasum -a 256 {}` failed ({}): {}",
            file.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// Removes staging files for `name` that an interrupted build left in `parent`; best effort, errors are ignored.
fn sweep_stale(parent: &Path, name: &str) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let prefix = format!(".{name}.");
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > STALE_AFTER);
        if stale && entry.file_name().to_string_lossy().starts_with(&prefix) {
            remove(&entry.path());
        }
    }
}

/// A staging file or directory, removed when dropped (after a successful rename there is nothing left to remove).
struct Staged(PathBuf);

impl Drop for Staged {
    fn drop(&mut self) {
        remove(&self.0);
    }
}

/// Removes a file or a directory tree; best effort, a path that is already gone is fine.
fn remove(path: &Path) {
    if fs::remove_dir_all(path).is_err() {
        let _ = fs::remove_file(path);
    }
}
