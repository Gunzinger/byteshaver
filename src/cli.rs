use clap::{ArgAction, Parser, Subcommand, ValueEnum};

/// EXIF metadata policy values of the `--exif` flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ExifPolicyArg {
    /// Copy EXIF verbatim into outputs that support it.
    Keep,
    /// Remove all EXIF from outputs (default).
    Strip,
    /// Keep all but `--exif-except <TAGS>`, or only `--exif-only <TAGS>`.
    Filter,
}

/// Behavior when animated input meets a target that cannot encode animations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum AnimatedInputArg {
    /// Encode the first frame only, printing a notice (default).
    #[default]
    FirstFrame,
    /// Fail the file with an error instead of silently dropping animation.
    Error,
}

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

    /// EXIF metadata handling policy (global).
    /// strip: remove all EXIF and bake the orientation into the pixels (default, privacy-safe).
    /// keep: copy EXIF verbatim into outputs where the container supports it.
    /// filter: keep all tags except --exif-except <TAGS>, or only --exif-only <TAGS>.
    #[clap(long, value_enum, value_name = "keep|strip|filter", global = true)]
    pub exif: Option<ExifPolicyArg>,

    /// Comma-separated EXIF tags/IFDs to drop when using --exif filter
    /// (e.g. --exif-except gps,GPSInfo,Orientation; names from --exif-list-tags,
    /// IFD wildcards ifd0/ifd1/exif/gps/interop, or numeric tags like 0x8825).
    /// Mutually exclusive with --exif-only; requires --exif filter.
    #[clap(
        long,
        value_delimiter = ',',
        value_name = "TAGS",
        global = true,
        requires = "exif"
    )]
    pub exif_except: Option<Vec<String>>,

    /// Comma-separated EXIF tags/IFDs to keep when using --exif filter
    /// (same syntax as --exif-except).
    /// Mutually exclusive with --exif-except; requires --exif filter.
    #[clap(
        long,
        value_delimiter = ',',
        value_name = "TAGS",
        global = true,
        conflicts_with = "exif_except",
        requires = "exif"
    )]
    pub exif_only: Option<Vec<String>>,

    /// Print recognized EXIF tag names and exit.
    #[clap(long, action = Some(ArgAction::SetTrue))]
    pub exif_list_tags: Option<bool>,

    /// How to treat HEIC/HEIF files containing more than one image
    /// (requires a build with the `dec-heif` feature):
    /// primary: decode only the primary image (default).
    /// all: decode every image; outputs are named stem.ext, stem_1.ext, ...
    #[cfg(all(feature = "dec-heif", any(not(target_os = "windows"), target_abi = "llvm")))]
    #[clap(long, value_enum, value_name = "primary|all", global = true)]
    pub heif_image_policy: Option<crate::config::HeifImagePolicy>,

    /// How to treat animated input (gif / animated webp / APNG) when the
    /// selected target cannot encode animations.
    /// first-frame: encode the first frame only, with a notice (default).
    /// error: fail the file with an error instead of silently dropping
    /// the animation (for pipelines that must never drop frames).
    #[clap(long, value_enum, value_name = "first-frame|error", global = true)]
    pub animated_input: Option<AnimatedInputArg>,

    /// Hard cap for decoded animation memory in MiB (frames are RGBA:
    /// width x height x 4 x frames). Files whose projected frame buffers
    /// exceed the cap fail with an error instead of risking an OOM.
    /// Defaults to 4096.
    #[clap(long, value_name = "MIB", global = true)]
    pub max_animation_memory: Option<u64>,

    /// Write one JSON object per progress event to this file (JSON lines)
    /// in addition to the regular stdout output (requires the `logs`
    /// feature, on by default).
    #[cfg(feature = "logs")]
    #[clap(long, global = true, value_name = "PATH")]
    pub json_log: Option<std::path::PathBuf>,
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

    /// Convert images to jpeg-xl format (using libjxl)
    #[cfg(feature = "jxl")]
    Jxl {
        /// JPEG-style quality 0-100 (higher = better). Mutually exclusive with --distance.
        #[clap(short, long, group = "jxl-quality-group")]
        quality: Option<f32>,

        /// Maximum Butteraugli distance 0.0-25.0 (0.0 = mathematically lossless,
        /// 1.0 = visually lossless, libjxl default 1.0). Mutually exclusive with --quality.
        #[clap(long, group = "jxl-quality-group")]
        distance: Option<f32>,

        /// Lossless mode. Overrides quality/distance.
        #[clap(long, action = Some(ArgAction::SetTrue))]
        lossless: Option<bool>,

        /// Encoding effort 1 (fastest) - 10 (slowest/best). Defaults to 7.
        #[clap(short, long, value_parser = clap::value_parser!(u8).range(1..=10))]
        effort: Option<u8>,

        /// Force the box-based container format (required for manual Exif/XMP embedding;
        /// auto-enabled when EXIF is embedded).
        #[clap(long, action = Some(ArgAction::SetTrue))]
        container: Option<bool>,

        /// Keep the original color profile (do not convert to internal XYB); needed for lossless.
        #[clap(long, action = Some(ArgAction::SetTrue))]
        original_profile: Option<bool>,

        /// Target decode speed tier 0-4 (higher = faster decode, larger file). Defaults to 0.
        #[clap(long, value_parser = clap::value_parser!(u8).range(0..=4))]
        decoding_speed: Option<u8>,

        /// Photometric target intensity in nits (HDR). Defaults to libjxl's 255.
        #[clap(long)]
        intensity_target: Option<f32>,

        /// Force output bit depth: 8 or 16 (default: follow the input).
        #[clap(long, value_parser = clap::value_parser!(u8).range(8..=16))]
        bit_depth: Option<u8>,

        /// Color encoding: srgb | linear-srgb | srgb-luma | linear-srgb-luma | icc-passthrough.
        /// Defaults to srgb.
        #[clap(long, value_enum)]
        color_encoding: Option<crate::config::JxlColorEncodingChoice>,

        /// Advanced: repeatable libjxl frame-setting passthrough, e.g. --setting brotli_effort=9
        /// (ids are resolved case-insensitively; unknown ids list the available set).
        #[clap(long = "setting", value_name = "ID=VALUE")]
        settings: Vec<String>,
    },

    /// Convert images to optimally compressed png format (using oxipng)
    #[cfg(feature = "opt-oxipng")]
    Oxipng {
        /// Optimization level 0-6 (preset), or `max`. Default 2. Higher = slower.
        ///
        /// Explicit flags are applied on top of the chosen preset
        /// (preset first, explicit overrides second).
        #[clap(short, long, value_enum)]
        level: Option<crate::config::OxipngLevel>,

        /// Use zopfli DEFLATE instead of libdeflate (much slower, slightly smaller).
        #[clap(long, action = Some(ArgAction::SetTrue))]
        zopfli: Option<bool>,

        /// zopfli iteration count when --zopfli is set. Defaults to 15.
        #[clap(long)]
        zopfli_iterations: Option<u32>,

        /// Interlacing handling: keep | none | adam7.
        /// Defaults to none (oxipng default removes interlacing).
        #[clap(long, value_enum)]
        interlace: Option<crate::config::OxipngInterlace>,

        /// Metadata stripping: none | safe | all. Defaults to none.
        #[clap(long, value_enum)]
        strip: Option<crate::config::OxipngStrip>,

        /// Filter strategies to try (repeatable, comma-separated):
        /// none, sub, up, avg, paeth, minsum, entropy, bigrams, bigent, brute.
        /// Replaces the filter set of the chosen level preset.
        #[clap(long, value_enum, value_delimiter = ',')]
        filters: Vec<crate::config::OxipngFilter>,

        /// Allow altering transparent pixel values for better compression (lossless visually).
        #[clap(long, action = Some(ArgAction::SetTrue))]
        optimize_alpha: Option<bool>,

        /// Disable lossless reductions (repeatable, comma-separated):
        /// bit-depth, color-type, palette, grayscale.
        #[clap(long, value_enum, value_delimiter = ',')]
        no_reduction: Vec<crate::config::OxipngReduction>,

        /// Force 16-bit to 8-bit scaling when losslessly possible.
        #[clap(long, action = Some(ArgAction::SetTrue))]
        scale_16: Option<bool>,

        /// Attempt fixing broken input pngs instead of erroring.
        #[clap(long, action = Some(ArgAction::SetTrue))]
        fix_errors: Option<bool>,

        /// Stop optimizing a file after this many seconds. Default: unlimited.
        #[clap(long, value_name = "SECS")]
        timeout_secs: Option<u64>,
    },

    /// Convert images to animated webp format (using webp-animation crate).
    /// Supports animated input (gif, animated webp, APNG); the source loop
    /// count is preserved. Still input becomes a 1-frame animation.
    #[cfg(feature = "anim-webp")]
    WebpAnim {
        /// Use lossless encoding mode. Defaults to false.
        #[clap(long, action = Some(ArgAction::SetTrue))]
        lossless: Option<bool>,

        /// Control target quality (0 - 100, lower is worse but results in smaller files).
        /// In lossless mode this is the compression effort. Defaults to 90.0.
        #[clap(short, long)]
        quality: Option<f32>,

        /// Minimum distance between keyframes (0 = libwebp default).
        #[clap(long)]
        kmin: Option<i32>,

        /// Maximum distance between keyframes (0 = disables keyframe insertion).
        #[clap(long)]
        kmax: Option<i32>,

        /// Minimize the output size (much slower; disables keyframe insertion).
        #[clap(long, action = Some(ArgAction::SetTrue))]
        minimize_size: Option<bool>,

        /// Allow mixed lossy/lossless frames (libwebp picks per frame).
        #[clap(long, action = Some(ArgAction::SetTrue))]
        allow_mixed: Option<bool>,

        /// Quality/speed trade-off (0 = fast, 6 = slower-better). Defaults to 4.
        #[clap(long)]
        method: Option<u8>,
    },

    /// Convert images to animated png format (APNG, using png crate).
    /// Supports animated input (gif, animated webp, APNG); the source loop
    /// count is preserved. Still input becomes a 1-frame animation.
    /// The default image stays readable by non-APNG-aware viewers.
    #[cfg(feature = "anim-apng")]
    Apng {
        /// Choose the png compression type (Fast pairs well with a
        /// following oxipng post-optimization pass)
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

    /// Convert images to (animated) gif format (using image crate).
    /// Supports animated input; the source loop count is preserved.
    /// Note: frame delays are rounded to the GIF-standard 10 ms granularity.
    Gif {
        /// Quantization speed (1 = best quality, 30 = fastest). Defaults to 10.
        #[clap(long)]
        speed: Option<i32>,
    },

    /// Remove files matching a glob pattern
    Clean {},
}
