//! Dropped files and a ⌘V image reach the CLI as typed paths, as `app/src/terminal/drop.ts` does; nothing is read.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{Image, ImageFormat};

// Ghostty's set (macOS `Shell.escape`): the CLIs recognise a dropped image path in this form, as iTerm2 and Terminal.app send it.
const SHELL_META: &[char] = &[
    '\\', ' ', '(', ')', '[', ']', '{', '}', '<', '>', '"', '\'', '`', '!', '#', '$', '&', ';',
    '|', '*', '?', '\t',
];

// AppKit's item-replacement staging (the ⌘⇧4 thumbnail's `NSIRD_screencaptureui_*`), which the source deletes when the drag ends.
const STAGED: &str = "/TemporaryItems/";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `path` with its shell metacharacters backslash-escaped, as a terminal types a dropped file.
fn escaped(path: &Path) -> String {
    let mut out = String::new();
    for c in path.to_string_lossy().chars() {
        if SHELL_META.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The text a terminal types for dropped files: each path escaped, joined by one space, none after.
pub fn typed(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| escaped(p))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `$PLY_HOME/ply-drops` when set, else `ply-drops` in the temporary directory, which macOS clears of old files.
fn drops_dir() -> PathBuf {
    std::env::var_os("PLY_HOME")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("ply-drops")
}

/// A new `pane-<id>-<ms>-<n>` folder for pane `pane_id`'s copies. errors: the folder cannot be created.
fn fresh_folder(pane_id: u64) -> std::io::Result<PathBuf> {
    let dir = drops_dir();
    std::fs::create_dir_all(&dir)?;
    let ms = now_ms();
    for n in 0..100 {
        let folder = dir.join(format!("pane-{pane_id}-{ms}-{}-{n}", std::process::id()));
        match std::fs::create_dir(&folder) {
            Ok(()) => return Ok(folder),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free drop folder"))
}

/// A staged `TemporaryItems` drop is linked (else copied) into the pane's folder before its source deletes it; synchronous.
pub fn keep_dropped(path: &Path, pane_id: u64) -> PathBuf {
    if !path.to_string_lossy().contains(STAGED) || !path.exists() {
        return path.to_path_buf();
    }
    let Some(name) = path.file_name() else {
        return path.to_path_buf();
    };
    let kept = match fresh_folder(pane_id) {
        Ok(folder) => folder.join(name),
        Err(e) => {
            tracing::warn!(pane_id, error = %e, "a staged drop could not be kept; typing its own path");
            return path.to_path_buf();
        }
    };
    if let Err(e) = std::fs::hard_link(path, &kept) {
        tracing::debug!(pane_id, error = %e, "a staged drop could not be linked; copying it");
        if let Err(e) = std::fs::copy(path, &kept) {
            tracing::warn!(pane_id, error = %e, "a staged drop could not be kept; typing its own path");
            return path.to_path_buf();
        }
    }
    tracing::info!(pane_id, from = %path.display(), to = %kept.display(), "kept a dropped file its source deletes when the drag ends");
    kept
}

/// The pasted image as a file in the pane's folder, turned into PNG with `sips` when the CLIs cannot read its format.
pub fn save_pasted_image(image: &Image, pane_id: u64) -> Option<PathBuf> {
    let folder = match fresh_folder(pane_id) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(pane_id, error = %e, "a pasted image could not be saved");
            return None;
        }
    };
    let ext = match image.format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        ImageFormat::Webp => "webp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Svg => "svg",
    };
    let raw = folder.join(format!("pasted.{ext}"));
    if let Err(e) = std::fs::write(&raw, &image.bytes) {
        tracing::warn!(pane_id, error = %e, "a pasted image could not be saved");
        return None;
    }
    if matches!(ext, "png" | "jpg" | "gif" | "webp") {
        return Some(raw);
    }
    let png = folder.join("pasted.png");
    match Command::new("/usr/bin/sips")
        .args(["-s", "format", "png"])
        .arg(&raw)
        .arg("--out")
        .arg(&png)
        .output()
    {
        Ok(out) if out.status.success() => Some(png),
        Ok(out) => {
            tracing::warn!(pane_id, status = %out.status, "sips could not turn a pasted image into PNG");
            Some(raw)
        }
        Err(e) => {
            tracing::warn!(pane_id, error = %e, "sips could not be run for a pasted image");
            Some(raw)
        }
    }
}

/// Deletes pane `pane_id`'s copies made before `before` (ms since the epoch; `u64::MAX` for all of them).
pub fn release(pane_id: u64, before: u64) {
    let dir = drops_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let prefix = format!("pane-{pane_id}-");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(made) = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.split('-').next()?.parse::<u64>().ok())
        else {
            continue;
        };
        if made < before
            && let Err(e) = std::fs::remove_dir_all(entry.path())
        {
            tracing::warn!(pane_id, error = %e, "a kept drop could not be deleted");
        }
    }
}

/// The time a pane's turn starts, for [`release`].
pub fn stamp() -> u64 {
    now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_paths_are_escaped_as_terminals_type_them() {
        let paths = vec![
            PathBuf::from("/Users/example/My Shot (1).png"),
            PathBuf::from("/tmp/a&b.jpg"),
        ];
        assert_eq!(
            typed(&paths),
            r"/Users/example/My\ Shot\ \(1\).png /tmp/a\&b.jpg"
        );
    }
}
