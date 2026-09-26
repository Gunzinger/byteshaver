# WS4 — EXIF handling: strip / keep / filter specific fields

**Depends on:** WS0 merged (metadata plumbing + config). Provides the metadata contract
consumed by WS2 (JXL boxes) and consulted by WS3 (oxipng strip interplay).
**Deliverable:** global `--exif` policy with per-field keep/strip lists; EXIF
extracted from all input formats that carry it; embedded into every target format
that supports it; orientation handled correctly when metadata is dropped.

---

## 1. Semantics & default

```rust
// src/metadata/policy.rs
pub enum ExifPolicy {
    /// Remove all EXIF from outputs. (DEFAULT — privacy-safe; matches today's
    /// effective behavior where metadata is silently lost)
    Strip,
    /// Copy EXIF verbatim into outputs where the container supports it.
    Keep,
    /// Keep all except the listed tags (--exif-except gps*,*orientation style).
    FilterExcept(Vec<TagSelector>),
    /// Keep only the listed tags.
    KeepOnly(Vec<TagSelector>),
}

pub enum TagSelector {
    Name(String),     // e.g. "GPSInfo" (IFD0) or "DateTimeOriginal" (Exif IFD)
    Ifd(In),          // whole IFD: "ifd0", "exif", "gps", "interoperability"
    TagNum(u16, In),  // numeric fallback e.g. 0x8825
}
```

### CLI (global flags on `CliArgs`, WS0 config)

```
--exif <keep|strip|filter>     policy (default: strip)
--exif-except <TAGS>           comma-separated tags to drop when --exif filter
--exif-only <TAGS>             comma-separated tags to keep when --exif filter
--exif-list-tags               print recognized tag names and exit (discoverability)
```

Validation: `--exif-except` and `--exif-only` are mutually exclusive; both require
`--exif filter`. Tag name → `exif::Tag` resolution via a small static table generated
from kamadak-exif's `Tag` constants for the common subset (Orientation, DateTime,
DateTimeOriginal, Make, Model, LensMake, LensModel, GPSInfo + all `gps` IFD names,
Artist, Copyright, ImageDescription, UserComment, XPosition…), plus
`exif::Tag::unknown(tag_num)` numeric fallback and per-IFD wildcards.

## 2. Extraction per input format (contract of `ImageMetadata.exif: Option<Vec<u8>>`)

Payload convention: **raw TIFF byte stream** (the content of a JPEG APP1 after
`b"Exif\0\0"`, the eXIf chunk body, the HEIF Exif item, the JXL Exif box payload minus
its 4-byte offset prefix). All extractors normalize to this.

| Input | Extraction path |
|-------|----------------|
| JPEG | `ImageDecoder::exif_metadata()` (image 0.25 JpegDecoder provides APP1 payload — verify it returns bytes *with or without* the `Exif\0\0` header at implementation time and normalize). Fallback: `kamadak_exif::Reader::read_from_container` directly on the file (handles JPEG APP1 scan itself). |
| PNG | `ImageDecoder::exif_metadata()` (PngDecoder parses eXIf since image 0.25). |
| TIFF | `ImageDecoder::exif_metadata()`. |
| WebP | `ImageDecoder::exif_metadata()` if implemented by image-webp (verify; if `Err`/`None`-always → manual RIFF scan, see below). |
| HEIC/HEIF (WS1) | `handle.metadata_block_ids(&mut ids, b"Exif")` + `handle.metadata(id)` (already wired in WS1 — it fills the same field). |
| JXL (WS2) | `aux_boxes().first_exif()` payload minus `tiff_header_offset()` prefix (already wired in WS2). |
| GIF/BMP/others | none (no EXIF concept). |

**Manual WebP RIFF fallback** (`src/metadata/riff.rs`, ~80 lines, no unsafe): parse
RIFF chunks at top level of the WEBP container; find `'EXIF'` chunk; body is
`b"Exif\0\0"` + TIFF (per WebP container spec) → strip 6-byte prefix. Same helper
later reused for writing (§4.3).

Orientation: capture `ImageDecoder::orientation()` into
`ImageMetadata.exif_applied_orientation` bookkeeping:
- decoders that auto-apply (WS1 libheif, WS2 jxl-oxide): pixels upright → mark true.
- `image` crate decoders: pixels **not** transformed (image 0.25 exposes the tag but
  `ImageReader::decode()` does not rotate) → mark false.
Decision (documented): **when policy results in the Orientation tag being dropped
(Strip/FilterExcept orientation), bake the transform into pixels first** using
`image::imageops::rotate90/180/270` + `flip_*` driven by the 1–8 orientation value.
`--exif keep` with false-bookkeeping → keep tag as-is (no transform, standard
viewer-side rotation). This avoids the classic "stripped photo appears sideways" bug.
Implementation via a shared `fn apply_orientation(img: &DynamicImage, o: Orientation)
-> DynamicImage` in `src/metadata/orientation.rs`, called from the WS0 pipeline right
after load, before encoders.

## 3. Field filtering / re-serialization

`src/metadata/exif.rs` using `kamadak-exif` 0.6.1:

```rust
pub fn parse(payload: &[u8]) -> Result<exif::Exif, Error>;     // Reader::read_from_raw? —
    // kamadak 0.6: Reader::read_raw / read_from_container on a Cursor; pick the
    // raw-bytes entry point; wrap TIFF header handling per payload convention.

pub fn transform(exif: &exif::Exif, policy: &ExifPolicy) -> Result<Vec<u8>, Error>;
```

- `Keep` → fast path: return original `payload` bytes verbatim (`exif.buf()`/input)
  — zero re-serialization risk.
- `FilterExcept`/`KeepOnly` → build with `exif::experimental::Writer`:
  `Writer::new()`, `push_field(&field)` for each surviving `Field`, `set_strips`/
  `set_tiles` only if target is TIFF (not our case), `write(&mut Cursor<Vec<u8>>,
  little_endian)` — endianness of the source TIFF. **Flag**: module is
  `experimental` → wrap failures: on writer error, log a per-file warning and fall
  back to `Keep` semantics (verbatim) — never silently lose all EXIF.
- `Strip` → `None`.
- Stats: count kept/dropped fields; pipeline prints `exif: 41 kept, 3 dropped (gps*)`
  per file at `--verbose` (WS0 logging site), totals in the summary.

## 4. Embedding per target format

`src/metadata/sink.rs` — `fn embed(format, encoded_bytes_or_writer, exif_payload)`.
Where the encoder API forces it, embedding happens **inside** the encoder module
(mozjpeg/png/jxl) calling into `metadata::` helpers — encoders receive
`&ImageMetadata` through `SourceImage` (WS0 contract), apply policy themselves via
`metadata::resolve(policy) -> Option<Vec<u8>>` (already filtered payload).

| Target | Mechanism | Notes |
|--------|-----------|-------|
| **JPEG** (mozjpeg) | In `converter/mozjpeg.rs`: after `start_compress`, `CompressStarted::write_marker(Marker::APP1, b"Exif\0\0" + payload)`. | Verified available on mozjpeg 0.10. Also expose existing ICC write (bonus: pipe `ImageMetadata.icc` → `write_icc_profile`, behind same policy? ICC policy out of scope — Phase 2 flag `--icc keep|strip`). |
| **PNG** (`png` command & oxipng transcode) | Switch `converter/png.rs` from image-crate `PngEncoder` to direct `png::Encoder` (`with_info`) so we can set `Info.exif_metadata = Some(payload)` (written as eXIf chunk) — or post-insert via `Writer::write_chunk(ChunkType(*b"eXIf"), payload)` after `write_header`. | png 0.18.1 supports both (verified). oxipng: `strip` forced to none/safe when policy ≠ Strip (WS3 §4). |
| **oxipng passthrough mode** | No re-embed needed — input eXIf already survives when strip=none; policy=Strip → set `StripChunks::Safe`+`All` mapping (WS3). | |
| **WebP** | `src/metadata/riff.rs` mux writer: rebuild container — read encoder output (VP8/VP8L chunk), add `VP8X` (or extend existing) with EXIF flag, append `'EXIF'` chunk (`Exif\0\0`+payload per spec), rewrite sizes. | Pure safe Rust over simple RIFF rules; validate with `webp-animation::Decoder`/`dwebp -` in tests (fixture check only, no runtime dep). Alternative if unacceptable: `libwebp-sys2` `mux` feature FFI (already a transitive dep via webp-animation after WS5) — prefer the safe pure-Rust writer first. |
| **AVIF** (ravif) | ❌ ravif has no metadata API. Behavior: policy `Strip` → silent; policy `Keep`/`Filter*` → print one-line warning per file ("avif target does not support EXIF embedding; metadata dropped"). | Phase 2 idea (out of scope): route AVIF output through libheif encoder (`ctx.add_exif_metadata`) — noted in README roadmap only. |
| **JXL** (WS2) | `JxlEncoderAddBox(b"Exif", 4-byte-tiff-offset + payload)` + `JxlEncoderUseBoxes`; WS4 provides `metadata::exif_for_jxl()`. | |
| **Animated webp/apng/gif** (WS5) | webp-animation: no metadata API → RIFF-mux the final animation bytes (§4 WebP path works on any WEBP container incl. animated). APNG: `png::Info.exif_metadata` on frame 1 header. GIF: none. | |

## 5. Policy interplay rules (single source of truth)

`metadata::resolve(policy, source_meta) -> Option<Vec<u8>>` is the only function
encoders call. Rules:
1. No source EXIF → `None` regardless of policy (no synthetic EXIF is created).
2. `Strip` → `None` (+ orientation bake-in, §2).
3. `Keep` → verbatim payload (+ keep Orientation tag semantics).
4. `Filter*` → re-serialized payload; on writer failure → verbatim + warning.
5. Target cannot embed + policy wants EXIF → warning (AVIF case) — counted in new
   `Outcome::Encoded` payload as `metadata_dropped: bool` for the summary
   ("N outputs could not carry EXIF").

## 6. Tests

- Fixtures: JPEG with EXIF (orientation 6 + GPS + standard tags), PNG with eXIf,
  HEIC with EXIF (WS1 fixture), WebP with EXIF chunk (create by muxing), JXL with
  Exif box (WS2 fixture).
- Round-trip: `keep` → parse output payload, tag-for-tag equality of values
  (byte-level equality only for Keep-on-JPEG).
- Filter: `--exif filter --exif-except GPSInfo` (plus all GPS IFD names) → GPS gone,
  DateTimeOriginal present, output parses with kamadak (self-consistency) **and**
  `exiftool`-style external validation skipped (no external deps) — validate by
  re-parsing with kamadak on every target's extracted payload (reuse §2 extractors).
- Orientation: strip-policy on an orientation-6 JPEG → output pixels rotated upright,
  no Orientation tag; `keep` → pixels unrotated + tag preserved.
- WebP mux: output parses with image crate (ignores EXIF) + manual chunk scan finds
  EXIF; invalid VP8X rebuilds are caught by tests.
- `--exif-list-tags` golden output test.

## 7. Risks / mitigations

- **kamadak `experimental::Writer` instability** → verbatim fallback rule (§5.4)
  makes worst case = `keep`; consider pinning + a `cargo deny`-style license/advisory
  check later.
- **WebP EXIF chunk placement rules** (must precede image data per spec; readers
  tolerate trailing) → implement spec-compliant order (VP8X first, then EXIF, then
  image payload) with a written test.
- **Orientation double-rotation when WS1/WS2 already applied** → the
  `exif_applied_orientation` flag is authoritative; test across heif→jpeg and
  jxl→jpeg paths.
- **Payload prefix convention bugs** (`Exif\0\0` vs raw TIFF vs JXL offset-prefixed) →
  every boundary crosses `metadata::` normalizers; unit-test each boundary pair
  explicitly.

## 8. Acceptance criteria

- [ ] `--exif strip|keep|filter` implemented across all commands via global flags;
      default `strip`.
- [ ] Extraction works for jpeg/png/tiff/webp (+heif/jxl via WS1/WS2 fixtures).
- [ ] Embedding works for jpeg/png(+oxipng)/webp/jxl; avif warns; animations covered.
- [ ] Orientation bake-in on strip; no sideways outputs.
- [ ] Summary statistics report kept/dropped fields and unembeddable outputs.
- [ ] README: EXIF section documenting policy, tag lists, per-format matrix.
