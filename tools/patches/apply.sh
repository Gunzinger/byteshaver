#!/usr/bin/env bash
# Applies the repository's vendored-crate patches to their submodules.
# Idempotent: skips patches that are already applied.
#
# Prerequisites: git submodules initialized
#   git submodule update --init --recursive
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APPLIED=0

apply() { # <submodule-dir> <patch-file>
    local dir="$ROOT/$1" patch="$ROOT/$2"
    [ -d "$dir/.git" ] || { echo "ERROR: $1 is not initialized (git submodule update --init --recursive)" >&2; exit 1; }
    if grep -rq "byteshaver patch" "$dir/src" 2>/dev/null; then
        echo "already applied: $2"
        return
    fi
    git -C "$dir" apply --whitespace=nowarn "$patch"
    echo "applied: $2 -> $1"
    APPLIED=1
}

apply tools/jpegxl-src tools/patches/jpegxl-src-avx512.patch

# keep the tree dirty-state visible but harmless; cargo reads it as-is
exit 0
