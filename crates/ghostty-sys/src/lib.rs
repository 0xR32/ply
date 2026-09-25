//! Raw bindings to libghostty-vt, Ghostty's terminal engine as a C library.
//!
//! This is the only crate in ply with FFI to libghostty-vt (spec 3.2, INV-17). Its `build.rs`
//! runs `zig build -Demit-lib-vt` on `vendor/libghostty-vt` with the flags ADR-0005 records
//! (offline `--system` packages, caches under `OUT_DIR`, never writing into the vendor tree) and
//! links the static library; `src/lib.rs` declares the C API by hand.
//!
//! It holds no logic (spec 3.2) and depends on no ply crate (spec 8.2). Only `ply-term` uses it,
//! and only with that crate's `engine` feature, which only `ply-daemon` enables. The bindings and
//! the build land in WP3; this crate is an empty placeholder until then.
