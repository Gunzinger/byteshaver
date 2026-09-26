FROM rust:1-alpine AS builder
WORKDIR /usr/src/myapp
COPY . .
# libheif + codec libraries for HEIC/HEIF/AVIF input decoding (dec-heif feature)
# TODO(ci): verify package names
RUN apk add --no-cache nasm musl-dev upx libheif-dev libde265-dev libaom-dev
RUN cargo install --profile release --path . --features dec-heif
RUN upx --best --ultra-brute /usr/local/cargo/bin/byteshaver

FROM alpine:latest
# runtime libraries of the HEIC/HEIF/AVIF decoders
# TODO(ci): verify package names
RUN apk add --no-cache libheif libde265 libaom
COPY --from=builder /usr/local/cargo/bin/byteshaver /usr/local/bin/byteshaver
WORKDIR /targets
CMD ["byteshaver"]
