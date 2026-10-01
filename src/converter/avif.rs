//! This module provides avif conversion via the ravif crate

use crate::Error;
use crate::converter::DEPENDENCIES;
use crate::converter::traits::ThreadBudget;
use crate::format::ImageFormat;
use clap::ValueEnum;
use image::DynamicImage;
use ravif_new::*;
use rgb::FromSlice;

#[cfg(feature = "enc-avif")]
use std::panic::{self, AssertUnwindSafe};

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
        let info = encoder_info(
            self.options.quality,
            self.options.speed,
            self.options.bit_depth,
            self.options.color_model,
        );
        // files that carry EXIF take the libheif route (much slower) — make
        // the speed difference explainable
        #[cfg(feature = "enc-avif")]
        let info = format!("{info}; EXIF is embedded via libheif when present");
        info
    }

    fn supports_animation(&self) -> bool {
        false
    }

    /// EXIF can be embedded when the `enc-avif` feature routes the encode
    /// through libheif (ravif itself has no metadata API). Without the
    /// feature the pipeline warns and drops the payload.
    #[cfg(feature = "enc-avif")]
    fn supports_metadata(&self) -> bool {
        true
    }

    #[cfg(not(feature = "enc-avif"))]
    fn supports_metadata(&self) -> bool {
        false
    }

    fn adjust_threading(&mut self, budget: ThreadBudget) {
        self.num_threads = budget.threads_per_encoder;
    }

    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error> {
        encode_avif(image, &self.options, self.num_threads)
    }

    fn encode_still_image_with_metadata(
        &self,
        image: &DynamicImage,
        metadata: &crate::metadata::ImageMetadata,
    ) -> Result<Vec<u8>, Error> {
        // ravif has no metadata API: when EXIF survives the policy, the file
        // is encoded through libheif instead. If libheif (or its AV1 encoder
        // plugin) is unavailable at runtime, fall back to ravif and warn —
        // the metadata is then lost, matching the feature-off behavior.
        #[cfg(feature = "enc-avif")]
        if let Some(exif) = metadata.exif.as_deref() {
            match encode_avif_via_heif(image, &self.options, self.num_threads, exif) {
                Ok(bytes) => return Ok(bytes),
                Err(err) => {
                    println!(
                        "Warning: AVIF encoding with EXIF via libheif failed ({err}); \
                         falling back to ravif, EXIF metadata is dropped"
                    );
                }
            }
        }
        #[cfg(not(feature = "enc-avif"))]
        let _ = metadata;
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

/// Encodes a `DynamicImage` to avif via the native libheif encoder, embedding
/// the EXIF payload (`heif_context_add_exif_metadata`, raw TIFF convention —
/// libheif prepends the 4-byte TIFF-header offset itself).
///
/// Used only when EXIF must survive into the output (the `enc-avif` feature);
/// everything else goes through the faster ravif path above. The ravif-only
/// options (`bit_depth`, `color_model`, `alpha_*`) do not apply here; quality
/// and speed map onto the AV1 encoder as lossy quality 0-100 and the `speed`
/// parameter (clamped to the plugin's range, 0 = slowest).
#[cfg(feature = "enc-avif")]
fn encode_avif_via_heif(
    image: &DynamicImage,
    options: &AvifOptions,
    num_threads: Option<usize>,
    exif: &[u8],
) -> Result<Vec<u8>, Error> {
    // libheif is C/C++ code: contain panics so a broken plugin cannot kill
    // the whole batch (nothing borrowed here escapes the closure; the
    // context/handles are not unwind-safe).
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        encode_avif_via_heif_inner(image, options, num_threads, exif)
    }));
    match result {
        Ok(inner) => inner,
        Err(payload) => Err(Error::from_string(format!(
            "libheif avif encoding panicked: {}",
            panic_message(&payload)
        ))),
    }
}

#[cfg(feature = "enc-avif")]
fn encode_avif_via_heif_inner(
    image: &DynamicImage,
    options: &AvifOptions,
    num_threads: Option<usize>,
    exif: &[u8],
) -> Result<Vec<u8>, Error> {
    use libheif_rs::{
        Channel, ColorSpace, CompressionFormat, EncoderParameterValue, EncoderQuality, HeifContext,
        Image as HeifImage, LibHeif, RgbChroma,
    };

    let lib_heif = LibHeif::new();
    let mut encoder = lib_heif
        .encoder_for_format(CompressionFormat::Av1)
        .map_err(|err| {
            Error::from_string(format!(
                "libheif provides no AV1 encoder (is an encoder plugin like libheif-aomenc \
                 installed?): {err}"
            ))
        })?;

    // ravif quality 0-100 (higher is better) == libheif lossy quality 0-100;
    // there is no separate alpha quality knob in libheif
    encoder
        .set_quality(EncoderQuality::Lossy(
            options.quality.round().clamp(0., 100.) as u8,
        ))
        .map_err(|err| Error::from_string(format!("libheif encoder setup failed: {err}")))?;
    // aom plugin parameters; guard with `parameter` so other/older plugins
    // without these knobs still work
    if encoder.parameter("speed").ok().flatten().is_some() {
        // both scales count 1 as slowest (ravif 1-10, aom cpu-used 0-8)
        let speed = i32::from(options.speed.clamp(1, 9)) - 1;
        encoder
            .set_parameter_value("speed", EncoderParameterValue::Int(speed))
            .map_err(|err| Error::from_string(format!("libheif encoder setup failed: {err}")))?;
    }
    if let Some(budget) = num_threads
        && encoder.parameter("threads").ok().flatten().is_some()
    {
        // libheif runs its own encoder threads (no shared rayon pool here)
        encoder
            .set_parameter_value(
                "threads",
                EncoderParameterValue::Int(i32::try_from(budget).unwrap_or(i32::MAX)),
            )
            .map_err(|err| Error::from_string(format!("libheif encoder setup failed: {err}")))?;
    }

    // Build the interleaved RGB(A) input plane; rows are strided, so copy
    // row-by-row (same decoding-side layout as `crate::input::heif`).
    let has_alpha = image.color().has_alpha();
    let chroma = if has_alpha {
        RgbChroma::Rgba
    } else {
        RgbChroma::Rgb
    };
    let pixels: Vec<u8> = if has_alpha {
        image.to_rgba8().into_raw()
    } else {
        image.to_rgb8().into_raw()
    };
    let (width, height) = (image.width(), image.height());
    let bytes_per_pixel = if has_alpha { 4usize } else { 3usize };
    let row_len = width as usize * bytes_per_pixel;

    let mut heif_image = HeifImage::new(width, height, ColorSpace::Rgb(chroma))
        .map_err(|err| Error::from_string(format!("libheif image allocation failed: {err}")))?;
    heif_image
        .create_plane(Channel::Interleaved, width, height, 8)
        .map_err(|err| Error::from_string(format!("libheif plane allocation failed: {err}")))?;
    {
        let planes = heif_image.planes_mut();
        let plane = planes.interleaved.ok_or_else(|| {
            Error::from_string("libheif did not return an interleaved plane".to_string())
        })?;
        if plane.data.len() < row_len * height as usize {
            return Err(Error::from_string(
                "libheif interleaved plane is too small for the image".to_string(),
            ));
        }
        for (y, row) in pixels.chunks_exact(row_len).enumerate() {
            let start = y * plane.stride;
            plane.data[start..start + row_len].copy_from_slice(row);
        }
    }

    let mut context = HeifContext::new()
        .map_err(|err| Error::from_string(format!("libheif context creation failed: {err}")))?;
    let handle = context
        .encode_image(&heif_image, &mut encoder, None)
        .map_err(|err| Error::from_string(format!("libheif avif encoding failed: {err}")))?;
    if !exif.is_empty()
        && let Err(err) = context.add_exif_metadata(&handle, exif)
    {
        return Err(Error::from_string(format!(
            "libheif could not attach the EXIF payload: {err}"
        )));
    }
    context
        .write_to_bytes()
        .map_err(|err| Error::from_string(format!("libheif avif serialization failed: {err}")))
}

/// Best-effort extraction of a panic message (mirrors `crate::input::heif`).
#[cfg(feature = "enc-avif")]
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}
