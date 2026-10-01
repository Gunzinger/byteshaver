//! AVIF EXIF embedding via ravif (`Encoder::with_exif`, avif-serialize).
//!
//! ravif >= 0.13 writes the resolved EXIF payload as a standard HEIF `Exif`
//! item (infe + iloc + iref 'cdsc') into the ISOBMFF container after the AV1
//! encode. These tests round-trip a JPEG/PNG fixture through the pipeline and
//! verify the item with a minimal pure-Rust ISOBMFF scan; when the `dec-heif`
//! feature is available, libheif is used as an independent cross-check.

#![cfg(feature = "exif")]

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

// the kamadak-exif crate (import name `exif`), fully qualified to avoid the
// name clash with byteshaver::metadata::exif
use ::exif as kx;
use byteshaver::config::{AvifOptions, ConversionConfig, EncoderConfig};
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
/// (alpha input produces a second image item plus an `auxl` iref — the mux
/// must keep both items intact).
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

// ---------------------------------------------------------------------------
// minimal ISOBMFF scan (verification without any optional dependency)
// ---------------------------------------------------------------------------

fn u16_be(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_be(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn box_type(b: &[u8], at: usize) -> Option<[u8; 4]> {
    b.get(at + 4..at + 8)
        .and_then(|slice| slice.try_into().ok())
}

/// Returns the payload of the first `Exif` item of an AVIF/HEIF container
/// (raw TIFF stream, the 4-byte `exif_tiff_header_offset` prefix stripped).
fn extract_avif_exif(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut exif_id = None;
    let mut iloc_body: Option<&[u8]> = None;

    for (off, size, typ) in top_level_boxes(bytes)? {
        if &typ == b"meta" {
            // fullbox: skip version/flags, then recurse one level
            let end = off + size;
            let mut at = off + 12;
            while at + 8 <= end {
                let child_size = u32_be(bytes, at)? as usize;
                if child_size < 8 || at + child_size > end {
                    return None;
                }
                let child = &bytes[at + 8..at + child_size];
                match &bytes[at + 4..at + 8] {
                    // box order inside meta is not guaranteed, so collect
                    // both bodies and resolve the id afterwards
                    b"iinf" => exif_id = find_exif_item_id(child),
                    b"iloc" => iloc_body = Some(child),
                    _ => {}
                }
                at += child_size;
            }
        }
    }
    let exif_id = exif_id?;
    let extents = read_iloc_extents(iloc_body?, exif_id)?;
    let mut item = Vec::new();
    for (start, len) in extents {
        item.extend_from_slice(bytes.get(start..start + len)?);
    }
    if item.len() < 4 {
        return None;
    }
    Some(item[4..].to_vec())
}

/// Iterates the top-level boxes of a container as (offset, size, type).
fn top_level_boxes(bytes: &[u8]) -> Option<Vec<(usize, usize, [u8; 4])>> {
    let mut boxes = Vec::new();
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let size = u32_be(bytes, at)? as usize;
        if size < 8 || at + size > bytes.len() {
            return None;
        }
        boxes.push((at, size, box_type(bytes, at)?));
        at += size;
    }
    Some(boxes)
}

/// Scans an `iinf` body (after its fullbox header) for the `Exif` item id.
fn find_exif_item_id(iinf_body: &[u8]) -> Option<u16> {
    // version 0: u16 entry_count then infe boxes; version 1+: u32
    let version = *iinf_body.first()?;
    let entries_at = if version == 0 { 6 } else { 8 };
    let end = iinf_body.len();
    let mut at = entries_at;
    while at + 8 <= end {
        let size = u32_be(iinf_body, at)? as usize;
        if size < 13 || at + size > end {
            return None;
        }
        let body = &iinf_body[at + 12..at + size]; // skip header + fullbox
        // infe v2 (version byte right after the box header): body =
        // u16 item_id, u16 protection, u32 item_type, NUL-terminated name
        if &iinf_body[at + 4..at + 8] == b"infe"
            && iinf_body[at + 8] == 2
            && body.get(4..8) == Some(b"Exif")
        {
            return u16_be(body, 0);
        }
        at += size;
    }
    None
}

/// Scans an `iloc` body (after its fullbox header) for the extents of
/// `item_id` (version 0, absolute file offsets).
fn read_iloc_extents(iloc_body: &[u8], item_id: u16) -> Option<Vec<(usize, usize)>> {
    if iloc_body.first().copied()? != 0 {
        return None; // only the layout avif-serialize writes
    }
    let sizes = *iloc_body.get(4)?;
    let (offset_size, length_size) = ((sizes >> 4) as usize, (sizes & 0xf) as usize);
    let base_offset_size = (iloc_body.get(5).copied().unwrap_or(0) >> 4) as usize;
    let count = u16_be(iloc_body, 6)? as usize;
    let mut extents = Vec::new();
    let mut at = 8;
    for _ in 0..count {
        let id = u16_be(iloc_body, at)?;
        at += 2; // item_ID
        at += 2; // data_reference_index
        at += base_offset_size;
        let extent_count = u16_be(iloc_body, at)? as usize;
        at += 2;
        for _ in 0..extent_count {
            let offset = read_uint(iloc_body, at, offset_size);
            at += offset_size;
            let length = read_uint(iloc_body, at, length_size);
            at += length_size;
            if id == item_id && length > 0 {
                extents.push((offset, length));
            }
        }
    }
    Some(extents)
}

fn read_uint(b: &[u8], at: usize, width: usize) -> usize {
    let mut value = 0usize;
    for byte in b.get(at..at + width).unwrap_or(&[]) {
        value = (value << 8) | usize::from(*byte);
    }
    value
}

// ---------------------------------------------------------------------------
// optional cross-check via libheif (dec-heif feature)
// ---------------------------------------------------------------------------

/// Reads the EXIF payload of an AVIF container back via libheif.
#[cfg(feature = "dec-heif")]
fn read_avif_exif_via_libheif(path: &Path) -> Option<Vec<u8>> {
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

/// Decodes the AVIF through libheif and asserts the pixel dimensions
/// (proves the container is intact, needs an AV1 decoder plugin).
#[cfg(feature = "dec-heif")]
fn assert_avif_decodes(path: &Path, width: u32, height: u32) {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

    let path_str = path.to_str().expect("unicode path");
    let ctx = HeifContext::read_from_file(path_str).expect("read avif container");
    let handle = ctx.primary_image_handle().expect("primary handle");
    let lib_heif = LibHeif::new();
    let decoded = lib_heif
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)
        .expect("decode avif");
    let plane = decoded.planes().interleaved.expect("interleaved plane");
    assert_eq!(plane.width, width);
    assert_eq!(plane.height, height);
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

/// keep → the AVIF carries the verbatim EXIF payload as an Exif item.
#[test]
fn keep_policy_avif_round_trip() {
    let dir = temp_dir("keep");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    let source_payload = build_exif_payload(1);
    write_jpeg_with_exif(&input, &source_payload, 24, 16);

    let stats = run(
        single_file_config(&input, &output, ExifPolicy::Keep),
        EncoderConfig::Avif(AvifOptions::default()),
    )
    .expect("run");
    let RunStats {
        successful,
        metadata_dropped,
        ..
    } = stats;
    assert_eq!(successful, 1);
    assert_eq!(metadata_dropped, 0, "avif carries EXIF via ravif");

    let out = output_file(&output, "photo", "avif");
    let out_payload =
        extract_avif_exif(&fs::read(&out).expect("read avif")).expect("avif carries an Exif item");
    assert_eq!(out_payload, source_payload, "verbatim payload expected");

    #[cfg(feature = "dec-heif")]
    {
        assert_eq!(
            read_avif_exif_via_libheif(&out).as_deref(),
            Some(source_payload.as_slice()),
            "libheif must agree with the raw scan"
        );
        assert_avif_decodes(&out, 24, 16);
    }
    let _ = fs::remove_dir_all(&dir);
}

/// keep on an alpha source → Exif item and the alpha item's iref coexist.
#[test]
fn keep_policy_avif_alpha_round_trip() {
    let dir = temp_dir("keep-alpha");
    let input = dir.join("transparent.png");
    let output = dir.join("out");
    let source_payload = build_exif_payload(1);
    write_rgba_png_with_exif(&input, &source_payload, 16, 16);

    let stats = run(
        single_file_config(&input, &output, ExifPolicy::Keep),
        EncoderConfig::Avif(AvifOptions::default()),
    )
    .expect("run");
    assert_eq!(stats.successful, 1);

    let bytes = fs::read(output_file(&output, "transparent", "avif")).expect("read avif");
    assert!(
        bytes.windows(4).any(|w| w == b"auxl"),
        "alpha auxiliary reference must survive"
    );
    let out_payload = extract_avif_exif(&bytes).expect("avif carries an Exif item");
    assert_eq!(out_payload, source_payload, "verbatim payload expected");

    #[cfg(feature = "dec-heif")]
    assert_eq!(
        read_avif_exif_via_libheif(&output_file(&output, "transparent", "avif")).as_deref(),
        Some(source_payload.as_slice())
    );
    let _ = fs::remove_dir_all(&dir);
}

/// strip → plain ravif output without an Exif item.
#[test]
fn strip_policy_avif_has_no_exif_item() {
    let dir = temp_dir("strip");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    write_jpeg_with_exif(&input, &build_exif_payload(1), 16, 16);

    let stats = run(
        single_file_config(&input, &output, ExifPolicy::Strip),
        EncoderConfig::Avif(AvifOptions::default()),
    )
    .expect("run");
    assert_eq!(stats.successful, 1);
    assert_eq!(
        stats.metadata_dropped, 0,
        "policy stripped, not the encoder"
    );

    let bytes = fs::read(output_file(&output, "photo", "avif")).expect("read avif");
    assert!(
        extract_avif_exif(&bytes).is_none(),
        "strip policy must not leave an Exif item"
    );
    let _ = fs::remove_dir_all(&dir);
}
