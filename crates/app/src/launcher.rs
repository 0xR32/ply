//! Starting plyd when nothing answers: the bundle's own plyd as a LaunchAgent, or a foreground one under `PLY_HOME`.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// `PLY_HOME`, else `~/Library/Application Support/ply`.
pub fn data_dir() -> PathBuf {
    std::env::var_os("PLY_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join("Library/Application Support/ply")
        })
}

/// `PLY_PLYD`, else the bundle's plyd beside this executable, else the checkout's release build, else its debug build.
fn find_plyd() -> Option<PathBuf> {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("plyd")));
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    [
        std::env::var_os("PLY_PLYD").map(PathBuf::from),
        beside,
        Some(repo.join("target/release/plyd")),
        Some(repo.join("target/debug/plyd")),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.is_file())
}

/// Makes sure plyd answers on `sock`: with `PLY_HOME` it starts `plyd --foreground` detached, else `plyd install-agent`
/// (plyd writes its own LaunchAgent); then waits up to 10 s. errors: plyd not built, failed to start, or never answering.
pub fn ensure_plyd(sock: &Path) -> Result<(), String> {
    if UnixStream::connect(sock).is_ok() {
        return Ok(());
    }
    let plyd =
        find_plyd().ok_or("plyd is not built (cargo build --release -p ply-daemon -p ply-hook)")?;
    if std::env::var_os("PLY_HOME").is_some() {
        Command::new(&plyd)
            .arg("--foreground")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("starting {}: {e}", plyd.display()))?;
        tracing::info!(plyd = %plyd.display(), "started plyd in the foreground");
    } else {
        let out = Command::new(&plyd)
            .arg("install-agent")
            .output()
            .map_err(|e| format!("plyd install-agent: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "plyd install-agent failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        tracing::info!(plyd = %plyd.display(), report = %String::from_utf8_lossy(&out.stdout).trim(), "plyd install-agent");
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if UnixStream::connect(sock).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!(
        "plyd did not answer on {} within 10 s",
        sock.display()
    ))
}
