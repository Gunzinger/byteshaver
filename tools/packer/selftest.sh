#!/usr/bin/env bash
# Round-trip selftest for the zpack (ELF) and zpe v3 (PE) packers.
#
#   tools/packer/selftest.sh
#
# ELF legs need a host C compiler. PE legs additionally need a windows cross
# compiler (`. ./env-win.sh` after setup-wsl.sh, or install gcc-mingw-w64)
# and, for automated runs, wine. If wine is missing, the packed PE artifact
# is left in ./selftest-out/ — run it directly from WSL (Windows interop:
# executes on the real Windows host) or copy it to a Windows box.
set -euo pipefail
cd "$(dirname "$0")/../.."

WORK="$(mktemp -d)"
OUT="$PWD/selftest-out"
trap 'rm -rf "$WORK"' EXIT
PASS=0; SKIP=0; FAIL=0
ok()   { echo "  PASS: $1"; PASS=$((PASS+1)); }
skip() { echo "  SKIP: $1"; SKIP=$((SKIP+1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL+1)); }

# --- payload source: prints argv, exits with atoi(argv[1]) when positive ---
cat > "$WORK/payload.c" <<'EOF'
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    for (int i = 1; i < argc; i++) printf("ARG:%s\n", argv[i]);
    printf("ARGS:%d\n", argc - 1);
    int code = (argc > 1) ? atoi(argv[1]) : 0;
    return code > 0 ? code : 0;
}
EOF

# corrupt byte inside frame 0 (offset from the packer trailer, so this works
# regardless of stub/payload sizes) -> stub must die with 127, not hang/crash
corrupt_payload_byte() { # <file>
    python3 - "$1" <<'EOF'
import struct, sys
p = sys.argv[1]
d = bytearray(open(p, "rb").read())
nframes, off = struct.unpack_from("<QQ", d, len(d) - 24)
assert d[len(d)-8:] in (b"ZPK2zstd", b"ZPK3pe64"), "bad trailer magic"
d[off + 16] ^= 0xFF
open(p, "wb").write(d)
EOF
}

echo "==> [1/4] zstd sources + host build"
if [ ! -f zstd-1.5.7/lib/zstd.h ]; then
    rm -rf zstd-1.5.7
    echo "  fetching zstd 1.5.7"
    curl -sL "https://github.com/facebook/zstd/releases/download/v1.5.7/zstd-1.5.7.tar.gz" | tar xz
fi
if [ ! -f zstd-1.5.7/lib/libzstd.a ]; then
    make -C zstd-1.5.7/lib libzstd.a -j"$(nproc)" \
        CFLAGS="-Os -ffunction-sections -fdata-sections -DDYNAMIC_BMI2=0" \
        ZSTD_LEGACY_SUPPORT=0 >/dev/null
fi

echo "==> [2/4] ELF: build stub + payload, pack, run"
if ! command -v gcc >/dev/null; then
    skip "no host gcc"
else
    gcc -Os -static -s -ffunction-sections -fdata-sections -Wl,--gc-sections \
        tools/packer/zpack_stub.c zstd-1.5.7/lib/libzstd.a \
        -Izstd-1.5.7/lib -lpthread -o "$WORK/zpack-stub"
    gcc -O2 -static -o "$WORK/payload" "$WORK/payload.c"
    python3 tools/packer/zpack.py "$WORK/zpack-stub" "$WORK/payload" "$WORK/packed" --frames 4

    out="$(cd /tmp && "$WORK/packed" alpha "two words" 2>&1)"
    want="$(printf 'ARG:alpha\nARG:two words\nARGS:2')"
    [ "$out" = "$want" ] && ok "args passthrough incl. spaces (run from other cwd)" \
                         || fail "args: got [$out]"
    rc=0; "$WORK/packed" 42 >/dev/null || rc=$?
    [ "$rc" -eq 42 ] && ok "exit code propagation (42)" || fail "exit code: got $rc"

    python3 tools/packer/zpack.py "$WORK/zpack-stub" "$WORK/payload" "$WORK/packed13" --frames 13
    rc=0; "$WORK/packed13" 7 >/dev/null || rc=$?
    [ "$rc" -eq 7 ] && ok "13-frame work-queue layout" || fail "13 frames: got $rc"

    cp "$WORK/packed" "$WORK/corrupt"
    corrupt_payload_byte "$WORK/corrupt"
    rc=0; "$WORK/corrupt" >/dev/null 2>&1 || rc=$?
    [ "$rc" -eq 127 ] && ok "corrupt payload detected, exit 127" \
                      || fail "corrupt payload: got rc=$rc (want 127)"
fi

echo "==> [3/4] PE (zpe v3): build stub + payload, pack"
PE_CC="" ; PE_AR=""
if [ -n "${CC_x86_64_pc_windows_gnullvm:-}" ] && command -v "$CC_x86_64_pc_windows_gnullvm" >/dev/null 2>&1; then
    PE_CC="$CC_x86_64_pc_windows_gnullvm"
    PE_AR="${AR_x86_64_pc_windows_gnullvm:-llvm-ar}"
elif command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    PE_CC=x86_64-w64-mingw32-gcc
    PE_AR=x86_64-w64-mingw32-ar
fi
mkdir -p "$OUT"
if [ -z "$PE_CC" ]; then
    skip "no windows cross compiler (run: . ./env-win.sh)"
else
    # second, windows-flavoured libzstd (clean first: never reuse host .o)
    rm -rf "$WORK/zstd-pe" && mkdir -p "$WORK/zstd-pe"
    cp -r zstd-1.5.7/lib/. "$WORK/zstd-pe/"
    make -C "$WORK/zstd-pe" clean >/dev/null 2>&1 || true
    make -C "$WORK/zstd-pe" libzstd.a -j"$(nproc)" CC="$PE_CC" AR="$PE_AR" \
        CFLAGS="-Os -ffunction-sections -fdata-sections -DDYNAMIC_BMI2=0" \
        ZSTD_LEGACY_SUPPORT=0 >/dev/null
    "$PE_CC" -O2 -o "$WORK/payload.exe" "$WORK/payload.c"
    "$PE_CC" -Os -static -s -ffunction-sections -fdata-sections -Wl,--gc-sections \
        tools/packer/zpe_stub.c "$WORK/zstd-pe/libzstd.a" \
        -I"$WORK/zstd-pe" -o "$WORK/zpe-stub.exe"
    python3 tools/packer/zpack.py "$WORK/zpe-stub.exe" "$WORK/payload.exe" \
        "$OUT/selftest-packed.exe" --pe
    ok "packed PE written to selftest-out/selftest-packed.exe"

    echo "==> [4/4] PE: run under wine"
    if ! command -v wine >/dev/null; then
        skip "no wine — run on real windows instead:"
        echo "    WSL interop (executes on the windows host): ./selftest-out/selftest-packed.exe alpha \"two words\""
        echo "    then: ./selftest-out/selftest-packed.exe 42 ; echo \$?   # expect 42"
    else
        export WINEDEBUG=-all
        out="$(wine "$OUT/selftest-packed.exe" alpha "two words" 2>/dev/null | tr -d '\r')"
        want="$(printf 'ARG:alpha\nARG:two words\nARGS:2')"
        [ "$out" = "$want" ] && ok "wine: args passthrough" || fail "wine args: got [$out]"
        rc=0; wine "$OUT/selftest-packed.exe" 42 >/dev/null 2>&1 || rc=$?
        [ "$rc" -eq 42 ] && ok "wine: exit code propagation" || fail "wine exit: got $rc"

        cp "$OUT/selftest-packed.exe" "$WORK/corrupt.exe"
        corrupt_payload_byte "$WORK/corrupt.exe"
        rc=0; timeout 15 wine "$WORK/corrupt.exe" >/dev/null 2>&1 || rc=$?
        # die() shows a MessageBox which can block headless wine -> 124 is
        # acceptable proof the stub's error path was reached
        { [ "$rc" -eq 127 ] || [ "$rc" -eq 124 ]; } \
            && ok "wine: corrupt payload hits stub error path (rc=$rc)" \
            || fail "wine corrupt: got rc=$rc"
    fi
fi

echo
echo "==> selftest: $PASS passed, $SKIP skipped, $FAIL failed"
[ "$FAIL" -eq 0 ]
