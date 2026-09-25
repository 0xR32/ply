//! Builds the vendored libghostty-vt with Zig and links it statically (ADR-0005 Decision 1, ADR-0008 Decision 4).
//!
//! Every `zig build` here runs offline with `--system <pkgdir>` and keeps its prefix and caches under `OUT_DIR`, so
//! `vendor/libghostty-vt` is never written to (INV-17): without `--system`, Zig 0.16 fetches packages into
//! `<build root>/zig-pkg/`. The package directory is `PLY_ZIG_PKG_DIR` when set, otherwise it is assembled in
//! `OUT_DIR` from the package archives in Zig's global cache.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Zig release `vendor/libghostty-vt/build.zig.zon` requires (`minimum_zig_version`).
const ZIG_VERSION: &str = "0.16.0";

/// The packages `zig build -Demit-lib-vt` needs at ghostty 44f2a44: name, Zig package hash, source URL.
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
    let manifest_dir = PathBuf::from(env_var("CARGO_MANIFEST_DIR")?);
    let vendor = manifest_dir.join("../../vendor/libghostty-vt");
    let out_dir = PathBuf::from(env_var("OUT_DIR")?);
    let target = env_var("TARGET")?;

    println!("cargo:rerun-if-changed=build.rs");
    for entry in [
        "build.zig",
        "build.zig.zon",
        "VERSION",
        "include",
        "src",
        "pkg",
    ] {
        println!("cargo:rerun-if-changed={}", vendor.join(entry).display());
    }
    for var in [
        "ZIG",
        "LIBGHOSTTY_VT_OPTIMIZE",
        "PLY_ZIG_PKG_DIR",
        "ZIG_GLOBAL_CACHE_DIR",
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
    let version = fs::read_to_string(vendor.join("VERSION"))
        .map_err(|e| format!("cannot read {}: {e}", vendor.join("VERSION").display()))?
        .trim()
        .to_owned();
    let pkg_dir = package_dir(&zig, &out_dir)?;

    let prefix = out_dir.join("zig-out");
    let mut cmd = Command::new(&zig);
    cmd.current_dir(&vendor)
        .arg("build")
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg("-Dsimd=true")
        .arg(format!("-Dtarget={zig_target}"))
        .arg(format!("-Dversion-string={version}"))
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
            "libghostty-vt at ghostty 44f2a44 needs Zig {ZIG_VERSION}, but `{zig} version` printed {found:?}"
        ))
    }
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
    let mut missing = String::new();
    for (name, hash, url) in PACKAGES {
        if pkg_dir.join(hash).is_dir() {
            continue;
        }
        let archive = cache.join(format!("{hash}.tar.gz"));
        println!("cargo:rerun-if-changed={}", archive.display());
        if !archive.is_file() {
            let _ = writeln!(missing, "    {zig} fetch {url}    # {name}");
            continue;
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
    if missing.is_empty() {
        Ok(pkg_dir)
    } else {
        Err(format!(
            "Zig's package cache {} lacks packages the offline build needs. Fetch them once (online), from a \
             scratch directory holding an empty build.zig (never inside vendor/libghostty-vt), with\n{missing}\
             or set PLY_ZIG_PKG_DIR to a directory holding the extracted packages by hash",
            cache.display()
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
