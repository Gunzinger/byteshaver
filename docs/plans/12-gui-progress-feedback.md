# 12 — GUI: progress-bar segments (active-file shading, minimal animation) & conversion confetti

**Size:** M (§1–2 S/M, §3 S) · **Depends on:** nothing · **Parallelizable
with:** all others (owns `gui/src/panels/footer.rs` progress area + new
`gui/src/celebrate.rs`; small additive touches in `app.rs`/`settings.rs`).

## Goal

1. The bottom progress bar becomes a **segmented bar**: one segment per
   work item, colored by state — finished segments carry a shade derived
   from the file's compression result, **files currently being worked on
   are shown in another shade of color**, and the active portion carries a
   **pleasant but minimal animation**.
2. Finished conversions celebrate the efficiency gain with a
   **configurable amount of confetti**, scaled by the achieved compression
   ratio.

Doctrine: all animation/particle math lives in pure structs
(headless-testable), panels only paint state.

## 1. Tracking the *active* files (prerequisite)

Today `Queue::begin_run` (`gui/src/queue.rs:420-429`) flips **every** row
to `Running` when a job starts — the ⟳ glyph lies on untouched rows and
"running" cannot mean "being worked on". Introduce the truthful tri-state:

- `QueueItem.status` keeps meaning "not yet finished in this run" (the
  current `Running` value is effectively *pending*; rename consideration
  noted in tasks but not required for wire compat),
- `RunningJob` gains `pub active: std::collections::HashSet<PathBuf>`
  (`gui/src/app.rs`):
  - `JobEvent::FileStarted` → insert path,
  - `JobEvent::FileFinished` → remove path,
  - cleared on `finish_job`.
- Rows render: `Running && active` = **active** (distinct shade + glyph
  `⟳`), `Running && !active` = *pending* (weak text, glyph `…`) — a
  side-effect fix: the ⟳ glyph now tells the truth. Discovered rows
  (directory expansion) join `active` the same way via `FileStarted`.

Unit tests (existing event-application harness in `app.rs` tests):
multiple concurrent `FileStarted` (rayon parallelism) coexist; finishes
remove exactly their own path; cancel path (`Aborted`) clears.

## 2. Segmented progress bar

### Widget

Replace `egui::ProgressBar` in `panels/footer.rs:31-45` with a custom
painted bar (keep the existing `x/y files` text at its right):

```
pending  pending  active▒▒  active▒▒  done▪  done▫  done▪  …
└────────┬────────┴──────────────────┴──────────────┘
   ≤ 300 items: 1 segment per item      else: proportional bands
```

- **≤ `SEGMENT_CAP` (≈ 300)** work items: literal per-file segments,
  order = deterministic work-item order (the events carry `index`;
  per-file state kept in a `Vec<SegState>` sized on `Started`).
  States map: `pending` faint gray · `active` selection-blue ·
  `done-good` green tint · `done-neutral` soft green-gray · `done-grew`
  amber · `error` red · `aborted/skipped` dark gray. The done-* shade is
  the file's compression ratio hint (same bands as plan 10 §phase 1 —
  shared pure helper; if 10 hasn't landed, a private copy lands first and
  10 unifies).
- **> cap**: aggregate into proportional bands (done/active/pending
  fractions with average-ratio coloring) — large batches stay readable
  and painting stays O(cap).
- Implementation: `ui.allocate_rect` + `ui.painter()` rects; 2 px corner
  radius; 1 px gaps between segments (≥ 3 px wide segments only, thinner
  ones merge — avoids moiré at high item counts).
- Hover tooltip over segments: file name + state (+ ratio when done) —
  hit-test via segment index ranges (pure `segment_at(x, layout)`).

### Animation (minimal, pleasant)

- Only the **active** segments animate (never the whole bar):
  a slow alpha pulse `0.75 + 0.25 * (0.5 + 0.5*sin(2πt/1.6s))` on the
  active shade, plus an optional 45° **striped shimmer** sweeping at
  ~12 px/s (painter clip-rect + diagonal strokes; stripes only when
  segments are wide enough). Pure function
  `fn active_alpha(t: f32, period: f32) -> f32` unit-tested.
- Animation budget: the footer already repaints at 100 ms while a job
  runs (`app.rs:506-508`) — keep that cadence (10 fps is deliberately
  calm; the shimmer stays smooth because its speed is low), no extra
  repaint requests.
- **Reduced motion**: settings flag `reduced_motion: bool` (default
  false; egui cannot read the OS preference portably — the flag is the
  escape hatch, also honored by §3). With it on: active segments get a
  steady distinct shade, zero time-varying painting.

## 3. Confetti on finished conversions

### Trigger & intensity

On `JobEvent::Finished` (in `finish_job`), celebrate iff:
`totals.successful ≥ 1` **and** `totals.input_size > totals.output_size`
(no party when nothing was gained / everything errored).

- Settings: `confetti: Off | Sprinkle | Regular | Excessive`
  (default **Regular**; decision point D1 — this is the "configurable
  amount" dial).
- Particle count = `base(setting) × ratio_factor`:
  - base: Sprinkle 40 · Regular 120 · Excessive 400,
  - `ratio_factor = clamp(1.6 − 1.2 × ratio, 0.4, 1.5)` — better ratio →
    more confetti (50 % ratio ≈ ×1.0, 10 % ≈ ×1.5, no gain ≈ ×0.4 floor).
    Pure `fn confetti_count(setting, ratio) -> usize` — unit-tested.
- No trigger when `report.error` is set (pre-flight failures) or the run
  was fully aborted; partial success still celebrates (scaled by factor
  above).

### Particle system (`gui/src/celebrate.rs`, new — no egui types in the core struct)

```rust
pub struct Confetti { parts: Vec<Particle>, t_left: f32, /* ≤ 2.5 s hard cap */ }
pub struct Particle { pos: Vec2, vel: Vec2, rot: f32, spin: f32, size: f32, color: ColorIdx }
impl Confetti {
    pub fn burst(count: usize, origin_area: RectPx, rng: &mut impl Rng) -> Self;
    pub fn step(&mut self, dt: f32);   // gravity ~600 px/s², drag, flutter (rot → width modulation)
    pub fn is_alive(&self) -> bool;
    pub fn rects(&self) -> impl Iterator<Item = (RectPx, RotRad, ColorIdx)>; // painter food
}
```

- Emission: from the footer's left edge / Convert-button corner upward
  (celebrating *the run that just finished*, next to where its button
  lives); deterministic seed per run → identical replay in tests.
- Colors: fixed 6-color palette mapped to dark/light visual schemes via
  `ColorIdx` (painter resolves the concrete `Color32`).
- Rendering: each frame `Confetti::step(dt)` then paint small rotated
  rects in a **foreground layer**
  (`ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, id))`)
  so confetti floats above all panels; pieces are 4–8 px, matte (no
  shadows/gradients — minimal).
- Lifetime: hard cap 2.5 s; `is_alive()` false → free the layer + stop
  extra repaints. While alive:
  `ctx.request_repaint_after(Duration::from_millis(16))` (60 fps only for
  the party — short and bounded).
- `reduced_motion` (§2): confetti replaced by a single-line footer
  flourish ("✔ 62 % saved — nice run." fading out) — still a celebration,
  zero motion sickness.
- Determinism/rng: tiny xorshift or `rand`? **no new dependency** —
  10-line LCG in-module (seeded, testable).

### Testing

- Headless: particle step invariants (bounded position drift per dt,
  gravity sign, life cap), count mapping table (setting × ratio → count
  incl. clamps), trigger conditions matrix (success/abort/error ×
  grew/saved), `active_alpha` bounds, segment state mapping per
  `Outcome`, `segment_at` hit-testing math, cap-aggregation proportions
  (±1 px).
- Manual: run against the `examples/` tree at each confetti level; verify
  60 fps party ends within 2.5 s; verify footer repaint cadence returns
  to idle afterwards; reduced-motion mode shows the text flourish.

## File-ownership map

| File | Owner | Notes |
|------|-------|-------|
| `gui/src/celebrate.rs` (new) | this plan | pure + painter glue |
| `gui/src/panels/footer.rs` | this plan (progress area + flourish) | 10 appends ratio text to `stats_line` only |
| `gui/src/app.rs` | `RunningJob.active` + confetti trigger in `finish_job` | localized |
| `gui/src/settings.rs` | `confetti`, `reduced_motion` fields `#[serde(default)]` | additive |
| `gui/src/queue.rs` | glyph/truthfulness fix only (pending vs active) | 10/11 own the rest |

## Decision points

1. **D1** confetti default level (proposed: Regular; Off ships as the
   explicit setting).
2. Shimmer stripes on active segments vs pulse-only (proposed: both, in
   that order of disablement under `reduced_motion`).
3. Segment cap 300 (proposed) vs adaptive to bar width.

## Risks

| Risk | Mitigation |
|------|------------|
| Custom bar regresses a11y vs `ProgressBar` | keep textual `x/y files` + ratio line always visible (information never lives in color alone) |
| Foreground layer intercepting input | `LayerId` foreground with `InteractionProbe` off — painter-only layer takes no input (verified pattern: egui uses it for tooltips) |
| Per-frame allocs in particle loop | preallocated `Vec`, `step` mutates in place |
| Animation burns CPU on long runs | only active segments animate at the existing 100 ms cadence; confetti is time-boxed |
