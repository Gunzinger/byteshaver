# WS3 — Optimal PNG compression via oxipng

**Depends on:** WS0 merged. No other workstream.
**Deliverable:** new `byteshaver <pattern> oxipng` target producing optimally compressed,
bit-exact PNGs; two distinct modes (re-optimize existing PNG bytes in place, and
re-encode from any input format then optimize).

---

## 1. Support-path analysis

| # | Path | Verdict |
|---|------|---------|
| 1 | **`oxipng` 10.2.1 crate as library (MIT)** | ✅ recommended. Pure-Rust+libdeflater (cc-built C, musl/mingw fine), `optimize_from_memory`, preset levels, zopfli optional. |
| 2 | image crate `CompressionType::Best` | ❌ already exposed by the `png` command; far weaker than oxipng (no palette/bit-depth reduction, no filter search, no zopfli). |
| 3 | shelling out to `oxipng` binary | ❌ external dep, no. |

**Key architectural insight (the optimization this plan is built around):**
oxipng operates on **encoded PNG bytes**, not pixels. Therefore:

- **Mode A (passthrough, PNG→PNG):** input is already PNG → feed the **original file
  bytes** straight into `oxipng::optimize_from_memory`. Pixels stay bit-exact, palette
  (PLTE), tRNS, and 16-bit data are preserved and reduced losslessly. Never
  decode/re-encode — this both preserves quality and is much faster than the
  decode→encode→optimize pipeline.
- **Mode B (transcode, non-PNG→PNG):** decode via the normal pipeline → encode a
  baseline PNG with the `png` crate at low effort → optimize those bytes.
  (Rationale: encoding with the `png` crate's `Compression::Fast` first and letting
  oxipng do the real compression is faster than encoding twice well; oxipng re-encodes
  the IDAT stream regardless when `idat_recoding` is on, which it is by default.)

## 2. Cargo changes

```toml
[features]
default = [..., "opt-oxipng"]
opt-oxipng = ["dep:oxipng"]

[dependencies]
oxipng = { version = "10.2", optional = true, default-features = false, features = ["zopfli"] }
```

**Deliberately excluded default features** (important, per WS0 §2.6 thread budget):
- `binary` — would pull clap/glob/env_logger duplicates.
- `parallel` — oxipng's internal rayon pool would multiply with byteshaver's per-file rayon
  parallelism (oversubscription). byteshaver parallelizes across files; oxipng stays
  single-threaded per file. (A `--oxipng-parallel` escape hatch can enable the feature
  later; not in Phase 1.)

## 3. CLI

```rust
/// Convert images to optimally compressed png format (using oxipng)
Oxipng {
    /// Optimization level 0-6 (preset), or `max`. Default 2. Higher = slower.
    #[clap(short, long, default_value = None, value_name = "LEVEL")]
    level: Option<OxipngLevel>,            // ValueEnum: Zero..Six, Max

    /// Use zopfli DEFLATE instead of libdeflate (much slower, slightly smaller).
    #[clap(long)]
    zopfli: bool,
    /// zopfli iteration count when --zopfli is set. Default 15.
    #[clap(long)]
    zopfli_iterations: Option<u32>,

    /// Interlacing handling: keep | none | adam7. Default: none (oxipng default removes it).
    #[clap(long, value_enum)]
    interlace: Option<OxipngInterlace>,

    /// Metadata stripping: none | safe | all. Default: none
    /// (WS4 contract: forced to `safe` or `none` when EXIF keep/filter policy is active).
    #[clap(long, value_enum)]
    strip: Option<OxipngStrip>,

    /// Filter strategies to try (repeatable): none sub up avg paeth entropy bigrams bruns
    #[clap(long, value_enum, value_delimiter = ',')]
    filters: Vec<OxipngFilter>,

    /// Allow altering transparent pixel values for better compression (lossless visually).
    #[clap(long)]
    optimize_alpha: bool,

    /// Disable lossless reductions: repeatable  bit-depth color-type palette grayscale
    #[clap(long, value_delimiter = ',')]
    no_reduction: Vec<OxipngReduction>,

    /// Force 16-bit → 8-bit scaling when losslessly possible.
    #[clap(long)]
    scale_16: bool,

    /// Attempt fixing broken input PNGs instead of erroring.
    #[clap(long)]
    fix_errors: bool,

    /// Stop optimizing a file after this duration (e.g. 30s, 2m). Default: unlimited.
    #[clap(long, value_name = "DURATION")]
    timeout: Option<humantime-ish parsed Duration>,   // implement tiny parser, no new dep

    /// Keep interlace state instead of changing it (equivalent to interlace keep).
    #[clap(long)]  // convenience, may fold into --interlace
    keep_interlace: bool,
}
```

(If `humantime`-style parsing feels heavy: accept plain seconds `--timeout-secs u64`.)

### Defaults rationale

`level` maps to `oxipng::Options::from_preset(n)`; default **2** matches oxipng's own
sane default (fast, good ratio); `max` → `Options::max_compression()`. All level presets
can be overridden by explicit flags (explicit flags are applied on top of the preset —
document that order).

## 4. Module design

`src/converter/oxipng.rs`:

```rust
pub struct OxipngOptions { /* mirror of CLI, defaults resolved */ }
impl OxipngOptions {
    pub fn to_oxipng(&self) -> oxipng::Options { ... }   // preset + explicit overrides
}

pub enum OxipngMode {
    /// input bytes are already PNG → pure re-optimization
    Passthrough,
    /// pixels decoded from another format → baseline-encode then optimize
    Transcode(DynamicImage),
}

pub fn optimize_bytes(png_bytes: &[u8], opts: &OxipngOptions) -> Result<Vec<u8>, Error>;
```

`ImageEncoder` implementation for the target (`EncoderConfig::Oxipng(OxipngOptions)`):
- `extension() = "png"`.
- `encode(&SourceImage)`:
  - if `source_format == Png` and `content == Still` and EXIF policy won't require
    re-embedding → **Passthrough**: `fs::read` the input file (already read once by
    pipeline? — ensure single read, reuse a `OnceCell`/passed-through byte buffer in
    `SourceImage` if WS0 kept one; otherwise a second read is acceptable, note it).
  - else → **Transcode**: baseline-encode via existing `converter::png::encode_png`
    with `Compression::Fast` + `Filter::Adaptive` → `optimize_bytes`.
- `supports_animation() = false` in Phase 1 (animated PNG passthrough: oxipng 10.x
  **does** support APNG-preserving optimization — enable for animated inputs behind a
  test; if it fails on fixtures, restrict to stills and document).
- `describe()` prints oxipng version (DEPENDENCIES table picks it up) + resolved
  options.

Wire `strip` interplay with WS4: `OxipngOptions::to_oxipng` consults the active
`ExifPolicy` (WS0 config):
- `keep`/`filter` → force `StripChunks::None` (eXIf must survive) and perform EXIF
  embedding **before** optimization (via WS4's png-eXIf helper) so oxipng preserves it
  (oxipng keeps unknown/ancillary chunks when strip=none; eXIf is ancillary → safe).
- `strip` → honor user's `--strip` (default none still keeps eXIf! → when policy is
  `strip`, default `--strip safe` instead so behavior matches policy).

## 5. Integration points (WS0 contract)

1. `src/format.rs`: no new `ImageFormat` variant needed — target is PNG. But
   `EncoderConfig::Oxipng` must map to extension `"png"`. Output-name collision with
   an input `.png` in the same directory: covered by WS0 collision policy
   (`overwrite-if-smaller` composes perfectly with passthrough mode: if oxipng can't
   beat the existing file, nothing is written).
2. CLI subcommand `Oxipng {...}` added after `Png {...}` in `Command` enum.
3. main.rs: one match arm building `EncoderConfig::Oxipng(OxipngOptions{..})`.
4. Registry: `converter/mod.rs` one entry.
5. Stats: passthrough mode's "input size" = original PNG bytes, "output size" =
   optimized bytes → the existing compression-ratio stats now show true savings for
   PNG→PNG optimization (nice property, verify in tests).

## 6. Threading & memory

- Per-file single-threaded (feature choice above). `optimize_from_memory` returns the
  new bytes in memory (no file I/O races) — matches WS0 pipeline write model.
- zopfli on huge images is extremely slow → respect `--timeout`
  (`Options.timeout: Option<Duration>`), and the huge-image warning (WS0, 8192 px)
  should additionally suggest `--level` ≤ 2 or `--timeout` for oxipng target.

## 7. Tests

- Fixtures: `examples/png` already has PNGs; add one 16-bit PNG, one palette PNG with
  tRNS, one interlaced PNG, one PNG with EXIF eXIf chunk (for WS4 interplay), one
  APNG.
- Property tests:
  - Passthrough bit-exactness: decode(optimized) pixels == decode(original) pixels
    (for default options without `--optimize-alpha`).
  - Monotonic size: `size(oxipng out) <= size(min(image-crate Best, original))` on the
    fixture set.
  - `--level max` never errors on fixtures; respects `--timeout` (simulate with tiny
    timeout + big fixture).
- WS4 interplay test: `--exif keep` + oxipng → eXIf chunk present in output
  (may be a stub until WS4 lands; gate with `#[cfg(feature = "exif")]`-equivalent).
- Determinism: same input + options → byte-identical output (oxipng is deterministic).

## 8. Risks / mitigations

- **libdeflater C build on windows-gnu** → it builds via `cc`; verify early with a CI
  `cargo check --target x86_64-pc-windows-gnu --features opt-oxipng`; fallback =
  stub mode like WS1/WS2 (unlikely to be needed).
- **APNG passthrough edge cases** → keep behind explicit test; if oxipng mangles an
  APNG fixture, restrict to stills and route animated PNGs to WS5's apng encoder.
- **User confusion png vs oxipng** → README section comparing `png` (fast,
  image-crate) vs `oxipng` (slow, optimal); `describe()` output shows the difference.

## 9. Acceptance criteria

- [ ] `byteshaver "examples/**/*.png" oxipng` shrinks or keeps every file (never grows, given
      WS0 overwrite semantics) and outputs remain bit-exact decodes.
- [ ] `byteshaver "examples/**/*.jpg" oxipng` transcodes jpg → optimized png.
- [ ] Full flag surface mapped to `oxipng::Options` incl. zopfli, filters, strip,
      reductions, timeout.
- [ ] No oxipng-internal parallelism (feature set verified by `cargo tree` check in CI).
- [ ] README documents `oxipng` command with defaults table.
