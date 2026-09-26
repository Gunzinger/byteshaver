//! WS4 integration tests: EXIF policies (strip/keep/filter), extraction and
//! embedding round-trips, orientation bake-in, WebP RIFF muxing and the
//! recognized-tag listing.
//!
//! All fixtures are generated at runtime: EXIF payloads are built with the
//! kamadak writer, spliced into a baseline JPEG (APP1 after SOI), written
//! into PNGs via the `eXIf` chunk and muxed into WebP containers via the
//! crate's own RIFF muxer.

#![cfg(feature = "exif")]

use std::borrow::Cow;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

// the kamadak-exif crate (import name `exif`), fully qualified to avoid the
// name clash with byteshaver::metadata::exif
use ::exif as kx;
use byteshaver::config::{ConversionConfig, EncoderConfig, PngOptions};
use byteshaver::format::ImageFormat;
use byteshaver::metadata::policy::{ExifPolicy, Ifd, TagSelector};
use byteshaver::metadata::{self, exif};
use byteshaver::{input, run};

// ---------------------------------------------------------------------------
// fixture generation
// ---------------------------------------------------------------------------

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("byteshaver-exif-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Builds a raw TIFF EXIF payload with Orientation, standard tags and GPS
/// tags via the kamadak writer.
///
/// Sub-IFD fields (Exif/GPS) carry `In::PRIMARY` as their IFD number —
/// kamadak models sub-IFDs through the tag context, not the IFD number.
fn build_exif_payload(orientation: u16, with_gps: bool) -> Vec<u8> {
    use kx::experimental::Writer;

    let mut fields = vec![kx::Field {
        tag: kx::Tag::Orientation,
        ifd_num: kx::In::PRIMARY,
        value: kx::Value::Short(vec![orientation]),
    }];
    fields.push(kx::Field {
        tag: kx::Tag::Make,
        ifd_num: kx::In::PRIMARY,
        value: kx::Value::Ascii(vec![b"TestCam\0".to_vec()]),
    });
    fields.push(kx::Field {
        tag: kx::Tag::Model,
        ifd_num: kx::In::PRIMARY,
        value: kx::Value::Ascii(vec![b"Byteshaver 9000\0".to_vec()]),
    });
    fields.push(kx::Field {
        tag: kx::Tag::DateTimeOriginal,
        ifd_num: kx::In::PRIMARY,
        value: kx::Value::Ascii(vec![b"2024:05:01 12:34:56\0".to_vec()]),
    });
    if with_gps {
        fields.push(kx::Field {
            tag: kx::Tag::GPSVersionID,
            ifd_num: kx::In::PRIMARY,
            value: kx::Value::Byte(vec![2, 3, 0, 0]),
        });
        fields.push(kx::Field {
            tag: kx::Tag::GPSLatitudeRef,
            ifd_num: kx::In::PRIMARY,
            value: kx::Value::Ascii(vec![b"N\0".to_vec()]),
        });
        fields.push(kx::Field {
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
        });
        fields.push(kx::Field {
            tag: kx::Tag::GPSLongitudeRef,
            ifd_num: kx::In::PRIMARY,
            value: kx::Value::Ascii(vec![b"E\0".to_vec()]),
        });
        fields.push(kx::Field {
            tag: kx::Tag::GPSLongitude,
            ifd_num: kx::In::PRIMARY,
            value: kx::Value::Rational(vec![
                kx::Rational { num: 11, denom: 1 },
                kx::Rational { num: 34, denom: 1 },
                kx::Rational {
                    num: 451_010,
                    denom: 10_000,
                },
            ]),
        });
    }

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
    let baseline = encode_shim_jpeg(width, height);
    let mut out = Vec::with_capacity(baseline.len() + payload.len() + 4);
    out.extend_from_slice(&baseline[..2]); // SOI
    out.extend_from_slice(&[0xff, 0xe1]); // APP1
    out.extend_from_slice(&((payload.len() as u16 + 2 + 6).to_be_bytes()));
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(payload);
    out.extend_from_slice(&baseline[2..]);
    fs::write(path, out).expect("write jpeg fixture");
}

/// Writes a PNG with an `eXIf` chunk carrying the raw TIFF payload.
fn write_png_with_exif(path: &Path, payload: &[u8], width: u32, height: u32) {
    let mut buffer = image::RgbImage::new(width, height);
    for (x, y, pixel) in buffer.enumerate_pixels_mut() {
        *pixel = image::Rgb([(x * 5 % 256) as u8, (y * 13 % 256) as u8, 128]);
    }
    let mut output = Vec::new();
    let mut info = png::Info::with_size(width, height);
    info.exif_metadata = Some(Cow::Borrowed(payload));
    let mut encoder = png::Encoder::with_info(&mut output, info).expect("png encoder");
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(buffer.as_raw()).expect("png data");
    writer.finish().expect("png finish");
    fs::write(path, output).expect("write png fixture");
}

/// Writes a WebP with an `EXIF` chunk via the crate's own RIFF muxer.
fn write_webp_with_exif(path: &Path, payload: &[u8], width: u32, height: u32) {
    let mut buffer = image::RgbaImage::new(width, height);
    for (x, y, pixel) in buffer.enumerate_pixels_mut() {
        *pixel = image::Rgba([(x * 3 % 256) as u8, (y * 17 % 256) as u8, 42, 255]);
    }
    let image = image::DynamicImage::ImageRgba8(buffer);
    let encoded = encode_shim_webp_image(&image);
    let muxed = metadata::riff::mux_exif(&encoded, payload).expect("webp mux");
    fs::write(path, muxed).expect("write webp fixture");
}

/// Encodes a small deterministic gradient JPEG through the mozjpeg encoder.
fn encode_shim_jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut buffer = image::RgbImage::new(width, height);
    for (x, y, pixel) in buffer.enumerate_pixels_mut() {
        *pixel = image::Rgb([
            (x * 7 % 256) as u8,
            (y * 11 % 256) as u8,
            ((x + y) % 256) as u8,
        ]);
    }
    let image = image::DynamicImage::ImageRgb8(buffer);
    encode_via_registry(
        EncoderConfig::Jpeg,
        image,
        byteshaver::metadata::ImageMetadata::default(),
    )
}

/// Encodes an image through the lossless webp (image crate) encoder.
fn encode_shim_webp_image(image: &image::DynamicImage) -> Vec<u8> {
    encode_via_registry(
        EncoderConfig::WebpImage,
        image.clone(),
        byteshaver::metadata::ImageMetadata::default(),
    )
}

/// Runs an image through the EncoderRegistry with empty metadata.
fn encode_via_registry(
    config: EncoderConfig,
    image: image::DynamicImage,
    metadata: byteshaver::metadata::ImageMetadata,
) -> Vec<u8> {
    let encoder = byteshaver::converter::EncoderRegistry::build(
        &config,
        byteshaver::converter::ThreadBudget::global(),
    );
    let source = input::SourceImage {
        content: input::ImageContent::Still(image),
        metadata,
        source_format: ImageFormat::Png,
        source_path: PathBuf::from("fixture.png"),
    };
    encoder.encode(&source).expect("shim encode")
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

/// Extracts the EXIF payload of a converted output file (raw TIFF payload).
fn extract_payload(path: &Path, format: ImageFormat) -> Option<Vec<u8>> {
    let bytes = fs::read(path).expect("read output");
    match format {
        // JPEG APP1 payload after the "Exif\0\0" header
        ImageFormat::Jpeg => find_jpeg_app1_exif(&bytes).map(|payload| payload.to_vec()),
        // PNG eXIf chunk body (raw TIFF)
        ImageFormat::Png => find_png_exif_chunk(&bytes).filter(|body| !body.is_empty()),
        // WebP EXIF chunk via the RIFF scan
        ImageFormat::Webp => metadata::riff::extract_exif_payload(&bytes),
        _ => None,
    }
}

/// Scans JPEG segments for the EXIF APP1 and returns the TIFF payload.
fn find_jpeg_app1_exif(jpeg: &[u8]) -> Option<&[u8]> {
    let mut offset = 2; // skip SOI
    while offset + 4 <= jpeg.len() {
        if jpeg[offset] != 0xff {
            return None;
        }
        let marker = jpeg[offset + 1];
        if marker == 0xd8 || (0xd0..=0xd7).contains(&marker) || marker == 0x01 {
            offset += 2;
            continue;
        }
        if marker == 0xda {
            return None; // start of scan; EXIF must come before
        }
        let length = usize::from(jpeg[offset + 2]) << 8 | usize::from(jpeg[offset + 3]);
        let segment = jpeg.get(offset + 4..offset + 2 + length)?;
        if marker == 0xe1 && segment.len() > 6 && &segment[..6] == b"Exif\0\0" {
            return Some(&segment[6..]);
        }
        offset += 2 + length;
    }
    None
}

/// Scans PNG chunks for `eXIf` and returns its body.
fn find_png_exif_chunk(png: &[u8]) -> Option<Vec<u8>> {
    let mut offset = 8; // signature
    while offset + 8 <= png.len() {
        let length = u32::from_be_bytes(png[offset..offset + 4].try_into().ok()?) as usize;
        let kind = &png[offset + 4..offset + 8];
        let body = png
            .get(offset + 8..offset + 8 + length)
            .unwrap_or(&[])
            .to_vec();
        if kind == b"eXIf" {
            return Some(body);
        }
        offset += 12 + length; // length + type + data + crc
    }
    None
}

/// Compares two EXIF payloads field by field (tag, IFD, value).
fn assert_fields_equal(left: &[u8], right: &[u8], context: &str) {
    let left = exif::parse(left).expect("left payload parses");
    let right = exif::parse(right).expect("right payload parses");
    let left_fields: Vec<(kx::Tag, kx::In, String)> = left
        .fields()
        .map(|field| (field.tag, field.ifd_num, field.display_value().to_string()))
        .collect();
    let right_fields: Vec<(kx::Tag, kx::In, String)> = right
        .fields()
        .map(|field| (field.tag, field.ifd_num, field.display_value().to_string()))
        .collect();
    assert_eq!(left_fields, right_fields, "{context}: field mismatch");
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

/// keep → JPEG output carries the verbatim payload (byte equality).
#[test]
fn keep_policy_jpeg_round_trip_verbatim() {
    let dir = temp_dir("keep-jpeg");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    let source_payload = build_exif_payload(1, true);
    write_jpeg_with_exif(&input, &source_payload, 8, 8);

    let stats = run(
        single_file_config(&input, &output, ExifPolicy::Keep),
        EncoderConfig::Jpeg,
    )
    .expect("run");
    assert_eq!(stats.successful, 1);
    assert_eq!(stats.metadata_dropped, 0);

    let out_payload = extract_payload(&output_file(&output, "photo", "jpeg"), ImageFormat::Jpeg)
        .expect("jpeg output carries EXIF");
    // Keep is byte-for-byte verbatim on JPEG (APP1 holds the raw payload)
    assert_eq!(out_payload, source_payload, "verbatim payload expected");
    let _ = fs::remove_dir_all(&dir);
}

/// keep → PNG output re-parses to the same fields (eXIf chunk body).
#[test]
fn keep_policy_png_round_trip() {
    let dir = temp_dir("keep-png");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    let source_payload = build_exif_payload(1, true);
    write_jpeg_with_exif(&input, &source_payload, 8, 8);

    let stats = run(
        single_file_config(&input, &output, ExifPolicy::Keep),
        EncoderConfig::Png(PngOptions::default()),
    )
    .expect("run");
    assert_eq!(stats.successful, 1);

    let out_payload = extract_payload(&output_file(&output, "photo", "png"), ImageFormat::Png)
        .expect("png output carries eXIf");
    assert_fields_equal(&source_payload, &out_payload, "png keep round trip");
    let _ = fs::remove_dir_all(&dir);
}

/// filter --exif-except gps* drops GPS fields and keeps DateTimeOriginal.
#[test]
fn filter_except_gps_drops_gps_keeps_rest() {
    let dir = temp_dir("filter-except");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    let source_payload = build_exif_payload(1, true);
    write_jpeg_with_exif(&input, &source_payload, 8, 8);

    let policy = ExifPolicy::FilterExcept(vec![
        TagSelector::Ifd(Ifd::Gps),
        TagSelector::Name("GPSInfo".to_string()),
    ]);
    let stats = run(
        single_file_config(&input, &output, policy),
        EncoderConfig::Jpeg,
    )
    .expect("run");
    assert_eq!(stats.successful, 1);

    let out_payload = extract_payload(&output_file(&output, "photo", "jpeg"), ImageFormat::Jpeg)
        .expect("filtered payload present");
    let parsed = exif::parse(&out_payload).expect("filtered payload parses");
    let gps_fields = parsed
        .fields()
        .filter(|field| {
            field.tag.context() == kx::Context::Gps || field.tag == kx::Tag::GPSInfoIFDPointer
        })
        .count();
    assert_eq!(gps_fields, 0, "GPS fields must be gone");
    assert!(
        parsed
            .get_field(kx::Tag::DateTimeOriginal, kx::In::PRIMARY)
            .is_some(),
        "DateTimeOriginal must survive"
    );
    assert!(parsed.get_field(kx::Tag::Make, kx::In::PRIMARY).is_some());
    let _ = fs::remove_dir_all(&dir);
}

/// filter --exif-only DateTimeOriginal keeps exactly that field.
#[test]
fn filter_only_keeps_selected_tags() {
    let dir = temp_dir("filter-only");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    let source_payload = build_exif_payload(1, true);
    write_jpeg_with_exif(&input, &source_payload, 8, 8);

    let policy = ExifPolicy::KeepOnly(vec![TagSelector::Name("DateTimeOriginal".to_string())]);
    run(
        single_file_config(&input, &output, policy),
        EncoderConfig::Jpeg,
    )
    .expect("run");

    let out_payload = extract_payload(&output_file(&output, "photo", "jpeg"), ImageFormat::Jpeg)
        .expect("filtered payload present");
    let parsed = exif::parse(&out_payload).expect("parses");
    // the writer synthesizes structural pointer fields; only those plus the
    // selected field may remain
    let tags: Vec<kx::Tag> = parsed.fields().map(|field| field.tag).collect();
    assert!(
        tags.iter()
            .all(|tag| *tag == kx::Tag::DateTimeOriginal || *tag == kx::Tag::ExifIFDPointer),
        "only DateTimeOriginal (plus structural pointers) survives: {tags:?}"
    );
    assert!(tags.contains(&kx::Tag::DateTimeOriginal));
    let _ = fs::remove_dir_all(&dir);
}

/// strip on an orientation-6 JPEG: pixels rotated upright, no EXIF left.
#[test]
fn strip_bakes_orientation_six_upright() {
    let dir = temp_dir("strip-orient");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    let source_payload = build_exif_payload(6, false);
    write_jpeg_with_exif(&input, &source_payload, 16, 8);

    let stats = run(
        single_file_config(&input, &output, ExifPolicy::Strip),
        EncoderConfig::Png(PngOptions::default()),
    )
    .expect("run");
    assert_eq!(stats.metadata_dropped, 0);

    let out_path = output_file(&output, "photo", "png");
    let out_image = image::open(&out_path).expect("decode output");
    assert_eq!(
        (out_image.width(), out_image.height()),
        (8, 16),
        "orientation 6 swaps width and height"
    );

    // expected pixels: the unrotated source pixels rotated 90° CW
    let source = image::open(&input).expect("decode fixture");
    let expected = image::imageops::rotate90(&source);
    assert_eq!(
        out_image.to_rgba8(),
        image::DynamicImage::from(expected).to_rgba8(),
        "pixels must be rotated upright"
    );
    assert!(
        extract_payload(&out_path, ImageFormat::Png).is_none(),
        "strip must not leave an eXIf chunk"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// keep on an orientation-6 JPEG: pixels untouched, tag preserved.
#[test]
fn keep_preserves_orientation_tag_and_raw_pixels() {
    let dir = temp_dir("keep-orient");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    let source_payload = build_exif_payload(6, false);
    write_jpeg_with_exif(&input, &source_payload, 16, 8);

    run(
        single_file_config(&input, &output, ExifPolicy::Keep),
        EncoderConfig::Png(PngOptions::default()),
    )
    .expect("run");

    let out_path = output_file(&output, "photo", "png");
    let out_image = image::open(&out_path).expect("decode output");
    assert_eq!(
        (out_image.width(), out_image.height()),
        (16, 8),
        "keep must not transform pixels"
    );
    let source = image::open(&input).expect("decode fixture");
    assert_eq!(out_image.to_rgba8(), source.to_rgba8());

    let out_payload =
        extract_payload(&out_path, ImageFormat::Png).expect("orientation tag kept via eXIf");
    let parsed = exif::parse(&out_payload).expect("parses");
    let orientation = parsed
        .get_field(kx::Tag::Orientation, kx::In::PRIMARY)
        .and_then(|field| field.value.get_uint(0));
    assert_eq!(orientation, Some(6));
    let _ = fs::remove_dir_all(&dir);
}

/// WebP embedding: image crate still decodes the muxed container and the
/// RIFF scan finds the EXIF payload.
#[test]
fn webp_embedding_round_trip() {
    let dir = temp_dir("webp-embed");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    let source_payload = build_exif_payload(1, true);
    write_jpeg_with_exif(&input, &source_payload, 12, 9);

    run(
        single_file_config(&input, &output, ExifPolicy::Keep),
        EncoderConfig::Webp(byteshaver::config::WebpOptions {
            lossless: false,
            quality: 90.0,
        }),
    )
    .expect("run");

    let out_path = output_file(&output, "photo", "webp");
    let decoded = image::open(&out_path).expect("image crate decodes muxed webp");
    assert_eq!((decoded.width(), decoded.height()), (12, 9));

    let out_payload = extract_payload(&out_path, ImageFormat::Webp).expect("riff scan finds EXIF");
    assert_fields_equal(&source_payload, &out_payload, "webp keep round trip");
    let _ = fs::remove_dir_all(&dir);
}

/// WebP (lossless image-crate encoder) embedding round trip.
#[test]
fn webp_image_embedding_round_trip() {
    let dir = temp_dir("webp-image-embed");
    let input = dir.join("photo.png");
    let output = dir.join("out");
    let source_payload = build_exif_payload(1, false);
    write_png_with_exif(&input, &source_payload, 7, 5);

    run(
        single_file_config(&input, &output, ExifPolicy::Keep),
        EncoderConfig::WebpImage,
    )
    .expect("run");

    let out_path = output_file(&output, "photo", "webp");
    let decoded = image::open(&out_path).expect("image crate decodes muxed webp");
    assert_eq!((decoded.width(), decoded.height()), (7, 5));
    let out_payload = extract_payload(&out_path, ImageFormat::Webp).expect("riff scan finds EXIF");
    assert_fields_equal(&source_payload, &out_payload, "webp-image round trip");
    let _ = fs::remove_dir_all(&dir);
}

/// PNG eXIf round trip: exif-png → jpeg keep is byte-identical.
#[test]
fn png_exif_source_round_trip() {
    let dir = temp_dir("png-exif-source");
    let input = dir.join("photo.png");
    let output = dir.join("out");
    let source_payload = build_exif_payload(3, true);
    write_png_with_exif(&input, &source_payload, 9, 6);

    // sanity: the source decoder surfaces the payload normalized (raw TIFF)
    let source = input::load_source(&input).expect("load png");
    assert_eq!(
        source.metadata.exif.as_deref(),
        Some(source_payload.as_slice())
    );

    run(
        single_file_config(&input, &output, ExifPolicy::Keep),
        EncoderConfig::Jpeg,
    )
    .expect("run");
    let out_payload = extract_payload(&output_file(&output, "photo", "jpeg"), ImageFormat::Jpeg)
        .expect("jpeg output carries EXIF");
    assert_eq!(out_payload, source_payload, "byte-identical on jpeg keep");
    let _ = fs::remove_dir_all(&dir);
}

/// AVIF cannot embed: one warning per file and the counter increments.
#[test]
fn avif_reports_dropped_metadata() {
    let dir = temp_dir("avif-drop");
    let input = dir.join("photo.jpg");
    let output = dir.join("out");
    let source_payload = build_exif_payload(1, false);
    write_jpeg_with_exif(&input, &source_payload, 8, 8);

    let stats = run(
        single_file_config(&input, &output, ExifPolicy::Keep),
        EncoderConfig::Avif(byteshaver::config::AvifOptions::default()),
    )
    .expect("run");
    assert_eq!(stats.successful, 1);
    assert_eq!(
        stats.metadata_dropped, 1,
        "avif output must count as metadata-dropped"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// strip on files without EXIF must not create anything nor rotate.
#[test]
fn strip_without_exif_is_noop() {
    let dir = temp_dir("strip-noop");
    let input = dir.join("plain.png");
    let output = dir.join("out");
    write_png_with_exif(&input, b"", 5, 5); // empty payload => no usable eXIf

    let source = input::load_source(&input).expect("load");
    assert!(source.metadata.exif.is_none());

    let stats = run(
        single_file_config(&input, &output, ExifPolicy::Strip),
        EncoderConfig::Jpeg,
    )
    .expect("run");
    assert_eq!(stats.successful, 1);
    assert_eq!(stats.metadata_dropped, 0);
    let _ = fs::remove_dir_all(&dir);
}

/// The recognized tag listing is stable and covers the documented names.
#[test]
fn exif_list_tags_output_stable() {
    let listing = byteshaver::metadata::policy::format_recognized_tags();
    assert_eq!(
        listing,
        byteshaver::metadata::policy::format_recognized_tags()
    );
    for name in [
        "Orientation",
        "DateTimeOriginal",
        "GPSInfo",
        "GPSLatitude",
        "LensModel",
    ] {
        assert!(listing.contains(name), "listing must contain {name}");
    }
}

/// The JXL box payload convention (offset prefix + TIFF stream).
#[test]
fn exif_for_jxl_uses_offset_prefix() {
    let payload = metadata::exif_for_jxl(b"II*\0");
    assert_eq!(payload, vec![8, 0, 0, 0, 0, 0, 0, 0, b'I', b'I', b'*', 0]);
}

/// Extraction normalizes payloads for every spliced container type.
#[test]
fn extraction_normalizes_all_container_types() {
    let dir = temp_dir("extract-normalize");
    let payload = build_exif_payload(6, true);

    let jpeg = dir.join("a.jpg");
    write_jpeg_with_exif(&jpeg, &payload, 8, 8);
    let source = input::load_source(&jpeg).expect("load jpeg");
    assert_eq!(source.metadata.exif.as_deref(), Some(payload.as_slice()));
    assert!(!source.metadata.exif_applied_orientation);

    let png = dir.join("b.png");
    write_png_with_exif(&png, &payload, 8, 8);
    let source = input::load_source(&png).expect("load png");
    assert_eq!(source.metadata.exif.as_deref(), Some(payload.as_slice()));

    let webp = dir.join("c.webp");
    write_webp_with_exif(&webp, &payload, 8, 8);
    let source = input::load_source(&webp).expect("load webp");
    assert_eq!(
        source.metadata.exif.as_deref(),
        Some(payload.as_slice()),
        "webp EXIF chunk body must be normalized to raw TIFF"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// CLI-level policy mapping: from_args mirrors the documented flag rules.
#[test]
fn cli_policy_flags_map_onto_policies() {
    use byteshaver::cli::{CliArgs, ExifPolicyArg};
    use clap::{Parser, ValueEnum};

    fn parse_args(extra: &[&str]) -> CliArgs {
        let mut all = vec!["byteshaver", "*.png", "jpeg"];
        all.extend_from_slice(extra);
        CliArgs::try_parse_from(all).expect("valid args")
    }

    // default strip
    let conf = ConversionConfig::from_args(&parse_args(&[]));
    assert_eq!(conf.exif, ExifPolicy::Strip);

    // keep
    let conf = ConversionConfig::from_args(&parse_args(&["--exif", "keep"]));
    assert_eq!(conf.exif, ExifPolicy::Keep);

    // filter + except (comma separated incl. IFD wildcard and hex number)
    let conf = ConversionConfig::from_args(&parse_args(&[
        "--exif",
        "filter",
        "--exif-except",
        "gps,GPSInfo,0x0112",
    ]));
    assert_eq!(
        conf.exif,
        ExifPolicy::FilterExcept(vec![
            TagSelector::Ifd(Ifd::Gps),
            TagSelector::Name("GPSInfo".to_string()),
            TagSelector::TagNum(0x0112, Ifd::Primary),
        ])
    );

    // filter + only
    let conf = ConversionConfig::from_args(&parse_args(&[
        "--exif",
        "filter",
        "--exif-only",
        "DateTimeOriginal",
    ]));
    assert_eq!(
        conf.exif,
        ExifPolicy::KeepOnly(vec![TagSelector::Name("DateTimeOriginal".to_string())])
    );

    // except requires filter (clap `requires` relation)
    let result = CliArgs::try_parse_from(["byteshaver", "*.png", "--exif-except", "gps", "jpeg"]);
    assert!(result.is_err(), "--exif-except without --exif must fail");

    // mutual exclusion (clap conflicts_with relation)
    let result = CliArgs::try_parse_from([
        "byteshaver",
        "*.png",
        "--exif",
        "filter",
        "--exif-except",
        "gps",
        "--exif-only",
        "Make",
        "jpeg",
    ]);
    assert!(result.is_err(), "--exif-except with --exif-only must fail");

    // policy value enum covers the documented spellings
    assert_eq!(
        ExifPolicyArg::from_str("keep", false).ok(),
        Some(ExifPolicyArg::Keep)
    );
    assert_eq!(
        ExifPolicyArg::from_str("strip", false).ok(),
        Some(ExifPolicyArg::Strip)
    );
    assert_eq!(
        ExifPolicyArg::from_str("filter", false).ok(),
        Some(ExifPolicyArg::Filter)
    );
}
