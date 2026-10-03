#!/usr/bin/env bash
# Decode-only static build of the libheif codec dependencies needed by the
# byteshaver `dec-heif` feature: libde265 (HEVC/HEIC) + dav1d (AV1/AVIF).
#
# Deliberately EXCLUDED (license/scope):
#   - x265 (GPL-2.0, HEVC encoder) — statically linking it into the MIT-licensed
#     byteshaver would GPL the distribution. libheif-sys's embedded libheif is
#     configured with all encoder plugins off in a controlled environment
#     (no x265/aom present on PKG_CONFIG_PATH), so nothing pulls them in.
#   - libaom (dav1d covers AV1 decode; smaller)
#   - libwebp/libsharpyuv (heif-webp encode only)
#
# Output: $DIST/lib/*.a + pkg-config files (relocatable prefix) + headers.
# On musl, the static C++ runtime (libstdc++.a, libgcc_eh.a) is bundled and
# referenced from libheif's pc file, so dependents only need musl.
#
# Usage: build-decode-only.sh <DIST> [JOBS]
set -euo pipefail

DIST="$(realpath "${1:?usage: build-decode-only.sh <DIST> [JOBS]}")"
JOBS="${2:-$(nproc)}"
WORK="${LIBHEIF_STATIC_WORKDIR:-$(mktemp -d)}"

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

# 1. libde265 (static, cmake)
if [ ! -f "$DIST/lib/libde265.a" ]; then
    fetch "$LIBDE265_URL" "$WORK/libde265.tar.gz"
    rm -rf "$WORK/libde265" && mkdir -p "$WORK/libde265"
    tar -xzf "$WORK/libde265.tar.gz" -C "$WORK/libde265" --strip-components=1
    cmake -S "$WORK/libde265" -B "$WORK/libde265-build" -G Ninja \
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

# 2. dav1d (static, meson)
if [ ! -f "$DIST/lib/libdav1d.a" ]; then
    fetch "$DAV1D_URL" "$WORK/dav1d.tar.gz"
    rm -rf "$WORK/dav1d" && mkdir -p "$WORK/dav1d"
    tar -xzf "$WORK/dav1d.tar.gz" -C "$WORK/dav1d" --strip-components=1
    meson setup "$WORK/dav1d-build" "$WORK/dav1d" \
        --prefix="$DIST" --libdir=lib \
        --default-library=static --buildtype=release \
        -Denable_tools=false -Denable_tests=false \
        -Denable_examples=false \
        --reconfigure 2>/dev/null || \
    meson setup "$WORK/dav1d-build" "$WORK/dav1d" \
        --prefix="$DIST" --libdir=lib \
        --default-library=static --buildtype=release \
        -Denable_tools=false -Denable_tests=false \
        -Denable_examples=false
    ninja -C "$WORK/dav1d-build" -j "$JOBS"
    ninja -C "$WORK/dav1d-build" install >/dev/null
fi

# 2b. libde265 1.1.x references GCC's cpu-detection runtime (__cpu_model,
#     __cpu_indicator_init_local) from libgcc.a — with -nodefaultlibs style
#     static links (rust musl self-contained) nothing else provides them.
#     Carry the archives in libde265.pc so every consumer (CLI, GUI,
#     dependencies thereof) resolves them right after -lde265.
if [ -f "$DIST/lib/pkgconfig/libde265.pc" ]; then
    if ! grep -q -- "-lgcc_eh" "$DIST/lib/pkgconfig/libde265.pc"; then
        sed -i.bak '/^Libs.private:/ s/$/ -lgcc -lgcc_eh/' "$DIST/lib/pkgconfig/libde265.pc"
        rm -f "$DIST/lib/pkgconfig/libde265.pc.bak"
    fi
fi

# 3. make pkg-config prefixes relocatable (bake-in-proof for CI staging)
for pc in "$DIST"/lib/pkgconfig/*.pc; do
    [ -e "$pc" ] || continue
    sed -i.bak 's|^prefix=.*|prefix=${pcfiledir}/../..|' "$pc"
    rm -f "$pc.bak"
done

# 4. musl: bundle the static C++ runtime and make sure libheif.pc pulls it
#    (the embedded libheif build in libheif-sys consumes this dist via
#    PKG_CONFIG_PATH; its Libs.private must carry the whole closure)
if ldd --version 2>&1 | grep -q musl; then
    for ARCHIVE in libstdc++.a libgcc_eh.a; do
        ARCHIVE_PATH="$(c++ -print-file-name="$ARCHIVE")"
        if [ "$ARCHIVE_PATH" = "$ARCHIVE" ]; then
            echo "ERROR: $ARCHIVE not found (install the static C++ runtime)" >&2
            exit 1
        fi
        cp "$ARCHIVE_PATH" "$DIST/lib/$ARCHIVE"
    done
fi

echo "==> decode-only heif deps ready under $DIST"
ls -l "$DIST/lib"/*.a
