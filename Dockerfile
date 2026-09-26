FROM rust:1-alpine AS builder
WORKDIR /usr/src/myapp
COPY . .
# libheif + codec libraries for HEIC/HEIF/AVIF input decoding (dec-heif feature)
# - gcc/g++/make/cmake are required by the vendored libjxl build (jxl feature)
# - alpine package for AV1 is `aom`/`aom-dev` (there is no `libaom` package)
# - validated against alpine latest (libheif 1.23.x)
RUN apk add --no-cache nasm musl-dev gcc g++ make cmake upx libheif-dev libde265-dev aom-dev
# The dec-heif feature links the system libheif/libde265/libaom shared libraries,
# which do not exist as static archives -> build this image with dynamic musl libc.
# (The downloadable static musl release binaries are built WITHOUT dec-heif, see CI.)
ENV RUSTFLAGS="-C target-feature=-crt-static"
RUN cargo install --profile release --path . --features dec-heif
RUN upx --best --ultra-brute /usr/local/cargo/bin/byteshaver

FROM alpine:latest
# runtime libraries of the HEIC/HEIF/AVIF decoders + C++ runtime (libjxl static
# archives link libstdc++; apk resolves transitive deps like libwebp on its own)
RUN apk add --no-cache libheif libde265 aom libstdc++
COPY --from=builder /usr/local/cargo/bin/byteshaver /usr/local/bin/byteshaver
WORKDIR /targets
CMD ["byteshaver"]
