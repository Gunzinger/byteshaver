//! Per-encoder options editors (plan WS8 §5.4, plan 13 §2B/§3).
//!
//! Every encoder's option surface is described as a **pure, egui-free**
//! [`Vec<OptionRow>`] schema (label, control spec, group, CLI flag,
//! trade-off icon, tooltip) and rendered by a single generic two-column
//! [`egui::Grid`] renderer with group sub-headers. Rows in the
//! [`GROUP_ADVANCED`] group (JXL `--setting` list, oxipng multi-selects)
//! are rendered inside a collapsed "Advanced" sub-header.
//!
//! The module also holds the pure plumbing the rest of the GUI needs: the
//! capability-name ↔ encoder-kind mapping, the per-encoder CLI defaults
//! (identical to the subcommand defaults) and the [`option_diff`] helper
//! powering the "↺ defaults" button. Schema and diff are unit-testable
//! without a display server.
//!
//! All state mutation happens directly on the `EncoderConfig` stored in
//! the app settings (immediate mode); no rendering state is kept here.
//! The EXIF policy fields of the oxipng/jxl options are **not** edited
//! here — they are injected from the global policy when building the job
//! spec (mirroring the CLI, see `app::App::build_job_spec`).

use std::time::Duration;

use byteshaver::config::{
    AlphaColorMode, ApngOptions, AvifOptions, BitDepth, ColorModel, CompressionType, EncoderConfig,
    FilterType, GifOptions, JxlBitDepthChoice, JxlColorEncodingChoice, JxlOptions, OxipngFilter,
    OxipngInterlace, OxipngLevel, OxipngOptions, OxipngReduction, OxipngStrip, PngOptions,
    WebpAnimOptions, WebpOptions,
};
use byteshaver::job::Capabilities;

use crate::app::JxlAdvancedDraft;

// ---- pure schema types ----------------------------------------------------

/// Type-erased logical value of one option field. Produced by a row's
/// getter, consumed by its setter; also the comparison payload of
/// [`option_diff`].
#[derive(Clone, Debug, PartialEq)]
pub enum OptionValue {
    /// Checkbox state.
    Flag(bool),
    /// Numeric value (all numeric fields are normalized to `f64` for the
    /// drag/slider widget; setters convert back to the field type).
    Number(f64),
    /// Index into a combo control's variant table.
    Index(usize),
    /// Membership flags of a multi-select's entries (in table order).
    Flags(Vec<bool>),
    /// Verbatim text of a custom editor (full equality via formatting).
    Text(String),
    /// Non-comparable content.
    Opaque,
}

impl OptionValue {
    fn flag(self) -> bool {
        match self {
            OptionValue::Flag(flag) => flag,
            other => panic!("expected a checkbox value, got {other:?}"),
        }
    }

    fn number(self) -> f64 {
        match self {
            OptionValue::Number(number) => number,
            other => panic!("expected a numeric value, got {other:?}"),
        }
    }

    fn index(self) -> usize {
        match self {
            OptionValue::Index(index) => index,
            other => panic!("expected a combo value, got {other:?}"),
        }
    }
}

/// Read access to one option field of an [`EncoderConfig`] (the config is
/// always of the variant the schema was built for — getters panic on a
/// mismatched variant, which is a programmer error, never user input).
pub type Getter = fn(&EncoderConfig) -> OptionValue;

/// Write access to one option field of an [`EncoderConfig`] (same variant
/// pairing contract as [`Getter`]).
pub type Setter = fn(&mut EncoderConfig, OptionValue);

/// One checkbox entry of a multi-select (membership in a `Vec` field).
#[derive(Clone, Copy, Debug)]
pub struct MultiEntry {
    /// Display name of the entry (also the checkbox label).
    pub name: &'static str,
    /// Whether the entry is currently a member of the list field.
    pub is_set: fn(&EncoderConfig) -> bool,
    /// Inserts/removes the entry from the list field.
    pub set: fn(&mut EncoderConfig, bool),
}

/// Hand-rendered control that a schema cannot express generically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomKind {
    /// The JXL advanced `--setting ID=VALUE` list editor (needs the
    /// `JxlAdvancedDraft` text-input state).
    JxlAdvanced,
}

/// Trade-off icon of an option row. Rule (plan 13 §1): an icon must encode
/// information, never decorate — bookkeeping options carry none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeOffIcon {
    /// `✦` — quality direction (higher = better).
    Quality,
    /// `▤` — estimated output size direction.
    Size,
    /// `⚡` — encode time direction ("slower" tooltips).
    Speed,
    /// No icon (pure bookkeeping option).
    None,
}

impl TradeOffIcon {
    fn glyph(self) -> &'static str {
        match self {
            TradeOffIcon::Quality => "✦",
            TradeOffIcon::Size => "▤",
            TradeOffIcon::Speed => "⚡",
            TradeOffIcon::None => "",
        }
    }
}

/// The editable control of an option row (pure data; the generic grid
/// renderer in this module consumes it).
#[derive(Clone, Copy, Debug)]
pub enum ControlSpec {
    /// Informational text row for encoders without options.
    Note(&'static str),
    /// Checkbox over a `bool` field.
    Checkbox {
        /// Reads the current state.
        get: Getter,
        /// Writes the new state.
        set: Setter,
    },
    /// Slider over a numeric field (typically a 0–100 quality).
    Slider {
        /// Inclusive `(min, max)` range.
        range: (f64, f64),
        /// Reads the current value (substituting a default when unset).
        get: Getter,
        /// Writes the new value.
        set: Setter,
    },
    /// Drag value over a numeric field.
    Drag {
        /// Inclusive `(min, max)` range.
        range: (f64, f64),
        /// Unit suffix rendered after the value (e.g. `"s"`).
        suffix: &'static str,
        /// Integer fields step by 1 and never show decimals.
        integer: bool,
        /// Reads the current value (substituting a default when unset).
        get: Getter,
        /// Writes the new value.
        set: Setter,
    },
    /// Dropdown over a fixed variant table (index-based; for `Option<T>`
    /// fields index 0 is the "auto" entry mapped to `None`).
    Combo {
        /// Display names of the variants, in index order.
        variants: &'static [&'static str],
        /// Reads the current index.
        get: Getter,
        /// Writes the variant at the given index.
        set: Setter,
    },
    /// Membership checkboxes over a `Vec` field (rendered in one wrapped
    /// horizontal row).
    MultiSelect {
        /// The fixed entry table.
        entries: &'static [MultiEntry],
    },
    /// Hand-rendered editor.
    Custom(CustomKind),
}

/// One option row of an encoder's schema (pure data).
#[derive(Clone, Copy, Debug)]
pub struct OptionRow {
    /// Option label shown in the left (right-aligned) grid column.
    pub label: &'static str,
    /// Group sub-header this row belongs under; [`GROUP_ADVANCED`] rows
    /// move into the collapsed "Advanced" sub-header.
    pub group: &'static str,
    /// CLI flag that mirrors this option (shown in the hover tooltip;
    /// empty only on [`ControlSpec::Note`] rows).
    pub cli_flag: &'static str,
    /// Trade-off icon (informational only, see [`TradeOffIcon`]).
    pub icon: TradeOffIcon,
    /// Extended explanation shown on hover (with the CLI flag).
    pub tooltip: &'static str,
    /// The editable control.
    pub control: ControlSpec,
    /// Extra condition for the control being enabled (e.g. zopfli
    /// iterations require zopfli).
    pub enabled_when: Option<fn(&EncoderConfig) -> bool>,
}

/// Group name that routes rows into the collapsed "Advanced" sub-header.
const GROUP_ADVANCED: &str = "Advanced";
const GROUP_QUALITY: &str = "Quality";
const GROUP_SPEED: &str = "Speed ⚡";
const GROUP_COLOR: &str = "Color";
const GROUP_COMPRESSION: &str = "Compression";
const GROUP_CONTAINER: &str = "Container";
const GROUP_LEVEL: &str = "Level";
const GROUP_SIZE: &str = "Size ▤";
const GROUP_OUTPUT: &str = "Output";
const GROUP_KEYFRAMES: &str = "Keyframes";
const GROUP_METHOD: &str = "Method";
const GROUP_PALETTE: &str = "Palette";

/// Registry-order (capability) names of all encoders, in the same order as
/// `capabilities().encoders` (test fixture for the availability checks and
/// default comparisons).
#[cfg(test)]
pub const ENCODER_NAMES: [&str; 10] = [
    "webp",
    "webp-image",
    "avif",
    "png",
    "jpeg",
    "jxl",
    "oxipng",
    "webp-anim",
    "apng",
    "gif",
];

/// The capability name (`CLI subcommand`) of an encoder config.
#[must_use]
pub fn encoder_kind_name(encoder: &EncoderConfig) -> &'static str {
    match encoder {
        EncoderConfig::Webp(_) => "webp",
        EncoderConfig::WebpImage => "webp-image",
        EncoderConfig::Avif(_) => "avif",
        EncoderConfig::Png(_) => "png",
        EncoderConfig::Jpeg => "jpeg",
        EncoderConfig::Jxl(_) => "jxl",
        EncoderConfig::Oxipng(_) => "oxipng",
        EncoderConfig::WebpAnim(_) => "webp-anim",
        EncoderConfig::Apng(_) => "apng",
        EncoderConfig::Gif(_) => "gif",
    }
}

/// Default encoder configuration for a capability name, matching the CLI
/// subcommand defaults (`EncoderConfig::from_args` with all flags unset;
/// for `gif` that means the crate-default palette speed, like the CLI).
#[must_use]
pub fn default_encoder_config(name: &str) -> Option<EncoderConfig> {
    match name {
        "webp" => Some(EncoderConfig::Webp(WebpOptions::default())),
        "webp-image" => Some(EncoderConfig::WebpImage),
        "avif" => Some(EncoderConfig::Avif(AvifOptions::default())),
        "png" => Some(EncoderConfig::Png(PngOptions::default())),
        "jpeg" => Some(EncoderConfig::Jpeg),
        "jxl" => Some(EncoderConfig::Jxl(JxlOptions::default())),
        "oxipng" => Some(EncoderConfig::Oxipng(OxipngOptions::default())),
        "webp-anim" => Some(EncoderConfig::WebpAnim(WebpAnimOptions::default())),
        "apng" => Some(EncoderConfig::Apng(ApngOptions::default())),
        "gif" => Some(EncoderConfig::Gif(GifOptions::default())),
        _ => None,
    }
}

/// Whether the named encoder is compiled into this build (from
/// [`capabilities()`][byteshaver::job::capabilities]).
#[must_use]
pub fn encoder_enabled(caps: &Capabilities, name: &str) -> bool {
    caps.encoders
        .iter()
        .find(|info| info.name == name)
        .is_some_and(|info| info.enabled)
}

/// Why the named encoder is unavailable (`None` when available or unknown).
#[must_use]
pub fn encoder_disabled_reason(caps: &Capabilities, name: &str) -> Option<&'static str> {
    caps.encoders
        .iter()
        .find(|info| info.name == name)
        .and_then(|info| info.disabled_reason)
}

// ---- option schemas --------------------------------------------------------

/// The option schema of an encoder config (dispatch by variant; the rows
/// must only be applied to a config of the same variant). This is also the
/// reflection surface plan 14 needs ("what options exist on this encoder").
#[must_use]
pub fn encoder_rows(encoder: &EncoderConfig) -> Vec<OptionRow> {
    match encoder {
        EncoderConfig::Webp(_) => webp_rows(),
        EncoderConfig::WebpImage => vec![note_row(
            "The image-crate lossless webp encoder has no options.",
        )],
        EncoderConfig::Avif(_) => avif_rows(),
        EncoderConfig::Png(_) => png_rows(),
        EncoderConfig::Jpeg => vec![note_row("The mozjpeg-based jpeg encoder has no options.")],
        EncoderConfig::Jxl(_) => jxl_rows(),
        EncoderConfig::Oxipng(_) => oxipng_rows(),
        EncoderConfig::WebpAnim(_) => webp_anim_rows(),
        EncoderConfig::Apng(_) => apng_rows(),
        EncoderConfig::Gif(_) => gif_rows(),
    }
}

/// Builds an option row with no extra enable-condition.
fn opt(
    label: &'static str,
    group: &'static str,
    cli_flag: &'static str,
    icon: TradeOffIcon,
    tooltip: &'static str,
    control: ControlSpec,
) -> OptionRow {
    OptionRow {
        label,
        group,
        cli_flag,
        icon,
        tooltip,
        control,
        enabled_when: None,
    }
}

/// Builds an informational note row (encoders without options; rendered
/// without a group header).
fn note_row(text: &'static str) -> OptionRow {
    opt(
        "",
        "",
        "",
        TradeOffIcon::None,
        text,
        ControlSpec::Note(text),
    )
}

// per-variant accessors (rows are always paired with their variant)

fn webp(config: &EncoderConfig) -> &WebpOptions {
    match config {
        EncoderConfig::Webp(options) => options,
        other => unreachable!("webp schema applied to {other:?}"),
    }
}

fn webp_mut(config: &mut EncoderConfig) -> &mut WebpOptions {
    match config {
        EncoderConfig::Webp(options) => options,
        other => unreachable!("webp schema applied to {other:?}"),
    }
}

fn avif(config: &EncoderConfig) -> &AvifOptions {
    match config {
        EncoderConfig::Avif(options) => options,
        other => unreachable!("avif schema applied to {other:?}"),
    }
}

fn avif_mut(config: &mut EncoderConfig) -> &mut AvifOptions {
    match config {
        EncoderConfig::Avif(options) => options,
        other => unreachable!("avif schema applied to {other:?}"),
    }
}

fn png(config: &EncoderConfig) -> &PngOptions {
    match config {
        EncoderConfig::Png(options) => options,
        other => unreachable!("png schema applied to {other:?}"),
    }
}

fn png_mut(config: &mut EncoderConfig) -> &mut PngOptions {
    match config {
        EncoderConfig::Png(options) => options,
        other => unreachable!("png schema applied to {other:?}"),
    }
}

fn apng(config: &EncoderConfig) -> &ApngOptions {
    match config {
        EncoderConfig::Apng(options) => options,
        other => unreachable!("apng schema applied to {other:?}"),
    }
}

fn apng_mut(config: &mut EncoderConfig) -> &mut ApngOptions {
    match config {
        EncoderConfig::Apng(options) => options,
        other => unreachable!("apng schema applied to {other:?}"),
    }
}

fn jxl(config: &EncoderConfig) -> &JxlOptions {
    match config {
        EncoderConfig::Jxl(options) => options,
        other => unreachable!("jxl schema applied to {other:?}"),
    }
}

fn jxl_mut(config: &mut EncoderConfig) -> &mut JxlOptions {
    match config {
        EncoderConfig::Jxl(options) => options,
        other => unreachable!("jxl schema applied to {other:?}"),
    }
}

fn oxipng(config: &EncoderConfig) -> &OxipngOptions {
    match config {
        EncoderConfig::Oxipng(options) => options,
        other => unreachable!("oxipng schema applied to {other:?}"),
    }
}

fn oxipng_mut(config: &mut EncoderConfig) -> &mut OxipngOptions {
    match config {
        EncoderConfig::Oxipng(options) => options,
        other => unreachable!("oxipng schema applied to {other:?}"),
    }
}

fn webp_anim(config: &EncoderConfig) -> &WebpAnimOptions {
    match config {
        EncoderConfig::WebpAnim(options) => options,
        other => unreachable!("webp-anim schema applied to {other:?}"),
    }
}

fn webp_anim_mut(config: &mut EncoderConfig) -> &mut WebpAnimOptions {
    match config {
        EncoderConfig::WebpAnim(options) => options,
        other => unreachable!("webp-anim schema applied to {other:?}"),
    }
}

fn gif(config: &EncoderConfig) -> &GifOptions {
    match config {
        EncoderConfig::Gif(options) => options,
        other => unreachable!("gif schema applied to {other:?}"),
    }
}

fn gif_mut(config: &mut EncoderConfig) -> &mut GifOptions {
    match config {
        EncoderConfig::Gif(options) => options,
        other => unreachable!("gif schema applied to {other:?}"),
    }
}

fn webp_rows() -> Vec<OptionRow> {
    vec![
        opt(
            "quality",
            GROUP_QUALITY,
            "-q, --quality",
            TradeOffIcon::Quality,
            "target quality (0–100, lower is worse but results in smaller files)",
            ControlSpec::Slider {
                range: (0.0, 100.0),
                get: |config| OptionValue::Number(f64::from(webp(config).quality)),
                set: |config, value| webp_mut(config).quality = value.number() as f32,
            },
        ),
        opt(
            "lossless",
            GROUP_QUALITY,
            "--lossless",
            TradeOffIcon::Size,
            "lossless encoding mode",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(webp(config).lossless),
                set: |config, value| webp_mut(config).lossless = value.flag(),
            },
        ),
    ]
}

fn avif_rows() -> Vec<OptionRow> {
    vec![
        opt(
            "quality",
            GROUP_QUALITY,
            "-q, --quality",
            TradeOffIcon::Quality,
            "target quality (0–100, lower is worse but results in smaller files)",
            ControlSpec::Slider {
                range: (0.0, 100.0),
                get: |config| OptionValue::Number(f64::from(avif(config).quality)),
                set: |config, value| avif_mut(config).quality = value.number() as f32,
            },
        ),
        opt(
            "alpha quality",
            GROUP_QUALITY,
            "-a, --alpha-quality",
            TradeOffIcon::Quality,
            "target alpha quality (0–100, lower is worse)",
            ControlSpec::Slider {
                range: (0.0, 100.0),
                get: |config| OptionValue::Number(f64::from(avif(config).alpha_quality)),
                set: |config, value| avif_mut(config).alpha_quality = value.number() as f32,
            },
        ),
        opt(
            "speed (1–10)",
            GROUP_SPEED,
            "-s, --speed",
            TradeOffIcon::Speed,
            "1 = slowest/best, 10 = fastest",
            ControlSpec::Drag {
                range: (1.0, 10.0),
                suffix: "",
                integer: true,
                get: |config| OptionValue::Number(f64::from(avif(config).speed)),
                set: |config, value| avif_mut(config).speed = value.number() as u8,
            },
        ),
        opt(
            "bit depth",
            GROUP_COLOR,
            "--bit-depth",
            TradeOffIcon::None,
            "internal bit depth of the generated avif file (auto = from the input)",
            ControlSpec::Combo {
                variants: &["auto", "8", "10"],
                get: |config| {
                    OptionValue::Index(match avif(config).bit_depth {
                        None => 0,
                        Some(BitDepth::Eight) => 1,
                        Some(BitDepth::Ten) => 2,
                        Some(BitDepth::Auto) => 0,
                    })
                },
                set: |config, value| {
                    avif_mut(config).bit_depth = match value.index() {
                        1 => Some(BitDepth::Eight),
                        2 => Some(BitDepth::Ten),
                        _ => None,
                    };
                },
            },
        ),
        opt(
            "color model",
            GROUP_COLOR,
            "--color-model",
            TradeOffIcon::None,
            "YCbCr is smaller for photographic content; RGB avoids chroma subsampling artifacts",
            ControlSpec::Combo {
                variants: &["auto", "YCbCr", "RGB"],
                get: |config| {
                    OptionValue::Index(match avif(config).color_model {
                        None => 0,
                        Some(ColorModel::YCbCr) => 1,
                        Some(ColorModel::RGB) => 2,
                    })
                },
                set: |config, value| {
                    avif_mut(config).color_model = match value.index() {
                        1 => Some(ColorModel::YCbCr),
                        2 => Some(ColorModel::RGB),
                        _ => None,
                    };
                },
            },
        ),
        opt(
            "alpha color mode",
            GROUP_COLOR,
            "--alpha-color-mode",
            TradeOffIcon::None,
            "irrelevant for images without transparency",
            ControlSpec::Combo {
                variants: &[
                    "auto",
                    "unassociated dirty",
                    "unassociated clean",
                    "premultiplied",
                ],
                get: |config| {
                    OptionValue::Index(match avif(config).alpha_color_mode {
                        None => 0,
                        Some(AlphaColorMode::UnassociatedDirty) => 1,
                        Some(AlphaColorMode::UnassociatedClean) => 2,
                        Some(AlphaColorMode::Premultiplied) => 3,
                    })
                },
                set: |config, value| {
                    avif_mut(config).alpha_color_mode = match value.index() {
                        1 => Some(AlphaColorMode::UnassociatedDirty),
                        2 => Some(AlphaColorMode::UnassociatedClean),
                        3 => Some(AlphaColorMode::Premultiplied),
                        _ => None,
                    };
                },
            },
        ),
    ]
}

/// Read/write accessor pair of one option field (schema helper type).
type Access = (Getter, Setter);

/// The shared compression/filter rows (png and apng encode the same fields,
/// just on different option structs).
fn compression_filter_rows(
    group: &'static str,
    compression: Access,
    filter: Access,
) -> Vec<OptionRow> {
    let (compression_get, compression_set) = compression;
    let (filter_get, filter_set) = filter;
    vec![
        opt(
            "compression",
            group,
            "--compression-type",
            TradeOffIcon::Size,
            "default = balanced, best = high compression (slow), fast",
            ControlSpec::Combo {
                variants: &["auto (default)", "best", "fast"],
                get: compression_get,
                set: compression_set,
            },
        ),
        opt(
            "filter",
            group,
            "--filter-type",
            TradeOffIcon::None,
            "adaptive picks a filter per scanline (encoder default)",
            ControlSpec::Combo {
                variants: &["auto (adaptive)", "none", "sub", "up", "avg", "paeth"],
                get: filter_get,
                set: filter_set,
            },
        ),
    ]
}

fn png_rows() -> Vec<OptionRow> {
    compression_filter_rows(
        GROUP_COMPRESSION,
        (
            |config| {
                OptionValue::Index(match png(config).compression_type {
                    None => 0,
                    Some(CompressionType::Default) => 0,
                    Some(CompressionType::Best) => 1,
                    Some(CompressionType::Fast) => 2,
                })
            },
            |config, value| {
                png_mut(config).compression_type = match value.index() {
                    1 => Some(CompressionType::Best),
                    2 => Some(CompressionType::Fast),
                    _ => None,
                };
            },
        ),
        (
            |config| {
                OptionValue::Index(match png(config).filter_type {
                    None => 0,
                    Some(FilterType::Adaptive) => 0,
                    Some(FilterType::NoFilter) => 1,
                    Some(FilterType::Sub) => 2,
                    Some(FilterType::Up) => 3,
                    Some(FilterType::Avg) => 4,
                    Some(FilterType::Paeth) => 5,
                })
            },
            |config, value| {
                png_mut(config).filter_type = match value.index() {
                    1 => Some(FilterType::NoFilter),
                    2 => Some(FilterType::Sub),
                    3 => Some(FilterType::Up),
                    4 => Some(FilterType::Avg),
                    5 => Some(FilterType::Paeth),
                    _ => None,
                };
            },
        ),
    )
}

fn apng_rows() -> Vec<OptionRow> {
    compression_filter_rows(
        GROUP_COMPRESSION,
        (
            |config| {
                OptionValue::Index(match apng(config).compression_type {
                    None => 0,
                    Some(CompressionType::Default) => 0,
                    Some(CompressionType::Best) => 1,
                    Some(CompressionType::Fast) => 2,
                })
            },
            |config, value| {
                apng_mut(config).compression_type = match value.index() {
                    1 => Some(CompressionType::Best),
                    2 => Some(CompressionType::Fast),
                    _ => None,
                };
            },
        ),
        (
            |config| {
                OptionValue::Index(match apng(config).filter_type {
                    None => 0,
                    Some(FilterType::Adaptive) => 0,
                    Some(FilterType::NoFilter) => 1,
                    Some(FilterType::Sub) => 2,
                    Some(FilterType::Up) => 3,
                    Some(FilterType::Avg) => 4,
                    Some(FilterType::Paeth) => 5,
                })
            },
            |config, value| {
                apng_mut(config).filter_type = match value.index() {
                    1 => Some(FilterType::NoFilter),
                    2 => Some(FilterType::Sub),
                    3 => Some(FilterType::Up),
                    4 => Some(FilterType::Avg),
                    5 => Some(FilterType::Paeth),
                    _ => None,
                };
            },
        ),
    )
}

fn jxl_rows() -> Vec<OptionRow> {
    vec![
        opt(
            "lossless",
            GROUP_QUALITY,
            "--lossless",
            TradeOffIcon::Size,
            "true lossless mode; overrides quality/distance, implies original profile",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(jxl(config).lossless),
                set: |config, value| jxl_mut(config).lossless = value.flag(),
            },
        ),
        opt(
            "quality (0–100)",
            GROUP_QUALITY,
            "-q, --quality",
            TradeOffIcon::Quality,
            "JPEG-style quality (higher = better); unset = follow --distance",
            ControlSpec::Drag {
                range: (0.0, 100.0),
                suffix: "",
                integer: true,
                get: |config| OptionValue::Number(jxl(config).quality.map_or(0.0, f64::from)),
                set: |config, value| jxl_mut(config).quality = Some(value.number() as f32),
            },
        ),
        opt(
            "distance (0–25)",
            GROUP_QUALITY,
            "--distance",
            TradeOffIcon::Quality,
            "maximum Butteraugli distance (0 = lossless, 1.0 = visually lossless); unset = libjxl default 1.0",
            ControlSpec::Drag {
                range: (0.0, 25.0),
                suffix: "",
                integer: false,
                get: |config| OptionValue::Number(jxl(config).distance.map_or(1.0, f64::from)),
                set: |config, value| jxl_mut(config).distance = Some(value.number() as f32),
            },
        ),
        opt(
            "effort (1–10)",
            GROUP_SPEED,
            "-e, --effort",
            TradeOffIcon::Speed,
            "encoding effort 1 (fastest) – 10 (slowest/best)",
            ControlSpec::Drag {
                range: (1.0, 10.0),
                suffix: "",
                integer: true,
                get: |config| OptionValue::Number(f64::from(jxl(config).effort)),
                set: |config, value| jxl_mut(config).effort = value.number() as u8,
            },
        ),
        opt(
            "decoding speed (0–4)",
            GROUP_SPEED,
            "--decoding-speed",
            TradeOffIcon::Speed,
            "target decode speed tier (higher = faster decode, larger file)",
            ControlSpec::Drag {
                range: (0.0, 4.0),
                suffix: "",
                integer: true,
                get: |config| OptionValue::Number(f64::from(jxl(config).decoding_speed)),
                set: |config, value| jxl_mut(config).decoding_speed = value.number() as u8,
            },
        ),
        opt(
            "intensity target (nits)",
            GROUP_COLOR,
            "--intensity-target",
            TradeOffIcon::None,
            "photometric target intensity in nits (HDR); unset = libjxl default 255",
            ControlSpec::Drag {
                range: (0.0, 10_000.0),
                suffix: "",
                integer: true,
                get: |config| {
                    OptionValue::Number(jxl(config).intensity_target.map_or(0.0, f64::from))
                },
                set: |config, value| {
                    jxl_mut(config).intensity_target = Some(value.number() as f32);
                },
            },
        ),
        opt(
            "bit depth",
            GROUP_COLOR,
            "--bit-depth",
            TradeOffIcon::None,
            "force output bit depth (auto = follow the input)",
            ControlSpec::Combo {
                variants: &["auto", "8", "16"],
                get: |config| {
                    OptionValue::Index(match jxl(config).bit_depth {
                        None => 0,
                        Some(JxlBitDepthChoice::Eight) => 1,
                        Some(JxlBitDepthChoice::Sixteen) => 2,
                    })
                },
                set: |config, value| {
                    jxl_mut(config).bit_depth = match value.index() {
                        1 => Some(JxlBitDepthChoice::Eight),
                        2 => Some(JxlBitDepthChoice::Sixteen),
                        _ => None,
                    };
                },
            },
        ),
        opt(
            "color encoding",
            GROUP_COLOR,
            "--color-encoding",
            TradeOffIcon::None,
            "color encoding of the output (auto = non-linear sRGB)",
            ControlSpec::Combo {
                variants: &[
                    "auto (sRGB)",
                    "sRGB",
                    "linear sRGB",
                    "sRGB (luma)",
                    "linear sRGB (luma)",
                    "ICC passthrough",
                ],
                get: |config| {
                    OptionValue::Index(match jxl(config).color_encoding {
                        None => 0,
                        Some(JxlColorEncodingChoice::Srgb) => 1,
                        Some(JxlColorEncodingChoice::LinearSrgb) => 2,
                        Some(JxlColorEncodingChoice::SrgbLuma) => 3,
                        Some(JxlColorEncodingChoice::LinearSrgbLuma) => 4,
                        Some(JxlColorEncodingChoice::IccPassthrough) => 5,
                    })
                },
                set: |config, value| {
                    jxl_mut(config).color_encoding = match value.index() {
                        1 => Some(JxlColorEncodingChoice::Srgb),
                        2 => Some(JxlColorEncodingChoice::LinearSrgb),
                        3 => Some(JxlColorEncodingChoice::SrgbLuma),
                        4 => Some(JxlColorEncodingChoice::LinearSrgbLuma),
                        5 => Some(JxlColorEncodingChoice::IccPassthrough),
                        _ => None,
                    };
                },
            },
        ),
        opt(
            "force container",
            GROUP_CONTAINER,
            "--container",
            TradeOffIcon::None,
            "force the ISOBMFF container (auto-enabled for EXIF boxes)",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(jxl(config).container),
                set: |config, value| jxl_mut(config).container = value.flag(),
            },
        ),
        opt(
            "original profile",
            GROUP_CONTAINER,
            "--original-profile",
            TradeOffIcon::None,
            "keep the original color profile (skip the XYB transform)",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(jxl(config).original_profile),
                set: |config, value| jxl_mut(config).original_profile = value.flag(),
            },
        ),
        opt(
            "advanced settings",
            GROUP_ADVANCED,
            "--setting ID=VALUE",
            TradeOffIcon::None,
            "libjxl frame-setting passthrough (ids are resolved case-insensitively at run time)",
            ControlSpec::Custom(CustomKind::JxlAdvanced),
        ),
    ]
}

const OXIPNG_LEVELS: &[&str] = &["0 (fastest)", "1", "2 (default)", "3", "4", "5", "6", "max"];

fn oxipng_level_index(level: OxipngLevel) -> usize {
    match level {
        OxipngLevel::Zero => 0,
        OxipngLevel::One => 1,
        OxipngLevel::Two => 2,
        OxipngLevel::Three => 3,
        OxipngLevel::Four => 4,
        OxipngLevel::Five => 5,
        OxipngLevel::Six => 6,
        OxipngLevel::Max => 7,
    }
}

fn oxipng_level_from_index(index: usize) -> OxipngLevel {
    match index {
        0 => OxipngLevel::Zero,
        1 => OxipngLevel::One,
        2 => OxipngLevel::Two,
        3 => OxipngLevel::Three,
        4 => OxipngLevel::Four,
        5 => OxipngLevel::Five,
        6 => OxipngLevel::Six,
        _ => OxipngLevel::Max,
    }
}

const OXIPNG_FILTERS: &[MultiEntry] = &[
    MultiEntry {
        name: "none",
        is_set: |config| oxipng(config).filters.contains(&OxipngFilter::None),
        set: |config, on| oxipng_list_member(config, on, OxipngFilter::None, |o| &mut o.filters),
    },
    MultiEntry {
        name: "sub",
        is_set: |config| oxipng(config).filters.contains(&OxipngFilter::Sub),
        set: |config, on| oxipng_list_member(config, on, OxipngFilter::Sub, |o| &mut o.filters),
    },
    MultiEntry {
        name: "up",
        is_set: |config| oxipng(config).filters.contains(&OxipngFilter::Up),
        set: |config, on| oxipng_list_member(config, on, OxipngFilter::Up, |o| &mut o.filters),
    },
    MultiEntry {
        name: "avg",
        is_set: |config| oxipng(config).filters.contains(&OxipngFilter::Avg),
        set: |config, on| oxipng_list_member(config, on, OxipngFilter::Avg, |o| &mut o.filters),
    },
    MultiEntry {
        name: "paeth",
        is_set: |config| oxipng(config).filters.contains(&OxipngFilter::Paeth),
        set: |config, on| oxipng_list_member(config, on, OxipngFilter::Paeth, |o| &mut o.filters),
    },
    MultiEntry {
        name: "min-sum",
        is_set: |config| oxipng(config).filters.contains(&OxipngFilter::MinSum),
        set: |config, on| oxipng_list_member(config, on, OxipngFilter::MinSum, |o| &mut o.filters),
    },
    MultiEntry {
        name: "entropy",
        is_set: |config| oxipng(config).filters.contains(&OxipngFilter::Entropy),
        set: |config, on| oxipng_list_member(config, on, OxipngFilter::Entropy, |o| &mut o.filters),
    },
    MultiEntry {
        name: "bigrams",
        is_set: |config| oxipng(config).filters.contains(&OxipngFilter::Bigrams),
        set: |config, on| oxipng_list_member(config, on, OxipngFilter::Bigrams, |o| &mut o.filters),
    },
    MultiEntry {
        name: "big-entropy",
        is_set: |config| oxipng(config).filters.contains(&OxipngFilter::BigEnt),
        set: |config, on| oxipng_list_member(config, on, OxipngFilter::BigEnt, |o| &mut o.filters),
    },
    MultiEntry {
        name: "brute",
        is_set: |config| oxipng(config).filters.contains(&OxipngFilter::Brute),
        set: |config, on| oxipng_list_member(config, on, OxipngFilter::Brute, |o| &mut o.filters),
    },
];

const OXIPNG_REDUCTIONS: &[MultiEntry] = &[
    MultiEntry {
        name: "bit depth",
        is_set: |config| {
            oxipng(config)
                .no_reduction
                .contains(&OxipngReduction::BitDepth)
        },
        set: |config, on| {
            oxipng_list_member(config, on, OxipngReduction::BitDepth, |o| {
                &mut o.no_reduction
            })
        },
    },
    MultiEntry {
        name: "color type",
        is_set: |config| {
            oxipng(config)
                .no_reduction
                .contains(&OxipngReduction::ColorType)
        },
        set: |config, on| {
            oxipng_list_member(config, on, OxipngReduction::ColorType, |o| {
                &mut o.no_reduction
            })
        },
    },
    MultiEntry {
        name: "palette",
        is_set: |config| {
            oxipng(config)
                .no_reduction
                .contains(&OxipngReduction::Palette)
        },
        set: |config, on| {
            oxipng_list_member(config, on, OxipngReduction::Palette, |o| {
                &mut o.no_reduction
            })
        },
    },
    MultiEntry {
        name: "grayscale",
        is_set: |config| {
            oxipng(config)
                .no_reduction
                .contains(&OxipngReduction::Grayscale)
        },
        set: |config, on| {
            oxipng_list_member(config, on, OxipngReduction::Grayscale, |o| {
                &mut o.no_reduction
            })
        },
    },
];

/// Inserts/removes a member from one of oxipng's `Vec` list fields.
fn oxipng_list_member<T: PartialEq>(
    config: &mut EncoderConfig,
    on: bool,
    member: T,
    list: impl Fn(&mut OxipngOptions) -> &mut Vec<T>,
) {
    let options = oxipng_mut(config);
    let list = list(options);
    if on {
        if !list.contains(&member) {
            list.push(member);
        }
    } else {
        list.retain(|existing| *existing != member);
    }
}

fn oxipng_rows() -> Vec<OptionRow> {
    vec![
        opt(
            "level",
            GROUP_LEVEL,
            "-l, --level",
            TradeOffIcon::Size,
            "optimization level preset (higher = slower; explicit flags apply on top)",
            ControlSpec::Combo {
                variants: OXIPNG_LEVELS,
                get: |config| OptionValue::Index(oxipng_level_index(oxipng(config).level)),
                set: |config, value| {
                    oxipng_mut(config).level = oxipng_level_from_index(value.index())
                },
            },
        ),
        opt(
            "timeout (s, 0 = unlimited)",
            GROUP_LEVEL,
            "--timeout-secs",
            TradeOffIcon::Speed,
            "per-file optimization time limit",
            ControlSpec::Drag {
                range: (0.0, 86_400.0),
                suffix: "s",
                integer: true,
                get: |config| {
                    OptionValue::Number(oxipng(config).timeout.map_or(0.0, |t| t.as_secs() as f64))
                },
                set: |config, value| {
                    let secs = value.number();
                    oxipng_mut(config).timeout =
                        (secs > 0.0).then(|| Duration::from_secs(secs as u64));
                },
            },
        ),
        opt(
            "fix errors",
            GROUP_LEVEL,
            "--fix-errors",
            TradeOffIcon::None,
            "attempt fixing broken input PNGs",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(oxipng(config).fix_errors),
                set: |config, value| oxipng_mut(config).fix_errors = value.flag(),
            },
        ),
        opt(
            "zopfli",
            GROUP_SIZE,
            "--zopfli",
            TradeOffIcon::Size,
            "zopfli DEFLATE instead of libdeflate (much slower, slightly smaller)",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(oxipng(config).zopfli),
                set: |config, value| oxipng_mut(config).zopfli = value.flag(),
            },
        ),
        opt(
            "zopfli iterations",
            GROUP_SIZE,
            "--zopfli-iterations",
            TradeOffIcon::Size,
            "zopfli iteration count (only effective when zopfli is enabled)",
            ControlSpec::Drag {
                range: (1.0, 1000.0),
                suffix: "",
                integer: true,
                get: |config| OptionValue::Number(f64::from(oxipng(config).zopfli_iterations)),
                set: |config, value| oxipng_mut(config).zopfli_iterations = value.number() as u32,
            },
        )
        .requiring(|config| oxipng(config).zopfli),
        opt(
            "optimize alpha",
            GROUP_SIZE,
            "--optimize-alpha",
            TradeOffIcon::Size,
            "allow altering transparent pixel values for better compression (lossless visually)",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(oxipng(config).optimize_alpha),
                set: |config, value| oxipng_mut(config).optimize_alpha = value.flag(),
            },
        ),
        opt(
            "interlace",
            GROUP_OUTPUT,
            "--interlace",
            TradeOffIcon::None,
            "keep = leave the input's interlace state, none = remove, adam7 = force",
            ControlSpec::Combo {
                variants: &["keep", "none", "adam7"],
                get: |config| {
                    OptionValue::Index(match oxipng(config).interlace {
                        OxipngInterlace::Keep => 0,
                        OxipngInterlace::None => 1,
                        OxipngInterlace::Adam7 => 2,
                    })
                },
                set: |config, value| {
                    oxipng_mut(config).interlace = match value.index() {
                        0 => OxipngInterlace::Keep,
                        1 => OxipngInterlace::None,
                        _ => OxipngInterlace::Adam7,
                    };
                },
            },
        ),
        opt(
            "strip",
            GROUP_OUTPUT,
            "--strip",
            TradeOffIcon::None,
            "metadata chunk stripping; the EXIF policy may adjust this at run time (keep policy forces stripping off)",
            ControlSpec::Combo {
                variants: &["none", "safe", "all"],
                get: |config| {
                    OptionValue::Index(match oxipng(config).strip {
                        OxipngStrip::None => 0,
                        OxipngStrip::Safe => 1,
                        OxipngStrip::All => 2,
                    })
                },
                set: |config, value| {
                    oxipng_mut(config).strip = match value.index() {
                        1 => OxipngStrip::Safe,
                        2 => OxipngStrip::All,
                        _ => OxipngStrip::None,
                    };
                },
            },
        ),
        opt(
            "strip explicitly set",
            GROUP_OUTPUT,
            "--strip",
            TradeOffIcon::None,
            "mirrors passing --strip on the CLI (drives the EXIF policy interplay)",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(oxipng(config).strip_explicit),
                set: |config, value| oxipng_mut(config).strip_explicit = value.flag(),
            },
        ),
        opt(
            "scale 16-bit → 8-bit",
            GROUP_OUTPUT,
            "--scale-16",
            TradeOffIcon::None,
            "potentially pixel-altering",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(oxipng(config).scale_16),
                set: |config, value| oxipng_mut(config).scale_16 = value.flag(),
            },
        ),
        opt(
            "filters (empty = level preset)",
            GROUP_ADVANCED,
            "--filters",
            TradeOffIcon::Speed,
            "explicit filter strategies replace the filter set of the chosen level preset",
            ControlSpec::MultiSelect {
                entries: OXIPNG_FILTERS,
            },
        ),
        opt(
            "disable reductions",
            GROUP_ADVANCED,
            "--no-reduction",
            TradeOffIcon::None,
            "lossless reductions to disable",
            ControlSpec::MultiSelect {
                entries: OXIPNG_REDUCTIONS,
            },
        ),
    ]
}

impl OptionRow {
    /// Restricts the row's control to configs satisfying `condition`.
    fn requiring(mut self, condition: fn(&EncoderConfig) -> bool) -> Self {
        self.enabled_when = Some(condition);
        self
    }
}

fn webp_anim_rows() -> Vec<OptionRow> {
    vec![
        opt(
            "lossless",
            GROUP_QUALITY,
            "--lossless",
            TradeOffIcon::Size,
            "lossless encoding mode",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(webp_anim(config).lossless),
                set: |config, value| webp_anim_mut(config).lossless = value.flag(),
            },
        ),
        opt(
            "quality",
            GROUP_QUALITY,
            "-q, --quality",
            TradeOffIcon::Quality,
            "target quality 0–100 (in lossless mode this is the compression effort)",
            ControlSpec::Slider {
                range: (0.0, 100.0),
                get: |config| OptionValue::Number(f64::from(webp_anim(config).quality)),
                set: |config, value| webp_anim_mut(config).quality = value.number() as f32,
            },
        ),
        opt(
            "kmin (0 = auto)",
            GROUP_KEYFRAMES,
            "--kmin",
            TradeOffIcon::None,
            "minimum distance between keyframes",
            ControlSpec::Drag {
                range: (0.0, 10_000.0),
                suffix: "",
                integer: true,
                get: |config| OptionValue::Number(webp_anim(config).kmin.map_or(0.0, f64::from)),
                set: |config, value| webp_anim_mut(config).kmin = Some(value.number() as i32),
            },
        ),
        opt(
            "kmax (0 = no keyframes)",
            GROUP_KEYFRAMES,
            "--kmax",
            TradeOffIcon::None,
            "maximum distance between keyframes (0 = disables keyframe insertion)",
            ControlSpec::Drag {
                range: (0.0, 10_000.0),
                suffix: "",
                integer: true,
                get: |config| OptionValue::Number(webp_anim(config).kmax.map_or(0.0, f64::from)),
                set: |config, value| webp_anim_mut(config).kmax = Some(value.number() as i32),
            },
        ),
        opt(
            "minimize size",
            GROUP_KEYFRAMES,
            "--minimize-size",
            TradeOffIcon::Size,
            "slow; disables keyframe insertion",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(webp_anim(config).minimize_size),
                set: |config, value| webp_anim_mut(config).minimize_size = value.flag(),
            },
        ),
        opt(
            "method (0–6)",
            GROUP_METHOD,
            "--method",
            TradeOffIcon::Speed,
            "quality/speed trade-off (0 = fast, 6 = slower-better; default 4)",
            ControlSpec::Drag {
                range: (0.0, 6.0),
                suffix: "",
                integer: true,
                get: |config| OptionValue::Number(webp_anim(config).method.map_or(0.0, f64::from)),
                set: |config, value| webp_anim_mut(config).method = Some(value.number() as u8),
            },
        ),
        opt(
            "allow mixed lossy/lossless frames",
            GROUP_METHOD,
            "--allow-mixed",
            TradeOffIcon::None,
            "allow mixed lossy/lossless frames (libwebp picks per frame)",
            ControlSpec::Checkbox {
                get: |config| OptionValue::Flag(webp_anim(config).allow_mixed),
                set: |config, value| webp_anim_mut(config).allow_mixed = value.flag(),
            },
        ),
    ]
}

fn gif_rows() -> Vec<OptionRow> {
    vec![opt(
        "palette speed (1–30)",
        GROUP_PALETTE,
        "--speed",
        TradeOffIcon::Speed,
        "quantization speed (1 = best quality, 30 = fastest; default 10)",
        ControlSpec::Drag {
            range: (1.0, 30.0),
            suffix: "",
            integer: true,
            get: |config| OptionValue::Number(gif(config).speed.map_or(10.0, f64::from)),
            set: |config, value| gif_mut(config).speed = Some(value.number() as i32),
        },
    )]
}

// ---- restore defaults -------------------------------------------------------

/// Labels of the options that differ between `current` and `default`
/// (drives the "↺ defaults" button tooltip and its "reset N options"
/// notice). Pure; unit-tested per encoder.
#[must_use]
pub fn option_diff(current: &EncoderConfig, default: &EncoderConfig) -> Vec<&'static str> {
    encoder_rows(current)
        .iter()
        .filter(|row| !matches!(row.control, ControlSpec::Note(_)))
        .filter(|row| control_value(&row.control, current) != control_value(&row.control, default))
        .map(|row| row.label)
        .collect()
}

/// Reads a row's control value from a config (comparison payload).
fn control_value(control: &ControlSpec, config: &EncoderConfig) -> OptionValue {
    match control {
        ControlSpec::Note(_) => OptionValue::Opaque,
        ControlSpec::Checkbox { get, .. }
        | ControlSpec::Slider { get, .. }
        | ControlSpec::Drag { get, .. }
        | ControlSpec::Combo { get, .. } => get(config),
        ControlSpec::MultiSelect { entries } => {
            OptionValue::Flags(entries.iter().map(|entry| (entry.is_set)(config)).collect())
        }
        ControlSpec::Custom(CustomKind::JxlAdvanced) => {
            OptionValue::Text(format!("{:?}", jxl(config).advanced))
        }
    }
}

// ---- rendering --------------------------------------------------------------

/// Renders the options editor for the selected encoder as an aligned
/// two-column grid with group sub-headers (rows in [`GROUP_ADVANCED`]
/// collapse into an "Advanced" sub-header; disabled encoders are never
/// selected, so no editor is shown for them).
pub fn show_encoder_options(
    ui: &mut egui::Ui,
    encoder: &mut EncoderConfig,
    draft: &mut JxlAdvancedDraft,
) {
    let rows = encoder_rows(encoder);
    let advanced: Vec<OptionRow> = rows
        .iter()
        .filter(|row| row.group == GROUP_ADVANCED)
        .copied()
        .collect();
    let main: Vec<OptionRow> = rows
        .iter()
        .filter(|row| row.group != GROUP_ADVANCED)
        .copied()
        .collect();

    render_grid(ui, "encoder-options-grid", encoder, &main, draft);
    if !advanced.is_empty() {
        ui.add_space(2.0);
        egui::CollapsingHeader::new("Advanced")
            .default_open(false)
            .show(ui, |ui| {
                render_grid(
                    ui,
                    "encoder-options-advanced-grid",
                    encoder,
                    &advanced,
                    draft,
                );
            });
    }
}

/// The tooltip of a row: explanation plus the mirroring CLI flag.
fn hover_text(row: &OptionRow) -> String {
    if row.cli_flag.is_empty() {
        row.tooltip.to_owned()
    } else {
        format!("{} (CLI: {})", row.tooltip, row.cli_flag)
    }
}

/// Renders the rows into an aligned two-column `egui::Grid`: label
/// (right-aligned, weak, with trade-off icon) + control, with a bold
/// sub-header row whenever the group changes.
fn render_grid(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    encoder: &mut EncoderConfig,
    rows: &[OptionRow],
    draft: &mut JxlAdvancedDraft,
) {
    egui::Grid::new(id_salt)
        .num_columns(2)
        .spacing([12.0, 7.0])
        .show(ui, |ui| {
            let mut open_group = "";
            for row in rows {
                if row.group != open_group {
                    open_group = row.group;
                    if !open_group.is_empty() {
                        ui.strong(open_group);
                        ui.end_row();
                    }
                }
                let hover = hover_text(row);
                if let ControlSpec::Note(text) = row.control {
                    ui.weak(text).on_hover_text(hover);
                    ui.end_row();
                    continue;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let glyph = row.icon.glyph();
                    let text = if glyph.is_empty() {
                        row.label.to_owned()
                    } else {
                        format!("{} {glyph}", row.label)
                    };
                    ui.weak(text).on_hover_text(hover.clone());
                });
                let enabled = row.enabled_when.is_none_or(|condition| condition(encoder));
                ui.add_enabled_ui(enabled, |ui| {
                    render_control(ui, encoder, row, draft, &hover);
                });
                ui.end_row();
            }
        });
}

/// Renders one row's control cell and writes edits back into the config.
fn render_control(
    ui: &mut egui::Ui,
    encoder: &mut EncoderConfig,
    row: &OptionRow,
    draft: &mut JxlAdvancedDraft,
    hover: &str,
) {
    match row.control {
        ControlSpec::Note(_) => unreachable!("note rows are handled by render_grid"),
        ControlSpec::Checkbox { get, set } => {
            let mut checked = get(encoder).flag();
            let response = ui.checkbox(&mut checked, "");
            if response.changed() {
                set(encoder, OptionValue::Flag(checked));
            }
            response.on_hover_text(hover);
        }
        ControlSpec::Slider { range, get, set } => {
            let mut value = get(encoder).number();
            let response = ui.add(egui::Slider::new(&mut value, range.0..=range.1));
            if response.changed() {
                set(encoder, OptionValue::Number(value));
            }
            response.on_hover_text(hover);
        }
        ControlSpec::Drag {
            range,
            suffix,
            integer,
            get,
            set,
        } => {
            let mut value = get(encoder).number();
            let mut drag = egui::DragValue::new(&mut value).range(range.0..=range.1);
            if integer {
                drag = drag.speed(1.0).max_decimals(0);
            }
            if !suffix.is_empty() {
                drag = drag.suffix(suffix);
            }
            let response = ui.add(drag);
            if response.changed() {
                set(encoder, OptionValue::Number(value));
            }
            response.on_hover_text(hover);
        }
        ControlSpec::Combo { variants, get, set } => {
            let mut index = get(encoder).index();
            let mut changed = false;
            let response = egui::ComboBox::from_id_salt(row.label)
                .selected_text(variants.get(index).copied().unwrap_or("?"))
                .show_ui(ui, |ui| {
                    for (candidate, name) in variants.iter().enumerate() {
                        if ui.selectable_value(&mut index, candidate, *name).clicked() {
                            changed = true;
                        }
                    }
                });
            if changed {
                set(encoder, OptionValue::Index(index));
            }
            response.response.on_hover_text(hover);
        }
        ControlSpec::MultiSelect { entries } => {
            ui.horizontal_wrapped(|ui| {
                for entry in entries {
                    let mut checked = (entry.is_set)(encoder);
                    let response = ui.checkbox(&mut checked, entry.name);
                    if response.changed() {
                        (entry.set)(encoder, checked);
                    }
                    response.on_hover_text(hover);
                }
            });
        }
        ControlSpec::Custom(CustomKind::JxlAdvanced) => jxl_advanced_ui(ui, encoder, draft),
    }
}

/// The JXL advanced `--setting ID=VALUE` passthrough editor (WS2); the
/// "add row" text inputs live in [`JxlAdvancedDraft`] between frames.
fn jxl_advanced_ui(ui: &mut egui::Ui, encoder: &mut EncoderConfig, draft: &mut JxlAdvancedDraft) {
    let options = jxl_mut(encoder);
    ui.weak("libjxl frame-setting passthrough (--setting ID=VALUE)");
    let mut remove_at: Option<usize> = None;
    for (index, (id, value)) in options.advanced.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let id_response = ui.add(egui::TextEdit::singleline(id).desired_width(220.0));
            id_response.on_hover_text("setting id (resolved case-insensitively at run time)");
            ui.add(egui::DragValue::new(value));
            if ui.button("✕").clicked() {
                remove_at = Some(index);
            }
        });
    }
    if let Some(index) = remove_at {
        options.advanced.remove(index);
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut draft.new_id)
                .hint_text("new setting id")
                .desired_width(220.0),
        );
        ui.add(egui::DragValue::new(&mut draft.new_value).range(i64::MIN..=i64::MAX));
        if ui.button("add").clicked() && !draft.new_id.trim().is_empty() {
            options
                .advanced
                .push((draft.new_id.trim().to_string(), draft.new_value));
            draft.new_id.clear();
            draft.new_value = 0;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_maps_to_a_config_and_kind_name_round_trips() {
        let caps = byteshaver::job::capabilities();
        for info in &caps.encoders {
            let config = default_encoder_config(info.name)
                .unwrap_or_else(|| panic!("no default config for {}", info.name));
            assert_eq!(encoder_kind_name(&config), info.name);
        }
        assert!(default_encoder_config("nope").is_none());
        assert_eq!(
            encoder_kind_name(&default_encoder_config("avif").expect("avif")),
            "avif"
        );
    }

    #[test]
    fn capability_enablement_matches_availability_helpers() {
        let caps = byteshaver::job::capabilities();
        // the default-feature gui build compiles in every optional encoder
        assert!(encoder_enabled(&caps, "webp"));
        assert!(encoder_enabled(&caps, "webp-image"));
        assert!(encoder_enabled(&caps, "avif"));
        assert!(encoder_enabled(&caps, "png"));
        assert!(encoder_enabled(&caps, "jpeg"));
        assert!(encoder_enabled(&caps, "jxl"));
        assert!(encoder_enabled(&caps, "oxipng"));
        assert!(encoder_enabled(&caps, "webp-anim"));
        assert!(encoder_enabled(&caps, "apng"));
        assert!(encoder_enabled(&caps, "gif"));
        assert!(encoder_disabled_reason(&caps, "webp").is_none());
        assert!(encoder_disabled_reason(&caps, "unknown-encoder").is_none());
        for name in ENCODER_NAMES {
            assert!(
                encoder_enabled(&caps, name),
                "encoder {name} should be listed as enabled in a default build"
            );
        }
        assert_eq!(ENCODER_NAMES.len(), caps.encoders.len());
    }

    #[test]
    fn defaults_match_the_cli_subcommand_defaults() {
        use byteshaver::cli::CliArgs;
        use clap::Parser;

        for name in ENCODER_NAMES {
            let expected = default_encoder_config(name).expect("variant exists");
            // build the CLI args for the same subcommand (no options set);
            // the CLI shape is `byteshaver <PATTERN> <SUBCOMMAND>`
            let args = CliArgs::parse_from(["byteshaver", "pattern", name]);
            let from_cli = EncoderConfig::from_args(&args.command)
                .expect("every gui-listed encoder is a conversion subcommand");
            assert_eq!(encoder_kind_name(&from_cli), name);
            assert_eq!(
                from_cli, expected,
                "gui defaults of {name} must match the CLI defaults"
            );
        }
    }

    #[test]
    fn schemas_are_complete_and_carry_cli_flags() {
        for name in ENCODER_NAMES {
            let config = default_encoder_config(name).expect("default config");
            let rows = encoder_rows(&config);
            assert!(!rows.is_empty(), "{name} must have at least one schema row");

            let mut labels = Vec::new();
            for row in &rows {
                if matches!(row.control, ControlSpec::Note(_)) {
                    continue;
                }
                assert!(!row.label.is_empty(), "{name}: control row without a label");
                assert!(
                    !row.cli_flag.is_empty(),
                    "{name}: row '{}' has no CLI flag",
                    row.label
                );
                assert!(
                    !row.tooltip.is_empty(),
                    "{name}: row '{}' has no tooltip",
                    row.label
                );
                labels.push(row.label);
            }
            let duplicate = labels
                .iter()
                .rfind(|label| labels.iter().filter(|other| *other == *label).count() > 1);
            assert_eq!(duplicate, None, "{name}: duplicate row label {duplicate:?}");
        }
    }

    /// Applies one row's control edit to a copy of `base` (test helper for
    /// the per-encoder diff check).
    fn touched_config(row: &OptionRow, base: &EncoderConfig) -> EncoderConfig {
        let mut config = base.clone();
        match row.control {
            ControlSpec::Note(_) => {}
            ControlSpec::Checkbox { set, .. } => {
                let flipped = !control_value(&row.control, &config).flag();
                set(&mut config, OptionValue::Flag(flipped));
            }
            ControlSpec::Slider { range, set, .. } | ControlSpec::Drag { range, set, .. } => {
                set(&mut config, OptionValue::Number(range.1.max(range.0 + 1.0)));
            }
            ControlSpec::Combo { variants, set, .. } => {
                let current = control_value(&row.control, &config).index();
                set(
                    &mut config,
                    OptionValue::Index((current + 1) % variants.len()),
                );
            }
            ControlSpec::MultiSelect { entries } => {
                let entry = &entries[0];
                let on = !(entry.is_set)(&config);
                (entry.set)(&mut config, on);
            }
            ControlSpec::Custom(CustomKind::JxlAdvanced) => {
                let EncoderConfig::Jxl(options) = &mut config else {
                    panic!("jxl advanced row must only exist on the jxl schema");
                };
                options.advanced.push(("brotli_effort".to_owned(), 9));
            }
        }
        config
    }

    #[test]
    fn option_diff_reports_exactly_the_touched_option() {
        for name in ENCODER_NAMES {
            let default = default_encoder_config(name).expect("default config");
            assert!(
                option_diff(&default, &default).is_empty(),
                "{name}: defaults must not differ from themselves"
            );
            for row in encoder_rows(&default) {
                if matches!(row.control, ControlSpec::Note(_)) {
                    continue;
                }
                let touched = touched_config(&row, &default);
                assert_eq!(
                    option_diff(&touched, &default),
                    vec![row.label],
                    "{name}: touching '{}' must diff exactly that label",
                    row.label
                );
            }
        }
    }

    #[test]
    fn every_encoder_schema_renders_headlessly() {
        // exercises the actual egui render path (grid + every control kind)
        // for every encoder, headless via egui's test context
        for name in ENCODER_NAMES {
            let mut config = default_encoder_config(name).expect("default config");
            let mut draft = crate::app::JxlAdvancedDraft::default();
            egui::__run_test_ctx(|ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show_encoder_options(ui, &mut config, &mut draft);
                });
            });
        }
    }

    #[test]
    fn every_touched_config_renders_headlessly() {
        // setters must only ever produce states the renderer can display
        // again (in-range combo indices, valid numbers, …)
        for name in ENCODER_NAMES {
            let default = default_encoder_config(name).expect("default config");
            for row in encoder_rows(&default) {
                if matches!(row.control, ControlSpec::Note(_)) {
                    continue;
                }
                let mut config = touched_config(&row, &default);
                let mut draft = crate::app::JxlAdvancedDraft::default();
                egui::__run_test_ctx(|ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        show_encoder_options(ui, &mut config, &mut draft);
                    });
                });
            }
        }
    }
}
