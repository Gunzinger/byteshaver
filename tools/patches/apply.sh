#!/usr/bin/env bash
# Applies the repository's vendored-crate patches to their submodules.
# Idempotent: skips patches that are already applied.
# Works both inside and outside git checkouts (uses patch(1), not git apply).
#
# Prerequisites: git submodules initialized
#   git submodule update --init --recursive
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

apply() { # <submodule-dir> <crate-subdir> <patch-file>
    local dir="$ROOT/$1" crate="$2" patch="$ROOT/$3"
    [ -d "$dir/$crate" ] || {
        echo "ERROR: $1 not initialized (git submodule update --init --recursive)" >&2
        exit 1
    }
    if grep -rq "byteshaver patch" "$dir/$crate/src" 2>/dev/null; then
        echo "already applied: $3"
        return
    fi
    patch -d "$dir" -p1 --forward < "$patch"
    echo "applied: $3 -> $1"
}

apply tools/jpegxl-src jpegxl-src tools/patches/jpegxl-src-avx512.patch
