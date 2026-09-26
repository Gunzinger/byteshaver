# WS2 — JPEG XL: source decoding + full-option target encoding

**Depends on:** WS0 merged. Metadata embedding (§8) coordinates with WS4's contract;
WS2 must not block on WS4 — land with `use_box`/metadata behind the WS4 trait if WS4
isn't merged yet.
**Deliverable:** `.jxl` inputs decode (stills + animation + EXIF/ICC); new `byteshaver <pat> jxl`
target exposes the complete libjxl encoding surface.

---

## 1. Support-path analysis

### Decode

| # | Path | Verdict |
|---|------|---------|
| 1 | **`jxl-oxide` 0.12.6 (MIT OR Apache-2.0, pure Rust)** | ✅ recommended. Full decode incl. animation (`render_frame(idx)`, `Render::duration`), Exif/XMP aux boxes, ICC + CMS, orientation, JPEG bitstream reconstruction. musl/mingw friendly (no C deps unless `lcms2` feature chosen). |
| 2 | `jpegxl-rs` decode (libjxl) | ❌ decode is **first-frame-only**, Exif/XMP boxes unreachable (`DecodeError::NotImplemented("box handling")`), and GPL — dominated by option 1. |
| 3 | image crate | ❌ no JXL support. |

Use **`moxcms`** (pure Rust) as the CMS feature, **not** `lcms2` (C) — keeps static
musl/mingw builds viable. jxl-oxide applies orientation during render; `width()`/`height()`
are post-orientation.

### Encode — ⚠️ DECISION REQUIRED (license)

| # | Path | License | Notes |
|---|------|---------|-------|
| A | **thin in-tree bindings to libjxl C API, libjxl built by `jpegxl-src` 0.12 (BSD-3) via cmake** | MIT-compatible ✅ | ~15–25 `extern "C"` declarations in a small `src/converter/jxl/ffi.rs` (no bindgen, no libclang in CI). Full access to `JxlEncoderSetFrameDuration` (animated), `JxlEncoderAddBox` (Exif/XMP), and the complete `JxlEncoderFrameSettingId` surface → genuinely "all encoding options". More initial work + unsafe code. |
| B | `jpegxl-rs` 0.15 + `jpegxl-sys` 0.13 (`vendored` = static libjxl via jpegxl-src) | **GPL-3.0-or-later** ❌ | Fastest route. But publishing `byteshaver` (MIT) with a GPL dep effectively GPLs the binary. Also: **no frame-duration API** (multi-frame without timing ≠ real animation), no Exif box read on decode, weaker option surface (though `set_frame_option(JxlEncoderFrameSettingId, i64)` exists). |

**Recommendation: A.** It is the only path that (a) preserves the MIT license,
(b) supports animated JXL properly (WS5 synergy), (c) exposes the full option surface
the task requires. Plan below is written for A; a §10 appendix maps the deltas if the
owner picks B.

Build requirements for A (all distros in CI): `cmake`, C++ compiler (already present;
nasm already required by rav1e). `jpegxl-src` builds **static** libjxl
(`BUILD_SHARED_LIBS=OFF`) — good for musl; for windows-gnu a cmake toolchain file is
needed → Phase 1 stubs jxl encode on windows-gnu (same stub pattern as WS1), full
support linux-gnu/docker/musl.

---

## 2. Cargo changes

```toml
[features]
default = [..., "jxl"]
jxl = ["dep:jxl-oxide", "dep:jpegxl-src", "dep:moxcms"]   # B: dep:jpegxl-rs

[dependencies]
jxl-oxide   = { version = "0.12", default-features = false, features = ["rayon", "moxcms", "image"], optional = true }
moxcms      = { version = "0.8", optional = true }
jpegxl-src  = { version = "0.12", optional = true }   # build-dep role: links libjxl
# [build-dependencies] if jpegxl-src is used as a build script dependency;
# simpler: our own build.rs hook calls jpegxl_src (see §3)
```

Note: `jpegxl-src` is a library crate exposing `jpegxl_src::build()` for use inside a
`build.rs`. Add to `[build-dependencies]` (feature-gated via
`#[cfg(feature = "jxl")]` guarded build logic — build-deps can't be optional; instead
call it from `build.rs` when the `jxl` feature is active, detected via
`CARGO_FEATURE_JXL` env var).

`src/converter/jxl/ffi.rs` declares the libjxl C API subset
(`libjxl-0.12/lib/include/jxl/encode.h`, `decode.h` not needed):

```c
JxlEncoder*                 JxlEncoderCreate(const void* version);
void                        JxlEncoderDestroy(JxlEncoder*);
JxlEncoderStatus            JxlEncoderProcessOne(JxlEncoder*);           // (0.12: JxlEncoderProcessOne/JxlEncoderFlush)
JxlEncoderStatus            JxlEncoderSetBasicInfo(JxlEncoder*, const JxlBasicInfo*);
JxlEncoderStatus            JxlEncoderSetColorEncoding(JxlEncoder*, const JxlColorEncoding*);
JxlEncoderStatus            JxlEncoderSetICCProfile(JxlEncoder*, const uint8_t*, size_t);
JxlEncoderStatus            JxlEncoderAddImageFrame(const JxlEncoderFrameSettings*, const JxlPixelFormat*, const void*, size_t);
JxlEncoderStatus            JxlEncoderSetFrameHeader(JxlEncoder*, const JxlFrameHeader*);
JxlEncoderStatus            JxlEncoderSetFrameDuration(const JxlEncoderFrameSettings*, uint32_t);
JxlEncoderStatus            JxlEncoderAddBox(const JxlEncoderFrameSettings*, JxlBoxType, const uint8_t*, size_t, int compress);
JxlEncoderStatus            JxlEncoderUseBoxes(JxlEncoder*);
JxlEncoderStatus            JxlEncoderSetFrameLossless(const JxlEncoderFrameSettings*, int);
JxlEncoderStatus            JxlEncoderSetFrameDistance(const JxlEncoderFrameSettings*, float);
JxlEncoderStatus            JxlEncoderSetFrameBitDepth(const JxlEncoderFrameSettings*, ...);   // if present in 0.12
JxlEncoderFrameSettings*    JxlEncoderFrameSettingsCreate(JxlEncoder*, const JxlEncoderFrameSettings*);
JxlEncoderStatus            JxlEncoderSetOption / JxlEncoderFrameSettingsSetOption(JxlEncoderFrameSettings*, JxlEncoderFrameSettingId, int64_t);
JxlEncoderStatus            JxlEncoderSetVectorConfig / JxlEncoderSetParallelRunner(JxlEncoder*, ...);
JxlEncoderStatus            JxlEncoderSetCodestreamLevel / JxlEncoderSetFrameName ...
```

(Exact 0.12 signatures to be cross-checked against vendored headers during
implementation; the surface above is the stable public encoder API. `JxlEncoderFrameSettingId`
constants for the advanced passthrough flag are copied verbatim from the header.)

## 3. Input: decoding `.jxl`

New file `src/input/jxl.rs`:

```rust
pub fn decode(path: &Path) -> Result<SourceImage, Error>
```

- `JxlImage::builder().open(path)`.
- **Still:** `image.render_frame(0)` → `Render::image_all_channels() -> FrameBuffer`
  → to `Rgba8`/`Rgb8` (check `PixelFormat`/channel count; extra channels beyond alpha:
  compose alpha only, ignore others with a warning). Result → `DynamicImage`.
- **Animation:** loop `0..image.num_loaded_keyframes()`, `render_frame(i)`,
  `Render::duration()` (ticks; timescale from
  `image.image_header().metadata.animation` — check `tick_multiplier`/`tps` fields at
  implementation time) → `std::time::Duration`. Loop count: header animation
  `num_loops` (0 = infinite) → `LoopCount`. Build `AnimationData` (WS0 type).
- **Metadata:** `image.aux_boxes().first_exif()` → `RawExif::payload()` +
  `tiff_header_offset()` (strip the offset prefix to get plain TIFF payload for
  `ImageMetadata.exif`); XMP via `first_xml()`; ICC via `original_icc()`.
  Orientation is applied by render → set `exif_applied_orientation = true`.
- **JPEG reconstruction bonus:** if `image.jpeg_reconstruction_status() == Available`
  and a new flag `--jxl-jpeg-reconstruct` is set with target `jpeg`: emit the original
  JPEG bitstream via `image.reconstruct_jpeg(writer)` (bit-exact passthrough). Status:
  stretch goal, cheap to add, clearly separate code path.
- Register in `input::load_source` dispatch: `ImageFormat::Jxl` (extension `"jxl"`),
  plus signature sniffing fallback (codestream starts `0xFF 0x0A`, container `JXL `)
  for wrong-extension files — route via extension only in Phase 1.

## 4. Target: `byteshaver <pattern> jxl` — full option surface

### CLI (all optional, defaults mirror `cjxl`)

```rust
/// Convert images to jpeg-xl format (libjxl)
Jxl {
    /// JPEG-style quality 0-100 (higher = better). Mutually exclusive with --distance.
    #[clap(short, long, group = "jxl-quality-group")]
    quality: Option<f32>,
    /// Maximum Butteraugli distance (0.0 = mathematically lossless, 1.0 ≈ visually lossless, default 1.0).
    #[clap(long, group = "jxl-quality-group")]
    distance: Option<f32>,
    /// Lossless mode. Overrides quality/distance.
    #[clap(long)]
    lossless: bool,
    /// Encoding effort 1 (fastest) – 10 (slowest/best). Default 7.
    #[clap(short, long, value_parser = clap::value_parser!(u8).range(1..=10))]
    effort: Option<u8>,
    /// Container/box handling: force ISOBMFF container (required for Exif/XMP embedding).
    #[clap(long)]
    container: bool,
    /// Keep original color profile (do not convert to internal XYB); needed for lossless.
    #[clap(long)]
    original_profile: bool,
    /// Target decode speed tier 0-4 (higher = faster decode, larger file). Default 0.
    #[clap(long)]
    decoding_speed: Option<u8>,
    /// Photometric target intensity in nits (HDR). Default 255.
    #[clap(long)]
    intensity_target: Option<f32>,
    /// Force output bit depth: 8 or 16 (default: follow input).
    #[clap(long, value_parser = clap::value_parser!(u8).range(8..=16))]
    bit_depth: Option<u8>,
    /// Color encoding: srgb | linear-srgb | srgb-luma | linear-srgb-luma | icc-passthrough
    #[clap(long, value_enum)]
    color_encoding: Option<JxlColorEncodingChoice>,
    /// Advanced: repeatable libjxl frame-setting passthrough, e.g. --setting brotli_effort=9
    #[clap(long = "setting", value_name = "ID=VALUE")]
    settings: Vec<String>,
}
```

Mapping to libjxl calls (Option A):
- `quality` → distance via libjxl's linear mapping (same formula `jpegxl-rs`
  `jpeg_quality()` uses: distance = f(quality)); `distance` direct;
  `lossless` → `JxlEncoderSetFrameLossless(1)` + `original_profile=1` auto.
- `effort` → `JxlEncoderFrameSettingsSetOption(…, JxlEncoderFrameSettingId::Effort, n)`.
- `decoding_speed` → `SetOption(DecodingSpeed, 0..4)` (validate; error outside).
- `intensity_target` → `JxlBasicInfo.intensity_target`.
- `bit_depth` → pixel format selection (u8 vs u16 buffers; convert input via
  `to_rgb8/to_rgba8` or `to_rgb16/to_rgba16`).
- `color_encoding` → `JxlEncoderSetColorEncoding` enum mapping, or
  `JxlEncoderSetICCProfile` with input ICC when `icc-passthrough` and input has ICC.
- `container` → `JxlEncoderUseContainer(1)`; auto-enabled when metadata boxes are
  added (§8).
- `settings` passthrough: parse `id=value`, resolve `id` case-insensitively against a
  name table generated from `JxlEncoderFrameSettingId` variants; unknown → CLI error
  listing available ids. This single flag future-proofs the "all options" requirement.

`JxlOptions` struct mirrors the CLI (with `Option` fields), lives in
`src/converter/jxl/mod.rs`, implements `ImageEncoder` (`supports_animation() = true`).

### Encoder flow (Option A)

```
JxlEncoderCreate → SetBasicInfo(w,h,bits,alpha,intensity,orientation=1)
  → color: SetColorEncoding | SetICCProfile | (lossless → uses_original_profile)
  → [metadata] JxlEncoderUseBoxes + AddBox(Exif/XMP)   (WS4 contract)
  → per frame: FrameSettingsCreate → SetFrameLossless/SetFrameDistance →
       animated: SetFrameHeader(is_last) + SetFrameDuration(ms ticks)
  → AddImageFrame(pixel_format, buf) for each frame (still = 1 frame)
  → JxlEncoderFinish → collect output buffer (grow loop, 64 KiB chunks)
```

Threading: single-threaded encoder per file by default (respect WS0 `ThreadBudget`;
optional `--setting num_threads=N`? libjxl uses a parallel runner — Phase 1: no
parallel runner, note as future optimization since rayon already parallelizes files).

### encode-animated note

With Option A, animated JXL encoding is real (frame durations). With fallback Option B,
`encode` on `ImageContent::Animated` must take frames **without timing** or reject with
`Error::Unsupported("animated jxl requires frame-timing support")` — document in §10.

## 5. Statistics/describe

`describe()` prints libjxl version (link `jpegxl-src` version const) + all active
options; uses WS0 `DEPENDENCIES` table (`jxl-oxide`, `jpegxl-src` entries appear
automatically via build.rs).

## 6. format.rs / dispatch

- `ImageFormat::Jxl` (shared with §3): `extension() = "jxl"`, `from_extension`: `"jxl"`.
- Input filter list: Jxl is an **input** too — allowed through (not excluded).
- Encoder registry: add `EncoderConfig::Jxl(JxlOptions)`.

## 7. Tests

- Fixtures: create small `.jxl` still (lossy), `.jxl` lossless, animated `.jxl`
  (3 frames, varying delays), `.jxl` with Exif box — generate with `cjxl` once in a
  scratch env and commit (or via `jxl-oxide` test helpers if impractical: min fixture =
  encode with our own encoder in the test then re-decode → round-trip property test).
- Round-trip property tests: png → jxl (lossless) → decode == original pixels.
- Animation round-trip: gif (WS5 fixtures) → jxl → frame count + delays preserved
  (±1 ms tolerance / documented tick rounding).
- Option surface: each CLI flag maps (unit test on `JxlOptions → encoder calls` shim);
  `--setting` unknown id errors with suggestions.
- 16-bit path: gray ramp u16 → jxl → decode u16 equality.
- musl: `cargo build --target x86_64-unknown-linux-musl --features jxl` in CI job.

## 8. Coordination contract with WS4 (EXIF)

- WS4 defines (in `src/metadata/mod.rs`): `fn exif_for_jxl(tiff_payload: &[u8]) ->
  Vec<u8>` = 4-byte little-endian tiff-header-offset prefix + payload (libjxl Exif box
  format). WS2 calls it if WS4 is merged; otherwise sets box code behind
  `#[cfg(feature = "exif")]` stub. Embedding auto-enables `JxlEncoderUseContainer`.
- XMP: same pattern (`xml ` box), Phase 2.

## 9. Risks / mitigations

- **Unsafe FFI correctness** → keep ffi.rs pure declarations; encode flow in safe
  wrapper; round-trip tests vs jxl-oxide (independent decoder = oracle); fuzz-lite
  loop over option matrix in tests.
- **libjxl 0.12 API drift** → pin jpegxl-src 0.12.x; CI builds once per release.
- **cmake in every dev env** → document in README prerequisites next to nasm.
- **windows-gnu** → stub mode identical to WS1 §6 (`Error::FeatureDisabled`), keep CLI
  present.

## 10. Appendix — deltas if Option B (jpegxl-rs) is chosen instead

- Remove ffi.rs/jpegxl-src wiring; add `jpegxl-rs = { version = "0.15", features =
  ["vendored"] }`; note GPL in README + Cargo description of the published crate.
- Encode via `encoder_builder().lossless(...).speed(EncoderSpeed::…).quality(distance)
  .jpeg_quality(…).decoding_speed(...).use_container(...).uses_original_profile(...)
  .has_alpha(...).color_encoding(...).target_intensity(...)`.
- `EncoderSpeed` map: `Lightning=1, Thunder=2, Falcon=3, Cheetah=4, Hare=5, Wombat=6,
  Squirrel=7, Kitten=8, Tortoise=9, Glacier=10` — CLI `--effort 1..=10` maps directly.
- Animated: `encoder.multiple(w,h)` + `add_frame` — **no timing**: reject animated
  inputs with clear error (or encode first frame; prefer reject).
- Metadata: `encoder.add_metadata(&Metadata::Exif(bytes), compress)` with the 4-byte
  offset prefix (same WS4 helper).
- Advanced `--setting` passthrough: partially available via
  `JxlEncoder::set_frame_option(JxlEncoderFrameSettingId, i64)`; keep the same CLI.

## 11. Acceptance criteria

- [ ] `.jxl` stills/animated/EXIF inputs decode; README input list updated.
- [ ] `byteshaver "examples/**/*" jxl --effort 7` produces valid JXL (verified by jxl-oxide
      decode in tests).
- [ ] Full documented option surface compiles and is covered by tests incl.
      `--setting` passthrough.
- [ ] Animated input → animated JXL output (Option A) with preserved delays.
- [ ] License status of all new deps recorded in this file + README (must be
      MIT-compatible under Option A).
