set shell := ["bash", "-euo", "pipefail", "-c"]

default: check

# Every gate CI runs except nextest and cargo-deny: formatting, lints, rustdoc, repo rules, INV-16, Biome, tsc.
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
    bun scripts/check-rules.ts
    bun scripts/check-deps.ts
    bunx biome check
    bunx tsc -p app
    bunx tsc -p scripts

test:
    cargo nextest run --workspace --locked --no-tests=pass
    cargo test --doc --workspace --locked
    bun test ./app ./scripts

# Licences, advisories and the INV-1/INV-3 crate bans.
deny:
    cargo deny check

gen:
    bun scripts/gen.ts

dev:
    bun --hot app/src/main.tsx

fmt:
    cargo fmt --all
    bunx biome check --write

# app/assets/icon/ply.icns from ply.svg, every size macOS asks for (needs resvg: brew install resvg).
icon:
    rm -rf target/ply.iconset && mkdir -p target/ply.iconset
    for s in 16 32 128 256 512; do resvg app/assets/icon/ply.svg target/ply.iconset/icon_${s}x${s}.png -w $s && resvg app/assets/icon/ply.svg target/ply.iconset/icon_${s}x${s}@2x.png -w $((s * 2)); done
    iconutil -c icns target/ply.iconset -o app/assets/icon/ply.icns

# dist/ply.app and dist/ply-<version>.dmg: plyd at profile dist, the app compiled by Bun to bytecode, ad-hoc signed.
dmg:
    bun scripts/dmg.ts

# Installs Geist and Geist Mono for this user: GPUIX loads no font file, so the app finds them only in ~/Library/Fonts.
fonts:
    mkdir -p ~/Library/Fonts
    cp app/assets/fonts/*.ttf ~/Library/Fonts/

# Journeys J1–J7 on the full app against the release plyd and the fake CLIs; needs a GUI session, opens windows unfocused.
e2e:
    cargo build --release --locked -p ply-daemon -p ply-hook
    PLY_E2E=1 bun test --timeout 120000 ./app/e2e
