//! Integration tests: run the conversion pipeline over the `examples/`
//! directory for every encoder and assert on the resulting statistics and
//! output files.

use std::fs;
use std::path::{Path, PathBuf};

use byteshaver::config::{AvifOptions, ConversionConfig, EncoderConfig, PngOptions, WebpOptions};
use byteshaver::{RunStats, run};

/// All example images: 3 png, 2 jpeg, 10 jpg files.
const EXAMPLE_INPUT_COUNT: u64 = 15;

fn examples_pattern() -> String {
    let mut base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    base.push("examples");
    base.push("**");
    base.push("*.*");
    base.display().to_string()
}

fn output_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "byteshaver-integration-{}-{}",
        name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn conversion_config(output: &Path) -> ConversionConfig {
    ConversionConfig {
        pattern: examples_pattern(),
        output: output.display().to_string(),
        ..ConversionConfig::default()
    }
}

fn assert_conversion(name: &str, enc: EncoderConfig) -> RunStats {
    let output = output_dir(name);
    let stats = run(conversion_config(&output), enc).expect("run should succeed");

    assert_eq!(
        stats.input_files, EXAMPLE_INPUT_COUNT,
        "{}: input count",
        name
    );
    assert_eq!(
        stats.successful, EXAMPLE_INPUT_COUNT,
        "{}: all files encoded",
        name
    );
    assert_eq!(stats.skipped, 0, "{}: nothing skipped", name);
    assert_eq!(stats.errors, 0, "{}: no errors", name);
    assert_eq!(stats.discarded, 0, "{}: nothing discarded", name);
    assert_eq!(stats.collisions, 0, "{}: no collisions", name);
    assert!(stats.output_size > 0, "{}: output size counted", name);
    assert!(stats.elapsed.as_nanos() > 0);

    let outputs = count_output_files(&output);
    assert_eq!(
        outputs, EXAMPLE_INPUT_COUNT as usize,
        "{}: output file count",
        name
    );
    assert_output_sizes_match(&stats, &output, name);

    let _ = fs::remove_dir_all(&output);
    stats
}

fn count_output_files(output: &Path) -> usize {
    walkdir(output)
}

fn walkdir(dir: &Path) -> usize {
    let mut count = 0;
    for entry in fs::read_dir(dir).expect("output dir exists") {
        let entry = entry.expect("read entry");
        let path = entry.path();
        if path.is_dir() {
            count += walkdir(&path);
        } else {
            count += 1;
            assert!(
                fs::metadata(&path).expect("metadata").len() > 0,
                "{}: output file must be non-trivial",
                path.display()
            );
        }
    }
    count
}

fn assert_output_sizes_match(stats: &RunStats, output: &Path, name: &str) {
    let mut total = 0;
    for entry in fs::read_dir(output).expect("output dir exists") {
        let entry = entry.expect("read entry");
        let path = entry.path();
        if path.is_dir() {
            for sub in fs::read_dir(&path).expect("subdir exists") {
                total += fs::metadata(sub.expect("sub entry").path())
                    .expect("metadata")
                    .len();
            }
        } else {
            total += fs::metadata(&path).expect("metadata").len();
        }
    }
    assert_eq!(
        total, stats.output_size,
        "{}: stats match written bytes",
        name
    );
}

#[test]
fn webp_conversion_over_examples() {
    assert_conversion(
        "webp",
        EncoderConfig::Webp(WebpOptions {
            lossless: false,
            quality: 90.0,
        }),
    );
}

#[test]
fn webp_image_conversion_over_examples() {
    assert_conversion("webp-image", EncoderConfig::WebpImage);
}

#[test]
fn avif_conversion_over_examples() {
    assert_conversion(
        "avif",
        EncoderConfig::Avif(AvifOptions {
            quality: 90.0,
            speed: 3,
            ..AvifOptions::default()
        }),
    );
}

#[test]
fn png_conversion_over_examples() {
    assert_conversion(
        "png",
        EncoderConfig::Png(PngOptions {
            compression_type: None,
            filter_type: None,
        }),
    );
}

#[test]
fn jpeg_conversion_over_examples() {
    assert_conversion("jpeg", EncoderConfig::Jpeg);
}

#[test]
fn no_matches_reports_empty_run() {
    let stats = run(
        ConversionConfig {
            pattern: "definitely/no/matches/*.*".to_string(),
            ..ConversionConfig::default()
        },
        EncoderConfig::Jpeg,
    )
    .expect("run should succeed");
    assert_eq!(stats, RunStats::default());
}

#[test]
fn overwrite_flags_produce_expected_outcomes() {
    // first run encodes, second run with default flags skips everything
    let output = output_dir("overwrite");
    let enc = EncoderConfig::Jpeg;

    let first = run(conversion_config(&output), enc.clone()).expect("first run");
    assert_eq!(first.successful, EXAMPLE_INPUT_COUNT);

    let second = run(conversion_config(&output), enc.clone()).expect("second run");
    assert_eq!(
        second.skipped, EXAMPLE_INPUT_COUNT,
        "existing outputs are skipped"
    );
    assert_eq!(second.successful, 0);

    // overwrite_existing forces re-encoding regardless of size
    let mut conf = conversion_config(&output);
    conf.overwrite_existing = true;
    let third = run(conf, enc).expect("third run");
    assert_eq!(third.successful, EXAMPLE_INPUT_COUNT);

    let _ = fs::remove_dir_all(&output);
}
