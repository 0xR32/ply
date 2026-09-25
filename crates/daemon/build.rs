//! Stamps plyd with its build id, `<package version>+<commit>`: the 12-character hash of the checkout's `HEAD`, or
//! `t<unix seconds>` of this script's run outside a git checkout. It reaches the crate as `PLYD_BUILD_ID`; C1's
//! `welcome.daemon_version` carries it, and the app compares it with its own id (`app/src/ipc/os.ts`, same format) to
//! say that the running plyd is from another build. The script runs `git` read-only, offline, and reruns when `HEAD`
//! or a branch moves.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Length of the abbreviated commit hash; the app abbreviates to the same length, so equal builds give equal ids.
const HASH_LEN: &str = "--short=12";

/// The trimmed standard output of `git -C <dir> <args>`, when git runs and succeeds with some output.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn main() {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    println!("cargo:rerun-if-changed=build.rs");
    let commit = match git(&dir, &["rev-parse", HASH_LEN, "HEAD"]) {
        Some(hash) => {
            // A missing path would rerun this script on every build, so only existing ones are watched.
            for name in ["HEAD", "packed-refs", "refs/heads"] {
                if let Some(path) =
                    git(&dir, &["rev-parse", "--git-path", name]).map(|p| dir.join(p))
                    && path.exists()
                {
                    println!("cargo:rerun-if-changed={}", path.display());
                }
            }
            hash
        }
        None => {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            format!("t{secs}")
        }
    };
    println!("cargo:rustc-env=PLYD_BUILD_ID={version}+{commit}");
}
