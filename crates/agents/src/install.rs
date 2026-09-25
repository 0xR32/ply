//! Read-only probes of a CLI's install layout for its version, so the spawn-time check never executes the CLI (R14).

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{Error, Result, json};
use crate::version::CliVersion;

/// Upper bound on a metadata file; a real `package.json` is a few KiB.
const MAX_METADATA_BYTES: u64 = 1 << 20;

/// `exe` with every symlink resolved; fails with [`Error::Io`] when `exe` does not exist.
pub(crate) fn resolve(exe: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(exe).map_err(|source| Error::Io {
        path: exe.to_path_buf(),
        source,
    })
}

/// The `version` of a JSON metadata file if it exists and, when `name` is given, its `name` field matches.
pub(crate) fn json_version(path: &Path, name: Option<&str>) -> Result<Option<CliVersion>> {
    if !path.is_file() {
        return Ok(None);
    }
    let io = |source| Error::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(io)?
        .take(MAX_METADATA_BYTES)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    let doc: Value = serde_json::from_slice(&bytes).map_err(json("install metadata"))?;
    if name.is_some_and(|n| doc.get("name").and_then(Value::as_str) != Some(n)) {
        return Ok(None);
    }
    doc.get("version")
        .and_then(Value::as_str)
        .map(str::parse)
        .transpose()
}

/// The version named by the path component right after `marker` (e.g. Homebrew's `Caskroom/<cask>/<version>/…`).
pub(crate) fn version_after(path: &Path, marker: &[&str]) -> Option<CliVersion> {
    let parts: Vec<&str> = path.iter().filter_map(|c| c.to_str()).collect();
    parts
        .windows(marker.len() + 1)
        .rev()
        .find(|w| w[..marker.len()] == *marker)
        .and_then(|w| w[marker.len()].parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_component_after_a_marker() {
        let cask = Path::new("/opt/homebrew/Caskroom/codex/0.156.1/codex-aarch64-apple-darwin");
        assert_eq!(
            version_after(cask, &["Caskroom", "codex"]),
            Some(CliVersion::new(0, 156, 1))
        );
        assert_eq!(version_after(cask, &["Caskroom", "claude-code"]), None);
        assert_eq!(
            version_after(Path::new("/usr/bin/codex"), &["Cellar", "codex"]),
            None
        );
    }
}
