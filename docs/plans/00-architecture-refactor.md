# WS0 — Core Architecture Refactor (blocking foundation)

**Owner:** must merge before WS1–WS5 start.
**Scope:** restructuring only — no new formats. Behavior of existing commands is preserved.
**Risk:** touches every file; therefore it lands first, alone, and defines the extension
contract that all other workstreams implement.

---

## 1. Why the current architecture cannot absorb the five features

Evidence from the code:

1. **Positional option explosion.**
   `convert_images(...)` (src/converter/mod.rs:95-107) already takes **9 positional
   `Option<T>` parameters**; `convert_image` (src/converter/mod.rs:459-477) takes 11.
   JPEG XL adds ~8 options, oxipng ~12, EXIF 3, animation several. The signature would
   exceed 30 parameters — unmaintainable and the #1 source of merge conflicts between
   parallel workstreams.

2. **CLI ↔ encoder coupling.**
   src/cli.rs:83-97 references `crate::converter::avif::BitDepth` etc. directly. The
   `copy_enum_variants!` macro in src/converter/avif.rs:7-20 exists only to work around
   this layering problem.

3. **Lossy-by-design decode pipeline.**
   `try_read_image` (src/converter/mod.rs:358-391) returns only `DynamicImage`:
   - EXIF/ICC/XMP are silently dropped (image 0.25's `ImageDecoder::exif_metadata()` is unused);
   - animated GIF/WebP/APNG inputs are silently reduced to frame 1;
   - >8-bit sources are flattened by eager `to_rgb8()`/`to_rgba8()` conversions inside encoders;
   - panics from zune-jpeg are caught with `panic::catch_unwind` hacks
     (src/converter/mod.rs:361-390) because format detection is not pinned before decode.

4. **Magic-number status tuples.**
   `(isize, usize, usize)` with codes -2/-1/0/1/2 (src/converter/mod.rs:208-247, 478-484).
   Two `TODO`s in the file admit statistics propagation is broken for skipped/discarded
   files (src/converter/mod.rs:131-132, 554, 564).

5. **Thread oversubscription.**
   rayon parallelizes across files (src/converter/mod.rs:208-210) while each encoder
   spawns its own thread pool (ravif, mozjpeg). Adding oxipng (its own rayon pool),
   libjxl (parallel runner) and libwebp would multiply this. Needs one global budget.

6. **Un-usable pieces for the new features**
   - `utils::is_supported` (src/utils.rs:16-27) reads the **entire file** into memory just
     to guess the format — unusable for animations and slow for big files.
   - The input filter hard-excludes AVIF with a FIXME (src/converter/mod.rs:110-114).
   - Output-name collision handling is an unresolved TODO (src/converter/mod.rs:131-132);
     becomes critical when HEIF files contain multiple images and animations produce
     same-stem outputs.
   - No tests, no clippy/fmt CI gate.

---

## 2. Target design

### 2.1 New module layout

```
src/
  main.rs                 — thin: parse CLI → build Config + EncoderConfig → run
  cli.rs                  — pure CLI (clap) definitions; no converter-type references
  config.rs (new)         — ConversionConfig (global flags incl. EXIF policy hooks)
  format.rs               — ImageFormat (input/target) + capability helpers
  error.rs                — Error enum (typed variants), no more from_string for internal paths
  input/  (new)           — decode layer
    mod.rs                — SourceImage, ImageMetadata, decode() entry, format detection
    animation.rs (seed)   — FrameData/AnimationData types (WS5 fills decoders)
  metadata/ (new, seeded) — MetadataBlob placeholder types (WS4 implements)
  converter/
    mod.rs                — orchestration only (pipeline, stats, progress, ctrl+c)
    traits.rs (new)       — ImageEncoder trait + EncoderRegistry
    webp.rs, webp_image.rs, avif.rs, png.rs, mozjpeg.rs  — migrated to the trait
  pipeline.rs (new)       — per-file convert flow (naming, overwrite logic, stats events)
  utils.rs                — cleanup (drop fs::read in is_supported)
```

### 2.2 Core data types (the extension contract)

```rust
// src/input/mod.rs
pub struct SourceImage {
    pub content: ImageContent,
    pub metadata: ImageMetadata,   // empty impl in WS0; WS4 fills
    pub source_format: ImageFormat,
}

pub enum ImageContent {
    Still(DynamicImage),
    Animated(AnimationData),       // defined in input/animation.rs
}

// src/input/animation.rs
pub struct AnimationData {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<FrameData>,   // WS5 may switch to a streaming decoder trait
    pub loop_count: LoopCount,    // reuse image::metadata::LoopCount
}
pub struct FrameData {
    pub buffer: RgbaImage,         // canonical interop format (RGBA8)
    pub delay: std::time::Duration,
}
```

Notes:
- `AnimationData` is seeded by WS0 with construction helpers but **only GIF decode via
  `GifDecoder::into_frames()`** is wired in WS0 (it already works through the same
  `image` dependency) — this proves the plumbing; WS5 completes WebP/APNG/JXL decode
  and all animated encoders.
- Canonical animation buffer is RGBA8; canonical still path stays `DynamicImage` so
  8-bit paths avoid needless copies. Encoders may internally convert.

```rust
// src/metadata/mod.rs (seed)
#[derive(Default, Clone)]
pub struct ImageMetadata {
    pub exif: Option<Vec<u8>>,     // raw TIFF/Exif payload, container-independent
    pub icc: Option<Vec<u8>>,
    pub xmp: Option<Vec<u8>>,
    pub exif_applied_orientation: bool, // whether pixels are already upright
}
```

### 2.3 Encoder abstraction

```rust
// src/converter/traits.rs
pub trait ImageEncoder: Send + Sync {
    fn format(&self) -> ImageFormat;          // target
    fn extension(&self) -> &'static str;
    fn describe(&self) -> String;             // replaces encoder_info() strings
    fn encode(&self, input: &SourceImage) -> Result<Vec<u8>, Error>;
    fn supports_animation(&self) -> bool { false }
    fn adjust_threading(&self, budget: ThreadBudget) {} // see 2.6
}

pub enum EncoderConfig {          // constructed from CLI, Send + Sync, cheap Clone
    Webp(WebpOptions),
    WebpImage,
    Avif(AvifOptions),
    Png(PngOptions),
    Jpeg,
}
```

Each existing `encode_*` fn becomes a struct implementing `ImageEncoder` with an
`Options` struct:

```rust
pub struct WebpOptions { pub lossless: bool, pub quality: f32 }
pub struct AvifOptions { pub quality: f32, pub speed: u8, pub bit_depth: Option<BitDepth>,
                         pub color_model: Option<ColorModel>,
                         pub alpha_color_mode: Option<AlphaColorMode>,
                         pub alpha_quality: f32 }
pub struct PngOptions { pub compression_type: Option<CompressionType>,
                        pub filter_type: Option<FilterType> }
```

Default `ImageEncoder::encode` for animated input = take first frame + emit a one-line
warning (preserves today's GIF behavior until WS5 lands real animated encoders).

### 2.4 Outcome model

```rust
// src/pipeline.rs
pub enum Outcome {
    Encoded { input_size: u64, output_size: u64 },
    SkippedExisting { input_size: u64, existing_size: u64 },
    DiscardedLargerThanInput { input_size: u64, encoded_size: u64 },
    DiscardedLargerThanExisting { input_size: u64, existing_size: u64 },
    Error(String),
    Aborted,
}
```

Replaces `(isize, usize, usize)`; fixes both statistics TODOs. `handle_conversion_error`
( src/converter/mod.rs:74-78) folds into `Outcome::Error` + a single print site.

### 2.5 Input layer (replaces try_read_image / fallback_retry_read_image)

```rust
pub fn load_source(path: &Path) -> Result<SourceImage, Error>
```

Steps:
1. `fs::File::open` + `ImageReader::new(file).with_guessed_format()` — **detect once**,
   keep the reader; remove the double-decode and all `panic::catch_unwind`
   (guess_format already validates before decode; the historic zune panic workaround is
   re-validated by tests; keep `jpeg-decoder` fallback path for pjpeg behind the same
   reader flow if a decode error occurs).
2. On success, collect `decoder.exif_metadata()`, `icc_profile()`, `orientation()`
   (no-ops in WS0, but the calls and plumbing are established).
3. If the concrete decoder implements `AnimationDecoder` **and** target handling is
   desired later, WS5 extends `load_source` with `load_source_animated`.
   In WS0: if detected animated gif → use `into_frames()` to build `AnimationData`
   (proves the type), everything else → `Still`.
4. AVIF remains excluded from inputs in WS0 (see WS1 for the libheif-based re-enable).

### 2.6 Threading budget

```rust
pub struct ThreadBudget { pub files: usize }   // = rayon threads
```

- `ThreadBudget::global()` computed once from `std::thread::available_parallelism()`.
- Encoders get `encoder_threads = max(1, available / files_in_flight)`; wire now for
  ravif (`Encoder::with_num_threads`) and document the field in the trait; oxipng
  (WS3) and jxl (WS2) consume it on arrival.

### 2.7 Naming & collisions

Add to `pipeline.rs`:
```rust
pub enum CollisionPolicy { KeepExisting, OverwriteIfSmaller, OverwriteAlways, Suffix }
```
- Keep current flags mapped onto it (`--overwrite-if-smaller`, `--overwrite-existing`).
- Implement detection of same-stem different-extension inputs mapping to one output:
  default policy for collisions = write first, report subsequent as
  `Outcome::SkippedCollision` (new variant) with a printed notice. Extensible in WS1
  (multi-image HEIF → `_1`, `_2` suffixes).

### 2.8 CLI

- Remove all `crate::converter::...` type paths from src/cli.rs; enums move to
  `src/config.rs` (or stay in encoder modules but are re-exported via `config`).
- Existing subcommands keep their exact flags (back-compat); their constructors build
  `EncoderConfig` variants. `main.rs` shrinks to:

```rust
let conf = ConversionConfig::from_args(&args);   // or TryFrom; explicit constructors,
let enc = EncoderConfig::from_args(&args);       // NOT two `args.into()` (same type
let report = byteshaver::run(conf, enc)?;        // cannot impl Into twice)
```

- `byteshaver::run(conf, enc)` becomes the single library entry point (currently lib users
  would have to call `convert_images` with 11 args).

### 2.9 CI / quality gates (new workflow job)

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- `cargo check --no-default-features` + per-feature checks (features introduced by
  WS1–WS5 must each compile alone).
- Keep `.cargo/config.toml` `target-cpu=x86-64-v3`; note RUSTFLAGS in CI overrides it
  (existing behavior, unchanged).

---

## 3. Implementation steps (in order)

1. Introduce `config.rs`, `pipeline.rs`, `converter/traits.rs`, `input/`, `metadata/`
   with the types above; port `Outcome`.
2. Port each encoder (`webp`, `webp_image`, `avif`, `png`, `mozjpeg`) to `ImageEncoder`
   one commit per encoder; keep their `encoder_info()` as `describe()`.
3. Rewrite `convert_images` → `run()` using the registry; move file discovery, sorting,
   output-dir creation, ctrl+c, progress bar, and stats into `pipeline.rs`.
4. Rewrite `load_source` per 2.5; delete `panic::catch_unwind` paths (validate with a
   regression test over `examples/`).
5. Fix `utils::is_supported` to use header sniffing on the first 512 bytes
   (`image::guess_format` accepts partial buffers? — if not, keep fs::read but only for
   this deprecated helper; mark `#[doc(hidden)]`).
6. Collision handling per 2.7.
7. Tests: unit tests for Outcome mapping, naming, collision; integration test running
   `run()` over `examples/` for each encoder comparing success counts and output sizes
   are within sane bounds; golden progress-statistics output test.
8. Update README "How to Use" code samples only where signatures changed (public API
   `convert_images` removed → document `byteshaver::run`).

## 4. Acceptance criteria

- [ ] `convert_images`/`convert_image` positional-parameter functions are gone.
- [ ] Adding a new encoder requires: one module + one `EncoderConfig` variant + one CLI
      subcommand + one registry line (documented in README of this plan suite).
- [ ] All existing commands behave identically (manual run over `examples/` before/after
      produces the same file set and sizes).
- [ ] Animated GIF inputs flow through `AnimationData` (first-frame encoding preserved).
- [ ] No `panic::catch_unwind` in decode path; `examples/` regression test green.
- [ ] fmt/clippy/test CI job green.

## 5. Out of scope (explicitly)

- Any new format, EXIF logic, oxipng, animation encoders (owned by WS1–WS6).
- Resize/rotate transformations, JSON logs (future).
