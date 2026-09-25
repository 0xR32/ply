//! Builds libghostty-vt from the pinned ghostty source with Zig and links it statically (ADR-0005 Decision 1, ADR-0008
//! Decision 4).
//!
//! The source is `PLY_GHOSTTY_SRC` when set (offline builds), otherwise ghostty [`GHOSTTY_COMMIT`] in the user's cache
//! directory, downloaded once and checked against [`GHOSTTY_ARCHIVE_SHA256`] by `fetch.rs` (INV-17). Every `zig build`
//! here runs offline with `--system <pkgdir>` and keeps its prefix and caches under `OUT_DIR`, so the source is never
//! written to: without `--system`, Zig 0.16 fetches packages into `<build root>/zig-pkg/`. The package directory is
//! `PLY_ZIG_PKG_DIR` when set, otherwise it is assembled in `OUT_DIR` from Zig's global cache, into which `zig fetch`
//! puts any package it lacks.

mod fetch;

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The ghostty commit libghostty-vt is built from.
const GHOSTTY_COMMIT: &str = "44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51";

/// GitHub's tarball of [`GHOSTTY_COMMIT`]; its one top directory is `ghostty-<commit>/`.
const GHOSTTY_ARCHIVE_URL: &str = "https://codeload.github.com/ghostty-org/ghostty/tar.gz/44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51";

/// The SHA-256 of the archive at [`GHOSTTY_ARCHIVE_URL`]; a download that hashes differently is rejected.
const GHOSTTY_ARCHIVE_SHA256: &str =
    "7bd1a8b6ce5c1b3bbab67020a84761139d15f83ceea7738fa6d3ddd1b759a779";

/// `-Dversion-string`: the `VERSION` file ghostty's source release carries at this commit, which the git tree lacks.
const GHOSTTY_VERSION: &str = "1.3.2-HEAD-+44f2a44df";

/// The Zig release ghostty's `build.zig.zon` requires (`minimum_zig_version`).
const ZIG_VERSION: &str = "0.16.0";

/// The packages `zig build -Demit-lib-vt` needs at [`GHOSTTY_COMMIT`]: name, Zig package hash, source URL.
const PACKAGES: &[(&str, &str, &str)] = &[
    (
        "translate_c",
        "translate_c-0.0.0-Q_BUWhVNBwDOEcIqub4VFPJPB6D9dgwzUMHTX5KWr8Xr",
        "https://codeberg.org/vancluever/translate-c/archive/4e879eb8aba615de112eabd1231ea6e01920cead.tar.gz",
    ),
    (
        "aro",
        "aro-0.0.0-JSD1Qk6lNgDdcDV4Vh7Sfy-34m2TluIVOdPzMmj_0BjX",
        "https://github.com/vancluever/arocc/archive/f97cdfc3779aec4b242299e2fc9a1c828c3547c6.tar.gz",
    ),
    (
        "uucode",
        "uucode-0.2.0-ZZjBPlK5VADj7fdoq7G8LIHzD5o6FSkcBXXrRWr4jnrA",
        "https://deps.files.ghostty.org/uucode-2826a37a4562284fdacd8fa029d49509cc9bffcd.tar.gz",
    ),
    (
        "zlib",
        "N-V-__8AAB0eQwD-0MdOEBmz7intriBReIsIDNlukNVoNu6o",
        "https://deps.files.ghostty.org/zlib-1220fed0c74e1019b3ee29edae2051788b080cd96e90d56836eea857b0b966742efb.tar.gz",
    ),
    (
        "highway",
        "N-V-__8AAGmZhABbsPJLfbqrh6JTHsXhY6qCaLAQyx25e0XE",
        "https://deps.files.ghostty.org/highway-66486a10623fa0d72fe91260f96c892e41aceb06.tar.gz",
    ),
    (
        "wuffs",
        "N-V-__8AAP5JWgCGP_AD0teWpa4krRvE9VPZzvviGdbmN4jI",
        "https://deps.files.ghostty.org/wuffs-7411f488fe2e2c205c3d3b3d28638b7356522930.tar.gz",
    ),
    (
        "pixels",
        "N-V-__8AADYiAAB_80AWnH1AxXC0tql9thT-R-DYO1gBqTLc",
        "https://deps.files.ghostty.org/pixels-12207ff340169c7d40c570b4b6a97db614fe47e0d83b5801a932dcd44917424c8806.tar.gz",
    ),
    (
        "iterm2_themes",
        "N-V-__8AAEFmBABuDGOKxAI6VMg41b9euMZ-z7HS9EcUdaor",
        "https://deps.files.ghostty.org/ghostty-themes-release-20260831-151010-752a9c0.tgz",
    ),
];

// `--system` also turns on every system integration `zig build -Demit-lib-vt --help` lists; ply builds none of them.
const NO_SYSTEM: &[&str] = &[
    "freetype",
    "harfbuzz",
    "fontconfig",
    "libpng",
    "zlib",
    "oniguruma",
    "glslang",
    "spirv-cross",
    "simdutf",
    "gtk4-layer-shell",
    "highway",
];

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
    let pkg_dir = package_dir(&zig, &out_dir)?;

    let prefix = out_dir.join("zig-out");
    let mut cmd = Command::new(&zig);
    cmd.current_dir(&source)
        .arg("build")
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg("-Dsimd=true")
        .arg(format!("-Dtarget={zig_target}"))
        .arg(format!("-Dversion-string={GHOSTTY_VERSION}"))
        .arg("-Demit-xcframework=false")
        .arg("--system")
        .arg(&pkg_dir);
    for integration in NO_SYSTEM {
        cmd.arg(format!("-fno-sys={integration}"));
    }
    cmd.arg("--prefix")
        .arg(&prefix)
        .arg("--cache-dir")
        .arg(out_dir.join("zig-cache"))
        .arg("--global-cache-dir")
        .arg(out_dir.join("zig-global-cache"));
    let status = cmd
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
    let dest = cache_dir()?.join(GHOSTTY_COMMIT);
    if !dest.exists() {
        announce(
            out_dir,
            &format!(
                "downloading the ghostty {GHOSTTY_COMMIT} source (about 40 MB, once) from {GHOSTTY_ARCHIVE_URL}"
            ),
        )?;
        let pin = fetch::Pin {
            url: GHOSTTY_ARCHIVE_URL,
            sha256: GHOSTTY_ARCHIVE_SHA256,
            top_dir: &format!("ghostty-{GHOSTTY_COMMIT}"),
        };
        fetch::fetch(&pin, &dest)?;
        announce(
            out_dir,
            &format!(
                "ghostty source verified (SHA-256 {GHOSTTY_ARCHIVE_SHA256}) and cached in {}",
                dest.display()
            ),
        )?;
    }
    check_source(&dest).map_err(|e| format!("{e}; delete it to download it again"))?;
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
        "HOME is not set, so there is no cache directory for the ghostty source; set HOME, or PLY_GHOSTTY_SRC to \
         an extracted ghostty source tree"
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

/// The `--system` directory: `PLY_ZIG_PKG_DIR`, or one assembled in `OUT_DIR` from Zig's global package cache.
fn package_dir(zig: &str, out_dir: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = env::var_os("PLY_ZIG_PKG_DIR") {
        let dir = PathBuf::from(dir);
        let missing: Vec<&str> = PACKAGES
            .iter()
            .filter(|(_, hash, _)| !dir.join(hash).is_dir())
            .map(|(name, _, _)| *name)
            .collect();
        return if missing.is_empty() {
            Ok(dir)
        } else {
            Err(format!(
                "PLY_ZIG_PKG_DIR={} lacks the packages {}",
                dir.display(),
                missing.join(", ")
            ))
        };
    }

    let pkg_dir = out_dir.join("zig-pkg");
    let cache = global_cache_dir(zig)?.join("p");
    for (name, hash, url) in PACKAGES {
        if pkg_dir.join(hash).is_dir() {
            continue;
        }
        let archive = cache.join(format!("{hash}.tar.gz"));
        if !archive.is_file() {
            fetch_package(zig, out_dir, name, hash, url)?;
            if !archive.is_file() {
                return Err(format!(
                    "`{zig} fetch {url}` did not put {} into Zig's package cache",
                    archive.display()
                ));
            }
        }
        fs::create_dir_all(&pkg_dir)
            .map_err(|e| format!("cannot create {}: {e}", pkg_dir.display()))?;
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&pkg_dir)
            .status()
            .map_err(|e| format!("cannot run tar: {e}"))?;
        if !status.success() || !pkg_dir.join(hash).is_dir() {
            return Err(format!(
                "extracting {} did not produce {hash}/ ({status})",
                archive.display()
            ));
        }
    }
    Ok(pkg_dir)
}

/// Runs `zig fetch <url>` into Zig's global cache and checks that the package hash it prints is `hash`.
fn fetch_package(
    zig: &str,
    out_dir: &Path,
    name: &str,
    hash: &str,
    url: &str,
) -> Result<(), String> {
    announce(
        out_dir,
        &format!("fetching the Zig package {name} (once) from {url}"),
    )?;
    // zig fetch needs a build root and extracts a copy into its zig-pkg/, so the root is an empty one under OUT_DIR.
    let root = out_dir.join("zig-fetch");
    fs::create_dir_all(&root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    fs::write(root.join("build.zig"), "")
        .map_err(|e| format!("cannot write {}: {e}", root.join("build.zig").display()))?;
    let output = Command::new(zig)
        .current_dir(&root)
        .arg("fetch")
        .arg(url)
        .output()
        .map_err(|e| format!("cannot run `{zig} fetch`: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`{zig} fetch {url}` failed ({}): {}; without network, set PLY_ZIG_PKG_DIR to a directory holding the \
             extracted packages by hash",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let fetched = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if fetched == hash {
        Ok(())
    } else {
        Err(format!(
            "`{zig} fetch {url}` produced the package hash {fetched}, but ghostty {GHOSTTY_COMMIT} pins {name} at {hash}"
        ))
    }
}

/// Zig's global cache directory, as `zig env` reports it (it honours `ZIG_GLOBAL_CACHE_DIR` and `XDG_CACHE_HOME`).
fn global_cache_dir(zig: &str) -> Result<PathBuf, String> {
    let output = Command::new(zig)
        .arg("env")
        .output()
        .map_err(|e| format!("cannot run `{zig} env`: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix(".global_cache_dir = \"")?;
            rest.strip_suffix("\",").map(PathBuf::from)
        })
        .ok_or_else(|| format!("`{zig} env` did not report .global_cache_dir"))
}
