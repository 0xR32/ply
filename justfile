# Idempotent: a patch whose reverse applies cleanly is already in and is skipped.
[doc("Apply patches/gpuix/*.patch inside vendor/gpuix, in order (INV-13)")]
vendor-patch:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{justfile_directory()}}/vendor/gpuix"
    for patch in "{{justfile_directory()}}"/patches/gpuix/*.patch; do
        name="$(basename "$patch")"
        if git apply --reverse --check "$patch" 2>/dev/null; then
            echo "vendor-patch: $name already applied, skipped"
            continue
        fi
        git apply --check "$patch"
        git apply "$patch"
        echo "vendor-patch: $name applied"
    done
