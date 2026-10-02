# DIY zstd-based executable packer — feasibility & analysis

Question: **can we replace UPX with a self-built packer that uses zstd, to get
much faster unpacking?** Short answer: **yes — built, measured, and it beats
UPX NRV on both size and unpack speed.** Whether we *should* ship it is a
separate question (§5) — it does not change the "ship unpacked by default"
decision, but it is a strictly better optional variant than `upx --best`.

- **Branch:** `ci/unpacked-releases-plus-upx-job` · **Date:** 2026-10-02
- **Subject binary:** the real CI artifact — `byteshaver` CLI, musl
  static-pie, 16,234,560 B (built like the release pipeline)
- **Machine:** i7-9700, idle, Linux 7.0 · zstd 1.5.7, UPX 5.0.2

---

## 1. Background: why this is plausible

UPX embeds hand-written asm decompression stubs for its own codecs. Their
measured effective decompression throughput on this binary (16.23 MiB payload,
from end-to-end startup deltas in `docs/upx-binary-size-report.md`):

| codec | effective throughput | unpack time |
|---|---:|---:|
| NRV2B/E (`upx --best`) | ~465 MB/s | ~44 ms |
| LZMA (`upx --best --lzma`) | ~66 MB/s | ~244 ms |

zstd decompression runs at **~0.9–1.1 GB/s** single-threaded regardless of
compression level (level 19 and level 3 differ by <20 % in decode speed while
differing by ~22 % in size). The conclusion writes itself: a packer built on
zstd should dominate `upx --best` on speed *and* size simultaneously.

## 2. Design of the DIY packer ("zpack")

Two tiny components (proof-of-concept, ~170 LOC total):

1. **Stub** — C, compiled static-pie against musl with libzstd's decompress
   objects only (`-Os -static`, 186,368 B):

   ```
   open("/proc/self/exe") → read trailer (last 40 bytes:
     magic "ZPK1zstd" | payload_offset | payload_len | unpacked_len)
   → mmap payload (page-aligned offset, zero-copy, page-cache backed)
   → memfd_create(MFD_EXEC) + ftruncate(unpacked_len) + mmap MAP_SHARED
   → ZSTD_decompress(dst ← src)
   → fexecve(memfd, argv, environ)      # replaces the process image,
                                        # no second process is spawned
   ```

2. **Packer** — Python/CI script: `stub (padded to 4 KiB) ‖ zstd -19 frame ‖
   trailer`. Compression is a one-time CI cost (`zstd -T0 -19`, ~15 s).

Key properties:

- **One `execve` total** — same as UPX: the kernel replaces the stub image
  with the decompressed ELF via `fexecve`; argv/env/exit-code pass through.
- Payload is a **plain zstd frame** — recoverable with `dd` + `zstd -d`
  (no proprietary container format, unlike UPX's `upx -d`).
- Linux-only by design (Windows needs a PE loader/relocation stub — see §6).
- Full source: appendix A.

## 3. Codec matrix on the real binary

Compressed sizes for the 16,234,560 B musl static-pie binary:

| codec | size | ratio | decompress | notes |
|---|---:|---:|---:|---|
| gzip -9 | 6,772,799 | 41.7 % | ~260 MB/s | |
| zstd -3 | 6,356,770 | 39.2 % | 1,057 MB/s | |
| zstd -9 | 5,752,964 | 35.4 % | ~950 MB/s | |
| zstd -14 | 5,696,813 | 35.1 % | ~950 MB/s | |
| **zstd -19** | **4,977,243** | **30.7 %** | **863 MB/s (18.8 ms)** | sweet spot |
| zstd -22 `--ultra` | 4,975,744 | 30.6 % | ≈ -19 | no gain |
| zstd -22 `--long=27` | 4,975,356 | 30.6 % | ≈ -19 | no gain on ELF |
| brotli q11 | 4,880,000 | 30.0 % | 284 MB/s (57 ms) | 36 s encode |
| xz -9e | 4,584,844 | 28.2 % | 105 MB/s (155 ms) | LZMA-class |
| *UPX NRV (reference)* | *5,429,464* | *33.4 %* | *~465 MB/s* | |
| *UPX LZMA (reference)* | *4,575,036* | *28.2 %* | *~66 MB/s* | |

Observations:

- zstd -19 is within 8 % payload size of xz/brotli while decompressing
  **2.3–8× faster** than them and ~1.9× faster than UPX NRV.
- `--long` and `-22` buy nothing on an ELF (relocations are scattered
  4-byte deltas; the 8 MB default window already covers all useful matches).
- `windowLog=21` (2 MB window): 33.5 % payload — costs 0.6 % size for zero
  measurable decompress/memory benefit here (window is barely touched during
  real decompression). Not worth it.

## 4. End-to-end results (working prototype)

Startup = `--version` median, hyperfine `-N`, 200–400 runs, warm cache.
Memory = max RSS of the same run (`/usr/bin/time -v`).

| variant | total size | ratio | startup | Δ vs plain | max RSS | minor faults |
|---|---:|---:|---:|---:|---:|---:|
| plain (musl static-pie) | 16,234,560 | 100 % | **3.36 ms** | — | 2.5 MB | 274 |
| UPX `--best` (NRV) | 5,429,464 | 33.4 % | 47.17 ms | +43.8 ms | 17.3 MB | 4,234 |
| UPX `--best --lzma` | 4,575,036 | 28.2 % | 247.69 ms | +244.3 ms | 16.7 MB | 4,222 |
| zpack zstd -3 | 6,543,166 | 40.3 % | 25.97 ms | +22.6 ms | 22.2 MB | ~4,300 |
| **zpack zstd -19** | **5,165,687** | **31.8 %** | **30.86 ms** | **+27.5 ms** | 20.8 MB | 4,335 |
| zpack zstd -19 wl21 | 5,436,507 | 33.5 % | 31.68 ms | +28.3 ms | 21.1 MB | — |

Correctness: conversion outputs of the packed binary are **byte-identical**
to the plain binary; `--version`, `--help`, symlinked invocation all pass.

### 4.1 Where the 27.5 ms go

| component | time | measured how |
|---|---:|---|
| zstd -19 library decompress (16.23 MB) | 18.8 ms | best-of-50 in-process benchmark (863 MB/s) |
| stub mechanics: payload mmap (page-cache), memfd + output page faults (4,096 pages), second ELF load, extra libc init | ~8.7 ms | residual |

### 4.2 Comparison verdict vs UPX

| axis | zpack zstd-19 | winner |
|---|---|---|
| size | 31.8 % vs NRV 33.4 % / LZMA 28.2 % | beats NRV, loses to LZMA by 3.6 %pt |
| startup | 30.9 ms vs NRV 47.2 ms / LZMA 247.7 ms | **1.53× faster than NRV, 8× faster than LZMA** |
| startup memory | 20.8 MB vs NRV 17.3 MB | UPX (zstd window + double mapping costs ~3.5 MB) |
| stub overhead | 186 KB vs UPX ~2 KB | UPX (3.6 % of artifact vs 0.04 %) |
| unpackability | plain zstd frame, trivially extractable | tie (both reversible) |
| complexity | ~170 LOC owned code | UPX (battle-tested, multi-platform) |

### 4.3 Further optimization options (not built)

- **Split-frame multi-threaded decompress:** pack the binary as 4 independent
  zstd frames, decompress with 4 threads in the stub. Measured ratio cost of
  independent frames: 32.8 % vs 30.7 % (+1.0 %pt total). Projected decompress
  ~5–7 ms → **~19–20 ms total startup** (vs 30.9). Needs pthreads in the stub
  (musl static: fine) + per-frame trailer entries. The natural next step if
  the ~31 ms variant isn't fast enough.
- `MAP_POPULATE` / hugepage memfd (`MFD_HUGEPAGE`): page-fault cost is small
  (~3 ms of the 8.7); hugepages need a preallocated hugetlb pool — not
  portable. Skip.
- brotli q11 as payload if size were the only goal: 30.0 % at 57 ms —
  dominated by zstd -19 for this use case.

## 5. Should we ship it?

| Question | Answer |
|---|---|
| Does it change the "ship unpacked" decision? | **No.** Even the best packed variant is 9× slower to start (30.9 vs 3.36 ms) and AV risk remains. Unpacked stays the default. |
| Is it better than the current optional `upx --best` `-upx` variants? | **Yes, on Linux**: smaller *and* 1.5× faster to start, with a documented, trivially reversible format. |
| Windows? | Not covered by this design. `fexecve`/`memfd` don't exist there; a Windows variant needs a PE unpacking stub (WriteProcessMemory + relocation application) — that is exactly the hard part UPX does well. Keep `upx --best` for Windows, use zpack for Linux, or skip packed Windows variants entirely. |
| Maintenance cost | ~170 LOC + a pinned zstd version. Real but small; the stub is format-stable (ELF exec + zstd frame). |
| Risks | memfd+fexecve is a known malware technique → AV heuristics may flag it *more* than UPX's well-known signature; `/proc` must be mounted; memfd needs kernel ≥ 3.17 (2014), `MFD_EXEC` ≥ 6.3 with graceful fallback (implemented); debugging/coredumps show the stub (same as UPX). |
| CI integration | trivial: build stub once (alpine stage), then `zpack.py stub binary out` per artifact in the existing `pack_binaries` job, replacing the `upx` call. |

**Recommendation:** if packed variants are wanted for Linux artifacts,
zpack-zstd-19 strictly dominates the current `upx --best` choice
(smaller + faster + trivially unpackable). Keep UPX only for the LZMA size
champion (28.2 %) if a further 3.6 %pt size reduction is ever worth 8× slower
startup — for a release artifact downloaded once, zpack's 31.8 % at 31 ms is
the better trade.

## 6. Reproducing

```bash
# stub (alpine: musl + libzstd source build, decompress objects only)
apk add gcc musl-dev zstd-dev make curl
curl -sL https://github.com/facebook/zstd/releases/download/v1.5.7/zstd-1.5.7.tar.gz | tar xz
make -C zstd-1.5.7/lib libzstd.a CFLAGS="-Os -ffunction-sections -fdata-sections"
gcc -Os -static -s -ffunction-sections -fdata-sections -Wl,--gc-sections \
  zpack_stub.c zstd-1.5.7/lib/libzstd.a -Izstd-1.5.7/lib -o zpack-stub-slim

# pack + run
python3 zpack.py zpack-stub-slim byteshaver byteshaver-zpk --level 19
./byteshaver-zpk --version

# benchmark
hyperfine -N --warmup 20 --min-runs 200 './byteshaver-zpk --version'
/usr/bin/time -v ./byteshaver-zpk --version    # Max RSS
```

---

## Appendix A: stub source (zpack_stub.c)

```c
/* zpack stub — self-extracting zstd packer for Linux ELF binaries.
 * layout: [stub ELF (padded to 4 KiB)][zstd payload][trailer]
 * trailer (40 B, LE): magic[8] | payload_off u64 | payload_len u64 | unpacked_len u64 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/mman.h>
#include <zstd.h>

static const char MAGIC[8] = {'Z','P','K','1','z','s','t','d'};
#ifndef MFD_EXEC
#define MFD_EXEC 0x0010U
#endif

static int memfd_exec(void) {
    int fd = memfd_create("zpk", MFD_CLOEXEC | MFD_EXEC);
    if (fd < 0 && errno == EINVAL)
        fd = memfd_create("zpk", MFD_CLOEXEC);   /* kernels < 6.3 */
    return fd;
}

int main(int argc, char **argv, char **envp) {
    int fd = open("/proc/self/exe", O_RDONLY | O_CLOEXEC);
    if (fd < 0) { perror("zpk: open /proc/self/exe"); return 127; }
    off_t fsz = lseek(fd, 0, SEEK_END);
    if (fsz < (off_t)sizeof(MAGIC) + 24) { fprintf(stderr, "zpk: file too small\n"); return 127; }
    uint64_t tr[4];
    if (pread(fd, tr, sizeof tr, fsz - sizeof tr) != sizeof tr ||
        memcmp(tr, MAGIC, 8) != 0) { fprintf(stderr, "zpk: bad trailer\n"); return 127; }
    uint64_t payload_off = tr[1], payload_len = tr[2], unpacked_len = tr[3];

    void *src = mmap(NULL, payload_len, PROT_READ, MAP_PRIVATE, fd, (off_t)payload_off);
    if (src == MAP_FAILED) { perror("zpk: mmap payload"); return 127; }
    int mfd = memfd_exec();
    if (mfd < 0) { perror("zpk: memfd_create"); return 127; }
    if (ftruncate(mfd, (off_t)unpacked_len) != 0) { perror("zpk: ftruncate"); return 127; }
    void *dst = mmap(NULL, unpacked_len, PROT_READ | PROT_WRITE, MAP_SHARED, mfd, 0);
    if (dst == MAP_FAILED) { perror("zpk: mmap memfd"); return 127; }

    size_t ret = ZSTD_decompress(dst, unpacked_len, src, payload_len);
    if (ZSTD_isError(ret) || ret != unpacked_len) {
        fprintf(stderr, "zpk: decompress failed: %s\n", ZSTD_getErrorName(ret));
        return 127;
    }
    fexecve(mfd, argv, envp);
    perror("zpk: fexecve");
    return 127;
}
```
