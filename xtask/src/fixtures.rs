//! Deterministic fixture generation for `docs/media/fixtures/` (plan 17
//! §5.4): the pinned, byte-stable demo inputs the capture scenes convert on
//! camera.
//!
//! Everything here is procedural or a byte-copy - no time, no randomness,
//! no encoder timestamps - so repeated `cargo media gen-fixtures` runs on
//! the same toolchain produce byte-identical files (verified by tests in
//! this module and by the CI determinism check). Fixtures are inputs only;
//! generated outputs live exclusively in the stage dir and never get
//! committed.
//!
//! Tree (see `docs/media/fixtures/README.md` for the authored guide):
//!
//! ```text
//! photos/   6 real photographs, byte-copies from examples/ (size mix)
//! screens/  dashboard.png (480x320), logo.png (640x360, carries EXIF incl.
//!           GPS, deliberately no Orientation tag)
//! anim/     loading.gif (8 frames), sparkle.png (APNG, 6 frames),
//!           spinner.webp (animated WebP, 8 frames, lossy q80)
//! jxl/      intentionally absent: a JXL encoder would need libjxl; the jxl
//!           scene converts a photo instead (README documents this)
//! ```

use anyhow::{Context, Result};
use exif::experimental::Writer as ExifWriter;
use exif::{Field, In, Rational, Tag, Value};
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use std::borrow::Cow;
use std::io::Cursor;
use std::path::Path;

use crate::util;

/// (repo-relative source, fixture file name). Picked for a size spread:
/// ~73 KB, ~172 KB, ~1.0 MB, ~1.1 MB, ~1.3 MB, ~1.4 MB; 3 JPEG + 3 PNG.
/// Byte-identical copies - the pipeline must demo *real* inputs.
const PHOTOS: &[(&str, &str)] = &[
    ("examples/jpeg/z.jpeg", "z.jpeg"),
    ("examples/jpg/4D71QrY.jpg", "4D71QrY.jpg"),
    ("examples/jpg/179zrSg.jpg", "179zrSg.jpg"),
    ("examples/png/deno.png", "deno.png"),
    ("examples/png/5613.png", "5613.png"),
    ("examples/png/AQ5MKa0.png", "AQ5MKa0.png"),
];

/// Regenerates the whole fixtures tree under `docs/media/fixtures/`.
pub fn gen_fixtures() -> Result<()> {
    let repo = util::repo_root();
    let dir = repo.join("docs/media/fixtures");

    // Only the generated subtrees are wiped; authored files next to them
    // (README.md) survive untouched.
    for sub in ["photos", "screens", "anim"] {
        util::remove_dir_if_exists(&dir.join(sub))?;
        util::ensure_dir(&dir.join(sub))?;
    }

    println!("[gen-fixtures] photos/: byte-copies from examples/");
    copy_photos(&repo, &dir.join("photos"))?;

    println!("[gen-fixtures] screens/: procedural PNGs (deterministic)");
    std::fs::write(dir.join("screens/dashboard.png"), build_dashboard_png()?)
        .context("writing screens/dashboard.png")?;
    std::fs::write(dir.join("screens/logo.png"), build_logo_png()?)
        .context("writing screens/logo.png")?;

    println!("[gen-fixtures] anim/: gif + apng + animated webp (deterministic)");
    std::fs::write(dir.join("anim/loading.gif"), build_loading_gif()?)
        .context("writing anim/loading.gif")?;
    std::fs::write(dir.join("anim/sparkle.png"), build_sparkle_apng()?)
        .context("writing anim/sparkle.png")?;
    std::fs::write(dir.join("anim/spinner.webp"), build_spinner_webp()?)
        .context("writing anim/spinner.webp")?;

    print_tree(&dir)?;
    Ok(())
}

fn copy_photos(repo: &Path, dst: &Path) -> Result<()> {
    for (src_rel, name) in PHOTOS {
        let src = repo.join(src_rel);
        anyhow::ensure!(
            src.is_file(),
            "fixture source {} is missing - the PHOTOS table in fixtures.rs must list existing files",
            src.display()
        );
        std::fs::copy(&src, dst.join(name))
            .with_context(|| format!("copying {} -> {}", src.display(), dst.join(name).display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// drawing helpers (integer math only -> pixel-identical across runs)
// ---------------------------------------------------------------------------

type Pixel = [u8; 4];

/// Vertical two-color gradient.
fn vgradient(img: &mut RgbaImage, top: [u8; 3], bottom: [u8; 3]) {
    let (w, h) = (img.width(), img.height());
    for y in 0..h {
        // integer lerp over the row; (h-1) so the last row hits `bottom`
        let denom = (h - 1).max(1) as u64;
        let row: Pixel = [
            (top[0] as u64 + (bottom[0] as u64 - top[0] as u64) * y as u64 / denom) as u8,
            (top[1] as u64 + (bottom[1] as u64 - top[1] as u64) * y as u64 / denom) as u8,
            (top[2] as u64 + (bottom[2] as u64 - top[2] as u64) * y as u64 / denom) as u8,
            255,
        ];
        for x in 0..w {
            img.put_pixel(x, y, image::Rgba(row));
        }
    }
}

fn fill_rect(img: &mut RgbaImage, x: i64, y: i64, w: i64, h: i64, color: Pixel) {
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    for yy in y.max(0)..(y + h).min(ih) {
        for xx in x.max(0)..(x + w).min(iw) {
            img.put_pixel(xx as u32, yy as u32, image::Rgba(color));
        }
    }
}

fn fill_circle(img: &mut RgbaImage, cx: i64, cy: i64, r: i64, color: Pixel) {
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    for yy in (cy - r).max(0)..(cy + r + 1).min(ih) {
        for xx in (cx - r).max(0)..(cx + r + 1).min(iw) {
            let dx = xx - cx;
            let dy = yy - cy;
            if dx * dx + dy * dy <= r * r {
                img.put_pixel(xx as u32, yy as u32, image::Rgba(color));
            }
        }
    }
}

/// Draws the 2x master for `screens/dashboard.png` (downscaled to 480x320
/// with a triangle filter afterwards, which is what makes the edges look
/// anti-aliased without pulling in a drawing crate).
fn build_dashboard_master() -> RgbaImage {
    let mut img = RgbaImage::new(960, 640);
    vgradient(&mut img, [24, 28, 38], [72, 84, 106]);
    // window header + accent strip
    fill_rect(&mut img, 0, 0, 960, 88, [32, 40, 56, 255]);
    fill_rect(&mut img, 0, 88, 960, 6, [86, 204, 158, 255]);
    // sidebar with menu entries
    fill_rect(&mut img, 0, 94, 220, 546, [28, 33, 45, 255]);
    for i in 0..5 {
        fill_rect(&mut img, 24, 130 + i * 56, 172, 28, [52, 62, 82, 255]);
    }
    // three stat cards with colored top strips
    let strips: [Pixel; 3] = [
        [86, 204, 158, 255],
        [108, 160, 220, 255],
        [222, 120, 120, 255],
    ];
    for (i, color) in strips.iter().enumerate() {
        let x = 250 + i as i64 * 243;
        fill_rect(&mut img, x, 130, 220, 180, [32, 40, 56, 255]);
        fill_rect(&mut img, x, 130, 220, 10, *color);
    }
    // donut chart + secondary circle
    fill_circle(&mut img, 360, 470, 90, [52, 62, 82, 255]);
    fill_circle(&mut img, 360, 470, 58, [86, 204, 158, 255]);
    fill_circle(&mut img, 360, 470, 26, [24, 28, 38, 255]);
    fill_circle(&mut img, 590, 470, 70, [108, 160, 220, 255]);
    fill_circle(&mut img, 590, 470, 30, [24, 28, 38, 255]);
    // mini bar chart
    let heights = [60_i64, 110, 80, 140, 95, 120];
    for (i, h) in heights.iter().enumerate() {
        fill_rect(&mut img, 720 + i as i64 * 36, 560 - h, 22, *h, [108, 160, 220, 255]);
    }
    img
}

/// Draws the 2x master for `screens/logo.png` (640x360 after downscale).
fn build_logo_master() -> RgbaImage {
    let mut img = RgbaImage::new(1280, 720);
    vgradient(&mut img, [14, 17, 24], [38, 50, 70]);
    // badge: light disc, dark ring, accent core
    fill_circle(&mut img, 640, 360, 250, [240, 246, 252, 255]);
    fill_circle(&mut img, 640, 360, 210, [30, 36, 50, 255]);
    fill_circle(&mut img, 640, 360, 110, [86, 204, 158, 255]);
    // crossbars suggesting "shaving down" file sizes
    fill_rect(&mut img, 330, 344, 620, 32, [240, 246, 252, 255]);
    fill_rect(&mut img, 400, 240, 480, 24, [108, 160, 220, 255]);
    fill_rect(&mut img, 400, 456, 480, 24, [108, 160, 220, 255]);
    img
}

/// 480x320 RGBA pixels (2x master -> triangle downscale).
fn build_dashboard_image() -> RgbaImage {
    image::imageops::resize(
        &build_dashboard_master(),
        480,
        320,
        image::imageops::FilterType::Triangle,
    )
}

/// 640x360 RGBA pixels (2x master -> triangle downscale).
fn build_logo_image() -> RgbaImage {
    image::imageops::resize(
        &build_logo_master(),
        640,
        360,
        image::imageops::FilterType::Triangle,
    )
}

// ---------------------------------------------------------------------------
// PNG encoding (static + APNG) with optional eXIf chunk
// ---------------------------------------------------------------------------

/// Encodes an RGBA8 frame as PNG into `out`, optionally with an eXIf chunk
/// (raw TIFF EXIF bytes as produced by `build_logo_exif`).
fn encode_png(out: &mut Vec<u8>, img: &RgbaImage, exif: Option<Vec<u8>>) -> Result<()> {
    let mut info = png::Info::with_size(img.width(), img.height());
    info.exif_metadata = exif.map(Cow::Owned);
    let mut encoder = png::Encoder::with_info(Cursor::new(out), info)
        .context("creating png encoder")?;
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().context("writing png header")?;
    writer
        .write_image_data(img.as_raw())
        .context("writing png data")?;
    writer.finish().context("finishing png")?;
    Ok(())
}

/// Encodes `frames` as an APNG with a fixed per-frame delay (milliseconds).
fn encode_apng(out: &mut Vec<u8>, frames: &[RgbaImage], delay_ms: u16) -> Result<()> {
    let first = frames.first().context("APNG needs at least one frame")?;
    let mut encoder = png::Encoder::new(Cursor::new(out), first.width(), first.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // num_plays = 0 -> loop forever
    encoder.set_animated(frames.len() as u32, 0)?;
    let mut writer = encoder.write_header().context("writing apng header")?;
    for frame in frames {
        // millisecond delay: numerator/denominator of a fraction of a second
        writer.set_frame_delay(delay_ms, 1000)?;
        writer.write_image_data(frame.as_raw())?;
    }
    writer.finish().context("finishing apng")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// EXIF for screens/logo.png
// ---------------------------------------------------------------------------

/// Builds the EXIF blob embedded into `screens/logo.png` (TIFF structure via
/// kamadak-exif's `experimental::Writer`, stored in a PNG `eXIf` chunk).
///
/// Deliberate contents, chosen so the `--exif filter --exif-except gps`
/// demo scene has something meaningful to strip:
/// - Make = "byteshaver", Model = "fixture-gen",
/// - DateTimeOriginal (Exif IFD),
/// - GPSLatitudeRef/Latitude + GPSLongitudeRef/Longitude + GPSVersionID,
/// - **no Orientation tag** - setting Orientation=1 would still introduce a
///   tag the EXIF scenes have to reason about; omitting it entirely keeps
///   every decoder treating the pixels as-is (no rotation surprises).
fn build_logo_exif() -> Result<Vec<u8>> {
    let fields = vec![
        Field {
            tag: Tag::Make,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"byteshaver".to_vec()]),
        },
        Field {
            tag: Tag::Model,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"fixture-gen".to_vec()]),
        },
        Field {
            tag: Tag::DateTimeOriginal,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"2026:01:01 12:00:00".to_vec()]),
        },
        Field {
            tag: Tag::GPSVersionID,
            ifd_num: In::PRIMARY,
            value: Value::Byte(vec![2, 3, 0, 0]),
        },
        Field {
            tag: Tag::GPSLatitudeRef,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"N".to_vec()]),
        },
        // Munich: 48deg 8' 15.348" N, 11deg 34' 5.238" E
        Field {
            tag: Tag::GPSLatitude,
            ifd_num: In::PRIMARY,
            value: Value::Rational(vec![
                Rational { num: 48, denom: 1 },
                Rational { num: 8, denom: 1 },
                Rational { num: 15348, denom: 1000 },
            ]),
        },
        Field {
            tag: Tag::GPSLongitudeRef,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"E".to_vec()]),
        },
        Field {
            tag: Tag::GPSLongitude,
            ifd_num: In::PRIMARY,
            value: Value::Rational(vec![
                Rational { num: 11, denom: 1 },
                Rational { num: 34, denom: 1 },
                Rational { num: 52380, denom: 1000 },
            ]),
        },
    ];
    let mut writer = ExifWriter::new();
    for field in &fields {
        writer.push_field(field);
    }
    let mut cursor = Cursor::new(Vec::new());
    writer.write(&mut cursor, true).context("writing EXIF blob")?;
    Ok(cursor.into_inner())
}

// ---------------------------------------------------------------------------
// animated fixtures
// ---------------------------------------------------------------------------

/// Frame `i` of the loading GIF: an accent ball orbiting a center plus a
/// progress bar that fills over the loop (240x180).
fn loading_frame(i: usize) -> RgbaImage {
    let mut img = RgbaImage::new(240, 180);
    vgradient(&mut img, [26, 31, 42], [48, 58, 76]);
    let step = std::f64::consts::TAU / 8.0;
    let angle = i as f64 * step;
    let cx = 120 + (80.0 * angle.cos()) as i64;
    let cy = 82 + (50.0 * angle.sin()) as i64;
    fill_circle(&mut img, cx, cy, 18, [86, 204, 158, 255]);
    fill_circle(&mut img, cx, cy, 8, [240, 246, 252, 255]);
    fill_rect(&mut img, 20, 150, 200, 12, [52, 62, 82, 255]);
    fill_rect(&mut img, 20, 150, 200 * (i + 1) as i64 / 8, 12, [86, 204, 158, 255]);
    img
}

/// Frame `i` of the APNG: an expanding two-tone ring pulse (320x240).
fn sparkle_frame(i: usize) -> RgbaImage {
    let mut img = RgbaImage::new(320, 240);
    vgradient(&mut img, [20, 24, 34], [44, 54, 72]);
    let r = 20 + i as i64 * 18;
    let color: Pixel = if i.is_multiple_of(2) {
        [86, 204, 158, 255]
    } else {
        [108, 160, 220, 255]
    };
    fill_circle(&mut img, 160, 120, r, color);
    fill_circle(&mut img, 160, 120, (r * 2 / 3).max(1), [20, 24, 34, 255]);
    img
}

/// Frame `i` of the animated WebP: a three-dot spinner chasing around a
/// circle, trailing dots fading via alpha (240x180).
fn spinner_frame(i: usize) -> RgbaImage {
    let mut img = RgbaImage::new(240, 180);
    vgradient(&mut img, [24, 28, 38], [44, 52, 68]);
    let step = std::f64::consts::TAU / 8.0;
    let base = i as f64 * step;
    for k in 0..3_u32 {
        let angle = base - k as f64 * (std::f64::consts::TAU / 6.0);
        let cx = 120 + (52.0 * angle.cos()) as i64;
        let cy = 84 + (52.0 * angle.sin()) as i64;
        let alpha = 255u32.saturating_sub(k * 70);
        fill_circle(&mut img, cx, cy, 14, [86, 204, 158, alpha as u8]);
    }
    fill_rect(&mut img, 70, 150, 100, 10, [52, 62, 82, 255]);
    img
}

fn build_dashboard_png() -> Result<Vec<u8>> {
    let mut out = Vec::new();
    encode_png(&mut out, &build_dashboard_image(), None)?;
    Ok(out)
}

fn build_logo_png() -> Result<Vec<u8>> {
    let mut out = Vec::new();
    encode_png(&mut out, &build_logo_image(), Some(build_logo_exif()?))?;
    Ok(out)
}

fn build_loading_gif() -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = GifEncoder::new(Cursor::new(&mut out));
    encoder.set_repeat(Repeat::Infinite)?;
    for i in 0..8 {
        // 100ms per frame (numer/denom of a second); `from_parts` is the
        // only way to attach a delay (the struct fields are private)
        let frame = Frame::from_parts(
            loading_frame(i),
            0,
            0,
            Delay::from_numer_denom_ms(100, 1000),
        );
        encoder.encode_frame(frame)?;
    }
    drop(encoder); // flush the GIF trailer before handing the buffer out
    Ok(out)
}

fn build_sparkle_apng() -> Result<Vec<u8>> {
    let frames: Vec<RgbaImage> = (0..6).map(sparkle_frame).collect();
    let mut out = Vec::new();
    encode_apng(&mut out, &frames, 100)?;
    Ok(out)
}

fn build_spinner_webp() -> Result<Vec<u8>> {
    // lossy q80 per the plan; timestamps are the animation track's frame
    // durations (100ms steps) and do NOT end up as bytes in the container
    let mut encoder = webp_animation::Encoder::new((240, 180))?;
    encoder.set_default_encoding_config(webp_animation::EncodingConfig::new_lossy(80.0))?;
    for i in 0..8 {
        encoder.add_frame(spinner_frame(i).as_raw(), (i * 100) as i32)?;
    }
    let data = encoder.finalize(800)?;
    Ok(data.to_vec())
}

/// Prints the regenerated tree with byte sizes (the numbers the commit
/// message/review care about).
fn print_tree(dir: &Path) -> Result<()> {
    for sub in ["photos", "screens", "anim"] {
        let sub_dir = dir.join(sub);
        if !sub_dir.is_dir() {
            continue;
        }
        println!("  {sub}/");
        for name in util::list_files(&sub_dir)? {
            let size = util::file_size(&sub_dir.join(&name));
            println!("    {name:<24} {:>10} bytes", size);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    #[test]
    fn logo_png_carries_exif_without_orientation() {
        let png = build_logo_png().unwrap();
        let exif = exif::Reader::new()
            .read_from_container(&mut Cursor::new(&png))
            .expect("logo.png must carry a readable EXIF block");
        // Value does not implement PartialEq in kamadak-exif 0.6, so the
        // Ascii payloads are compared via pattern matching
        let make = exif.get_field(Tag::Make, In::PRIMARY).expect("Make tag");
        match &make.value {
            Value::Ascii(parts) => assert_eq!(parts, &vec![b"byteshaver".to_vec()]),
            other => panic!("Make must be Ascii, got {other:?}"),
        }
        let model = exif.get_field(Tag::Model, In::PRIMARY).expect("Model tag");
        match &model.value {
            Value::Ascii(parts) => assert_eq!(parts, &vec![b"fixture-gen".to_vec()]),
            other => panic!("Model must be Ascii, got {other:?}"),
        }
        assert!(
            exif.get_field(Tag::DateTimeOriginal, In::PRIMARY).is_some(),
            "DateTimeOriginal must be present"
        );
        let lat = exif
            .get_field(Tag::GPSLatitude, In::PRIMARY)
            .expect("GPSLatitude must be present");
        assert!(matches!(lat.value, Value::Rational(ref v) if v.len() == 3));
        assert!(exif.get_field(Tag::GPSLongitude, In::PRIMARY).is_some());
        // no Orientation tag at all (not even =1): no rotation surprises
        assert!(
            exif.get_field(Tag::Orientation, In::PRIMARY).is_none(),
            "Orientation must NOT be set"
        );
    }

    #[test]
    fn dashboard_png_has_no_exif_and_correct_dimensions() {
        let png = build_dashboard_png().unwrap();
        let (w, h) = image::load_from_memory(&png)
            .expect("dashboard.png must decode")
            .dimensions();
        assert_eq!((w, h), (480, 320));
        assert!(
            exif::Reader::new()
                .read_from_container(&mut Cursor::new(&png))
                .is_err(),
            "dashboard.png must not carry EXIF"
        );
    }

    #[test]
    fn generation_is_byte_deterministic() {
        // every encoder used here (png/gif/webp) must emit identical bytes
        // for identical input; this is the in-process guard behind the
        // "run gen-fixtures twice, diff the tree" verification
        assert_eq!(build_dashboard_png().unwrap(), build_dashboard_png().unwrap());
        assert_eq!(build_logo_png().unwrap(), build_logo_png().unwrap());
        assert_eq!(build_loading_gif().unwrap(), build_loading_gif().unwrap());
        assert_eq!(build_sparkle_apng().unwrap(), build_sparkle_apng().unwrap());
        assert_eq!(build_spinner_webp().unwrap(), build_spinner_webp().unwrap());
    }

    #[test]
    fn photo_sources_exist() {
        let repo = util::repo_root();
        for (src, _) in PHOTOS {
            assert!(
                repo.join(src).is_file(),
                "pinned fixture source {src} is missing"
            );
        }
    }
}
