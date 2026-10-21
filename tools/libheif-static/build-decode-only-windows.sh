#!/usr/bin/env bash
# Cross-build static libde265 + dav1d for Windows (x86_64) with the
# gcc-mingw-w64 toolchain (decode-only; same exclusions as the musl recipe).
#
# Run inside an alpine container with:
#   apk add bash build-base cmake meson ninja nasm pkgconf curl git python3
#           mingw-w64-gcc
# Usage: build-decode-only-windows.sh <DIST> [JOBS]
set -euo pipefail

DIST="$(realpath "${1:?usage: build-decode-only-windows.sh <DIST> [JOBS]}")"
JOBS="${2:-$(nproc)}"
WORK="${LIBHEIF_STATIC_WORKDIR:-$(mktemp -d)}"
TRIPLE="x86_64-w64-mingw32"
# compiler overrides: point these at an llvm-mingw toolchain for the
# gnullvm/ucrt target (defaults = gcc-mingw-w64 / MSVCRT)
CC="${HEIF_CC:-${TRIPLE}-gcc}"
CXX="${HEIF_CXX:-${TRIPLE}-g++}"
AR="${HEIF_AR:-${TRIPLE}-ar}"
RC="${HEIF_RC:-${TRIPLE}-windres}"
SYSROOT="${HEIF_SYSROOT:-/usr/${TRIPLE}}"

LIBDE265_VERSION="${LIBDE265_VERSION:-1.1.3}"
LIBDE265_URL="https://github.com/strukturag/libde265/releases/download/v${LIBDE265_VERSION}/libde265-${LIBDE265_VERSION}.tar.gz"
# SHA-256 defaults match the pinned versions above; when overriding a
# *_VERSION, also override the matching *_SHA256 (or set it empty to
# explicitly download unverified — loud warning either way on mismatch/drift)
: "${LIBDE265_SHA256:=554228bd17788c99a7e63b37ab5634722190e6e2bf60c1dcb01cef328e133905}"
DAV1D_VERSION="${DAV1D_VERSION:-1.5.1}"
# official release archive (download.videolan.org) instead of the gitlab
# auto-generated archive endpoint, whose bytes can change on regeneration
DAV1D_URL="https://download.videolan.org/pub/videolan/dav1d/${DAV1D_VERSION}/dav1d-${DAV1D_VERSION}.tar.xz"
: "${DAV1D_SHA256:=401813f1f89fa8fd4295805aa5284d9aed9bc7fc1fdbe554af4292f64cbabe21}"

mkdir -p "$DIST/lib" "$DIST/include" "$WORK"

fetch() { # <url> <dest> <sha256> <NAME>
    if [ ! -f "$2" ]; then
        echo "==> fetching $1"
        curl -sL --fail "$1" -o "$2"
    fi
    if [ -n "$3" ]; then
        echo "==> verifying $4 checksum"
        echo "$3  $2" | sha256sum -c - || {
            echo "ERROR: $4 checksum mismatch for $2 (stale download or changed upstream — delete the file and retry, or update ${4}_SHA256)" >&2
            exit 1
        }
    else
        echo "WARNING: $4 downloaded WITHOUT checksum verification (${4}_SHA256 is empty)" >&2
    fi
}

cat > "$WORK/toolchain.cmake" <<EOF
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR AMD64)
set(CMAKE_C_COMPILER $CC)
set(CMAKE_CXX_COMPILER $CXX)
set(CMAKE_AR $AR)
set(CMAKE_RC_COMPILER $RC)
set(CMAKE_FIND_ROOT_PATH $SYSROOT $DIST)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
EOF

cat > "$WORK/cross.ini" <<EOF
[binaries]
c = '$CC'
cpp = '$CXX'
ar = '$AR'
strip = '$TRIPLE-strip'
pkg-config = 'pkg-config'

[host_machine]
system = 'windows'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'
EOF

# 1. libde265 (static, cmake, windows cross)
DE265_STAMP="$DIST/.stamp-libde265"
DE265_STAMP_DATA="libde265-$LIBDE265_VERSION cc=$CC cxx=$CXX ar=$AR sysroot=$SYSROOT"
# stamp (not mere artifact presence) gates the rebuild: a version or
# toolchain bump in a reused DIST re-fetches and recompiles instead of
# silently shipping the previously built objects
if [ ! -f "$DIST/lib/libde265.a" ] || [ "$(cat "$DE265_STAMP" 2>/dev/null)" != "$DE265_STAMP_DATA" ]; then
    fetch "$LIBDE265_URL" "$WORK/libde265.tar.gz" "$LIBDE265_SHA256" LIBDE265
    rm -rf "$WORK/libde265" && mkdir -p "$WORK/libde265"
    tar -xzf "$WORK/libde265.tar.gz" -C "$WORK/libde265" --strip-components=1
    cmake -S "$WORK/libde265" -B "$WORK/libde265-build" -G Ninja \
        -DCMAKE_TOOLCHAIN_FILE="$WORK/toolchain.cmake" \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX="$DIST" \
        -DCMAKE_PREFIX_PATH="$DIST" \
        -DBUILD_SHARED_LIBS=OFF \
        -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
        -DENABLE_SDL=OFF \
        -DENABLE_DECODER=OFF \
        -DENABLE_ENCODER=OFF
    cmake --build "$WORK/libde265-build" -j "$JOBS"
    cmake --install "$WORK/libde265-build" >/dev/null
    echo "$DE265_STAMP_DATA" > "$DE265_STAMP"
fi

# 2. dav1d (static, meson, windows cross)
DAV1D_STAMP="$DIST/.stamp-dav1d"
DAV1D_STAMP_DATA="dav1d-$DAV1D_VERSION cc=$CC cxx=$CXX ar=$AR sysroot=$SYSROOT"
if [ ! -f "$DIST/lib/libdav1d.a" ] || [ "$(cat "$DAV1D_STAMP" 2>/dev/null)" != "$DAV1D_STAMP_DATA" ]; then
    fetch "$DAV1D_URL" "$WORK/dav1d.tar.xz" "$DAV1D_SHA256" DAV1D
    rm -rf "$WORK/dav1d" && mkdir -p "$WORK/dav1d"
    tar -xJf "$WORK/dav1d.tar.xz" -C "$WORK/dav1d" --strip-components=1
    meson setup "$WORK/dav1d-build" "$WORK/dav1d" \
        --prefix="$DIST" --libdir=lib \
        --default-library=static --buildtype=release \
        -Denable_tools=false -Denable_tests=false -Denable_examples=false \
        --cross-file "$WORK/cross.ini" \
        --reconfigure 2>/dev/null || \
    meson setup "$WORK/dav1d-build" "$WORK/dav1d" \
        --prefix="$DIST" --libdir=lib \
        --default-library=static --buildtype=release \
        -Denable_tools=false -Denable_tests=false -Denable_examples=false \
        --cross-file "$WORK/cross.ini"
    ninja -C "$WORK/dav1d-build" -j "$JOBS"
    ninja -C "$WORK/dav1d-build" install >/dev/null
    echo "$DAV1D_STAMP_DATA" > "$DAV1D_STAMP"
fi

# 3. relocatable pkg-config prefixes + mingw pthread fix:
# -lpthread would resolve mingw's libpthread.dll.a (import lib) which then
# collides with rustc's own static -l:libpthread.a; pin the exact static
# archive instead.
for pc in "$DIST"/lib/pkgconfig/*.pc; do
    [ -e "$pc" ] || continue
    sed -i.bak 's|^prefix=.*|prefix=${pcfiledir}/../..|' "$pc"
    sed -i.bak -e 's|-l:libpthread.a||g' -e 's|-lpthread||g' "$pc"
    rm -f "$pc.bak"
done

echo "==> windows decode-only heif deps ready under $DIST"
ls -l "$DIST/lib"/*.a
