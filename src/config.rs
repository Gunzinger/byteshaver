//! User-facing configuration types: the global [`ConversionConfig`] and the
//! per-target [`EncoderConfig`] with their option structs.
//!
//! Encoder-specific value enums live in the encoder modules and are
//! re-exported here so CLI and library users never depend on converter
//! internals directly.

use crate::cli::Command;
use crate::metadata::policy::{ExifPolicy, parse_tag_list};
use clap::ValueEnum;

pub use crate::converter::avif::{AlphaColorMode, AvifOptions, BitDepth, ColorModel};
#[cfg(feature = "opt-oxipng")]
pub use crate::converter::oxipng::{
    OxipngFilter, OxipngInterlace, OxipngLevel, OxipngOptions, OxipngReduction, OxipngStrip,
};
pub use crate::converter::png::{CompressionType, FilterType, PngOptions};
pub use crate::converter::webp::WebpOptions;

/// How to treat HEIC/HEIF files that contain more than one image.
///
/// Only relevant when the `dec-heif` feature is enabled; without it, HEIF
/// input fails per-file regardless of this setting.
#[derive(Clone, Copy, Debug, Default, ValueEnum, PartialEq, Eq)]
pub enum HeifImagePolicy {
    /// Decode only the primary image of each file (default).
    #[default]
    Primary,
    /// Decode every image of a file; outputs are named
    /// `stem.ext`, `stem_1.ext`, `stem_2.ext`, ...
    All,
}

/// Configuration parameters shared across all encoders.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConversionConfig {
    /// Glob pattern to match images to convert.
    /// Example: `images/**/*.png`
    pub pattern: String,

    /// Output directory (flat) of processed images.
    /// Defaults to the same location as the original images with the new file extension.
    pub output: String,

    /// By default, byteshaver will process input files in lexicographical order after expanding the pattern.
    /// Setting this starts the process from the back.
    /// Defaults to false.
    pub reverse_processing_order: bool,

    /// Overwrite the existing output file if the current conversion resulted in a smaller file.
    /// Defaults to false.
    pub overwrite_if_smaller: bool,

    /// Overwrite existing outputs?
    /// Defaults to false. (Determined by filename match)
    pub overwrite_existing: bool,

    /// Discards the encoding result if it is larger than the input file (does not create an output file).
    /// Defaults to false.
    pub discard_if_larger_than_input: bool,

    /// Discards the alpha channel of the image if it is present.
    /// Defaults to false.
    pub discard_input_alpha_channel: bool,

    /// How EXIF metadata of the source is treated (strip/keep/filter).
    /// Defaults to strip: metadata is removed and orientation transforms
    /// are baked into the pixels.
    pub exif: ExifPolicy,

    /// How HEIC/HEIF files with multiple images are treated
    /// (`dec-heif` feature): only the primary image, or every image with
    /// suffixed output names. Defaults to [`HeifImagePolicy::Primary`].
    pub heif_image_policy: HeifImagePolicy,
}

impl ConversionConfig {
    /// Builds the global configuration from parsed CLI arguments.
    ///
    /// # Panics
    ///
    /// Exits the process (exit code 2) when the EXIF flag combination is
    /// invalid (`--exif-except`/`--exif-only` require `--exif filter` and
    /// are mutually exclusive) or a tag selector cannot be resolved.
    #[must_use]
    pub fn from_args(args: &crate::cli::CliArgs) -> Self {
        ConversionConfig {
            pattern: args.pattern.clone(),
            output: args.output.clone().unwrap_or_default(),
            reverse_processing_order: args.reverse_processing_order.unwrap_or_default(),
            overwrite_if_smaller: args.overwrite_if_smaller.unwrap_or_default(),
            overwrite_existing: args.overwrite_existing.unwrap_or_default(),
            discard_if_larger_than_input: args.discard_if_larger_than_input.unwrap_or_default(),
            discard_input_alpha_channel: args.discard_input_alpha_channel.unwrap_or_default(),
            exif: exif_policy_from_args(args),
            heif_image_policy: heif_image_policy_from_args(args),
        }
    }
}

/// Resolves the HEIF multi-image policy from the CLI flags.
///
/// The flag only exists in builds with the `dec-heif` feature; other builds
/// always use the default (primary image only).
#[cfg(feature = "dec-heif")]
fn heif_image_policy_from_args(args: &crate::cli::CliArgs) -> HeifImagePolicy {
    args.heif_image_policy.unwrap_or_default()
}

/// Feature-less fallback of [`heif_image_policy_from_args`].
#[cfg(not(feature = "dec-heif"))]
fn heif_image_policy_from_args(_args: &crate::cli::CliArgs) -> HeifImagePolicy {
    HeifImagePolicy::default()
}

/// Resolves the EXIF policy from the CLI flags.
///
/// `--exif-except` and `--exif-only` are mutually exclusive (also enforced
/// by clap) and both require `--exif filter`.
fn exif_policy_from_args(args: &crate::cli::CliArgs) -> ExifPolicy {
    use crate::cli::ExifPolicyArg;
    if args.exif_except.is_some() && args.exif_only.is_some() {
        unreachable!("clap enforces mutual exclusion");
    }
    let selector_lists_used = args.exif_except.is_some() || args.exif_only.is_some();
    match args.exif {
        Some(ExifPolicyArg::Filter) => match (&args.exif_except, &args.exif_only) {
            (Some(tags), None) => selectors_or_exit(tags, true),
            (None, Some(tags)) => selectors_or_exit(tags, false),
            _ => exit_with_exif_error(
                "--exif filter requires either --exif-except <TAGS> or --exif-only <TAGS>",
            ),
        },
        Some(ExifPolicyArg::Keep) if !selector_lists_used => ExifPolicy::Keep,
        Some(ExifPolicyArg::Strip) if !selector_lists_used => ExifPolicy::Strip,
        None if !selector_lists_used => ExifPolicy::Strip,
        _ => exit_with_exif_error(
            "--exif-except and --exif-only are mutually exclusive and require --exif filter",
        ),
    }
}

/// Parses the selector list or exits with a helpful message.
fn selectors_or_exit(specs: &[String], except: bool) -> ExifPolicy {
    match parse_tag_list(specs) {
        Ok(selectors) if except => ExifPolicy::FilterExcept(selectors),
        Ok(selectors) => ExifPolicy::KeepOnly(selectors),
        Err(message) => exit_with_exif_error(&message),
    }
}

/// Prints an EXIF configuration error and terminates with exit code 2.
fn exit_with_exif_error(message: &str) -> ! {
    eprintln!("Error: {message}");
    eprintln!("Run with --exif-list-tags to print recognized tag names.");
    std::process::exit(2);
}

/// Selection of the target encoder with its options.
///
/// Constructed from the CLI subcommand (see [`EncoderConfig::from_args`]);
/// cheap to clone and `Send + Sync`.
#[derive(Clone, Debug, PartialEq)]
pub enum EncoderConfig {
    /// webp encoder of the webp crate
    Webp(WebpOptions),
    /// lossless webp encoder of the image crate
    WebpImage,
    /// avif encoder of the ravif crate
    Avif(AvifOptions),
    /// png encoder of the image crate
    Png(PngOptions),
    /// optimized jpeg encoder of the mozjpeg crate
    Jpeg,
    /// png re-optimization / transcoding encoder of the oxipng crate
    /// (requires the `opt-oxipng` feature, on by default)
    #[cfg(feature = "opt-oxipng")]
    Oxipng(OxipngOptions),
}

impl EncoderConfig {
    /// Builds the encoder configuration from a CLI subcommand.
    ///
    /// Returns `None` for commands that do not convert (i.e. `Command::Clean`).
    #[must_use]
    pub fn from_args(command: &Command) -> Option<Self> {
        match command {
            Command::Webp { lossless, quality } => Some(EncoderConfig::Webp(WebpOptions {
                lossless: lossless.unwrap_or_default(),
                quality: quality.unwrap_or(90.),
            })),
            Command::WebpImage {} => Some(EncoderConfig::WebpImage),
            Command::Avif {
                quality,
                speed,
                bit_depth,
                color_model,
                alpha_color_mode,
                alpha_quality,
            } => Some(EncoderConfig::Avif(AvifOptions {
                quality: quality.unwrap_or(90.),
                speed: speed.unwrap_or(3),
                bit_depth: *bit_depth,
                color_model: *color_model,
                alpha_color_mode: *alpha_color_mode,
                alpha_quality: alpha_quality.unwrap_or(90.),
            })),
            Command::Png {
                compression_type,
                filter_type,
            } => Some(EncoderConfig::Png(PngOptions {
                compression_type: *compression_type,
                filter_type: *filter_type,
            })),
            Command::Jpeg {} => Some(EncoderConfig::Jpeg),
            #[cfg(feature = "opt-oxipng")]
            Command::Oxipng {
                level,
                zopfli,
                zopfli_iterations,
                interlace,
                strip,
                filters,
                optimize_alpha,
                no_reduction,
                scale_16,
                fix_errors,
                timeout_secs,
            } => Some(EncoderConfig::Oxipng(OxipngOptions {
                level: level.unwrap_or_default(),
                zopfli: zopfli.unwrap_or_default(),
                zopfli_iterations: zopfli_iterations.unwrap_or(15),
                interlace: interlace.unwrap_or_default(),
                strip: strip.unwrap_or_default(),
                strip_explicit: strip.is_some(),
                #[cfg(feature = "exif")]
                exif_policy: Default::default(),
                filters: filters.clone(),
                optimize_alpha: optimize_alpha.unwrap_or_default(),
                no_reduction: no_reduction.clone(),
                scale_16: scale_16.unwrap_or_default(),
                fix_errors: fix_errors.unwrap_or_default(),
                timeout: timeout_secs.map(std::time::Duration::from_secs),
            })),
            Command::Clean {} => None,
        }
    }
}
