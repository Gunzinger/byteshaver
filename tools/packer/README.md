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

| variant (byteshaver CLI, musl static-pie) | size | startup `--version` |
|---|---:|---:|
| plain | 16.23 MB | 3.4 ms |
| `upx --best` (NRV) | 33.4 % | 47.2 ms |
| **zpack, 2 frames** | **31.5 %** | **18.1 ms** |
| **zpack, 4 frames** | 33.3 % | **11.8 ms** |
| **zpack, 6 frames** | 33.5 % | **8.9 ms** |

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

- `--frames n` — 2 = best ratio, 4 = balanced (recommended), 6 = fastest.
  More frames lose cross-boundary matches (~+1 %pt per doubling) and gain
  decompression parallelism.
- `--page 65536` — use when the artifact must also run on 16K/64K-page ARM
  kernels (payload offset must be page-aligned for `mmap`).

## Debug knobs

- `ZPK_THREADS=n` — cap worker threads (1 = serial; used in benchmarks).
- `ZPK_DEBUG_SLEEP_MS=n` — sleep before `fexecve` (external `/proc` sampling).

## Known limitations

- Linux only (macOS lacks memfd/fexecve; Windows needs a PE stub — see the
  analysis doc's roadmap).
- `/proc` must be mounted (the stub locates itself via `/proc/self/exe`).
- `memfd_create` needs Linux ≥ 3.17; the `MFD_EXEC` flag needs ≥ 6.3
  (graceful fallback implemented).
- Payloads are plain zstd frames — recoverable via `dd` + `zstd -d`, and the
  AV-heuristic caveats of self-extracting executables apply.
