set shell := ["bash", "-euo", "pipefail", "-c"]

default: check

# Every gate CI runs except nextest and cargo-deny: formatting, lints, rustdoc, repo rules, INV-16, Biome and tsc on scripts/.
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
    bun scripts/check-rules.ts
    bun scripts/check-deps.ts
    bunx biome check
    bunx tsc -p scripts

test:
    cargo nextest run --workspace --locked --no-tests=pass
    cargo test --doc --workspace --locked
    bun test ./scripts

# Licences, advisories and the INV-1 crate ban.
deny:
    cargo deny check

fmt:
    cargo fmt --all
    bunx biome check --write

# crates/app/assets/icon/ply.icns from ply.svg, every size macOS asks for (needs resvg: brew install resvg).
icon:
    rm -rf target/ply.iconset && mkdir -p target/ply.iconset
    for s in 16 32 128 256 512; do resvg crates/app/assets/icon/ply.svg target/ply.iconset/icon_${s}x${s}.png -w $s && resvg crates/app/assets/icon/ply.svg target/ply.iconset/icon_${s}x${s}@2x.png -w $((s * 2)); done
    iconutil -c icns target/ply.iconset -o crates/app/assets/icon/ply.icns

# dist/ply.app and dist/ply-<version>.dmg: the GPUI app, plyd and ply-hook at profile dist, ad-hoc signed.
dmg:
    bun scripts/dmg.ts
