# Plan 17 — AVX-512 runtime dispatch for vendored libjxl & fully static libheif in all artifacts

> **STATUS: implemented** (this branch), with one scoped deferral:
> **A** done — vendored `jpegxl-src` patch enables `AVX3` + `AVX3_ZEN4`
> (AVX3_SPR intentionally off); verified 59,796 zmm instructions in
> `libjxl.a`, AVX2-machine encode/decode unchanged and 342/342 tests green.
> **B Linux** done — `dec-heif` is now a default feature with libheif 1.23.1
> statically embedded (`libheif-sys` `embedded-libheif`, an even cleaner
> mechanism than the planned external libheif build — no fork needed) plus
> static libde265 1.1.3 + dav1d 1.5.1 from `tools/libheif-static/`
> (decode-only: x265/aom/libwebp excluded); musl static-pie binary
> 16.2 → 22.7 MB, packed 7.19 MB (31.7 %), HEIC+AVIF decode verified,
> x265 symbols absent. **B Windows** done via the gnullvm/ucrt trial (§B.6):
> the gcc-built deps cross-compile cleanly, but mingw's dynamic `-lstdc++`
> (libheif.pc Libs.private) pulls `libstdc++-6.dll` and cannot be outranked
> by static link flags — switching the Windows leg to the llvm-mingw
> toolchain (clang + static libc++, UCRT) resolves the whole class of
> problem. Windows binaries link libc++/libunwind statically (the dynamic
> import libs are removed from the toolchain), self-contain the full
> dec-heif stack, and pass wine HEIC+AVIF decode. CI builds Windows on
> `x86_64-pc-windows-gnullvm` with a pinned, cached llvm-mingw 20260922.
> UCRT baseline: Windows 10+.
> **A** enables libjxl's AVX-512 kernels with highway runtime dispatch
> (safe on AVX2 clients), **B** ships the `dec-heif` feature in all static
> artifacts via [audivir/libheif-static](https://github.com/audivir/libheif-static).
> Both were researched to root-cause level; implementation is mechanical.

---

## Part A — libjxl with AVX-512 runtime dispatch

### A.1 Root cause: why the vendored build has no AVX-512 kernels

Findings from the vendored source (`jpegxl-src 0.12.0` → `libjxl`,
`CMakeLists.txt`) and the build tree CMakeCache of the current release build:

1. libjxl declares all highway targets with explicit per-target options
   (`CMakeLists.txt` line 128–137):

   ```cmake
   set(JPEGXL_HWY_TARGETS AVX2 AVX3 AVX3_DL AVX3_SPR AVX3_ZEN4 ...)
   set(JPEGXL_HWY_TARGETS_OFF_BY_DEFAULT AVX3 AVX3_SPR AVX3_ZEN4 RVV SSSE3 SVE_256)
   ```

   → **`AVX3`, `AVX3_ZEN4`, `AVX3_SPR` are OFF by default** — an upstream
   binary-size choice, *not* a toolchain probe failure.

2. Disabled targets become compile definitions
   (`CMakeLists.txt` line 228–241):

   ```cmake
   -DHWY_DISABLED_TARGETS=(HWY_AVX3|HWY_AVX3_SPR|HWY_AVX3_ZEN4)
   -DFJXL_ENABLE_AVX512=0
   ```

3. Highway kernels are compiled into the *consumer* translation units, so the
   evidence lives in `libjxl.a` (`libhwy.a` only holds the dispatch runtime —
   0 SIMD instructions there is expected). Measured: `libjxl.a` contains
   **zero** `zmm` instructions; the 18,389 `zmm` instructions in the shipped
   binary come from Rust-side dependencies, not libjxl.

4. `jpegxl-src 0.12.0` (`src/lib.rs`) hardcodes its `cmake::Config` defines
   (`BUILD_TESTING=OFF`, `JPEGXL_ENABLE_TOOLS=OFF`, …) and exposes **no
   mechanism to add cmake cache variables**.

### A.2 Why enabling AVX3 is safe on AVX2 clients (the core question)

Enabling `HWY_AVX3` et al. does **not** change which code runs on a given
CPU — highway's dispatch model is exactly the runtime-detection design asked
for:

- every target is guarded by runtime `cpuid` checks **plus** `xgetbv` OS-state
  checks (AVX-512 requires OS ZMM/opmask support — highway checks `xcr0`
  bits 5–7 before selecting AVX3-class targets);
- `AVX3_DL` additionally requires VBMI/VNNI; `AVX3_ZEN4`/`AVX3_SPR` check
  their respective feature bits;
- a machine without those features dispatches to the best compiled target it
  *does* support (AVX2 today) — byte-for-byte the current behavior.

The only reason AVX-512 kernels don't run today is that they were never
compiled into the vendored library. Flipping the options cannot regress
AVX2/non-AVX2 clients; the risks are binary size and build toolchain
tolerance (A.5/A.6).

### A.3 Change

| cmake variable | current | new |
|---|---|---|
| `JPEGXL_ENABLE_HWY_AVX3` | false | **true** |
| `JPEGXL_ENABLE_HWY_AVX3_ZEN4` | false | **true** |
| `JPEGXL_ENABLE_HWY_AVX3_SPR` | false | optional (start false; +size, covers Sapphire-Rapids-class AVX-512 only) |
| `JPEGXL_ENABLE_HWY_AVX3_DL` | true | unchanged (already on) |

(`AVX3_DL` implies AVX3-class runtime checks incl. VBMI/VNNI; with AVX3 also
enabled, plain AVX-512 machines get AVX3 and richer machines get AVX3_DL.)

### A.4 Injection mechanism (jpegxl-src doesn't expose defines)

Options, in preference order:

- **A (implemented): submodule + patch.** `tools/jpegxl-src` is a git
  submodule pinned to the `jpegxl-src` 0.12.0 release commit
  (inflation/jpegxl-rs @ `884f36f`; libjxl rides along as its nested
  submodule), plus `tools/patches/jpegxl-src-avx512.patch` applied by
  `tools/patches/apply.sh` after checkout — three lines in `src/lib.rs`:

  ```rust
  .define("JPEGXL_ENABLE_HWY_AVX3", "ON")
  .define("JPEGXL_ENABLE_HWY_AVX3_ZEN4", "ON")
  // .define("JPEGXL_ENABLE_HWY_AVX3_SPR", "ON")  // optional, see A.3
  ```

  and pin it in the workspace `Cargo.toml`. Small diff against upstream;
  easy to drop when an upstream PR lands.
- **B: implement the cmake invocation in our `build.rs`** with the `cmake`
  build-dep (already transitively present), pointing at the vendored libjxl
  source shipped inside the jpegxl-src crate. More control (e.g. also
  switching `-DFJXL_ENABLE_AVX512`), more code to maintain.
- **C: upstream PR to jpegxl-src** exposing `JPEGXL_CMAKE_DEFINES` env.
  Do in parallel; don't block on it.

### A.5 Expected impact

- `libjxl.a` grows by the AVX3/AVX3_ZEN4 kernel sections (expect roughly
  +1.5–3 MB archive size; final binary +~1–2 MB before packer — measure).
- Startup grows proportionally to image size (see the size/startup scaling in
  `docs/startup-and-cpu-variant-exploration.md` §A.2) — packed `-packed`
  variants absorb most of it; unpacked artifacts get slower by ~0.3–0.5 ms.
- **jxl encode/decode speed on AVX-512 hardware improves** — not measurable
  on the AVX2-only development machine; treat as expected-not-proven until a
  v4 machine run (same caveat as the v3/v4 merge analysis).
- AVX2 machines: no behavior or performance change expected; verify anyway.

### A.6 Validation checklist

1. Build succeeds on both vendored-toolchain paths (linux/musl gcc,
   windows-gnu mingw-w64). Highway uses per-target `#pragma` flags — mingw
   gcc 13+ handles AVX-512 pragmas; watch for assembler/binutils hiccups.
2. `objdump -d libjxl.a | grep -c zmm` > 0 (per enabled target family).
3. Run the jxl encode + decode round-trip on the AVX2 dev machine:
   **no SIGILL**, output decodes correctly (libjxl output is not guaranteed
   bit-identical across target dispatch — compare via decode validity, not
   bytes).
4. Confirm the disabled-targets define is gone: no
   `HWY_DISABLED_TARGETS=(HWY_AVX3` in the cmake build log.
5. Full existing test suite (`cargo test --workspace`).
6. Optional: Intel SDE run (`-cpuid_in` AVX-512 skylake-avx512 model) to
   exercise the AVX3 dispatch path without hardware — timing meaningless,
   SIGILL-freedom meaningful.

---

## Part B — static libheif (dec-heif) in all artifacts

### B.1 What audivir/libheif-static provides

- `build.sh` producing **fully static** `libheif.a` + codec dependencies
  (libde265, x265, libaom, dav1d, libwebp/libsharpyuv as git submodules),
  `ENABLE_PLUGIN_LOADING=OFF` (plugins baked in), relocatable pkg-config
  files, headers in `dist/`.
- **musl archives bundle `libstdc++.a` + `libgcc_eh.a` and reference them
  from their pkg-config `Libs.private`** — exactly what our static-pie link
  needs. glibc and musl both supported; CI proves musl (Alpine) builds.
- Windows targets via **llvm-mingw** (UCRT), `windows-amd64`/`-arm64`, with
  prebuilt archives and native smoke tests.
- Repo code MIT; `NOTICE` lists vendored-library licenses.
- Smoke-test script for HEIC/AVIF decode through the static lib.

### B.2 License handling (blocking decisions inside)

| library | license | static distribution |
|---|---|---|
| libheif | **LGPL-3.0** | allowed with §4 obligations: ship "Installation Information" + Corresp. Source (or object files) enabling users to relink a modified libheif. We satisfy this by pinning exact source commits + publishing our build script (it *is* the relink procedure) and keeping `THIRD-PARTY-NOTICES.md` updated. |
| libde265 | **LGPL-3.0** | same treatment as libheif |
| dav1d | BSD-2 (VideoLAN) | notice only |
| libaom | BSD-3 + Apache/PATENTS | notice only (likely dropped, see below) |
| **x265** | **GPL-2.0** | **must be excluded** — statically linked GPL code in an MIT binary makes the distribution GPL-2.0. Decode-only doesn't need x265 (HEVC *encode*). |
| libwebp/libsharpyuv | BSD | only needed for heif webp *encode* — droppable for decode-only |

**Build recipe change vs upstream `build.sh` (decode-only, license-clean):**

```
libde265:  keep (HEVC/HEIC decode)
dav1d:     keep (AV1/AVIF decode)
x265:      DROP (WITH_X265=OFF)          ← GPL-2.0, encoder only
libaom:    DROP (dav1d covers AV1 decode) ← smaller; keep only if dav1d
                                            parity gaps appear
libwebp:   DROP (libsharpyuv is heif-webp-encode only)
```

Drop the corresponding steps (2, 3, 5) and the x265.pc/aom.pc verification
entries; set `-DWITH_X265=OFF -DWITH_AOM_DECODER=OFF -DWITH_AOM_ENCODER=OFF
-DWITH_LIBSHARPYUV=OFF`. Maintainer sign-off on the LGPL static-linking
compliance approach is the go/no-go decision.

### B.3 Integration into byteshaver

1. **Vendor the patched recipe**: add `tools/libheif-static/` as a pinned
   fork/submodule snapshot (upstream repo + our decode-only patch to
   `build.sh`), commit-pinned for reproducibility.
2. **Linux build (`build_binaries`, alpine step)**: before `cargo build`,
   run the patched `build.sh` (~5–10 min; cacheable on the
   `byteshaver-linux-target-*` cache key or its own key on the pinned
   commit), then export:
   ```bash
   export PKG_CONFIG_PATH="$DIST/lib/pkgconfig"
   export PKG_CONFIG="pkg-config --static"
   ```
   `libheif-sys` 5.3.1 (used by `libheif-rs` 3.0) resolves via pkg-config and
   emits the `.a` link directives incl. `Libs.private` (libstdc++.a,
   libgcc_eh.a — bundled by the recipe). Verify the final musl link stays
   static-pie (existing `file` assertion catches regressions).
3. **Feature flip**: move `dec-heif` into `default` features (keep the
   feature so an escape-hatch `--no-default-features` build remains). The
   `--heif-image-policy` flag and all heif decoding code are already
   feature-wired; no source changes expected beyond `Cargo.toml` + README.
4. **Size/startup budget** (estimates, verify in implementation):
   libheif ~3–5 MB + libde265 ~2–3 MB + dav1d ~1.5–2.5 MB of `.a` text →
   binary **16.2 → ~22–25 MB** (musl static-pie), `-packed` ~6.5–8 MB;
   unpacked startup +~0.5–0.8 ms (image-size scaling), packed +~1–2 ms.
   Acceptance of this regression is part of the go/no-go (it conflicts with
   the "instant startup" goal for unpacked artifacts; the packed variants
   hide most of it).
5. **Docker**: afterwards the alpine images can install the static binaries
   directly and drop `libheif-dev/libde265-dev/aom-dev` + shared runtime
   packages; keep `validate_docker`'s real-HEIC end-to-end test as the
   parity gate. Optional second step; dynamic docker images remain valid.
6. **Windows**: upstream supports `windows-amd64` via **llvm-mingw (UCRT)**.
   Our windows-gnu toolchain is gcc/MSVCRT — mixing UCRT-built static C++
   libs into an MSVCRT link is unsupported. Options (phase 2, not blocking):
   - (a) build libheif+libde265+dav1d with our mingw-w64 gcc directly
     (cmake/meson recipes, moderate effort, known-good compilers), or
   - (b) switch the windows target to `x86_64-pc-windows-gnullvm`
     (llvm-mingw) — larger toolchain migration, aligns with (a)'s output.
   **RESOLVED (gnullvm trial):** the windows leg now builds on
   `x86_64-pc-windows-gnullvm` (llvm-mingw, UCRT — Windows 10+ baseline)
   with dec-heif statically embedded; the older gcc-mingw/MSVCRT route is
   what couldn't link statically. gcc-built deps also cross-compile, so
   either toolchain produces the codec libraries — the gnullvm toolchain is
   what makes the final binary self-contained.
7. **CI smoke**: extend the pack/build verification with a real `.heic` +
   `.avif` decode on the suffixless linux binary (the `validate_docker`
   fixture logic, moved to the binary smoke step).

### B.4 Risks & mitigations

| risk | mitigation |
|---|---|
| LGPL-3.0 compliance slip | compliance checklist in THIRD-PARTY-NOTICES; pinned sources + published build script; revisit before each release |
| binary size growth breaks size goals | measure gate in CI (log artifact sizes); `-packed` variants absorb most; `--no-default-features` escape hatch remains |
| libheif version drift vs libheif-rs 3.0 (needs ≥1.19; alpine CI validated 1.23.x) | pin the vendored libheif commit; libheif-sys 5.3.1 is built against 1.23.x — pin that exact minor |
| musl C++ runtime link issues (libstdc++.a ordering, `libgcc_eh`) | recipe already bundles + patches .pc files; final `file` static-pie assertion in CI |
| dav1d vs aom decode parity (10-bit AVIF, multi-image avif) | golden-file tests for the existing heif fixture set; keep libaom-decoder as a fallback option if gaps appear |
| mingw/clang build of libjxl AVX-512 kernels (Part A) on windows toolchain | gate: build must pass; else keep windows libjxl at AVX2 (per-target matrix is possible via the same cmake defines) |

### B.5 Rollout order

1. **A (libjxl AVX-512)** — small, self-contained, no licensing impact;
   land first behind the vendored-patch mechanism; measure size delta.
2. **B Linux** — recipe fork + alpine build step + feature flip + CI smoke +
   size report. Docker simplification afterwards.
3. **B Windows** — needs the toolchain decision (gcc-built deps vs gnullvm);
   separate plan/PR.

### B.6 Open decisions for the maintainer

1. Accept the LGPL-3.0 static-linking compliance approach (§B.2)?
2. Accept the estimated +6–9 MB artifact growth (unpacked) for
   heic/avif-everywhere, or keep dec-heif docker-only?
3. AVX3_SPR kernels in Part A: include (largest size cost, narrowest benefit)
   or defer?
4. Windows: gcc-built deps (pragmatic) vs gnullvm migration (cleaner, bigger)?
