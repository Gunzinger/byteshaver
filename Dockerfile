# byteshaver runtime image.
#
# Ships the PREBUILT static-pie binary from the CI build stage instead of
# compiling in-container: the musl static binary self-contains everything
# (musl libc, libjxl with runtime-dispatched SIMD, and the dec-heif stack —
# libheif + libde265 + dav1d statically embedded), so the image is just the
# binary on a minimal alpine base. Starts instantly, no shared runtime libs.
#
# The binary is placed into the build context by the CI jobs
# (validate_docker / publish_docker download the x86-64-v3 build).
FROM alpine:latest
COPY byteshaver /usr/local/bin/byteshaver
RUN chmod +x /usr/local/bin/byteshaver
WORKDIR /targets
CMD ["byteshaver"]
