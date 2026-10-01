//! AVIF EXIF embedding via libheif (`enc-avif` feature).
//!
//! When EXIF survives the policy, AVIF output is routed through the native
//! libheif encoder (`heif_context_add_exif_metadata`) because ravif has no
//! metadata API. These tests need the native libheif with an AV1 encoder
//! plugin (e.g. `libheif-plugin-aomenc`/libaom) and therefore run in the
//! docker/gnu CI jobs only — like the `dec-heif` input tests.

#![cfg(all(feature = "enc-avif", feature = "exif"))]

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

// the kamadak-exif crate (import name `exif`), fully qualified to avoid the
// name clash with byteshaver::metadata::exif
use ::exif as kx;
use byteshaver::config::{ConversionConfig, EncoderConfig};
use byteshaver::metadata::policy::ExifPolicy;
use byteshaver::{RunStats, run};

// ---------------------------------------------------------------------------
// fixture generation (mirrors tests/exif_integration.rs)
// ---------------------------------------------------------------------------

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "byteshaver-avif-meta-{}-{}",
        name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Builds a raw TIFF EXIF payload with standard and GPS tags via the kamadak
/// writer (~250 bytes, like a real camera payload minus the maker notes).
fn build_exif_payload(orientation: u16) -> Vec<u8> {
    use kx::experimental::Writer;

    let fields = [
        kx::Field {
            tag: kx::Tag::Orientation,
            ifd_num: kx::In::PRIMARY,
            value: kx::Value::Short(vec![orientation]),
        },
        kx::Field {
            tag: kx::Tag::Make,
            ifd_num: kx::In::PRIMARY,
            value: kx::Value::Ascii(vec![b"TestCam\0".to_vec()]),
        },
        kx::Field {
            tag: kx::Tag::Model,
            ifd_num: kx::In::PRIMARY,
            value: kx::Value::Ascii(vec![b"Byteshaver 9000\0".to_vec()]),
        },
        kx::Field {
            tag: kx::Tag::DateTimeOriginal,
            ifd_num: kx::In::PRIMARY,
            value: kx::Value::Ascii(vec![b"2024:05:01 12:34:56\0".to_vec()]),
        },
        kx::Field {
            tag: kx::Tag::GPSLatitudeRef,
            ifd_num: kx::In::PRIMARY,
            value: kx::Value::Ascii(vec![b"N\0".to_vec()]),
        },
        kx::Field {
            tag: kx::Tag::GPSLatitude,
            ifd_num: kx::In::PRIMARY,
            value: kx::Value::Rational(vec![
                kx::Rational { num: 48, denom: 1 },
                kx::Rational { num: 8, denom: 1 },
                kx::Rational {
                    num: 153_653,
                    denom: 10_000,
                },
            ]),
        },
    ];

    let mut writer = Writer::new();
    for field in &fields {
        writer.push_field(field);
    }
    let mut serialized = Cursor::new(Vec::new());
    writer
        .write(&mut serialized, true)
        .expect("fixture exif writer");
    serialized.into_inner()
}

/// Writes a baseline JPEG with the image crate and splices an APP1 segment
/// (`Exif\0\0` + TIFF payload) right after the SOI marker.
fn write_jpeg_with_exif(path: &Path, payload: &[u8], width: u32, height: u32) {
    let mut buffer = image::RgbImage::new(width, height);
    for (x, y, pixel) in buffer.enumerate_pixels_mut() {
        *pixel = image::Rgb([
            (x * 7 % 256) as u8,
            (y * 11 % 256) as u8,
            ((x + y) % 256) as u8,
        ]);
    }
    let mut baseline = Vec::new();
    let image = image::DynamicImage::ImageRgb8(buffer);
    image
        .write_to(&mut Cursor::new(&mut baseline), image::ImageFormat::Jpeg)
        .expect("write baseline jpeg");

    let mut out = Vec::with_capacity(baseline.len() + payload.len() + 4);
    out.extend_from_slice(&baseline[..2]); // SOI
    out.extend_from_slice(&[0xff, 0xe1]); // APP1
    out.extend_from_slice(&((payload.len() as u16 + 2 + 6).to_be_bytes()));
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(payload);
    out.extend_from_slice(&baseline[2..]);
    fs::write(path, out).expect("write jpeg fixture");
}

/// Writes an RGBA PNG with an `eXIf` chunk carrying the raw TIFF payload
/// (alpha input exercises the RGBA branch of the libheif encode path).
fn write_rgba_png_with_exif(path: &Path, payload: &[u8], width: u32, height: u32) {
    let mut buffer = image::RgbaImage::new(width, height);
    for (x, y, pixel) in buffer.enumerate_pixels_mut() {
        *pixel = image::Rgba([(x * 3 % 256) as u8, (y * 17 % 256) as u8, 42, 200]);
    }
    let mut output = Vec::new();
    let mut info = png::Info::with_size(width, height);
    info.exif_metadata = Some(std::borrow::Cow::Borrowed(payload));
    let mut encoder = png::Encoder::with_info(&mut output, info).expect("png encoder");
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(buffer.as_raw()).expect("png data");
    writer.finish().expect("png finish");
    fs::write(path, output).expect("write png fixture");
}

fn single_file_config(pattern: &Path, output: &Path, policy: ExifPolicy) -> ConversionConfig {
    ConversionConfig {
        pattern: pattern.display().to_string(),
        output: output.display().to_string(),
        exif: policy,
        ..ConversionConfig::default()
    }
}

fn output_file(output: &Path, stem: &str, extension: &str) -> PathBuf {
    let path = output.join(format!("{stem}.{extension}"));
    assert!(path.exists(), "expected output {}", path.display());
    path
}

/// Reads the EXIF payload of an AVIF container back via libheif (raw TIFF
/// payload without the 4-byte TIFF-header offset prefix).
fn read_avif_exif(path: &Path) -> Option<Vec<u8>> {
    use libheif_rs::HeifContext;

    let path_str = path.to_str().expect("unicode path");
    let ctx = HeifContext::read_from_file(path_str).expect("read avif container");
    let handle = ctx.primary_image_handle().expect("primary handle");
    let capacity = handle.number_of_metadata_blocks(b"Exif").max(0) as usize;
    let mut ids: Vec<libheif_rs::ItemId> = vec![0; capacity];
    let count = handle.metadata_block_ids(&mut ids, b"Exif");
    for id in ids.into_iter().take(count) {
        if let Ok(raw) = handle.metadata(id)
            && raw.len() >= 4
        {
            return Some(raw[4..].to_vec());
        }
    }
    None
}

/// Decodes the AVIF through libheif and returns the pixel dimensions
/// (proves the file itself is intact, needs an AV1 decoder plugin).
fn avif_dimensions(path: &Path) -> (u32, u32) {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

    let path_str = path.to_str().expect("unicode path");
    let ctx = HeifContext::read_from_file(path_str).expect("read avif container");
    let handle = ctx.primary_image_handle().expect("primary handle");
    let (w, h) = (handle.width(), handle.height());
    let lib_heif = LibHeif::new();
    let decoded = lib_heif
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)
        .expect("decode avif");
    let plane = decoded.planes().interleaved.expect("interleaved plane");
    assert_eq!(plane.width, w);
    assert_eq!(plane.height, h);
    (w, h)
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

/// keep → the AVIF carries the verbatim EXIF payload, image decodes fine.
#[test]
fn keep_policy_avif_round_trip_via_libheif() {
    let dir = temp_dir("keep");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    let source_payload = build_exif_payload(1);
    write_jpeg_with_exif(&input, &source_payload, 24, 16);

    let stats = run(
        single_file_config(&input, &output, ExifPolicy::Keep),
        EncoderConfig::Avif(byteshaver::config::AvifOptions::default()),
    )
    .expect("run");
    let RunStats {
        successful,
        metadata_dropped,
        ..
    } = stats;
    assert_eq!(successful, 1);
    assert_eq!(metadata_dropped, 0, "avif must carry EXIF (enc-avif)");

    let out = output_file(&output, "photo", "avif");
    let out_payload = read_avif_exif(&out).expect("avif carries EXIF");
    assert_eq!(out_payload, source_payload, "verbatim payload expected");
    assert_eq!(avif_dimensions(&out), (24, 16));
    let _ = fs::remove_dir_all(&dir);
}

/// keep on an alpha source → the RGBA branch embeds EXIF as well.
#[test]
fn keep_policy_avif_rgba_round_trip_via_libheif() {
    let dir = temp_dir("keep-rgba");
    let input = dir.join("transparent.png");
    let output = dir.join("out");
    let source_payload = build_exif_payload(1);
    write_rgba_png_with_exif(&input, &source_payload, 16, 16);

    let stats = run(
        single_file_config(&input, &output, ExifPolicy::Keep),
        EncoderConfig::Avif(byteshaver::config::AvifOptions::default()),
    )
    .expect("run");
    assert_eq!(stats.successful, 1);

    let out_payload =
        read_avif_exif(&output_file(&output, "transparent", "avif")).expect("avif carries EXIF");
    assert_eq!(out_payload, source_payload, "verbatim payload expected");
    let _ = fs::remove_dir_all(&dir);
}

/// strip → the fast ravif path runs and the output has no Exif item.
#[test]
fn strip_policy_avif_has_no_exif_item() {
    let dir = temp_dir("strip");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    write_jpeg_with_exif(&input, &build_exif_payload(1), 16, 16);

    let stats = run(
        single_file_config(&input, &output, ExifPolicy::Strip),
        EncoderConfig::Avif(byteshaver::config::AvifOptions::default()),
    )
    .expect("run");
    assert_eq!(stats.successful, 1);
    assert_eq!(
        stats.metadata_dropped, 0,
        "policy stripped, not the encoder"
    );

    assert!(
        read_avif_exif(&output_file(&output, "photo", "avif")).is_none(),
        "strip policy must not leave an Exif item"
    );
    let _ = fs::remove_dir_all(&dir);
}
