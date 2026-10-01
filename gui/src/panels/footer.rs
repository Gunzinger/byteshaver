//! Run footer: Convert/Cancel buttons, the segmented progress bar (plan
//! 12 §2), the aggregate statistics (fed by `JobEvent::ProgressStats`
//! while running and by the final `RunReport` afterwards — identical math
//! to the CLI summary) and the post-run celebration surfaces (the
//! reduced-motion text flourish plus the confetti overlay stepping, plan
//! 12 §3).
//!
//! The bar layout/hit-testing lives in pure functions below ([`SegRect`],
//! [`layout_segments`], [`segment_at`]) with unit tests; the painting
//! glue only maps [`SegTone`] onto colors and animates the *active*
//! segments (pulse + shimmer, both disabled under `reduced_motion`).

use std::path::PathBuf;

use crate::app::{App, FooterStats, SegState, SegTone};
use crate::queue::format_size;

/// Maximum work items painted as literal per-file segments (decision D3);
/// larger batches aggregate into proportional bands so painting stays
/// O(cap) and large batches stay readable.
pub const SEGMENT_CAP: usize = 300;

/// Bar height in points.
const BAR_HEIGHT: f32 = 14.0;

/// Corner radius of the bar track and segments.
const BAR_RADIUS: f32 = 2.0;

/// Gap between two segments (only drawn when both sides are wide enough;
/// thinner segments merge to avoid moiré at high item counts).
const SEGMENT_GAP: f32 = 1.0;

/// Segments narrower than this merge with same-colored neighbors instead
/// of showing per-item gaps.
const MIN_SEG_WIDTH: f32 = 3.0;

/// Pulse period of the active segments in seconds (plan 12 §2).
const PULSE_PERIOD: f32 = 1.6;

/// Shimmer sweep speed in px/s (plan 12 §2: ~12 px/s).
const SHIMMER_SPEED: f32 = 12.0;

/// Spacing between two shimmer stripes (45° diagonals).
const SHIMMER_PERIOD: f32 = 9.0;

/// Renders the footer panel (run controls + progress + totals).
pub fn show(app: &mut App, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("byteshaver-footer").show(ctx, |ui| {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            // run controls -------------------------------------------------
            let mut convert = ui.add_enabled(app.can_start(), egui::Button::new("▶ Convert"));
            if let Some(reason) = app.start_blocker() {
                convert = convert.on_disabled_hover_text(reason);
            }
            if convert.clicked() {
                app.start_job();
            }
            let cancel_enabled = app.running.as_ref().is_some_and(|job| !job.stop.raised());
            if ui
                .add_enabled(cancel_enabled, egui::Button::new("✕ Cancel"))
                .on_disabled_hover_text("no running job")
                .clicked()
            {
                app.cancel_job();
            }

            // progress ------------------------------------------------------
            show_progress(app, ui);

            // aggregate statistics ------------------------------------------
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let stats_text = if let Some(job) = &app.running {
                    stats_line(&job.stats, Some(job.started_at.elapsed()))
                } else if let Some(report) = &app.report {
                    stats_line(
                        &FooterStats {
                            input_bytes: report.totals.input_size,
                            output_bytes: report.totals.output_size,
                            ok: report.totals.successful,
                            skipped: report.totals.skipped,
                            errors: report.totals.errors,
                        },
                        Some(report.elapsed),
                    )
                } else {
                    String::new()
                };
                if !stats_text.is_empty() {
                    ui.label(egui::RichText::new(stats_text).weak());
                }
            });
        });
        ui.add_space(2.0);
        // reduced-motion celebration line (plan 12 §3), before start
        // blockers so both can coexist visually
        show_flourish(app, ui);
        // start blockers visible in full when present
        if let Some(error) = &app.start_error {
            ui.colored_label(ui.visuals().error_fg_color, error.clone());
        }
        // plan 15 F17: the directory-mode-without-folder blocker renders
        // in the error line while it is the thing preventing a start (a
        // tooltip alone is too easy to miss — no silent fallback)
        if let Some(message) = app.output_blocker()
            && app.start_blocker().as_deref() == Some(message.as_str())
        {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
        ui.add_space(2.0);
    });
    // celebration overlay above all panels + its repaint budget
    crate::celebrate::paint(app, ctx);
}

// ---- segmented progress bar ----------------------------------------------

/// One painted bar segment covering work items `first..=last` (pure
/// layout result, unit-tested; painting maps `tone` onto colors).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegRect {
    /// Left edge relative to the bar's left.
    pub x0: f32,
    /// Right edge relative to the bar's left.
    pub x1: f32,
    /// Paint class shared by the covered items.
    pub tone: SegTone,
    /// First covered work-item index.
    pub first: usize,
    /// Last covered work-item index (inclusive).
    pub last: usize,
}

impl SegRect {
    /// Horizontal extent in points.
    #[must_use]
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
}

/// Computes the bar layout (pure, unit-tested): runs of same-colored
/// items become segments — one rect per item while segments stay ≥ 3 px
/// wide, merged same-color runs (or the proportional bands of
/// `band_mode`) otherwise. The final rect always ends exactly at `width`.
#[must_use]
pub fn layout_segments(states: &[SegState], width: f32, band_mode: bool) -> Vec<SegRect> {
    if states.is_empty() || width <= 0.0 {
        return Vec::new();
    }
    // collapse equal-tone neighbors into runs
    let mut runs: Vec<(SegTone, usize, usize)> = Vec::new();
    for (index, state) in states.iter().enumerate() {
        match runs.last_mut() {
            Some((tone, _, last)) if *tone == state.tone() => *last = index,
            _ => runs.push((state.tone(), index, index)),
        }
    }
    let total = states.len() as f32;
    let per_item = width / total;
    let merge = band_mode || per_item < MIN_SEG_WIDTH;
    let mut rects = Vec::new();
    let mut x = 0.0;
    for (tone, first, last) in runs {
        if merge {
            // one proportional rect per run (band aggregation merges
            // every run; the thin mode only the same-colored ones)
            let span = (last - first + 1) as f32 / total * width;
            rects.push(SegRect {
                x0: x,
                x1: x + span,
                tone,
                first,
                last,
            });
            x += span;
        } else {
            for index in first..=last {
                rects.push(SegRect {
                    x0: x,
                    x1: x + per_item,
                    tone,
                    first: index,
                    last: index,
                });
                x += per_item;
            }
        }
    }
    if let Some(last_rect) = rects.last_mut() {
        last_rect.x1 = width; // absorb float drift
    }
    rects
}

/// Hit-test (pure, unit-tested): the segment under bar-relative `x`,
/// `None` outside the bar. Boundary-tolerant: `x` at a segment edge
/// resolves to the segment starting there.
#[must_use]
pub fn segment_at(layout: &[SegRect], x: f32) -> Option<&SegRect> {
    let last = layout.last()?;
    if !(0.0..=last.x1).contains(&x) {
        return None;
    }
    // pull the exact right edge (and float jitter at it) into the last
    // segment; 1e-3 is safely above the ulp of typical bar widths
    let x = x.min((last.x1 - 1e-3).max(0.0));
    layout.iter().find(|rect| rect.x0 <= x && x < rect.x1)
}

/// The segmented progress bar plus its textual `x/y files` companion —
/// the information never lives in color alone (plan 12 risk table).
fn show_progress(app: &App, ui: &mut egui::Ui) {
    let Some(job) = &app.running else {
        return;
    };
    let text = match job.progress() {
        Some(_) => format!(
            "{}/{} files",
            job.finished_items,
            job.total_items.unwrap_or(0)
        ),
        None => "discovering files…".to_string(),
    };
    let width = (ui.available_width() * 0.4).clamp(160.0, 640.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, BAR_HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    // track behind the segments (visible while discovering / at 0 items)
    painter.rect_filled(rect, BAR_RADIUS, ui.visuals().extreme_bg_color);
    if !job.segments.is_empty() {
        let band_mode = job.segments.len() > SEGMENT_CAP;
        let layout = layout_segments(&job.segments, rect.width(), band_mode);
        let t = ui.input(|input| input.time) as f32;
        let reduced_motion = app.settings.reduced_motion;
        for seg in &layout {
            let segment_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left() + seg.x0, rect.top()),
                egui::pos2(rect.left() + seg.x1, rect.bottom()),
            );
            // 1 px gaps between wide segments only (thinner ones merge)
            let gap = if seg.width() >= MIN_SEG_WIDTH {
                SEGMENT_GAP
            } else {
                0.0
            };
            let segment_rect = segment_rect.shrink2(egui::vec2(gap * 0.5, 0.0));
            paint_segment(ui, painter, segment_rect, seg.tone, t, reduced_motion);
        }
        // hover tooltip: file name + state (+ ratio when done)
        if let Some(pos) = response.hover_pos()
            && let Some(seg) = segment_at(&layout, pos.x - rect.left())
        {
            response.on_hover_text(segment_tooltip(seg, &job.segments, &job.seg_paths));
        }
    }
    ui.weak(text);
}

/// Paints one segment: a steady shade, or — for *active* segments — the
/// alpha pulse plus a 45° striped shimmer sweeping at ~12 px/s (both
/// disabled under `reduced_motion`, in that order of disablement; only
/// active segments ever animate, never the whole bar).
fn paint_segment(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    tone: SegTone,
    t: f32,
    reduced_motion: bool,
) {
    let color = tone_color(ui, tone);
    if tone != SegTone::Active {
        painter.rect_filled(rect, BAR_RADIUS, color);
        return;
    }
    let alpha = if reduced_motion {
        1.0
    } else {
        crate::celebrate::active_alpha(t, PULSE_PERIOD)
    };
    painter.rect_filled(rect, BAR_RADIUS, color.gamma_multiply(alpha));
    if !reduced_motion && rect.width() >= 3.0 * MIN_SEG_WIDTH {
        // diagonal strokes, clipped to the segment (plan 12 §2)
        let clipped = painter.with_clip_rect(rect);
        let stroke = egui::Stroke::new(
            2.0_f32,
            ui.visuals().strong_text_color().gamma_multiply(0.12),
        );
        let height = rect.height();
        let offset = (t * SHIMMER_SPEED) % (SHIMMER_PERIOD * 2.0);
        let mut x = rect.left() - height - SHIMMER_PERIOD + offset;
        while x < rect.right() + height {
            clipped.line_segment(
                [
                    egui::pos2(x - height, rect.bottom()),
                    egui::pos2(x, rect.top()),
                ],
                stroke,
            );
            x += SHIMMER_PERIOD;
        }
    }
}

/// Segment color per paint class (subtle tints per plan 12 §2; theme
/// colors where a semantic already exists).
fn tone_color(ui: &egui::Ui, tone: SegTone) -> egui::Color32 {
    match tone {
        SegTone::Pending => ui.visuals().weak_text_color().gamma_multiply(0.35),
        SegTone::Active => ui.visuals().selection.stroke.color,
        SegTone::Good => egui::Color32::from_rgb(90, 190, 110),
        SegTone::Neutral => egui::Color32::from_rgb(125, 150, 125),
        SegTone::Grew => egui::Color32::from_rgb(214, 158, 70),
        SegTone::Error => ui.visuals().error_fg_color,
        SegTone::Skipped => ui.visuals().weak_text_color(),
    }
}

/// Tooltip text of a segment: file name(s) + state (+ saved ratio when a
/// single done file is hovered; bands show their shared state).
fn segment_tooltip(seg: &SegRect, states: &[SegState], paths: &[Option<PathBuf>]) -> String {
    let count = seg.last - seg.first + 1;
    let name = paths.get(seg.first).and_then(Option::as_deref).map(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string())
    });
    let subject = if count == 1 {
        name.unwrap_or_else(|| "1 file".to_string())
    } else {
        format!("{count} files")
    };
    let state = match states.get(seg.first) {
        Some(SegState::Done {
            tone: SegTone::Good | SegTone::Neutral,
            ratio: Some(ratio),
        }) if count == 1 => {
            let saved = (((1.0 - ratio) * 100.0).round() as i64).max(0);
            format!("done — {saved} % saved")
        }
        Some(state) => match state.tone() {
            SegTone::Pending => "pending".to_string(),
            SegTone::Active => "converting…".to_string(),
            SegTone::Grew => "done — larger than input".to_string(),
            SegTone::Error => "failed".to_string(),
            SegTone::Skipped => "skipped / not processed".to_string(),
            SegTone::Good | SegTone::Neutral => "done".to_string(),
        },
        None => String::new(),
    };
    format!("{subject} — {state}")
}

/// The reduced-motion celebration: a single fading footer text line
/// (plan 12 §3) — the same percentage as the confetti would celebrate,
/// zero motion.
fn show_flourish(app: &App, ui: &mut egui::Ui) {
    let Some(flourish) = &app.flourish else {
        return;
    };
    if !flourish.is_alive() {
        return;
    }
    let base = egui::Color32::from_rgb(90, 190, 110);
    let faded = ui
        .visuals()
        .panel_fill
        .lerp_to_gamma(base, flourish.alpha());
    ui.label(egui::RichText::new(&flourish.text).color(faded));
}

/// Footer statistics line (same numbers as the CLI progress bar message),
/// with the plan-10 ratio appendix `(62% · saved 89.1MiB)` right after the
/// sizes (the same [`crate::ratio`] helpers the report totals use, so both
/// surfaces show identical strings).
fn stats_line(stats: &FooterStats, elapsed: Option<std::time::Duration>) -> String {
    let mut line = format!(
        "{} → {} {} | ✔ {} — {} ✖ {}",
        format_size(stats.input_bytes),
        format_size(stats.output_bytes),
        crate::ratio::ratio_appendix(stats.input_bytes, stats.output_bytes),
        stats.ok,
        stats.skipped,
        stats.errors
    );
    if let Some(elapsed) = elapsed {
        let secs = elapsed.as_secs_f32();
        line.push_str(&format!(" · {secs:.1}s"));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states(pattern: &[SegTone]) -> Vec<SegState> {
        pattern
            .iter()
            .map(|tone| match tone {
                SegTone::Pending => SegState::Pending,
                SegTone::Active => SegState::Active,
                tone => SegState::Done {
                    tone: *tone,
                    ratio: Some(0.5),
                },
            })
            .collect()
    }

    // ---- layout ------------------------------------------------------------

    #[test]
    fn wide_enough_bars_paint_one_rect_per_item_with_proportional_spans() {
        let sample = states(&[
            SegTone::Pending,
            SegTone::Pending,
            SegTone::Active,
            SegTone::Good,
        ]);
        let layout = layout_segments(&sample, 400.0, false);
        assert_eq!(layout.len(), 4, "100 px per item ≥ 3 px → no merging");
        for (index, rect) in layout.iter().enumerate() {
            assert_eq!(rect.first, index);
            assert_eq!(rect.last, index);
            assert!((rect.width() - 100.0).abs() < 1e-3);
        }
        // segments tile the bar exactly
        assert!((layout[0].x0 - 0.0).abs() < 1e-4);
        assert!((layout[3].x1 - 400.0).abs() < 1e-4);
        for pair in layout.windows(2) {
            assert!((pair[0].x1 - pair[1].x0).abs() < 1e-4, "no holes");
        }
    }

    #[test]
    fn thin_segments_merge_same_colored_runs_only() {
        let sample = states(&[
            SegTone::Pending,
            SegTone::Pending,
            SegTone::Active,
            SegTone::Good,
        ]);
        // 400 px / 4 items... use a bar where per-item width < 3 px
        let layout = layout_segments(&sample, 8.0, false);
        assert_eq!(
            layout.len(),
            3,
            "the two pending items merge, active/good stay separate"
        );
        assert_eq!(layout[0].first, 0);
        assert_eq!(layout[0].last, 1);
        assert_eq!(layout[1].tone, SegTone::Active);
        assert_eq!(layout[2].tone, SegTone::Good);
        assert!((layout[2].x1 - 8.0).abs() < 1e-4, "bar fully covered");
    }

    #[test]
    fn band_mode_merges_every_run_proportionally() {
        let sample: Vec<SegState> = (0..SEGMENT_CAP + 50)
            .map(|index| {
                if index < 100 {
                    SegState::Done {
                        tone: SegTone::Good,
                        ratio: Some(0.5),
                    }
                } else if index < 200 {
                    SegState::Active
                } else {
                    SegState::Pending
                }
            })
            .collect();
        let total = sample.len();
        let layout = layout_segments(&sample, 350.0, true);
        assert_eq!(layout.len(), 3, "done/active/pending bands");
        assert_eq!(layout[0].tone, SegTone::Good);
        assert_eq!(layout[0].first, 0);
        assert_eq!(layout[0].last, 99);
        // ±1 px aggregation proportions
        let expected_good = 350.0 * 100.0 / total as f32;
        assert!(
            (layout[0].width() - expected_good).abs() <= 1.0,
            "band {} vs expected {expected_good}",
            layout[0].width()
        );
        let expected_active = 350.0 * 100.0 / total as f32;
        assert!((layout[1].width() - expected_active).abs() <= 1.0);
        let expected_pending = 350.0 * 150.0 / total as f32;
        assert!((layout[2].width() - expected_pending).abs() <= 1.0);
        assert!((layout[2].x1 - 350.0).abs() < 1e-4);
    }

    #[test]
    fn layout_handles_empty_and_degenerate_inputs() {
        assert!(layout_segments(&[], 100.0, false).is_empty());
        let sample = states(&[SegTone::Pending]);
        assert!(layout_segments(&sample, 0.0, false).is_empty());
        assert!(layout_segments(&sample, -5.0, false).is_empty());
        // single item covers the full bar
        let layout = layout_segments(&sample, 120.0, false);
        assert_eq!(layout.len(), 1);
        assert!((layout[0].width() - 120.0).abs() < 1e-4);
    }

    // ---- hit-testing ---------------------------------------------------------

    #[test]
    fn segment_at_resolves_indices_and_edges() {
        // 3 items over 300 px → 100 px segments: [0,100) [100,200) [200,300]
        let sample = states(&[SegTone::Pending, SegTone::Active, SegTone::Good]);
        let layout = layout_segments(&sample, 300.0, false);
        assert_eq!(segment_at(&layout, -1.0), None, "outside left");
        assert_eq!(segment_at(&layout, 300.5), None, "outside right");
        assert_eq!(segment_at(&layout, 0.0).expect("first").first, 0);
        assert_eq!(segment_at(&layout, 99.9).expect("first half").first, 0);
        assert_eq!(
            segment_at(&layout, 100.0).expect("boundary → next").first,
            1
        );
        assert_eq!(segment_at(&layout, 199.9).expect("middle").first, 1);
        assert_eq!(segment_at(&layout, 299.9).expect("last").first, 2);
        // the exact right edge clamps into the last segment
        assert_eq!(segment_at(&layout, 300.0).expect("clamped").first, 2);
        assert_eq!(segment_at(&[], 5.0), None);
    }

    // ---- tooltips -------------------------------------------------------------

    #[test]
    fn tooltips_carry_name_state_and_ratio() {
        let sample = states(&[SegTone::Good, SegTone::Active, SegTone::Pending]);
        let layout = layout_segments(&sample, 300.0, false);
        let paths = vec![
            Some(PathBuf::from("/photos/trip.png")),
            Some(PathBuf::from("/photos/busy.jpg")),
            None,
        ];
        assert_eq!(
            segment_tooltip(&layout[0], &sample, &paths),
            "trip.png — done — 50 % saved"
        );
        assert_eq!(
            segment_tooltip(&layout[1], &sample, &paths),
            "busy.jpg — converting…"
        );
        assert_eq!(
            segment_tooltip(&layout[2], &sample, &paths),
            "1 file — pending"
        );
        // merged runs show a count instead of one name
        let merged = layout_segments(&states(&[SegTone::Pending, SegTone::Pending]), 4.0, false);
        assert_eq!(
            segment_tooltip(
                &merged[0],
                &states(&[SegTone::Pending, SegTone::Pending]),
                &paths
            ),
            "2 files — pending"
        );
        // grew states name the problem instead of a percentage
        let grew = states(&[SegTone::Grew]);
        let grew_layout = layout_segments(&grew, 100.0, false);
        assert_eq!(
            segment_tooltip(&grew_layout[0], &grew, &paths),
            "trip.png — done — larger than input"
        );
    }

    // ---- stats line (plan 10 §phase 1) ----------------------------------------

    #[test]
    fn stats_line_carries_the_ratio_appendix() {
        let stats = FooterStats {
            input_bytes: 146 * 1024 * 1024,
            output_bytes: 91 * 1024 * 1024,
            ok: 1,
            skipped: 0,
            errors: 0,
        };
        let line = stats_line(&stats, None);
        assert!(
            line.contains("→ 91.00MiB (62% · saved 55.00MiB) |"),
            "appendix directly after the sizes: {line}"
        );
        // grew names the growth instead of savings
        let grew = FooterStats {
            input_bytes: 1000,
            output_bytes: 1200,
            ..stats
        };
        assert!(
            stats_line(&grew, None).contains("(120% · grew 200B)"),
            "larger outputs are honest about it"
        );
        // zero-size input: no appendix at all (undefined ratio)
        let empty = FooterStats {
            input_bytes: 0,
            output_bytes: 0,
            ..stats
        };
        assert!(!stats_line(&empty, None).contains('('));
        // parity with the report label (same helper, same string)
        assert!(
            stats_line(&stats, None).contains(&crate::ratio::ratio_label(
                stats.input_bytes,
                stats.output_bytes
            ))
        );
    }
}
