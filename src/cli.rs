use clap::{ArgAction, Parser, Subcommand};

/// Image converter CLI
#[derive(Parser, Debug)]
#[command(
    version,
    about,
    long_about = None
)]
pub struct CliArgs {
    /// The command to execute.
    #[command(subcommand)]
    pub command: Command,

    /// Glob pattern to match images to convert.
    /// Example: `images/**/*.png`
    //#[clap(global = true)]
    // arguments can't be global and required
    // => early exit for no pattern matches
    pub pattern: String,

    /// Output directory (flat) of processed images.
    /// Defaults to the same location as the original images with the new file extension.
    /// If set, replaces the fixed base of the pattern directory structure of the input pattern. (before any * in the glob pattern)
    #[clap(short, long, global = true, default_value = None)]
    pub output: Option<String>,

    /// By default, byteshaver will process input files in lexicographical order after expanding the pattern.
    /// Setting this starts the process from the back.
    #[clap(long, global = true, action = Some(ArgAction::SetTrue))]
    pub reverse_processing_order: Option<bool>,

    /// Overwrite the existing output file if the current conversion resulted in a smaller file.
    #[clap(long, global = true, action = Some(ArgAction::SetTrue))]
    pub overwrite_if_smaller: Option<bool>,

    /// Overwrite existing output files regardless of size.
    #[clap(long, global = true, action = Some(ArgAction::SetTrue))]
    pub overwrite_existing: Option<bool>,

    /// Discards the encoding result if it is larger than the input file (does not create an output file).
    #[clap(long, global = true, action = Some(ArgAction::SetTrue))]
    pub discard_if_larger_than_input: Option<bool>,

    /// Discards the alpha channel of the input image(s) if it is present.
    ///  (this does not make loading faster, but it can improve the encoding result)
    #[clap(long, global = true, action = Some(ArgAction::SetTrue))]
    pub discard_input_alpha_channel: Option<bool>,
}

/// Image converter actions
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Convert images to webp format (using webp crate)
    Webp {
        /// Use lossless encoding mode. Defaults to false.
        #[clap(long, action = Some(ArgAction::SetTrue))]
        lossless: Option<bool>,

        /// Control target quality (0 - 100, lower is worse but results in smaller files).
        /// Defaults to 90.0.
        #[clap(short, long)]
        quality: Option<f32>,
    },

    /// Convert images to webp format (using image crate)
    WebpImage {}, // only lossless is available, no configuration parameters

    /// Convert images to avif format (using ravif crate)
    Avif {
        /// Control target quality (0 - 100, lower is worse but results in smaller files).
        /// Defaults to 90.0.
        #[clap(short, long)]
        quality: Option<f32>,

        /// Control encoding speed (1 - 10, lower is much slower but has a better quality and lower filesize).
        /// Defaults to 3.
        #[clap(short, long)]
        speed: Option<u8>,

        /// Choose internal bit depth. (in the generated avif file, nothing to do with the input file)
        #[clap(long, value_enum)]
        bit_depth: Option<crate::config::BitDepth>,

        /// Choose internal color model. (in the generated avif file, nothing to do with the input file)
        #[clap(long, value_enum)]
        color_model: Option<crate::config::ColorModel>,

        /// Choose internal alpha color mode. (in the generated avif file, nothing to do with the input file)
        /// Irrelevant for images without transparency.
        #[clap(long, value_enum)]
        alpha_color_mode: Option<crate::config::AlphaColorMode>,

        /// Control target alpha quality (0 - 100, lower is worse).
        /// Defaults to 90.0.
        #[clap(short, long)]
        alpha_quality: Option<f32>,
    },

    /// Convert images to png format (using image crate)
    Png {
        /// Choose the png compression type
        ///
        /// See: https://docs.rs/image/latest/image/codecs/png/enum.CompressionType.html
        #[clap(long, value_enum)]
        compression_type: Option<crate::config::CompressionType>,

        /// Choose the png filter type
        ///
        /// See: https://docs.rs/image/latest/image/codecs/png/enum.CompressionType.html
        #[clap(long, value_enum)]
        filter_type: Option<crate::config::FilterType>,
    },

    /// Convert images to optimized jpeg format (using mozjpeg crate)
    Jpeg {},

    /// Remove files matching a glob pattern
    Clean {},
}
