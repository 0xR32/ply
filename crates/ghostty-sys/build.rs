//! Builds libghostty-vt from the pinned ghostty source with Zig and links it statically (ADR-0005 Decision 1, ADR-0008
//! Decision 4).
//!
//! The source is `PLY_GHOSTTY_SRC` when set (offline builds), otherwise ghostty [`GHOSTTY_COMMIT`] in the user's cache
//! directory, downloaded once and checked against [`GHOSTTY_ARCHIVE_SHA256`] by `fetch.rs` (INV-17). The Zig packages
//! are `PLY_ZIG_PKG_DIR` when set, otherwise the release's dependency tarball, cached and checked the same way. Every
//! `zig build` here runs offline with `--system <pkgdir>` and keeps its prefix and caches under `OUT_DIR`, so the source
//! is never written to: without `--system`, Zig fetches packages into `<build root>/zig-pkg/`.

mod fetch;

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The ghostty commit libghostty-vt is built from.
const GHOSTTY_COMMIT: &str = "0fa3d577a5614150ff8e2c152de5fb9d5c6d2dec";

/// GitHub's tarball of [`GHOSTTY_COMMIT`]; its one top directory is `ghostty-<commit>/`.
const GHOSTTY_ARCHIVE_URL: &str = "https://codeload.github.com/ghostty-org/ghostty/tar.gz/0fa3d577a5614150ff8e2c152de5fb9d5c6d2dec";

/// The SHA-256 of the archive at [`GHOSTTY_ARCHIVE_URL`]; a download that hashes differently is rejected.
const GHOSTTY_ARCHIVE_SHA256: &str =
    "2f38f10edcc7498a91a4b02baf317f408c9808f637b341c22966b2c7522b95ea";

/// `-Dlib-version-string`: the libghostty-vt release tagged at [`GHOSTTY_COMMIT`] (`vt-1.0.0`), which the git tree,
/// lacking its `LIB_VERSION` file, would report as `1.0.0-dev`.
const LIBGHOSTTY_VT_VERSION: &str = "1.0.0";

/// That release's dependency tarball: the Zig packages of its `deps/libghostty-vt.txt`, each unpacked under its hash.
const DEPS_ARCHIVE_URL: &str =
    "https://release.files.ghostty.org/libghostty-vt/1.0.0/libghostty-vt-1.0.0-deps.tar.gz";

/// The SHA-256 of the archive at [`DEPS_ARCHIVE_URL`], as the release's signed `SHA256SUMS` lists it.
const DEPS_ARCHIVE_SHA256: &str =
    "4d5ea5e448e4c7ea8fb55ccbaa8668f2a97d44f438cee831e2b7ecd162d85c29";

/// The Zig release ghostty's `build.zig.zon` requires (`minimum_zig_version`).
const ZIG_VERSION: &str = "0.17.0";

const OPTIMIZE_MODES: &[&str] = &["Debug", "ReleaseSafe", "ReleaseFast", "ReleaseSmall"];

fn main() {
    if let Err(message) = run() {
        eprintln!("error: ghostty-sys could not build libghostty-vt: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let out_dir = PathBuf::from(env_var("OUT_DIR")?);
    let target = env_var("TARGET")?;

    println!("cargo:rerun-if-changed=build.rs");
    for var in [
        "PLY_GHOSTTY_SRC",
        "ZIG",
        "LIBGHOSTTY_VT_OPTIMIZE",
        "PLY_ZIG_PKG_DIR",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let zig = env::var("ZIG").unwrap_or_else(|_| "zig".to_owned());
    check_zig_version(&zig)?;
    let zig_target = zig_target(&target)?;
    let optimize = env::var("LIBGHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".to_owned());
    if !OPTIMIZE_MODES.contains(&optimize.as_str()) {
        return Err(format!(
            "LIBGHOSTTY_VT_OPTIMIZE={optimize} is not one of {}",
            OPTIMIZE_MODES.join(", ")
        ));
    }
    let source = ghostty_source(&out_dir)?;
    // A purged cache makes the next build fetch the source again; input.test.ts reads its headers.
    println!(
        "cargo:rerun-if-changed={}",
        source.join("build.zig").display()
    );
    let pkg_dir = package_dir(&out_dir)?;

    let prefix = out_dir.join("zig-out");
    let status = Command::new(&zig)
        .current_dir(&source)
        .arg("build")
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg("-Dsimd=true")
        .arg(format!("-Dtarget={zig_target}"))
        .arg(format!("-Dlib-version-string={LIBGHOSTTY_VT_VERSION}"))
        .arg("-Demit-xcframework=false")
        .arg("--system")
        .arg(&pkg_dir)
        .arg("--prefix")
        .arg(&prefix)
        .arg("--cache-dir")
        .arg(out_dir.join("zig-cache"))
        // Zig 0.17 has no --global-cache-dir, and a global cache holding package archives can crash the build.
        .env("ZIG_GLOBAL_CACHE_DIR", out_dir.join("zig-global-cache"))
        .status()
        .map_err(|e| format!("cannot run `{zig} build`: {e}"))?;
    if !status.success() {
        return Err(format!("`{zig} build -Demit-lib-vt` failed with {status}"));
    }

    // A directory holding only the archive keeps `-lghostty-vt` from resolving to the dylib zig installs beside it.
    let archive = prefix.join("lib/libghostty-vt.a");
    let static_dir = out_dir.join("static");
    fs::create_dir_all(&static_dir)
        .map_err(|e| format!("cannot create {}: {e}", static_dir.display()))?;
    fs::copy(&archive, static_dir.join("libghostty-vt.a"))
        .map_err(|e| format!("cannot copy {}: {e}", archive.display()))?;
    println!("cargo:rustc-link-search=native={}", static_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    Ok(())
}

fn env_var(name: &str) -> Result<String, String> {
    env::var(name).map_err(|e| format!("{name} is not set by cargo: {e}"))
}

fn check_zig_version(zig: &str) -> Result<(), String> {
    let output = Command::new(zig).arg("version").output().map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            format!(
                "zig not found (looked for `{zig}`). Install Zig {ZIG_VERSION} and put `zig` on PATH, \
                 or point the ZIG environment variable at the binary"
            )
        } else {
            format!("cannot run `{zig} version`: {e}")
        }
    })?;
    let found = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if found == ZIG_VERSION {
        Ok(())
    } else {
        Err(format!(
            "libghostty-vt at ghostty {GHOSTTY_COMMIT} needs Zig {ZIG_VERSION}, but `{zig} version` printed {found:?}"
        ))
    }
}

/// The ghostty source tree: `PLY_GHOSTTY_SRC`, or [`GHOSTTY_COMMIT`] in the cache, downloaded and verified when absent.
fn ghostty_source(out_dir: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = env::var_os("PLY_GHOSTTY_SRC") {
        let dir = PathBuf::from(dir);
        check_source(&dir).map_err(|e| format!("PLY_GHOSTTY_SRC: {e}"))?;
        return Ok(dir);
    }
    let pin = fetch::Pin {
        url: GHOSTTY_ARCHIVE_URL,
        sha256: GHOSTTY_ARCHIVE_SHA256,
        top_dir: &format!("ghostty-{GHOSTTY_COMMIT}"),
    };
    let dest = cached(
        out_dir,
        &pin,
        GHOSTTY_COMMIT,
        "the ghostty source (about 40 MB)",
    )?;
    check_source(&dest).map_err(|e| format!("{e}; delete it to download it again"))?;
    Ok(dest)
}

/// The `--system` directory: `PLY_ZIG_PKG_DIR`, or the release's dependency tarball in the cache.
fn package_dir(out_dir: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = env::var_os("PLY_ZIG_PKG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let top_dir = format!("libghostty-vt-{LIBGHOSTTY_VT_VERSION}-deps");
    let pin = fetch::Pin {
        url: DEPS_ARCHIVE_URL,
        sha256: DEPS_ARCHIVE_SHA256,
        top_dir: &top_dir,
    };
    cached(
        out_dir,
        &pin,
        &top_dir,
        "libghostty-vt's Zig packages (about 6 MB)",
    )
}

/// `name` in the cache directory, holding `pin`'s top directory; downloaded and verified first when absent.
fn cached(out_dir: &Path, pin: &fetch::Pin<'_>, name: &str, what: &str) -> Result<PathBuf, String> {
    let dest = cache_dir()?.join(name);
    if !dest.exists() {
        announce(
            out_dir,
            &format!("downloading {what}, once, from {}", pin.url),
        )?;
        fetch::fetch(pin, &dest)?;
        announce(
            out_dir,
            &format!(
                "verified (SHA-256 {}) and cached in {}",
                pin.sha256,
                dest.display()
            ),
        )?;
    }
    Ok(dest)
}

/// Prints a download notice as a cargo warning, shown by this build only.
fn announce(out_dir: &Path, message: &str) -> Result<(), String> {
    println!("cargo:warning={message}");
    // Cargo replays a fresh build script's warnings on every build; a stamp newer than this run reruns it once, quietly.
    let stamp = out_dir.join("downloaded.stamp");
    fs::write(&stamp, "").map_err(|e| format!("cannot write {}: {e}", stamp.display()))?;
    println!("cargo:rerun-if-changed={}", stamp.display());
    Ok(())
}

/// Where downloaded ghostty sources live: `~/Library/Caches/ply/ghostty` on macOS, the XDG cache elsewhere.
fn cache_dir() -> Result<PathBuf, String> {
    let home = env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Caches"))
    } else {
        env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".cache")))
    };
    base.map(|b| b.join("ply/ghostty")).ok_or_else(|| {
        "HOME is not set, so there is no cache directory for the ghostty source; set HOME, or PLY_GHOSTTY_SRC and \
         PLY_ZIG_PKG_DIR to an extracted ghostty source tree and its packages"
            .to_owned()
    })
}

fn check_source(dir: &Path) -> Result<(), String> {
    for file in ["build.zig", "include/ghostty/vt.h"] {
        if !dir.join(file).is_file() {
            return Err(format!(
                "{} is not a ghostty source tree (it has no {file})",
                dir.display()
            ));
        }
    }
    Ok(())
}

fn zig_target(target: &str) -> Result<&'static str, String> {
    Ok(match target {
        "aarch64-apple-darwin" => "aarch64-macos",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        other => return Err(format!("no Zig target known for the Rust target {other}")),
    })
}
