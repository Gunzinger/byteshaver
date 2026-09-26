# byteshaver Feature Expansion — Implementation Plan Suite

This directory contains parallelizable implementation plans for five feature workstreams
plus a blocking architecture refactor. Each plan is written so a single subagent can
implement it without re-doing ecosystem research.

| Plan | Workstream | Depends on | Parallelizable with |
|------|-----------|------------|---------------------|
| [00-architecture-refactor.md](00-architecture-refactor.md) | WS0 — core refactor (blocking) | — | nothing (must land first) |
| [01-heic-heif-decoding.md](01-heic-heif-decoding.md) | WS1 — HEIC/HEIF (and AVIF) input | WS0 | WS2, WS3, WS4, WS5 |
| [02-jpeg-xl-source-and-target.md](02-jpeg-xl-source-and-target.md) | WS2 — JPEG XL input + output | WS0 (WS4 for metadata boxes) | WS1, WS3, WS5 |
| [03-png-oxipng-optimization.md](03-png-oxipng-optimization.md) | WS3 — oxipng PNG target | WS0 | everyone (smallest, good first) |
| [04-exif-metadata-handling.md](04-exif-metadata-handling.md) | WS4 — EXIF policies | WS0 | WS1, WS2 (contract in plan), WS3, WS5 |
| [05-animation-reencoding.md](05-animation-reencoding.md) | WS5 — animated gif/webp/apng/jxl source, webp/apng/gif target | WS0, WS4 contract | WS1, WS2, WS3 |
| [06-animated-avif-experimental.md](06-animated-avif-experimental.md) | WS6 — animated AVIF (experimental) | WS5, WS1 | standalone after WS5 |
| [07-headless-core-job-api.md](07-headless-core-job-api.md) | WS7 — headless job API (pre-GUI amendment) | WS0 | after WS1–WS6 or in parallel (small) |
| [08-gui.md](08-gui.md) | WS8 — desktop GUI | WS7 | single agent |

## Suggested execution order

```
WS0  ──▶  WS3 (merge first: proves the encoder pattern on a tiny diff)
     ├─▶  WS1 ──▶ WS6
     ├─▶  WS4 ──▶ (nothing, but WS2 wants it)
     ├─▶  WS2
     └─▶  WS5

WS0..WS6 ──▶ WS7 (headless job API) ──▶ WS8 (GUI)
```

All of WS1–WS5 can run in parallel once WS0 is merged, **provided each agent
respects the file-ownership map below**.

## File ownership map (merge-conflict avoidance)

Shared hot files and who may edit them:

| File | Owner | Others may |
|------|-------|-----------|
| `src/error.rs` | WS0 | append new variants only via WS0-provided `CustomError` fallback |
| `src/format.rs` | WS0 seeds; each WS adds **only its own enum variant + extension mapping** (one contiguous block) | small additions |
| `src/cli.rs` | each WS adds **only its own subcommand variant** (one contiguous block) + WS4 adds global EXIF args | small additions |
| `src/main.rs` | WS0 rewrites dispatch; each WS adds one match arm | small additions |
| `src/converter/mod.rs` | WS0 owns after refactor; WSs add module declarations + one registry entry | small additions |
| `Cargo.toml` | each WS appends its deps in the marked section | small additions |
| `src/converter/<encoder>.rs` | the WS owning that encoder | none |
| `src/metadata/*` (new) | WS4 | read-only consumers |
| `src/animation/*` (new) | WS5 | read-only consumers |

## Global decision points (need project-owner sign-off)

1. **JPEG XL encoder license** (WS2): `jpegxl-rs`/`jpegxl-sys` are **GPL-3.0-or-later**,
   which is incompatible with the MIT `byteshaver` crate. Recommended path is thin in-tree
   bindings to libjxl (BSD-3) built via `jpegxl-src` (BSD). Fallback: accept GPL by
   using `jpegxl-rs`. See WS2 §Decision.
2. **HEIC on Windows/musl release binaries** (WS1): libheif cannot be fully statically
   linked with codecs today without significant CI work. Recommended: full support on
   Linux gnu/docker, compile-time-stubbed (clear runtime error) on windows-gnu and musl
   until CI grows a static libheif+libde265+aom build.
3. **Default EXIF policy** (WS4): proposed default is `strip` (privacy-safe, matches
   current effective behavior of losing metadata). `keep` is opt-in.
4. **Animated AVIF** (WS6): no safe maintained Rust path; raw libheif-sys FFI only.
   Recommended: experimental behind a cargo feature, shipped only in docker/gnu builds.
5. **GUI framework** (WS8): recommended egui/eframe (pure Rust, MIT, first-class file
   drop); Tauri 2 as the polished alternative. Requires WS7 (headless job API) first —
   see WS7 §1 for the audited conflict list against WS0.

## Verified ecosystem facts used by the plans (as of 2026-09)

- `image` 0.25.10 (MIT OR Apache-2.0): `GifDecoder`/`WebPDecoder`/`png::ApngDecoder`
  implement `AnimationDecoder` (`into_frames()`, `loop_count()` — new in 0.25.10);
  `ImageDecoder` provides `exif_metadata()`, `orientation()`, `icc_profile()`,
  `xmp_metadata()`.
- `png` 0.18.1 (MIT OR Apache-2.0): native APNG **encoding** via
  `Encoder::set_animated/set_frame_delay/set_dispose_op/set_blend_op/set_sep_def_img`;
  `Info::exif_metadata` for eXIf chunks.
- `libheif-rs` 3.0.0 / `libheif-sys` 5.3.1+1.23.1 (MIT): read/decode incl. sequences
  (v1_20+); `embedded-libheif` feature statically links libheif 1.23.0 but **not** its
  codec libs (libde265/aom). Windows only via vcpkg.
- `jxl-oxide` 0.12.6 (MIT OR Apache-2.0, pure Rust): decode incl. animation, Exif/XMP
  aux boxes, ICC/CMS (`moxcms` pure-Rust CMS option), orientation applied on render.
  Decode-only (no encoder exists in pure Rust).
- `jpegxl-rs` 0.15 / `jpegxl-sys` 0.13 (**GPL-3.0-or-later**), `jpegxl-src` (BSD-3,
  builds static libjxl 0.12 via cmake).
- `oxipng` 10.2.1 (MIT): `optimize_from_memory`, `Options::{from_preset,
  max_compression}`, features `parallel`/`zopfli`/`binary` are optional.
- `kamadak-exif` 0.6.1 (BSD-2-Clause): read + `experimental::Writer` to re-serialize
  filtered EXIF; `Exif::buf()` for verbatim round-trip.
- `webp-animation` 0.10.0 (MIT OR Apache-2.0): animated WebP encode over
  `libwebp-sys2` 0.2 (BSD-3; bundles libwebp, works static/musl).
- `ravif` 0.13 (BSD-3): stills only, **no** sequence support; `avif-serialize` 0.8.9:
  no `avis` (animated AVIF) muxing.
