//! WS2 integration tests: JPEG XL round-trips through jxl-oxide (the
//! independent decoder acts as the oracle), option surface behavior and
//! animated encode/decode with real frame durations.

#![cfg(feature = "jxl")]

use std::path::PathBuf;
use std::time::Duration;

use byteshaver::config::{EncoderConfig, JxlBitDepthChoice, JxlColorEncodingChoice, JxlOptions};
use byteshaver::converter::traits::{EncoderRegistry, ImageEncoder, ThreadBudget};
use byteshaver::input::{AnimationData, FrameData, ImageContent, SourceImage, load_source};
use byteshaver::metadata::ImageMetadata;
use byteshaver::metadata::policy::ExifPolicy;
use image::metadata::LoopCount;
use image::{ExtendedColorType, RgbaImage};

fn jxl_encoder(options: JxlOptions) -> Box<dyn ImageEncoder> {
    EncoderRegistry::build(&EncoderConfig::Jxl(options), ThreadBudget::global())
}

fn temp_file(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("byteshaver-jxl-test-{}-{name}", std::process::id()));
    path
}

/// Deterministic patterned RGBA image (varied values incl. alpha).
fn pattern_rgba(size: u32, seed: u8) -> RgbaImage {
    RgbaImage::from_fn(size, size, |x, y| {
        let v = (x * 7 + y * 13).wrapping_add(u32::from(seed) * 31) as u8;
        image::Rgba([v, v.rotate_left(3), !v, 64 + v / 2])
    })
}

fn write_png_rgba(path: &std::path::Path, image: &RgbaImage) {
    image::save_buffer(
        path,
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::Rgba8,
    )
    .expect("write png fixture");
}

fn still_source(image: RgbaImage, metadata: ImageMetadata) -> SourceImage {
    SourceImage {
        content: ImageContent::Still(image::DynamicImage::ImageRgba8(image)),
        metadata,
        source_format: byteshaver::format::ImageFormat::Png,
        source_path: PathBuf::from("test.png"),
    }
}

#[test]
fn encoder_reports_format_animation_and_options() {
    let encoder = jxl_encoder(JxlOptions {
        effort: 9,
        quality: Some(80.0),
        ..JxlOptions::default()
    });
    assert_eq!(encoder.format(), byteshaver::format::ImageFormat::Jxl);
    assert_eq!(encoder.extension(), "jxl");
    assert!(encoder.supports_animation());
    // plan 15 F4: the capability description is option-free identity
    let description = encoder.describe();
    assert!(description.contains("libjxl"), "{description}");
    assert!(!description.contains("effort=9"), "{description}");
    assert!(!description.contains("quality=80"), "{description}");
    // the per-run notice keeps the resolved options
    let options = encoder.describe_options();
    assert!(options.contains("libjxl"), "{options}");
    assert!(options.contains("effort=9"), "{options}");
    assert!(options.contains("quality=80"), "{options}");
}

#[test]
fn lossless_round_trip_preserves_rgba_pixels() {
    let original = pattern_rgba(33, 1);
    let source = still_source(original.clone(), ImageMetadata::default());
    let encoded = jxl_encoder(JxlOptions {
        lossless: true,
        effort: 1, // fastest: pixel equality is what matters here
        ..JxlOptions::default()
    })
    .encode(&source)
    .expect("lossless jxl encode");
    assert_eq!(&encoded[..2], &[0xFF, 0x0A], "naked codestream signature");

    let path = temp_file("lossless.jxl");
    std::fs::write(&path, &encoded).expect("write jxl");
    let decoded = load_source(&path).expect("decode jxl");
    assert_eq!(decoded.source_format, byteshaver::format::ImageFormat::Jxl);
    let ImageContent::Still(image) = &decoded.content else {
        panic!("expected still content");
    };
    assert_eq!(image.to_rgba8(), original, "pixels must round-trip");
    // the renderer applied orientation; nothing was rotated (identity)
    assert!(decoded.metadata.exif_applied_orientation);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sixteen_bit_path_round_trips() {
    // gray ramp u16 pattern: full 16-bit range in R, wrapping G, opaque B
    let original = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_fn(16, 16, |x, y| {
        let i = (x + y * 16) as u16;
        image::Rgb([
            i.wrapping_mul(4369),
            i.wrapping_mul(257),
            i.wrapping_mul(4369).wrapping_neg(),
        ])
    });
    let source = SourceImage {
        content: ImageContent::Still(image::DynamicImage::ImageRgb16(original.clone())),
        metadata: ImageMetadata::default(),
        source_format: byteshaver::format::ImageFormat::Png,
        source_path: PathBuf::from("test16.png"),
    };
    let encoded = jxl_encoder(JxlOptions {
        lossless: true,
        bit_depth: Some(JxlBitDepthChoice::Sixteen),
        effort: 1,
        ..JxlOptions::default()
    })
    .encode(&source)
    .expect("16-bit lossless jxl encode");

    let path = temp_file("rgb16.jxl");
    std::fs::write(&path, &encoded).expect("write jxl");
    let decoded = load_source(&path).expect("decode jxl");
    let ImageContent::Still(image) = &decoded.content else {
        panic!("expected still content");
    };
    assert_eq!(
        image,
        &image::DynamicImage::ImageRgb16(original),
        "16-bit samples must round-trip exactly"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn png_to_lossless_jxl_via_load_source() {
    let png_path = temp_file("source.png");
    let original = pattern_rgba(17, 7);
    write_png_rgba(&png_path, &original);

    let source = load_source(&png_path).expect("load png");
    let encoded = jxl_encoder(JxlOptions {
        lossless: true,
        effort: 2,
        ..JxlOptions::default()
    })
    .encode(&source)
    .expect("encode");

    let jxl_path = temp_file("roundtrip.jxl");
    std::fs::write(&jxl_path, &encoded).expect("write jxl");
    let decoded = load_source(&jxl_path).expect("decode jxl");
    let ImageContent::Still(image) = &decoded.content else {
        panic!("expected still content");
    };
    assert_eq!(image.to_rgba8(), original);
    let _ = std::fs::remove_file(&png_path);
    let _ = std::fs::remove_file(&jxl_path);
}

#[test]
fn animated_round_trip_preserves_frames_and_delays() {
    let delays = [40, 80, 120];
    let frames: Vec<FrameData> = delays
        .iter()
        .enumerate()
        .map(|(i, &ms)| FrameData {
            buffer: pattern_rgba(8, u8::try_from(i).expect("small index")),
            delay: Duration::from_millis(ms),
        })
        .collect();
    let animation = AnimationData {
        width: 8,
        height: 8,
        frames,
        loop_count: LoopCount::Finite(std::num::NonZeroU32::new(2).expect("2 is non-zero")),
    };
    let source = SourceImage {
        content: ImageContent::Animated(animation),
        metadata: ImageMetadata::default(),
        source_format: byteshaver::format::ImageFormat::Gif,
        source_path: PathBuf::from("test.gif"),
    };

    let encoded = jxl_encoder(JxlOptions {
        lossless: true,
        effort: 1,
        ..JxlOptions::default()
    })
    .encode(&source)
    .expect("animated jxl encode");

    let path = temp_file("animated.jxl");
    std::fs::write(&path, &encoded).expect("write jxl");
    let decoded = load_source(&path).expect("decode animated jxl");
    let ImageContent::Animated(result) = &decoded.content else {
        panic!("expected animated content");
    };
    assert_eq!(result.frames.len(), 3, "frame count must round-trip");
    for (frame, &ms) in result.frames.iter().zip(&delays) {
        // 1 ms tick timescale on both sides; tolerate +-1 ms rounding
        let diff = frame.delay.as_millis().abs_diff(u128::from(ms));
        assert!(
            diff <= 1,
            "delay {}ms decoded as {:?} (diff {diff})",
            ms,
            frame.delay
        );
    }
    assert!(matches!(
        result.loop_count,
        LoopCount::Finite(loops) if loops.get() == 2
    ));
    for (i, frame) in result.frames.iter().enumerate() {
        let expected = pattern_rgba(8, u8::try_from(i).expect("small index"));
        assert_eq!(frame.buffer, expected, "frame {i} pixels must round-trip");
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn infinite_loop_count_round_trips() {
    let animation = AnimationData {
        width: 4,
        height: 4,
        frames: vec![FrameData {
            buffer: RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])),
            delay: Duration::from_millis(100),
        }],
        loop_count: LoopCount::Infinite,
    };
    let source = SourceImage {
        content: ImageContent::Animated(animation),
        metadata: ImageMetadata::default(),
        source_format: byteshaver::format::ImageFormat::Gif,
        source_path: PathBuf::from("loop.gif"),
    };
    let encoded = jxl_encoder(JxlOptions {
        lossless: true,
        effort: 1,
        ..JxlOptions::default()
    })
    .encode(&source)
    .expect("encode");

    let path = temp_file("infinite.jxl");
    std::fs::write(&path, &encoded).expect("write jxl");
    let decoded = load_source(&path).expect("decode");
    let ImageContent::Animated(result) = &decoded.content else {
        panic!("expected animated content");
    };
    assert!(matches!(result.loop_count, LoopCount::Infinite));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn exif_policy_keep_embeds_box_round_trip() {
    // minimal valid little-endian TIFF: header + zero IFD entries
    let tiff = b"II*\0\x08\0\0\0\0\0".to_vec();
    let source = still_source(
        pattern_rgba(8, 3),
        ImageMetadata {
            exif: Some(tiff.clone()),
            ..ImageMetadata::default()
        },
    );
    let encoder = jxl_encoder(JxlOptions {
        lossless: true,
        effort: 1,
        exif_policy: ExifPolicy::Keep,
        ..JxlOptions::default()
    });
    let encoded = encoder.encode(&source).expect("encode with exif box");

    let path = temp_file("exif.jxl");
    std::fs::write(&path, &encoded).expect("write jxl");
    let decoded = load_source(&path).expect("decode");
    assert_eq!(
        decoded.metadata.exif.as_deref(),
        Some(tiff.as_slice()),
        "EXIF payload must survive the jxl box round trip"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn exif_policy_strip_drops_metadata() {
    let tiff = b"II*\0\x08\0\0\0\0\0".to_vec();
    let source = still_source(
        pattern_rgba(8, 4),
        ImageMetadata {
            exif: Some(tiff),
            ..ImageMetadata::default()
        },
    );
    let encoded = jxl_encoder(JxlOptions::default())
        .encode(&source)
        .expect("encode");

    let path = temp_file("stripped.jxl");
    std::fs::write(&path, &encoded).expect("write jxl");
    let decoded = load_source(&path).expect("decode");
    assert_eq!(decoded.metadata.exif, None, "strip policy must drop EXIF");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn unknown_setting_id_errors_listing_available() {
    let source = still_source(pattern_rgba(8, 5), ImageMetadata::default());
    let error = jxl_encoder(JxlOptions {
        advanced: vec![("definitely_not_an_id".to_string(), 1)],
        ..JxlOptions::default()
    })
    .encode(&source)
    .expect_err("unknown setting id must fail");
    let message = error.to_string();
    assert!(message.contains("definitely_not_an_id"), "{message}");
    assert!(message.contains("brotli_effort"), "{message}");
    assert!(message.contains("effort"), "{message}");
}

#[test]
fn known_setting_passthrough_encodes() {
    let source = still_source(pattern_rgba(8, 6), ImageMetadata::default());
    let encoded = jxl_encoder(JxlOptions {
        advanced: vec![
            ("BROTLI_EFFORT".to_string(), 4), // case-insensitive resolution
            ("modular".to_string(), 1),
        ],
        ..JxlOptions::default()
    })
    .encode(&source)
    .expect("known setting ids must resolve");
    assert!(!encoded.is_empty());
}

#[test]
fn icc_passthrough_without_profile_falls_back_to_srgb() {
    let source = still_source(pattern_rgba(8, 8), ImageMetadata::default());
    let encoded = jxl_encoder(JxlOptions {
        color_encoding: Some(JxlColorEncodingChoice::IccPassthrough),
        effort: 1,
        ..JxlOptions::default()
    })
    .encode(&source)
    .expect("fallback encode without ICC");
    assert!(!encoded.is_empty());
}

#[test]
fn luma_color_encoding_rejects_color_input() {
    let source = still_source(pattern_rgba(8, 9), ImageMetadata::default());
    let error = jxl_encoder(JxlOptions {
        color_encoding: Some(JxlColorEncodingChoice::SrgbLuma),
        ..JxlOptions::default()
    })
    .encode(&source)
    .expect_err("luma color encoding requires grayscale input");
    assert!(error.to_string().contains("grayscale"));
}

#[test]
fn gray_input_round_trips_as_grayscale() {
    let original = image::GrayImage::from_fn(12, 12, |x, y| image::Luma([(x * 13 + y * 29) as u8]));
    let source = SourceImage {
        content: ImageContent::Still(image::DynamicImage::ImageLuma8(original.clone())),
        metadata: ImageMetadata::default(),
        source_format: byteshaver::format::ImageFormat::Png,
        source_path: PathBuf::from("gray.png"),
    };
    let encoded = jxl_encoder(JxlOptions {
        lossless: true,
        effort: 1,
        ..JxlOptions::default()
    })
    .encode(&source)
    .expect("encode gray");

    let path = temp_file("gray.jxl");
    std::fs::write(&path, &encoded).expect("write jxl");
    let decoded = load_source(&path).expect("decode");
    let ImageContent::Still(image) = &decoded.content else {
        panic!("expected still content");
    };
    assert_eq!(
        image,
        &image::DynamicImage::ImageLuma8(original),
        "grayscale layout must be preserved"
    );
    let _ = std::fs::remove_file(&path);
}
