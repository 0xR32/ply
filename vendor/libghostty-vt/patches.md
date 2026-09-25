# libghostty-vt local patches

None. `vendor/libghostty-vt` is the pristine ghostty source at 44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51
(ADR-0008). `bun scripts/check-rules.ts` fails when the tree differs from `content_hash` in `vendor.json`.

A local patch needs an ADR, an entry here (reason, files touched, upstream link, how to verify) and a
new `content_hash` in `vendor.json` (`bun scripts/check-rules.ts --vendor-hash` prints it).
