FROM rust:1-alpine AS builder
WORKDIR /usr/src/myapp
COPY . .
# libheif + codec libraries for HEIC/HEIF/AVIF input decoding (dec-heif feature)
# - gcc/g++/make/cmake are required by the vendored libjxl build (jxl feature)
# - libde265/dav1d: decode plugins for the statically embedded libheif
#   (dec-heif; HEIC via libde265, AVIF via dav1d — x265/aom excluded, see
#   tools/libheif-static/ and docs/plans/17-static-libheif-and-jxl-avx512.md)
RUN apk add --no-cache nasm musl-dev gcc g++ make cmake libde265-dev dav1d-dev
# The dec-heif feature statically embeds libheif 1.23.1 (libheif-sys
# `embedded-libheif`); the HEVC/AV1 decode plugins resolve from the static
# libde265 + dav1d above via pkg-config.
ENV RUSTFLAGS="-C target-feature=-crt-static"
# shipped unpacked: packed containers pay the decompression toll on every
# start while OCI layer compression already minimizes transfer size
RUN cargo install --profile release --path .

FROM alpine:latest
# libjxl static archives link libstdc++; apk resolves transitive deps on its own
RUN apk add --no-cache libstdc++
COPY --from=builder /usr/local/cargo/bin/byteshaver /usr/local/bin/byteshaver
WORKDIR /targets
CMD ["byteshaver"]
