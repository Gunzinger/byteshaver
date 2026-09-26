# WS5 — Animation re-encoding (gif → webp / apng / avif / jxl targets)

**Depends on:** WS0 (AnimationData plumbing), WS4 contract (embedded metadata in
animated containers), WS2 only for animated-JXL **target** (independent sub-phase).
**Deliverable:** animated inputs (gif, animated webp, APNG, animated jxl) decode with
frame timing preserved and re-encode to animated webp, APNG, animated gif (bonus),
and animated jxl (via WS2 Option A); animated AVIF is split into WS6.

---

## 1. Ecosystem analysis (verified)

### Decode (animated sources)

| Format | Crate/API | Facts |
|--------|-----------|-------|
| GIF | `image` 0.25 `GifDecoder` + `AnimationDecoder::into_frames()` / `loop_count()` (loop_count new in 0.25.10) | already used by WS0 seed; frames come pre-coalesced as RGBA8 `Frame`s with `Delay`; disposal handled by decoder. ⚠️ validate partial-frame/disposal behavior on tricky fixtures. |
| WebP | `image` 0.25 `WebPDecoder` (image-webp 0.2.4): `is_animated()`, `num_frames()`, `loop_count()`, `read_frame()` → ms duration, `reset_animation()` | pure Rust; wrapped as `AnimationDecoder` in image 0.25. |
| APNG | `image` 0.25 `png::ApngDecoder` (adapter over `PngDecoder`, since 0.23.6) | implements `AnimationDecoder` incl. `loop_count`; **not** auto-selected by `ImageReader` → must wrap explicitly. |
| JXL | WS2 `input/jxl.rs` frame loop (`render_frame(i)`, `Render::duration()`) | durations as ticks; loop count from header. |

### Encode (animated targets)

| Format | Crate | Facts |
|--------|-------|-------|
| **WebP** | **`webp-animation` 0.10.0** (MIT OR Apache-2.0) over `libwebp-sys2` 0.2 (BSD-3, bundles libwebp; `std,0_5,0_6,1_2,demux,mux` features) | `Encoder::new((w,h))`, `add_frame(&[u8] rgba, timestamp_ms: i32)`, `finalize(ts) -> Vec<u8>`; `EncoderOptions{color_mode, encoding_config, anim_params, kmin, kmax, minimize_size, allow_mixed, verbose}`; **lossless default**; quality via `encoding_config` (libwebp `WebPConfig`). Loop count: webp-animation has no API for the ANIM loop-count field → Phase 1 outputs infinite-loop (matches most source gifs); Phase 2: post-mux the ANIM chunk flags via WS4's RIFF helper. |
| **APNG** | **`png` crate 0.18.1 directly** (already in tree via image) — `Encoder::set_animated(num_frames, num_plays)`, `set_frame_delay(num, den)`, `set_dispose_op`, `set_blend_op`, `set_sep_def_img`, per-frame `Writer::set_frame_delay` | avoids the stale `apng` 0.3.4 crate (MIT-only, unmaintained since 2024, pins image 0.24 — rejected). `num_plays` = loop count (0 = infinite). RGBA8, 16 ms-granular delays via fraction (delay_den=1000). |
| **GIF** (bonus target) | `image::codecs::gif::GifEncoder` (`encode_frame`, `set_repeat`) | trivial to add; frames must be palette-quantized (encoder does it); delay in 10 ms units (rounding lossy — document). |
| **AVIF** | **no safe maintained path** (`ravif`: stills only; `avif-serialize`: no `avis` muxing) | deferred to WS6 (raw libheif-sys sequence API). |
| **JXL** | WS2 Option A `JxlEncoderSetFrameDuration` | animated encode comes free with WS2 Option A; WS5 only guarantees `AnimationData` flows in. Under WS2 Option B: reject with clear error. |

### Crate-graph caution
`webp-animation` → `libwebp-sys2` bundles a **second copy of libwebp** (the `webp`
crate already links `libwebp-sys` 0.9's bundled copy). Acceptable in Phase 1 (both
static, name-collisions avoided by distinct crate symbol scoping); note a Phase-2
consolidation opportunity: port still-webp encode to `libwebp-sys2` and drop the
`webp` crate (~binary size win, single libwebp).

## 2. Detection & decode flow (extends WS0 `input::load_source`)

1. After format detection, if format ∈ {Gif, Webp, Png, Jxl}:
   - **Gif/JXL:** decode as animation directly (all gifs are potentially animated;
     single-frame gifs → 1-frame `AnimationData`? No — single-frame must decode as
     `Still` to keep still-paths fast: check `frame count > 1` via cheap probe first:
     gif = `Decoder`... `next_frame_info()` twice is awkward; simpler: try
     `into_frames()`, if `Frames` yields 1 frame → treat as Still).
   - **WebP:** `WebPDecoder::is_animated()` probe → animated path or normal decode.
   - **PNG:** try `ApngDecoder` only when the file contains `acTL` (cheap signature
     scan of first 512 KiB or full header walk via `png` crate info) → else Still.
2. Decode **streaming-aware**: `AnimationData.frames` is a `Vec` in Phase 1 with a
   hard guard: `--max-animation-memory <MiB>` (default 4096). When a file's projected
   footprint (w×h×4×frames) exceeds the cap → `Outcome::Error` with a clear message
   (encoders would OOM otherwise; progress bar shows skip). Phase 2 (optional):
   streaming `Frames` iterator into webp-animation's `add_frame` (only webp target
   benefits; APNG/png writer needs seekable output — keep Vec there).
3. `AnimationData.loop_count` normalization: `image::metadata::LoopCount::{Infinite,
   Finite(n)}`; gif `Repeat`, webp `loop_count()`, apng `num_plays`, jxl `num_loops`
   (0→Infinite).

## 3. Animated encoders

### 3.1 WebP target (`converter/webp_anim.rs`, feature `anim-webp`)

```rust
pub struct WebpAnimOptions {
    pub quality: f32,        // 0-100, default 90; ignored when lossless
    pub lossless: bool,      // default false in CLI (webp-animation lib default differs)
    pub kmin: Option<i32>,   // min keyframe distance (0 = auto)
    pub kmax: Option<i32>,   // max keyframe distance (0 = auto)
    pub minimize_size: bool, // libwebp min-size mode
    pub allow_mixed: bool,   // allow mixed lossy/lossless frames
    pub method: Option<u8>,  // WebPConfig method 0-6 (speed/effort)
}
```

- Timestamps: cumulative `delay` sums (ms, i32; clamp total ≥ i32::MAX with error).
- `add_frame(rgba_bytes, ts)`; `finalize(last_ts)`.
- Pixel format: RGBA always (lossy webp handles alpha internally).
- Loop count: Phase 1 infinite; Phase 2 ANIM-chunk patch via `metadata/riff.rs`.
- CLI: `WebpAnim` subcommand (kept **separate from** existing `webp` still command —
  no surprise behavior changes; also expose `--animated` on `webp` later if desired).

### 3.2 APNG target (`converter/apng.rs`, feature `anim-apng`)

- Direct `png` 0.18 usage: `Encoder::with_info` / `new` + `set_color(Rgba8)` +
  `set_depth(Eight)` + `set_animated(n, num_plays)` + `set_sep_def_img(true)`
  (keep default image visible in non-APNG viewers).
- Per frame: `Writer::set_frame_delay(d_num, 1000)` (ms → 1/1000 s), dispose `None`,
  blend `Source`, `write_image_data(&frame)`.
- Reuse `converter::png` compression knobs (add `--compression-type/--filter-type`
  passthrough to the new command; default `Compression::Fast` since oxipng (WS3) can
  post-optimize APNGs losslessly — document that combo).
- Output extension: `.png` (APNG *is* PNG). Collision with WS0 naming is fine.
- `Config`: `num_plays` from `LoopCount` (`Finite(n)` → n, Infinite → 0).

### 3.3 GIF target (bonus, feature-gated off by default? — include, it's ~50 lines)

`GifEncoder::new_with_speed(w, set_repeat(Repeat mapping))`, `encode_frame(rgba, delay)`
per frame. Delay rounding ms→10 ms documented in `--help`.

### 3.4 JXL target

Delegated to WS2 (`EncoderConfig::Jxl` + `supports_animation()`); WS5's responsibility
is only that animated decode produces `AnimationData` for all four source formats.

### 3.5 AVIF target (without WS6)

When input is animated and target lacks animation support (`EncoderConfig::{Avif,
Webp(still), WebpImage, Png, Jpeg, Oxipng}`): current WS0 default = first frame + warn.
Upgrade: make it an explicit choice — new global flag:

```
--animated-input <first-frame|error>   default: first-frame (prints notice)
```
(`error` mode for pipelines that must never silently drop animation.)

## 4. Timing/rounding rules (canonical: `Duration` ms)

| From → To | Rule |
|-----------|------|
| gif (10 ms u16) → internal | `Duration::from_millis(delay*10)` |
| webp ms → internal | as-is |
| apng (num/den s) → internal | rational → ms, rounded |
| jxl ticks → internal | ticks × (1000 / timescale), rounded (exact scaling verified against header fields in WS2) |
| internal → webp | cumulative ms i32 |
| internal → apng | num=ms, den=1000 |
| internal → gif | `(ms / 10).max(1)` (0 is invalid/"asap" — clamp) |

## 5. CLI additions

```rust
/// Convert images to animated webp format (webp-animation)
WebpAnim { /* §3.1 fields */ }
/// Convert images to animated png format (APNG via png crate)
Apng { /* compression/filter passthrough + --num-plays override */ }
/// (bonus) GIF target
Gif { /* --speed */ }
```
Plus global: `--animated-input`, `--max-animation-memory`.

`src/format.rs`: `ImageFormat` gains **target** variants `WebpAnim("webp")`,
`Apng("png")`, `Gif` (input already exists). Input filter list unchanged (gif/webp/png
already allowed inputs).

## 6. Tests

- Fixtures (self-created, committed): 2-frame gif, 24-frame gradient gif (300 ms/frame),
  gif with per-frame varying delays, animated webp, APNG with 3 delays, 1-frame gif
  (must stay Still).
- Round-trip matrix: each animated source × each animated target → frame count equal,
  per-frame delay within documented rounding (±10 ms webp/gif chains, ±1 ms apng),
  loop count preserved (except webp target Phase 1 = Infinite — assert + document),
  pixels: first frame near-identical (lossy targets get SSIM-ish threshold check or
  plain size sanity), later frames verified for count/timing only on lossless chains
  (gif→apng, gif→webp-lossless).
- Memory guard: fixture with 200 frames → cap exceeded → `Outcome::Error` message.
- `--animated-input error` → animated input + still target errors.
- Oversubscription sanity: convert 8 animated files in parallel; assert completion
  (webp-animation/libwebp is internally threaded — rely on WS0 budget; libwebp anim
  encoder is single-threaded per file by default → no extra wiring needed, note it).

## 7. Risks / mitigations

- **image-crate gif coalescing bugs on exotic disposal** → keep the decoder-owned
  coalescing, add a "disposal torture" fixture; fallback: switch GIF decode to the
  `gif` 0.14 crate directly (same family, more control incl. dispose/needs_user_input)
  — decision deferred to implementation if fixture fails.
- **i32 ms timestamp overflow** for very long animations → guard with clear error.
- **webp-animation API gaps (loop count)** → documented Phase 2 RIFF patch; output
  remains spec-valid meanwhile.
- **Duplicate libwebp in binary** → accepted Phase 1; consolidation note (§1).
- **APNG viewers** → `set_sep_def_img(true)` keeps a static default image for
  non-APNG-aware consumers.

## 8. Acceptance criteria

- [ ] gif/webp/apng/jxl animated inputs decode with frames+timing+loop count.
- [ ] Animated webp, APNG, gif targets produce valid animations (validated by
      decoding outputs back with image crate / webp-animation Decoder).
- [ ] Memory guard + `--animated-input` behavior implemented and tested.
- [ ] Still inputs to animated targets → single-frame animation output (documented,
      tested) — deliberate: WebpAnim with one frame writes a valid 1-frame webp.
- [ ] README updated: animated matrix (which sources × which targets), rounding notes.
