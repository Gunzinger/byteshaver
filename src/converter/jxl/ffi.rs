//! Thin manual FFI bindings to the libjxl 0.12 encoder C API (plan WS2,
//! Option A: in-tree declarations instead of bindgen/jpegxl-rs).
//!
//! Every signature and constant here is transcribed verbatim from the
//! vendored libjxl headers that `jpegxl-src` 0.12 builds
//! (`libjxl/lib/include/jxl/{encode,codestream_header,color_encoding,types}.h`).
//! All `unsafe` is confined to this module; the rest of the crate uses the
//! small safe wrappers defined at the bottom.
//!
//! Notable deviations from the plan draft (verified against the 0.12 header):
//! - the output-buffer API is [`JxlEncoderProcessOutput`]
//!   (`JxlEncoderProcessOne`/`JxlEncoderFlushOutputBuffer` do not exist),
//! - frame durations are set through [`JxlFrameHeader::duration`] +
//!   `JxlEncoderSetFrameHeader` (there is no `JxlEncoderSetFrameDuration`),
//! - [`JxlEncoderAddBox`] takes the encoder (not frame settings) and a
//!   `JXL_BOOL compress_box` parameter,
//! - [`JxlEncoderCreate`] takes a `const JxlMemoryManager*` (nullable), not a
//!   version pointer.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use core::ffi::{c_char, c_int, c_void};

/// `JXL_BOOL` (`types.h`): a portable bool replacement, actually `int`.
pub type JXL_BOOL = c_int;
/// `JXL_TRUE` (`types.h`).
pub const JXL_TRUE: JXL_BOOL = 1;
/// `JXL_FALSE` (`types.h`).
pub const JXL_FALSE: JXL_BOOL = 0;

/// `JxlEncoderStatus` (`encode.h`).
pub type JxlEncoderStatus = c_int;
/// `JXL_ENC_SUCCESS` (`encode.h`).
pub const JXL_ENC_SUCCESS: JxlEncoderStatus = 0;
/// `JXL_ENC_ERROR` (`encode.h`).
pub const JXL_ENC_ERROR: JxlEncoderStatus = 1;
/// `JXL_ENC_NEED_MORE_OUTPUT` (`encode.h`).
pub const JXL_ENC_NEED_MORE_OUTPUT: JxlEncoderStatus = 2;

/// `JxlDataType` (`types.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum JxlDataType {
    /// 32-bit single-precision float, nominal range 0.0-1.0.
    JXL_TYPE_FLOAT = 0,
    /// `uint8_t` samples.
    JXL_TYPE_UINT8 = 2,
    /// `uint16_t` samples.
    JXL_TYPE_UINT16 = 3,
    /// 16-bit IEEE 754 half-precision float.
    JXL_TYPE_FLOAT16 = 5,
}

/// `JxlEndianness` (`types.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum JxlEndianness {
    /// Use the endianness of the system.
    JXL_NATIVE_ENDIAN = 0,
    /// Force little endian.
    JXL_LITTLE_ENDIAN = 1,
    /// Force big endian.
    JXL_BIG_ENDIAN = 2,
}

/// `JxlPixelFormat` (`types.h`): interleaved per-pixel channel layout of the
/// input buffer.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct JxlPixelFormat {
    /// Amount of channels available in a pixel buffer (1/2/3/4).
    pub num_channels: u32,
    /// Data type of each channel.
    pub data_type: JxlDataType,
    /// Endianness for multi-byte sample types.
    pub endianness: JxlEndianness,
    /// Align scanlines to a multiple of `align` bytes, or 0 for none.
    pub align: usize,
}

/// `JxlAnimationHeader` (`codestream_header.h`).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct JxlAnimationHeader {
    /// Numerator of ticks per second of a single animation frame time unit.
    pub tps_numerator: u32,
    /// Denominator of ticks per second of a single animation frame time unit.
    pub tps_denominator: u32,
    /// Amount of animation loops, or 0 to repeat infinitely.
    pub num_loops: u32,
    /// Whether animation time codes are present at animation frames.
    pub have_timecodes: JXL_BOOL,
}

/// `JxlPreviewHeader` (`codestream_header.h`).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct JxlPreviewHeader {
    /// Preview width in pixels.
    pub xsize: u32,
    /// Preview height in pixels.
    pub ysize: u32,
}

/// `JxlBasicInfo` (`codestream_header.h`), transcribed field-by-field.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct JxlBasicInfo {
    /// Whether the codestream is embedded in the container format.
    pub have_container: JXL_BOOL,
    /// Width of the image in pixels, before applying orientation.
    pub xsize: u32,
    /// Height of the image in pixels, before applying orientation.
    pub ysize: u32,
    /// Original image color channel bit depth.
    pub bits_per_sample: u32,
    /// Floating point exponent bits, or 0 for unsigned integer samples.
    pub exponent_bits_per_sample: u32,
    /// Upper bound on the intensity level present in the image in nits.
    pub intensity_target: f32,
    /// Lower bound on the intensity level present in the image in nits.
    pub min_nits: f32,
    /// Whether the intensity level is relative to max display brightness.
    pub relative_to_max_display: JXL_BOOL,
    /// Lower bound on intensity relative to display brightness.
    pub linear_below: f32,
    /// Whether to use the original (profile-defined) color transform.
    pub uses_original_profile: JXL_BOOL,
    /// Indicates a preview image exists near the beginning of the codestream.
    pub have_preview: JXL_BOOL,
    /// Indicates animation frames exist in the codestream.
    pub have_animation: JXL_BOOL,
    /// Image orientation, value 1-8 (JEITA CP-3451C/Exif values).
    pub orientation: c_int,
    /// Number of color channels encoded in the image (1 grayscale, 3 color).
    pub num_color_channels: u32,
    /// Number of additional image channels (including main alpha).
    pub num_extra_channels: u32,
    /// Bit depth of the encoded alpha channel, or 0 if there is none.
    pub alpha_bits: u32,
    /// Alpha channel floating point exponent bits, or 0 for unsigned integer.
    pub alpha_exponent_bits: u32,
    /// Whether the alpha channel is premultiplied.
    pub alpha_premultiplied: JXL_BOOL,
    /// Dimensions of encoded preview image, only used if `have_preview`.
    pub preview: JxlPreviewHeader,
    /// Animation header, only used if `have_animation`.
    pub animation: JxlAnimationHeader,
    /// Intrinsic width of the image (recommended display width).
    pub intrinsic_xsize: u32,
    /// Intrinsic height of the image (recommended display height).
    pub intrinsic_ysize: u32,
    /// Padding for forwards-compatibility, in case more fields are exposed.
    pub padding: [u8; 100],
}

/// `JxlBlendInfo` (`codestream_header.h`).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct JxlBlendInfo {
    /// Blend mode (`JxlBlendMode`).
    pub blendmode: c_int,
    /// Reference frame ID to use as the 'bottom' layer (0-3).
    pub source: u32,
    /// Which extra channel to use as the 'alpha' channel for blend modes.
    pub alpha: u32,
    /// Clamp values to [0,1] for the purpose of blending.
    pub clamp: JXL_BOOL,
}

/// `JxlLayerInfo` (`codestream_header.h`).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct JxlLayerInfo {
    /// Whether cropping is applied for this frame.
    pub have_crop: JXL_BOOL,
    /// Horizontal offset of the frame (can be negative).
    pub crop_x0: i32,
    /// Vertical offset of the frame (can be negative).
    pub crop_y0: i32,
    /// Width of the frame (number of columns).
    pub xsize: u32,
    /// Height of the frame (number of rows).
    pub ysize: u32,
    /// The blending info for the color channels.
    pub blend_info: JxlBlendInfo,
    /// After blending, save the frame as reference frame with this ID (0-3).
    pub save_as_reference: u32,
}

/// `JxlFrameHeader` (`codestream_header.h`): header of one displayed frame.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct JxlFrameHeader {
    /// How long to wait after rendering, in ticks (see `JxlAnimationHeader`).
    pub duration: u32,
    /// SMPTE timecode of the current frame in form 0xHHMMSSFF, or 0.
    pub timecode: u32,
    /// Length of the frame name in bytes, or 0 if no name (encoder: ignored).
    pub name_length: u32,
    /// Indicates this is the last animation frame (encoder: ignored, use
    /// `JxlEncoderCloseFrames` instead).
    pub is_last: JXL_BOOL,
    /// Information about the layer in case of no coalescing.
    pub layer_info: JxlLayerInfo,
}

/// `JxlColorSpace` (`color_encoding.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum JxlColorSpace {
    /// Trichromatic (RGB) color space.
    JXL_COLOR_SPACE_RGB = 0,
    /// Grayscale color space.
    JXL_COLOR_SPACE_GRAY = 1,
    /// XYB color space.
    JXL_COLOR_SPACE_XYB = 2,
    /// Unknown or invalid color space.
    JXL_COLOR_SPACE_UNKNOWN = 999,
}

/// `JxlColorEncoding` (`color_encoding.h`): an enum-based color encoding.
///
/// C enum fields are transcribed as `c_int`-sized placeholders; use the
/// `JxlColorEncodingSetToSRGB`/`JxlColorEncodingSetToLinearSRGB` helpers to
/// fill the struct instead of hand-assigning enum values.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct JxlColorEncoding {
    /// Color space of the image data.
    pub color_space: JxlColorSpace,
    /// Built-in white point (`JxlWhitePoint`).
    pub white_point: c_int,
    /// Numerical whitepoint values in CIE xy space.
    pub white_point_xy: [f64; 2],
    /// Built-in RGB primaries (`JxlPrimaries`).
    pub primaries: c_int,
    /// Numerical red primary values in CIE xy space.
    pub primaries_red_xy: [f64; 2],
    /// Numerical green primary values in CIE xy space.
    pub primaries_green_xy: [f64; 2],
    /// Numerical blue primary values in CIE xy space.
    pub primaries_blue_xy: [f64; 2],
    /// Transfer function if `have_gamma` is 0 (`JxlTransferFunction`).
    pub transfer_function: c_int,
    /// Gamma value used when transfer_function is `GAMMA`.
    pub gamma: f64,
    /// Rendering intent defined for the color profile.
    pub rendering_intent: c_int,
}

unsafe extern "C" {
    /// `encode.h`: packed libjxl version, `(major << 16) | (minor << 8) | patch`.
    pub fn JxlEncoderVersion() -> u32;

    /// `encode.h`: creates an encoder instance; `memory_manager` may be null.
    pub fn JxlEncoderCreate(memory_manager: *const c_void) -> *mut JxlEncoder;

    /// `encode.h`: deinitializes and frees an encoder instance.
    pub fn JxlEncoderDestroy(enc: *mut JxlEncoder);

    /// `encode.h`: sets the global metadata of the image to encode.
    pub fn JxlEncoderSetBasicInfo(
        enc: *mut JxlEncoder,
        info: *const JxlBasicInfo,
    ) -> JxlEncoderStatus;

    /// `encode.h`: sets the original color encoding (enum-based form).
    pub fn JxlEncoderSetColorEncoding(
        enc: *mut JxlEncoder,
        color: *const JxlColorEncoding,
    ) -> JxlEncoderStatus;

    /// `encode.h`: sets the original color encoding as ICC binary data.
    pub fn JxlEncoderSetICCProfile(
        enc: *mut JxlEncoder,
        icc_profile: *const u8,
        size: usize,
    ) -> JxlEncoderStatus;

    /// `encode.h`: creates a new set of frame options (may return null on OOM).
    pub fn JxlEncoderFrameSettingsCreate(
        enc: *mut JxlEncoder,
        source: *const JxlEncoderFrameSettings,
    ) -> *mut JxlEncoderFrameSettings;

    /// `encode.h`: sets a frame-specific option of integer type.
    pub fn JxlEncoderFrameSettingsSetOption(
        frame_settings: *mut JxlEncoderFrameSettings,
        option: FrameSettingId,
        value: i64,
    ) -> JxlEncoderStatus;

    /// `encode.h`: sets a frame-specific option of float type.
    pub fn JxlEncoderFrameSettingsSetFloatOption(
        frame_settings: *mut JxlEncoderFrameSettings,
        option: FrameSettingId,
        value: f32,
    ) -> JxlEncoderStatus;

    /// `encode.h`: enables lossless encoding for frames using these settings.
    pub fn JxlEncoderSetFrameLossless(
        frame_settings: *mut JxlEncoderFrameSettings,
        lossless: JXL_BOOL,
    ) -> JxlEncoderStatus;

    /// `encode.h`: sets the target max Butteraugli distance (0.0 .. 25.0).
    pub fn JxlEncoderSetFrameDistance(
        frame_settings: *mut JxlEncoderFrameSettings,
        distance: f32,
    ) -> JxlEncoderStatus;

    /// `encode.h`: sets the bit depth of the input buffer.
    pub fn JxlEncoderSetFrameBitDepth(
        frame_settings: *mut JxlEncoderFrameSettings,
        bit_depth: *const JxlBitDepth,
    ) -> JxlEncoderStatus;

    /// `encode.h`: sets the frame header (including animation duration) for
    /// the frames encoded with these settings afterwards.
    pub fn JxlEncoderSetFrameHeader(
        frame_settings: *mut JxlEncoderFrameSettings,
        frame_header: *const JxlFrameHeader,
    ) -> JxlEncoderStatus;

    /// `encode.h`: adds one frame of pixel data.
    pub fn JxlEncoderAddImageFrame(
        frame_settings: *const JxlEncoderFrameSettings,
        pixel_format: *const JxlPixelFormat,
        buffer: *const c_void,
        size: usize,
    ) -> JxlEncoderStatus;

    /// `encode.h`: forces the box-based container format (BMFF).
    pub fn JxlEncoderUseContainer(
        enc: *mut JxlEncoder,
        use_container: JXL_BOOL,
    ) -> JxlEncoderStatus;

    /// `encode.h`: indicates the intention to add metadata boxes; requires
    /// `JxlEncoderCloseBoxes` (or `CloseInput`) before finishing.
    pub fn JxlEncoderUseBoxes(enc: *mut JxlEncoder) -> JxlEncoderStatus;

    /// `encode.h`: adds one metadata box (e.g. `Exif`, `"xml "`).
    pub fn JxlEncoderAddBox(
        enc: *mut JxlEncoder,
        box_type: *const JxlBoxType,
        contents: *const u8,
        size: usize,
        compress_box: JXL_BOOL,
    ) -> JxlEncoderStatus;

    /// `encode.h`: declares that no further frames will be added.
    pub fn JxlEncoderCloseFrames(enc: *mut JxlEncoder);

    /// `encode.h`: declares that no further boxes will be added.
    pub fn JxlEncoderCloseBoxes(enc: *mut JxlEncoder);

    /// `encode.h`: closes frame and box input at once.
    pub fn JxlEncoderCloseInput(enc: *mut JxlEncoder);

    /// `encode.h`: processes available output; `*avail_out` must be >= 32.
    pub fn JxlEncoderProcessOutput(
        enc: *mut JxlEncoder,
        next_out: *mut *mut u8,
        avail_out: *mut usize,
    ) -> JxlEncoderStatus;

    /// `encode.h`: returns the (last) error code after `JXL_ENC_ERROR`.
    pub fn JxlEncoderGetError(enc: *mut JxlEncoder) -> c_int;

    /// `encode.h`: maps a JPEG-style quality factor (0-100) to a distance.
    pub fn JxlEncoderDistanceFromQuality(quality: f32) -> f32;

    /// `encode.h`: initializes basic info to defaults (8-bit RGB, no alpha).
    pub fn JxlEncoderInitBasicInfo(info: *mut JxlBasicInfo);

    /// `encode.h`: initializes a frame header to defaults.
    pub fn JxlEncoderInitFrameHeader(frame_header: *mut JxlFrameHeader);

    /// `encode.h`: sets a color encoding to be (non-linear) sRGB.
    pub fn JxlColorEncodingSetToSRGB(color_encoding: *mut JxlColorEncoding, is_gray: JXL_BOOL);

    /// `encode.h`: sets a color encoding to be linear sRGB.
    pub fn JxlColorEncodingSetToLinearSRGB(
        color_encoding: *mut JxlColorEncoding,
        is_gray: JXL_BOOL,
    );
}

/// Opaque encoder state (`encode.h`).
#[repr(C)]
pub struct JxlEncoder {
    _private: [u8; 0],
}

/// Opaque per-frame encoder options (`encode.h`).
#[repr(C)]
pub struct JxlEncoderFrameSettings {
    _private: [u8; 0],
}

/// `JxlBoxType` (`types.h`): 4-character box type identifier.
pub type JxlBoxType = [c_char; 4];

/// `JxlBitDepthType` (`types.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum JxlBitDepthType {
    /// Input uses the full range of the pixel format data type (default).
    JXL_BIT_DEPTH_FROM_PIXEL_FORMAT = 0,
    /// Input uses the range defined by `bits_per_sample` of the basic info.
    JXL_BIT_DEPTH_FROM_CODESTREAM = 1,
}

/// `JxlBitDepth` (`types.h`).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct JxlBitDepth {
    /// Bit depth setting.
    pub type_: JxlBitDepthType,
}

/// `JxlEncoderFrameSettingId` (`encode.h`), transcribed verbatim.
///
/// Used for the `--setting ID=VALUE` passthrough; the discriminants are the
/// numeric values of the C enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum FrameSettingId {
    /// Encoder effort/speed level 1-10 (default 7, "squirrel").
    Effort = 0,
    /// Decoding speed tier 0-4 (default 0).
    DecodingSpeed = 1,
    /// Downsampling factor (-1 default, 1/2/4/8).
    Resampling = 2,
    /// Downsampling factor for extra channels (-1 default, 1/2/4/8).
    ExtraChannelResampling = 3,
    /// The frame is already downsampled (0 disable, 1 enable).
    AlreadyDownsampled = 4,
    /// Photographic film noise strength (e.g. 100, 3200; default 0).
    PhotonNoise = 5,
    /// Adaptive noise generation (-1 default, 0/1).
    Noise = 6,
    /// Dots generation (-1 default, 0/1).
    Dots = 7,
    /// Patches generation (-1 default, 0/1).
    Patches = 8,
    /// Edge preserving filter level (-1 default, 0-3).
    Epf = 9,
    /// Gaborish filter (-1 default, 0/1).
    Gaborish = 10,
    /// Modular encoding (-1 default, 0 VarDCT, 1 modular).
    Modular = 11,
    /// Preserving color of invisible pixels (-1 default, 0/1).
    KeepInvisible = 12,
    /// Region storage order (-1 default, 0 scanline, 1 center-first).
    GroupOrder = 13,
    /// Horizontal center position for center-first group order.
    GroupOrderCenterX = 14,
    /// Vertical center position for center-first group order.
    GroupOrderCenterY = 15,
    /// Progressive encoding for modular mode (-1 default, 0/1).
    Responsive = 16,
    /// Progressive mode for AC coefficients via spectral progression.
    ProgressiveAc = 17,
    /// Progressive mode for AC coefficients via LSB quantization.
    QProgressiveAc = 18,
    /// Progressive mode using lower-resolution DC images (0/1/2).
    ProgressiveDc = 19,
    /// Global channel palette percentage (0-100, -1 default).
    ChannelColorsGlobalPercent = 20,
    /// Local (per-group) channel palette percentage (0-100, -1 default).
    ChannelColorsGroupPercent = 21,
    /// Color palette if amount of colors <= this (-1 default).
    PaletteColors = 22,
    /// Delta palette (-1 default, 0/1).
    LossyPalette = 23,
    /// Internal color transform (-1 default, 0 XYB, 1 none, 2 YCbCr).
    ColorTransform = 24,
    /// Reversible color transform for modular (0-41, -1 default).
    ModularColorSpace = 25,
    /// Modular group size (-1 default, 0=128, 1=256, 2=512, 3=1024).
    ModularGroupSize = 26,
    /// Modular predictor (-1 default, 0-15).
    ModularPredictor = 27,
    /// Fraction of pixels used to learn MA trees as a percentage.
    ModularMaTreeLearningPercent = 28,
    /// Number of extra (previous-channel) MA tree properties (0-11, -1 default).
    ModularNbPrevChannels = 29,
    /// Chroma-from-luma for lossless JPEG recompression (-1 default, 0/1).
    JpegReconCfl = 30,
    /// Prepare the frame for indexing in the frame index box (0/1).
    IndexBox = 31,
    /// Brotli encode effort for JPEG recompression and brob boxes (-1, 0-11).
    BrotliEffort = 32,
    /// Brotli compression of boxes derived from JPEG frames (-1 default, 0/1).
    JpegCompressBoxes = 33,
    /// Input buffering mode when using chunked image frames (-1, 0-3).
    Buffering = 34,
    /// Keep/discard Exif boxes derived from JPEG frames (-1 default, 0/1).
    JpegKeepExif = 35,
    /// Keep/discard XMP boxes derived from JPEG frames (-1 default, 0/1).
    JpegKeepXmp = 36,
    /// Keep/discard JUMBF boxes derived from JPEG frames (-1 default, 0/1).
    JpegKeepJumbf = 37,
    /// Full-image heuristics (0 disabled, 1 enabled default).
    UseFullImageHeuristics = 38,
    /// Disable perceptual optimizations (0 enabled default, 1 disabled).
    DisablePerceptualHeuristics = 39,
    /// Output ordering/memory trade-off mode (-1 default, 0/1/2).
    OutputMode = 40,
}

impl FrameSettingId {
    /// CLI names of all ids (snake case of the C enum names without the
    /// `JXL_ENC_FRAME_SETTING_` prefix), in stable order.
    pub const ALL: &'static [(&'static str, FrameSettingId)] = &[
        ("effort", FrameSettingId::Effort),
        ("decoding_speed", FrameSettingId::DecodingSpeed),
        ("resampling", FrameSettingId::Resampling),
        (
            "extra_channel_resampling",
            FrameSettingId::ExtraChannelResampling,
        ),
        ("already_downsampled", FrameSettingId::AlreadyDownsampled),
        ("photon_noise", FrameSettingId::PhotonNoise),
        ("noise", FrameSettingId::Noise),
        ("dots", FrameSettingId::Dots),
        ("patches", FrameSettingId::Patches),
        ("epf", FrameSettingId::Epf),
        ("gaborish", FrameSettingId::Gaborish),
        ("modular", FrameSettingId::Modular),
        ("keep_invisible", FrameSettingId::KeepInvisible),
        ("group_order", FrameSettingId::GroupOrder),
        ("group_order_center_x", FrameSettingId::GroupOrderCenterX),
        ("group_order_center_y", FrameSettingId::GroupOrderCenterY),
        ("responsive", FrameSettingId::Responsive),
        ("progressive_ac", FrameSettingId::ProgressiveAc),
        ("qprogressive_ac", FrameSettingId::QProgressiveAc),
        ("progressive_dc", FrameSettingId::ProgressiveDc),
        (
            "channel_colors_global_percent",
            FrameSettingId::ChannelColorsGlobalPercent,
        ),
        (
            "channel_colors_group_percent",
            FrameSettingId::ChannelColorsGroupPercent,
        ),
        ("palette_colors", FrameSettingId::PaletteColors),
        ("lossy_palette", FrameSettingId::LossyPalette),
        ("color_transform", FrameSettingId::ColorTransform),
        ("modular_color_space", FrameSettingId::ModularColorSpace),
        ("modular_group_size", FrameSettingId::ModularGroupSize),
        ("modular_predictor", FrameSettingId::ModularPredictor),
        (
            "modular_ma_tree_learning_percent",
            FrameSettingId::ModularMaTreeLearningPercent,
        ),
        (
            "modular_nb_prev_channels",
            FrameSettingId::ModularNbPrevChannels,
        ),
        ("jpeg_recon_cfl", FrameSettingId::JpegReconCfl),
        ("index_box", FrameSettingId::IndexBox),
        ("brotli_effort", FrameSettingId::BrotliEffort),
        ("jpeg_compress_boxes", FrameSettingId::JpegCompressBoxes),
        ("buffering", FrameSettingId::Buffering),
        ("jpeg_keep_exif", FrameSettingId::JpegKeepExif),
        ("jpeg_keep_xmp", FrameSettingId::JpegKeepXmp),
        ("jpeg_keep_jumbf", FrameSettingId::JpegKeepJumbf),
        (
            "use_full_image_heuristics",
            FrameSettingId::UseFullImageHeuristics,
        ),
        (
            "disable_perceptual_heuristics",
            FrameSettingId::DisablePerceptualHeuristics,
        ),
        ("output_mode", FrameSettingId::OutputMode),
    ];

    /// Resolves a CLI name (case-insensitive) onto a setting id.
    #[must_use]
    pub fn from_cli_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|&&(known, _)| known.eq_ignore_ascii_case(name))
            .map(|&(_, id)| id)
    }
}

/// libjxl version as `(major, minor, patch)`; `JxlEncoderVersion` packs it
/// as `major * 1_000_000 + minor * 1_000 + patch`.
#[must_use]
pub fn version() -> (u32, u32, u32) {
    let v = unsafe { JxlEncoderVersion() };
    (v / 1_000_000, (v / 1_000) % 1_000, v % 1_000)
}

/// RAII guard owning a `JxlEncoder` instance.
pub(crate) struct Encoder(*mut JxlEncoder);

impl Encoder {
    /// Creates an encoder with the default memory manager, or `None` on OOM.
    pub(crate) fn new() -> Option<Self> {
        let raw = unsafe { JxlEncoderCreate(core::ptr::null()) };
        (!raw.is_null()).then(|| Self(raw))
    }

    /// Raw pointer for the FFI calls.
    pub(crate) fn raw(&self) -> *mut JxlEncoder {
        self.0
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { JxlEncoderDestroy(self.0) };
    }
}
