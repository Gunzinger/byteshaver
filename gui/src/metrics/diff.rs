//! Pixel math of the visual difference inspector (plan 10 §phase 3):
//! the amplified abs-diff heatmap and the swipe-split geometry. Pure,
//! egui-free, unit-tested on tiny fixtures — the panel only paints the
//! results.

use image::RgbaImage;

/// Colormap stops of the heatmap (black → deep purple → crimson →
/// orange → pale yellow, magma-ish; positions are fractions of the
/// amplified difference in `0..=1`).
const STOPS: [(f32, [u8; 3]); 5] = [
    (0.0, [0, 0, 0]),
    (0.25, [48, 18, 88]),
    (0.5, [180, 45, 90]),
    (0.75, [235, 130, 45]),
    (1.0, [250, 233, 165]),
];

/// Builds the amplified difference heatmap of two **same-sized** images:
/// per-pixel mean absolute difference over RGB, scaled by `amp`
/// (`1.0..=32.0`), clamped and mapped through a 5-stop gradient. Alpha
/// stays opaque. Mismatched sizes yield an empty image (callers
/// normalize first; this is a guard, not a panic).
#[must_use]
pub fn diff_heatmap(a: &RgbaImage, b: &RgbaImage, amp: f32) -> RgbaImage {
    if a.dimensions() != b.dimensions() || a.width() == 0 || a.height() == 0 {
        return RgbaImage::new(0, 0);
    }
    let amp = if amp.is_finite() { amp.max(0.0) } else { 0.0 };
    let mut out = RgbaImage::new(a.width(), a.height());
    for (pa, (pb, po)) in a.pixels().zip(b.pixels().zip(out.pixels_mut())) {
        let diff = mean_abs_diff_rgb(pa.0, pb.0);
        let [r, g, bl] = heat_color(diff * amp / 255.0);
        *po = image::Rgba([r, g, bl, 255]);
    }
    out
}

/// Mean absolute difference of two pixels' RGB channels in `0..=255`
/// (alpha excluded: fully transparent regions would fake differences).
#[must_use]
pub fn mean_abs_diff_rgb(a: [u8; 4], b: [u8; 4]) -> f32 {
    ((f32::from(a[0]) - f32::from(b[0])).abs()
        + (f32::from(a[1]) - f32::from(b[1])).abs()
        + (f32::from(a[2]) - f32::from(b[2])).abs())
        / 3.0
}

/// Maps an intensity in `0.0..=1.0` onto the 5-stop gradient (piecewise
/// linear interpolation; values outside clamp to the end stops).
#[must_use]
pub fn heat_color(t: f32) -> [u8; 3] {
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
    for window in STOPS.windows(2) {
        let (t0, c0) = window[0];
        let (t1, c1) = window[1];
        if t <= t1 {
            let f = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
            return [
                (f32::from(c0[0]) + f * (f32::from(c1[0]) - f32::from(c0[0]))) as u8,
                (f32::from(c0[1]) + f * (f32::from(c1[1]) - f32::from(c0[1]))) as u8,
                (f32::from(c0[2]) + f * (f32::from(c1[2]) - f32::from(c0[2]))) as u8,
            ];
        }
    }
    STOPS[STOPS.len() - 1].1
}

/// Swipe-split pixel position of `fraction` (`0.0..=1.0`) across `width`,
/// clamped so both sides stay grabbable (`1 px`..`width - 1 px`).
#[must_use]
pub fn swipe_split(width: f32, fraction: f32) -> f32 {
    if !width.is_finite() || width <= 0.0 {
        return 0.0;
    }
    if !fraction.is_finite() {
        return width * 0.5;
    }
    (fraction.clamp(0.0, 1.0) * width).clamp(1.0, (width - 1.0).max(1.0))
}

/// Inverse of [`swipe_split`]: pointer `x` (relative to the widget's
/// left) back to a `0.0..=1.0` fraction.
#[must_use]
pub fn swipe_fraction(width: f32, x: f32) -> f32 {
    if !width.is_finite() || width <= 0.0 {
        return 0.5;
    }
    (x / width).clamp(0.0, 1.0)
}

/// Whether a pointer at `x` grabs the handle centered at `split`
/// (± [`HANDLE_RADIUS`] px — a generous touch target).
pub const HANDLE_RADIUS: f32 = 12.0;

/// Hit test of the swipe handle (pure, unit-tested).
#[must_use]
pub fn handle_grab(x: f32, split: f32) -> bool {
    (x - split).abs() <= HANDLE_RADIUS
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn identical_images_are_black_heatmaps() {
        let a = RgbaImage::from_pixel(4, 4, Rgba([30, 40, 50, 255]));
        let heat = diff_heatmap(&a, &a, 8.0);
        assert_eq!(heat.dimensions(), (4, 4));
        assert!(heat.pixels().all(|p| p.0 == [0, 0, 0, 255]));
    }

    #[test]
    fn amplified_differences_light_up_the_map() {
        // right half shifted by 30 in red only
        let a = RgbaImage::from_pixel(4, 4, Rgba([100, 100, 100, 255]));
        let b = RgbaImage::from_fn(4, 4, |x, _| {
            if x < 2 {
                Rgba([100, 100, 100, 255])
            } else {
                Rgba([130, 100, 100, 255])
            }
        });
        // mean abs diff of changed pixels = 30/3 = 10 → t = 10·amp/255
        let low = diff_heatmap(&a, &b, 1.0);
        let high = diff_heatmap(&a, &b, 32.0);
        // unchanged pixels stay black on both
        assert_eq!(low.get_pixel(0, 0).0, [0, 0, 0, 255]);
        // changed pixel: low amp stays dark-ish, high amp clearly brightens
        let low_blue = u16::from(low.get_pixel(3, 0).0[2]);
        let high_blue = u16::from(high.get_pixel(3, 0).0[2]);
        assert!(high_blue > low_blue, "{high_blue} vs {low_blue}");
        assert!(high_blue > 100, "32× amplification lights the pixel up");
        // changed pixels differ from unchanged ones
        assert_ne!(low.get_pixel(3, 0).0, low.get_pixel(0, 0).0);
    }

    #[test]
    fn heatmap_math_on_exact_fixtures() {
        // one pixel off by 90 in red only → mean = 30; amp 1 → t = 30/255
        let a = RgbaImage::from_pixel(2, 1, Rgba([100, 100, 100, 255]));
        let mut b = a.clone();
        b.put_pixel(1, 0, Rgba([190, 100, 100, 255]));
        let heat = diff_heatmap(&a, &b, 1.0);
        // mean abs diff = 90/3 = 30 → t = 30/255 ≈ 0.1176 → between stop 0 and 1
        let expected_t = 30.0 / 255.0;
        let expected = heat_color(expected_t);
        assert_eq!(heat.get_pixel(1, 0).0[..3], expected, "per-pixel math exact");
        // alpha excluded from the difference
        b.put_pixel(1, 0, Rgba([100, 100, 100, 12]));
        assert!(diff_heatmap(&a, &b, 32.0).pixels().all(|p| p.0 == [0, 0, 0, 255]));
    }

    #[test]
    fn gradient_hits_the_five_stops() {
        assert_eq!(heat_color(0.0), [0, 0, 0]);
        assert_eq!(heat_color(1.0), [250, 233, 165]);
        assert_eq!(heat_color(0.25), [48, 18, 88]);
        assert_eq!(heat_color(0.5), [180, 45, 90]);
        assert_eq!(heat_color(0.75), [235, 130, 45]);
        // out-of-range clamps
        assert_eq!(heat_color(-3.0), [0, 0, 0]);
        assert_eq!(heat_color(9.0), [250, 233, 165]);
        assert_eq!(heat_color(f32::NAN), [0, 0, 0]);
    }

    #[test]
    fn mismatched_sizes_yield_an_empty_map() {
        let a = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));
        let b = RgbaImage::from_pixel(2, 2, Rgba([0, 0, 0, 255]));
        assert_eq!(diff_heatmap(&a, &b, 8.0).dimensions(), (0, 0));
    }

    #[test]
    fn swipe_split_clamps_and_round_trips() {
        assert!((swipe_split(100.0, 0.3) - 30.0).abs() < 1e-4);
        assert_eq!(swipe_split(100.0, 0.0), 1.0, "left edge keeps 1 px");
        assert_eq!(swipe_split(100.0, 1.0), 99.0, "right edge keeps 1 px");
        assert_eq!(swipe_split(100.0, -1.0), 1.0);
        assert_eq!(swipe_split(100.0, f32::NAN), 50.0);
        assert_eq!(swipe_split(0.0, 0.5), 0.0);
        // inverse
        assert_eq!(swipe_fraction(100.0, 30.0), 0.3);
        assert_eq!(swipe_fraction(100.0, 500.0), 1.0);
        assert_eq!(swipe_fraction(100.0, -5.0), 0.0);
        // handle grab radius
        assert!(handle_grab(50.0, 50.0));
        assert!(handle_grab(50.0, 61.0));
        assert!(!handle_grab(50.0, 70.0));
    }
}
