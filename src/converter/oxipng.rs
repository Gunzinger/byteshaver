//! Optimal PNG compression via the [`oxipng`] crate (plan WS3).
//!
//! oxipng operates on encoded PNG bytes, not pixels, which enables two
//! distinct modes:
//!
//! - **Passthrough (PNG→PNG):** the original input file bytes are fed
//!   straight into [`oxipng::optimize_from_memory`]. Pixels stay bit-exact;
//!   palette (PLTE), tRNS and 16-bit data are preserved and reduced
//!   losslessly, and APNG animation chunks (acTL/fcTL/fdAT) survive.
//! - **Transcode (non-PNG→PNG):** pixels are decoded through the normal
//!   pipeline, baseline-encoded with the existing png encoder module at low
//!   effort ([`CompressionType::Fast`] + [`FilterType::Adaptive`]), and then
//!   optimized. oxipng re-encodes the IDAT stream anyway, so spending effort
//!   on the baseline encode would be wasted.
//!
//! Threading: oxipng is compiled without its `parallel` feature on purpose;
//! it runs single-threaded per file and never taps into the crate-wide
//! [`ThreadBudget`](super::ThreadBudget) (byteshaver parallelizes across
//! files instead, avoiding pool oversubscription).
//!
//! Note: the EXIF policy interplay (`strip` adjustment and payload embedding
//! in transcode mode) is active when the `exif` feature is enabled.

use std::time::Duration;

use clap::ValueEnum;
use image::DynamicImage;

use crate::Error;
use crate::converter::DEPENDENCIES;
use crate::converter::png::{CompressionType, FilterType, encode_png};
use crate::format::ImageFormat;
use crate::input::{ImageContent, SourceImage};
#[cfg(feature = "exif")]
use crate::metadata::policy::ExifPolicy;

/// Optimization level preset, mapped onto `oxipng::Options::from_preset`.
///
/// Higher levels try more filter strategies and stronger DEFLATE settings
/// (much slower, potentially smaller). `Max` maps to
/// `oxipng::Options::max_compression()`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize,
)]
pub enum OxipngLevel {
    /// Preset 0: fastest, experimental (basic filters only).
    Zero,
    /// Preset 1: fast.
    One,
    /// Preset 2: oxipng's own sane default (also the byteshaver default).
    #[default]
    Two,
    /// Preset 3: slower, more filters.
    Three,
    /// Preset 4: slower still, stronger DEFLATE.
    Four,
    /// Preset 5: exhaustive (slow) filter evaluation.
    Five,
    /// Preset 6: extremely slow, best ratio of the numeric presets.
    Six,
    /// `max_compression()`: currently identical to preset 6.
    Max,
}

impl OxipngLevel {
    /// Numeric preset of this level, `None` for [`OxipngLevel::Max`].
    #[must_use]
    pub fn preset(self) -> Option<u8> {
        match self {
            OxipngLevel::Zero => Some(0),
            OxipngLevel::One => Some(1),
            OxipngLevel::Two => Some(2),
            OxipngLevel::Three => Some(3),
            OxipngLevel::Four => Some(4),
            OxipngLevel::Five => Some(5),
            OxipngLevel::Six => Some(6),
            OxipngLevel::Max => None,
        }
    }
}

/// Interlacing handling of the optimized output.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize,
)]
pub enum OxipngInterlace {
    /// Leave the interlace state of the input unchanged.
    Keep,
    /// Remove interlacing (default; matches the oxipng default).
    #[default]
    None,
    /// Force Adam7 interlacing on the output.
    Adam7,
}

/// Metadata chunk stripping policy.
///
/// Standalone for now; a later merge couples this to the EXIF policy
/// (see plan WS4).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize,
)]
pub enum OxipngStrip {
    /// Keep all ancillary chunks (default).
    #[default]
    None,
    /// Remove all chunks that do not affect image display.
    Safe,
    /// Remove all non-critical chunks.
    All,
}

/// Filter strategies oxipng may try per image.
///
/// Explicitly configured filters replace the filter set of the chosen
/// [`OxipngLevel`] preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum OxipngFilter {
    /// Same (no) filter for every row.
    None,
    /// Sub filter for every row.
    Sub,
    /// Up filter for every row.
    Up,
    /// Average filter for every row.
    Avg,
    /// Paeth filter for every row.
    Paeth,
    /// Minimum sum of absolute differences heuristic.
    MinSum,
    /// Shannon entropy heuristic.
    Entropy,
    /// Distinct-bigram-count heuristic.
    Bigrams,
    /// Bigram Shannon entropy heuristic.
    BigEnt,
    /// Brute-force DEFLATE evaluation of candidate rows.
    Brute,
}

/// Lossless reductions that can be disabled individually.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum OxipngReduction {
    /// Bit-depth reduction (only ever performed when losslessly possible).
    BitDepth,
    /// Color-type reduction (e.g. RGBA → RGB).
    ColorType,
    /// Palette reduction.
    Palette,
    /// Grayscale reduction.
    Grayscale,
}

/// Options of the oxipng encoder with CLI defaults already resolved.
///
/// Explicitly configured flags are applied on top of the chosen
/// [`OxipngLevel`] preset (preset first, explicit overrides second).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OxipngOptions {
    /// Optimization level preset. Default [`OxipngLevel::Two`].
    pub level: OxipngLevel,
    /// Use zopfli DEFLATE instead of libdeflate (much slower, slightly
    /// smaller). Default false.
    pub zopfli: bool,
    /// zopfli iteration count when [`OxipngOptions::zopfli`] is set.
    /// Default 15.
    pub zopfli_iterations: u32,
    /// Interlacing handling. Default [`OxipngInterlace::None`].
    pub interlace: OxipngInterlace,
    /// Metadata chunk stripping. Default [`OxipngStrip::None`].
    pub strip: OxipngStrip,
    /// Filter strategies to try; empty keeps the preset's filter set.
    pub filters: Vec<OxipngFilter>,
    /// Allow altering transparent pixel values for better compression
    /// (lossless visually). Default false.
    pub optimize_alpha: bool,
    /// Lossless reductions to disable. Default none disabled.
    pub no_reduction: Vec<OxipngReduction>,
    /// Force 16-bit → 8-bit scaling (potentially pixel-altering).
    /// Default false.
    pub scale_16: bool,
    /// Attempt fixing broken input PNGs instead of erroring. Default false.
    pub fix_errors: bool,
    /// Per-file optimization time limit. Default unlimited.
    pub timeout: Option<Duration>,
    /// Whether the user explicitly passed `--strip` (drives the EXIF policy
    /// interplay: under the default `Strip` policy an unset `--strip` is
    /// bumped to `safe` so eXIf chunks do not silently survive).
    pub strip_explicit: bool,
    /// Active EXIF policy of the run; injected from the global config
    /// (default [`ExifPolicy::Strip`]). Only available with the `exif`
    /// feature.
    #[cfg(feature = "exif")]
    pub exif_policy: ExifPolicy,
}

impl Default for OxipngOptions {
    fn default() -> Self {
        OxipngOptions {
            level: OxipngLevel::Two,
            zopfli: false,
            zopfli_iterations: 15,
            interlace: OxipngInterlace::None,
            strip: OxipngStrip::None,
            filters: Vec::new(),
            optimize_alpha: false,
            no_reduction: Vec::new(),
            scale_16: false,
            fix_errors: false,
            timeout: None,
            strip_explicit: false,
            #[cfg(feature = "exif")]
            exif_policy: ExifPolicy::Strip,
        }
    }
}

impl OxipngOptions {
    /// Builds the native [`oxipng::Options`] from these resolved options:
    /// the level preset is applied first, explicit flags override on top.
    #[must_use]
    pub fn to_oxipng(&self) -> oxipng::Options {
        let mut opts = match self.level.preset() {
            Some(preset) => oxipng::Options::from_preset(preset),
            None => oxipng::Options::max_compression(),
        };
        if !self.filters.is_empty() {
            opts.filters = self.filters.iter().map(|f| f.to_strategy()).collect();
        }
        opts.interlace = match self.interlace {
            OxipngInterlace::Keep => None,
            OxipngInterlace::None => Some(false),
            OxipngInterlace::Adam7 => Some(true),
        };
        opts.strip = match self.strip {
            OxipngStrip::None => oxipng::StripChunks::None,
            OxipngStrip::Safe => oxipng::StripChunks::Safe,
            OxipngStrip::All => oxipng::StripChunks::All,
        };
        // EXIF policy interplay (plan WS3 §4 / WS4 §4): when metadata must
        // survive, stripping is forced off; under the default strip policy an
        // unset `--strip` is bumped to `safe` so behavior follows the policy.
        #[cfg(feature = "exif")]
        match self.exif_policy {
            ExifPolicy::Strip => {
                if !self.strip_explicit {
                    opts.strip = oxipng::StripChunks::Safe;
                }
            }
            ExifPolicy::Keep | ExifPolicy::FilterExcept(_) | ExifPolicy::KeepOnly(_) => {
                opts.strip = oxipng::StripChunks::None;
            }
        }
        opts.optimize_alpha = self.optimize_alpha;
        for reduction in &self.no_reduction {
            match reduction {
                OxipngReduction::BitDepth => opts.bit_depth_reduction = false,
                OxipngReduction::ColorType => opts.color_type_reduction = false,
                OxipngReduction::Palette => opts.palette_reduction = false,
                OxipngReduction::Grayscale => opts.grayscale_reduction = false,
            }
        }
        opts.scale_16 = self.scale_16;
        opts.fix_errors = self.fix_errors;
        opts.timeout = self.timeout;
        if self.zopfli {
            let mut zopfli = oxipng::ZopfliOptions::default();
            // 0 is not a meaningful iteration count (NonZeroU64); keep default
            if let Ok(iterations) = u64::from(self.zopfli_iterations).try_into() {
                zopfli.iteration_count = iterations;
            }
            opts.deflater = oxipng::Deflater::Zopfli(zopfli);
        }
        opts
    }
}

impl OxipngFilter {
    /// CLI name of this filter strategy.
    fn name(self) -> &'static str {
        match self {
            OxipngFilter::None => "none",
            OxipngFilter::Sub => "sub",
            OxipngFilter::Up => "up",
            OxipngFilter::Avg => "avg",
            OxipngFilter::Paeth => "paeth",
            OxipngFilter::MinSum => "minsum",
            OxipngFilter::Entropy => "entropy",
            OxipngFilter::Bigrams => "bigrams",
            OxipngFilter::BigEnt => "bigent",
            OxipngFilter::Brute => "brute",
        }
    }

    /// Maps this filter onto the native oxipng filter strategy.
    ///
    /// The heuristics-only strategies (`MinSum`, `Entropy`, `Bigrams`,
    /// `BigEnt`) map 1:1; `Brute` uses oxipng's strongest preset brute
    /// parameters (8 lines, level 5).
    #[must_use]
    pub fn to_strategy(self) -> oxipng::FilterStrategy {
        match self {
            OxipngFilter::None => oxipng::FilterStrategy::NONE,
            OxipngFilter::Sub => oxipng::FilterStrategy::SUB,
            OxipngFilter::Up => oxipng::FilterStrategy::UP,
            OxipngFilter::Avg => oxipng::FilterStrategy::AVERAGE,
            OxipngFilter::Paeth => oxipng::FilterStrategy::PAETH,
            OxipngFilter::MinSum => oxipng::FilterStrategy::MinSum,
            OxipngFilter::Entropy => oxipng::FilterStrategy::Entropy,
            OxipngFilter::Bigrams => oxipng::FilterStrategy::Bigrams,
            OxipngFilter::BigEnt => oxipng::FilterStrategy::BigEnt,
            OxipngFilter::Brute => oxipng::FilterStrategy::Brute {
                num_lines: 8,
                level: 5,
            },
        }
    }
}

impl std::fmt::Display for OxipngOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = self
            .level
            .preset()
            .map_or_else(|| "max".to_string(), |p| p.to_string());
        let mut reductions = vec!["bit-depth", "color-type", "palette", "grayscale"];
        for reduction in &self.no_reduction {
            let name = match reduction {
                OxipngReduction::BitDepth => "bit-depth",
                OxipngReduction::ColorType => "color-type",
                OxipngReduction::Palette => "palette",
                OxipngReduction::Grayscale => "grayscale",
            };
            reductions.retain(|r| *r != name);
        }
        write!(
            f,
            "level={} zopfli={} zopfli-iterations={} interlace={:?} strip={:?} \
             optimize-alpha={} reductions=[{}] scale-16={} fix-errors={} timeout={:?}",
            level,
            self.zopfli,
            self.zopfli_iterations,
            self.interlace,
            self.strip,
            self.optimize_alpha,
            reductions.join(","),
            self.scale_16,
            self.fix_errors,
            self.timeout,
        )?;
        if !self.filters.is_empty() {
            let filters = self
                .filters
                .iter()
                .map(|f| f.name())
                .collect::<Vec<_>>()
                .join(",");
            write!(f, " filters=[{filters}]")?;
        }
        Ok(())
    }
}

/// Encoder for optimal png format using the oxipng crate.
///
/// See the module docs for the passthrough vs. transcode mode distinction.
pub struct OxipngEncoder {
    /// Optimization options.
    pub options: OxipngOptions,
}

impl OxipngEncoder {
    /// Creates an encoder with the given options.
    pub fn new(options: OxipngOptions) -> Self {
        OxipngEncoder { options }
    }

    /// Transcode mode: baseline-encode at low effort, then optimize.
    /// oxipng re-encodes the IDAT stream regardless (idat_recoding is on
    /// by default), so spending compression effort here would be wasted.
    ///
    /// When the EXIF policy resolves a payload for the source, it is embedded
    /// into the baseline PNG (`eXIf` chunk) before optimization; the strip
    /// interplay in [`OxipngOptions::to_oxipng`] guarantees the chunk
    /// survives when the policy wants it kept.
    fn transcode(
        &self,
        image: &DynamicImage,
        exif_payload: Option<&[u8]>,
    ) -> Result<Vec<u8>, Error> {
        let baseline = encode_png(
            image,
            Some(CompressionType::Fast),
            Some(FilterType::Adaptive),
            exif_payload,
        )?;
        optimize_bytes(&baseline, &self.options)
    }
}

impl super::ImageEncoder for OxipngEncoder {
    fn format(&self) -> ImageFormat {
        ImageFormat::Png
    }

    fn extension(&self) -> &'static str {
        "png"
    }

    fn describe(&self) -> String {
        // we might have multiple versions of the package, use rfind to find the newest one
        let oxipng_version = DEPENDENCIES
            .iter()
            .rfind(|&&(name, _)| name == "oxipng")
            .map_or("unknown", |(_, version)| *version);
        format!(
            "Using \"oxipng\" ({oxipng_version}) with options: {}",
            self.options
        )
    }

    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error> {
        self.transcode(image, None)
    }

    fn encode(&self, input: &SourceImage) -> Result<Vec<u8>, Error> {
        // Passthrough mode: PNG input (stills and animated PNGs alike — the
        // input layer decodes APNG as a still first frame, and oxipng
        // preserves APNG chunk structure during optimization). Feeding the
        // original bytes avoids a decode/re-encode round trip and keeps the
        // result bit-exact on the pixel level. Pixel-level global flags like
        // --discard-input-alpha-channel do not apply in this mode (the
        // operation would not be a lossless passthrough anymore).
        if input.source_format == ImageFormat::Png {
            let original = std::fs::read(&input.source_path)?;
            return optimize_bytes(&original, &self.options);
        }
        match &input.content {
            ImageContent::Still(image) => {
                #[cfg(feature = "exif")]
                let payload =
                    crate::metadata::exif::resolve(&self.options.exif_policy, &input.metadata);
                #[cfg(not(feature = "exif"))]
                let payload = None;
                self.transcode(image, payload.as_deref())
            }
            ImageContent::Animated(animation) => {
                let first = animation.first_frame().ok_or_else(|| {
                    Error::from_string("Animation does not contain any frames".to_string())
                })?;
                println!(
                    "Warning: {} does not support animated input yet; encoding the first frame only",
                    self.describe()
                );
                self.encode_still_image(&DynamicImage::ImageRgba8(first.buffer.clone()))
            }
        }
    }

    fn huge_image_hint(&self) -> Option<&'static str> {
        Some(
            "the oxipng target can take very long on huge images; \
             consider a lower --level (<= 2) or --timeout-secs",
        )
    }
}

/// Optimizes already-encoded PNG bytes with the configured oxipng options.
///
/// This is the single entry point both encoder modes funnel through.
///
/// # Errors
///
/// Returns an [`Error`] if oxipng cannot decode or optimize the input bytes.
pub fn optimize_bytes(png_bytes: &[u8], options: &OxipngOptions) -> Result<Vec<u8>, Error> {
    oxipng::optimize_from_memory(png_bytes, &options.to_oxipng())
        .map_err(|e| Error::from_string(format!("oxipng optimization failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::converter::ImageEncoder as _;

    #[test]
    fn default_options_map_to_level_two_preset() {
        let opts = OxipngOptions::default().to_oxipng();
        // preset 2 == library default
        let expected = oxipng::Options::default();
        assert_eq!(opts.filters, expected.filters);
        assert_eq!(opts.deflater, expected.deflater);
        assert_eq!(opts.interlace, Some(false));
        // EXIF policy interplay: the default Strip policy with no explicit
        // `--strip` bumps stripping to `safe` (plan WS3 §4 / WS4 §4)
        #[cfg(feature = "exif")]
        assert_eq!(opts.strip, oxipng::StripChunks::Safe);
        #[cfg(not(feature = "exif"))]
        assert_eq!(opts.strip, oxipng::StripChunks::None);
        assert!(!opts.optimize_alpha);
        assert!(opts.bit_depth_reduction);
        assert!(opts.color_type_reduction);
        assert!(opts.palette_reduction);
        assert!(opts.grayscale_reduction);
        assert!(!opts.scale_16);
        assert!(!opts.fix_errors);
        assert_eq!(opts.timeout, None);
    }

    #[test]
    fn max_level_maps_to_max_compression() {
        let opts = OxipngOptions {
            level: OxipngLevel::Max,
            ..OxipngOptions::default()
        }
        .to_oxipng();
        let expected = oxipng::Options::max_compression();
        assert_eq!(opts.filters, expected.filters);
        assert_eq!(opts.deflater, expected.deflater);
        assert!(!opts.fast_evaluation);
    }

    #[test]
    fn explicit_flags_override_preset() {
        let opts = OxipngOptions {
            level: OxipngLevel::Max,
            zopfli: true,
            zopfli_iterations: 8,
            interlace: OxipngInterlace::Adam7,
            strip: OxipngStrip::Safe,
            filters: vec![OxipngFilter::None, OxipngFilter::Paeth],
            optimize_alpha: true,
            no_reduction: vec![
                OxipngReduction::BitDepth,
                OxipngReduction::ColorType,
                OxipngReduction::Palette,
                OxipngReduction::Grayscale,
            ],
            scale_16: true,
            fix_errors: true,
            timeout: Some(Duration::from_secs(30)),
            strip_explicit: true,
            #[cfg(feature = "exif")]
            exif_policy: ExifPolicy::Strip,
        }
        .to_oxipng();
        assert_eq!(opts.filters.len(), 2);
        assert!(
            opts.filters
                .contains(&oxipng::FilterStrategy::Basic(oxipng::RowFilter::Paeth))
        );
        assert_eq!(opts.interlace, Some(true));
        assert_eq!(opts.strip, oxipng::StripChunks::Safe);
        assert!(opts.optimize_alpha);
        assert!(!opts.bit_depth_reduction);
        assert!(!opts.color_type_reduction);
        assert!(!opts.palette_reduction);
        assert!(!opts.grayscale_reduction);
        assert!(opts.scale_16);
        assert!(opts.fix_errors);
        assert_eq!(opts.timeout, Some(Duration::from_secs(30)));
        match opts.deflater {
            oxipng::Deflater::Zopfli(zopfli) => {
                assert_eq!(zopfli.iteration_count.get(), 8);
            }
            other => panic!("expected zopfli deflater, got {other:?}"),
        }
    }

    #[test]
    fn interlace_keep_maps_to_none() {
        let opts = OxipngOptions {
            interlace: OxipngInterlace::Keep,
            ..OxipngOptions::default()
        }
        .to_oxipng();
        assert_eq!(opts.interlace, None);
    }

    #[test]
    fn zopfli_iteration_zero_falls_back_to_default() {
        let opts = OxipngOptions {
            zopfli: true,
            zopfli_iterations: 0,
            ..OxipngOptions::default()
        }
        .to_oxipng();
        match opts.deflater {
            oxipng::Deflater::Zopfli(zopfli) => {
                assert_eq!(zopfli.iteration_count.get(), 15);
            }
            other => panic!("expected zopfli deflater, got {other:?}"),
        }
    }

    #[test]
    fn describe_mentions_version_and_options() {
        let encoder = OxipngEncoder::new(OxipngOptions::default());
        let description = encoder.describe();
        assert!(description.contains("oxipng"), "{description}");
        assert!(description.contains("10.2"), "{description}");
        assert!(description.contains("level=2"), "{description}");
    }
}
