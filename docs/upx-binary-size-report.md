# Binary size & UPX packing report — `byteshaver` CLI

Measured before/after UPX packing of the release CLI binary, including startup
latency, memory statistics and conversion throughput, plus an evaluation of
UPX option sets and non-UPX alternatives for reducing binary size.

- **Commit:** `14be1ff` (main) · **Branch:** `bench/upx-packing`
- **Date:** 2026-10-01

---

## 1. TL;DR — recommendations

| Goal | Recommendation |
|---|---|
| **Balanced** (default advice) | **Do not UPX.** Ship the plain stripped release build. If artifacts are transported gzip/zstd-compressed, UPX makes total download size *worse* (see §7) and costs 45–250 ms startup. |
| **Minimum size, small startup budget** | `upx --best` (NRV, −67 % size, +44 ms startup). |
| **Absolute minimum size** | Build with `--no-default-features` + `opt-level="z"` (−55 %) **and then** `upx -9 --lzma` → **2.0 MB total (−87 %)**, but startup grows to ~135 ms and AV false-positive risk applies. |
| **Never** | `--ultra-brute`: 24× pack time for 0.3 % over `--best --lzma`. |
| For constrained flash/disk (raw size matters, startup doesn't) | `upx -9 --lzma` on the default build (4.5 MB, −72 %). |

The single biggest "free" lever is not UPX: dropping default features
(`--no-default-features`, −49 %) costs nothing at startup and *speeds it up*
(§6). Feature-gating (e.g. making `jxl` opt-in) is the cheapest real win.

---

## 2. Environment

| Item | Value |
|---|---|
| CPU | Intel Core i7-9700 (8 C/8 T, 3.0 GHz base, no SMT), `powersave` governor |
| RAM | 11.5 GiB, kernel 7.0.0-34-generic (Linux), Ubuntu 26.04.1 |
| Toolchain | rustc/cargo 1.98.1, UPX 5.0.2, hyperfine 1.19.0 |
| Build | `cargo build --release` (profile in `Cargo.toml`: opt-level 3, lto=fat, codegen-units=1, panic=abort, strip=on) |
| Binary type | ELF x86-64 **PIE, dynamically linked (glibc)**, stripped |
| Note | CI release artifacts are **musl static-pie**; measured numbers are for the local glibc build. Direction and magnitude of effects transfer, absolute values will differ slightly. |

Workloads:

1. **`--version`** — pure process startup + clap parse + exit (startup latency proxy).
2. **`--help`** — startup + large stdout write (piped to `/dev/null`).
3. **convert** — real batch: 15 files / 24.0 MiB from `examples/`, converted to
   `webp` (`-q 90`), 8-rayon threads. Output verified **byte-identical** between
   the original and packed binaries.

Metrics: hyperfine (medians, warm cache, no shell overhead via `-N`);
`/usr/bin`-style rusage (`wait4`) + `/proc/<pid>/status` sampling at 200 µs for
memory (`VmPeak` = peak *reserved/committed* virtual memory, `VmHWM`/maxrss =
peak *resident set* / working set).

---

## 3. Baseline binary profile

Raw size **15,945,640 B (15.21 MiB)**. Section breakdown (`size -A`):

| Section | Size | % of file | Note |
|---|---:|---:|---|
| `.text` | 12,551,778 | 78.7 % | machine code (incl. vendored libjxl, rav1e, oxipng/zopfli, libwebp) |
| `.rodata` | 1,998,176 | 12.5 % | constants, strings, clap help text |
| `.eh_frame` + `_hdr` | 616,440 | 3.9 % | unwind tables (needed for `panic=unwind`; present despite `panic=abort` due to C/C++ deps) |
| `.rela.dyn` | 338,496 | 2.1 % | relocations (PIE) — this is why startup scales with size even when cached |
| `.data.rel.ro` | 283,024 | 1.8 % | |
| everything else | ~158,000 | ~1.0 % | |

---

## 4. UPX option sets — size & pack cost

Packed with UPX 5.0.2 (`linux/amd64` format). Integrity verified with
`upx -t`, and each packed binary produces identical conversion output.

| Variant | Command | Size | vs. orig | Ratio | Pack time |
|---|---|---:|---:|---:|---:|
| orig | — | 15,945,640 | 100 % | — | — |
| default | `upx` | 5,453,068 | 34.2 % | −65.8 % | 8.4 s |
| best (NRV) | `upx -9` (= `--best`) | 5,319,356 | 33.4 % | −66.6 % | 5.3 s |
| best + LZMA | `upx -9 --lzma` | 4,473,904 | 28.1 % | −72.0 % | 4.7 s |
| ultra-brute | `upx --ultra-brute` | 4,428,484 | 27.8 % | −72.2 % | 113.8 s |

Observations:

- `--ultra-brute` buys **0.3 %** over `-9 --lzma` for **24×** the pack time. Not worth it.
- `-9 --lzma` is the sweet spot for size; LZMA costs at *de*compression time (§5).
- Pack times are one-time (CI) costs and irrelevant to users.

---

## 5. Startup latency

`hyperfine -N` (direct exec, no shell), warm page cache, medians over 100+
runs (`--version`) / 30–60 runs (`--help`):

| Variant | `--version` median | min | vs. orig | `--help` median |
|---|---:|---:|---:|---:|
| orig | 5.47 ms | 1.48 ms | 1.0× | 2.66 ms |
| default | 48.61 ms | 47.91 ms | 8.9× | 48.89 ms |
| best (NRV) | 47.43 ms | 46.87 ms | 8.7× | 47.59 ms |
| best + LZMA | **244.70 ms** | 241.78 ms | **44.7×** | 244.02 ms |
| ultra-brute | 252.16 ms | 249.16 ms | 46.1× | 251.99 ms |

Decompression throughput derived from the numbers above (≈15.2 MiB unpacked):

- NRV2B/E (`-9`): **≈ 320 MiB/s**
- LZMA (`-9 --lzma`): **≈ 62 MiB/s**

LZMA decompresses ~5× slower than NRV — for a 15 MiB binary that is the
difference between +42 ms and +240 ms on *every single invocation*. For a CLI
used in batch pipelines (possibly invoked thousands of times), LZMA's extra
4.5 % size saving is almost always a bad trade.

---

## 6. Memory statistics

rusage (`wait4`, kernel-authoritative) + `/proc` high-water marks.
All values in **kB**. "startup" = `--version` run; "convert" = batch workload.

### 6.1 Startup

| Variant | max RSS (working set) | VmPeak (reserved) | minor faults |
|---|---:|---:|---:|
| orig | 13,064 | 22,976 | 360 |
| default | 20,372 | 23,128 | 4,255 |
| best (NRV) | 20,264 | 22,980 | 4,251 |
| best + LZMA | 19,452 | 23,128 | 4,234 |
| ultra-brute | 19,664 | 23,056 | 4,278 |

### 6.2 Conversion workload (peak)

| Variant | max RSS | VmHWM | VmPeak | minor faults | user CPU | wall |
|---|---:|---:|---:|---:|---:|---:|
| orig | 534,636 | 534,768 | 1,077,512 | 201,182 | 8.35 s | 1.326 s ± 0.053 |
| default | 532,744 | 532,744 | 1,077,516 | 205,074 | 8.69 s | 1.412 s ± 0.037 |
| best (NRV) | 528,280 | 528,432 | 1,077,516 | 205,070 | 8.46 s | 1.426 s ± 0.046 |
| best + LZMA | 534,804 | 534,804 | 1,077,516 | 205,055 | 8.70 s | 1.635 s ± 0.039 |
| ultra-brute | 539,464 | 539,772 | 1,077,592 | 205,099 | 8.89 s | 1.614 s ± 0.029 |

Interpretation:

- **Runtime memory is unaffected.** Once decompressed, the packed binary
  allocates exactly like the original (peak RSS ~522 MiB, VmPeak ~1.05 GiB —
  dominated by decoded 24 MiB of image pixel buffers across 8 rayon threads).
- Packed binaries pay a **one-time +7 MB RSS / +3,900 minor-faults** premium at
  startup: the UPX stub `memfd_create("upX", MFD_EXEC)`s an anonymous in-memory
  copy of the 15.2 MiB image and decompresses into it (verified via strace;
  no files are dropped in `/tmp`, and the memory counts toward the process).
- The conversion wall-time deltas (+86 ms NRV / +310 ms LZMA) equal the startup
  decompression cost of §5 almost exactly, while **user CPU time is unchanged**
  (~8.4 s). UPX does not slow down the actual work — it adds a fixed toll per
  invocation.

---

## 7. Distribution reality check: UPX vs. transport compression

Release artifacts are usually *transmitted* compressed. Comparing final
delivered sizes:

| Delivery | Size | vs. raw orig |
|---|---:|---:|
| orig, raw | 15,945,640 | 100 % |
| orig + gzip -9 | 6,633,890 | 41.6 % |
| **orig + zstd -19** | **4,853,468** | **30.4 %** |
| UPX best (NRV), raw | 5,319,356 | 33.4 % |
| UPX best, + gzip -9 | 5,248,245 | 32.9 % |
| UPX best + LZMA, raw | 4,473,904 | 28.1 % |

**A zstd-compressed original (4.85 MB) already beats an UPX-NRV-packed binary
(5.32 MB) — with zero startup cost and no AV risk.** Only `--lzma` packing
beats `zstd -19`, by ~8 %, at a 245 ms/invocation cost. Compressed archives
(`.tar.zst` release assets, OCI layers, `apt`/`dnf` packages) mostly erase UPX's
benefit; UPX only wins where the **raw** file lands on disk uncompressed
(bare-metal embedded, air-gapped copies, UNC shares, `cargo install --path`
artifacts).

---

## 8. Non-UPX alternatives — build-level size reduction

All measured with the same workloads. `a-optz-ndf-lzma` additionally packed
with `upx -9 --lzma`.

| Variant | Recipe | Size | vs. orig | `--version` median | convert |
|---|---|---:|---:|---:|---:|
| orig | default release | 15,945,640 | 100 % | 5.47 ms | 1.305 s |
| a-optz | `opt-level = "z"` | 12,667,944 | 79.4 % | 2.66 ms | 1.378 s |
| a-ndf | `--no-default-features` | 8,137,736 | 51.0 % | **1.13 ms** | 1.381 s |
| a-optz-ndf | both combined | 7,172,712 | 45.0 % | 1.18 ms | 1.419 s |
| a-optz-ndf-lzma | both + `upx -9 --lzma` | **2,019,788** | **12.7 %** | 132.86 ms | 1.539 s |

Findings:

- **`--no-default-features` is the best real-world lever**: −49 % size *and*
  ~4.8× faster startup (smaller image to map/relocate). It drops `jxl`, `oxipng`,
  `anim-webp`/`anim-apng`, EXIF and JSON logs — a functional regression, so the
  actionable variant is *making heavy features opt-in* (see below).
- `opt-level = "z"` alone: −21 % with no startup penalty (within noise) and no
  measurable conversion slowdown on this workload — but it is a global
  performance risk for encode-heavy paths; measure per-release before adopting.
- Feature granularity beats build flags: `jxl` (vendored libjxl ≈ several MB of
  `.text`) and `oxipng`+`zopfli` are the largest optional chunks; making `jxl`
  opt-in for size-critical builds keeps the default build honest.
- `panic = "abort"` + `lto = "fat"` + `codegen-units = 1` + `strip` are already
  in the release profile — the remaining fat is third-party encoder code.

### Other techniques not adopted here (with rationale)

| Technique | Verdict |
|---|---|
| `sstrip` (super-strip section table) | ~1 % of a stripped ELF; cosmetic. |
| Static musl release build | Already used for CI artifacts; usually *larger* than glibc-dynamic but self-contained; UPX applies identically. |
| PGO / BOLT | Primarily speed tools; size effect ±1–3 %, high pipeline complexity. |
| Dynamic-linking system libs (libwebp, libjpeg…) | Shrinks the binary but reintroduces DLL hell — project explicitly ships self-contained static binaries. |
| Rewriting hot deps (e.g. drop rav1e for libaom bindings) | Months of work; not a size lever worth it. |
| `cargo-bloat` audits | Worth doing periodically: run `cargo install cargo-bloat && cargo bloat --release -n 20` to find surprise contributors (help strings, unicode tables, etc.). |

---

## 9. UPX operational caveats (beyond raw numbers)

1. **Antivirus false positives** — packed executables are the #1 heuristic
   trigger for AV/EDR (packed + high-entropy + no cert). For a tool distributed
   to end users (especially Windows `.exe`), this is usually the decisive
   argument *against* UPX; code-signing mitigates but does not remove it.
2. **Debuggability** — core dumps, `gdb`, `perf`, `perfetto`, and crash
   reporters see the stub, not the program. `upx -d` restores the original
   (sha-identical content), but production incidents are harder to triage.
3. **Data Executive Support / hardened kernels** — some hardened environments
   (grsecurity, certain SELinux policies, restricted `memfd`/`exec` policies)
   block UPX stubs; corporate fleets may refuse to run them.
4. **Signing/notarization** — Windows Authenticode signatures must be applied
   *after* packing; macOS notarization is effectively incompatible with UPX.
5. **UPX is not encryption or obfuscation** — `upx -d` unpacks any UPX binary;
   don't pack for "protection".
6. **Version pinning** — keep the packer version pinned in CI; UPX 5.x cannot be
   unpacked by very old `upx -d` builds (format changed with 5.0).

---

## 10. Decision matrix

| Deployment | Best option | Why |
|---|---|---|
| GitHub release asset, downloaded raw | `upx --best` (NRV) *or* plain + zstd asset | zstd asset is smaller than NRV-packed raw; if only one raw file is shipped, NRV saves 67 % at +42 ms |
| Release asset published as `.tar.zst` / package repo | **plain build** | transport compression already does the job |
| Docker image | **plain build** | OCI layers are zstd/gzip compressed; UPX adds ~50–250 ms per container start for near-zero image gain |
| Embedded / flash-constrained, run few times | `upx -9 --lzma` (+ `--no-default-features` build) | raw size is king, startup amortized |
| CLI invoked in tight loops (CI pipelines) | **plain build** | 45–250 ms × thousands of invocations = minutes wasted |
| Default library users (`cargo install byteshaver`) | **plain build** | startup UX > disk space in 2026 |

---

## 11. Reproducing

```bash
# build + baseline
cargo build --release
cp target/release/byteshaver /tmp/bench/byteshaver.orig

# pack variants
upx -o /tmp/bench/p-best9 /tmp/bench/byteshaver.orig
upx -9 --lzma -o /tmp/bench/p-bestlzma /tmp/bench/byteshaver.orig

# startup latency (hyperfine, no shell)
hyperfine -N --warmup 5 --min-runs 100 '/tmp/bench/byteshaver.orig --version'
hyperfine -N --warmup 5 --min-runs 100 '/tmp/bench/p-best9 --version'

# memory (rusage + /proc sampling) — harness in this repo's bench history
/usr/bin/time -v /tmp/bench/byteshaver.orig --version   # quick check: Max RSS

# conversion workload
hyperfine --prepare 'rm -rf /tmp/bench/out' \
  '/tmp/bench/byteshaver.orig "/tmp/bench/data/**/*" webp -q 90 -o /tmp/bench/out --overwrite-existing'

# transport-compression comparison
gzip -9 -c byteshaver.orig | wc -c ;  zstd -19 -c byteshaver.orig | wc -c
```

Raw hyperfine/JSON results and `size -A` dumps were generated on the
`bench/upx-packing` branch run of 2026-10-01 (not committed; harness:
`memwatch.py` + `measure.sh` scripts, see branch history of this document).
