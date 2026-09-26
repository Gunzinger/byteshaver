//! EXIF orientation bake-in: turns pixel data upright when the Orientation
//! tag would be lost by the policy.

use image::DynamicImage;
use image::imageops::{flip_horizontal, flip_vertical, rotate90, rotate180, rotate270};

/// Applies the EXIF orientation value (1–8) to the pixels of `img`,
/// producing the upright image a viewer would display.
///
/// Values outside 1–8 (and 1 itself) return the image unchanged.
/// The transforms follow the EXIF spec (JEITA CP-3451, Orientation tag):
/// pure rotations and mirrored rotations, composed from
/// [`image::imageops`] primitives.
#[must_use]
pub fn apply_orientation(img: &DynamicImage, orientation: u8) -> DynamicImage {
    match orientation {
        1 => img.clone(),
        2 => DynamicImage::from(flip_horizontal(img)),
        3 => DynamicImage::from(rotate180(img)),
        4 => DynamicImage::from(flip_vertical(img)),
        // mirrored, then rotated 270° CW == transpose across the main diagonal
        5 => DynamicImage::from(rotate270(&flip_horizontal(img))),
        6 => DynamicImage::from(rotate90(img)),
        // mirrored, then rotated 90° CW == transverse diagonal
        7 => DynamicImage::from(rotate90(&flip_horizontal(img))),
        8 => DynamicImage::from(rotate270(img)),
        _ => img.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, Rgba};

    /// 2x1 image with a red left and a blue right half.
    fn horizontal_test_image() -> DynamicImage {
        let mut buffer = image::RgbaImage::new(2, 1);
        buffer.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        buffer.put_pixel(1, 0, Rgba([0, 0, 255, 255]));
        DynamicImage::ImageRgba8(buffer)
    }

    fn pixel_at(img: &DynamicImage, x: u32, y: u32) -> Rgba<u8> {
        img.get_pixel(x, y)
    }

    #[test]
    fn orientation_one_and_invalid_are_no_ops() {
        let img = horizontal_test_image();
        let applied = apply_orientation(&img, 1);
        assert_eq!(
            applied.as_rgba8().expect("rgba"),
            img.as_rgba8().expect("rgba")
        );

        let untouched = apply_orientation(&img, 42);
        assert_eq!(
            untouched.as_rgba8().expect("rgba"),
            img.as_rgba8().expect("rgba")
        );
    }

    #[test]
    fn orientation_six_rotates_90_cw() {
        let img = horizontal_test_image();
        let rotated = apply_orientation(&img, 6);
        assert_eq!((rotated.width(), rotated.height()), (1, 2));
        // 90° CW: the left (red) half ends up on top
        assert_eq!(pixel_at(&rotated, 0, 0), Rgba([255, 0, 0, 255]));
        assert_eq!(pixel_at(&rotated, 0, 1), Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn orientation_eight_rotates_270_cw() {
        let img = horizontal_test_image();
        let rotated = apply_orientation(&img, 8);
        assert_eq!((rotated.width(), rotated.height()), (1, 2));
        // 270° CW: the left (red) half ends up at the bottom
        assert_eq!(pixel_at(&rotated, 0, 0), Rgba([0, 0, 255, 255]));
        assert_eq!(pixel_at(&rotated, 0, 1), Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn orientation_three_flips_180() {
        let img = horizontal_test_image();
        let rotated = apply_orientation(&img, 3);
        assert_eq!(pixel_at(&rotated, 0, 0), Rgba([0, 0, 255, 255]));
        assert_eq!(pixel_at(&rotated, 1, 0), Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn orientation_two_and_four_mirror() {
        let img = horizontal_test_image();
        let mirrored = apply_orientation(&img, 2);
        assert_eq!(pixel_at(&mirrored, 0, 0), Rgba([0, 0, 255, 255]));

        let mirrored = apply_orientation(&img, 4);
        assert_eq!(pixel_at(&mirrored, 0, 0), Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn orientation_five_and_seven_transpose() {
        let mut buffer = image::RgbaImage::new(2, 2);
        let mut counter = 0u8;
        for y in 0..2 {
            for x in 0..2 {
                buffer.put_pixel(x, y, Rgba([counter, 0, 0, 255]));
                counter += 1;
            }
        }
        let img = DynamicImage::ImageRgba8(buffer);

        // transpose: (x, y) -> (y, x)
        let transposed = apply_orientation(&img, 5);
        assert_eq!(pixel_at(&transposed, 1, 0), Rgba([2, 0, 0, 255]));
        assert_eq!(pixel_at(&transposed, 0, 1), Rgba([1, 0, 0, 255]));

        // transverse: (x, y) -> (H-1-y, W-1-x)
        let transverse = apply_orientation(&img, 7);
        assert_eq!(pixel_at(&transverse, 0, 0), Rgba([3, 0, 0, 255]));
        assert_eq!(pixel_at(&transverse, 1, 1), Rgba([0, 0, 0, 255]));
    }
}
