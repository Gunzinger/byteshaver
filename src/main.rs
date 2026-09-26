use byteshaver::cli::{CliArgs, Command};
use byteshaver::config::{ConversionConfig, EncoderConfig};
use byteshaver::metadata::policy;
use byteshaver::utils::remove_files;
use byteshaver::{Error, run};
use clap::Parser;

fn main() -> Result<(), Error> {
    // --exif-list-tags must work without the required glob pattern, so it is
    // checked before argument parsing and prints the tag table, then exits.
    if std::env::args().any(|arg| arg == "--exif-list-tags") {
        print!("{}", policy::format_recognized_tags());
        return Ok(());
    }

    let args = CliArgs::parse();
    if matches!(args.command, Command::Clean { .. }) {
        remove_files(&args.pattern)?;
        return Ok(());
    }
    warn_if_exif_feature_missing(&args);
    let conf = ConversionConfig::from_args(&args);
    let mut enc = EncoderConfig::from_args(&args.command)
        .expect("conversion subcommand expected, clean is handled separately");
    #[cfg(all(feature = "opt-oxipng", feature = "exif"))]
    if let byteshaver::config::EncoderConfig::Oxipng(oxipng_options) = &mut enc {
        oxipng_options.exif_policy = conf.exif.clone();
    }
    run(conf, enc)?;
    Ok(())
}

/// Without the `exif` feature, metadata is always stripped; warn when the
/// user explicitly requested to keep some.
#[cfg(not(feature = "exif"))]
fn warn_if_exif_feature_missing(args: &CliArgs) {
    if args.exif.is_some() || args.exif_except.is_some() || args.exif_only.is_some() {
        eprintln!(
            "Warning: this build was compiled without the \"exif\" feature; \
             EXIF metadata is always stripped regardless of --exif flags"
        );
    }
}

/// With the `exif` feature enabled (default), nothing to warn about.
#[cfg(feature = "exif")]
fn warn_if_exif_feature_missing(_args: &CliArgs) {}
