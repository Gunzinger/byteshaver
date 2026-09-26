//! Animated PNG (APNG) conversion via the png crate (WS5).
//!
//! The default image is kept separate from the animation frames
//! (`set_sep_def_img(true)`) so non-APNG-aware viewers still see the first
//! frame. Frame delays are written with a 1/1000 s denominator (ms exact);
//! delays of a minute or more are clamped (u16 numerator). Loop count maps
//! onto `num_plays` (0 = infinite). Still inputs produce a valid 1-frame
//! APNG with a 100 ms default display time (deliberate, documented).

use crate::Error;
use crate::converter::DEPENDENCIES;
use crate::converter::png::{
    CompressionType, FilterType, convert_compression_type_to_ext, convert_filter_type_to_ext,
};
use crate::format::ImageFormat;
use crate::input::AnimationData;
use image::DynamicImage;
use image::metadata::LoopCount;
use std::borrow::Cow;
use std::io::Write;
use std::time::Duration;

/// Default display time for still inputs to the animated target (ms).
const STILL_FRAME_DELAY_MS: u32 = 100;

/// Options of the APNG (animated png) encoder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ApngOptions {
    /// PNG compression type. Defaults to the encoder default (Fast is a good
    /// pairing with a following oxipng post-optimization pass).
    pub compression_type: Option<CompressionType>,
    /// PNG filter type. Defaults to the encoder default.
    pub filter_type: Option<FilterType>,
}

/// Encoder for animated png format using the png crate.
pub struct ApngEncoder {
    /// Encoding options.
    pub options: ApngOptions,
}

impl ApngEncoder {
    /// Creates an encoder with the given options.
    pub fn new(options: ApngOptions) -> Self {
        ApngEncoder { options }
    }
}

impl super::ImageEncoder for ApngEncoder {
    fn format(&self) -> ImageFormat {
        ImageFormat::Apng
    }

    fn extension(&self) -> &'static str {
        "png"
    }

    fn describe(&self) -> String {
        encoder_info()
    }

    fn supports_animation(&self) -> bool {
        true
    }

    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error> {
        encode_apng(&still_animation(image), self.options, None)
    }

    fn encode_still_image_with_metadata(
        &self,
        image: &DynamicImage,
        metadata: &crate::metadata::ImageMetadata,
    ) -> Result<Vec<u8>, Error> {
        encode_apng(
            &still_animation(image),
            self.options,
            metadata.exif.as_deref(),
        )
    }

    fn encode(&self, input: &crate::input::SourceImage) -> Result<Vec<u8>, Error> {
        match &input.content {
            crate::input::ImageContent::Still(image) => {
                self.encode_still_image_with_metadata(image, &input.metadata)
            }
            crate::input::ImageContent::Animated(animation) => {
                encode_apng(animation, self.options, input.metadata.exif.as_deref())
            }
        }
    }
}

/// Wraps a still image into a 1-frame animation (default 100 ms display time).
fn still_animation(image: &DynamicImage) -> AnimationData {
    AnimationData {
        width: image.width(),
        height: image.height(),
        frames: vec![crate::input::FrameData {
            buffer: image.to_rgba8(),
            delay: Duration::from_millis(u64::from(STILL_FRAME_DELAY_MS)),
        }],
        loop_count: LoopCount::Infinite,
    }
}

impl ApngEncoder {
    /// Encodes an [`AnimationData`] into APNG bytes, optionally embedding an
    /// EXIF payload as the `eXIf` chunk of the first (default) image header.
    pub fn encode_animation(
        &self,
        animation: &AnimationData,
        exif_payload: Option<&[u8]>,
    ) -> Result<Vec<u8>, Error> {
        encode_apng(animation, self.options, exif_payload)
    }
}

/// Encodes an [`AnimationData`] to APNG bytes (RGBA8 frames).
fn encode_apng(
    animation: &AnimationData,
    options: ApngOptions,
    exif_payload: Option<&[u8]>,
) -> Result<Vec<u8>, Error> {
    if animation.width == 0 || animation.height == 0 {
        return Err(Error::from_string(
            "APNG encoding requires non-zero dimensions".to_string(),
        ));
    }

    let num_plays = match animation.loop_count {
        LoopCount::Infinite => 0,
        LoopCount::Finite(n) => n.get(),
    };

    let mut output = Vec::new();
    let mut info = png::Info::with_size(animation.width, animation.height);
    // EXIF rides on the header of the first (default) image
    if let Some(exif) = exif_payload {
        info.exif_metadata = Some(Cow::Borrowed(exif));
    }

    let mut encoder = png::Encoder::with_info(&mut output, info)
        .map_err(|e| Error::from_string(format!("apng encoding failed: {e:?}")))?;
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(convert_compression_type_to_ext(options.compression_type));
    encoder.set_filter(convert_filter_type_to_ext(options.filter_type));
    encoder
        .set_animated(animation.frames.len() as u32, num_plays)
        .map_err(|e| Error::from_string(format!("apng encoding failed: {e:?}")))?;
    encoder
        .set_sep_def_img(true)
        .map_err(|e| Error::from_string(format!("apng encoding failed: {e:?}")))?;

    let mut writer = encoder
        .write_header()
        .map_err(|e| Error::from_string(format!("apng encoding failed: {e:?}")))?;

    // png-crate APNG model with `set_sep_def_img(true)`: the FIRST
    // `write_image_data` call becomes the IDAT default image (no fcTL, seen
    // by non-APNG viewers), every following call becomes an animation frame
    // (fcTL + fdAT). The default image mirrors the first animation frame.
    let first = animation
        .frames
        .first()
        .ok_or_else(|| Error::from_string("APNG requires at least one frame".to_string()))?;
    writer
        .write_image_data(first.buffer.as_raw())
        .map_err(|e| Error::from_string(format!("apng encoding failed: {e:?}")))?;

    for frame in &animation.frames {
        let (delay_num, delay_den) = frame_delay_fraction(frame.delay);
        writer
            .set_frame_delay(delay_num, delay_den)
            .map_err(|e| Error::from_string(format!("apng encoding failed: {e:?}")))?;
        writer
            .write_image_data(frame.buffer.as_raw())
            .map_err(|e| Error::from_string(format!("apng encoding failed: {e:?}")))?;
    }

    writer
        .finish()
        .map_err(|e| Error::from_string(format!("apng encoding failed: {e:?}")))?;
    let _ = output.flush();
    Ok(output)
}

/// Converts a frame delay into an APNG delay fraction (numerator/1000 s).
///
/// Delays of 65535 ms or more are clamped: the numerator is a u16 with a
/// 1/1000 s denominator.
fn frame_delay_fraction(delay: Duration) -> (u16, u16) {
    let ms = delay.as_millis().min(u16::MAX as u128) as u16;
    (ms, 1000)
}

/// Provides encoder information
fn encoder_info() -> String {
    // we might have multiple versions of the package, use rfind to find the newest one
    let mut png_version = "";
    match DEPENDENCIES.iter().rfind(|&&(name, _)| name == "png") {
        Some((_name, version)) => png_version = version,
        None => println!("Package 'png' not found"),
    };

    format!("Using \"png\" ({png_version}) in animated (APNG) mode")
}
