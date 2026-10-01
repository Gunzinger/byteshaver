//! Memory-bounded decode for metric/inspector comparisons (plan 10
//! §phase 2): files are loaded through `image::io::Limits` (512 MiB
//! allocation cap, 16 384 px edge cap) and downscaled to the requested
//! longest edge with a Triangle (box-like) filter. Both images of a pair
//! go through the **same** geometry: a 1-px rounding mismatch is repaired
//! by resizing `b` to `a`'s exact size.
//!
//! Pure + unit-testable; never called on the UI thread.

use std::path::Path;

use image::imageops::FilterType;
use image::RgbaImage;

/// Decode allocation cap (same budget as the thumbnail worker).
pub const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;
/// Hard decode edge cap (accepts the core's 32 K inputs up to here;
/// beyond it the decode fails and the metric reports "unmeasurable"
/// instead of risking an OOM).
pub const MAX_DECODE_EDGE: u32 = 16_384;

/// Decodes `path` to RGBA8, bounded to `max_edge` on the longest edge
/// (aspect preserved, never upscaled). `None` on any failure (missing or
/// unsupported file, corrupt data, allocation-limit hit).
#[must_use]
pub fn load_bounded(path: &Path, max_edge: u32) -> Option<RgbaImage> {
    let mut reader = image::ImageReader::open(path).ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits.max_image_width = Some(MAX_DECODE_EDGE);
    limits.max_image_height = Some(MAX_DECODE_EDGE);
    reader.limits(limits);
    let reader = reader.with_guessed_format().ok()?;
    let image = reader.decode().ok()?.to_rgba8();
    Some(downscale(image, max_edge))
}

/// Downscales to `max_edge` on the longest edge (aspect preserved, never
/// upscaled, never zero-edged) with a Triangle filter — fast and
/// box-like enough for metrics.
#[must_use]
pub fn downscale(image: RgbaImage, max_edge: u32) -> RgbaImage {
    let (width, height) = fit_inside(image.width(), image.height(), max_edge);
    if (width, height) == (image.width(), image.height()) {
        return image;
    }
    image::imageops::resize(&image, width, height, FilterType::Triangle)
}

/// Scales `(width, height)` to fit inside `edge` (aspect preserved, no
/// upscaling; zero inputs stay zero → clamped by the caller). Same math
/// as the thumbnail worker's helper, kept local so the metric module is
/// self-contained.
#[must_use]
pub fn fit_inside(width: u32, height: u32, edge: u32) -> (u32, u32) {
    if width == 0 || height == 0 || (width <= edge && height <= edge) {
        return (width, height);
    }
    if width >= height {
        (
            edge,
            ((u64::from(height) * u64::from(edge)) / u64::from(width)).max(1) as u32,
        )
    } else {
        (
            ((u64::from(width) * u64::from(edge)) / u64::from(height)).max(1) as u32,
            edge,
        )
    }
}

/// Geometry normalization of a comparison pair (plan 10 §phase 2): the
/// output is compared at the *input's* bounded dimensions — aspect is
/// preserved by the encoders, so only a 1-px rounding mismatch remains,
/// repaired by resizing `b` to `a`'s exact size. Returns the pair in
/// `(a, b)` order with guaranteed equal dimensions.
#[must_use]
pub fn normalize_pair(a: RgbaImage, b: RgbaImage) -> (RgbaImage, RgbaImage) {
    if b.dimensions() == a.dimensions() {
        return (a, b);
    }
    let resized = image::imageops::resize(&b, a.width(), a.height(), FilterType::Triangle);
    (a, resized)
}

/// Loads and normalizes a comparison pair in one step (both decodes
/// bounded to `max_edge`, then geometry-matched to the input's size).
#[must_use]
pub fn load_pair(input: &Path, output: &Path, max_edge: u32) -> Option<(RgbaImage, RgbaImage)> {
    let a = load_bounded(input, max_edge)?;
    let b = load_bounded(output, max_edge)?;
    Some(normalize_pair(a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// Encodes an RGBA buffer as PNG bytes.
    fn png_of(image: &RgbaImage) -> Vec<u8> {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image.clone())
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode fixture");
        png
    }

    #[test]
    fn fit_inside_preserves_aspect_and_never_upscales() {
        assert_eq!(fit_inside(8000, 6000, 4096), (4096, 3072));
        assert_eq!(fit_inside(6000, 8000, 4096), (3072, 4096));
        assert_eq!(fit_inside(2048, 1000, 4096), (2048, 1000), "no upscale");
        assert_eq!(fit_inside(0, 10, 4096), (0, 10));
        // extreme aspect ratios never produce a zero edge
        assert_eq!(fit_inside(20_000, 3, 4096), (4096, 1));
        assert_eq!(fit_inside(3, 20_000, 4096), (1, 4096));
    }

    #[test]
    fn downscale_caps_the_longest_edge() {
        let big = RgbaImage::from_pixel(5000, 2500, Rgba([9, 9, 9, 255]));
        let small = downscale(big, 4096);
        assert_eq!((small.width(), small.height()), (4096, 2048));
        // already small enough: returned as-is
        let tiny = RgbaImage::from_pixel(8, 8, Rgba([1, 2, 3, 255]));
        assert_eq!(downscale(tiny, 4096).width(), 8);
    }

    #[test]
    fn normalize_pair_repairs_rounding_mismatches() {
        let a = RgbaImage::from_pixel(4096, 4096, Rgba([0, 0, 0, 255]));
        let b = RgbaImage::from_pixel(4097, 4096, Rgba([0, 0, 0, 255]));
        let (a, b) = normalize_pair(a, b);
        assert_eq!(a.dimensions(), (4096, 4096));
        assert_eq!(b.dimensions(), (4096, 4096), "b resized to a's size");
        // matching sizes pass through untouched
        let (a, b) = normalize_pair(a, b);
        assert_eq!(a.dimensions(), b.dimensions());
    }

    #[test]
    fn bounded_decode_normalizes_odd_pairs() {
        let dir =
            std::env::temp_dir().join(format!("byteshaver-metric-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        // odd sizes: input 3001×999, output 3000×999 (encoder rounding)
        let input_path = dir.join("in.png");
        let output_path = dir.join("out.png");
        std::fs::write(
            &input_path,
            png_of(&RgbaImage::from_pixel(3001, 999, Rgba([50, 60, 70, 255]))),
        )
        .expect("write input");
        std::fs::write(
            &output_path,
            png_of(&RgbaImage::from_pixel(3000, 999, Rgba([50, 60, 70, 255]))),
        )
        .expect("write output");

        let (a, b) = load_pair(&input_path, &output_path, 256).expect("decodes");
        assert_eq!(a.dimensions(), (256, 85), "bounded to the longest edge");
        assert_eq!(a.dimensions(), b.dimensions(), "rounding repaired");

        // small images pass through unrescaled
        let tiny_path = dir.join("tiny.png");
        std::fs::write(&tiny_path, png_of(&RgbaImage::from_pixel(4, 4, Rgba([1, 1, 1, 255]))))
            .expect("write tiny");
        let (a, b) = load_pair(&tiny_path, &tiny_path, 256).expect("decodes");
        assert_eq!(a.dimensions(), (4, 4));
        assert_eq!(a.dimensions(), b.dimensions());

        // failures → None (missing, junk, limit hit)
        assert_eq!(load_pair(&dir.join("missing.png"), &output_path, 256), None);
        let junk = dir.join("junk.png");
        std::fs::write(&junk, b"not an image at all").expect("write junk");
        assert_eq!(load_pair(&input_path, &junk, 256), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
