# WS6 — Animated AVIF (experimental)

> **SPIKE VERDICT (recorded 2026-09-26, during implementation round 1):**
> **BLOCKED IN THIS ENVIRONMENT — DEFERRED.** The spike requires a libheif ≥ 1.20
> build with an AV1 encoder (libaom/SVT-AV1) to validate the sequence encode path.
> The development environment has no libheif/pkg-config at all, so neither the
> capability probe nor a runtime validation is possible. Per the plan's own gate
> ("spike verdict before any non-FFI code"), no FFI code was written.
> Revisit when a CI job with a full libheif+AV1-encoder stack is available.
> WS5's `--animated-input` behavior already degrades gracefully (first-frame
> encode with notice).

**Depends on:** WS5 merged (AnimationData consumers exist). Independent from WS1's
*decode* work but **reuses the libheif dependency family**; coordinate `Cargo.toml`
features so both can coexist.
**Deliverable (stretch):** `byteshaver "*.gif" avif` producing `avis`-brand animated AVIF.
This is the only workstream without a maintained safe-Rust dependency — everything is
explicitly experimental.

---

## 1. Path analysis (verified)

| # | Path | Verdict |
|---|------|---------|
| 1 | `ravif` 0.13 | ❌ stills only, zero sequence API. |
| 2 | `avif-serialize` 0.8.9 | ❌ can mux still AVIF (incl. alpha/Exif); **no `avis`/image-sequence or multi-tile writing**. |
| 3 | **raw `libheif-sys` 5.3.1 sequence API** (`heif_sequence_encoding_options_alloc`, `heif_context_encode_sequence_*` etc., bound but unwrapped; requires libheif ≥ 1.20 **with an AV1 encoder plugin** — libaom or SVT/rav1e plugin) | ⚠️ only viable path. Heavy unsafe FFI. Recommended *only* behind a cargo feature and only for gnu/docker builds. |
| 4 | rav1e + hand-rolled ISOBMFF `avis` muxing | ❌ rejected: writing a full ISOBMFF sequence muxer (moov/trak/tref/iprp…) is a multi-week project on its own. |

## 2. Feature & build design

```toml
[features]
# mutually independent of dec-heif but shares native libheif presence
anim-avif = ["dep:libheif-sys"]   # dec-heif already brings libheif-rs->libheif-sys
```

- Feature **off by default** for the published crate until validated; enabled in the
  alpine/debian Dockerfiles (their libheif builds include libaom → AV1 encoding).
- Runtime capability probe at encoder construction: attempt
  `heif_context_get_encoder_for_format(HEIF_COMPRESSION_AV1)`; if unavailable →
  `Error::FeatureDisabled("libheif lacks AV1 encoder")` (same UX as WS1 stub mode).
- Windows/musl release binaries: feature off (CI note), stub error like WS1 §6.

## 3. FFI surface (unsafe module `src/converter/avif_anim/ffi.rs`)

Declarations over libheif 1.20+ sequence C API (verify exact signatures against the
libheif 1.23.1 headers that libheif-sys 5.3.1 was generated from):

- `heif_sequence_encoding_options_alloc/free`
- `heif_context_alloc` / `heif_sequence_track_new(ctx, HEIF_COMPRESSION_AV1, content_kind, timescale, options)`
- per sample: `heif_track_add_raw_sample(track, data, size, duration, ...)` — fed with
  AV1 OBUs; **question to resolve first:** does libheif's sequence API accept
  *uncompressed* frames and encode internally (like `heif_encode_image`), or only raw
  codec samples? libheif 1.20 sequences encode via `heif_context_encode_sequence` over
  decoded images (plugin-based) — confirm during implementation spike; if it only
  accepts pre-encoded AV1 samples, evaluate wiring `rav1e` to produce OBU samples
  (adds significant scope; abort criterion below).
- `heif_track_set_timescale/duration`, `heif_context_write_to_file`.

**Abort criterion (spike, max ~2 days):** if the encode path requires pre-encoded AV1
samples AND no plugin-based image-sequence encoding exists in the bundled libheif,
stop and record WS6 as "blocked upstream"; the `--animated-input` behavior from WS5
already degrades gracefully.

## 4. Options exposed (only if spike succeeds)

`AvifAnimOptions { quality: u8 (EncoderQuality::Lossy), speed: u8, timescale: u32,
keyframe_interval_frames: u32, thread_count: u32 }` — mirroring the
`heif_sequence_encoding_options` fields actually available; every option mapped 1:1 in
the CLI (`avif --animated` mutates the existing `avif` subcommand into the sequence
encoder when input is animated, gated by the feature).

## 5. Testing

- Spike-level first: encode a 3-frame synthetic animation, verify output brand is
  `avis` (magic-byte check) and decodes via libheif's own sequence decode (WS1 API —
  convenient round-trip oracle in-tree).
- Timing round-trip vs WS5 fixture delays (±1 tick).
- Explicit unsafe-code review checklist in the PR template: all `CStr` lifetimes,
  every `heif_*_free` paired, error codes checked via `heif_error` struct.

## 6. Risks

- Entirely unsafe FFI with limited upstream documentation → hard spike gate, feature
  default-off, CI only for linux docker/gnu.
- libheif AV1 encoder availability varies by distro build → runtime probe + docs.
- **Fallback behavior (always implemented):** animated input + avif target without
  the feature → WS5's `--animated-input first-frame` notice. Nothing regresses.

## 7. Acceptance criteria

- [ ] Spike verdict recorded in this file (success or "blocked upstream") before any
      non-FFI code is written.
- [ ] If unblocked: `byteshaver "fixtures/*.gif" avif --animated` emits `avis` files that
      round-trip through libheif sequence decode; feature off by default; stubs
      behave per WS1 §6.
