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

# Installs Geist and Geist Mono for this user: GPUIX loads no font file, so the app finds them only in ~/Library/Fonts.
fonts:
    mkdir -p ~/Library/Fonts
    cp app/assets/fonts/*.ttf ~/Library/Fonts/

# Journeys J1–J6 on the full app against the release plyd and the fake CLIs; needs a GUI session, opens windows unfocused.
e2e:
    cargo build --release --locked -p ply-daemon -p ply-hook
    PLY_E2E=1 bun test --timeout 120000 ./app/e2e
