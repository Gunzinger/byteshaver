//! DSSIM (multi-scale SSIM) engine (plan 10 §phase 2, decision D1):
//! wraps the pure-Rust `dssim` crate (kornelski). Score `0.0` = identical,
//! higher = worse (see [`super::interpret_dssim`]). Pixels go in as
//! non-premultiplied sRGB8 RGBA via `create_image_rgba`.

use super::{MetricResult, QualityMetric};

/// DSSIM engine holding the reusable analyzer context (scale weights).
#[derive(Debug)]
pub struct DssimEngine {
    attr: dssim::Dssim,
}

impl Default for DssimEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl DssimEngine {
    /// Creates the analyzer with the crate's default scale weights.
    #[must_use]
    pub fn new() -> Self {
        DssimEngine {
            attr: dssim::Dssim::new(),
        }
    }

    /// Raw DSSIM score of two **same-sized** RGBA buffers (0.0 = identical).
    #[must_use]
    pub fn score(&self, a: &image::RgbaImage, b: &image::RgbaImage) -> f32 {
        if a.dimensions() != b.dimensions() {
            return f32::INFINITY;
        }
        let Some(original) = self.create(a) else {
            return f32::INFINITY;
        };
        let Some(modified) = self.create(b) else {
            return f32::INFINITY;
        };
        let (val, _) = self.attr.compare(&original, &modified);
        f64::from(val) as f32
    }

    /// Builds the analyzer's image representation from an RGBA8 buffer.
    fn create(&self, image: &image::RgbaImage) -> Option<dssim::DssimImage<f32>> {
        use rgb::FromSlice;
        let pixels: &[rgb::RGBA<u8>] = image.as_raw().as_rgba();
        self.attr
            .create_image_rgba(pixels, image.width() as usize, image.height() as usize)
    }
}

impl QualityMetric for DssimEngine {
    fn name(&self) -> &'static str {
        "dssim"
    }

    fn compare(&self, a: &image::RgbaImage, b: &image::RgbaImage) -> MetricResult {
        let score = self.score(a, b);
        MetricResult {
            engine: self.name(),
            score,
            pretty: format!("{score:.4}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    #[test]
    fn identical_images_score_zero() {
        let a = RgbaImage::from_pixel(32, 32, image::Rgba([10, 20, 30, 255]));
        let engine = DssimEngine::new();
        let result = engine.compare(&a, &a);
        assert_eq!(result.engine, "dssim");
        assert!(
            result.score.abs() < 1e-6,
            "identical images must score ~0, got {}",
            result.score
        );
        assert_eq!(result.interpretation(), "identical");
    }

    #[test]
    fn worse_damage_scores_higher() {
        let a = RgbaImage::from_fn(32, 32, |x, y| {
            image::Rgba([((x * 8) % 256) as u8, ((y * 8) % 256) as u8, 60, 255])
        });
        // small perturbation
        let small = RgbaImage::from_fn(32, 32, |x, y| {
            image::Rgba([
                ((x * 8) % 256) as u8,
                ((y * 8) % 256) as u8,
                (60 + (x % 4)) as u8,
                255,
            ])
        });
        // heavy noise
        let heavy = RgbaImage::from_fn(32, 32, |x, y| {
            image::Rgba([
                (((x * 8) % 256) as u8).wrapping_add(90),
                ((y * 8) % 256) as u8,
                200,
                255,
            ])
        });
        let engine = DssimEngine::new();
        let base = engine.score(&a, &small);
        let bad = engine.score(&a, &heavy);
        assert!(base > 0.0, "some difference detected: {base}");
        assert!(bad > base, "heavy noise must score worse: {bad} vs {base}");
    }

    #[test]
    fn tiny_and_grayscale_images_are_handled() {
        // 1×1 must not panic (downsampling bottoms out) and stays finite
        let a = RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
        let b = RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let engine = DssimEngine::new();
        let result = engine.compare(&a, &b);
        assert!(result.score.is_finite(), "1×1 compare stays finite");
        // tiny-but-not-degenerate sizes behave normally
        let a = RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 255]));
        let b = RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 255]));
        assert!(engine.score(&a, &b) > 0.0);
    }
}
