//! This module provides png conversion via the png crate
//!
//! WS4: switched from the image-crate `PngEncoder` wrapper to the `png`
//! crate directly so `Info::exif_metadata` (the `eXIf` chunk) can be set.
//! The option→encoder mapping replicates `image::codecs::png::PngEncoder`
//! exactly (byte-identical output for identical options):
//! `Default→Compression::Balanced`, `Best→Compression::High`,
//! `Fast→Compression::Fast`; filter types map 1:1.

use crate::Error;
use crate::converter::DEPENDENCIES;
use crate::format::ImageFormat;
use clap::ValueEnum;
use image::DynamicImage;
use std::borrow::Cow;
use std::io::Write;

/// Compression type of the png encoder (surface of the image-crate encoder).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum CompressionType {
    /// Default compression level.
    Default,
    /// High compression level, low performance.
    Best,
    /// Low compression level, high performance.
    Fast,
}

/// Filter type of the png encoder (surface of the image-crate encoder).
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

/// Options of the png encoder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PngOptions {
    /// PNG compression type. Defaults to the encoder default.
    pub compression_type: Option<CompressionType>,
    /// PNG filter type. Defaults to the encoder default.
    pub filter_type: Option<FilterType>,
}

/// Replicates `image::codecs::png::CompressionType` → `png::Compression`.
fn convert_compression_type_to_ext(compression_type: Option<CompressionType>) -> png::Compression {
    match compression_type.unwrap_or(CompressionType::Default) {
        CompressionType::Default => png::Compression::Balanced,
        CompressionType::Fast => png::Compression::Fast,
        CompressionType::Best => png::Compression::High,
    }
}

/// Replicates `image::codecs::png::FilterType` → `png::Filter`.
fn convert_filter_type_to_ext(filter_type: Option<FilterType>) -> png::Filter {
    match filter_type.unwrap_or(FilterType::Adaptive) {
        FilterType::NoFilter => png::Filter::NoFilter,
        FilterType::Sub => png::Filter::Sub,
        FilterType::Up => png::Filter::Up,
        FilterType::Avg => png::Filter::Avg,
        FilterType::Paeth => png::Filter::Paeth,
        FilterType::Adaptive => png::Filter::Adaptive,
    }
}

/// Encoder for png format using the png crate.
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
            None,
        )
    }

    fn encode_still_image_with_metadata(
        &self,
        image: &DynamicImage,
        metadata: &crate::metadata::ImageMetadata,
    ) -> Result<Vec<u8>, Error> {
        // WS4: the resolved EXIF payload becomes the eXIf chunk body
        encode_png(
            image,
            self.options.compression_type,
            self.options.filter_type,
            metadata.exif.as_deref(),
        )
    }
}

/// Provides encoder information
fn encoder_info() -> String {
    // we might have multiple versions of the package, use rfind to find the newest one
    let mut png_version = "";
    match DEPENDENCIES.iter().rfind(|&&(name, _)| name == "png") {
        Some((_name, version)) => {
            png_version = version;
        }
        None => {
            println!("Package 'png' not found");
        }
    };

    format!("Using \"png\" ({})", png_version)
}

/// Encodes a `DynamicImage` to bytes of png format, optionally embedding an
/// EXIF payload as the `eXIf` chunk (raw TIFF payload, no marker prefix).
fn encode_png(
    image: &DynamicImage,
    compression_type: Option<CompressionType>,
    filter_type: Option<FilterType>,
    exif_payload: Option<&[u8]>,
) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    let (color_type, buffer): (png::ColorType, Vec<u8>) = if image.color().has_alpha() {
        (png::ColorType::Rgba, image.to_rgba8().into_raw())
    } else {
        (png::ColorType::Rgb, image.to_rgb8().into_raw())
    };

    let mut info = png::Info::with_size(image.width(), image.height());
    if let Some(exif) = exif_payload {
        info.exif_metadata = Some(Cow::Borrowed(exif));
    }

    let mut encoder = png::Encoder::with_info(&mut output, info)
        .map_err(|e| Error::from_string(format!("png encoding failed: {:?}", e)))?;
    encoder.set_color(color_type);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(convert_compression_type_to_ext(compression_type));
    encoder.set_filter(convert_filter_type_to_ext(filter_type));

    let mut writer = encoder
        .write_header()
        .map_err(|e| Error::from_string(format!("png encoding failed: {:?}", e)))?;
    writer
        .write_image_data(&buffer)
        .map_err(|e| Error::from_string(format!("png encoding failed: {:?}", e)))?;
    writer
        .finish()
        .map_err(|e| Error::from_string(format!("png encoding failed: {:?}", e)))?;
    let _ = output.flush();
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_mapping_matches_image_crate_semantics() {
        // image 0.25 maps Default→Balanced, Best→High, Fast→Fast
        assert!(matches!(
            convert_compression_type_to_ext(None),
            png::Compression::Balanced
        ));
        assert!(matches!(
            convert_compression_type_to_ext(Some(CompressionType::Best)),
            png::Compression::High
        ));
        assert!(matches!(
            convert_compression_type_to_ext(Some(CompressionType::Fast)),
            png::Compression::Fast
        ));
        assert!(matches!(
            convert_filter_type_to_ext(None),
            png::Filter::Adaptive
        ));
        assert!(matches!(
            convert_filter_type_to_ext(Some(FilterType::Paeth)),
            png::Filter::Paeth
        ));
    }

    #[test]
    fn png_encoding_round_trips_pixels() {
        let mut buffer = image::RgbaImage::new(3, 2);
        for (x, _, pixel) in buffer.enumerate_pixels_mut() {
            *pixel = image::Rgba([x as u8, 7, 9, 255]);
        }
        let image = DynamicImage::ImageRgba8(buffer);
        let encoded = encode_png(&image, None, None, None).expect("encode");
        let decoded = image::load_from_memory(&encoded).expect("decode");
        assert_eq!(decoded.to_rgba8(), image.to_rgba8());
    }
}
