//! JPEG XL target encoding via thin in-tree FFI bindings to libjxl
//! (vendored static build through `jpegxl-src`; plan WS2 Option A).
//!
//! The [`JxlEncoder`] implements the full documented option surface:
//! lossless/quality/distance, effort, decoding speed, container forcing,
//! original profile, intensity target, bit depth, color encoding selection,
//! EXIF embedding (WS4 policy contract) and a raw `--setting` passthrough
//! for every `JxlEncoderFrameSettingId`. Animated input is encoded as real
//! animation with per-frame durations (this is why the in-tree FFI route
//! was chosen over the GPL `jpegxl-rs` bindings).
//!
//! # Animation timescale
//!
//! Frames are encoded with an animation timescale of 1000 ticks per second
//! (`tps_numerator=1000`, `tps_denominator=1`), i.e. one tick = one
//! millisecond. Source delays are truncated to whole milliseconds when
//! they cannot map exactly (documented rounding); libjxl itself limits
//! durations to `uint32_t` ticks.
//!
//! Threading: single-threaded libjxl encoder per file (no parallel runner
//! is registered); rayon already parallelizes across files.

use std::time::Duration;

use image::metadata::LoopCount;
use image::{ColorType, DynamicImage};

use crate::Error;
/// Manual `extern "C"` declarations of the vendored libjxl 0.12 encoder API.
pub mod ffi;

use self::ffi::{
    FrameSettingId, JXL_ENC_ERROR, JXL_ENC_NEED_MORE_OUTPUT, JXL_ENC_SUCCESS, JXL_FALSE, JXL_TRUE,
    JxlAnimationHeader, JxlBasicInfo, JxlColorEncoding, JxlColorSpace, JxlDataType, JxlEndianness,
    JxlFrameHeader, JxlPixelFormat,
};
use crate::converter::ImageEncoder;
use crate::format::ImageFormat;
use crate::input::{AnimationData, ImageContent, SourceImage};
use crate::metadata::exif_for_jxl;

/// Target bit depth of the encoded JXL (8 or 16 bits per channel).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
pub enum JxlBitDepthChoice {
    /// 8 bits per channel.
    Eight,
    /// 16 bits per channel.
    Sixteen,
}

/// Color encoding selection of the encoded JXL (mirrors `cjxl` choices).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum JxlColorEncodingChoice {
    /// Non-linear sRGB (default; matches the libjxl assumption for integer
    /// pixel buffers when no color encoding is set).
    #[default]
    Srgb,
    /// Linear sRGB.
    LinearSrgb,
    /// Non-linear sRGB grayscale (requires grayscale input).
    SrgbLuma,
    /// Linear sRGB grayscale (requires grayscale input).
    LinearSrgbLuma,
    /// Embed the source ICC profile verbatim (`JxlEncoderSetICCProfile`).
    IccPassthrough,
}

impl JxlColorEncodingChoice {
    /// Whether this choice selects a grayscale color encoding.
    fn is_luma(self) -> bool {
        matches!(
            self,
            JxlColorEncodingChoice::SrgbLuma | JxlColorEncodingChoice::LinearSrgbLuma
        )
    }
}

/// Options of the libjxl-based jpeg-xl encoder with CLI defaults resolved.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JxlOptions {
    /// True lossless mode. Overrides quality/distance and implies
    /// `original_profile`. Default false.
    pub lossless: bool,
    /// JPEG-style quality 0-100 (higher = better); mapped to distance via
    /// libjxl's `JxlEncoderDistanceFromQuality`. Default None.
    pub quality: Option<f32>,
    /// Maximum Butteraugli distance 0.0-25.0 (0 = lossless, 1.0 = visually
    /// lossless; libjxl default 1.0). Mutually exclusive with `quality`.
    pub distance: Option<f32>,
    /// Encoding effort 1 (fastest) - 10 (slowest/best). Default 7.
    pub effort: u8,
    /// Force the ISOBMFF container format. Auto-enabled when EXIF boxes are
    /// embedded. Default false.
    pub container: bool,
    /// Keep the original color profile (skip the XYB transform); needed for
    /// lossless. Default false.
    pub original_profile: bool,
    /// Target decode speed tier 0-4 (higher = faster decode, larger file).
    /// Default 0.
    pub decoding_speed: u8,
    /// Photometric target intensity in nits (HDR). Default: libjxl default
    /// (255).
    pub intensity_target: Option<f32>,
    /// Force output bit depth 8 or 16 (default: follow the input).
    pub bit_depth: Option<JxlBitDepthChoice>,
    /// Color encoding selection. Default: sRGB.
    pub color_encoding: Option<JxlColorEncodingChoice>,
    /// Advanced libjxl frame-setting passthrough (`--setting ID=VALUE`),
    /// resolved case-insensitively against [`FrameSettingId::ALL`].
    pub advanced: Vec<(String, i64)>,
    /// Active EXIF policy of the run; injected from the global config
    /// (default [`crate::metadata::policy::ExifPolicy::Strip`]). Only
    /// available with the `exif` feature.
    #[cfg(feature = "exif")]
    pub exif_policy: crate::metadata::policy::ExifPolicy,
}

impl Default for JxlOptions {
    fn default() -> Self {
        JxlOptions {
            lossless: false,
            quality: None,
            distance: None,
            effort: 7,
            container: false,
            original_profile: false,
            decoding_speed: 0,
            intensity_target: None,
            bit_depth: None,
            color_encoding: None,
            advanced: Vec::new(),
            #[cfg(feature = "exif")]
            exif_policy: crate::metadata::policy::ExifPolicy::Strip,
        }
    }
}

impl std::fmt::Display for JxlOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "lossless={} quality={} distance={} effort={} container={} \
             original-profile={} decoding-speed={} intensity-target={} bit-depth={} \
             color-encoding={} settings={}",
            self.lossless,
            self.quality.map_or("-".to_string(), |q| q.to_string()),
            self.distance.map_or("-".to_string(), |d| d.to_string()),
            self.effort,
            self.container,
            self.original_profile,
            self.decoding_speed,
            self.intensity_target
                .map_or("-".to_string(), |t| t.to_string()),
            match self.bit_depth {
                Some(JxlBitDepthChoice::Eight) => "8",
                Some(JxlBitDepthChoice::Sixteen) => "16",
                None => "auto",
            },
            match self.color_encoding {
                Some(choice) => format!("{choice:?}"),
                None => "srgb".to_string(),
            },
            self.advanced
                .iter()
                .map(|(id, value)| format!("{id}={value}"))
                .collect::<Vec<_>>()
                .join(","),
        )
    }
}

/// Encoder for jpeg-xl format using the libjxl C API (vendored build).
pub struct JxlEncoder {
    /// Encoding options.
    pub options: JxlOptions,
}

impl JxlEncoder {
    /// Creates an encoder with the given options.
    pub fn new(options: JxlOptions) -> Self {
        JxlEncoder { options }
    }
}

/// Normalized pixel buffer of one frame: interleaved samples in native
/// byte order plus the channel count and sample depth actually handed to
/// libjxl.
struct FramePixels {
    channels: u32,
    bits: u32,
    samples_u16: bool,
    data: Vec<u8>,
}

/// One prepared frame: pixels plus its presentation delay.
struct PreparedFrame {
    pixels: FramePixels,
    delay: Duration,
}

/// Geometry and frame set of one encode request.
struct EncodeRequest {
    /// Canvas width in pixels.
    width: u32,
    /// Canvas height in pixels.
    height: u32,
    /// Frames in presentation order.
    frames: Vec<PreparedFrame>,
    /// Whether the animation basic-info header is emitted (real durations).
    animated: bool,
    /// How often the animation loops (0 = infinite; stills use 0).
    num_loops: u32,
}

/// Prepares the single frame of a still image.
fn still_request(image: &DynamicImage, bit_depth: Option<JxlBitDepthChoice>) -> EncodeRequest {
    EncodeRequest {
        width: image.width(),
        height: image.height(),
        frames: vec![PreparedFrame {
            pixels: normalize_still(image, bit_depth),
            delay: Duration::ZERO,
        }],
        animated: false,
        num_loops: 0,
    }
}

/// Prepares the full-canvas frames of an animation (real durations).
fn animated_request(animation: &AnimationData) -> EncodeRequest {
    EncodeRequest {
        width: animation.width,
        height: animation.height,
        frames: animation
            .frames
            .iter()
            .map(|frame| PreparedFrame {
                pixels: normalize_still(&DynamicImage::ImageRgba8(frame.buffer.clone()), None),
                delay: frame.delay,
            })
            .collect(),
        animated: true,
        num_loops: match animation.loop_count {
            LoopCount::Infinite => 0,
            LoopCount::Finite(loops) => loops.get(),
        },
    }
}

/// Channel count of a color type (1/2 gray(+alpha), 3/4 rgb(+alpha)).
fn channel_count(color: ColorType) -> u32 {
    match color {
        ColorType::L8 | ColorType::L16 => 1,
        ColorType::La8 | ColorType::La16 => 2,
        ColorType::Rgb8 | ColorType::Rgb16 | ColorType::Rgb32F => 3,
        _ => 4,
    }
}

/// Converts a still image into the normalized pixel buffer, honoring the
/// `bit_depth` option (`None` follows the input depth). Bit depth
/// conversions are delegated to the `image` crate (full-range scaling).
fn normalize_still(image: &DynamicImage, bit_depth: Option<JxlBitDepthChoice>) -> FramePixels {
    let input_u16 = matches!(
        image.color(),
        ColorType::L16 | ColorType::La16 | ColorType::Rgb16 | ColorType::Rgba16
    );
    let use16 = match bit_depth {
        Some(JxlBitDepthChoice::Sixteen) => true,
        Some(JxlBitDepthChoice::Eight) => false,
        None => input_u16,
    };
    let channels = channel_count(image.color());
    let data = match (use16, channels) {
        (false, 1) => image.to_luma8().into_raw(),
        (false, 2) => image.to_luma_alpha8().into_raw(),
        (false, 3) => image.to_rgb8().into_raw(),
        (false, _) => image.to_rgba8().into_raw(),
        (true, 1) => u16_ne_bytes(&image.to_luma16().into_raw()),
        (true, 2) => u16_ne_bytes(&image.to_luma_alpha16().into_raw()),
        (true, 3) => u16_ne_bytes(&image.to_rgb16().into_raw()),
        (true, _) => u16_ne_bytes(&image.to_rgba16().into_raw()),
    };
    FramePixels {
        channels,
        bits: if use16 { 16 } else { 8 },
        samples_u16: use16,
        data,
    }
}

/// Interleaves `u16` samples into native-endian bytes.
fn u16_ne_bytes(samples: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_ne_bytes());
    }
    bytes
}

impl ImageEncoder for JxlEncoder {
    fn format(&self) -> ImageFormat {
        ImageFormat::Jxl
    }

    fn extension(&self) -> &'static str {
        "jxl"
    }

    fn describe(&self) -> String {
        let (major, minor, patch) = ffi::version();
        format!(
            "still-image/animated JPEG XL encoder (libjxl {major}.{minor}.{patch}, vendored via jpegxl-src)"
        )
    }

    fn describe_options(&self) -> String {
        let (major, minor, patch) = ffi::version();
        format!(
            "Using \"libjxl\" ({major}.{minor}.{patch}, vendored via jpegxl-src) with options: {}",
            self.options
        )
    }

    fn supports_animation(&self) -> bool {
        true
    }

    fn encode_still_image(&self, image: &DynamicImage) -> Result<Vec<u8>, Error> {
        let request = still_request(image, self.options.bit_depth);
        encode_jxl(&self.options, &request, None, None)
    }

    fn encode_still_image_with_metadata(
        &self,
        image: &DynamicImage,
        metadata: &crate::metadata::ImageMetadata,
    ) -> Result<Vec<u8>, Error> {
        let request = still_request(image, self.options.bit_depth);
        encode_jxl(
            &self.options,
            &request,
            metadata.icc.as_deref(),
            metadata.exif.as_deref(),
        )
    }

    fn encode(&self, input: &SourceImage) -> Result<Vec<u8>, Error> {
        // WS4 contract (oxipng pattern): resolve the EXIF policy carried by
        // the options; the resolved payload is embedded as an `Exif` box.
        #[cfg(feature = "exif")]
        let exif_payload = crate::metadata::resolve(&self.options.exif_policy, &input.metadata);
        #[cfg(not(feature = "exif"))]
        let exif_payload: Option<Vec<u8>> = None;
        let icc = input.metadata.icc.clone();

        let request = match &input.content {
            ImageContent::Still(image) => still_request(image, self.options.bit_depth),
            ImageContent::Animated(animation) => {
                if animation.frames.is_empty() {
                    return Err(Error::from_string(
                        "Animation does not contain any frames".to_string(),
                    ));
                }
                animated_request(animation)
            }
        };
        encode_jxl(
            &self.options,
            &request,
            icc.as_deref(),
            exif_payload.as_deref(),
        )
    }
}

/// Resolves the effective Butteraugli distance: an explicit `distance`
/// overrides `quality` (mapped through libjxl's own quality function);
/// otherwise the libjxl default of 1.0 applies.
fn effective_distance(options: &JxlOptions) -> f32 {
    options.distance.unwrap_or_else(|| {
        options.quality.map_or(1.0, |quality| unsafe {
            ffi::JxlEncoderDistanceFromQuality(quality)
        })
    })
}

/// Validates the advanced `--setting` passthrough entries against the
/// transcribed [`FrameSettingId`] table (case-insensitive).
fn resolve_advanced(advanced: &[(String, i64)]) -> Result<Vec<(FrameSettingId, i64)>, Error> {
    let mut resolved = Vec::with_capacity(advanced.len());
    for (name, value) in advanced {
        match FrameSettingId::from_cli_name(name) {
            Some(id) => resolved.push((id, *value)),
            None => {
                let available = FrameSettingId::ALL
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(Error::from_string(format!(
                    "unknown jxl --setting id {name:?}; available ids: {available}"
                )));
            }
        }
    }
    Ok(resolved)
}

/// Converts a `Duration` delay into libjxl millisecond ticks (truncating;
/// clamped to `u32::MAX` ticks).
fn delay_to_ticks(delay: Duration) -> u32 {
    u32::try_from(delay.as_millis()).unwrap_or(u32::MAX)
}

/// Core encoding routine shared by the still and animated paths.
fn encode_jxl(
    options: &JxlOptions,
    request: &EncodeRequest,
    icc: Option<&[u8]>,
    exif_payload: Option<&[u8]>,
) -> Result<Vec<u8>, Error> {
    let EncodeRequest {
        width,
        height,
        frames,
        animated,
        num_loops,
    } = request;
    let (width, height) = (*width, *height);
    let advanced = resolve_advanced(&options.advanced)?;
    let first = frames.first().ok_or_else(|| {
        Error::from_string("jxl encoding requires at least one frame".to_string())
    })?;
    let gray = first.pixels.channels <= 2;
    let alpha = matches!(first.pixels.channels, 2 | 4);
    let bits = first.pixels.bits;

    let encoder = ffi::Encoder::new().ok_or_else(|| {
        Error::from_string("libjxl: could not allocate encoder instance".to_string())
    })?;
    let enc = encoder.raw();

    // -- basic info --------------------------------------------------------
    let info = JxlBasicInfo {
        have_container: JXL_FALSE,
        xsize: width,
        ysize: height,
        bits_per_sample: bits,
        exponent_bits_per_sample: 0,
        intensity_target: 255.0,
        min_nits: 0.0,
        relative_to_max_display: JXL_FALSE,
        linear_below: 0.0,
        uses_original_profile: into_jxl_bool(options.original_profile || options.lossless),
        have_preview: JXL_FALSE,
        have_animation: into_jxl_bool(*animated),
        orientation: 1, // pixel buffers are upright; orientation metadata is handled by the EXIF policy
        num_color_channels: if gray { 1 } else { 3 },
        num_extra_channels: u32::from(alpha),
        alpha_bits: if alpha { bits } else { 0 },
        alpha_exponent_bits: 0,
        alpha_premultiplied: JXL_FALSE,
        preview: ffi::JxlPreviewHeader { xsize: 0, ysize: 0 },
        animation: JxlAnimationHeader {
            tps_numerator: 1000,
            tps_denominator: 1,
            num_loops: *num_loops,
            have_timecodes: JXL_FALSE,
        },
        intrinsic_xsize: width,
        intrinsic_ysize: height,
        padding: [0; 100],
    };
    check(
        unsafe { ffi::JxlEncoderSetBasicInfo(enc, &info) },
        "SetBasicInfo",
    )?;

    // -- color encoding ------------------------------------------------------
    let color_choice = options.color_encoding.unwrap_or_default();
    match color_choice {
        JxlColorEncodingChoice::IccPassthrough => match icc {
            Some(icc) => check(
                unsafe { ffi::JxlEncoderSetICCProfile(enc, icc.as_ptr(), icc.len()) },
                "SetICCProfile",
            )?,
            None => {
                println!(
                    "Warning: jxl --color-encoding icc-passthrough requested but the input has no ICC profile; falling back to sRGB"
                );
                set_enum_color_encoding(enc, JxlColorEncodingChoice::Srgb, gray)?;
            }
        },
        choice => {
            if choice.is_luma() && !gray {
                return Err(Error::from_string(
                    "jxl --color-encoding srgb-luma/linear-srgb-luma requires a grayscale input image"
                        .to_string(),
                ));
            }
            set_enum_color_encoding(enc, choice, gray)?;
        }
    }

    // -- container & metadata boxes ------------------------------------------
    if options.container {
        check(
            unsafe { ffi::JxlEncoderUseContainer(enc, JXL_TRUE) },
            "UseContainer",
        )?;
    }
    if let Some(payload) = exif_payload {
        // Adding a box requires the container + UseBoxes; libjxl enables the
        // container itself when UseBoxes is set.
        check(unsafe { ffi::JxlEncoderUseBoxes(enc) }, "UseBoxes")?;
        // WS4: libjxl expects a 4-byte big-endian tiff-header offset prefix.
        let boxed = exif_for_jxl(payload);
        let box_type: ffi::JxlBoxType = [b'E' as _, b'x' as _, b'i' as _, b'f' as _];
        check(
            unsafe {
                ffi::JxlEncoderAddBox(
                    enc,
                    &box_type,
                    boxed.as_ptr(),
                    boxed.len(),
                    JXL_TRUE, // brob-compressed box
                )
            },
            "AddBox(Exif)",
        )?;
    }

    // -- frames ---------------------------------------------------------------
    let distance = effective_distance(options);
    for frame in frames {
        // SAFETY: `enc` is a live encoder owned by the RAII guard; every
        // pointer passed refers to a valid object for the duration of the
        // respective call, and `frame.data` is verified to have exactly the
        // length implied by the dimensions and pixel format.
        unsafe {
            let frame_settings = ffi::JxlEncoderFrameSettingsCreate(enc, core::ptr::null());
            if frame_settings.is_null() {
                return Err(Error::from_string(
                    "libjxl: could not allocate frame settings".to_string(),
                ));
            }
            check(
                ffi::JxlEncoderFrameSettingsSetOption(
                    frame_settings,
                    FrameSettingId::Effort,
                    i64::from(options.effort),
                ),
                "SetOption(Effort)",
            )?;
            check(
                ffi::JxlEncoderFrameSettingsSetOption(
                    frame_settings,
                    FrameSettingId::DecodingSpeed,
                    i64::from(options.decoding_speed),
                ),
                "SetOption(DecodingSpeed)",
            )?;
            for (id, value) in &advanced {
                check(
                    ffi::JxlEncoderFrameSettingsSetOption(frame_settings, *id, *value),
                    "SetOption(passthrough)",
                )?;
            }
            if options.lossless {
                check(
                    ffi::JxlEncoderSetFrameLossless(frame_settings, JXL_TRUE),
                    "SetFrameLossless",
                )?;
            } else {
                check(
                    ffi::JxlEncoderSetFrameDistance(frame_settings, distance),
                    "SetFrameDistance",
                )?;
            }
            if *animated {
                let mut header = JxlFrameHeader {
                    duration: delay_to_ticks(frame.delay),
                    timecode: 0,
                    name_length: 0,
                    is_last: JXL_FALSE, // ignored by the encoder; CloseInput marks the last frame
                    layer_info: ffi::JxlLayerInfo {
                        have_crop: JXL_FALSE,
                        crop_x0: 0,
                        crop_y0: 0,
                        xsize: width,
                        ysize: height,
                        blend_info: ffi::JxlBlendInfo {
                            blendmode: 0, // JXL_BLEND_REPLACE: full-canvas frames
                            source: 0,
                            alpha: 0,
                            clamp: JXL_FALSE,
                        },
                        save_as_reference: 0,
                    },
                };
                ffi::JxlEncoderInitFrameHeader(&mut header);
                header.duration = delay_to_ticks(frame.delay);
                check(
                    ffi::JxlEncoderSetFrameHeader(frame_settings, &header),
                    "SetFrameHeader",
                )?;
            }
            let pixel_format = JxlPixelFormat {
                num_channels: frame.pixels.channels,
                data_type: if frame.pixels.samples_u16 {
                    JxlDataType::JXL_TYPE_UINT16
                } else {
                    JxlDataType::JXL_TYPE_UINT8
                },
                endianness: JxlEndianness::JXL_NATIVE_ENDIAN,
                align: 0,
            };
            let sample_size = if frame.pixels.samples_u16 { 2 } else { 1 };
            let expected =
                frame.pixels.channels as usize * sample_size * width as usize * height as usize;
            if frame.pixels.data.len() != expected {
                return Err(Error::from_string(format!(
                    "libjxl: frame buffer size mismatch ({} bytes, expected {expected})",
                    frame.pixels.data.len()
                )));
            }
            check(
                ffi::JxlEncoderAddImageFrame(
                    frame_settings,
                    &pixel_format,
                    frame.pixels.data.as_ptr().cast(),
                    frame.pixels.data.len(),
                ),
                "AddImageFrame",
            )?;
        }
    }

    // -- finalize ----------------------------------------------------------------
    // Closes frames and (when boxes were used) boxes in one call, as
    // required before the final ProcessOutput.
    unsafe { ffi::JxlEncoderCloseInput(enc) };

    collect_output(enc)
}

/// Sets an enum-based color encoding via the libjxl helper constructors.
fn set_enum_color_encoding(
    enc: *mut ffi::JxlEncoder,
    choice: JxlColorEncodingChoice,
    gray: bool,
) -> Result<(), Error> {
    let mut encoding = JxlColorEncoding {
        color_space: if gray || choice.is_luma() {
            JxlColorSpace::JXL_COLOR_SPACE_GRAY
        } else {
            JxlColorSpace::JXL_COLOR_SPACE_RGB
        },
        white_point: 0,
        white_point_xy: [0.0; 2],
        primaries: 0,
        primaries_red_xy: [0.0; 2],
        primaries_green_xy: [0.0; 2],
        primaries_blue_xy: [0.0; 2],
        transfer_function: 0,
        gamma: 0.0,
        rendering_intent: 0,
    };
    let is_gray = into_jxl_bool(gray || choice.is_luma());
    // SAFETY: `enc` is a live encoder and `encoding` outlives both calls.
    unsafe {
        match choice {
            JxlColorEncodingChoice::LinearSrgb | JxlColorEncodingChoice::LinearSrgbLuma => {
                ffi::JxlColorEncodingSetToLinearSRGB(&mut encoding, is_gray);
            }
            _ => {
                ffi::JxlColorEncodingSetToSRGB(&mut encoding, is_gray);
            }
        }
        check(
            ffi::JxlEncoderSetColorEncoding(enc, &encoding),
            "SetColorEncoding",
        )
    }
}

fn into_jxl_bool(value: bool) -> ffi::JXL_BOOL {
    if value { JXL_TRUE } else { JXL_FALSE }
}

/// Grows the output buffer until the encoder reports success (64 KiB
/// chunks; `ProcessOutput` requires at least 32 bytes of headroom per call).
fn collect_output(enc: *mut ffi::JxlEncoder) -> Result<Vec<u8>, Error> {
    const CHUNK: usize = 64 * 1024;
    let mut output: Vec<u8> = Vec::with_capacity(CHUNK);
    loop {
        if output.capacity() - output.len() < 32 {
            output.reserve(CHUNK);
        }
        // SAFETY: the pointer range covers the reserved spare capacity of
        // `output`; `set_len` afterwards only marks the bytes libjxl wrote.
        unsafe {
            let mut avail_out = output.capacity() - output.len();
            let mut next_out = output.as_mut_ptr().add(output.len());
            let status = ffi::JxlEncoderProcessOutput(enc, &mut next_out, &mut avail_out);
            let written = (output.capacity() - output.len()) - avail_out;
            output.set_len(output.len() + written);
            match status {
                JXL_ENC_SUCCESS => return Ok(output),
                JXL_ENC_NEED_MORE_OUTPUT => continue,
                JXL_ENC_ERROR => {
                    let code = ffi::JxlEncoderGetError(enc);
                    return Err(Error::from_string(format!(
                        "libjxl encoding failed (JxlEncoderError {code:#x})"
                    )));
                }
                other => {
                    return Err(Error::from_string(format!(
                        "libjxl encoding failed with unexpected status {other}"
                    )));
                }
            }
        }
    }
}

/// Validates a libjxl status and maps errors onto [`Error`].
fn check(status: ffi::JxlEncoderStatus, what: &str) -> Result<(), Error> {
    if status == JXL_ENC_SUCCESS {
        Ok(())
    } else {
        Err(Error::from_string(format!(
            "libjxl {what} failed (status {status})"
        )))
    }
}
