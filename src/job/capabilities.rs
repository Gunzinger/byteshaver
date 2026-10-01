//! Capability discovery for front-ends (plan WS7 §2.3): every registry
//! encoder with its typed properties and compile-time availability.
//!
//! Enabled entries are driven by [`EncoderRegistry`] (the description is
//! the encoder's option-free identity line, `describe()` — plan 15 F4);
//! encoders compiled out (`jxl`, `opt-oxipng`, `anim-webp`, `anim-apng`)
//! are still listed with `enabled = false` and a reason so UIs can gray
//! them out up front instead of failing per file at runtime.

use crate::converter::{EncoderRegistry, ThreadBudget};

/// Typed description of one target encoder.
#[derive(Clone, Debug, serde::Serialize)]
pub struct EncoderInfo {
    /// Stable identifier (CLI subcommand name), e.g. `"webp-anim"`.
    pub name: &'static str,
    /// Output file extension (without dot).
    pub extension: &'static str,
    /// Whether the encoder can encode animated input natively.
    pub supports_animation: bool,
    /// Whether the encoder can embed EXIF metadata into its output.
    pub supports_metadata: bool,
    /// `false` when this build was compiled without the encoder's feature.
    pub enabled: bool,
    /// Why the encoder is unavailable (`None` when enabled).
    pub disabled_reason: Option<&'static str>,
    /// Human-readable identity description of the encoder (library or
    /// binding name plus one-line character; no option values — those
    /// change at runtime and would go stale here, plan 15 F4).
    pub description: String,
}

/// Compile-time capabilities of this build of `byteshaver`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Capabilities {
    /// Every target encoder, in registry order (webp, webp-image, avif,
    /// png, jpeg, jxl, oxipng, webp-anim, apng, gif).
    pub encoders: Vec<EncoderInfo>,
    /// Whether HEIC/HEIF/AVIF container input decoding is compiled in
    /// (`dec-heif` feature). Without it, such inputs fail per file.
    pub heif_input_enabled: bool,
}

/// Builds one live (compiled-in) entry from a default-constructed config.
fn live(name: &'static str, config: &crate::config::EncoderConfig) -> EncoderInfo {
    // single-thread throwaway budget: capabilities() only inspects metadata
    let encoder = EncoderRegistry::build(
        config,
        ThreadBudget {
            files: 1,
            threads_per_encoder: Some(1),
        },
    );
    EncoderInfo {
        name,
        extension: encoder.extension(),
        supports_animation: encoder.supports_animation(),
        supports_metadata: encoder.supports_metadata(),
        enabled: true,
        disabled_reason: None,
        description: encoder.describe(),
    }
}

/// Builds one compiled-out entry with a static fallback description.
// unused when every optional encoder feature is enabled
#[cfg_attr(
    all(
        feature = "jxl",
        feature = "opt-oxipng",
        feature = "anim-webp",
        feature = "anim-apng"
    ),
    allow(dead_code)
)]
fn disabled(
    name: &'static str,
    extension: &'static str,
    supports_animation: bool,
    supports_metadata: bool,
    reason: &'static str,
    description: &str,
) -> EncoderInfo {
    EncoderInfo {
        name,
        extension,
        supports_animation,
        supports_metadata,
        enabled: false,
        disabled_reason: Some(reason),
        description: description.to_string(),
    }
}

/// Collects the encoder entries of this build in registry order.
fn encoder_infos() -> Vec<EncoderInfo> {
    use crate::config::EncoderConfig;

    let mut infos = vec![
        live(
            "webp",
            &EncoderConfig::Webp(crate::config::WebpOptions {
                lossless: false,
                quality: 90.0,
            }),
        ),
        live("webp-image", &EncoderConfig::WebpImage),
        live(
            "avif",
            &EncoderConfig::Avif(crate::config::AvifOptions {
                quality: 90.0,
                speed: 3,
                ..crate::config::AvifOptions::default()
            }),
        ),
        live(
            "png",
            &EncoderConfig::Png(crate::config::PngOptions::default()),
        ),
        live("jpeg", &EncoderConfig::Jpeg),
    ];

    #[cfg(feature = "jxl")]
    infos.push(live(
        "jxl",
        &EncoderConfig::Jxl(crate::config::JxlOptions::default()),
    ));
    #[cfg(not(feature = "jxl"))]
    infos.push(disabled(
        "jxl",
        "jxl",
        true,
        true,
        "this build was compiled without the \"jxl\" feature",
        "jpeg-xl encoder via libjxl (lossy, lossless, animation, EXIF boxes)",
    ));

    #[cfg(feature = "opt-oxipng")]
    infos.push(live(
        "oxipng",
        &EncoderConfig::Oxipng(crate::config::OxipngOptions::default()),
    ));
    #[cfg(not(feature = "opt-oxipng"))]
    infos.push(disabled(
        "oxipng",
        "png",
        false,
        true,
        "this build was compiled without the \"opt-oxipng\" feature",
        "png re-optimization / transcoding via oxipng (level 2)",
    ));

    #[cfg(feature = "anim-webp")]
    infos.push(live(
        "webp-anim",
        &EncoderConfig::WebpAnim(crate::config::WebpAnimOptions::default()),
    ));
    #[cfg(not(feature = "anim-webp"))]
    infos.push(disabled(
        "webp-anim",
        "webp",
        true,
        false,
        "this build was compiled without the \"anim-webp\" feature",
        "animated webp encoder via webp-animation (quality 90.0)",
    ));

    #[cfg(feature = "anim-apng")]
    infos.push(live(
        "apng",
        &EncoderConfig::Apng(crate::config::ApngOptions::default()),
    ));
    #[cfg(not(feature = "anim-apng"))]
    infos.push(disabled(
        "apng",
        "png",
        true,
        false,
        "this build was compiled without the \"anim-apng\" feature",
        "animated png (APNG) encoder via the png crate",
    ));

    infos.push(live(
        "gif",
        &EncoderConfig::Gif(crate::config::GifOptions { speed: Some(10) }),
    ));

    infos
}

/// Returns the compile-time capabilities of this build.
///
/// The extension of a target is its output extension: `oxipng`, `png` and
/// `apng` all produce `png` files; `webp`, `webp-image` and `webp-anim` all
/// produce `webp` files (see `ImageFormat::extension`).
#[must_use]
pub fn capabilities() -> Capabilities {
    Capabilities {
        encoders: encoder_infos(),
        heif_input_enabled: cfg!(feature = "dec-heif"),
    }
}
