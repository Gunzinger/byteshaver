#!/usr/bin/env bash
# Applies the repository's vendored-crate patches to their submodules.
# Idempotent: skips patches recorded as applied by a stamp file holding the
# patch's sha256 (written after a successful patch(1) run — grep markers in
# the patched sources used to be the idempotency signal and broke on edits).
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
    # stamp lives inside the submodule working tree: a fresh checkout (which
    # resets the gitlink) can never inherit a stale stamp
    local stamp="$dir/.byteshaver-applied-$(basename "$patch")"
    local sha
    sha="$(sha256sum "$patch" | cut -d' ' -f1)"
    if [ -f "$stamp" ] && [ "$(cat "$stamp" 2>/dev/null)" = "$sha" ]; then
        echo "already applied: $3"
        return
    fi
    local out rc
    out="$(patch -d "$dir" -p1 -l --forward < "$patch" 2>&1)" && rc=0 || rc=$?
    if [ "$rc" -ne 0 ] && ! echo "$out" | grep -q "previously applied"; then
        echo "$out" >&2
        echo "ERROR: failed to apply $3" >&2
        exit 1
    fi
    [ "$rc" -eq 0 ] && echo "$out"
    # rc != 0 with "previously applied": legacy state (applied before stamps
    # existed) — record the stamp now so the next run skips cleanly
    echo "$sha" > "$stamp"
    echo "applied: $3 -> $1"
}

apply tools/jpegxl-src jpegxl-src tools/patches/jpegxl-src-avx512.patch
