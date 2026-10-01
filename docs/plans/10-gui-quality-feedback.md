# 10 — GUI: compression feedback, quality metrics & visual difference inspector

**Size:** M (phase 1 S, phases 2–3 M) · **Depends on:** 11 for the core
`Outcome::Encoded { output_path }` amendment (phase 3 context-menu
integration only; phases 1–2 fully independent) · **Parallelizable with:**
all others (owns new `gui/src/metrics*` + `gui/src/panels/inspector.rs`;
edits shared files only in small, localized spots named below).

## Goal

After a conversion the GUI currently reports sizes only. This plan adds:

1. **Compression-ratio feedback** everywhere results are shown, with a
   subtle color hint (footer, queue rows, report window).
2. **Perceptual quality measurement** of the conversion (DSSIM and/or
   other sensible methods) — manual and optional automatic — presented
   per file and aggregated.
3. A **visual difference inspector** opened from a right-click menu on
   converted items (feasibility is assessed in §5: **GO**, with explicit
   memory guardrails).

Design doctrine stays: everything headless-testable lives outside the
rendering closures; the egui layer is thin.

## Phase 1 — compression ratio + color hint (S)

### Where ratios come from

`QueueItem { input_size, output_size }` (`gui/src/queue.rs:208-210`) and
`report.totals` already carry everything. Ratio definition (match the CLI
summary): `output_size / input_size * 100 %`; savings = `100 − ratio`.
`Outcome::DiscardedLargerThanInput` counts as "grew" (it was discarded).

### Pure helpers (`queue.rs` or new `gui/src/format_util.rs`)

```rust
pub fn ratio_percent(input: u64, output: u64) -> Option<f32>;        // None if input==0
pub fn ratio_label(input: u64, output: u64) -> String;               // "62%" / "—"
pub fn ratio_hint(input: u64, output: u64) -> Hint;                  // Good | Neutral | Grew
pub fn hint_color(hint: Hint, ui: &egui::Ui) -> egui::Color32;      // rendering stays in panels
```

Hint bands (decision point, defaults proposed):

| band | ratio | color (subtle!) |
|------|-------|-----------------|
| Good | ≤ 80 % | green tint `from_rgb(90,190,110)` at low alpha |
| Neutral | 80–100 % | no tint |
| Grew | > 100 % | amber/red tint `error_fg_color` at low alpha |

### Rendering

- **Queue rows** (`panels/file_table.rs`, superseded by plan 11's table but
  independently shippable): append the ratio to the size column text
  (`4.20MiB → 1.30MiB (31%)`) and tint the cell background with
  `ui.painter().rect_filled(row_rect, 2.0, hint_color.with_alpha(10))` —
  a *slight* hint, never a full row fill. The text carries the information;
  color is redundancy (accessibility: never color-only).
- **Footer** (`panels/footer.rs:80-94` `stats_line`): append
  `(62% · saved 89.1MiB)` after the existing `→` sizes.
- **Report window totals** (`panels/report.rs:180-190` already prints a
  percentage; unify it with `ratio_label` so all three surfaces show
  identical strings).

Unit tests: band boundaries (79.9/80/100/100.1), zero-size input, label
formatting parity between footer and report.

## Phase 2 — quality metrics (M)

### Metric candidates (verify versions/licences at implementation time; as of 2026-10)

| Metric | Crate | License | Notes |
|--------|-------|---------|-------|
| **DSSIM** (multi-scale SSIM) | `dssim` (kornelski) | MIT OR Apache-2.0 (verify) | pure Rust; internally downsamples (large-image friendly); score 0.0 = identical, higher = worse; can emit per-pixel attribute maps (useful for phase 3) |
| **SSIMULACRA2** | `ssimulacra2` (kornelski) | MIT OR Apache-2.0 (verify) | pure Rust; perceptually closer to Butteraugli; score 0 = identical; supports error maps |
| **PSNR** | none (own impl, ~40 lines) | — | cheap baseline, familiar number; compute on the same bounded buffers |

**Recommendation:** ship DSSIM + PSNR first (single `dssim` dependency +
trivial PSNR), keep SSIMULACRA2 behind the same trait as an optional
second engine (decision point D1). All engines implement:

```rust
// gui/src/metrics/mod.rs  (new; zero egui imports — headless-testable)
pub trait QualityMetric: Send + Sync {
    fn name(&self) -> &'static str;
    /// Both images as sRGB8 RGBA buffers **already bounded** (see below).
    fn compare(&self, a: &image::RgbaImage, b: &image::RgbaImage) -> MetricResult;
}
pub struct MetricResult { pub score: f32, /* dssim: 0=identical; psnr: dB, cap 100 */ pub pretty: String }
```

### Memory-bounded decode (the huge-image guardrail)

The core supports ~1 GiB / 32 K×16 K inputs; decoding two of those to RGBA
is ~2 GiB each — unacceptable. **All metric computation runs on bounded
decodes**: load via `image::ImageReader` with
`image::io::Limits::max_image_width/height/allocation` (e.g. 16 384 px and
512 MiB) and, when a dimension exceeds `METRIC_MAX_EDGE` (default 4096 px,
settings-exposed), downscale with a box filter to the longest edge before
comparing (both images through the *same* geometry: the output is resized
to the input's bounded dimensions — aspect is preserved by the encoders,
so dimensions match after this normalization; a 1-px rounding mismatch is
handled by resizing `b` to `a`'s exact size).

Caveat (documented in the UI tooltip): metrics on downscaled copies
understate fine-texture loss; the setting can be raised at the user's
risk. This is the same trade-off DSSIM's own multi-scale design makes.

### Execution model

- One dedicated metric worker thread (`std::thread` + mpsc), decoupled
  from the conversion job; **paused while a job runs** (the same policy as
  plan 11's thumbnail worker — share one worker for both, see 11 §6).
- `App` gains `metric_state: MetricState` (queue of pending paths, results
  map `HashMap<PathBuf, MetricResult>`), drained per frame like job events.
- Settings: `quality_metric: Off | Manual | AutoAfterRun` (default
  **Manual**: metrics are computed on demand — Auto doubles decode cost of
  every run; decision point D2). Metric engine choice + max edge also in
  settings.

### Presentation

- Queue row (and report row): `dssim 0.012 · psnr 41.2dB` appended in the
  status column (or the dedicated column once 11's table lands), tooltip
  with the interpretation scale ("0 = identical; <0.005 excellent; <0.05
  noticeable; higher = visible loss" — wording pinned in a pure
  `interpret(score) -> &'static str` helper, unit-tested).
- Report totals: aggregate mean DSSIM over measured files.
- Right-click menu on a converted item: **"Measure quality"** (phase 3
  menu; if 11 hasn't landed yet, a small button in the status column).

## Phase 3 — visual difference inspector (M) — feasibility: **GO**

### Feasibility assessment (asked for explicitly)

| Concern | Assessment |
|---------|-----------|
| Decoding both files in the GUI | Both input & output formats are decodable by the same `image`-based path the core uses; jxl input works (feature default-on), HEIF input only in `dec-heif` builds — grayed out otherwise (same capability pattern as the queue's unsupported rows) |
| Memory | Bounded decode (§ phase 2) to ≤ 2048 px longest edge by default → 3 buffers ≈ 3 × 16 MiB; full-res diff only offered when `w*h*8` fits a 512 MiB cap, else disabled with an explanatory tooltip |
| Diff computation | Per-pixel `abs(a-b)` amplified ×N (slider 1–32) → heatmap texture; plus optional DSSIM attribute map overlay when the `dssim` engine produced one |
| GPU/textures | `ctx.load_texture` of `egui::ColorImage` (~2048² is trivial); textures dropped when the window closes |
| Animations | First frame only (documented in-window); comparing frame 1 of animated input vs the (first-frame) output is honest, since non-anim targets already encode frame 1 |
| egui surface | `Response::context_menu` exists in 0.32 (verified `response.rs:940`) → right-click on a converted row opens: *Open output / Show in folder (needs 11's core amendment) / Measure quality / **Inspect visual difference*** |
| Verdict | **Feasible** as a modal `egui::Window` (or reuse 09's viewport pattern if the user wants it outside the main window). No new native deps beyond the metric crate. |

### UI sketch (`gui/src/panels/inspector.rs`, new)

```
┌─ visual difference — IMG_2043.jpg → IMG_2043.avif ────────────────┐
│ [side-by-side | swipe | difference]   zoom-to-fit  1:1   close ✕  │
│ ┌──────────────────┬──────────────────┐   amplification ▁▁▂▃▅ ×8  │
│ │   original       │  converted (31%) │   metric: dssim 0.012     │
│ └──────────────────┴──────────────────┘        psnr 41.2 dB       │
│ swipe handle ◄───────────────► (drag to reveal)  decoded at 2048px │
└────────────────────────────────────────────────────────────────────┘
```

- **side-by-side**: two textures, synchronized zoom/pan (shared
  `egui::ScrollArea` pair).
- **swipe**: one texture layered, clip rect at the draggable handle.
- **difference**: amplified abs-diff heatmap texture; slider re-computes
  on release (debounced) to avoid per-drag recompute.

Pure part (unit-tested): the pixel loops (`diff_heatmap(a, b, amp)`,
`swipe_geometry`) operate on `RgbaImage`s without egui types.

## File-ownership map (merge-conflict avoidance)

| File | This plan owns | Others may |
|------|----------------|------------|
| `gui/src/metrics/` (new) | everything | — |
| `gui/src/panels/inspector.rs` (new) | everything | — |
| `gui/src/queue.rs` | ratio helpers (+tests) | 11 owns the rest of the row model |
| `gui/src/panels/footer.rs` | stats_line ratio append | 12 owns the progress bar |
| `gui/src/panels/file_table.rs` | ratio cell tint + context menu | 11 rewrites the table (carries these over) |
| `gui/Cargo.toml` | adds `dssim`, (own `image` dep already added by 11 — coordinate: first of the two plans adds it) | 11/12 append their deps |

## Testing

- Headless unit: ratio bands/labels; `interpret()` thresholds; PSNR on
  synthetic images (identical → inf/cap; 1-value-off → known dB); bounded
  decode geometry (odd sizes, rounding mismatch normalization); diff
  heatmap math on 4×4 fixtures (fixtures as raw `RgbaImage::from_fn`,
  no files needed).
- Integration: run a real conversion via the existing `JobHandle` harness
  (`app.rs:772-781` pattern), then metric-pipeline on the produced file
  pair — asserts DSSIM(identical re-encode of a lossless target) ≈ 0.
- Manual: huge image (32 K px) does not OOM (bounded path taken, tooltip
  shows "decoded at 2048 px"); HEIF grayed without `dec-heif`.

## Decision points (owner sign-off)

1. **D1** metric engine: DSSIM only (recommended) vs SSIMULACRA2 only vs
   both behind a selector.
2. **D2** default of `quality_metric`: **Manual** (recommended — no
   surprise CPU cost) vs AutoAfterRun.
3. Ratio hint bands/thresholds (§ phase 1 table).

## Risks

| Risk | Mitigation |
|------|------------|
| `dssim` crate API/licence drift | verify at impl time (plans list it as candidate, not vendored); PSNR is the dependency-free fallback shipped regardless |
| Metric worker competing with conversion CPU | worker paused during jobs (same policy as thumbnails) |
| Downscaled metrics misleading users | tooltip + docs note; full-res option with memory cap |
| Row context menu + ratio edits collide with 11's table rewrite | 11 carries the ratio cell + menu over; plans share the ownership map above |
