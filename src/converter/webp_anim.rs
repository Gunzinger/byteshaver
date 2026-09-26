//! Animated webp conversion via the webp-animation crate (libwebp anim encoder).
//!
//! WS5: animated input is re-encoded frame by frame; timestamps are the
//! cumulative frame delays in milliseconds (i32; longer animations error).
//! The loop count of the source is carried over through
//! `EncoderOptions::anim_params.loop_count` (0 = infinite).
//! EXIF metadata is muxed into the finished container via the RIFF muxer.

use crate::Error;
use crate::converter::DEPENDENCIES;
use crate::format::ImageFormat;
use crate::input::AnimationData;
use image::DynamicImage;
use image::metadata::LoopCount;
use webp_animation::{
    AnimParams, Encoder, EncoderOptions, EncodingConfig, EncodingType, LossyEncodingConfig,
};

/// Options of the webp-animation animated webp encoder.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WebpAnimOptions {
    /// Target quality (0 - 100, lower is worse but results in smaller files).
    /// In lossless mode this is the compression effort. Defaults to 90.0.
    pub quality: f32,
    /// Use lossless encoding mode. Defaults to false.
    pub lossless: bool,
    /// Minimum distance between keyframes (0 = auto/libwebp default).
    pub kmin: Option<i32>,
    /// Maximum distance between keyframes (0 = disables keyframe insertion).
    pub kmax: Option<i32>,
    /// Minimize the output size (slow; disables keyframe insertion).
    pub minimize_size: bool,
    /// Allow mixed lossy/lossless frames (libwebp picks per frame).
    pub allow_mixed: bool,
    /// Quality/speed trade-off (0 = fast, 6 = slower-better). Defaults to 4.
    pub method: Option<u8>,
}

impl Default for WebpAnimOptions {
    fn default() -> Self {
        WebpAnimOptions {
            quality: 90.,
            lossless: false,
            kmin: None,
            kmax: None,
            minimize_size: false,
            allow_mixed: false,
            method: None,
        }
    }
}

/// Encoder for animated webp format using the webp-animation crate.
pub struct WebpAnimEncoder {
    /// Encoding options.
    pub options: WebpAnimOptions,
}

impl WebpAnimEncoder {
    /// Creates an encoder with the given options.
    pub fn new(options: WebpAnimOptions) -> Self {
        WebpAnimEncoder { options }
    }
}

impl super::ImageEncoder for WebpAnimEncoder {
    fn format(&self) -> ImageFormat {
        ImageFormat::WebpAnim
    }

    fn extension(&self) -> &'static str {
        "webp"
    }

    fn describe(&self) -> String {
        encoder_info(&self.options)
    }

    fn supports_animation(&self) -> bool {
        true
    }

    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error> {
        // still input to an animated target: a valid 1-frame animation
        // with a default display time of 100 ms (deliberate, documented)
        let animation = AnimationData {
            width: image.width(),
            height: image.height(),
            frames: vec![crate::input::FrameData {
                buffer: image.to_rgba8(),
                delay: std::time::Duration::from_millis(100),
            }],
            loop_count: LoopCount::Infinite,
        };
        self.encode_animation(&animation, None)
    }

    fn encode_still_image_with_metadata(
        &self,
        image: &DynamicImage,
        metadata: &crate::metadata::ImageMetadata,
    ) -> Result<Vec<u8>, Error> {
        let animation = AnimationData {
            width: image.width(),
            height: image.height(),
            frames: vec![crate::input::FrameData {
                buffer: image.to_rgba8(),
                delay: std::time::Duration::from_millis(100),
            }],
            loop_count: LoopCount::Infinite,
        };
        self.encode_animation(&animation, metadata.exif.as_deref())
    }

    fn encode(&self, input: &crate::input::SourceImage) -> Result<Vec<u8>, Error> {
        match &input.content {
            crate::input::ImageContent::Still(image) => {
                self.encode_still_image_with_metadata(image, &input.metadata)
            }
            crate::input::ImageContent::Animated(animation) => {
                self.encode_animation(animation, input.metadata.exif.as_deref())
            }
        }
    }
}

impl WebpAnimEncoder {
    /// Encodes an [`AnimationData`] into animated webp bytes, optionally
    /// muxing an EXIF payload into the finished container.
    pub fn encode_animation(
        &self,
        animation: &AnimationData,
        exif_payload: Option<&[u8]>,
    ) -> Result<Vec<u8>, Error> {
        let loop_count = match animation.loop_count {
            LoopCount::Infinite => 0,
            LoopCount::Finite(n) => i32::try_from(n.get()).unwrap_or(i32::MAX),
        };

        let encoding_config = EncodingConfig {
            encoding_type: if self.options.lossless {
                EncodingType::Lossless
            } else {
                EncodingType::Lossy(LossyEncodingConfig::default())
            },
            quality: self.options.quality,
            method: usize::from(self.options.method.unwrap_or(4)),
        };

        let mut options = EncoderOptions {
            anim_params: AnimParams { loop_count },
            minimize_size: self.options.minimize_size,
            kmin: self.options.kmin.map(|v| v as isize).unwrap_or_default(),
            kmax: self.options.kmax.map(|v| v as isize).unwrap_or_default(),
            allow_mixed: self.options.allow_mixed,
            ..Default::default()
        };
        options.encoding_config = Some(encoding_config);

        let dimensions = (animation.width, animation.height);
        if dimensions.0 == 0 || dimensions.1 == 0 {
            return Err(Error::from_string(
                "Animated webp encoding requires non-zero dimensions".to_string(),
            ));
        }
        let mut encoder = Encoder::new_with_options(dimensions, options).map_err(|e| {
            Error::from_string(format!("Failed to create animated webp encoder: {e:?}"))
        })?;

        // libwebp semantics: a frame is displayed AT its timestamp, so the
        // timestamp of frame i is the sum of the delays of frames 0..i and
        // the total sum gives the last frame its display duration. A 0 ms
        // delay is bumped by 1 ms (timestamps must be strictly increasing).
        let mut timestamp = 0i64;
        let mut previous = -1i64;
        for frame in &animation.frames {
            let frame_ts = timestamp.max(previous + 1);
            if frame_ts > i32::MAX as i64 {
                return Err(Error::from_string(
                    "Animation is too long for animated webp (timestamp overflow)".to_string(),
                ));
            }
            encoder
                .add_frame(frame.buffer.as_raw(), frame_ts as i32)
                .map_err(|e| {
                    Error::from_string(format!("Animated webp frame encoding failed: {e:?}"))
                })?;
            previous = frame_ts;
            timestamp += frame.delay.as_millis() as i64;
        }
        let finalize_ts = timestamp.min(i32::MAX as i64) as i32;
        let webp_data = encoder
            .finalize(finalize_ts)
            .map_err(|e| Error::from_string(format!("Animated webp finalization failed: {e:?}")))?;

        super::webp::embed_exif_into_webp(webp_data.to_vec(), exif_payload)
    }
}

/// Provides encoder information
fn encoder_info(options: &WebpAnimOptions) -> String {
    // we might have multiple versions of the package, use rfind to find the newest one
    let mut version = "";
    match DEPENDENCIES
        .iter()
        .rfind(|&&(name, _)| name == "webp-animation")
    {
        Some((_name, found)) => version = found,
        None => println!("Package 'webp-animation' not found"),
    };

    format!(
        "Using \"webp-animation\" ({}) with options (lossless: {}, qualify: {}, kmin: {:?}, kmax: {:?}, minimize_size: {}, allow_mixed: {}, method: {})",
        version,
        options.lossless,
        options.quality,
        options.kmin,
        options.kmax,
        options.minimize_size,
        options.allow_mixed,
        options.method.unwrap_or(4)
    )
}
