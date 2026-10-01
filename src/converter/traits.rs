//! Encoder abstraction: the [`ImageEncoder`] trait, the [`EncoderConfig`]
//! selector and the [`EncoderRegistry`] that materializes encoders.

use image::DynamicImage;

use crate::Error;
use crate::config::EncoderConfig;
#[cfg(feature = "anim-apng")]
use crate::converter::apng::ApngEncoder;
use crate::converter::avif::AvifEncoder;
use crate::converter::gif::GifEncoder;
#[cfg(feature = "jxl")]
use crate::converter::jxl::JxlEncoder;
use crate::converter::mozjpeg::MozjpegEncoder;
#[cfg(feature = "opt-oxipng")]
use crate::converter::oxipng::OxipngEncoder;
use crate::converter::png::PngEncoder;
use crate::converter::webp::WebpEncoder;
#[cfg(feature = "anim-webp")]
use crate::converter::webp_anim::WebpAnimEncoder;
use crate::converter::webp_image::WebpImageEncoder;
use crate::format::ImageFormat;
use crate::input::SourceImage;

/// Global threading budget shared between file processing and encoders.
///
/// Computed once from `std::thread::available_parallelism()`; rayon
/// parallelizes across files with the same pool that encoders may tap into.
#[derive(Clone, Copy, Debug)]
pub struct ThreadBudget {
    /// Number of files that may be encoded in flight (rayon pool size).
    pub files: usize,
    /// Explicit thread cap per encoder; `None` lets an encoder use the
    /// shared global pool (preserves current behavior).
    pub threads_per_encoder: Option<usize>,
}

impl ThreadBudget {
    /// Budget computed from the available parallelism of the machine.
    ///
    /// Files are processed in flight by rayon while encoders share the same
    /// global pool; a strict `available / files_in_flight` split per encoder
    /// arrives with WS2/WS3 (it changes encoder output today).
    #[must_use]
    pub fn global() -> Self {
        let available = std::thread::available_parallelism().map_or(1, |n| n.get());
        ThreadBudget {
            files: available,
            threads_per_encoder: None,
        }
    }

    /// Budget that caps each in-flight encoder to its share of the pool.
    #[must_use]
    pub fn per_files(total_files: usize) -> Self {
        let mut budget = ThreadBudget::global();
        let in_flight = total_files.min(budget.files).max(1);
        budget.threads_per_encoder = Some((budget.files / in_flight).max(1));
        budget
    }
}

/// Common interface implemented by all target encoders.
pub trait ImageEncoder: Send + Sync {
    /// Target image format produced by this encoder.
    fn format(&self) -> ImageFormat;

    /// File extension (without dot) of the encoder output.
    fn extension(&self) -> &'static str;

    /// Human-readable description of the encoder and its active options.
    fn describe(&self) -> String;

    /// Encodes a still image into the encoder's target format.
    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error>;

    /// Encodes a still image, embedding the metadata the target format
    /// supports (WS4: EXIF via `metadata.exif`, already resolved by the
    /// pipeline policy hook).
    ///
    /// The default ignores the metadata and behaves like
    /// [`ImageEncoder::encode_still_image`]; embedding encoders override it.
    fn encode_still_image_with_metadata(
        &self,
        image: &DynamicImage,
        metadata: &crate::metadata::ImageMetadata,
    ) -> Result<Vec<u8>, Error> {
        let _ = metadata;
        self.encode_still_image(image)
    }

    /// Encodes a source image.
    ///
    /// The default handles still content and, until animated encoders land
    /// (WS5), falls back to encoding the first frame of animated input with
    /// a one-line warning.
    fn encode(&self, input: &SourceImage) -> Result<Vec<u8>, Error> {
        match &input.content {
            crate::input::ImageContent::Still(image) => {
                self.encode_still_image_with_metadata(image, &input.metadata)
            }
            crate::input::ImageContent::Animated(animation) => {
                let first = animation.first_frame().ok_or_else(|| {
                    Error::from_string("Animation does not contain any frames".to_string())
                })?;
                println!(
                    "Warning: {} cannot encode animated input; encoding the first frame only (use --animated-input error to reject such files)",
                    self.describe()
                );
                self.encode_still_image_with_metadata(
                    &DynamicImage::ImageRgba8(first.buffer.clone()),
                    &input.metadata,
                )
            }
        }
    }

    /// Whether this encoder can encode animated input natively.
    fn supports_animation(&self) -> bool {
        false
    }

    /// Extra hint printed alongside the huge-image (8192 px) warning.
    ///
    /// Encoders with slow worst-case settings can suggest the user options
    /// that bound the runtime (e.g. `--level`, `--timeout-secs`).
    fn huge_image_hint(&self) -> Option<&'static str> {
        None
    }

    /// Whether this encoder can embed EXIF metadata into its output (WS4).
    ///
    /// Encoders that cannot (e.g. the gif target) report `false`; the
    /// pipeline then drops the payload with a per-file warning and counts it
    /// in [`RunStats::metadata_dropped`][crate::pipeline::RunStats].
    fn supports_metadata(&self) -> bool {
        true
    }

    /// Applies a threading budget to the encoder before encoding starts.
    fn adjust_threading(&mut self, _budget: ThreadBudget) {}
}

/// Registry that materializes concrete encoders from [`EncoderConfig`] values.
///
/// Adding a new encoder means: one module, one `EncoderConfig` variant, one
/// CLI subcommand, one match arm here.
pub struct EncoderRegistry;

impl EncoderRegistry {
    /// Builds the encoder described by `config` and applies `budget` to it.
    #[must_use]
    pub fn build(config: &EncoderConfig, budget: ThreadBudget) -> Box<dyn ImageEncoder> {
        let mut encoder: Box<dyn ImageEncoder> = match config {
            EncoderConfig::Webp(options) => Box::new(WebpEncoder::new(*options)),
            EncoderConfig::WebpImage => Box::new(WebpImageEncoder::new()),
            EncoderConfig::Avif(options) => Box::new(AvifEncoder::new(*options)),
            EncoderConfig::Png(options) => Box::new(PngEncoder::new(*options)),
            EncoderConfig::Jpeg => Box::new(MozjpegEncoder::new()),
            #[cfg(feature = "jxl")]
            EncoderConfig::Jxl(options) => Box::new(JxlEncoder::new(options.clone())),
            #[cfg(feature = "opt-oxipng")]
            EncoderConfig::Oxipng(options) => Box::new(OxipngEncoder::new(options.clone())),
            #[cfg(feature = "anim-webp")]
            EncoderConfig::WebpAnim(options) => Box::new(WebpAnimEncoder::new(*options)),
            #[cfg(feature = "anim-apng")]
            EncoderConfig::Apng(options) => Box::new(ApngEncoder::new(*options)),
            EncoderConfig::Gif(options) => Box::new(GifEncoder::new(*options)),
        };
        encoder.adjust_threading(budget);
        encoder
    }
}
