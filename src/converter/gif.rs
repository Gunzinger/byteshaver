//! Animated GIF conversion via the image crate's `GifEncoder` (WS5 bonus).
//!
//! Frame delays are rounded down to the GIF-standard 10 ms granularity and
//! clamped to at least 10 ms (a zero delay means "as fast as possible" and
//! is rarely intended). Loop count maps onto the encoder's repeat behavior.
//! GIF cannot carry EXIF (`supports_metadata` = false). Still inputs produce
//! a valid 1-frame GIF with a 100 ms default display time (documented).

use crate::Error;
use crate::converter::DEPENDENCIES;
use crate::format::ImageFormat;
use crate::input::AnimationData;
use image::Delay;
use image::DynamicImage;
use image::Frame as AnimFrame;
use image::codecs::gif::{GifEncoder as ImageCrateGifEncoder, Repeat};
use image::metadata::LoopCount;
use std::io::Cursor;
use std::time::Duration;

/// Default display time for still inputs to the animated target (ms).
const STILL_FRAME_DELAY_MS: u32 = 100;

/// Options of the animated GIF encoder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GifOptions {
    /// Quantization speed of the palette encoder (1 = best quality, 30 =
    /// fastest; see `gif::Frame::from_rgba_speed`). Defaults to 10.
    pub speed: Option<i32>,
}

/// Encoder for animated GIF format using the image crate.
pub struct GifEncoder {
    /// Encoding options.
    pub options: GifOptions,
}

impl GifEncoder {
    /// Creates an encoder with the given options.
    pub fn new(options: GifOptions) -> Self {
        GifEncoder { options }
    }
}

impl super::ImageEncoder for GifEncoder {
    fn format(&self) -> ImageFormat {
        ImageFormat::Gif
    }

    fn extension(&self) -> &'static str {
        "gif"
    }

    fn describe(&self) -> String {
        encoder_info()
    }

    fn supports_animation(&self) -> bool {
        true
    }

    fn supports_metadata(&self) -> bool {
        false
    }

    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error> {
        encode_gif(&still_animation(image), self.options)
    }

    fn encode(&self, input: &crate::input::SourceImage) -> Result<Vec<u8>, Error> {
        match &input.content {
            crate::input::ImageContent::Still(image) => self.encode_still_image(image),
            crate::input::ImageContent::Animated(animation) => encode_gif(animation, self.options),
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

/// Maps the loop count onto the GIF repeat behavior (0 = infinite).
fn repeat_from_loop_count(loop_count: LoopCount) -> Repeat {
    match loop_count {
        LoopCount::Infinite => Repeat::Infinite,
        LoopCount::Finite(n) => Repeat::Finite(n.get().min(u16::MAX as u32 + 1) as u16),
    }
}

/// Converts a frame delay into GIF centiseconds (rounded down, minimum 1).
fn delay_centiseconds(delay: Duration) -> u16 {
    let cs = delay.as_millis() / 10;
    cs.clamp(1, u16::MAX as u128) as u16
}

/// Encodes an [`AnimationData`] into animated GIF bytes (RGBA frames; the
/// encoder palette-quantizes internally).
fn encode_gif(animation: &AnimationData, options: GifOptions) -> Result<Vec<u8>, Error> {
    if animation.width == 0 || animation.height == 0 {
        return Err(Error::from_string(
            "GIF encoding requires non-zero dimensions".to_string(),
        ));
    }

    let mut output = Vec::new();
    let mut encoder =
        ImageCrateGifEncoder::new_with_speed(Cursor::new(&mut output), options.speed.unwrap_or(10));
    encoder
        .set_repeat(repeat_from_loop_count(animation.loop_count))
        .map_err(|e| Error::from_string(format!("gif encoding failed: {e:?}")))?;

    for frame in &animation.frames {
        let cs = delay_centiseconds(frame.delay);
        let anim_frame = AnimFrame::from_parts(
            frame.buffer.clone(),
            0,
            0,
            Delay::from_numer_denom_ms(u32::from(cs) * 10, 1),
        );
        encoder
            .encode_frame(anim_frame)
            .map_err(|e| Error::from_string(format!("gif encoding failed: {e:?}")))?;
    }

    drop(encoder); // flush the underlying gif writer
    Ok(output)
}

/// Provides encoder information
fn encoder_info() -> String {
    // we might have multiple versions of the package, use rfind to find the newest one
    let mut image_version = "";
    match DEPENDENCIES.iter().rfind(|&&(name, _)| name == "image") {
        Some((_name, version)) => image_version = version,
        None => println!("Package 'image' not found"),
    };

    format!("Using \"image\" ({image_version}) GIF encoder (animated)")
}
