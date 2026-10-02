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

> The single-threaded packer described above has been superseded by the
> **multithreaded v2** (`tools/packer/` in the repo) — see §7. The v2 stub
> additionally requires `ZSTD_LEGACY_SUPPORT=0` and `-DDYNAMIC_BMI2=0`
> (92 KB stub instead of 194 KB).

---

# Part 2 — multithreaded decompression (v2, implemented) & roadmap

The v1 prototype left three levers on the table: decompression is
single-threaded, the stub was 194 KB, and RSS attribution was unmeasured.
All three are now closed, plus one negative result that settles the max-RSS
question empirically. **Everything below is implemented and measured on the
same idle machine / same 16,234,560 B musl static-pie reference binary.**

## 7. v2: split-frame multithreaded decompression

### 7.1 Design

- Packer splits the image into *n* contiguous chunks, compresses each as an
  **independent zstd frame** (`-19 -T1`); frame table (comp/uncomp lengths)
  goes into a v2 trailer (`"ZPK2zstd"`).
- Stub spawns one pthread per frame (capped to `nframes`, overridable via
  `ZPK_THREADS`), each decompresses its disjoint region of one big
  `memfd` mapping — no locks, no cross-frame dependencies — then `fexecve`.
- Sources: `tools/packer/zpack_stub.c` + `tools/packer/zpack.py` (vendored).

### 7.2 Frame-count sweep (idle machine, warm cache)

Final stub: atomic **work queue** (any frame count spreads over
`min(ncpu, nframes)` workers — frames beyond the worker count are no longer
run inline) + **per-frame `madvise(MADV_DONTNEED)`**: every worker releases
its input pages when its frame completes, so resident input is
`~workers × chunk`, not the whole payload.

| frames | total size | ratio | startup median | peak RSS |
|---:|---:|---:|---:|---:|
| 1 | 5,173,887 | 31.9 % | 31.37 ms | — |
| 2 | 5,117,970 | **31.5 %** | 18.05 ms | ~18 MB |
| 4 | 5,401,456 | 33.3 % | 11.73 ms | 19.7 MB |
| 6 | 5,441,917 | 33.5 % | **8.93 ms** | 19.0 MB |
| 12 | 5,630,523 | 34.7 % | 9.26 ms | **17.0 MB** |

Reference points on the same binary: plain 3.36 ms / 100 % / 2.5 MB RSS,
`upx --best` (NRV) 47.17 ms / 33.4 % / 17.3 MB, `upx --best --lzma`
247.7 ms / 28.2 % / 16.7 MB.

- **2 frames beats UPX NRV on both axes** (31.5 % vs 33.4 %, 18.1 vs 47.2 ms).
- 6 frames ≈ 8.9 ms — the memory-bandwidth knee (~3.2 GB/s aggregate decode).
- **12 frames ≈ RSS floor**: with only ~6 workers in flight, resident input
  drops to ~0.8 MB and peak RSS lands at 17.0 MB — within 0.3 MB of UPX NRV
  and 0.8 MB above the theoretical floor — *without* giving up the ~9 ms
  startup (the work queue makes the extra frames free).
- Max RSS is otherwise flat; thread stacks are lazy.

Two design notes recorded for posterity:
1. The first v2 dispatched frames beyond `workers-1` *inline* on the main
   thread — 12 frames/6 threads degenerated to 23.4 ms. The work-queue
   restructure fixed exactly that.
2. Worker-side `madvise` after the *pre-queue* layout already recovered
   ~1.4 MB (21.0 → 19.6 MB); combined with the queue (smaller in-flight
   chunks) it reaches 17.0 MB. Startup cost: unmeasurable (< 0.2 ms).

### 7.3 Core scaling (6 frames, `taskset`)

| CPUs available | startup median |
|---|---:|
| 8 | 11.9 ms (8.8 ms without the `taskset` exec layer) |
| 4 | 18.0 ms |
| 2 | 28.1 ms |
| 1 | 32.4 ms (≈ serial, as designed) |

Degrades gracefully: MT never hurts, even on single-core machines.

### 7.4 Robustness (all pass)

- Corrupted payload (bytes flipped at 2 offsets) → clean `zpk: a frame failed`,
  exit 127 — no crash, no partial exec.
- `--help | head` (SIGPIPE), env passthrough, foreign CWD, symlink invocation ✓
- 50 concurrent instances → 50× correct output (memfd/shmem isolation) ✓
- Outputs byte-identical to the plain binary ✓
- Debug hook `ZPK_DEBUG_SLEEP_MS` (sleep before `fexecve`) for external
  `/proc/<pid>/smaps_rollup` sampling.

### 7.5 Stub size: 194.6 KB → 91.8 KB

Size attribution found three layers of ballast in the naive build:

| change | stub size |
|---|---:|
| naive `-Os -static` against stock `libzstd.a` | 194,576 B |
| + `ZSTD_LEGACY_SUPPORT=0` (drops v0.6/v0.7 legacy decoders the archive pulls in) | 124,608 B |
| + stdio-free error reporting (`write(2)` instead of `perror`/`fprintf` → no `printf_core`) | 124,608 B (stdio was ≤2.7 KB here; kept for hygiene) |
| + `-DDYNAMIC_BMI2=0` (drops zstd's duplicated BMI2 decoder variants) | **91,840 B** |

`-Oz`, `--gc-sections`, `-ffunction-sections` were already applied everywhere.
Remaining: ~60 KB zstd decompressor + ~20 KB musl + pthread glue ≈ **0.17 %**
of the packed artifact — UPX's ~2 KB asm stub is smaller, but at this scale
the difference is no longer material.

### 7.6 Negative result: streaming input (stub v3)

Hypothesis: replacing the payload `mmap` with `pread` + streaming
`ZSTD_decompressStream` into a reused 256 KB buffer should cut ~5 MB of RSS.
**Measured: worse on both axes.**

| | mmap + `ZSTD_decompress` (v2) | pread + `ZSTD_decompressStream` (v3) |
|---|---:|---:|
| startup (6 frames) | 8.93 ms | 20.06 ms |
| max RSS | 21.0 MB | 28.3 MB |
| minor faults | 4,391 | 8,755 |

Cause: per-thread `DStream` workspaces + double-copy through kernel buffers
outweigh the removed file mapping, which is clean page-cache the kernel
reclaims on demand anyway. **mmap-based input is the right design** — the RSS
floor is the decompressed output itself (irreducible).

### 7.7 Updated verdict

| config | size | startup | vs UPX NRV |
|---|---:|---:|---|
| zpack 2 frames | **31.5 %** | 18.1 ms | smaller **and** 2.6× faster |
| zpack 4 frames (recommended default) | 33.3 % | 11.8 ms | ≈ size, 4× faster |
| zpack 6 frames | 33.5 % | 8.9 ms | +0.1 %pt size, 5.3× faster |

## 8. Roadmap — further startup, RSS, stub-size, portability

### 8.1 Startup latency (current floor: ~8.9 ms = 3.4 plain + ~5.5 unpack+mechanics)

| idea | expected | effort | verdict |
|---|---|---|---|
| `posix_fadvise(WILLNEED)` on payload before decompress | cold-cache only | trivial | do it (helps first-run-after-download) |
| Hugepage memfd (`MFD_HUGEPAGE`, 2 MiB-aligned) → ~2,000 fewer minor faults | ~0.5–1 ms | low | try; needs hugetlb pool → keep fallback |
| Decompress directly at final addresses (UPX-style section loader, no memfd + no second ELF load) | −3–4 ms | **high** (custom ELF loader in the stub) | only if the ~4 ms floor ever matters; big complexity jump |
| Parallel decode > 6 threads | 0 (bandwidth-bound) | — | measured: don't |
| Skip `--check` frames (`zstd --no-check`) | ~0.2 ms | trivial | marginal; loses integrity check |
| Higher-level: don't pack (the standing decision) | −5.5 ms | — | still the right default |

### 8.2 Max RSS — **done** (was "planned madvise")

Per-frame worker-side `madvise(MADV_DONTNEED)` + the work queue achieved it:

| config | peak RSS | notes |
|---|---:|---|
| UPX NRV | 17.3 MB | reference |
| zpack 6 frames | 19.0 MB | speed-optimal |
| **zpack 12 frames** | **17.0 MB** | **UPX parity, at 9.3 ms** |
| theoretical floor | 16.2 MB | the unpacked image itself |

The floor is the output image (irreducible by design — `fexecve`/PE load need
the full image). Streaming-input decomposition was measured and rejected
earlier (§7.6); per-frame input drop after the queue restructure is the
winning combination. Nothing further planned here.

### 8.3 Stub size (done: 194.6 → 91.8 KB; remaining ≈ 0.17 % of artifact)

| idea | expected | verdict |
|---|---|---|
| `-nostdlib` + raw syscalls + custom `_start` (drop musl remainder) | −15–20 KB | possible, poor ROI |
| zstd decoder subset (drop 4X2-Huffman or sequence variants) | −20–30 KB | format risk; upstream-unfriendly |
| UPX-style compressed stub (chained bootstrapping) | −80 KB | complexity not justified at 0.17 % |

Conclusion: **closed** — further shrinking is cosmetic.

### 8.4 Cross-platform

| platform | status | plan |
|---|---|---|
| Linux x86-64 (musl/glibc, static & static-pie, dynamic) | **works** (validated) | — |
| Linux arm64 | expected to work unchanged | pad stub to 64 KB (`--page 65536`) for 16K/64K-page kernels; add CI smoke |
| Linux dynamic-ELF payloads | expected to work (ld.so loads from disk as usual) | smoke-test once |
| FreeBSD | memfd + fexecve exist (13+) | port = recompile; low priority |
| **Windows PE32+** | **implemented (`tools/packer/zpe_stub.c`) and wine-tested — see §10** | real-Windows smoke test in CI remains |
| macOS | no `memfd`/`fexecve` | out of scope; a temp-file fallback weakens the model (on-disk artifact re-appears, AV scans it). Keep macOS unpacked. |

### 8.5 CI integration plan (concrete)

1. ~~Vendor `tools/packer/{zpack_stub.c,zpe_stub.c,zpack.py,README.md}`~~ **done**.
2. `pack_binaries` job: build the stubs once (alpine stage: musl + mingw-w64
   cross zstd, pin zstd 1.5.7, cache), then replace
   `upx --best` with `zpack.py --frames 6` for Linux **and Windows** artifacts.
3. Rename artifact suffix `-upx` → `-packed` (format-agnostic).
4. CI checks for packed artifacts: `--version` smoke (exists) — Windows smoke
   can run the packed exe under wine on the runner — plus a
   packed-vs-plain output diff on one sample image.
5. Track a release metric: packed size + startup latency of the packed
   artifact on the runner, one log line per release (regression tripwire).

---

## 10. Windows: in-memory PE loader (`zpe_stub.c`) — implemented & wine-tested

Same container format, new stub for PE32+ x64 payloads:

1. locate self via `GetModuleFileNameA` (no `/proc` on Windows),
2. parallel decompress — `CreateThread` work queue (same atomic-claim pattern
   as Linux) into one `VirtualAlloc` buffer,
3. in-memory load of the payload PE:
   headers + sections → **base relocation** (`.reloc`, DIR64/HIGHLOW) →
   **imports** (`LoadLibraryA` + `GetProcAddress`, name & ordinal) →
   **`RtlAddFunctionTable`** on `.pdata` (x64 unwind info) → per-section
   `VirtualProtect` → start `AddressOfEntryPoint` on a fresh thread with the
   header's stack reserve; wait; propagate the exit code via `ExitProcess`.

### 10.1 Wine validation (wine 10.0, mingw-w64 GCC 15 toolchain)

| payload | packed size | result under wine |
|---|---:|---|
| mingw C `hello.exe` (59 KB) | 109 KB | args/env passed, exit code 42 propagated ✓ |
| Rust windows-gnu `rshello.exe` (857 KB) | 450 KB (52.5 %) | identical output, exit 0 ✓ |
| corrupted payload | — | clean `zpe: a frame failed`, exit 127 ✓ |
| **real `byteshaver.exe` (16.96 MB, full features)** | **5.57 MB (32.8 %)** | real conversion: 2 JPEGs → webp, outputs **byte-identical** to the unpacked exe ✓ |

(The C-payload ratio is meaningless at 59 KB — the 94 KB stub dominates.
For real artifacts the Linux ratios apply.)

### 10.2 Windows-specific caveats (honest scope)

- The payload is *not* registered in the loader's module list: its
  `GetModuleFileName` sees the stub's path; anything enumerating its own
  module (some updaters) breaks.
- TLS callbacks run in stub context — not chained to the payload (mingw
  emutls unaffected in tested payloads; flagged for exotic cases).
- C++ exceptions *unwind* via the registered `.pdata`; `catch`-less SEH
  filters or needs-CLI payloads are untested.
- In-memory PE loading is a known AV heuristic trigger — same class of risk
  as UPX, but less "known-good"; code-signing the packed output is required
  for enterprise distribution.
- Real-Windows validation (11/10 Defender, ARM64-on-x64 emulation) still
  pending — CI follow-up.

**Verdict:** Windows packed variants are technically solved and validated
under wine; whether to ship them stays a product decision (the standing
"unpacked by default" logic applies identically).

## 9. Reproducing (v2)

```sh
# stub — see tools/packer/README.md (legacy-free, bmi2-free libzstd build)
python3 tools/packer/zpack.py zpack-stub byteshaver byteshaver-zpk --frames 4

# startup + scaling
hyperfine -N --warmup 20 --min-runs 200 './byteshaver-zpk --version'
ZPK_THREADS=1 hyperfine -N --warmup 20 --min-runs 200 './byteshaver-zpk --version'  # serial control
taskset -c 0-3 hyperfine -N --warmup 10 --min-runs 100 './byteshaver-zpk --version' # core scaling

# memory
/usr/bin/time -v ./byteshaver-zpk --version
ZPK_DEBUG_SLEEP_MS=2000 ./byteshaver-zpk --version &   # then sample /proc/<pid>/smaps_rollup
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
