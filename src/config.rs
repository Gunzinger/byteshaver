//! User-facing configuration types: the global [`ConversionConfig`] and the
//! per-target [`EncoderConfig`] with their option structs.
//!
//! Encoder-specific value enums live in the encoder modules and are
//! re-exported here so CLI and library users never depend on converter
//! internals directly.

use crate::cli::Command;

pub use crate::converter::avif::{AlphaColorMode, AvifOptions, BitDepth, ColorModel};
pub use crate::converter::png::{CompressionType, FilterType, PngOptions};
pub use crate::converter::webp::WebpOptions;

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
}

impl ConversionConfig {
    /// Builds the global configuration from parsed CLI arguments.
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
        }
    }
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
            Command::Clean {} => None,
        }
    }
}
