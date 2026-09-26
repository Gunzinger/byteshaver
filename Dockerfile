FROM rust:1-alpine AS builder
WORKDIR /usr/src/myapp
COPY . .
RUN apk add --no-cache nasm musl-dev upx
RUN cargo install --profile release --path .
RUN upx --best --ultra-brute /usr/local/cargo/bin/byteshaver

FROM alpine:latest
COPY --from=builder /usr/local/cargo/bin/byteshaver /usr/local/bin/byteshaver
WORKDIR /targets
CMD ["byteshaver"]
