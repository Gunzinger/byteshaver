//! JPEG XL source decoding via `jxl-oxide` (plan WS2).
//!
//! Stills are decoded losslessly into `DynamicImage` (8/16-bit, gray/RGB,
//! with or without alpha); animations decode into [`AnimationData`] with
//! real per-frame durations derived from the codestream animation
//! timescale. EXIF/XMP aux boxes and the original ICC profile are
//! collected into [`ImageMetadata`].
//!
//! Orientation: jxl-oxide applies the EXIF orientation during render, so
//! the decoded pixels are upright and `exif_applied_orientation` is set;
//! the (now redundant) Orientation tag is dropped from the EXIF payload.
//!
//! CMYK: the black channel cannot map onto RGB/RGBA buffers, so CMYK
//! images are converted to sRGB via `moxcms` (with a notice); if the
//! conversion fails the error is propagated.

use std::num::NonZeroU32;
use std::path::Path;
use std::time::Duration;

use image::metadata::LoopCount;
use image::{DynamicImage, ImageBuffer};
use jxl_oxide::{AuxBoxData, EnumColourEncoding, JxlImage, Moxcms, RenderingIntent};

use crate::Error;
use crate::format::ImageFormat;
use crate::input::{AnimationData, FrameData, ImageContent, SourceImage};
use crate::metadata::ImageMetadata;

/// Decodes a JPEG XL file into a [`SourceImage`].
pub fn load_source_jxl(path: &Path) -> Result<SourceImage, Error> {
    let mut image = JxlImage::builder()
        .open(path)
        .map_err(|err| Error::from_string(format!("jxl decode failed: {err}")))?;

    let bits_per_sample = image.image_header().metadata.bit_depth.bits_per_sample();
    // (tps_numerator, tps_denominator, num_loops) of the animation header,
    // present when the codestream is animated
    let animation_header = image
        .image_header()
        .metadata
        .animation
        .as_ref()
        .map(|animation| {
            (
                animation.tps_numerator,
                animation.tps_denominator,
                animation.num_loops,
            )
        });

    // CMYK inputs cannot map onto RGB(A) buffers: convert to sRGB up front
    // (the moxcms transform consumes the black channel).
    if image.pixel_format().has_black() {
        println!(
            "Warning: jxl input {} is CMYK; converting pixels to sRGB (black channel merged)",
            path.display()
        );
        image.set_cms(Moxcms);
        image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
    }

    let metadata = ImageMetadata {
        exif: extract_exif(&image),
        icc: image.original_icc().map(<[u8]>::to_vec),
        xmp: extract_xmp(&image),
        // jxl-oxide applies the EXIF orientation during render
        exif_applied_orientation: true,
    };

    if let Some(animation) = animation_header {
        decode_animated(&image, animation, metadata, path)
    } else {
        let render = render_frame(&image, 0)?;
        let content = ImageContent::Still(render_still(&render, bits_per_sample)?);
        Ok(SourceImage {
            content,
            metadata,
            source_format: ImageFormat::Jxl,
            source_path: path.to_path_buf(),
        })
    }
}

/// Renders one keyframe, mapping decode errors onto [`Error`].
fn render_frame(image: &JxlImage, keyframe: usize) -> Result<jxl_oxide::Render, Error> {
    image
        .render_frame(keyframe)
        .map_err(|err| Error::from_string(format!("jxl frame render failed: {err}")))
}

/// Converts a rendered still into a `DynamicImage`, preserving bit depth
/// and gray/alpha layout. Extra channels beyond alpha are dropped with a
/// warning by [`render_to_dynamic`].
fn render_still(render: &jxl_oxide::Render, bits_per_sample: u32) -> Result<DynamicImage, Error> {
    render_to_dynamic(render, bits_per_sample > 8, "still")
}

/// Converts a rendered frame into an RGBA8 canonical buffer for
/// [`AnimationData`] (16-bit animation frames are converted to 8-bit).
fn render_frame_rgba(
    render: &jxl_oxide::Render,
    keyframe: usize,
) -> Result<image::RgbaImage, Error> {
    let dynamic = render_to_dynamic(render, false, &format!("animation frame {keyframe}"))?;
    Ok(dynamic.to_rgba8())
}

/// Writes a rendered frame into an interleaved sample buffer and maps it
/// onto the matching `DynamicImage` variant.
///
/// The stream layout is gray(+alpha)/rgb(+alpha); CMYK inputs are already
/// converted to sRGB before rendering (see `load_source_jxl`), so any
/// channel count outside 1-4 is reported as an error.
fn render_to_dynamic(
    render: &jxl_oxide::Render,
    use16: bool,
    context: &str,
) -> Result<DynamicImage, Error> {
    let mut stream = render.stream();
    let (width, height, channels) = (stream.width(), stream.height(), stream.channels());
    if !(1..=4).contains(&channels) {
        return Err(Error::from_string(format!(
            "jxl {context}: unsupported channel count {channels}"
        )));
    }
    let sample_count = width as usize * height as usize * channels as usize;
    if use16 {
        let mut samples = vec![0u16; sample_count];
        let written = stream.write_to_buffer(&mut samples);
        if written != sample_count {
            return Err(Error::from_string(format!(
                "jxl {context}: unexpected sample count ({written} of {sample_count})"
            )));
        }
        Ok(match channels {
            1 => DynamicImage::ImageLuma16(
                ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_raw(width, height, samples)
                    .ok_or_else(|| buffer_error(context))?,
            ),
            2 => DynamicImage::ImageLumaA16(
                ImageBuffer::<image::LumaA<u16>, Vec<u16>>::from_raw(width, height, samples)
                    .ok_or_else(|| buffer_error(context))?,
            ),
            3 => DynamicImage::ImageRgb16(
                ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_raw(width, height, samples)
                    .ok_or_else(|| buffer_error(context))?,
            ),
            _ => DynamicImage::ImageRgba16(
                ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_raw(width, height, samples)
                    .ok_or_else(|| buffer_error(context))?,
            ),
        })
    } else {
        let mut samples = vec![0u8; sample_count];
        let written = stream.write_to_buffer(&mut samples);
        if written != sample_count {
            return Err(Error::from_string(format!(
                "jxl {context}: unexpected sample count ({written} of {sample_count})"
            )));
        }
        Ok(match channels {
            1 => DynamicImage::ImageLuma8(
                ImageBuffer::<image::Luma<u8>, Vec<u8>>::from_raw(width, height, samples)
                    .ok_or_else(|| buffer_error(context))?,
            ),
            2 => DynamicImage::ImageLumaA8(
                ImageBuffer::<image::LumaA<u8>, Vec<u8>>::from_raw(width, height, samples)
                    .ok_or_else(|| buffer_error(context))?,
            ),
            3 => DynamicImage::ImageRgb8(
                ImageBuffer::<image::Rgb<u8>, Vec<u8>>::from_raw(width, height, samples)
                    .ok_or_else(|| buffer_error(context))?,
            ),
            _ => DynamicImage::ImageRgba8(
                ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(width, height, samples)
                    .ok_or_else(|| buffer_error(context))?,
            ),
        })
    }
}

fn buffer_error(context: &str) -> Error {
    Error::from_string(format!("jxl {context}: buffer does not match dimensions"))
}

/// Decodes all keyframes into [`AnimationData`].
///
/// Tick math: the codestream animation header stores a timescale as
/// `tps_numerator / tps_denominator` ticks per second; a frame shown for
/// `t` ticks displays for `t * tps_denominator / tps_numerator` seconds
/// (rounded to whole nanoseconds; documented rounding).
fn decode_animated(
    image: &JxlImage,
    animation: (u32, u32, u32),
    metadata: ImageMetadata,
    path: &Path,
) -> Result<SourceImage, Error> {
    let (tps_numerator_raw, tps_denominator_raw, num_loops) = animation;
    let keyframes = image.num_loaded_keyframes();
    if keyframes == 0 {
        return Err(Error::from_string(format!(
            "Animated jxl {} does not contain any frames",
            path.display()
        )));
    }
    let tps_numerator = u64::from(tps_numerator_raw).max(1);
    let tps_denominator = u64::from(tps_denominator_raw).max(1);
    let loop_count = match num_loops {
        0 => LoopCount::Infinite,
        loops => LoopCount::Finite(NonZeroU32::new(loops).unwrap_or(NonZeroU32::MIN)),
    };

    let mut frames = Vec::with_capacity(keyframes);
    for keyframe in 0..keyframes {
        let render = render_frame(image, keyframe)?;
        let delay = ticks_to_duration(render.duration(), tps_numerator, tps_denominator);
        let buffer = render_frame_rgba(&render, keyframe)?;
        frames.push(FrameData { buffer, delay });
    }

    Ok(SourceImage {
        content: ImageContent::Animated(AnimationData {
            width: image.width(),
            height: image.height(),
            frames,
            loop_count,
        }),
        metadata,
        source_format: ImageFormat::Jxl,
        source_path: path.to_path_buf(),
    })
}

/// Converts animation ticks into a [`Duration`] using the codestream
/// timescale (`tps_numerator / tps_denominator` ticks per second).
fn ticks_to_duration(ticks: u32, tps_numerator: u64, tps_denominator: u64) -> Duration {
    let nanos =
        u128::from(ticks) * u128::from(tps_denominator) * 1_000_000_000 / u128::from(tps_numerator);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// Extracts the raw TIFF EXIF payload from the `Exif` aux box.
///
/// Box convention (libjxl/JXL spec): the payload starts with a 4-byte
/// offset field (big-endian as read by jxl-oxide); the TIFF header follows
/// at that offset. The Orientation tag is dropped because the renderer has
/// already applied the transform.
fn extract_exif(image: &JxlImage) -> Option<Vec<u8>> {
    match image.aux_boxes().first_exif() {
        Ok(AuxBoxData::Data(exif)) => {
            let offset = exif.tiff_header_offset() as usize;
            let payload = exif.payload().get(offset..).map(<[u8]>::to_vec)?;
            Some(drop_orientation_tag(payload))
        }
        Ok(_) => None,
        Err(err) => {
            println!("Warning: could not parse the jxl Exif box: {err}");
            None
        }
    }
}

/// Removes the Orientation tag from a raw TIFF payload (the pixels are
/// already upright). Falls back to the original payload when it cannot be
/// re-serialized.
#[cfg(feature = "exif")]
fn drop_orientation_tag(payload: Vec<u8>) -> Vec<u8> {
    use exif::Tag;
    use exif::experimental::Writer;

    let Ok(parsed) = crate::metadata::exif::parse(&payload) else {
        return payload;
    };
    let mut writer = Writer::new();
    let mut seen: Vec<(exif::In, Tag)> = Vec::new();
    let mut kept = 0usize;
    for field in parsed.fields() {
        if field.tag == Tag::Orientation {
            continue;
        }
        // the writer rejects duplicate fields; keep the first occurrence
        let key = (field.ifd_num, field.tag);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        writer.push_field(field);
        kept += 1;
    }
    let mut cursor = std::io::Cursor::new(Vec::new());
    match writer.write(&mut cursor, parsed.little_endian()) {
        Ok(()) if kept > 0 => cursor.into_inner(),
        // everything but Orientation vanished: keep the original payload
        _ => payload,
    }
}

#[cfg(not(feature = "exif"))]
fn drop_orientation_tag(payload: Vec<u8>) -> Vec<u8> {
    payload
}

/// Extracts the raw XMP packet from the `xml ` aux box.
fn extract_xmp(image: &JxlImage) -> Option<Vec<u8>> {
    match image.aux_boxes().first_xml() {
        AuxBoxData::Data(xml) => Some(xml.to_vec()),
        _ => None,
    }
}
