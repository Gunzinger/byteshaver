//! Celebration layer (plan 12 §3): after a conversion run that actually
//! gained something, the footer throws a compression-ratio-scaled burst of
//! confetti — or, under the `reduced_motion` setting, shows a single
//! fading text line instead.
//!
//! Doctrine: the particle math lives in pure, egui-free structs
//! ([`Confetti`], [`Particle`], [`TextFlourish`], the count/trigger
//! helpers) so everything is unit-testable headless; the rendering glue
//! ([`paint`], [`palette`]) is kept thin and only maps state onto painter
//! calls. The burst uses a tiny in-module LCG (no `rand` dependency) with
//! a deterministic per-run seed, so a given run always produces the same
//! party.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::app::App;

/// Hard lifetime cap of one burst in seconds (plan 12 §3); the footer
/// stops the extra 16 ms repaints as soon as it expires.
pub const LIFETIME_CAP: f32 = 2.5;

/// Gravity in px/s² (screen coordinates: y grows downward).
const GRAVITY: f32 = 600.0;

/// Linear air drag per second (fraction of the velocity kept per step is
/// `1 / (1 + DRAG · dt)`).
const DRAG: f32 = 1.1;

/// Largest per-step delta accepted by the simulation (guards against a
/// stalled frame teleporting every piece to the ceiling).
const MAX_DT: f32 = 0.1;

/// Number of colors in the celebration palette (see [`palette`]).
const PALETTE_LEN: u8 = 6;

/// Confetti intensity (plan 12 §3, decision D1): the "configurable
/// amount" dial of the post-run celebration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfettiLevel {
    /// Never celebrate with particles.
    Off,
    /// A modest handful of pieces.
    Sprinkle,
    /// The default amount (still scaled by the achieved ratio).
    #[default]
    Regular,
    /// Far too many pieces. Deliberately available.
    Excessive,
}

/// Base piece count per [`ConfettiLevel`] (plan 12 §3 table: 40 / 120 /
/// 400; `Off` yields 0).
#[must_use]
pub fn base_count(level: ConfettiLevel) -> usize {
    match level {
        ConfettiLevel::Off => 0,
        ConfettiLevel::Sprinkle => 40,
        ConfettiLevel::Regular => 120,
        ConfettiLevel::Excessive => 400,
    }
}

/// Piece count for a run with compression `ratio` (`output / input`):
/// `base(level) × clamp(1.6 − 1.2 · ratio, 0.4, 1.5)` — the better the
/// ratio, the louder the party (50 % ≈ ×1.0, 10 % ≈ ×1.5, no gain ≈ ×0.4
/// floor). Non-finite ratios fall back to the neutral ×1.0 factor.
#[must_use]
pub fn confetti_count(level: ConfettiLevel, ratio: f32) -> usize {
    let base = base_count(level);
    if base == 0 {
        return 0;
    }
    let factor = if ratio.is_finite() {
        (1.6 - 1.2 * ratio).clamp(0.4, 1.5)
    } else {
        1.0
    };
    (base as f32 * factor).round() as usize
}

/// Whether a finished run earns a celebration (plan 12 §3 trigger): at
/// least one success, a real size gain, no pre-flight failure
/// (`report.error`) and not a fully aborted run.
#[must_use]
pub fn should_celebrate(summary: &RunSummary) -> bool {
    if summary.failed || summary.successful == 0 || summary.input_size <= summary.output_size {
        return false;
    }
    // a run with successes cannot be *fully* aborted, but the plan's guard
    // stays explicit (and future-proof against counter drift)
    summary.input_files == 0 || summary.aborted < summary.input_files
}

/// Aggregate result of a finished run: the trigger inputs of
/// [`should_celebrate`], decoupled from the core's `RunReport` so the
/// decision is testable headless.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunSummary {
    /// Files encoded successfully.
    pub successful: u64,
    /// Total counted input bytes.
    pub input_size: u64,
    /// Total counted output bytes.
    pub output_size: u64,
    /// Discovered work items of the run.
    pub input_files: u64,
    /// Items not processed because cancelation was requested.
    pub aborted: u64,
    /// Whether the run failed pre-flight (`report.error` was set).
    pub failed: bool,
}

/// Alpha multiplier of the *active* progress-bar segments: a slow pulse
/// `0.75 + 0.25 · (0.5 + 0.5 · sin(2π t / period))` ∈ `[0.75, 1.0]`
/// (plan 12 §2). Non-positive/non-finite periods fall back to 1 s.
#[must_use]
pub fn active_alpha(t: f32, period: f32) -> f32 {
    let period = if period.is_finite() && period > 0.0 {
        period
    } else {
        1.0
    };
    0.75 + 0.25 * (0.5 + 0.5 * (std::f32::consts::TAU * t / period).sin())
}

/// Minimal random source of the burst (kept a trait so tests can plug in
/// a deterministic sequence; no external `rand` dependency).
pub trait Rng {
    /// Next uniform value in `0.0..1.0`.
    fn next_f32(&mut self) -> f32;

    /// Next uniform value in `lo..hi`.
    fn next_range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next_f32() * (hi - lo)
    }
}

/// Ten-line linear congruential generator (numerical-recipes constants),
/// seeded per run so a given run replays identically in tests.
#[derive(Clone, Debug)]
pub struct Lcg {
    state: u64,
}

impl Lcg {
    /// Seeds the generator (`| 1` keeps the state off the zero fixed
    /// point of the LCG recurrence).
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }
}

impl Rng for Lcg {
    fn next_f32(&mut self) -> f32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // top 31 bits → uniform in [0, 1)
        (self.state >> 33) as f32 / (1u64 << 31) as f32
    }
}

/// Palette slot of a piece (resolved to a concrete `Color32` per
/// light/dark scheme by the [`palette`] glue).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorIdx(pub u8);

/// Axis-aligned rectangle in screen pixels (the egui-free counterpart of
/// `egui::Rect` used by the particle core).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectPx {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

/// Rotation in radians.
pub type RotRad = f32;

/// One piece of confetti (pure data — no egui types).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    /// Position x (px).
    pub x: f32,
    /// Position y (px, grows downward).
    pub y: f32,
    /// Velocity x (px/s).
    pub vx: f32,
    /// Velocity y (px/s; negative = upward).
    pub vy: f32,
    /// Flutter rotation (radians; modulates the painted width).
    pub rot: f32,
    /// Rotation speed (rad/s).
    pub spin: f32,
    /// Piece edge length in px (4–8 per plan 12 §3).
    pub size: f32,
    /// Palette slot.
    pub color: ColorIdx,
    /// Individual remaining life in seconds (≤ [`LIFETIME_CAP`]) so the
    /// burst thins out instead of vanishing all at once.
    pub life: f32,
}

/// One burst of confetti (pure simulation, plan 12 §3 sketch).
pub struct Confetti {
    parts: Vec<Particle>,
    t_left: f32,
}

impl Confetti {
    /// Ejects `count` pieces from the Convert-button corner area upward
    /// (celebrating *the run that just finished*, next to where its
    /// button lives). Deterministic for a given seed.
    #[must_use]
    pub fn burst(count: usize, origin_area: RectPx, rng: &mut impl Rng) -> Self {
        let parts = (0..count)
            .map(|_| {
                // spray cone around straight-up (±~52°)
                let angle = rng.next_range(-0.9, 0.9);
                let speed = rng.next_range(240.0, 540.0);
                Particle {
                    x: rng.next_range(origin_area.x, origin_area.x + origin_area.w),
                    y: rng.next_range(origin_area.y, origin_area.y + origin_area.h),
                    vx: speed * angle.sin(),
                    vy: -speed * angle.cos(),
                    rot: rng.next_range(0.0, std::f32::consts::TAU),
                    spin: rng.next_range(-7.0, 7.0),
                    size: rng.next_range(4.0, 8.0),
                    color: ColorIdx(rng.next_range(0.0, PALETTE_LEN as f32) as u8),
                    life: rng.next_range(1.2, LIFETIME_CAP),
                }
            })
            .collect();
        Self {
            parts,
            t_left: LIFETIME_CAP,
        }
    }

    /// Advances the simulation by `dt` seconds: gravity pulls down,
    /// drag thins the velocity, `spin` advances the flutter rotation and
    /// lives tick toward the hard cap. Mutates in place (no per-frame
    /// reallocation).
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, MAX_DT);
        if dt <= 0.0 {
            return;
        }
        self.t_left = (self.t_left - dt).max(0.0);
        let drag = 1.0 / (1.0 + DRAG * dt);
        for part in &mut self.parts {
            if part.life <= 0.0 {
                continue;
            }
            part.life -= dt;
            part.vy += GRAVITY * dt;
            part.vx *= drag;
            part.vy *= drag;
            part.rot += part.spin * dt;
            part.x += part.vx * dt;
            part.y += part.vy * dt;
        }
    }

    /// Whether the party is still on (hard [`LIFETIME_CAP`] and pieces
    /// left).
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.t_left > 0.0 && !self.parts.is_empty()
    }

    /// Number of still-visible pieces.
    #[must_use]
    pub fn visible(&self) -> usize {
        self.parts.iter().filter(|part| part.life > 0.0).count()
    }

    /// Global fade multiplier over the last 0.5 s of the burst (1.0
    /// before that) so the party ends softly instead of popping off.
    #[must_use]
    pub fn fade_alpha(&self) -> f32 {
        (self.t_left / 0.5).clamp(0.0, 1.0)
    }

    /// Painter food: `(rect, rotation, palette slot)` per live piece.
    pub fn rects(&self) -> impl Iterator<Item = (RectPx, RotRad, ColorIdx)> + '_ {
        self.parts
            .iter()
            .filter(|part| part.life > 0.0)
            .map(|part| {
                (
                    RectPx {
                        x: part.x - part.size * 0.5,
                        y: part.y - part.size * 0.5,
                        w: part.size,
                        h: part.size * 0.6,
                    },
                    part.rot,
                    part.color,
                )
            })
    }
}

/// Reduced-motion celebration (plan 12 §3): a single fading footer text
/// line instead of moving particles.
#[derive(Clone, Debug, PartialEq)]
pub struct TextFlourish {
    /// The rendered text, e.g. `✔ 62 % saved — nice run.`
    pub text: String,
    age: f32,
}

impl TextFlourish {
    /// Builds the flourish for the given saved percentage (0–100).
    #[must_use]
    pub fn new(percent_saved: u32) -> Self {
        Self {
            text: format!("✔ {percent_saved} % saved — nice run."),
            age: 0.0,
        }
    }

    /// Advances the fade clock.
    pub fn step(&mut self, dt: f32) {
        self.age += dt.clamp(0.0, MAX_DT);
    }

    /// Seconds since the flourish started.
    ///
    /// (Diagnostic accessor; the fade logic works off `alpha`/`is_alive`.)
    #[allow(dead_code)]
    #[must_use]
    pub fn age(&self) -> f32 {
        self.age
    }

    /// Whether the line is still on screen (hard 2.5 s cap, same budget
    /// as the confetti).
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.age < LIFETIME_CAP
    }

    /// Opacity: full for the first second, then a linear fade to zero.
    #[must_use]
    pub fn alpha(&self) -> f32 {
        ((LIFETIME_CAP - self.age) / (LIFETIME_CAP - 1.0)).clamp(0.0, 1.0)
    }
}

/// Resolves a palette slot to a concrete matte color for the current
/// light/dark scheme (painter glue; bright tones on dark backgrounds,
/// deeper ones on light).
#[must_use]
pub fn palette(idx: ColorIdx, dark_mode: bool) -> egui::Color32 {
    let (dark, light) = match idx.0 % PALETTE_LEN {
        0 => ((233, 99, 140), (200, 40, 90)),   // rose
        1 => ((240, 180, 70), (204, 132, 16)),  // amber
        2 => ((120, 200, 120), (56, 148, 80)),  // green
        3 => ((100, 190, 210), (16, 128, 158)), // cyan
        4 => ((125, 152, 240), (62, 92, 200)),  // blue
        _ => ((182, 140, 230), (118, 78, 190)), // violet
    };
    let (r, g, b) = if dark_mode { dark } else { light };
    egui::Color32::from_rgb(r, g, b)
}

/// Steps and paints the active celebration (called once per frame from
/// the footer): confetti pieces go into a painter-only foreground layer
/// (takes no input) above all panels, and both celebration forms manage
/// their own repaint budget — 16 ms for the bounded confetti party, the
/// calm 100 ms cadence for the text fade. Expired celebrations drop so
/// the repaint cadence returns to idle.
pub fn paint(app: &mut App, ctx: &egui::Context) {
    let dt = ctx.input(|input| input.stable_dt);
    let dark_mode = ctx.style().visuals.dark_mode;

    if let Some(confetti) = app.confetti.as_mut() {
        confetti.step(dt);
        if confetti.is_alive() && confetti.visible() > 0 {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("byteshaver-confetti"),
            ));
            let fade = confetti.fade_alpha();
            for (rect, rot, color_idx) in confetti.rects() {
                // flutter: the spin modulates the painted width, so the
                // piece appears to flip over while falling (the painter
                // has no rotated-rect primitive; rotation → width)
                let flutter = (rot.cos().abs() * 0.65 + 0.35).clamp(0.35, 1.0);
                let piece = egui::Rect::from_min_size(
                    egui::pos2(rect.x, rect.y),
                    egui::vec2((rect.w * flutter).max(1.0), rect.h),
                );
                painter.rect_filled(
                    piece,
                    1.0,
                    palette(color_idx, dark_mode).gamma_multiply(fade),
                );
            }
            ctx.request_repaint_after(Duration::from_millis(16));
        } else {
            app.confetti = None; // party over: free the layer, stop repaints
        }
    }

    if let Some(flourish) = app.flourish.as_mut() {
        flourish.step(dt);
        if flourish.is_alive() {
            // the footer paints the line itself; only the (already calm)
            // repaint budget is managed here
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            app.flourish = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- particle invariants ----------------------------------------------

    #[test]
    fn burst_yields_requested_pieces_with_expected_geometry() {
        let mut rng = Lcg::new(42);
        let confetti = Confetti::burst(
            50,
            RectPx {
                x: 10.0,
                y: 100.0,
                w: 40.0,
                h: 20.0,
            },
            &mut rng,
        );
        assert_eq!(confetti.visible(), 50);
        assert!(confetti.is_alive());
        for (rect, _rot, color) in confetti.rects() {
            assert!((4.0..=8.0).contains(&rect.w), "piece width {}", rect.w);
            assert!(rect.h < rect.w && rect.h > 0.0, "flat piece");
            assert!(color.0 < PALETTE_LEN);
        }
    }

    #[test]
    fn burst_is_deterministic_for_a_seed() {
        let origin = RectPx {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 50.0,
        };
        let mut rng_a = Lcg::new(7);
        let mut rng_b = Lcg::new(7);
        let a = Confetti::burst(30, origin, &mut rng_a);
        let b = Confetti::burst(30, origin, &mut rng_b);
        assert_eq!(a.rects().collect::<Vec<_>>(), b.rects().collect::<Vec<_>>());
    }

    #[test]
    fn step_applies_gravity_drag_and_flutter_per_kinematics() {
        let origin = RectPx {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let mut confetti = Confetti::burst(1, origin, &mut Lcg::new(2));
        let p0 = confetti.parts[0];
        let dt = 0.05;
        confetti.step(dt);
        let p1 = confetti.parts[0];
        // the step integrates gravity first, then drag over the sum:
        // vy₁ = (vy₀ + g·dt) / (1 + DRAG·dt) — gravity pulls toward +y
        let drag = 1.0 / (1.0 + DRAG * dt);
        assert!(
            (p1.vy - (p0.vy + GRAVITY * dt) * drag).abs() < 1e-3,
            "vy must follow the gravity-then-drag integration"
        );
        assert!(p1.vy > p0.vy, "net gravity pull toward +y (screen-down)");
        // …drag thins the horizontal component…
        assert!((p1.vx - p0.vx * drag).abs() < 1e-3);
        // …flutter rotation advances…
        assert!((p1.rot - (p0.rot + p0.spin * dt)).abs() < 1e-4);
        // …and the per-step drift stays inside the kinematics bound
        // (launch speed ≤ 540 px/s plus the gravity gained this step;
        // position integrates the post-gravity velocity — semi-implicit
        // Euler)
        assert!((p1.y - p0.y - p1.vy * dt).abs() < 1e-2);
        assert!((p1.x - p0.x - p1.vx * dt).abs() < 1e-2);
        let drift = (p1.x - p0.x).abs() + (p1.y - p0.y).abs();
        assert!(drift <= (540.0 + GRAVITY * dt) * dt + 0.5, "drift {drift}");
    }

    #[test]
    fn drag_keeps_the_fall_slower_than_free_fall() {
        let origin = RectPx {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let mut confetti = Confetti::burst(1, origin, &mut Lcg::new(2));
        let v0 = confetti.parts[0].vy;
        let dt = 0.05;
        for _ in 0..20 {
            confetti.step(dt);
        }
        let free_fall = v0 + GRAVITY * dt * 20.0;
        assert!(
            confetti.parts[0].vy < free_fall,
            "drag must keep the speed below undamped free fall ({} < {free_fall})",
            confetti.parts[0].vy
        );
    }

    #[test]
    fn life_cap_expires_within_hard_limit() {
        let origin = RectPx {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let mut confetti = Confetti::burst(10, origin, &mut Lcg::new(3));
        let mut total = 0.0;
        while confetti.is_alive() {
            confetti.step(0.016);
            total += 0.016;
            assert!(total <= LIFETIME_CAP + 0.1, "burst must die at the cap");
        }
        assert!(
            total >= LIFETIME_CAP - 0.1,
            "a fresh burst lives about the cap, not shorter ({total})"
        );
        assert_eq!(confetti.t_left, 0.0, "lifetime never goes negative");
        assert!(!confetti.is_alive());
    }

    #[test]
    fn individual_pieces_thin_out_before_the_cap() {
        // constructed directly for exact life values (parts are in-module)
        let mut confetti = Confetti {
            parts: vec![
                Particle {
                    x: 0.0,
                    y: 0.0,
                    vx: 0.0,
                    vy: 0.0,
                    rot: 0.0,
                    spin: 1.0,
                    size: 6.0,
                    color: ColorIdx(0),
                    life: 1.5,
                },
                Particle {
                    x: 5.0,
                    y: 5.0,
                    vx: 0.0,
                    vy: 0.0,
                    rot: 1.0,
                    spin: -1.0,
                    size: 6.0,
                    color: ColorIdx(1),
                    life: 2.4,
                },
            ],
            t_left: LIFETIME_CAP,
        };
        // `step` clamps its delta (frame-time guard), so advance in
        // frame-sized increments
        for _ in 0..16 {
            confetti.step(0.1);
        }
        assert_eq!(confetti.visible(), 1, "the short-lived piece expired");
        assert!(confetti.is_alive(), "burst cap still ticking");
        for _ in 0..10 {
            confetti.step(0.1);
        }
        assert_eq!(confetti.visible(), 0, "all pieces gone before the cap");
    }

    #[test]
    fn step_ignores_zero_and_huge_deltas() {
        let origin = RectPx {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let mut confetti = Confetti::burst(2, origin, &mut Lcg::new(11));
        let (y0, life0) = (confetti.parts[0].y, confetti.parts[0].life);
        confetti.step(0.0);
        assert_eq!(confetti.parts[0].y, y0, "zero dt changes nothing");
        // a stalled frame must not teleport pieces: the delta clamps
        confetti.step(60.0);
        assert!(
            (confetti.parts[0].life - (life0 - MAX_DT)).abs() < 1e-4,
            "huge dt clamps to MAX_DT"
        );
        let drifted = confetti.parts[0].y - y0;
        assert!(drifted <= (540.0 + GRAVITY * MAX_DT) * MAX_DT + 0.5);
    }

    #[test]
    fn empty_burst_is_never_alive() {
        let origin = RectPx {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let confetti = Confetti::burst(0, origin, &mut Lcg::new(1));
        assert!(!confetti.is_alive());
        assert_eq!(confetti.rects().count(), 0);
    }

    // ---- count mapping (setting × ratio) ----------------------------------

    #[test]
    fn confetti_count_table_including_clamps() {
        // Off never celebrates
        for ratio in [0.0, 0.1, 0.5, 1.0, 2.0] {
            assert_eq!(confetti_count(ConfettiLevel::Off, ratio), 0);
        }
        // neutral ratio (0.5) → plain base
        assert_eq!(confetti_count(ConfettiLevel::Sprinkle, 0.5), 40);
        assert_eq!(confetti_count(ConfettiLevel::Regular, 0.5), 120);
        assert_eq!(confetti_count(ConfettiLevel::Excessive, 0.5), 400);
        // great ratio (0.1) → ×1.48 rounds up to the ×1.5 clamp band
        assert_eq!(confetti_count(ConfettiLevel::Regular, 0.1), 178);
        // perfect ratio (0.0) → ×1.5 ceiling clamp
        assert_eq!(confetti_count(ConfettiLevel::Regular, 0.0), 180);
        assert_eq!(confetti_count(ConfettiLevel::Excessive, 0.0), 600);
        // no gain (1.0) → ×0.4 floor
        assert_eq!(confetti_count(ConfettiLevel::Regular, 1.0), 48);
        // grew (2.0) → clamped to the same ×0.4 floor
        assert_eq!(confetti_count(ConfettiLevel::Regular, 2.0), 48);
        // nonsense ratios stay finite (neutral ×1.0 fallback)
        assert_eq!(confetti_count(ConfettiLevel::Regular, f32::NAN), 120);
        assert_eq!(confetti_count(ConfettiLevel::Regular, f32::INFINITY), 120);
    }

    // ---- trigger matrix ----------------------------------------------------

    fn summary(successful: u64, input: u64, output: u64) -> RunSummary {
        RunSummary {
            successful,
            input_size: input,
            output_size: output,
            input_files: successful,
            aborted: 0,
            failed: false,
        }
    }

    #[test]
    fn trigger_matrix_success_gain_error_abort() {
        // the happy case: saved bytes, at least one success
        assert!(should_celebrate(&summary(1, 100, 40)));
        assert!(should_celebrate(&summary(3, 100, 99)));
        // no gain / grew
        assert!(!should_celebrate(&summary(1, 100, 100)));
        assert!(!should_celebrate(&summary(1, 100, 140)));
        // nothing succeeded
        assert!(!should_celebrate(&summary(0, 100, 40)));
        // pre-flight failure
        let mut failed = summary(1, 100, 40);
        failed.failed = true;
        assert!(!should_celebrate(&failed));
        // fully aborted (despite bytes set)
        let mut aborted = summary(0, 100, 40);
        aborted.aborted = 5;
        aborted.input_files = 5;
        assert!(!should_celebrate(&aborted));
        // partial abort still celebrates
        let mut partial = summary(2, 100, 40);
        partial.aborted = 1;
        partial.input_files = 3;
        assert!(should_celebrate(&partial));
    }

    // ---- active pulse alpha --------------------------------------------------

    #[test]
    fn active_alpha_stays_within_pulse_bounds() {
        for step in 0..=160 {
            let t = step as f32 * 0.1;
            let alpha = active_alpha(t, 1.6);
            assert!((0.75..=1.0).contains(&alpha), "alpha {alpha} at t={t}");
        }
        // the extremes of the sine sweep
        assert!((active_alpha(0.4, 1.6) - 1.0).abs() < 1e-6, "peak");
        assert!((active_alpha(1.2, 1.6) - 0.75).abs() < 1e-6, "trough");
        // degenerate periods fall back instead of producing NaN
        assert!(active_alpha(1.0, 0.0).is_finite());
        assert!(active_alpha(1.0, -2.0).is_finite());
    }

    // ---- reduced-motion flourish ----------------------------------------------

    #[test]
    fn flourish_text_alpha_and_lifetime() {
        let mut flourish = TextFlourish::new(62);
        assert_eq!(flourish.text, "✔ 62 % saved — nice run.");
        assert_eq!(flourish.alpha(), 1.0);
        // `step` clamps its delta (frame-time guard), so advance in
        // frame-sized increments
        for _ in 0..10 {
            flourish.step(0.1);
        }
        assert!((flourish.alpha() - 1.0).abs() < 1e-6, "held for 1 s");
        for _ in 0..7 {
            flourish.step(0.1);
        }
        flourish.step(0.05);
        assert!((flourish.alpha() - 0.5).abs() < 1e-6, "linear fade");
        for _ in 0..100 {
            flourish.step(0.1);
            assert!((0.0..=1.0).contains(&flourish.alpha()));
        }
        assert!(!flourish.is_alive(), "dies at the 2.5 s cap");
    }

    // ---- rng ----------------------------------------------------------------

    #[test]
    fn lcg_is_deterministic_and_normalized() {
        let mut a = Lcg::new(9);
        let mut b = Lcg::new(9);
        for _ in 0..100 {
            let value = a.next_f32();
            assert_eq!(value, b.next_f32(), "same seed → same stream");
            assert!((0.0..1.0).contains(&value), "value {value} out of range");
        }
        // the |1 fixup keeps a zero seed off the LCG's zero fixed point…
        let zero_value = Lcg::new(0).next_f32();
        assert!(zero_value > 0.0, "seed 0 must not lock at zero");
        // …and different seeds diverge (0|1 = 1 ≠ 3 = 2|1)
        assert_ne!(zero_value, Lcg::new(2).next_f32());
        assert_ne!(a.next_f32(), Lcg::new(2).next_f32());
    }

    #[test]
    fn next_range_stays_within_bounds() {
        let mut rng = Lcg::new(4);
        for _ in 0..100 {
            let value = rng.next_range(-3.0, 5.0);
            assert!((-3.0..5.0).contains(&value));
        }
    }

    // ---- palette ------------------------------------------------------------

    #[test]
    fn palette_resolves_all_slots_in_both_schemes() {
        for slot in 0..PALETTE_LEN {
            let dark = palette(ColorIdx(slot), true);
            let light = palette(ColorIdx(slot), false);
            assert_ne!(dark, light);
        }
        // slots wrap modulo the palette
        assert_eq!(
            palette(ColorIdx(PALETTE_LEN + 1), true),
            palette(ColorIdx(1), true)
        );
    }
}
