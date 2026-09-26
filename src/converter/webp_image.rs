//! This module provides webp conversion via the image crate

use crate::Error;
use crate::converter::DEPENDENCIES;
use crate::format::ImageFormat;
use image::{DynamicImage, ImageEncoder as _};

/// Encoder for webp format using the lossless VP8L encoder from the image crate.
#[derive(Default)]
pub struct WebpImageEncoder;

impl WebpImageEncoder {
    /// Creates an encoder with default options.
    pub fn new() -> Self {
        WebpImageEncoder
    }
}

impl super::ImageEncoder for WebpImageEncoder {
    fn format(&self) -> ImageFormat {
        ImageFormat::WebpImage
    }

    fn extension(&self) -> &'static str {
        "webp"
    }

    fn describe(&self) -> String {
        encoder_info()
    }

    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error> {
        encode_webp_image(image, None)
    }

    fn encode_still_image_with_metadata(
        &self,
        image: &DynamicImage,
        metadata: &crate::metadata::ImageMetadata,
    ) -> Result<Vec<u8>, Error> {
        // WS4: the lossless VP8L container is rebuilt with an EXIF chunk
        encode_webp_image(image, metadata.exif.as_deref())
    }
}

/// Provides encoder information
fn encoder_info() -> String {
    // we might have multiple versions of the package, use rfind to find the newest one
    let mut image_version = "";
    match DEPENDENCIES.iter().rfind(|&&(name, _)| name == "image") {
        Some((_name, version)) => {
            image_version = version;
        }
        None => {
            println!("Package 'image' not found");
        }
    };

    format!("Using \"webp (from image crate)\" ({})", image_version)
}

/// Encodes a `DynamicImage` to bytes of webp format; an optional EXIF
/// payload (raw TIFF stream) is muxed into the container afterwards.
fn encode_webp_image(image: &DynamicImage, exif_payload: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    let encode = if image.color().has_alpha() {
        let source_image = image.to_rgba8();
        image::codecs::webp::WebPEncoder::new_lossless(&mut output).write_image(
            source_image.as_ref(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
    } else {
        let source_image = image.to_rgb8();
        image::codecs::webp::WebPEncoder::new_lossless(&mut output).write_image(
            source_image.as_ref(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
    };
    encode.map_err(|e| Error::from_string(format!("webp-image encoding failed: {:?}", e)))?;

    let Some(payload) = exif_payload else {
        return Ok(output);
    };
    match crate::metadata::riff::mux_exif(&output, payload) {
        Some(muxed) => Ok(muxed),
        None => {
            println!("Warning: could not embed EXIF into the webp container; metadata skipped");
            Ok(output)
        }
    }
}
