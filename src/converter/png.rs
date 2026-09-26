//! This module provides png conversion via the image crate

use crate::Error;
use crate::converter::DEPENDENCIES;
use crate::format::ImageFormat;
use clap::ValueEnum;
use image::{DynamicImage, ImageEncoder as _};

/// Compression type of the png encoder from the image crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum CompressionType {
    /// Default compression level.
    Default,
    /// High compression level, low performance.
    Best,
    /// Low compression level, high performance.
    Fast,
}

/// Filter type of the png encoder from the image crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum FilterType {
    /// No filtering applied.
    NoFilter,
    /// Subtractive filtering.
    Sub,
    /// Upwards filtering.
    Up,
    /// Average filtering.
    Avg,
    /// Paeth filtering.
    Paeth,
    /// Adaptive filtering per scanline.
    Adaptive,
}

/// Options of the image-crate png encoder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PngOptions {
    /// PNG compression type. Defaults to the encoder default.
    pub compression_type: Option<CompressionType>,
    /// PNG filter type. Defaults to the encoder default.
    pub filter_type: Option<FilterType>,
}

fn convert_compression_type_to_ext(
    compression_type: Option<CompressionType>,
) -> image::codecs::png::CompressionType {
    match compression_type.unwrap_or(CompressionType::Default) {
        CompressionType::Default => image::codecs::png::CompressionType::Default,
        CompressionType::Fast => image::codecs::png::CompressionType::Fast,
        CompressionType::Best => image::codecs::png::CompressionType::Best,
    }
}
fn convert_filter_type_to_ext(filter_type: Option<FilterType>) -> image::codecs::png::FilterType {
    match filter_type.unwrap_or(FilterType::Adaptive) {
        FilterType::NoFilter => image::codecs::png::FilterType::NoFilter,
        FilterType::Sub => image::codecs::png::FilterType::Sub,
        FilterType::Up => image::codecs::png::FilterType::Up,
        FilterType::Avg => image::codecs::png::FilterType::Avg,
        FilterType::Paeth => image::codecs::png::FilterType::Paeth,
        FilterType::Adaptive => image::codecs::png::FilterType::Adaptive,
    }
}

/// Encoder for png format using the image crate.
pub struct PngEncoder {
    /// Encoding options.
    pub options: PngOptions,
}

impl PngEncoder {
    /// Creates an encoder with the given options.
    pub fn new(options: PngOptions) -> Self {
        PngEncoder { options }
    }
}

impl super::ImageEncoder for PngEncoder {
    fn format(&self) -> ImageFormat {
        ImageFormat::Png
    }

    fn extension(&self) -> &'static str {
        "png"
    }

    fn describe(&self) -> String {
        encoder_info()
    }

    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error> {
        encode_png(
            image,
            self.options.compression_type,
            self.options.filter_type,
        )
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

    format!("Using \"png (from image crate)\" ({})", image_version)
}

/// Encodes a `DynamicImage` to bytes of png format
pub(crate) fn encode_png(
    image: &DynamicImage,
    compression_type: Option<CompressionType>,
    filter_type: Option<FilterType>,
) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    let ext_compression_type = convert_compression_type_to_ext(compression_type); // default is fast
    let ext_filter_type = convert_filter_type_to_ext(filter_type); // default is adaptive
    if image.color().has_alpha() {
        let source_image = image.to_rgba8();
        image::codecs::png::PngEncoder::new_with_quality(
            &mut output,
            ext_compression_type,
            ext_filter_type,
        )
        .write_image(
            source_image.as_ref(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| Error::from_string(format!("png encoding failed: {:?}", e)))?;
    } else {
        let source_image = image.to_rgb8();
        image::codecs::png::PngEncoder::new_with_quality(
            &mut output,
            ext_compression_type,
            ext_filter_type,
        )
        .write_image(
            source_image.as_ref(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| Error::from_string(format!("png encoding failed: {:?}", e)))?;
    }
    Ok(output)
}
