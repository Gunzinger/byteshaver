# WS1 — HEIC/HEIF Decoding Support (input)

**Depends on:** WS0 merged.
**Deliverable:** `byteshaver "<pattern>" webp` (etc.) transparently decodes `.heic` / `.heif`
(and `.avif`) inputs, including EXIF extraction hooks, multi-image files, and
orientation handling. No HEIF *output* (out of scope).

---

## 1. Support-path analysis

| # | Path | Verdict |
|---|------|---------|
| 1 | **`libheif-rs` 3.0 (MIT) + `libheif-sys` 5.x, system libheif ≥1.17 via pkg-config** | ✅ recommended. Mature, maintained (libheif 1.23.1 era), decode of HEIC (HEVC), HEIF, AVIF (AV1), JPEG-in-HEIF; EXIF/XMP metadata API; sequence decode API. Requires native lib. |
| 2 | `libheif-sys` `embedded-libheif` feature (static libheif 1.23.0 built from source) | ⚠️ Links libheif statically **but codec libs (libde265, libaom/dav1d) are NOT included** — not self-contained; useful only to pin libheif itself. Not a musl solution on its own. |
| 3 | Pure-Rust HEVC decode | ❌ does not exist (no maintained pure-Rust HEVC/AV1 still decoder covers HEVC-in-HEIF). |
| 4 | `ffmpeg-next` / CLI ffmpeg | ❌ massive dep, licensing ambiguity, shell-out UX; rejected. |
| 5 | image crate `avif-native` (dav1d) for AVIF input only | ⚠️ cargo comment says "problematic on windows"; superseded by path 1 which also unlocks HEIC. |

### Portability reality (drives the feature flag design)

- **Linux gnu / debian docker:** `apt install libheif-dev` (+ `libde265-dev`, `libaom-dev`
  as codec providers). pkg-config discovery via `system-deps` (already a transitive
  build-dep of libheif-sys).
- **Alpine docker:** `apk add libheif-dev libde265-dev libaom-dev` in builder stage;
  runtime stage needs the shared libs (`libheif`, `libde265`, `libaom`) — switch runtime
  stage from `alpine:latest` plain to one with those packages, or accept larger image.
- **musl static (release binaries):** libheif+libde265+libaom fully static is possible
  (all three build with meson/cmake) but requires custom build orchestration in CI.
  **Phase 1:** musl release binaries are built **without** the `dec-heif` feature and
  print a clear runtime message (see §6 Stub mode). **Phase 2 (optional):** CI job that
  vendors static libheif stack into the musl build.
- **windows-gnu release binaries:** libheif only via vcpkg (`vcpkg` crate path in
  libheif-sys) — not wired for our mingw cross job. **Phase 1:** stubbed like musl.

**Decision recorded in README:** HEIF enabled for gnu/docker, stubbed elsewhere until
Phase 2. This matches how `ravif` was treated historically (nasm requirement).

---

## 2. Cargo changes

```toml
[features]
default = ["dec-heif"]
dec-heif = ["dep:libheif-rs"]

[dependencies]
# native-codec input decoders (alphabetical grouping, keep tidy)
libheif-rs = { version = "3.0", optional = true, default-features = false, features = ["v1_20"] }
```

- Pin `v1_20` (not `latest`/`v1_23`) as the minimum for wider distro compat; libheif
  1.20 already covers sequence decode. Distros with newer libheif are forward-compatible
  (features are minimum-version gates).
- `default-features = false` to avoid pulling its `image` integration feature (we wire
  decoding manually, no global hooks).

## 3. Module design

New file `src/input/heif.rs` (all `#[cfg(feature = "dec-heif")]`):

```rust
pub struct HeifDecodedImage {
    pub image: DynamicImage,
    pub exif: Option<Vec<u8>>,   // raw Exif payload (TIFF), feeds ImageMetadata
    pub xmp: Option<Vec<u8>>,
    pub icc: Option<Vec<u8>>,    // color_profile_raw() bytes
    pub is_primary: bool,
    pub index_in_file: usize,
    pub total_images: usize,
}

/// Decode one image item from an open context.
pub fn decode_handle(ctx: &HeifContext, handle: &ImageHandle) -> Result<HeifDecodedImage, Error>;

/// Enumerate decodable image handles (primary first, then others).
pub fn image_handles(ctx: &HeifContext) -> Result<Vec<ImageHandle>, Error>;

/// Quick header check used for counting / multi-image policy before full decode.
pub fn probe(path: &Path) -> Result<HeifFileInfo, Error>;
```

Implementation notes (verified against libheif-rs 3.0 API):
- Open: `HeifContext::read_from_file(path)` (files are on disk already; avoids the
  custom `Reader` trait path).
- Handles: `ctx.primary_image_handle()` for primary; enumeration via
  `ctx.top_level_image_handles()` (3.0 API; do **not** use the removed
  `image_handles()`).
- Decode: `LibHeif::new().decode(&handle, ColorSpace::Rgb(RgbChroma::InterleavedRgba or
  InterleavedRgb), None)` — choose RGBA vs RGB from `handle.has_alpha_channel()`.
  There is **no** `convert_to_rgba8`; requesting chroma at decode time is correct.
- Pixels: `image.planes().interleaved` → `Plane { data, stride, width, height }`;
  copy row-by-row into `image::RgbaImage`/`RgbImage` respecting stride (do **not**
  assume packed rows).
- HDR (>8-bit): check `handle.luma_bits_per_pixel()`. Phase 1: decode 8-bit RGB(A)
  and warn when source is 10/12-bit ("down-converting to 8-bit"). Phase 2 (optional):
  request `InterleavedRgba(16)`-style chroma (`RRGGBBAA_LE`) → `DynamicImage::ImageRgba16`
  for targets that support 16-bit (avif/jxl/png).
- EXIF/XMP: `handle.metadata_block_ids(&mut ids, b"Exif")` + `handle.metadata(id)`
  (same for `b"mime"`/XMP via `metadata_content_type() == "application/rdf+xml"`).
- ICC: `handle.color_profile_raw()` (rICC/prof) or nclx → keep raw bytes; nclx-only
  images: leave `icc = None` (no synthetic profile in Phase 1).
- Orientation: libheif **applies irot/imir during decode** and `handle.width()/height()`
  are post-transform — so decoded pixels are upright; set
  `ImageMetadata.exif_applied_orientation = true` and (with WS4 contract) drop the
  Orientation tag when re-embedding EXIF to avoid double rotation.
- Thumbnails / auxiliary images / depth maps: ignore (decode master images only).

## 4. Integration into input pipeline (WS0 contract)

1. `src/format.rs`: add `ImageFormat::Heif` (input-only) with extensions
   `"heif"`, `"heic"`, `"hif"`, `"avif"` mapping. Keep `extension()` returning `"heif"`
   (never a target). Update the `From<&Path>` path untouched (uses `from_extension`).
2. In `input::load_source` format dispatch: for `Heif`, route to `input/heif.rs`
   instead of `ImageReader`. **Remove the blanket AVIF exclusion**
   (src/converter/mod.rs:110-114 FIXME) — AVIF inputs now flow through libheif.
3. Multi-image policy — new global flag:

```rust
/// How to treat HEIF files containing more than one image. (default: primary)
#[clap(long, global = true, value_enum)]
pub heif_image_policy: Option<HeifImagePolicy>,   // Primary | All
```

   `All` expands a file into N outputs named `stem.ext`, `stem_1.ext`, … using the
   WS0 collision/suffix machinery. File-count statistics must count *images*, not files
   (progress bar length = expanded image count; WS0's producer thread feeds PathBuf —
   change queue item to `enum WorkItem { Single(PathBuf), HeifMulti { path, index } }`).
4. `discard_input_alpha_channel` and the huge-image warning logic must work for
   `SourceImage` coming from the HEIF path (they operate on `DynamicImage` — free).
5. ctrl+c / rayon: unchanged; libheif decode is thread-safe per `LibHeif` instance —
   create one `LibHeif` per decode call (cheap) to avoid shared-state questions.

## 5. Docker / CI changes

- `Dockerfile` (alpine): builder adds `libheif-dev libde265-dev libaom-dev`; runtime
  stage adds `libheif libde265 libaom` (apk runtime packages).
- `Dockerfile-debian` (trixie): builder adds `libheif-dev libde265-dev libaom-dev`;
  runtime `libheif1 libde265-1 libaom3` + existing libc6.
- workflow.yaml: musl + windows-gnu builds get `--no-default-features` +
  `--features "dec-heif"` omitted → add a build step comment + `byteshaver` stub behavior
  (§6). Add an apt `libheif-dev` install for the linux-gnu test job.
- Smoke test in CI (gnu): `byteshaver "tests/fixtures/**/*.heic" webp` counts success.

## 6. Stub mode (feature disabled builds)

WS0's `input::load_source` for `Heif` when `dec-heif` is off returns
`Error::FeatureDisabled("HEIC/HEIF input requires a build with the dec-heif feature")`.
Files are counted and reported per-file as `Outcome::Error` (batch continues).
`--help` stays identical across builds.

## 7. Test plan

- Fixtures (add to `tests/fixtures/heif/`, keep each < 300 KiB): 1× HEIC 8-bit RGB,
  1× HEIC with alpha, 1× HEIC with EXIF+orientation (irot 90°), 1× HEIF burst-like
  file with 2 master images, 1× AVIF still. Generate with `heif-enc` in a scratch env
  once; commit binaries (license: self-created content).
- Unit: `probe` counts; EXIF bytes non-empty; orientation pixels upright (spot-check
  corner pixel of a synthetic gradient).
- Integration: `byteshaver fixtures/heic/*.heic webp` → N files, decodable by `image` crate;
  same for `avif` target; `--heif-image-policy all` produces stem suffixed outputs.
- Negative: corrupt .heic → `Outcome::Error`, batch continues.

## 8. Risks / mitigations

- **libheif codec availability per distro** → CI matrix runs the gnu test job on
  ubuntu-latest with explicit dev packages; document alpine runtime package set.
- **zune-style panics in native code** → wrap `decode_handle` in
  `panic::catch_unwind` **only at the dispatch boundary** (C FFI can abort; keep the
  catch so one bad file doesn't kill the batch), converting to `Outcome::Error`.
- **Binary size** (musl stubs avoid it for now; docker images grow ~10–20 MB).
- **10/12-bit down-conversion** is lossy → explicit printed warning; Phase 2 ticket
  left in plan suite.

## 9. Acceptance criteria

- [ ] `byteshaver "**/*.heic" avif` converts on linux-gnu/docker; EXIF reaches WS4 hooks.
- [ ] AVIF input exclusion removed; `byteshaver "**/*.avif" webp` works.
- [ ] `--heif-image-policy all` expands multi-image files with suffix naming.
- [ ] musl/windows-gnu binaries build, run, and report `FeatureDisabled` for HEIF.
- [ ] No regression in existing formats (WS0 regression suite stays green).
