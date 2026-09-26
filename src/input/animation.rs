//! Animation container types.
//!
//! Seeded by WS0 with GIF decode wired in (`input::load_source`); WS5
//! completes WebP/APNG/JXL decode and all animated encoders.

use std::time::Duration;

use image::RgbaImage;
use image::metadata::LoopCount;

/// A single animation frame in the canonical interop format (RGBA8).
#[derive(Clone)]
pub struct FrameData {
    /// Full-canvas RGBA8 pixel buffer of this frame.
    pub buffer: RgbaImage,
    /// Time this frame is displayed.
    pub delay: Duration,
}

/// Decoded animation data of a source image.
#[derive(Clone)]
pub struct AnimationData {
    /// Canvas width of the animation in pixels.
    pub width: u32,
    /// Canvas height of the animation in pixels.
    pub height: u32,
    /// Frames in presentation order.
    pub frames: Vec<FrameData>,
    /// How often the animation should loop.
    pub loop_count: LoopCount,
}

impl AnimationData {
    /// Returns the first frame, if any.
    ///
    /// Still encoders fall back to this until animated encoders land (WS5).
    pub fn first_frame(&self) -> Option<&FrameData> {
        self.frames.first()
    }
}

impl FrameData {
    /// Converts a decoded `image` crate frame into the canonical format.
    pub fn from_image_frame(frame: image::Frame) -> Self {
        let (numer, denom) = frame.delay().numer_denom_ms();
        let denom = u64::from(denom).max(1);
        FrameData {
            buffer: frame.into_buffer(),
            delay: Duration::from_nanos(u64::from(numer) * 1_000_000 / denom),
        }
    }
}
