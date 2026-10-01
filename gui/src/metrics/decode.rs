//! Memory-bounded decode for metric/inspector comparisons (plan 10
//! §phase 2): files decode through the **core** input decoders
//! ([`byteshaver::input::decode_rgba`] — image-crate formats, JPEG XL via
//! jxl-oxide, HEIF/AVIF where libheif is compiled in; plan 15 F15) behind
//! this module's memory caps (512 MiB RGBA allocation, 16 384 px edge)
//! and a downscale to the requested longest edge with a Triangle
//! (box-like) filter. Both images of a pair go through the **same**
//! geometry: a 1-px rounding mismatch is repaired by resizing `b` to
//! `a`'s exact size.
//!
//! Pure + unit-testable; never called on the UI thread. Failures carry a
//! reason string so metric rows can surface them (plan 15 F19).

use std::path::Path;

use image::imageops::FilterType;
use image::RgbaImage;

/// Decode allocation cap (same budget as the thumbnail worker): the RGBA8
/// buffer of one decode may never exceed this.
pub const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;
/// Hard decode edge cap (accepts the core's 32 K inputs up to here;
/// beyond it the decode fails and the metric reports "unmeasurable"
/// instead of risking an OOM).
pub const MAX_DECODE_EDGE: u32 = 16_384;

/// Decodes `path` to RGBA8, bounded to `max_edge` on the longest edge
/// (aspect preserved, never upscaled). `Err` carries the reason (missing
/// or unsupported file, corrupt data, over the decode caps).
pub fn load_bounded(path: &Path, max_edge: u32) -> Result<RgbaImage, String> {
    // image-crate formats can be capped from the header alone: reject
    // oversized images *before* spending the decode's allocation
    if let Ok((width, height)) = header_dimensions(path) {
        check_caps(width, height)?;
    }
    let image = byteshaver::input::decode_rgba(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    // formats without a sniffable header (jxl, heif) can only be capped
    // after the decode — same limits, applied before any downscale
    check_caps(image.width(), image.height())?;
    Ok(downscale(image, max_edge))
}

/// Header-only `(width, height)` via the image crate; errors for unknown
/// formats (jxl/heif) and unreadable files — those fall through to the
/// full decode, which reports its own reason.
fn header_dimensions(path: &Path) -> Result<(u32, u32), ()> {
    let file = std::fs::File::open(path).map_err(|_| ())?;
    let mut reader = image::ImageReader::new(std::io::BufReader::new(file));
    reader.no_limits();
    let reader = reader.with_guessed_format().map_err(|_| ())?;
    reader.into_dimensions().map_err(|_| ())
}

/// The decode caps, phrased as the "too large" failure the image-rs
/// limits used to produce (plan 15 F15: checked on the dimensions *before*
/// downscaling, early for sniffable formats).
fn check_caps(width: u32, height: u32) -> Result<(), String> {
    if width > MAX_DECODE_EDGE || height > MAX_DECODE_EDGE {
        return Err(format!(
            "image too large: {width}×{height} px (decode cap is {MAX_DECODE_EDGE} px per edge)"
        ));
    }
    let alloc = u64::from(width) * u64::from(height) * 4;
    if alloc > MAX_DECODE_ALLOC {
        return Err(format!(
            "image too large: {width}×{height} px needs {alloc} B of RGBA8 (decode cap is {MAX_DECODE_ALLOC} B)"
        ));
    }
    Ok(())
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
/// `Err` carries the first decode failure's reason.
pub fn load_pair(
    input: &Path,
    output: &Path,
    max_edge: u32,
) -> Result<(RgbaImage, RgbaImage), String> {
    let a = load_bounded(input, max_edge)?;
    let b = load_bounded(output, max_edge)?;
    Ok(normalize_pair(a, b))
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
        std::fs::write(
            &tiny_path,
            png_of(&RgbaImage::from_pixel(4, 4, Rgba([1, 1, 1, 255]))),
        )
        .expect("write tiny");
        let (a, b) = load_pair(&tiny_path, &tiny_path, 256).expect("decodes");
        assert_eq!(a.dimensions(), (4, 4));
        assert_eq!(a.dimensions(), b.dimensions());

        // failures carry a reason (missing, junk)
        assert!(load_pair(&dir.join("missing.png"), &output_path, 256)
            .expect_err("missing input")
            .contains("missing.png"));
        let junk = dir.join("junk.png");
        std::fs::write(&junk, b"not an image at all").expect("write junk");
        assert!(load_pair(&input_path, &junk, 256)
            .expect_err("junk output")
            .to_lowercase()
            .contains("format"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_bounded_rejects_over_cap_images_before_downscaling() {
        let dir = std::env::temp_dir().join(format!("byteshaver-metric-caps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        // one pixel past the edge cap: the header check must reject it
        // before the decode allocation happens
        let wide = dir.join("wide.png");
        std::fs::write(
            &wide,
            png_of(&RgbaImage::from_pixel(MAX_DECODE_EDGE + 1, 2, Rgba([1, 2, 3, 255]))),
        )
        .expect("write wide");
        let error = load_bounded(&wide, 4096).expect_err("over the edge cap");
        assert!(error.contains("too large"), "{error}");
        assert!(error.contains(&format!("{}", MAX_DECODE_EDGE + 1)), "{error}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_bounded_decodes_jxl_outputs_via_the_core_decoders() {
        use byteshaver::config::{EncoderConfig, JxlOptions};
        use byteshaver::converter::{EncoderRegistry, ThreadBudget};
        use byteshaver::input::{ImageContent, SourceImage};
        use byteshaver::metadata::ImageMetadata;

        let dir = std::env::temp_dir().join(format!("byteshaver-metric-jxl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let rgba = RgbaImage::from_pixel(21, 13, Rgba([80, 90, 100, 255]));
        let path = dir.join("out.jxl");
        let encoder = EncoderRegistry::build(
            &EncoderConfig::Jxl(JxlOptions {
                lossless: true,
                effort: 1,
                ..JxlOptions::default()
            }),
            ThreadBudget::global(),
        );
        let encoded = encoder
            .encode(&SourceImage {
                content: ImageContent::Still(image::DynamicImage::ImageRgba8(rgba.clone())),
                metadata: ImageMetadata::default(),
                source_format: byteshaver::format::ImageFormat::Png,
                source_path: path.clone(),
            })
            .expect("encode jxl fixture");
        std::fs::write(&path, encoded).expect("write jxl");

        // the image crate cannot read jxl; the core dispatch must
        let decoded = load_bounded(&path, 4096).expect("jxl decodes (plan 15 F15)");
        assert_eq!(decoded.dimensions(), (21, 13));
        assert_eq!(decoded.as_raw(), rgba.as_raw(), "lossless round trip");

        // and it flows through the pair loader like any other output
        let input = dir.join("in.png");
        std::fs::write(&input, png_of(&rgba)).expect("write input png");
        let (a, b) = load_pair(&input, &path, 4096).expect("pair decodes");
        assert_eq!(a.dimensions(), b.dimensions());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
