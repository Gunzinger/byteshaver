# zpack — self-extracting zstd packer (Linux)

A minimal UPX alternative that trades UPX's hand-written asm stubs for a tiny
C stub plus the zstd codec: **smaller artifacts than `upx --best` and ~4-5×
faster unpacking**. Design rationale and full measurements:
[`docs/zstd-packer-analysis.md`](../../docs/zstd-packer-analysis.md).

```
packed layout:  [stub ELF, padded to page size][zstd frame 0]...[frame n-1][trailer]
trailer:        n×{comp_len,uncomp_len} | nframes u64 | payload_off u64 | magic "ZPK2zstd"
```

The stub maps its own payload, decompresses the frames in parallel (one
pthread per frame) into a `memfd(MFD_EXEC)` and `fexecve()`s it — one `execve`
total, argv/env/exit-code preserved. Works with static and static-pie ELF
payloads (the byteshaver release artifacts).

| variant (byteshaver CLI, musl static-pie) | size | startup `--version` | peak RSS |
|---|---:|---:|---:|
| plain | 16.23 MB | 3.4 ms | 2.5 MB |
| `upx --best` (NRV) | 33.4 % | 47.2 ms | 17.3 MB |
| **zpack, 2 frames** | **31.5 %** | **18.1 ms** | ~18 MB |
| **zpack, 4 frames** | 33.3 % | 11.8 ms | 19.7 MB |
| **zpack, 6 frames** | 33.5 % | **8.9 ms** | 19.0 MB |
| **zpack, 12 frames (RSS mode)** | 34.7 % | 9.3 ms | **17.0 MB** |

The stub uses an atomic work queue (any frame count parallelizes over
`min(ncpu, nframes)` workers) and each worker `madvise(MADV_DONTNEED)`s its
input pages when done, so with `--frames` well above the worker count the
resident input stays at ~workers × chunk size and peak RSS approaches the
16.2 MB output floor.

## Build the stub

Requires an Alpine (musl) environment and zstd 1.5.x sources:

```sh
apk add gcc musl-dev make curl
curl -sL https://github.com/facebook/zstd/releases/download/v1.5.7/zstd-1.5.7.tar.gz | tar xz
make -C zstd-1.5.7/lib libzstd.a -j4 \
  CFLAGS="-Os -ffunction-sections -fdata-sections -DDYNAMIC_BMI2=0" \
  ZSTD_LEGACY_SUPPORT=0
gcc -Os -static -s -ffunction-sections -fdata-sections -Wl,--gc-sections \
  zpack_stub.c zstd-1.5.7/lib/libzstd.a -Izstd-1.5.7/lib -lpthread -o zpack-stub
ls -l zpack-stub        # ~92 KB static-pie
```

`ZSTD_LEGACY_SUPPORT=0` and `-DDYNAMIC_BMI2=0` are load-bearing: without them
the archive links legacy v0.6/v0.7 decoders and duplicated BMI2 code paths
(194 KB stub instead of 92 KB).

## Pack

```sh
python3 zpack.py zpack-stub byteshaver byteshaver-zpk --frames 4
./byteshaver-zpk --version
```

- `--frames n` — 2 = best ratio, 4 = balanced, 6 = fastest, 12 = lowest RSS
  (frames beyond the worker count pull from a work queue; each finished frame
  releases its input pages, so peak RSS approaches the unpacked image size).
  More frames lose cross-boundary matches (~+1 %pt per doubling).
- `--page 65536` — use when the artifact must also run on 16K/64K-page ARM
  kernels (payload offset must be page-aligned for `mmap`).

## Debug knobs

- `ZPK_THREADS=n` — cap worker threads (1 = serial; used in benchmarks).
- `ZPK_DEBUG_SLEEP_MS=n` — sleep before `fexecve` (external `/proc` sampling).

## Known limitations

- `/proc` must be mounted on Linux (the stub locates itself via `/proc/self/exe`).
- `memfd_create` needs Linux ≥ 3.17; the `MFD_EXEC` flag needs ≥ 6.3
  (graceful fallback implemented).
- Payloads are plain zstd frames — recoverable via `dd` + `zstd -d`, and the
  AV-heuristic caveats of self-extracting executables apply.

## Windows (zpe_stub.c) - extract-to-temp + CreateProcess

The Windows stub uses the extract-to-temp model: it decompresses the payload
to %TEMP%, then CreateProcess's it with the original command line and
environment. The payload runs as a normal PE loaded by the standard Windows
loader - TLS, SEH, COM, module-list registration, DEP/CFG all work because
the OS does the loading. The temp file is deleted after the process exits.

This is deliberately simpler than in-memory PE mapping (which requires
reimplementing TLS directory processing, SEH interplay, and module-list
registration - see plan 17 section B for why that approach was abandoned).
The trade-off is a visible temp file during execution.

### Build

```sh
# in an alpine container with the mingw-w64 cross toolchain
apk add mingw-w64-gcc make curl
curl -sL https://github.com/facebook/zstd/releases/download/v1.5.7/zstd-1.5.7.tar.gz | tar xz
make -C zstd-1.5.7/lib libzstd.a -j4 CC=x86_64-w64-mingw32-gcc AR=x86_64-w64-mingw32-ar \
  CFLAGS="-Os -ffunction-sections -fdata-sections -DDYNAMIC_BMI2=0" ZSTD_LEGACY_SUPPORT=0
x86_64-w64-mingw32-gcc -Os -static -s -ffunction-sections -fdata-sections \
  -Wl,--gc-sections tools/packer/zpe_stub.c zstd-1.5.7/lib/libzstd.a \
  -Izstd-1.5.7/lib -o zpe-stub.exe
python3 zpack.py zpe-stub.exe byteshaver.exe byteshaver-packed.exe --pe
```

### Diagnostic env

- ZPE_DEBUG=1 - write phase-by-phase status to %TEMP%\zpe-debug.log
