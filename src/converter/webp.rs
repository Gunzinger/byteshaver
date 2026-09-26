//! This module provides webp conversion via the webp crate

use crate::Error;
use crate::converter::DEPENDENCIES;
use crate::format::ImageFormat;
use image::DynamicImage;
use webp::Encoder;

/// Options of the webp-crate webp encoder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WebpOptions {
    /// Use lossless encoding mode. Defaults to false.
    pub lossless: bool,
    /// Target quality (0 - 100, lower is worse but results in smaller files). Defaults to 90.0.
    pub quality: f32,
}

impl Default for WebpOptions {
    fn default() -> Self {
        WebpOptions {
            lossless: false,
            quality: 90.,
        }
    }
}

/// Encoder for webp format using the webp crate.
pub struct WebpEncoder {
    /// Encoding options.
    pub options: WebpOptions,
}

impl WebpEncoder {
    /// Creates an encoder with the given options.
    pub fn new(options: WebpOptions) -> Self {
        WebpEncoder { options }
    }
}

impl super::ImageEncoder for WebpEncoder {
    fn format(&self) -> ImageFormat {
        ImageFormat::Webp
    }

    fn extension(&self) -> &'static str {
        "webp"
    }

    fn describe(&self) -> String {
        encoder_info(self.options.lossless, self.options.quality)
    }

    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error> {
        encode_webp(image, self.options.lossless, self.options.quality)
    }
}

/// Provides encoder information
fn encoder_info(lossless: bool, qualify: f32) -> String {
    // we might have multiple versions of the package, use rfind to find the newest one
    let mut webp_version = "";
    match DEPENDENCIES.iter().rfind(|&&(name, _)| name == "webp") {
        Some((_name, version)) => {
            webp_version = version;
        }
        None => {
            println!("Package 'webp' not found");
        }
    };

    format!(
        "Using \"webp\" ({}) with options (lossless: {}, qualify: {})",
        webp_version, lossless, qualify
    )
}

/// Encodes a `DynamicImage` to bytes of webp format
fn encode_webp(image: &DynamicImage, lossless: bool, quality: f32) -> Result<Vec<u8>, Error> {
    let converted_image: Option<DynamicImage> = match image {
        DynamicImage::ImageLuma8(_) => Some(DynamicImage::ImageRgb8(image.to_rgb8())),
        DynamicImage::ImageLumaA8(_) => Some(DynamicImage::ImageRgba8(image.to_rgba8())),
        _ => None,
    };

    let encoder = if let Some(ref img) = converted_image {
        // Use the converted image (Luma[A]8 paths are unimplemented in the webp lib :D
        Encoder::from_image(img).map_err(|e| {
            Error::from_string(format!(
                "Failed to create webp encoder for luma input: {:?}",
                e
            ))
        })?
    } else {
        Encoder::from_image(image)
            .map_err(|e| Error::from_string(format!("Failed to create webp encoder: {:?}", e)))?
    };

    let webp_data = encoder
        .encode_simple(lossless, quality)
        .map_err(|e| Error::from_string(format!("webp encoding failed: {:?}", e)))?;

    Ok(webp_data.to_vec())
}
