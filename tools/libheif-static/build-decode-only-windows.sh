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

LIBDE265_VERSION="${LIBDE265_VERSION:-1.1.3}"
LIBDE265_URL="https://github.com/strukturag/libde265/releases/download/v${LIBDE265_VERSION}/libde265-${LIBDE265_VERSION}.tar.gz"
DAV1D_VERSION="${DAV1D_VERSION:-1.5.1}"
DAV1D_URL="https://code.videolan.org/videolan/dav1d/-/archive/${DAV1D_VERSION}/dav1d-${DAV1D_VERSION}.tar.gz"

mkdir -p "$DIST/lib" "$DIST/include" "$WORK"

fetch() { # <url> <dest>
    if [ ! -f "$2" ]; then
        echo "==> fetching $1"
        curl -sL --fail "$1" -o "$2"
    fi
}

cat > "$WORK/toolchain.cmake" <<EOF
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR AMD64)
set(CMAKE_C_COMPILER ${TRIPLE}-gcc)
set(CMAKE_CXX_COMPILER ${TRIPLE}-g++)
set(CMAKE_RC_COMPILER ${TRIPLE}-windres)
set(CMAKE_FIND_ROOT_PATH /usr/${TRIPLE} $DIST)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
EOF

cat > "$WORK/cross.ini" <<EOF
[binaries]
c = '$TRIPLE-gcc'
cpp = '$TRIPLE-g++'
ar = '$TRIPLE-ar'
strip = '$TRIPLE-strip'
pkg-config = 'pkg-config'

[host_machine]
system = 'windows'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'
EOF

# 1. libde265 (static, cmake, windows cross)
if [ ! -f "$DIST/lib/libde265.a" ]; then
    fetch "$LIBDE265_URL" "$WORK/libde265.tar.gz"
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
fi

# 2. dav1d (static, meson, windows cross)
if [ ! -f "$DIST/lib/libdav1d.a" ]; then
    fetch "$DAV1D_URL" "$WORK/dav1d.tar.gz"
    rm -rf "$WORK/dav1d" && mkdir -p "$WORK/dav1d"
    tar -xzf "$WORK/dav1d.tar.gz" -C "$WORK/dav1d" --strip-components=1
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
