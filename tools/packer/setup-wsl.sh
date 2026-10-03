#!/usr/bin/env bash
# One-shot WSL/Ubuntu setup for the x86_64-pc-windows-gnullvm build.
# Mirrors the CI "Build windows binaries (cross)" job; idempotent — safe to
# re-run. Afterwards:  . ./env-win.sh && cargo build --target x86_64-pc-windows-gnullvm
set -euo pipefail
cd "$(dirname "$0")/../.." # repo root

echo "==> [1/5] system packages"
sudo apt-get update -qq
sudo apt-get install -y --no-install-recommends \
    build-essential cmake nasm ninja-build meson pkg-config patch curl

echo "==> [2/5] rust target"
rustup target add x86_64-pc-windows-gnullvm

echo "==> [3/5] llvm-mingw toolchain"
LLVM_MINGW_VERSION=20260922
TC="$HOME/.tools/llvm-mingw-$LLVM_MINGW_VERSION"
if [ ! -x "$TC/bin/x86_64-w64-mingw32-clang" ]; then
    mkdir -p "$HOME/.tools"
    curl -sL "https://github.com/mstorsjo/llvm-mingw/releases/download/$LLVM_MINGW_VERSION/llvm-mingw-$LLVM_MINGW_VERSION-ucrt-ubuntu-22.04-x86_64.tar.xz" |
        tar -xJ -C "$HOME/.tools"
    rm -rf "$TC"
    mv "$HOME/.tools/llvm-mingw-$LLVM_MINGW_VERSION-ucrt-ubuntu-22.04-x86_64" "$TC"
    # self-contained exes: drop the dynamic libc++/libunwind import libs so
    # the linker picks the static archives (no libc++.dll / libunwind.dll)
    rm "$TC/x86_64-w64-mingw32/lib/libc++.dll.a" "$TC/x86_64-w64-mingw32/lib/libunwind.dll.a"
fi
export PATH="$TC/bin:$PATH"

echo "==> [4/5] static codec deps (libde265 + dav1d, decode-only)"
DEPS_DIR="${HEIF_DEPS_DIR:-$PWD/heif-deps-win}"
if [ -f "$HOME/heif-deps-win/lib/libde265.a" ] && [ -f "$HOME/heif-deps-win/lib/libdav1d.a" ]; then
    DEPS_DIR="$HOME/heif-deps-win"
    echo "  reusing existing $DEPS_DIR"
fi
if [ ! -f "$DEPS_DIR/lib/libde265.a" ] || [ ! -f "$DEPS_DIR/lib/libdav1d.a" ]; then
    HEIF_CC=x86_64-w64-mingw32-clang HEIF_CXX=x86_64-w64-mingw32-clang++ \
        HEIF_AR=llvm-ar HEIF_SYSROOT="$TC/x86_64-w64-mingw32" \
        tools/libheif-static/build-decode-only-windows.sh "$DEPS_DIR"
else
    echo "  $DEPS_DIR up to date"
fi

echo "==> [5/5] writing env-win.sh"
cat > env-win.sh <<EOF
# source before building the windows target:  . ./env-win.sh
# mirrors the CI windows job env (workflow.yaml "Build windows binaries")
export PATH="$TC/bin:\$PATH"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER=x86_64-w64-mingw32-clang
export CC_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-clang
export CXX_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-clang++
export AR_x86_64_pc_windows_gnullvm=llvm-ar
export PKG_CONFIG_PATH="$DEPS_DIR/lib/pkgconfig"
export PKG_CONFIG_ALLOW_CROSS=1
export CXXFLAGS="-DLIBDE265_STATIC_BUILD -DLIBHEIF_STATIC_BUILD"
EOF

# stale libheif-sys caches bake in the old (codec-less) cmake configure
rm -rf target/x86_64-pc-windows-gnullvm/release/build/libheif-sys-*

echo
echo "==> done. build with:"
echo "  . ./env-win.sh"
echo "  cargo build --release --target x86_64-pc-windows-gnullvm -p byteshaver"
echo "  cargo build --release --target x86_64-pc-windows-gnullvm -p byteshaver-gui --no-default-features"
