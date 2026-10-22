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

DIST_ARG="${1:?usage: build-decode-only.sh <DIST> [JOBS]}"
# create the target before resolving it: GNU realpath requires every path
# component except the last to exist, and callers pass not-yet-created
# nested paths on a cache miss (the CI glibc step passes
# heif-deps-glibc/dist with neither component existing)
mkdir -p "$DIST_ARG"
DIST="$(realpath "$DIST_ARG")"
JOBS="${2:-$(nproc)}"
WORK="${LIBHEIF_STATIC_WORKDIR:-$(mktemp -d)}"

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

# 1. libde265 (static, cmake)
DE265_STAMP="$DIST/.stamp-libde265"
DE265_STAMP_DATA="libde265-$LIBDE265_VERSION"
# stamp (not mere artifact presence) gates the rebuild: a version bump in a
# reused DIST re-fetches and recompiles instead of silently shipping the
# previously built version
if [ ! -f "$DIST/lib/libde265.a" ] || [ "$(cat "$DE265_STAMP" 2>/dev/null)" != "$DE265_STAMP_DATA" ]; then
    fetch "$LIBDE265_URL" "$WORK/libde265.tar.gz" "$LIBDE265_SHA256" LIBDE265
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
    echo "$DE265_STAMP_DATA" > "$DE265_STAMP"
fi

# 2. dav1d (static, meson)
DAV1D_STAMP="$DIST/.stamp-dav1d"
DAV1D_STAMP_DATA="dav1d-$DAV1D_VERSION"
if [ ! -f "$DIST/lib/libdav1d.a" ] || [ "$(cat "$DAV1D_STAMP" 2>/dev/null)" != "$DAV1D_STAMP_DATA" ]; then
    fetch "$DAV1D_URL" "$WORK/dav1d.tar.xz" "$DAV1D_SHA256" DAV1D
    rm -rf "$WORK/dav1d" && mkdir -p "$WORK/dav1d"
    tar -xJf "$WORK/dav1d.tar.xz" -C "$WORK/dav1d" --strip-components=1
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
    echo "$DAV1D_STAMP_DATA" > "$DAV1D_STAMP"
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
