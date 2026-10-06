# syntax=docker/dockerfile:1
#
# byteshaver — unified runtime image.
#
# DEFAULT — full source build (reproduces the entire release pipeline):
#   patched libjxl with AVX3/AVX3_ZEN4 highway kernels (runtime dispatch —
#   safe on AVX2 and non-AVX machines), statically embedded libheif 1.23.1
#   with libde265 (HEVC/HEIC) and dav1d (AV1/AVIF) decode plugins, all
#   linked into a static-pie musl binary.
#   Requires the repository cloned with submodules:
#     git clone --recurse-submodules https://github.com/Gunzinger/byteshaver
#   then:
#     docker build -t byteshaver .
#   (~15–30 min; needs network for the pinned codec tarballs and crates)
#
# SHORT-CIRCUIT — prebuilt binary (what the CI pipeline does):
#   place a CI-built static binary named `byteshaver` next to this
#   Dockerfile and skip the whole build stage:
#     docker build --build-arg SOURCE=prebuilt -t byteshaver .

ARG SOURCE=build

# --- source build -----------------------------------------------------------
FROM rust:1-alpine AS build
WORKDIR /src
COPY . .
RUN apk add --no-cache \
      bash build-base cmake meson ninja nasm pkgconf curl git python3 file patch \
 && tools/patches/apply.sh \
 && tools/libheif-static/build-decode-only.sh /usr/local \
 && export PKG_CONFIG_PATH=/usr/local/lib/pkgconfig \
 && cargo build --release --target x86_64-unknown-linux-musl -p byteshaver \
 && file target/x86_64-unknown-linux-musl/release/byteshaver | grep -q "static-pie" \
 && cp target/x86_64-unknown-linux-musl/release/byteshaver /byteshaver

# --- prebuilt short-circuit -------------------------------------------------
FROM scratch AS prebuilt
COPY byteshaver /

# --- binary selection -------------------------------------------------------
# COPY --from cannot expand variables under BuildKit ("variable expansion is
# not supported for --from"), but FROM consumes the global SOURCE arg — alias
# the selected stage here so the runtime stage can COPY from a fixed name.
FROM ${SOURCE} AS byteshaver-bin

# --- runtime -----------------------------------------------------------------
FROM alpine
COPY --from=byteshaver-bin /byteshaver /usr/local/bin/byteshaver
COPY THIRD-PARTY-NOTICES.md docs/dependency-licenses.md /usr/share/doc/byteshaver/
RUN chmod +x /usr/local/bin/byteshaver
WORKDIR /targets
CMD ["byteshaver"]
