//! This module provides avif conversion via the ravif crate

use crate::Error;
use crate::converter::DEPENDENCIES;
use crate::converter::traits::ThreadBudget;
use crate::format::ImageFormat;
use clap::ValueEnum;
use image::DynamicImage;
use ravif_new::*;
use rgb::FromSlice;

/// Internal bit depth of a generated avif file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum BitDepth {
    /// Encode with 8 bits per channel.
    Eight,
    /// Encode with 10 bits per channel.
    Ten,
    /// Choose the bit depth automatically from the input data.
    Auto,
}

/// Internal color model of a generated avif file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum ColorModel {
    /// YCbCr color model (smaller files for photographic content).
    YCbCr,
    /// RGB color model (larger files, avoids chroma subsampling artifacts).
    RGB,
}

/// Internal alpha color mode of a generated avif file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum AlphaColorMode {
    /// Unassociated dirty alpha.
    UnassociatedDirty,
    /// Unassociated clean alpha.
    UnassociatedClean,
    /// Premultiplied alpha.
    Premultiplied,
}

/// Options of the ravif-based avif encoder.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AvifOptions {
    /// Target quality (0 - 100, lower is worse but results in smaller files). Defaults to 90.0.
    pub quality: f32,
    /// Encoding speed (1 - 10, lower is much slower but has a better quality and lower filesize). Defaults to 3.
    pub speed: u8,
    /// Internal bit depth of the generated avif file.
    pub bit_depth: Option<BitDepth>,
    /// Internal color model of the generated avif file.
    pub color_model: Option<ColorModel>,
    /// Internal alpha color mode of the generated avif file.
    pub alpha_color_mode: Option<AlphaColorMode>,
    /// Target alpha quality (0 - 100, lower is worse). Defaults to 90.0.
    pub alpha_quality: f32,
}

impl Default for AvifOptions {
    fn default() -> Self {
        AvifOptions {
            quality: 90.,
            speed: 3,
            bit_depth: None,
            color_model: None,
            alpha_color_mode: None,
            alpha_quality: 90.,
        }
    }
}

fn convert_bit_depth_to_ext(bit_depth: Option<BitDepth>) -> ravif_new::BitDepth {
    match bit_depth.unwrap_or(BitDepth::Auto) {
        BitDepth::Eight => ravif_new::BitDepth::Eight,
        BitDepth::Ten => ravif_new::BitDepth::Ten,
        BitDepth::Auto => ravif_new::BitDepth::Auto,
    }
}
fn convert_color_model_to_ext(color_model: Option<ColorModel>) -> ravif_new::ColorModel {
    match color_model.unwrap_or(ColorModel::YCbCr) {
        ColorModel::YCbCr => ravif_new::ColorModel::YCbCr,
        ColorModel::RGB => ravif_new::ColorModel::RGB,
    }
}
fn convert_alpha_color_mode_to_ext(
    alpha_color_mode: Option<AlphaColorMode>,
) -> ravif_new::AlphaColorMode {
    match alpha_color_mode.unwrap_or(AlphaColorMode::UnassociatedClean) {
        AlphaColorMode::UnassociatedDirty => ravif_new::AlphaColorMode::UnassociatedDirty,
        AlphaColorMode::UnassociatedClean => ravif_new::AlphaColorMode::UnassociatedClean,
        AlphaColorMode::Premultiplied => ravif_new::AlphaColorMode::Premultiplied,
    }
}

/// Encoder for avif format using the ravif crate.
pub struct AvifEncoder {
    /// Encoding options.
    pub options: AvifOptions,
    /// Explicit thread cap forwarded to ravif; `None` uses the shared global pool.
    num_threads: Option<usize>,
}

impl AvifEncoder {
    /// Creates an encoder with the given options.
    pub fn new(options: AvifOptions) -> Self {
        AvifEncoder {
            options,
            num_threads: None,
        }
    }
}

impl super::ImageEncoder for AvifEncoder {
    fn format(&self) -> ImageFormat {
        ImageFormat::Avif
    }

    fn extension(&self) -> &'static str {
        "avif"
    }

    fn describe(&self) -> String {
        format!(
            "still-image AV1 encoder (ravif {})",
            super::dependency_version("ravif")
        )
    }

    fn describe_options(&self) -> String {
        encoder_info(
            self.options.quality,
            self.options.speed,
            self.options.bit_depth,
            self.options.color_model,
        )
    }

    fn supports_animation(&self) -> bool {
        false
    }

    /// ravif has no metadata API, so EXIF cannot be embedded into avif
    /// outputs (the pipeline warns and counts these files).
    fn supports_metadata(&self) -> bool {
        false
    }

    fn adjust_threading(&mut self, budget: ThreadBudget) {
        self.num_threads = budget.threads_per_encoder;
    }

    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error> {
        encode_avif(image, &self.options, self.num_threads)
    }
}

/// Provides encoder information
fn encoder_info(
    quality: f32,
    speed: u8,
    bit_depth: Option<BitDepth>,
    color_model: Option<ColorModel>,
) -> String {
    // we have multiple ravif versions (one through image crate, one direct for the newest encoder version)
    //  with the implicit ordering through the build.rs generation we can use rfind to find the newest one
    let mut ravif_version = "";
    match DEPENDENCIES.iter().rfind(|&&(name, _)| name == "ravif") {
        Some((_name, version)) => {
            ravif_version = version;
        }
        None => {
            println!("Package 'ravif' not found");
        }
    };

    format!(
        "Using \"ravif\" ({}) with options (quality: {}, speed: {}, bit depth: {:?}, color model: {:?})",
        ravif_version,
        quality,
        speed,
        convert_bit_depth_to_ext(bit_depth),
        convert_color_model_to_ext(color_model)
    )
}

/// Encodes a `DynamicImage` to bytes of avif format
fn encode_avif(
    image: &DynamicImage,
    options: &AvifOptions,
    num_threads: Option<usize>,
) -> Result<Vec<u8>, Error> {
    let encoder = Encoder::new()
        .with_quality(options.quality)
        .with_speed(options.speed) // speed: 1-10, 10 is fastest, but still slow
        .with_num_threads(num_threads) // explicit budget; None = shared global rayon pool
        .with_bit_depth(convert_bit_depth_to_ext(options.bit_depth))
        .with_internal_color_model(convert_color_model_to_ext(options.color_model));
    let avif_res: EncodedImage = if image.color().has_alpha() {
        let source_image = image.to_rgba8();
        let image = Img::new(
            source_image.as_rgba(),
            image.width() as usize,
            image.height() as usize,
        );
        encoder
            .with_alpha_quality(options.alpha_quality) // TODO: expose parameter
            .with_alpha_color_mode(convert_alpha_color_mode_to_ext(options.alpha_color_mode)) // internal ravif default
            .encode_rgba(image)
            .map_err(|e| Error::from_string(format!("avif encoding failed: {:?}", e)))?
    } else {
        let source_image = image.to_rgb8();
        let image = Img::new(
            source_image.as_rgb(),
            image.width() as usize,
            image.height() as usize,
        );
        encoder
            .encode_rgb(image)
            .map_err(|e| Error::from_string(format!("avif encoding failed: {:?}", e)))?
    };
    Ok(avif_res.avif_file)
}
