#!/usr/bin/env bash
# Build the packer stubs into ./packer-out/ for packing real release binaries.
#
#   . ./env-win.sh              # or have gcc-mingw-w64 installed
#   tools/packer/build-stubs.sh
#   python3 tools/packer/zpack.py packer-out/zpe-stub.exe \
#       target/x86_64-pc-windows-gnullvm/release/byteshaver.exe \
#       byteshaver-packed.exe --pe
#
# Windows flavour needs a windows cross compiler (llvm-mingw or mingw-gcc);
# the ELF stub needs a host gcc.
set -euo pipefail
cd "$(dirname "$0")/../.."
OUT="$PWD/packer-out"
mkdir -p "$OUT"

echo "==> zstd sources"
if [ ! -f zstd-1.5.7/lib/zstd.h ]; then
    rm -rf zstd-1.5.7
    echo "  fetching zstd 1.5.7"
    curl -sL "https://github.com/facebook/zstd/releases/download/v1.5.7/zstd-1.5.7.tar.gz" | tar xz
fi

echo "==> ELF stub (linux)"
if command -v gcc >/dev/null; then
    [ -f zstd-1.5.7/lib/libzstd.a ] || \
        make -C zstd-1.5.7/lib libzstd.a -j"$(nproc)" \
            CFLAGS="-Os -ffunction-sections -fdata-sections -DDYNAMIC_BMI2=0" \
            ZSTD_LEGACY_SUPPORT=0 >/dev/null
    gcc -Os -static -s -ffunction-sections -fdata-sections -Wl,--gc-sections \
        tools/packer/zpack_stub.c zstd-1.5.7/lib/libzstd.a \
        -Izstd-1.5.7/lib -lpthread -o "$OUT/zpack-stub"
    echo "  $OUT/zpack-stub ($(stat -c%s "$OUT/zpack-stub") B)"
else
    echo "  SKIP: no host gcc"
fi

echo "==> PE stub (windows, zpe v3)"
PE_CC="" ; PE_AR=""
if [ -n "${CC_x86_64_pc_windows_gnullvm:-}" ] && command -v "$CC_x86_64_pc_windows_gnullvm" >/dev/null 2>&1; then
    PE_CC="$CC_x86_64_pc_windows_gnullvm"
    PE_AR="${AR_x86_64_pc_windows_gnullvm:-llvm-ar}"
elif command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    PE_CC=x86_64-w64-mingw32-gcc
    PE_AR=x86_64-w64-mingw32-ar
fi
if [ -z "$PE_CC" ]; then
    echo "  SKIP: no windows cross compiler (run: . ./env-win.sh)"
else
    # windows-flavoured libzstd in a private copy (clean first: never reuse
    # objects from a host build)
    rm -rf "$OUT/zstd-pe" && mkdir -p "$OUT/zstd-pe"
    cp -r zstd-1.5.7/lib/. "$OUT/zstd-pe/"
    make -C "$OUT/zstd-pe" clean >/dev/null 2>&1 || true
    make -C "$OUT/zstd-pe" libzstd.a -j"$(nproc)" CC="$PE_CC" AR="$PE_AR" \
        CFLAGS="-Os -ffunction-sections -fdata-sections -DDYNAMIC_BMI2=0" \
        ZSTD_LEGACY_SUPPORT=0 >/dev/null
    "$PE_CC" -Os -static -s -ffunction-sections -fdata-sections -Wl,--gc-sections \
        tools/packer/zpe_stub.c "$OUT/zstd-pe/libzstd.a" \
        -I"$OUT/zstd-pe" -o "$OUT/zpe-stub.exe"
    echo "  $OUT/zpe-stub.exe ($(stat -c%s "$OUT/zpe-stub.exe") B)"
fi
