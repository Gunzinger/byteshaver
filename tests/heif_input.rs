//! HEIC/HEIF/AVIF input tests (WS1).
//!
//! **Locally verifiable tests only:** the feature-off stub behavior and the
//! config plumbing run everywhere, because `dec-heif` is not part of the
//! default features. Tests that need the flag or actual HEIF decoding are
//! gated on the `dec-heif` feature and require the native libheif + codec
//! libraries (libde265/libaom) — they run in the gnu/docker CI jobs only.

#[cfg(not(feature = "dec-heif"))]
use std::fs;
#[cfg(not(feature = "dec-heif"))]
use std::path::{Path, PathBuf};

#[cfg(not(feature = "dec-heif"))]
use byteshaver::config::EncoderConfig;
use byteshaver::config::{ConversionConfig, HeifImagePolicy};
#[cfg(not(feature = "dec-heif"))]
use byteshaver::input;
#[cfg(not(feature = "dec-heif"))]
use byteshaver::{RunStats, run};

#[cfg(not(feature = "dec-heif"))]
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "byteshaver-heif-test-{}-{}",
        name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[cfg(not(feature = "dec-heif"))]
fn write_png(path: &Path) {
    let rgba = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
    image::save_buffer(path, rgba.as_raw(), 4, 4, image::ExtendedColorType::Rgba8)
        .expect("write png");
}

/// Without the `dec-heif` feature, `.heic` files report a clear per-file
/// error instead of aborting the batch, and other files still convert.
#[test]
#[cfg(not(feature = "dec-heif"))]
fn heif_stub_reports_per_file_error_and_batch_continues() {
    let dir = temp_dir("stub");
    let heic = dir.join("garbage.heic");
    fs::write(&heic, b"this is not a heif container at all").expect("write garbage heic");
    let avif = dir.join("still.avif");
    fs::write(&avif, b"not an avif either").expect("write garbage avif");
    let png = dir.join("valid.png");
    write_png(&png);

    // direct loader stub message
    let err = match input::load_source(&heic) {
        Err(err) => err,
        Ok(_) => panic!("heif must fail without dec-heif"),
    };
    assert_eq!(
        err.to_string(),
        "HEIC/HEIF input requires a build with the dec-heif feature"
    );
    let err = match input::load_source(&avif) {
        Err(err) => err,
        Ok(_) => panic!("avif input must fail without dec-heif"),
    };
    assert_eq!(
        err.to_string(),
        "HEIC/HEIF input requires a build with the dec-heif feature"
    );

    // batch behavior: the two heif-family files are counted as errors, the
    // png still converts and the run succeeds overall (like the CLI's exit
    // code 0 despite per-file errors)
    let output = dir.join("out");
    let pattern = format!("{}/**/*.*", dir.display());
    let stats = run(
        ConversionConfig {
            pattern,
            output: output.display().to_string(),
            ..ConversionConfig::default()
        },
        EncoderConfig::Jpeg,
    )
    .expect("run should succeed despite per-file errors");

    assert_eq!(stats.input_files, 3);
    assert_eq!(stats.errors, 2, "garbage.heic + garbage.avif fail");
    assert_eq!(stats.successful, 1, "valid.png still converts");
    assert_eq!(stats.skipped, 0);
    assert!(stats.output_size > 0);
    assert!(output.join("valid.jpeg").is_file());

    let _ = fs::remove_dir_all(&dir);
}

/// The policy type is part of the build-independent config surface.
#[test]
fn heif_image_policy_defaults_to_primary() {
    assert_eq!(HeifImagePolicy::default(), HeifImagePolicy::Primary);
    assert_eq!(
        ConversionConfig::default().heif_image_policy,
        HeifImagePolicy::Primary
    );
    let conf = ConversionConfig {
        heif_image_policy: HeifImagePolicy::All,
        ..ConversionConfig::default()
    };
    assert_eq!(conf.heif_image_policy, HeifImagePolicy::All);
}

/// The value enum exposes exactly the documented flag values.
#[test]
fn heif_image_policy_flag_values() {
    use clap::ValueEnum;
    let names: Vec<String> = HeifImagePolicy::value_variants()
        .iter()
        .filter_map(|policy| {
            policy
                .to_possible_value()
                .map(|pv| pv.get_name().to_string())
        })
        .collect();
    assert_eq!(names, ["primary", "all"]);
}

/// Flag parsing (only in builds that have the flag, i.e. `dec-heif` on).
#[test]
#[cfg(feature = "dec-heif")]
fn heif_image_policy_flag_parses() {
    use byteshaver::cli::CliArgs;
    use clap::Parser;

    let args = CliArgs::parse_from([
        "byteshaver",
        "input/**/*.*",
        "webp",
        "--heif-image-policy",
        "all",
    ]);
    assert_eq!(
        args.heif_image_policy,
        Some(HeifImagePolicy::All),
        "global flags work after the subcommand"
    );
    let conf = ConversionConfig::from_args(&args);
    assert_eq!(conf.heif_image_policy, HeifImagePolicy::All);

    let args = CliArgs::parse_from([
        "byteshaver",
        "--heif-image-policy",
        "primary",
        "input/**/*.*",
        "png",
    ]);
    assert_eq!(
        ConversionConfig::from_args(&args).heif_image_policy,
        HeifImagePolicy::Primary
    );

    // default when the flag is absent
    let args = CliArgs::parse_from(["byteshaver", "input/**/*.*", "jpeg"]);
    assert_eq!(args.heif_image_policy, None);
    assert_eq!(
        ConversionConfig::from_args(&args).heif_image_policy,
        HeifImagePolicy::Primary
    );
}

/// Stats of a heif-stub run keep their invariants (helper reference test for
/// CI builds with the feature: the garbage file still fails there, but with
/// a decoder error message instead of the stub).
#[test]
#[cfg(not(feature = "dec-heif"))]
fn heif_stub_run_stats_are_sane() {
    let dir = temp_dir("stats");
    fs::write(dir.join("a.heic"), b"garbage").expect("write heic");
    let stats: RunStats = run(
        ConversionConfig {
            pattern: format!("{}/*.heic", dir.display()),
            ..ConversionConfig::default()
        },
        EncoderConfig::Jpeg,
    )
    .expect("run succeeds");
    assert_eq!(
        (stats.input_files, stats.errors, stats.successful),
        (1, 1, 0)
    );

    let _ = fs::remove_dir_all(&dir);
}
