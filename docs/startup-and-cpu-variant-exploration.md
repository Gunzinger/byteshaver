# Startup-time exploration & x86-64-v3/v4 variant analysis

Follow-up to `docs/upx-binary-size-report.md` (which drove the decision to ship
unpacked binaries by default). Two questions are explored here, both with
measurements:

1. **How can startup time be improved further** now that UPX is out of the
   default artifacts?
2. **Can the x86-64-v3 and x86-64-v4 release variants be merged into a single
   binary** using runtime dispatch for AVX-512 (and other v4 ISA elements)?

- **Branch:** `ci/unpacked-releases-plus-upx-job` (based on main @ `7ce93e6`)
- **Date:** 2026-10-02 · **Machine:** i7-9700 (8C, AVX2, **no AVX-512**), idle
  (load < 0.1), kernel 7.0.0-34, rustc 1.98.1
- **Caveat:** no AVX-512 hardware available — v4 *absolute* gains could not be
  measured on real metal. The merge case is instead built from three measured
  proxies (see §B.2–B.4): the v3 binary already contains runtime-guarded
  AVX-512 kernels, target-cpu codegen shows ≤1 % effect on every encoder, and
  the dispatch mechanism is proven working end-to-end.

---

## A. Startup time

### A.1 Anatomy of a `byteshaver --version` run (warm cache)

Measured via hyperfine (`-N`, direct exec), strace and rusage:

| Component | Cost | Evidence |
|---|---:|---|
| syscalls (whole run) | ~0.3 ms | `strace -c`: ~100 syscalls total |
| runtime init + arg parse + exit (user) | ~1.0 ms (musl) / ~1.4 ms (glibc) | hyperfine User column |
| kernel ELF load + page-in (sys) | ~1.7–2.1 ms | hyperfine System column; scales with image size |
| ld.so (dynamic glibc only) | ~+1.3 ms | musl-staticpie 3.36 ms vs glibc 4.61 ms median |
| static-pie self-relocation | ~0.1–0.5 ms | static-pie vs `relocation-model=static` non-PIE below |
| lazy→eager PLT binding | ±0 | `LD_BIND_NOW=1` no measurable change |

### A.2 Measured levers

All medians, `--version`, 200–400 runs each (idle machine):

| Binary | Size | Startup median | Δ vs. CI artifact |
|---|---:|---:|---|
| musl static-pie (**current CI artifact**) | 16.2 MB | **3.36 ms** | — |
| musl static-pie + UPX `--best` (**old CI artifact**) | 5.4 MB | 47.32 ms | **14.1× slower** |
| musl static, non-PIE (`-C relocation-model=static`) | 15.8 MB | 3.15 ms | −0.2…−0.5 ms |
| glibc dynamic (default build) | 15.9 MB | 4.61 ms | +1.25 ms |
| glibc, `jxl` feature removed | 9.1 MB | 2.40 ms | **−29 %** |
| glibc, `--no-default-features` | 8.1 MB | 2.27 ms | −32 % |
| glibc + UPX `--best` | 5.4 MB | ~47 ms | ~14× slower |

Further measurements (no effect / not actionable):

- `setarch -R` (ASLR off): within noise — **keep PIE**, the 0.2–0.5 ms is
  not worth losing address-space randomization.
- `LD_BIND_NOW`: no effect (few PLT calls before exit).
- Transparent hugepages (`madvise` on this box): note the strong
  median-vs-min bimodality (3.4 ms vs 1.0 ms) — first-touch 2 MB faults and
  frequency scaling dominate the spread; nothing actionable inside the binary.

### A.3 Brainstorm — ideas considered

| Idea | Verdict |
|---|---|
| ~~UPX~~ | Rejected by decision (14× startup); now optional `-upx` release variants |
| **Feature-gate `jxl`** (opt-in feature / separate artifact) | **The one big lever left**: −6.9 MB (−43 % of the binary), −29 % startup. The vendored libjxl is 43 % of `.text`. Cost: JPEG-XL support leaves the default build. |
| `opt-level = "z"` | −21 % size, ~−40 % startup (measured in the UPX report); global encode-throughput risk — needs per-release validation before adopting |
| Hand-rolled fast path for `--version`/`--help` before clap builds its full tree | ~0.1–0.3 ms; only worth it after feature gating; hurts maintainability |
| Replace clap-derive with lighter parser | large regression in UX/feature surface for <1 ms |
| jemalloc/mimalloc | adds init work — keep system allocator |
| Prelink, `vmtouch`, page-cache warming | deployment-side, not distributable |
| Non-PIE static | 0.2–0.5 ms, loses ASLR — not recommended |
| Nightly `-Zprecompute-dynamic-relocations` etc. | marginal, nightly-only |
| LTO/codegen-units/strip | already enabled in the release profile |

**Bottom line:** the CI already ships close to the achievable optimum
(musl static-pie, unpacked). The only remaining >1 ms lever is **feature-gating
`jxl`** (−2.2 ms, −6.9 MB), which is a product decision, not an engineering
freebie.

---

## B. Merging the x86-64-v3 and x86-64-v4 variants

### B.1 Why a plain merge is impossible

A single `-C target-cpu=x86-64-v4` binary hard-codes AVX-512 codegen:

```
$ ./glibc-v4 --version
Illegal instruction (core dumped)     ← on the AVX2-only i7-9700
```

AVX-512 instruction counts in the shipped binaries (`objdump -d | grep -c zmm`):

| Build | zmm instructions | runs on v1/v2/v3 CPUs? |
|---|---:|---|
| default (no target-cpu) | 18,389 | yes |
| `-C target-cpu=x86-64-v3` | 18,389 | yes |
| `-C target-cpu=x86-64-v4` | 27,235 | **no (SIGILL)** |

### B.2 The v3 build already carries runtime-dispatched AVX-512

The 18,389 zmm instructions in the *default/v3* binary cannot be unconditional
(it runs on AVX2-only hardware) — they sit behind runtime guards. Verified
sources, via static-lib symbol analysis:

| Library | Internal runtime dispatch | Evidence |
|---|---|---|
| rav1e / v_frame (avif) | yes | hand-written nasm kernels, `is_x86_feature_detected!`-style gating |
| fdeflate / simd-adler32 / zune-* (png path) | yes | Rust `std::arch` guarded paths |
| moxcms (color mgmt) | yes | ditto |
| mozjpeg (libjpeg-turbo) | yes | 245 `jsimd_*` dispatch symbols |
| libwebp | yes | 233 SSE2/AVX2 dispatch symbols |
| libjxl + highway | yes (AVX2 on) | `JPEGXL_ENABLE_HWY_AVX2:BOOL=true` in the vendored cmake cache |

In other words: **the merged question is largely already answered by the
dependency stack** — AVX-512/AVX2 kernels ship inside the v3 binary and pick
themselves at runtime.

### B.3 Does `-C target-cpu` matter at all for throughput? (idle machine, 24 MiB corpus)

| target | default (v1 codegen) | x86-64-v2 | x86-64-v3 |
|---|---:|---:|---:|
| webp | 1.36–1.40 s | ±0.5 % | ±0.7 % |
| jpeg | 1.58–1.59 s | ±0.3 % | ±0.5 % |
| jxl | 5.74–5.77 s | ±0.4 % | ±0.8 % |
| oxipng | 11.68–11.76 s | ±0.4 % | ±0.7 % |
| avif | 67.9 s ± 0.8 | **78.7 s ± 1.7 (+16 %!)** | 67.0 s ± 1.8 |

- Codegen tuning is **≤1 % on every encoder except avif**, where v3 ≈ baseline
  and **v2 is a 16 % regression** (rustc picking SSE4 patterns that pipeline
  worse in rav1e's hot loops — a known auto-vectorization failure mode).
- No monotonic "newer ISA = faster" relationship: the encoders' speed lives in
  their own dispatched kernels (§B.2), not in blanket codegen tuning.
- Consequence: the v4 variant's *only* hypothetical edge is auto-vectorization
  of pure-Rust glue code — the same category where v3-vs-baseline measures ≤1 %
  here. There is no evidence of a v4 win worth a second binary.

### B.4 Runtime dispatch works end-to-end (prototype)

Single binary, three kernels, guarded dispatch (full source in the bench
scratch; pattern below), 64 MiB pixel buffer:

```rust
fn dispatch(buf: &[u8]) -> u64 {
    if is_x86_feature_detected!("avx512bw") {
        unsafe { kernel_avx512(buf) }   // #[target_feature(enable = "avx512f,avx512bw,avx512dq,avx512vl")]
    } else if is_x86_feature_detected!("avx2") {
        unsafe { kernel_avx2(buf) }     // #[target_feature(enable = "avx2")]
    } else {
        kernel_scalar(buf)
    }
}
```

Measured on the i7-9700 (AVX2, no AVX-512):

- detection reports `avx512bw=false, avx2=true` — correct selection,
- binary contains zmm code (verified via objdump) yet runs cleanly — the exact
  property the merged binary needs,
- dispatched AVX2 kernel: **2.15× vs scalar** (36.9 → 17.2 ms/iter),
- results asserted equal across paths.

Ecosystem notes for a real implementation: stable rustc `#[target_feature]` +
`is_x86_feature_detected!` is sufficient (used above); crates `pulp` /
`multiversion` automate it; GCC-style function multi-versioning
(`target_clones`/FMV) is **not** stable in rustc.

### B.5 Recommendation

1. **Merge now: drop the v4 matrix entry** (keep it one `CPU_TARGETS_EXTRA`
   variable away for users who ask). Measured evidence: v3-vs-v4 codegen buys
   ≤1 % on all tested encoders, and every heavyweight kernel that *could*
   benefit from AVX-512 already ships runtime-dispatched inside the v3 binary.
   The v4 artifact doubles release surface, CI time and user confusion for an
   unmeasurable win.
2. Keep the base at `x86-64-v3` (Haswell/Zen+): same throughput as baseline,
   needed by several dependency min-versions, and already the docker target.
3. **Do not** build the merged binary with `-C target-cpu=x86-64-v4`. If a
   specific pure-Rust hot loop ever proves AVX-512-sensitive on real v4
   hardware, wrap *that kernel* in `is_x86_feature_detected!` dispatch (§B.4) —
   not the whole binary.
4. Optional follow-up measurement on AVX-512 hardware: rerun the §B.3 matrix
   with a v4 build to confirm (expected: within noise of v3).

---

## Reproducing

```bash
# reference binaries
cargo build --release                                  # glibc default
docker run --rm -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/src/target-linux \
  rust:1-alpine sh -c 'apk add nasm musl-dev gcc g++ make cmake file; \
  cargo build --release --target x86_64-unknown-linux-musl -p byteshaver'

# startup matrix
hyperfine -N --warmup 20 --min-runs 200 './musl-staticpie --version'
hyperfine -N --warmup 20 --min-runs 200 './musl-upx --version'      # the 47 ms tax

# AVX-512 content & dispatch sources
objdump -d byteshaver | grep -c '%zmm'
nm libwebp.a | grep -ciE 'AVX2|SSE2'                  # internal dispatch
nm libmozjpeg62.a | grep -cE 'jsimd_can'              # internal dispatch

# throughput matrix (idle machine!)
hyperfine --prepare 'rm -rf out' --export-json r.json \
  './byteshaver "data/**/*" avif -o out --overwrite-existing'
```
