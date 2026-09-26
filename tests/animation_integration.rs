//! WS5 animation integration tests: synthetic animated fixtures (gif,
//! animated webp, APNG) generated in-test, round-trip matrix through the
//! new animated encoders, plus the global animation flags.
//!
//! Requires the default `anim-webp` + `anim-apng` features.

#![cfg(all(feature = "anim-webp", feature = "anim-apng"))]

use byteshaver::Error;
use byteshaver::config::{
    AnimatedInputPolicy, ApngOptions, ConversionConfig, EncoderConfig, GifOptions, WebpAnimOptions,
};
use byteshaver::converter::{EncoderRegistry, ThreadBudget};
use byteshaver::format::ImageFormat;
use byteshaver::input::{self, ImageContent, SourceImage};
use byteshaver::pipeline::run;
use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::metadata::LoopCount;
use image::{AnimationDecoder, Delay, Frame as AnimFrame, ImageDecoder, RgbaImage};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---- fixture generation -------------------------------------------------

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("byteshaver-ws5-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Frame `i` of the fixture canvas: solid color varying per frame plus a
/// moving marker so frames are pairwise distinct.
fn fixture_frame(width: u32, height: u32, i: u32) -> RgbaImage {
    let mut buffer = RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([
            (i * 40 % 256) as u8,
            30,
            200u8.saturating_sub((i * 17 % 200) as u8),
            255,
        ]),
    );
    for y in 0..height.min(4) {
        buffer.put_pixel(i % width, y, image::Rgba([255, 255, 0, 255]));
    }
    buffer
}

/// Writes a multi-frame GIF with the given per-frame delays (ms) and repeat.
fn write_gif(path: &Path, delays_ms: &[u32], repeat: Repeat) {
    let file = std::fs::File::create(path).expect("create gif");
    let mut encoder = GifEncoder::new(file);
    encoder.set_repeat(repeat).expect("set repeat");
    for (i, &delay) in delays_ms.iter().enumerate() {
        // GIF delays have 10 ms granularity; fixtures stay on that grid
        let cs = (delay / 10).max(1);
        let frame = AnimFrame::from_parts(
            fixture_frame(16, 16, i as u32),
            0,
            0,
            Delay::from_numer_denom_ms(cs * 10, 1),
        );
        encoder.encode_frame(frame).expect("encode gif frame");
    }
}

/// Writes an APNG with the given per-frame delays (ms) and num_plays.
fn write_apng(path: &Path, delays_ms: &[u32], num_plays: u32) {
    let mut output = Vec::new();
    let mut encoder = png::Encoder::new(&mut output, 16, 16);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .set_animated(delays_ms.len() as u32, num_plays)
        .unwrap();
    encoder.set_sep_def_img(true).unwrap();
    let mut writer = encoder.write_header().unwrap();
    // first image becomes the IDAT default image (sep_def_img), then the
    // animation frames follow as fcTL+fdAT
    writer
        .write_image_data(fixture_frame(16, 16, 0).as_raw())
        .unwrap();
    for (i, &delay) in delays_ms.iter().enumerate() {
        writer
            .set_frame_delay(delay.min(u16::MAX as u32) as u16, 1000)
            .unwrap();
        writer
            .write_image_data(fixture_frame(16, 16, i as u32).as_raw())
            .unwrap();
    }
    writer.finish().unwrap();
    std::fs::write(path, output).expect("write apng");
}

/// Builds an animated webp fixture by round-tripping through our own encoder.
fn make_animated_webp(delays_ms: &[u32], loop_count: LoopCount) -> Vec<u8> {
    let frames = delays_ms
        .iter()
        .enumerate()
        .map(|(i, &delay)| input::FrameData {
            buffer: fixture_frame(16, 16, i as u32),
            delay: Duration::from_millis(u64::from(delay)),
        })
        .collect();
    let animation = input::AnimationData {
        width: 16,
        height: 16,
        frames,
        loop_count,
    };
    let encoder = EncoderRegistry::build(
        &EncoderConfig::WebpAnim(WebpAnimOptions {
            lossless: true,
            quality: 10.,
            ..Default::default()
        }),
        ThreadBudget::global(),
    );
    encoder
        .encode(&SourceImage {
            content: ImageContent::Animated(animation),
            metadata: Default::default(),
            source_format: ImageFormat::Gif,
            source_path: PathBuf::from("fixture.gif"),
        })
        .expect("encode animated webp fixture")
}

// ---- decode helpers -----------------------------------------------------

/// A decoded animation: RGBA buffers, delays (ms), loop count.
struct DecodedAnimation {
    frames: Vec<(RgbaImage, u64)>,
    loop_count: LoopCount,
}

fn frame_delay_ms(frame: &image::Frame) -> u64 {
    let (numer, denom) = frame.delay().numer_denom_ms();
    u64::from(numer) / u64::from(denom.max(1))
}

fn decode_webp_animation(bytes: &[u8]) -> DecodedAnimation {
    let decoder = WebPDecoder::new(Cursor::new(bytes)).expect("webp decoder");
    assert!(decoder.has_animation(), "expected an animated webp");
    let loop_count = decoder.loop_count();
    let frames: Vec<(RgbaImage, u64)> = decoder
        .into_frames()
        .map(|frame| {
            let frame = frame.expect("webp frame");
            let delay = frame_delay_ms(&frame);
            (frame.into_buffer(), delay)
        })
        .collect();
    DecodedAnimation { frames, loop_count }
}

fn decode_apng_animation(bytes: &[u8]) -> DecodedAnimation {
    let decoder = PngDecoder::new(Cursor::new(bytes)).expect("png decoder");
    assert!(
        decoder.is_apng().expect("is_apng"),
        "expected an animated png"
    );
    let apng_decoder = decoder.apng().expect("apng decoder");
    let loop_count = apng_decoder.loop_count();
    let frames: Vec<(RgbaImage, u64)> = AnimationDecoder::into_frames(apng_decoder)
        .map(|frame| {
            let frame = frame.expect("apng frame");
            let delay = frame_delay_ms(&frame);
            (frame.into_buffer(), delay)
        })
        .collect();
    DecodedAnimation { frames, loop_count }
}

fn decode_gif_animation(bytes: &[u8]) -> DecodedAnimation {
    let decoder = GifDecoder::new(Cursor::new(bytes)).expect("gif decoder");
    let loop_count = decoder.loop_count();
    let frames: Vec<(RgbaImage, u64)> = decoder
        .into_frames()
        .map(|frame| {
            let frame = frame.expect("gif frame");
            let delay = frame_delay_ms(&frame);
            (frame.into_buffer(), delay)
        })
        .collect();
    DecodedAnimation { frames, loop_count }
}

/// Encodes a fixture source through the encoder for `config`.
fn encode_source(source: &SourceImage, config: &EncoderConfig) -> Result<Vec<u8>, Error> {
    let encoder = EncoderRegistry::build(config, ThreadBudget::global());
    encoder.encode(source)
}

fn delays_of(decoded: &DecodedAnimation) -> Vec<u64> {
    decoded.frames.iter().map(|(_, ms)| *ms).collect()
}

/// `LoopCount` implements neither `PartialEq` nor `Debug`.
fn assert_loop_count(loop_count: &LoopCount, finite: Option<u32>) {
    match (loop_count, finite) {
        (LoopCount::Infinite, None) => {}
        (LoopCount::Finite(n), Some(want)) => assert_eq!(n.get(), want),
        other => panic!(
            "unexpected loop count: {}",
            match other {
                (LoopCount::Infinite, _) => "infinite".to_string(),
                (LoopCount::Finite(n), _) => format!("finite({})", n.get()),
            }
        ),
    }
}

fn assert_delays_close(actual: &[u64], expected: &[u32], tolerance_ms: u64) {
    assert_eq!(actual.len(), expected.len(), "frame count mismatch");
    for (i, (&got, &want)) in actual.iter().zip(expected).enumerate() {
        let want = u64::from(want);
        assert!(
            got.abs_diff(want) <= tolerance_ms,
            "frame {i}: delay {got} ms differs from {want} ms by more than {tolerance_ms} ms"
        );
    }
}

// ---- round-trip matrix --------------------------------------------------

#[test]
fn gif_to_apng_round_trip_frames_delays_and_loop_count() {
    let dir = temp_dir("gif-apng");
    let gif = dir.join("anim.gif");
    write_gif(&gif, &[100, 250, 500], Repeat::Infinite);

    let source = input::load_source(&gif).expect("load gif");
    let ImageContent::Animated(animation) = &source.content else {
        panic!("expected animated gif");
    };
    assert_eq!(animation.frames.len(), 3);
    assert_loop_count(&animation.loop_count, None);

    let apng = encode_source(&source, &EncoderConfig::Apng(ApngOptions::default())).expect("apng");
    let decoded = decode_apng_animation(&apng);
    assert_eq!(decoded.frames.len(), 3);
    assert_delays_close(&delays_of(&decoded), &[100, 250, 500], 1);
    assert_loop_count(&decoded.loop_count, None);

    // finite loop count survives as num_plays
    let gif2 = dir.join("anim2.gif");
    write_gif(&gif2, &[80, 120], Repeat::Finite(2));
    let source2 = input::load_source(&gif2).expect("load gif2");
    let apng2 =
        encode_source(&source2, &EncoderConfig::Apng(ApngOptions::default())).expect("apng2");
    let decoded2 = decode_apng_animation(&apng2);
    assert_loop_count(&decoded2.loop_count, Some(2));
    // first frame pixels stay identical on the lossless chain
    let ImageContent::Animated(animation2) = &source2.content else {
        panic!("expected animated gif2");
    };
    assert_eq!(decoded2.frames[0].0, animation2.frames[0].buffer);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gif_to_webp_round_trip_frames_delays_and_loop_count() {
    let dir = temp_dir("gif-webp");
    let gif = dir.join("anim.gif");
    write_gif(&gif, &[100, 250, 500], Repeat::Infinite);
    let source = input::load_source(&gif).expect("load gif");

    let options = WebpAnimOptions {
        lossless: true,
        quality: 10.,
        ..Default::default()
    };
    let webp = encode_source(&source, &EncoderConfig::WebpAnim(options)).expect("webp");
    let decoded = decode_webp_animation(&webp);
    assert_eq!(decoded.frames.len(), 3);
    assert_delays_close(&delays_of(&decoded), &[100, 250, 500], 10);
    // webp-animation 0.10 carries the source loop count via anim_params
    assert_loop_count(&decoded.loop_count, None);

    // finite loop count
    let gif2 = dir.join("anim2.gif");
    write_gif(&gif2, &[80, 120], Repeat::Finite(3));
    let source2 = input::load_source(&gif2).expect("load gif2");
    let webp2 = encode_source(&source2, &EncoderConfig::WebpAnim(options)).expect("webp2");
    let decoded2 = decode_webp_animation(&webp2);
    assert_loop_count(&decoded2.loop_count, Some(3));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gif_to_gif_round_trip_frames_delays_and_loop_count() {
    let dir = temp_dir("gif-gif");
    let gif = dir.join("anim.gif");
    write_gif(&gif, &[100, 250, 500], Repeat::Finite(2));
    let source = input::load_source(&gif).expect("load gif");

    let gif_bytes =
        encode_source(&source, &EncoderConfig::Gif(GifOptions::default())).expect("gif");
    let decoded = decode_gif_animation(&gif_bytes);
    assert_eq!(decoded.frames.len(), 3);
    // GIF delays are 10 ms granular: exact on-grid fixtures stay exact
    assert_delays_close(&delays_of(&decoded), &[100, 250, 500], 10);
    assert_loop_count(&decoded.loop_count, Some(2));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn webp_to_apng_round_trip_preserves_pixels_and_timing() {
    let dir = temp_dir("webp-apng");
    let webp = make_animated_webp(&[80, 120, 160], LoopCount::Infinite);
    let webp_path = dir.join("anim.webp");
    std::fs::write(&webp_path, &webp).expect("write webp fixture");

    let source = input::load_source(&webp_path).expect("load animated webp");
    assert_eq!(source.source_format, ImageFormat::Webp);
    let ImageContent::Animated(animation) = &source.content else {
        panic!("expected animated webp content");
    };
    assert_eq!(animation.frames.len(), 3);

    let apng = encode_source(&source, &EncoderConfig::Apng(ApngOptions::default())).expect("apng");
    let decoded = decode_apng_animation(&apng);
    assert_eq!(decoded.frames.len(), 3);
    assert_delays_close(&delays_of(&decoded), &[80, 120, 160], 1);
    // lossless webp chain: frame pixels survive exactly
    for (i, (buffer, _)) in decoded.frames.iter().enumerate() {
        assert_eq!(
            *buffer, animation.frames[i].buffer,
            "frame {i} pixels differ"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- still paths --------------------------------------------------------

#[test]
fn single_frame_gif_resolves_to_still() {
    let dir = temp_dir("gif-still");
    let gif = dir.join("one.gif");
    write_gif(&gif, &[100], Repeat::Infinite);

    let source = input::load_source(&gif).expect("load gif");
    assert!(
        matches!(source.content, ImageContent::Still(_)),
        "1-frame gif must resolve to the fast still path"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn single_frame_webp_and_apng_resolve_to_still() {
    let dir = temp_dir("still-collapse");
    // 1-frame APNG
    let apng_path = dir.join("one.png");
    write_apng(&apng_path, &[100], 0);
    let source = input::load_source(&apng_path).expect("load apng");
    assert!(matches!(source.content, ImageContent::Still(_)));

    // 1-frame animated webp (via our encoder)
    let webp = make_animated_webp(&[100], LoopCount::Infinite);
    let webp_path = dir.join("one.webp");
    std::fs::write(&webp_path, webp).expect("write webp");
    let source = input::load_source(&webp_path).expect("load webp");
    assert!(matches!(source.content, ImageContent::Still(_)));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn still_input_to_animated_targets_makes_single_frame_animations() {
    let dir = temp_dir("still-to-anim");
    let png_path = dir.join("still.png");
    let rgba = RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]));
    image::save_buffer(
        &png_path,
        rgba.as_raw(),
        8,
        8,
        image::ExtendedColorType::Rgba8,
    )
    .expect("write png");

    let source = input::load_source(&png_path).expect("load png");

    // webp-anim: still in, valid 1-frame webp out. libwebp optimizes a
    // single-frame animation into a plain still webp (OptimizeSingleFrame);
    // either way the output must decode and preserve the pixels.
    let options = WebpAnimOptions {
        lossless: true,
        quality: 10.,
        ..Default::default()
    };
    let webp = encode_source(&source, &EncoderConfig::WebpAnim(options)).expect("webp");
    let decoded = image::load_from_memory(&webp).expect("valid webp output");
    assert_eq!(decoded.to_rgba8(), rgba);

    // APNG: still in, valid 1-frame APNG out
    let apng = encode_source(&source, &EncoderConfig::Apng(ApngOptions::default())).expect("apng");
    let decoded = decode_apng_animation(&apng);
    assert_eq!(decoded.frames.len(), 1);

    // GIF: still in, valid 1-frame gif out
    let gif = encode_source(&source, &EncoderConfig::Gif(GifOptions::default())).expect("gif");
    let decoded = decode_gif_animation(&gif);
    assert_eq!(decoded.frames.len(), 1);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- global flags -------------------------------------------------------

#[test]
fn memory_guard_errors_when_projection_exceeds_cap() {
    let dir = temp_dir("mem-guard");
    let gif = dir.join("anim.gif");
    write_gif(&gif, &[100, 100, 100], Repeat::Infinite);

    let out = dir.join("out");
    let stats = run(
        ConversionConfig {
            pattern: format!("{}/*.gif", dir.display()),
            output: out.display().to_string(),
            max_animation_memory_mib: 0, // reject any animation
            ..ConversionConfig::default()
        },
        EncoderConfig::WebpAnim(WebpAnimOptions::default()),
    )
    .expect("run");
    assert_eq!(stats.input_files, 1);
    assert_eq!(stats.errors, 1);
    assert_eq!(stats.successful, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn animated_input_error_mode_rejects_animated_input_to_still_target() {
    let dir = temp_dir("anim-error");
    let gif = dir.join("anim.gif");
    write_gif(&gif, &[100, 100], Repeat::Infinite);

    let out = dir.join("out");
    let stats = run(
        ConversionConfig {
            pattern: format!("{}/*.gif", dir.display()),
            output: out.display().to_string(),
            animated_input: AnimatedInputPolicy::Error,
            ..ConversionConfig::default()
        },
        EncoderConfig::Jpeg,
    )
    .expect("run");
    assert_eq!(stats.errors, 1, "animated input must error in error mode");
    assert_eq!(stats.successful, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn animated_input_first_frame_default_encodes_still_target() {
    let dir = temp_dir("anim-first-frame");
    let gif = dir.join("anim.gif");
    write_gif(&gif, &[100, 100], Repeat::Infinite);

    let out = dir.join("out");
    let stats = run(
        ConversionConfig {
            pattern: format!("{}/*.gif", dir.display()),
            output: out.display().to_string(),
            ..ConversionConfig::default()
        },
        EncoderConfig::Jpeg,
    )
    .expect("run");
    assert_eq!(
        stats.successful, 1,
        "default policy encodes the first frame"
    );
    assert_eq!(stats.errors, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn animated_gif_to_webp_anim_through_pipeline() {
    let dir = temp_dir("pipeline-webpanim");
    let gif = dir.join("anim.gif");
    write_gif(&gif, &[100, 200], Repeat::Infinite);

    let out = dir.join("out");
    let stats = run(
        ConversionConfig {
            pattern: format!("{}/*.gif", dir.display()),
            output: out.display().to_string(),
            ..ConversionConfig::default()
        },
        EncoderConfig::WebpAnim(WebpAnimOptions {
            lossless: true,
            quality: 10.,
            ..Default::default()
        }),
    )
    .expect("run");
    assert_eq!(stats.successful, 1);

    let output = out.join("anim.webp");
    let bytes = std::fs::read(output).expect("output file");
    let decoded = decode_webp_animation(&bytes);
    assert_eq!(decoded.frames.len(), 2);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- container details --------------------------------------------------

#[test]
fn animated_webp_embeds_exif_payload() {
    let tiff = b"II*\0\x08\0\0\0\0\0";
    let frames = vec![input::FrameData {
        buffer: fixture_frame(8, 8, 0),
        delay: Duration::from_millis(100),
    }];
    let animation = input::AnimationData {
        width: 8,
        height: 8,
        frames,
        loop_count: LoopCount::Infinite,
    };
    let encoder = EncoderRegistry::build(
        &EncoderConfig::WebpAnim(WebpAnimOptions {
            lossless: true,
            quality: 10.,
            ..Default::default()
        }),
        ThreadBudget::global(),
    );
    let webp = encoder
        .encode(&SourceImage {
            content: ImageContent::Animated(animation),
            metadata: byteshaver::metadata::ImageMetadata {
                exif: Some(tiff.to_vec()),
                ..Default::default()
            },
            source_format: ImageFormat::Gif,
            source_path: PathBuf::from("fixture.gif"),
        })
        .expect("encode");
    assert_eq!(
        byteshaver::metadata::riff::extract_exif_payload(&webp).as_deref(),
        Some(tiff.as_slice()),
        "EXIF chunk must be muxed into the animated container"
    );
}

#[test]
fn apng_exif_embedding_via_exif_chunk() {
    let dir = temp_dir("apng-exif");
    let gif = dir.join("anim.gif");
    write_gif(&gif, &[100, 100], Repeat::Infinite);
    let mut source = input::load_source(&gif).expect("load gif");
    source.metadata.exif = Some(b"II*\0\x08\0\0\0\0\0".to_vec());

    let apng = encode_source(&source, &EncoderConfig::Apng(ApngOptions::default())).expect("apng");
    // the payload rides in the eXIf chunk of the default image header and
    // must survive a re-decode
    let mut decoder = PngDecoder::new(Cursor::new(&apng)).expect("png decoder");
    assert_eq!(
        decoder.exif_metadata().expect("exif").as_deref(),
        Some(&b"II*\0\x08\0\0\0\0\0"[..])
    );
    // and the animation still decodes
    let decoded = decode_apng_animation(&apng);
    assert_eq!(decoded.frames.len(), 2);

    let _ = std::fs::remove_dir_all(&dir);
}
