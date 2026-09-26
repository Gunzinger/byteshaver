//! Decode layer: turns file paths into [`SourceImage`] values.
//!
//! The format is detected exactly once from the file header (no double
//! decode, no panic-based fallbacks). Still images stay `DynamicImage`;
//! animated GIF inputs are decoded into [`AnimationData`].

pub mod animation;
#[cfg(feature = "dec-heif")]
mod heif;
/// JPEG XL source decoding via jxl-oxide (WS2).
#[cfg(feature = "jxl")]
pub mod jxl;

use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use image::metadata::Orientation;
use image::{
    AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat as ImageCrateFormat, ImageReader,
    Limits, codecs::gif::GifDecoder, codecs::png::PngDecoder, codecs::webp::WebPDecoder,
};

use crate::Error;
use crate::format::ImageFormat;
use crate::metadata::ImageMetadata;

pub use animation::{AnimationData, FrameData};

/// A decoded input image with its metadata and detected source format.
pub struct SourceImage {
    /// Pixel content of the image.
    pub content: ImageContent,
    /// Metadata collected from the source container (ignored by WS0 encoders).
    pub metadata: ImageMetadata,
    /// Format detected from the file header (or extension as fallback).
    pub source_format: ImageFormat,
    /// Path of the file this image was loaded from.
    ///
    /// Byte-passthrough encoders (e.g. the oxipng target) re-read the
    /// original file bytes from here instead of re-encoding pixels.
    pub source_path: PathBuf,
}

/// Pixel content of a source image.
pub enum ImageContent {
    /// A single still image; the canonical path for 8-bit sources.
    Still(DynamicImage),
    /// A decoded animation (GIF in WS0).
    Animated(AnimationData),
}

impl SourceImage {
    /// Removes the alpha channel of the source image if it is present.
    pub fn into_without_alpha(mut self) -> Self {
        match self.content {
            ImageContent::Still(image) => {
                self.content = ImageContent::Still(image.into_without_alpha());
            }
            ImageContent::Animated(mut animation) => {
                // flatten alpha onto opaque RGB values, keeping RGBA8 buffers
                for frame in &mut animation.frames {
                    let buffer = std::mem::take(&mut frame.buffer);
                    let rgb = DynamicImage::ImageRgba8(buffer).into_rgb8();
                    frame.buffer = DynamicImage::ImageRgb8(rgb).to_rgba8();
                }
                self.content = ImageContent::Animated(animation);
            }
        }
        self
    }

    /// Returns the still image of the content, taking the first frame for animations.
    pub fn first_frame_still(&self) -> Option<DynamicImage> {
        match &self.content {
            ImageContent::Still(image) => Some(image.clone()),
            ImageContent::Animated(animation) => animation
                .first_frame()
                .map(|frame| DynamicImage::ImageRgba8(frame.buffer.clone())),
        }
    }
}

/// Loads and decodes the image at `path` into a [`SourceImage`]
/// (the primary image for multi-image HEIC/HEIF containers).
pub fn load_source(path: &Path) -> Result<SourceImage, Error> {
    load_source_with_index(path, None)
}

/// Like [`load_source`], but `Some(index)` decodes the `index`-th image of a
/// multi-image HEIC/HEIF container (`--heif-image-policy all`).
///
/// The index is ignored for formats that cannot contain multiple images.
pub fn load_source_with_index(
    path: &Path,
    image_index: Option<usize>,
) -> Result<SourceImage, Error> {
    // JPEG XL is not known to the image crate; route it to the dedicated
    // jxl-oxide decoder before the generic format detection kicks in.
    if ImageFormat::from(path) == ImageFormat::Jxl {
        #[cfg(feature = "jxl")]
        return jxl::load_source_jxl(path);
        #[cfg(not(feature = "jxl"))]
        return Err(Error::from_string(format!(
            "JPEG XL support is not compiled in (feature \"jxl\" disabled): {}",
            path.display()
        )));
    }

    // HEIC/HEIF/HIF/AVIF containers are decoded by libheif, not the image
    // crate; without the `dec-heif` feature they report a per-file error and
    // the batch continues
    if ImageFormat::from(path) == ImageFormat::Heif {
        return load_heif_source(path, image_index);
    }

    let reader = open_reader(path)?;
    // detect the format once from the file header; keep the reader pinned to it
    let reader = match reader.with_guessed_format() {
        Ok(reader) => reader,
        Err(_) => {
            // content sniffing failed: fall back to the file extension
            let ext_format = ImageFormat::from(path);
            let crate_format = ext_format.to_image_format().ok_or_else(|| {
                Error::from_string(format!("Unsupported image format: {}", path.display()))
            })?;
            let mut fallback = open_reader(path)?;
            fallback.set_format(crate_format);
            fallback
        }
    };
    let source_format = reader
        .format()
        .map(ImageFormat::from_image_format)
        .unwrap_or_else(|| ImageFormat::from(path));

    if source_format == ImageFormat::Gif {
        return load_source_animated(path, source_format);
    }
    // WS5: WebP and PNG are only animated when the container says so; probe
    // cheaply on a dedicated reader and fall through to the still path
    // otherwise (single-frame results collapse to `Still` as well)
    if matches!(source_format, ImageFormat::Webp | ImageFormat::Png)
        && let Some(source) = load_source_animated_if_container_is(path, source_format)?
    {
        return Ok(source);
    }

    let mut decoder = reader.into_decoder()?;
    let mut metadata = collect_metadata(&mut decoder)?;
    // WS4: formats whose decoder does not surface the raw payload get an
    // extraction fallback (WebP RIFF scan, TIFF container scan)
    if metadata.exif.is_none() {
        metadata.exif = crate::metadata::extract_exif_fallback(path, &source_format);
    }
    let image = match DynamicImage::from_decoder(decoder) {
        Ok(image) => image,
        Err(err) => fallback_retry_read_image(path, Error::new(err))?,
    };

    Ok(SourceImage {
        content: ImageContent::Still(image),
        metadata,
        source_format,
        source_path: path.to_path_buf(),
    })
}

/// Decodes a HEIC/HEIF/AVIF container via libheif (feature `dec-heif`).
#[cfg(feature = "dec-heif")]
fn load_heif_source(path: &Path, image_index: Option<usize>) -> Result<SourceImage, Error> {
    heif::load_source(path, image_index)
}

/// Counts the decodable master images of a HEIC/HEIF container without
/// decoding pixels (feature `dec-heif`); used by the pipeline's
/// `--heif-image-policy all` expansion.
#[cfg(feature = "dec-heif")]
pub(crate) fn heif_probe(path: &Path) -> Result<usize, Error> {
    heif::probe(path)
}

/// Feature-off stub: HEIC/HEIF/AVIF input needs the native libheif, so these
/// files fail per-file (surfacing as `Outcome::Error` in the pipeline) while
/// the rest of the batch keeps converting.
#[cfg(not(feature = "dec-heif"))]
fn load_heif_source(_path: &Path, _image_index: Option<usize>) -> Result<SourceImage, Error> {
    // io::Error::other keeps the displayed message clean (no error-type prefix)
    Err(Error::new(std::io::Error::other(
        "HEIC/HEIF input requires a build with the dec-heif feature",
    )))
}

/// Decodes an animated GIF into [`AnimationData`] (WS5: also WebP/APNG via
/// container probing; single-frame results collapse to the still path).
fn load_source_animated(path: &Path, source_format: ImageFormat) -> Result<SourceImage, Error> {
    let file = fs::File::open(path)?;
    let decoder = GifDecoder::new(BufReader::new(file))?;
    let loop_count = decoder.loop_count();
    let (width, height) = decoder.dimensions();

    let mut frames = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.map_err(Error::new)?;
        frames.push(FrameData::from_image_frame(frame));
    }
    if frames.is_empty() {
        return Err(Error::from_string(format!(
            "Animated image {} does not contain any frames",
            path.display()
        )));
    }
    if frames.len() == 1 {
        // single-frame gif: resolve to the fast still path
        let frame = frames.into_iter().next().expect("checked length above");
        return Ok(SourceImage {
            content: ImageContent::Still(DynamicImage::ImageRgba8(frame.buffer)),
            metadata: ImageMetadata::default(),
            source_format,
            source_path: path.to_path_buf(),
        });
    }

    Ok(SourceImage {
        content: ImageContent::Animated(AnimationData {
            width,
            height,
            frames,
            loop_count,
        }),
        metadata: ImageMetadata::default(),
        source_format,
        source_path: path.to_path_buf(),
    })
}

/// Probes the container of a potentially animated WebP/PNG file and, when it
/// declares an animation, decodes it into [`AnimationData`].
///
/// Returns `Ok(None)` for still containers (the caller falls back to the
/// regular still decode path). Single-frame animations are collapsed into
/// [`ImageContent::Still`] so the still encoder paths stay fast.
fn load_source_animated_if_container_is(
    path: &Path,
    source_format: ImageFormat,
) -> Result<Option<SourceImage>, Error> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    let (loop_count, dimensions, frames, metadata) = match source_format {
        ImageFormat::Webp => {
            let mut decoder = WebPDecoder::new(reader)?;
            if !decoder.has_animation() {
                return Ok(None);
            }
            let loop_count = decoder.loop_count();
            let (width, height) = decoder.dimensions();
            // EXIF rides in a RIFF chunk of the animated container
            let metadata = ImageMetadata {
                exif: decoder
                    .exif_metadata()
                    .map_err(Error::new)?
                    .and_then(crate::metadata::normalize_exif_payload),
                icc: decoder.icc_profile().map_err(Error::new)?,
                xmp: None,
                exif_applied_orientation: decoder.orientation().map_err(Error::new)?
                    == Orientation::NoTransforms,
            };
            let mut frames = Vec::new();
            for frame in decoder.into_frames() {
                let frame = frame.map_err(Error::new)?;
                frames.push(FrameData::from_image_frame(frame));
            }
            (loop_count, (width, height), frames, metadata)
        }
        ImageFormat::Png => {
            let mut decoder = PngDecoder::new(reader)?;
            if !decoder.is_apng().map_err(Error::new)? {
                return Ok(None);
            }
            let (width, height) = decoder.dimensions();
            let metadata = ImageMetadata {
                exif: decoder
                    .exif_metadata()
                    .map_err(Error::new)?
                    .and_then(crate::metadata::normalize_exif_payload),
                icc: decoder.icc_profile().map_err(Error::new)?,
                xmp: None,
                exif_applied_orientation: decoder.orientation().map_err(Error::new)?
                    == Orientation::NoTransforms,
            };
            let apng = decoder.apng().map_err(Error::new)?;
            let loop_count = apng.loop_count();
            let mut frames = Vec::new();
            for frame in apng.into_frames() {
                let frame = frame.map_err(Error::new)?;
                frames.push(FrameData::from_image_frame(frame));
            }
            (loop_count, (width, height), frames, metadata)
        }
        _ => return Ok(None),
    };

    if frames.is_empty() {
        return Err(Error::from_string(format!(
            "Animated image {} does not contain any frames",
            path.display()
        )));
    }
    if frames.len() == 1 {
        // single-frame animation: resolve to the fast still path
        let frame = frames.into_iter().next().expect("checked length above");
        return Ok(Some(SourceImage {
            content: ImageContent::Still(DynamicImage::ImageRgba8(frame.buffer)),
            metadata,
            source_format,
            source_path: path.to_path_buf(),
        }));
    }

    Ok(Some(SourceImage {
        content: ImageContent::Animated(AnimationData {
            width: dimensions.0,
            height: dimensions.1,
            frames,
            loop_count,
        }),
        metadata,
        source_format,
        source_path: path.to_path_buf(),
    }))
}

fn open_reader(path: &Path) -> Result<ImageReader<BufReader<fs::File>>, Error> {
    let mut reader = ImageReader::new(BufReader::new(fs::File::open(path)?));
    // disable image-rs decoding limits
    reader.no_limits();
    Ok(reader)
}

fn collect_metadata(decoder: &mut impl ImageDecoder) -> Result<ImageMetadata, Error> {
    // WS4: payloads are normalized to the raw TIFF convention here —
    // decoders may include the b"Exif\0\0" marker prefix (e.g. the WebP
    // EXIF chunk body); the pipeline and encoders only ever see raw TIFF.
    let exif = decoder
        .exif_metadata()
        .map_err(Error::new)?
        .and_then(crate::metadata::normalize_exif_payload);
    let icc = decoder.icc_profile().map_err(Error::new)?;
    let orientation: Orientation = decoder.orientation().map_err(Error::new)?;
    // decoders do not apply the EXIF orientation; applying it is WS4 territory
    let exif_applied_orientation = orientation == Orientation::NoTransforms;
    Ok(ImageMetadata {
        exif,
        icc,
        xmp: None,
        exif_applied_orientation,
    })
}

/// Retry decoding with the jpeg-decoder crate (progressive jpegs) and
/// explicit extension-based formats before giving up.
fn fallback_retry_read_image(input_path: &Path, input_error: Error) -> Result<DynamicImage, Error> {
    let err = input_error;
    let ext = input_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // try jpeg-decoder to support loading progressive jpegs
    if (ext == "pjpeg" || ext == "jpg" || ext == "jpeg")
        && let Ok(file) = fs::File::open(input_path)
    {
        let mut decoder = jpeg_decoder::Decoder::new(file);
        if let Ok(pixels) = decoder.decode()
            && let Some(info) = decoder.info()
        {
            // Convert raw pixels to RgbImage
            let img =
                image::RgbImage::from_raw(u32::from(info.width), u32::from(info.height), pixels)
                    .ok_or_else(|| {
                        Error::from_string(
                            "Failed to convert jpeg-decoder output to RgbImage".to_string(),
                        )
                    })?;
            return Ok(DynamicImage::ImageRgb8(img));
        }
    }

    let mut reader = ImageReader::open(input_path)?;
    // disable image-rs decoding limits
    reader.limits(Limits::no_limits());
    match ext.as_str() {
        "pjpeg" | "jpg" | "jpeg" => reader.set_format(ImageCrateFormat::Jpeg),
        "x-png" | "png" => reader.set_format(ImageCrateFormat::Png),
        _ => return Err(err), // nothing else to try
    }

    if let Ok(decoded) = reader.decode() {
        Ok(decoded)
    } else {
        Err(err)
    }
}

/// trait to enable discard_input_alpha_channel functionality
trait IntoWithoutAlpha {
    fn into_without_alpha(self) -> DynamicImage;
}

impl IntoWithoutAlpha for DynamicImage {
    fn into_without_alpha(self) -> DynamicImage {
        match self {
            DynamicImage::ImageLumaA8(_) => DynamicImage::ImageLuma8(self.into_luma8()),
            DynamicImage::ImageLumaA16(_) => DynamicImage::ImageLuma16(self.into_luma16()),

            DynamicImage::ImageRgba8(_) => DynamicImage::ImageRgb8(self.into_rgb8()),
            DynamicImage::ImageRgba16(_) => DynamicImage::ImageRgb16(self.into_rgb16()),
            DynamicImage::ImageRgba32F(_) => DynamicImage::ImageRgb32F(self.into_rgb32f()),

            // otherwise input already has no alpha channel
            other => other,
        }
    }
}

/// returns the buffer size of the DynamicImage in B
pub fn buffer_size_bytes(img: &DynamicImage) -> u64 {
    let (w, h) = (img.width(), img.height());
    let bytes_per_pixel = img.color().bytes_per_pixel() as u64;

    w as u64 * h as u64 * bytes_per_pixel
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::metadata::LoopCount;
    use image::{ExtendedColorType, Frame, RgbaImage};

    fn temp_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "byteshaver-input-test-{}-{}",
            std::process::id(),
            name
        ));
        path
    }

    #[test]
    fn animated_gif_flows_through_animation_data() {
        let path = temp_path("anim.gif");
        let file = fs::File::create(&path).expect("create temp gif");
        let mut encoder = GifEncoder::new(file);
        encoder.set_repeat(Repeat::Infinite).expect("set repeat");
        for i in 0..3u8 {
            let mut buffer = RgbaImage::from_pixel(8, 8, image::Rgba([i, 0, 0, 255]));
            buffer.get_pixel_mut(0, 0).0[0] = 255;
            encoder
                .encode_frame(Frame::new(buffer))
                .expect("encode frame");
        }
        drop(encoder);

        let source = load_source(&path).expect("load gif");
        assert_eq!(source.source_format, ImageFormat::Gif);
        let ImageContent::Animated(animation) = &source.content else {
            panic!("expected animated content");
        };
        assert_eq!(animation.width, 8);
        assert_eq!(animation.height, 8);
        assert_eq!(animation.frames.len(), 3);
        assert!(matches!(animation.loop_count, LoopCount::Infinite));

        // alpha discard keeps RGBA8 canonical buffers with opaque pixels
        let flattened = source.into_without_alpha();
        let ImageContent::Animated(animation) = &flattened.content else {
            panic!("expected animated content");
        };
        assert!(
            animation
                .frames
                .iter()
                .all(|f| f.buffer.pixels().all(|p| p.0[3] == 255))
        );

        // still encoders take the first frame of animated input
        let encoder = crate::converter::traits::EncoderRegistry::build(
            &crate::config::EncoderConfig::Png(crate::config::PngOptions::default()),
            crate::converter::traits::ThreadBudget::global(),
        );
        let encoded = encoder.encode(&flattened).expect("encode first frame");
        assert!(!encoded.is_empty());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn still_input_decodes_with_detected_format() {
        let path = temp_path("still.png");
        let rgba = RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        image::save_buffer(&path, rgba.as_raw(), 4, 4, ExtendedColorType::Rgba8)
            .expect("write png");

        let source = load_source(&path).expect("load png");
        assert_eq!(source.source_format, ImageFormat::Png);
        let ImageContent::Still(image) = &source.content else {
            panic!("expected still content");
        };
        assert_eq!((image.width(), image.height()), (4, 4));

        let _ = fs::remove_file(&path);
    }
}
