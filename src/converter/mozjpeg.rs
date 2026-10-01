//! This module provides optimized jpeg conversion via the mozjpeg crate

use crate::Error;
use crate::converter::DEPENDENCIES;
use crate::format::ImageFormat;
use crate::metadata::sink;
use image::{DynamicImage, EncodableLayout};
use std::panic;

/// Encoder for optimized jpeg format using the mozjpeg crate.
#[derive(Default)]
pub struct MozjpegEncoder;

impl MozjpegEncoder {
    /// Creates an encoder with default options.
    pub fn new() -> Self {
        MozjpegEncoder
    }
}

impl super::ImageEncoder for MozjpegEncoder {
    fn format(&self) -> ImageFormat {
        ImageFormat::Jpeg
    }

    fn extension(&self) -> &'static str {
        "jpeg"
    }

    fn describe(&self) -> String {
        format!(
            "still-image JPEG encoder (mozjpeg {})",
            super::dependency_version("mozjpeg")
        )
    }

    fn describe_options(&self) -> String {
        encoder_info()
    }

    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error> {
        encode_mozjpeg(image, None)
    }

    fn encode_still_image_with_metadata(
        &self,
        image: &DynamicImage,
        metadata: &crate::metadata::ImageMetadata,
    ) -> Result<Vec<u8>, Error> {
        // WS4: write the resolved EXIF payload as an APP1 marker
        let exif_marker = metadata
            .exif
            .as_deref()
            .and_then(|payload| match sink::exif_app1_marker(payload) {
                Some(marker) => Some(marker),
                None => {
                    println!(
                        "Warning: EXIF payload too large for a single JPEG APP1 segment; metadata skipped"
                    );
                    None
                }
            });
        encode_mozjpeg(image, exif_marker.as_deref())
    }
}

/// Provides encoder information
fn encoder_info() -> String {
    // we might have multiple versions of the package, use rfind to find the newest one
    let mut mozjpeg_version = "";
    match DEPENDENCIES.iter().rfind(|&&(name, _)| name == "mozjpeg") {
        Some((_name, version)) => {
            mozjpeg_version = version;
        }
        None => {
            println!("Package 'mozjpeg' not found");
        }
    };

    format!("Using \"mozjpeg\" ({})", mozjpeg_version)
}

/// Encodes a `DynamicImage` to bytes of jpeg format; an optional EXIF APP1
/// marker payload (`b"Exif\0\0"` + TIFF stream) is written right after
/// `start_compress`.
fn encode_mozjpeg(image: &DynamicImage, exif_app1: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    let result = panic::catch_unwind(|| {
        let mut comp = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        comp.set_size(image.width() as usize, image.height() as usize);

        let mut comp = comp.start_compress(Vec::new()).map_err(|e| {
            Error::from_string(format!("mozjpeg encoding (start_compress) failed: {:?}", e))
        })?;

        if let Some(exif) = exif_app1 {
            comp.write_marker(mozjpeg::Marker::APP(1), exif);
        }

        comp.write_scanlines(image.to_rgb8().as_bytes())
            .map_err(|e| {
                Error::from_string(format!(
                    "mozjpeg encoding (write_scanlines) failed: {:?}",
                    e
                ))
            })?;

        comp.finish()
            .map_err(|e| Error::from_string(format!("mozjpeg encoding (finish) failed: {:?}", e)))
    });

    result.unwrap_or_else(|e| {
        Err(Error::from_string(format!(
            "mozjpeg encoding panicked: {:?}",
            e
        )))
    })
}
