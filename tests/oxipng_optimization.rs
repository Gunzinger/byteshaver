//! Integration + property tests for the oxipng target command (plan WS3).
//!
//! All fixtures are generated programmatically with the `png` crate (and
//! oxipng itself for the interlaced fixture); no binary test assets needed.
#![cfg(feature = "opt-oxipng")]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use byteshaver::config::{
    ConversionConfig, EncoderConfig, OxipngFilter, OxipngLevel, OxipngOptions, PngOptions,
};
use byteshaver::run;

/// EXIF payload (minimal little-endian TIFF header with an empty IFD).
const EXIF_PAYLOAD: &[u8] = &[0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00];

/// Temporary root for generated fixtures, keyed per test name (tests run in
/// parallel within one process).
fn fixture_dir(test: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "byteshaver-oxipng-test-{test}-{}",
        std::process::id()
    ))
}

fn fixture_path(test: &str, name: &str) -> PathBuf {
    let dir = fixture_dir(test);
    fs::create_dir_all(&dir).expect("create fixture dir");
    dir.join(name)
}

fn clean_fixtures(test: &str) {
    let _ = fs::remove_dir_all(fixture_dir(test));
}

fn output_dir(name: &str) -> PathBuf {
    let dir = output_root(name);
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn output_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "byteshaver-oxipng-out-{}-{}",
        name,
        std::process::id()
    ))
}

/// Writes a PNG of the given color type/depth; `pixels` are raw scanlines.
fn write_png(
    path: &Path,
    width: u32,
    height: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    pixels: &[u8],
) {
    let file = fs::File::create(path).expect("create png file");
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(color);
    encoder.set_depth(depth);
    let mut writer = encoder.write_header().expect("write png header");
    writer.write_image_data(pixels).expect("write png data");
    writer.finish().expect("finish png");
}

/// 16-bit RGBA gradient (deterministic pattern, exercises 16-bit path).
fn make_16bit_rgba(path: &Path) {
    let (width, height) = (32u32, 16u32);
    let mut pixels = Vec::with_capacity((width * height * 8) as usize);
    for y in 0..height {
        for x in 0..width {
            let hi = ((x * 8) % 256) as u8;
            let lo = ((y * 16) % 256) as u8;
            // 16-bit samples with distinct high/low bytes: not losslessly
            // reducible to 8-bit, so reductions must keep pixels intact
            pixels.extend_from_slice(&[hi, lo, lo, hi, hi, lo, 0xFF, 0xFF]);
        }
    }
    write_png(
        path,
        width,
        height,
        png::ColorType::Rgba,
        png::BitDepth::Sixteen,
        &pixels,
    );
}

/// Palette PNG with a tRNS chunk (exercises palette/tRNS passthrough).
fn make_palette_trns(path: &Path) {
    let (width, height) = (24u32, 24u32);
    let file = fs::File::create(path).expect("create png file");
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_palette(
        [
            0xFF, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x00, 0x10, 0x20,
            0x30, 0xC0, 0xC0, 0xC0,
        ]
        .to_vec(),
    );
    encoder.set_trns(vec![0x00, 0x40, 0x80, 0xFF, 0xFF, 0xFF]);
    let mut writer = encoder.write_header().expect("write png header");
    let pixels: Vec<u8> = (0..(width * height)).map(|i| (i % 6) as u8).collect();
    writer.write_image_data(&pixels).expect("write png data");
    writer.finish().expect("finish png");
}

/// PNG carrying an eXIf chunk (for the later WS4 interplay).
fn make_exif_png(path: &Path) {
    let file = fs::File::create(path).expect("create png file");
    let mut info = png::Info::default();
    info.width = 8;
    info.height = 8;
    info.bit_depth = png::BitDepth::Eight;
    info.color_type = png::ColorType::Rgba;
    info.exif_metadata = Some(std::borrow::Cow::Borrowed(EXIF_PAYLOAD));
    let encoder = png::Encoder::with_info(file, info).expect("encoder with info");
    let mut writer = encoder.write_header().expect("write png header");
    let pixels = vec![77u8; 8 * 8 * 4];
    writer.write_image_data(&pixels).expect("write png data");
    writer.finish().expect("finish png");
}

/// APNG with two frames, written via the png crate's animation API.
fn make_apng(path: &Path) {
    let file = fs::File::create(path).expect("create png file");
    let mut encoder = png::Encoder::new(file, 8, 8);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_animated(2, 0).expect("set animation control");
    let mut writer = encoder.write_header().expect("write png header");
    for frame in 0..2u8 {
        let mut pixels = Vec::with_capacity(8 * 8 * 4);
        for _ in 0..(8 * 8) {
            pixels.extend_from_slice(&[frame * 120, 10, 20, 255]);
        }
        writer.write_image_data(&pixels).expect("write frame");
    }
    writer.finish().expect("finish apng");
}

/// Interlaced PNG, produced by optimizing a normal PNG with
/// `interlace = Some(true)` via oxipng itself (per plan §7).
fn make_interlaced(test: &str, path: &Path) {
    let plain = fixture_path(test, "interlaced-plain.png");
    let (width, height) = (16u32, 16u32);
    let pixels: Vec<u8> = (0..(width * height * 4))
        .map(|i| ((i * 7) % 251) as u8)
        .collect();
    write_png(
        &plain,
        width,
        height,
        png::ColorType::Rgba,
        png::BitDepth::Eight,
        &pixels,
    );

    let original = fs::read(&plain).expect("read plain png");
    let opts = oxipng::Options {
        interlace: Some(true),
        ..oxipng::Options::default()
    };
    let interlaced = oxipng::optimize_from_memory(&original, &opts)
        .expect("oxipng interlace fixture generation");
    fs::write(path, interlaced).expect("write interlaced fixture");
    let _ = fs::remove_file(&plain);
}

/// Noisy RGB fixture (poorly compressible, makes filter work meaningful).
fn make_noise(path: &Path) {
    let (width, height) = (256u32, 256u32);
    // xorshift for cheap deterministic pseudo-noise
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let pixels: Vec<u8> = (0..(width * height * 3)).map(|_| next() as u8).collect();
    write_png(
        path,
        width,
        height,
        png::ColorType::Rgb,
        png::BitDepth::Eight,
        &pixels,
    );
}

/// Generates the full fixture set of a test; returns the fixture paths.
fn make_all_fixtures(test: &str) -> Vec<PathBuf> {
    let dir = fixture_dir(test);
    fs::create_dir_all(&dir).expect("create fixture dir");
    let names = [
        "rgba16.png",
        "palette-trns.png",
        "exif.png",
        "interlaced.png",
        "apng.png",
        "noise.png",
    ];
    make_16bit_rgba(&fixture_path(test, names[0]));
    make_palette_trns(&fixture_path(test, names[1]));
    make_exif_png(&fixture_path(test, names[2]));
    make_interlaced(test, &fixture_path(test, names[3]));
    make_apng(&fixture_path(test, names[4]));
    make_noise(&fixture_path(test, names[5]));
    names.iter().map(|n| dir.join(n)).collect()
}

fn oxipng_encoder(options: OxipngOptions) -> Box<dyn byteshaver::converter::ImageEncoder> {
    byteshaver::converter::EncoderRegistry::build(
        &EncoderConfig::Oxipng(options),
        byteshaver::converter::ThreadBudget::global(),
    )
}

/// Runs the pipeline over the fixture directory of a test into a fresh
/// output directory.
fn run_over_fixtures(test: &str, options: OxipngOptions) -> byteshaver::RunStats {
    let pattern = fixture_dir(test).join("**").join("*.png");
    run(
        ConversionConfig {
            pattern: pattern.display().to_string(),
            output: output_dir(test).display().to_string(),
            ..ConversionConfig::default()
        },
        EncoderConfig::Oxipng(options),
    )
    .expect("run should succeed")
}

fn decode_pixels(path: &Path) -> image::DynamicImage {
    image::ImageReader::open(path)
        .expect("open image")
        .with_guessed_format()
        .expect("guess format")
        .decode()
        .expect("decode image")
}

const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn assert_valid_png(bytes: &[u8]) {
    assert!(bytes.len() > PNG_MAGIC.len(), "output too short");
    assert_eq!(&bytes[..PNG_MAGIC.len()], PNG_MAGIC, "not a png");
}

// ---- Property: passthrough bit-exactness --------------------------------

#[test]
fn passthrough_preserves_pixels_and_chunks() {
    let test = "bitexact";
    let fixtures = make_all_fixtures(test);
    let options = OxipngOptions::default();
    let stats = run_over_fixtures(test, options.clone());
    assert_eq!(stats.errors, 0, "no conversion errors");
    assert_eq!(stats.successful, fixtures.len() as u64);

    for input in &fixtures {
        let output = output_root(test).join(input.file_name().unwrap());
        assert!(output.exists(), "output missing for {}", input.display());

        // pixels decode identical (compared as normalized RGBA8 so that
        // representation-level reductions — e.g. RGBA -> palette+tRNS,
        // RGB -> grayscale — do not count as pixel changes)
        let original = decode_pixels(input);
        let optimized = decode_pixels(&output);
        assert_eq!(
            original.to_rgba8().into_raw(),
            optimized.to_rgba8().into_raw(),
            "pixels changed for {}",
            input.display()
        );

        // mode check: PNG input must be a passthrough, i.e. the ancillary
        // chunk structure of the source survives — with one policy-driven
        // exception: under the default EXIF `Strip` policy an unset
        // `--strip` is bumped to `safe`, which removes `eXIf` chunks
        // (plan WS3 §4 / WS4 §4). With a keep policy the chunk must survive.
        let original_bytes = fs::read(input).expect("read original");
        let output_bytes = fs::read(&output).expect("read output");
        assert_valid_png(&output_bytes);
        for chunk in ["tRNS", "acTL", "fcTL", "fdAT"] {
            let present_in_input = has_chunk(&original_bytes, chunk);
            let present_in_output = has_chunk(&output_bytes, chunk);
            assert_eq!(
                present_in_input,
                present_in_output,
                "chunk {chunk} presence changed for {}",
                input.display()
            );
        }
        #[cfg(feature = "exif")]
        {
            use byteshaver::metadata::policy::ExifPolicy;
            let e_x_if_in = has_chunk(&original_bytes, "eXIf");
            let e_x_if_out = has_chunk(&output_bytes, "eXIf");
            if e_x_if_in {
                match options.exif_policy {
                    ExifPolicy::Strip => {
                        assert!(
                            !e_x_if_out,
                            "eXIf should be stripped by default for {}",
                            input.display()
                        );
                    }
                    _ => {
                        assert!(
                            e_x_if_out,
                            "eXIf must survive under keep policy for {}",
                            input.display()
                        );
                    }
                }
            }
        }
    }
    clean_fixtures(test);
}

/// Walks the PNG chunk structure and reports whether a chunk of the given
/// (4-character) type is present anywhere in the file.
fn has_chunk(png: &[u8], name: &str) -> bool {
    assert_eq!(name.len(), 4);
    let mut offset = 8; // signature
    while offset + 8 <= png.len() {
        let len = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
        let chunk_type = &png[offset + 4..offset + 8];
        if chunk_type == name.as_bytes() {
            return true;
        }
        if chunk_type == b"IEND" {
            return false;
        }
        offset += 12 + len; // len + type + data + crc
    }
    false
}

// ---- Property: monotonic size --------------------------------------------

#[test]
fn passthrough_output_never_larger_than_original_or_best_encode() {
    let test = "sizes";
    let fixtures = make_all_fixtures(test);
    for input in &fixtures {
        let original_bytes = fs::read(input).expect("read original");
        let optimized = byteshaver::converter::oxipng::optimize_bytes(
            &original_bytes,
            &OxipngOptions::default(),
        )
        .expect("optimize fixture");
        assert!(
            optimized.len() <= original_bytes.len(),
            "{}: oxipng output {} > original {}",
            input.display(),
            optimized.len(),
            original_bytes.len()
        );

        // image-crate Best encode of the decoded pixels as an upper bound.
        // Only meaningful for like-for-like plain raster representations:
        // passthrough keeps palette/tRNS/eXIf/APNG chunk overhead the
        // baseline encode does not produce, and the APNG passthrough keeps
        // every frame while the baseline encode covers the first one.
        if input.file_name().unwrap() != "palette-trns.png"
            && input.file_name().unwrap() != "exif.png"
            && input.file_name().unwrap() != "apng.png"
        {
            let pixels = decode_pixels(input);
            let best = byteshaver::converter::EncoderRegistry::build(
                &EncoderConfig::Png(PngOptions {
                    compression_type: Some(byteshaver::config::CompressionType::Best),
                    filter_type: Some(byteshaver::config::FilterType::Adaptive),
                }),
                byteshaver::converter::ThreadBudget::global(),
            )
            .encode_still_image(&pixels)
            .expect("best encode");
            assert!(
                optimized.len() <= best.len(),
                "{}: oxipng output {} > image-Best encode {}",
                input.display(),
                optimized.len(),
                best.len()
            );
        }
    }
    clean_fixtures(test);
}

// ---- Property: determinism ------------------------------------------------

#[test]
fn optimization_is_deterministic() {
    let test = "determinism";
    let fixtures = make_all_fixtures(test);
    let options = OxipngOptions {
        filters: vec![OxipngFilter::None, OxipngFilter::Paeth],
        ..OxipngOptions::default()
    };
    for input in &fixtures {
        let bytes = fs::read(input).expect("read fixture");
        let first =
            byteshaver::converter::oxipng::optimize_bytes(&bytes, &options).expect("first run");
        let second =
            byteshaver::converter::oxipng::optimize_bytes(&bytes, &options).expect("second run");
        assert_eq!(
            first,
            second,
            "nondeterministic output for {}",
            input.display()
        );
    }
    clean_fixtures(test);
}

// ---- Property: level max never errors, timeout is respected ---------------

#[test]
fn level_max_never_errors_on_fixtures() {
    let test = "level-max";
    let fixtures = make_all_fixtures(test);
    let options = OxipngOptions {
        level: OxipngLevel::Max,
        ..OxipngOptions::default()
    };
    let stats = run_over_fixtures(test, options);
    assert_eq!(stats.errors, 0);
    assert_eq!(stats.successful, fixtures.len() as u64);
    clean_fixtures(test);
}

#[test]
fn timeout_bounds_optimization_time() {
    let test = "timeout";
    let fixtures = make_all_fixtures(test);
    let noise = fixtures.last().expect("noise fixture").clone();
    let bytes = fs::read(&noise).expect("read noise");
    let options = OxipngOptions {
        level: OxipngLevel::Max,
        timeout: Some(Duration::from_secs(1)),
        ..OxipngOptions::default()
    };
    let start = Instant::now();
    let result = byteshaver::converter::oxipng::optimize_bytes(&bytes, &options);
    let elapsed = start.elapsed();
    let optimized = result.expect("timed optimization must succeed");
    assert!(
        elapsed < Duration::from_secs(60),
        "timeout not respected: took {elapsed:?}"
    );
    assert_valid_png(&optimized);
    clean_fixtures(test);
}

#[test]
fn zopfli_deflater_produces_valid_deterministic_output() {
    let test = "zopfli";
    let fixtures = make_all_fixtures(test);
    let options = OxipngOptions {
        zopfli: true,
        zopfli_iterations: 1,
        timeout: Some(Duration::from_secs(30)),
        ..OxipngOptions::default()
    };
    for input in &fixtures {
        let bytes = fs::read(input).expect("read fixture");
        let first =
            byteshaver::converter::oxipng::optimize_bytes(&bytes, &options).expect("zopfli run");
        let second =
            byteshaver::converter::oxipng::optimize_bytes(&bytes, &options).expect("zopfli rerun");
        assert_valid_png(&first);
        assert_eq!(
            first,
            second,
            "zopfli nondeterministic for {}",
            input.display()
        );
    }
    clean_fixtures(test);
}

// ---- Integration: pipeline over examples/ ---------------------------------

#[test]
fn oxipng_passthrough_over_png_examples() {
    let mut base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    base.push("examples");
    base.push("**");
    base.push("*.png");
    let examples_png = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("png");
    let output = output_dir("examples-png");
    let stats = run(
        ConversionConfig {
            pattern: base.display().to_string(),
            output: output.display().to_string(),
            ..ConversionConfig::default()
        },
        EncoderConfig::Oxipng(OxipngOptions::default()),
    )
    .expect("run should succeed");

    assert_eq!(stats.input_files, 3, "expected the 3 example pngs");
    assert_eq!(stats.errors, 0);
    assert_eq!(stats.successful, 3);

    for entry in fs::read_dir(&examples_png).expect("examples/png exists") {
        let input = entry.expect("entry").path();
        if input.extension().and_then(|e| e.to_str()) != Some("png") {
            continue;
        }
        // the pattern base is "examples", so the png/ subdirectory is
        // preserved under the output directory
        let output_file = output.join("png").join(input.file_name().unwrap());
        assert!(
            output_file.exists(),
            "output missing: {}",
            output_file.display()
        );
        let input_size = fs::metadata(&input).expect("metadata").len();
        let output_size = fs::metadata(&output_file).expect("metadata").len();
        assert!(
            output_size <= input_size,
            "{}: output {output_size} > input {input_size}",
            input.display()
        );
        // pixels survive (normalized comparison, see fixture test)
        let original = decode_pixels(&input);
        let optimized = decode_pixels(&output_file);
        assert_eq!(
            original.to_rgba8().into_raw(),
            optimized.to_rgba8().into_raw(),
            "pixels changed for {}",
            input.display()
        );
    }

    let _ = fs::remove_dir_all(&output);
}

#[test]
fn oxipng_transcodes_jpg_examples_to_png() {
    let mut base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    base.push("examples");
    base.push("**");
    base.push("*.jpg");
    let output = output_dir("examples-jpg");
    let stats = run(
        ConversionConfig {
            pattern: base.display().to_string(),
            output: output.display().to_string(),
            ..ConversionConfig::default()
        },
        EncoderConfig::Oxipng(OxipngOptions::default()),
    )
    .expect("run should succeed");

    assert_eq!(stats.input_files, 10, "expected the 10 example jpgs");
    assert_eq!(stats.errors, 0);
    assert_eq!(stats.successful, 10);

    let jpg_output = output.join("jpg");
    for entry in fs::read_dir(&jpg_output).expect("jpg subdir exists") {
        let output_file = entry.expect("entry").path();
        assert_eq!(
            output_file.extension().and_then(|e| e.to_str()),
            Some("png"),
            "transcoded output must have png extension"
        );
        let bytes = fs::read(&output_file).expect("read output");
        assert_valid_png(&bytes);
        assert!(decode_pixels(&output_file).width() > 0);
    }

    let _ = fs::remove_dir_all(&output);
}

// ---- describe() -----------------------------------------------------------

#[test]
fn describe_prints_version_and_resolved_options() {
    let encoder = oxipng_encoder(OxipngOptions {
        level: OxipngLevel::Four,
        zopfli: true,
        ..OxipngOptions::default()
    });
    let description = encoder.describe();
    assert!(description.contains("oxipng"), "{description}");
    assert!(description.contains("10.2"), "{description}");
    assert!(description.contains("level=4"), "{description}");
    assert!(description.contains("zopfli=true"), "{description}");
}
