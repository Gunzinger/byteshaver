# byteshaver Feature Expansion — Implementation Plan Suite

> **STATUS: implemented** on branch `feat/feature-expansion` (WS0–WS5, WS7 and WS8
> done; WS6 spike recorded as blocked upstream, see
> [06-animated-avif-experimental.md](06-animated-avif-experimental.md)). Deviations
> that landed differently than specced are noted inline in each plan file (e.g.
> `dec-heif` is opt-in rather than default-on; GUI ships musl/windows binaries per
> the 08 §5.5 addendum; `webp-animation` uses its `static` feature for reproducible
> static linking). The plans are kept as historical specification + decision records.

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

---

# GUI improvement suite — Implementation Plan Suite 2

> **STATUS: implemented** on branch `plans/gui-ux-improvements` (plans 09–14
> done; decision gates resolved per each plan's recommendation: 13 shipped
> **approach B** — quality-ladder chips with the aligned form as the
> custom interior; 10 shipped DSSIM 3.5 + PSNR with `Manual` default;
> 11 adopted `egui_extras` 0.32.3). Notable deviations: 11's column
> *widths* persist per session only (visible set/sort/thumbnail mode
> persist in settings.json); the Modified column renders UTC (std has no
> portable local-time conversion); 14 migrates old flat settings layouts
> via a manual `Deserialize` and ships a built-in profile superset that
> covers every encoder (plan §5's format list subsumed); 10's metric
> settings live in settings.json with the row context menu as their UI.
> Deviations that landed differently than specced are noted in the merge
> commits. The plans remain as historical specification + decision record.

| Plan | Workstream | Size | Depends on | Parallelizable with |
|------|-----------|------|------------|---------------------|
| [09-gui-report-window.md](09-gui-report-window.md) | report window as independent OS window + resize fixes | S | — | all |
| [10-gui-quality-feedback.md](10-gui-quality-feedback.md) | compression ratio + color hint, DSSIM/PSNR quality metrics, visual difference inspector (feasibility: GO) | M | 11 (core `output_path` amendment, phase 3 only) | all |
| [11-gui-file-table.md](11-gui-file-table.md) | sortable table columns, target-format column, configurable fields (date/EXIF), open/reveal actions, thumbnails (overhead-budgeted) | L | — (owns the core amendment, lands it first) | all after its first commit |
| [12-gui-progress-feedback.md](12-gui-progress-feedback.md) | segmented progress bar (active-file shading, minimal animation), confetti scaled by compression ratio | M | — | all |
| [13-gui-options-ux.md](13-gui-options-ux.md) | three candidate presentations of the target-format options (decision gate), restore-defaults, icons; auto-sizing collapsible sections | M | — (integrates with 14) | all |
| [14-gui-presets.md](14-gui-presets.md) | user presets (persistence/sharing incl. titles+descriptions), literature-based built-in profiles for avif/webp/jxl | L | — (integrates with 13) | all |

## Suggested execution order

```
09 (S, independent)  ─┐
11 first commit ──────┼─▶ 10 (uses output_path)   13 ◀──(chips◀──built-ins)──▶ 14
12 (independent)     ─┘
```

09, 12 and 13 are fully independent; 11's isolated first commit (the core
`Outcome::Encoded { output_path }` amendment) unblocks 10's phase 3; 13
and 14 interlock via the quality-ladder chips / built-in profiles (either
order, migration path specced in 13 §2B).

## Shared-file ownership (cross-plan hot files)

| File | Owner | Others may |
|------|-------|-----------|
| `src/pipeline.rs` (`Outcome` amendment) | 11, first commit only | 10 consumes |
| `gui/src/panels/file_table.rs` | 11 (rewrite) | 10's ratio cell + context menu carried over by 11 |
| `gui/src/panels/footer.rs` | 12 (progress area) | 10 appends ratio text to `stats_line` |
| `gui/src/panels/options_panel.rs`, `gui/src/options.rs` | 13 | 14 adds the preset dropdown hook |
| `gui/src/queue.rs` | 11 (row model) | 10 ratio helpers, 12 pending/active glyph fix |
| `gui/src/settings.rs` | additive `#[serde(default)]` fields per plan (11/12/14) | — |
| `gui/Cargo.toml` | per-plan appends: 11 `egui_extras`+`image`, 10 `dssim` | — |

## Global decision points (need project-owner sign-off)

1. **13 D1** which target-format presentation ships (A aligned form /
   B quality ladder — recommended / C comparison matrix).
2. **10 D1/D2** metric engine (DSSIM recommended) and default mode
   (Manual recommended vs auto-after-run).
3. **11 D1** adopt `egui_extras` 0.32 (recommended) vs hand-rolled sort
   headers.
4. **12 D1** confetti default level (proposed: Regular).
5. **14 D2** which formats get built-in profiles (proposed: avif, webp,
   jxl + oxipng safe re-optimize).

## Verified ecosystem facts used by suite 2 (as of 2026-10)

- egui 0.32.3 (the pinned line, `gui/Cargo.toml`): `Context::
  show_viewport_immediate` (native OS windows; `context.rs:3920`),
  `Response::context_menu` (`response.rs:940`),
  `Context::animate_value_with_time` (`context.rs:3025`) all present.
- All core config types (`EncoderConfig`, policy enums) derive
  `Serialize + Deserialize` (WS7 C6) — presets/settings persistence is
  serde-native.
- `Outcome::Encoded` carries sizes but **no output path** today — suite 2
  amends it (11 §0).
- `Queue::begin_run` marks every row `Running`, so "currently being worked
  on" needs a distinct active-file set (12 §1).
- Candidate crates marked *verify at implementation time* in the plans:
  `egui_extras` 0.32, `dssim`/`ssimulacra2` (licence+API), `opener`/
  `open` reveal support, `image::io::Limits` sizing.

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

---

# Media capture pipeline — Plan Suite 3

> **STATUS: implemented** on branch `plans/media-capture-pipeline` (P0–P5;
> plan 17 §17 lists the deviations that landed differently: wgpu-based
> renderer, VHS 0.12 quirk handling in the xtask, a `stable`/`unstable`
> freshness classification in the manifest, Rust-scene registry).

| Plan | Workstream | Size | Depends on | Parallelizable with |
|------|-----------|------|------------|---------------------|
| [17-media-capture-pipeline.md](17-media-capture-pipeline.md) | scripted README screenshots/videos for CLI + GUI incl. post-processing, cache busting, CI auto-refresh | L | pending egui 0.36 upgrade landing | all |

## Verified ecosystem facts used by suite 3 (as of 2026-10)

- `egui_kittest` 0.36.2 (MIT OR Apache-2.0) tracks egui 0.36 exactly: `eframe`
  feature hosts a real `eframe::App` headlessly (no display, software raster by
  default; `snapshot` feature for PNG output; optional `wgpu` for GPU parity);
  MSRV 1.95.
- VHS (charmbracelet, MIT) requires `ttyd` + `ffmpeg`; `Output` supports
  gif/mp4/webm/**PNG-frame dirs** (no WebP — animated WebP comes from our
  ffmpeg post-process); `Wait /regex/` screen-content sync, `Screenshot`
  stills, `Source`-able shared settings, theme JSON, pinned font/geometry/
  margin/windowbar/framerate; `.ascii` golden output; docker image and CI
  action available.
