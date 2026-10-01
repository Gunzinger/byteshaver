//! Perceptual quality measurement of a conversion (plan 10 §phase 2):
//! engines ([`QualityMetric`]), the bounded-decode pipeline
//! ([`decode`]), the background worker ([`worker`]) and the pure
//! presentation helpers ([`interpret`], [`aggregate_line`]).
//!
//! The core parts are **egui-free** so everything is headless-testable;
//! only `worker::MetricState::poll` touches `egui::Context` (to upload
//! inspector textures on receipt, mirroring the thumb worker's pattern).
//!
//! # Memory guardrail
//!
//! The core supports ~1 GiB / 32 K px inputs; decoding two of those to
//! RGBA would need gigabytes. **All metric computation runs on bounded
//! decodes** (512 MiB allocation cap, 16 384 px edge cap, then a
//! downscale to `metric_max_edge`, default 4096). Metrics on downscaled
//! copies understate fine-texture loss — the same trade-off DSSIM's own
//! multi-scale design makes; the setting can be raised at the user's
//! risk (surfaced in the table tooltip).

pub mod decode;
pub mod dssim;
pub mod psnr;
pub mod worker;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use self::dssim::DssimEngine;
pub use self::psnr::Psnr;
pub use self::worker::{MetricEntry, MetricState};

/// Longest edge metrics are computed at by default (settings-exposed,
/// clamped to 256..=16384). 4096 px ≈ 67 MiB per RGBA buffer.
pub const DEFAULT_METRIC_MAX_EDGE: u32 = 4096;

/// Clamp for the settings-exposed metric edge (very small values make the
/// numbers meaningless, huge ones defeat the guardrail).
#[must_use]
pub fn clamp_metric_max_edge(edge: u32) -> u32 {
    edge.clamp(256, 16_384)
}

/// When quality metrics are computed (persisted setting; plan 10 §phase 2,
/// decision D2: the default is **Manual** — no surprise CPU cost).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetricMode {
    /// Never (no menu action, no auto run).
    Off,
    /// On demand, from a row's context menu (default).
    #[default]
    Manual,
    /// Automatically after every finished run for all successful files.
    AutoAfterRun,
}

/// Which [`QualityMetric`] engine measures (persisted setting; DSSIM is
/// the plan-recommended default, PSNR the dependency-free baseline).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetricEngineChoice {
    /// Multi-scale SSIM, score 0.0 = identical (default).
    #[default]
    Dssim,
    /// Peak signal-to-noise ratio in dB, cap 100.
    Psnr,
}

impl MetricEngineChoice {
    /// Fresh engine instance of this choice.
    #[must_use]
    pub fn engine(self) -> Box<dyn QualityMetric> {
        match self {
            MetricEngineChoice::Dssim => Box::new(DssimEngine::new()),
            MetricEngineChoice::Psnr => Box::new(Psnr),
        }
    }
}

/// One comparison engine (plan 10 §phase 2 trait). Engines are cheap to
/// construct and stateless; the worker builds one per job from the
/// request's [`MetricEngineChoice`].
pub trait QualityMetric: Send + Sync {
    /// Short lowercase engine id used in row texts (`"dssim"`, `"psnr"`).
    fn name(&self) -> &'static str;
    /// Compares both images as sRGB8 RGBA buffers **already bounded and
    /// geometry-normalized** (same dimensions) — see [`decode`].
    fn compare(&self, a: &image::RgbaImage, b: &image::RgbaImage) -> MetricResult;
}

/// Result of one comparison.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricResult {
    /// Engine id (see [`QualityMetric::name`]).
    pub engine: &'static str,
    /// Raw score (dssim: 0 = identical, higher = worse; psnr: dB, cap
    /// 100 = identical).
    pub score: f32,
    /// Human-readable one-liner, e.g. `dssim 0.0120` / `psnr 41.2dB`.
    pub pretty: String,
}

impl MetricResult {
    /// The interpretation line for this result (see [`interpret`]).
    #[must_use]
    pub fn interpretation(&self) -> &'static str {
        interpret(self.engine, self.score)
    }
}

/// Plain-language reading of a score (plan 10 §phase 2: pinned wording,
/// unit-tested thresholds; shown in row tooltips).
#[must_use]
pub fn interpret(engine: &str, score: f32) -> &'static str {
    match engine {
        "dssim" => interpret_dssim(score),
        _ => interpret_psnr(score),
    }
}

/// DSSIM scale: `0 = identical; <0.005 excellent; <0.05 noticeable;
/// higher = visible loss`.
#[must_use]
pub fn interpret_dssim(score: f32) -> &'static str {
    if score <= 0.0 {
        "identical"
    } else if score < 0.005 {
        "excellent match"
    } else if score < 0.05 {
        "noticeable difference"
    } else {
        "visible loss"
    }
}

/// PSNR scale (dB, cap 100): ≥ 50 excellent, ≥ 35 minor, ≥ 25 noticeable,
/// below visible loss.
#[must_use]
pub fn interpret_psnr(db: f32) -> &'static str {
    if db >= 100.0 {
        "identical"
    } else if db >= 50.0 {
        "excellent match"
    } else if db >= 35.0 {
        "minor differences"
    } else if db >= 25.0 {
        "noticeable difference"
    } else {
        "visible loss"
    }
}

/// Aggregated metric line for the report totals block: per-engine mean
/// over the measured files, e.g.
/// `quality: dssim 0.0082 mean (12 files) · psnr 41.3dB mean (12 files)`.
/// `None` when nothing was measured.
#[must_use]
pub fn aggregate_line(results: &HashMap<PathBuf, MetricEntry>) -> Option<String> {
    // group scores by engine (a map can mix engines across settings flips)
    let mut groups: Vec<(&'static str, Vec<f32>)> = Vec::new();
    for entry in results.values() {
        let engine = entry.result.engine;
        let group = groups.iter_mut().find(|(name, _)| *name == engine);
        match group {
            Some((_, scores)) => scores.push(entry.result.score),
            None => groups.push((engine, vec![entry.result.score])),
        }
    }
    groups.sort_by_key(|(name, _)| *name);
    let mut line = String::new();
    for (name, scores) in groups {
        let mean = scores.iter().sum::<f32>() / scores.len() as f32;
        if !line.is_empty() {
            line.push_str(" · ");
        }
        line.push_str(&format!(
            "{} {} mean ({} file{})",
            name,
            pretty_score(name, mean),
            scores.len(),
            if scores.len() == 1 { "" } else { "s" }
        ));
    }
    if line.is_empty() {
        None
    } else {
        Some(format!("quality: {line}"))
    }
}

/// Formats a mean score the way the engine's own [`MetricResult::pretty`]
/// does (without the engine prefix).
#[must_use]
pub fn pretty_score(engine: &str, score: f32) -> String {
    match engine {
        "dssim" => format!("{score:.4}"),
        _ => format!("{score:.1}dB"),
    }
}

/// The `(input, output)` pairs to auto-measure after a run (pure; plan 10
/// §phase 2 AutoAfterRun mode): exactly the successfully *encoded* files
/// of the report — skipped/collided/discarded/failed rows have no fresh
/// output worth measuring. Empty unless the mode is `AutoAfterRun`.
#[must_use]
pub fn auto_measure_pairs(report: &byteshaver::job::RunReport, mode: MetricMode) -> Vec<(PathBuf, PathBuf)> {
    if mode != MetricMode::AutoAfterRun {
        return Vec::new();
    }
    report
        .files
        .iter()
        .filter_map(|file| match &file.outcome {
            byteshaver::pipeline::Outcome::Encoded { output_path, .. } => {
                Some((file.path.clone(), output_path.clone()))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpret_thresholds_match_the_pinned_wording() {
        // dssim: 0 = identical; <0.005 excellent; <0.05 noticeable; higher loss
        assert_eq!(interpret_dssim(0.0), "identical");
        assert_eq!(interpret_dssim(0.001), "excellent match");
        assert_eq!(interpret_dssim(0.0049), "excellent match");
        assert_eq!(interpret_dssim(0.005), "noticeable difference");
        assert_eq!(interpret_dssim(0.049), "noticeable difference");
        assert_eq!(interpret_dssim(0.05), "visible loss");
        assert_eq!(interpret_dssim(1.0), "visible loss");
        // psnr: cap 100 = identical
        assert_eq!(interpret_psnr(100.0), "identical");
        assert_eq!(interpret_psnr(60.0), "excellent match");
        assert_eq!(interpret_psnr(50.0), "excellent match");
        assert_eq!(interpret_psnr(40.0), "minor differences");
        assert_eq!(interpret_psnr(35.0), "minor differences");
        assert_eq!(interpret_psnr(30.0), "noticeable difference");
        assert_eq!(interpret_psnr(25.0), "noticeable difference");
        assert_eq!(interpret_psnr(10.0), "visible loss");
        // dispatch by engine id; unknown ids fall back to the dB scale
        assert_eq!(interpret("dssim", 0.0), "identical");
        assert_eq!(interpret("psnr", 100.0), "identical");
        assert_eq!(interpret("unknown", 0.0), "visible loss", "dB fallback");
    }

    #[test]
    fn metric_mode_defaults_to_manual_and_round_trips() {
        assert_eq!(MetricMode::default(), MetricMode::Manual, "decision D2");
        assert_eq!(
            MetricEngineChoice::default(),
            MetricEngineChoice::Dssim,
            "decision D1"
        );
        for (mode, engine) in [
            (MetricMode::Off, MetricEngineChoice::Psnr),
            (MetricMode::AutoAfterRun, MetricEngineChoice::Dssim),
        ] {
            let json = serde_json::to_string(&mode).expect("mode");
            assert_eq!(serde_json::from_str::<MetricMode>(&json).expect("mode"), mode);
            let json = serde_json::to_string(&engine).expect("engine");
            assert_eq!(
                serde_json::from_str::<MetricEngineChoice>(&json).expect("engine"),
                engine
            );
        }
    }

    #[test]
    fn clamp_keeps_the_metric_edge_sane() {
        assert_eq!(clamp_metric_max_edge(4096), 4096);
        assert_eq!(clamp_metric_max_edge(1), 256);
        assert_eq!(clamp_metric_max_edge(255), 256);
        assert_eq!(clamp_metric_max_edge(16_384), 16_384);
        assert_eq!(clamp_metric_max_edge(u32::MAX), 16_384);
    }

    #[test]
    fn aggregate_line_means_scores_per_engine() {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        let entry = |engine: &'static str, score: f32| MetricEntry {
            output_mtime: None,
            result: MetricResult {
                engine,
                score,
                pretty: String::new(),
            },
        };
        assert_eq!(aggregate_line(&map), None, "nothing measured");
        map.insert(PathBuf::from("a"), entry("dssim", 0.010));
        map.insert(PathBuf::from("b"), entry("dssim", 0.020));
        assert_eq!(
            aggregate_line(&map),
            Some("quality: dssim 0.0150 mean (2 files)".to_string())
        );
        map.insert(PathBuf::from("c"), entry("psnr", 40.0));
        let line = aggregate_line(&map).expect("two engines");
        assert!(line.contains("dssim 0.0150 mean (2 files)"), "{line}");
        assert!(line.contains("psnr 40.0dB mean (1 file)"), "{line}");
    }

    #[test]
    fn auto_measure_pairs_only_when_enabled_and_only_encoded() {
        use byteshaver::job::{RunReport, Totals};
        use byteshaver::pipeline::FileResult;
        use byteshaver::pipeline::Outcome;
        let report = RunReport {
            elapsed: std::time::Duration::ZERO,
            files: vec![
                FileResult {
                    path: PathBuf::from("/x/a.png"),
                    outcome: Outcome::Encoded {
                        input_size: 10,
                        output_size: 4,
                        metadata_dropped: false,
                        output_path: PathBuf::from("/x/out/a.webp"),
                    },
                },
                FileResult {
                    path: PathBuf::from("/x/b.png"),
                    outcome: Outcome::Error("boom".to_string()),
                },
                FileResult {
                    path: PathBuf::from("/x/c.png"),
                    outcome: Outcome::SkippedExisting {
                        input_size: 10,
                        existing_size: 4,
                        output_path: PathBuf::from("/x/out/c.webp"),
                    },
                },
            ],
            totals: Totals::default(),
            metadata_dropped_count: 0,
            error: None,
        };
        // off/manual → nothing
        assert!(auto_measure_pairs(&report, MetricMode::Off).is_empty());
        assert!(auto_measure_pairs(&report, MetricMode::Manual).is_empty());
        // auto → exactly the encoded row
        assert_eq!(
            auto_measure_pairs(&report, MetricMode::AutoAfterRun),
            vec![(PathBuf::from("/x/a.png"), PathBuf::from("/x/out/a.webp"))]
        );
    }
}
