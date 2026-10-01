//! PSNR (peak signal-to-noise ratio) engine (plan 10 §phase 2): the
//! cheap, familiar baseline — ~40 lines, no dependencies. Computed over
//! all four RGBA channels of the bounded buffers; identical images
//! shortcut to the 100 dB cap.

use super::{MetricResult, QualityMetric};

/// Highest reported PSNR in dB (real cap of 8-bit signals is ~96 dB;
/// 100 reads as "identical" in [`super::interpret_psnr`]).
pub const PSNR_CAP_DB: f32 = 100.0;

/// Peak signal of 8-bit samples.
const MAX_SIGNAL: f64 = 255.0;

/// PSNR engine (stateless).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Psnr;

impl Psnr {
    /// Raw PSNR in dB between two **same-sized** RGBA buffers (identical
    /// input yields the cap).
    #[must_use]
    pub fn psnr_db(a: &image::RgbaImage, b: &image::RgbaImage) -> f32 {
        if a.dimensions() != b.dimensions() {
            return 0.0;
        }
        let mut sum_sq = 0.0_f64;
        for (pa, pb) in a.pixels().zip(b.pixels()) {
            for channel in 0..4 {
                let diff = f64::from(pa.0[channel]) - f64::from(pb.0[channel]);
                sum_sq += diff * diff;
            }
        }
        if sum_sq == 0.0 {
            return PSNR_CAP_DB;
        }
        let mse = sum_sq / (a.width() as f64 * a.height() as f64 * 4.0);
        let psnr = 10.0 * (MAX_SIGNAL * MAX_SIGNAL / mse).log10();
        (psnr as f32).min(PSNR_CAP_DB)
    }
}

impl QualityMetric for Psnr {
    fn name(&self) -> &'static str {
        "psnr"
    }

    fn compare(&self, a: &image::RgbaImage, b: &image::RgbaImage) -> MetricResult {
        let db = Self::psnr_db(a, b);
        MetricResult {
            engine: self.name(),
            score: db,
            pretty: format!("{db:.1}dB"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    #[test]
    fn identical_images_hit_the_cap() {
        let a = RgbaImage::from_pixel(16, 16, image::Rgba([10, 20, 30, 255]));
        let result = Psnr.compare(&a, &a);
        assert_eq!(result.engine, "psnr");
        assert_eq!(result.score, PSNR_CAP_DB);
        assert_eq!(result.pretty, "100.0dB");
        assert_eq!(result.interpretation(), "identical");
    }

    #[test]
    fn known_offset_yields_the_expected_db() {
        // every sample off by exactly 10 → mse = 100 → psnr = 10·log10(255²/100)
        let a = RgbaImage::from_pixel(8, 8, image::Rgba([100, 100, 100, 255]));
        let b = RgbaImage::from_pixel(8, 8, image::Rgba([110, 110, 110, 245]));
        let db = Psnr::psnr_db(&a, &b);
        // mean squared error over 4 channels: (3·100 + 100)/4 = 100
        let expected = 10.0 * (255.0_f64 * 255.0 / 100.0).log10();
        assert!(
            (f64::from(db) - expected).abs() < 1e-4,
            "{db} vs {expected}"
        );
        assert!(
            (25.0..35.0).contains(&db),
            "a 10-value offset sits in the 'noticeable difference' band"
        );
    }

    #[test]
    fn larger_error_means_lower_db_and_scorer_is_monotonic() {
        let a = RgbaImage::from_pixel(8, 8, image::Rgba([100, 100, 100, 255]));
        let small = RgbaImage::from_pixel(8, 8, image::Rgba([101, 101, 101, 255]));
        let large = RgbaImage::from_pixel(8, 8, image::Rgba([200, 200, 200, 255]));
        let clean = Psnr::psnr_db(&a, &small);
        let noisy = Psnr::psnr_db(&a, &large);
        assert!(clean > noisy);
        assert!(noisy < 25.0, "visible loss band: {noisy}");
    }

    #[test]
    fn dimension_mismatch_reports_zero_not_a_panic() {
        let a = RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 255]));
        let b = RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]));
        assert_eq!(Psnr::psnr_db(&a, &b), 0.0);
    }
}
