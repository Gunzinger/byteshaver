//! The compiled-in built-in presets (plan 14 §5): literature-derived
//! profiles for AVIF, WebP and JPEG XL plus the safe-re-optimize oxipng
//! profile — and, as plan-13 integration, the complete chip-ladder table
//! for *every* encoder (the interim `chips.rs` table migrated verbatim;
//! those extra profiles are the GUI's curated defaults for encoders whose
//! literature is too thin for a plan-§5 claim).
//!
//! # Sources & verification checklist
//!
//! The plan-§5 numbers below were cross-checked against the cited
//! material before being baked in. `// VERIFY:` notes mark what to
//! re-check when touching a profile (`cjxl --help` / libjxl `doc/`
//! distance semantics; kornelski's ravif README quality guidance;
//! Google's WebP compression study, developers.google.com/speed/webp;
//! Netflix "AVIF for next-generation image coding", netflixtechblog.com;
//! Cloudinary AVIF study 2020; the JPEG XL whitepaper / ICASSP paper).
//! All wording in the descriptions stays **hedged** — these are
//! candidate values from published data, not benchmark results of this
//! encoder build.
//!
//! - **AVIF quality band**: the ravif (kornelski) README recommends the
//!   quality ~50–70 range as the useful band (`VERIFY:` wording drifts
//!   between releases; the band, not a single number, is the claim).
//!   Netflix's techblog and Cloudinary's 2020 study place AVIF q≈50 near
//!   JPEG q≈75–80 perceptually at roughly half the bytes. 10-bit reduces
//!   banding at high quality (AOM/AV1 10-bit guidance).
//! - **WebP**: Google's WebP compression study reports q≈70–75 as a
//!   strong photo-fidelity point and lossless WebP ~25–35 % smaller than
//!   PNG. libwebp's own default quality is 75 (`VERIFY:` stable across
//!   the `webp` crate's C sources).
//! - **JPEG XL**: libjxl docs define *distance* as the maximum
//!   Butteraugli distance — d1.0 is the canonical "visually lossless"
//!   point and cjxl's default lossy setting; d2.0 trades perceptual
//!   margin for roughly twice the savings (whitepaper rate-distortion
//!   data). Effort 7 is the cjxl default; 8 spends more search time.
//! - **oxipng**: level 2 is the tool's documented sane default; the
//!   "safe re-optimize" profile deliberately avoids zopfli/16-bit scaling
//!   (literature thin — hedged wording, `VERIFY:` against the oxipng
//!   README when bumping levels).

use byteshaver::config::{
    ApngOptions, AvifOptions, BitDepth, ColorModel, CompressionType, EncoderConfig, FilterType,
    GifOptions, JxlOptions, OxipngLevel, OxipngOptions, PngOptions, WebpAnimOptions, WebpOptions,
};

use super::{PRESET_SCHEMA, Preset, PresetContent, SizeTag, SpeedTag};

/// One built-in profile: the preset payload plus the chip-ladder
/// presentation metadata (plan 13 approach-B icons; plan 14 §5).
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinProfile {
    /// Full preset title (unique across all built-ins), shown in the
    /// preset dropdown, e.g. `AVIF · Compact`.
    pub preset_title: &'static str,
    /// Short card title of the chip ladder, e.g. `Compact`.
    pub chip_title: &'static str,
    /// One-line description with the concrete values and the hedged
    /// rationale (shown under the ladder and as hover text).
    pub description: &'static str,
    /// Relative quality as `✦` dot count (1–5).
    pub quality_dots: u8,
    /// Qualitative size tag (rendered after `▤`).
    pub size_tag: SizeTag,
    /// Qualitative encode-time tag (rendered after `⚡`).
    pub speed_tag: SpeedTag,
    /// The complete configuration this profile applies.
    pub encoder: EncoderConfig,
}

/// The built-in table (ordered compact → high within each encoder).
///
/// Built once per process ([`OnceLock`]): the option structs carry heap
/// types, so the table cannot be a `const`/`static` initializer.
pub fn profiles() -> &'static [BuiltinProfile] {
    static TABLE: std::sync::OnceLock<Vec<BuiltinProfile>> = std::sync::OnceLock::new();
    TABLE.get_or_init(
        || vec![
    // ---- webp (libwebp via the `webp` crate) --------------------------------
        BuiltinProfile {
        preset_title: "WebP · Compact",
        chip_title: "Compact",
        description: "q70 — smaller files with mild softening (Google's WebP study reports strong photo fidelity around q70–75)",
        quality_dots: 2,
        size_tag: SizeTag::Smallest,
        speed_tag: SpeedTag::Faster,
        encoder: EncoderConfig::Webp(WebpOptions {
            quality: 70.0,
            ..WebpOptions::default()
        }),
    },
    BuiltinProfile {
        preset_title: "WebP · Balanced",
        chip_title: "Balanced",
        description: "q80 — libwebp's default 75 nudged up for web photos; a widely cited sweet spot",
        quality_dots: 3,
        size_tag: SizeTag::Smaller,
        speed_tag: SpeedTag::Baseline,
        encoder: EncoderConfig::Webp(WebpOptions {
            quality: 80.0,
            ..WebpOptions::default()
        }),
    },
    BuiltinProfile {
        preset_title: "WebP · High fidelity",
        chip_title: "High fidelity",
        description: "q90 — visually-safe re-encode tier; beyond 90 returns diminish sharply",
        quality_dots: 4,
        size_tag: SizeTag::Typical,
        speed_tag: SpeedTag::Baseline,
        encoder: EncoderConfig::Webp(WebpOptions::default()),
    },
    BuiltinProfile {
        preset_title: "WebP · Lossless",
        chip_title: "Lossless",
        description: "byte-identical output, larger files (the Google study reports ~25–35 % smaller PNGs as lossless WebP)",
        quality_dots: 5,
        size_tag: SizeTag::Larger,
        speed_tag: SpeedTag::Slower,
        encoder: EncoderConfig::Webp(WebpOptions {
            lossless: true,
            ..WebpOptions::default()
        }),
    },
    // ---- webp-image (image crate) --------------------------------------------
    BuiltinProfile {
        preset_title: "WebP(image) · Lossless",
        chip_title: "Lossless",
        description: "image-crate lossless webp (no options)",
        quality_dots: 5,
        size_tag: SizeTag::Typical,
        speed_tag: SpeedTag::Baseline,
        encoder: EncoderConfig::WebpImage,
    },
    // ---- avif (ravif) ----------------------------------------------------------
    BuiltinProfile {
        preset_title: "AVIF · Compact",
        chip_title: "Compact",
        description: "q50 · speed 6 · 8-bit — smallest files, for thumbnails and bulk archives (studies put AVIF q≈50 near JPEG q≈75–80 at fewer bytes)",
        quality_dots: 2,
        size_tag: SizeTag::Smallest,
        speed_tag: SpeedTag::Faster,
        encoder: EncoderConfig::Avif(AvifOptions {
            quality: 50.0,
            speed: 6,
            bit_depth: Some(BitDepth::Eight),
            color_model: Some(ColorModel::YCbCr),
            ..AvifOptions::default()
        }),
    },
    BuiltinProfile {
        preset_title: "AVIF · Balanced",
        chip_title: "Balanced",
        description: "q62 · speed 4 · 8-bit — web photos; inside the 60–70 band the ravif author calls useful, roughly half the bytes of JPEG q85 at equal perception",
        quality_dots: 3,
        size_tag: SizeTag::Smaller,
        speed_tag: SpeedTag::Baseline,
        encoder: EncoderConfig::Avif(AvifOptions {
            quality: 62.0,
            speed: 4,
            bit_depth: Some(BitDepth::Eight),
            color_model: Some(ColorModel::YCbCr),
            ..AvifOptions::default()
        }),
    },
    BuiltinProfile {
        preset_title: "AVIF · High fidelity",
        chip_title: "High fidelity",
        description: "q78 · speed 3 · 10-bit — near-transparent quality, slowest encode (10-bit reduces banding at high quality, AOM/AV1 guidance)",
        quality_dots: 5,
        size_tag: SizeTag::Larger,
        speed_tag: SpeedTag::Slower,
        encoder: EncoderConfig::Avif(AvifOptions {
            quality: 78.0,
            speed: 3,
            bit_depth: Some(BitDepth::Ten),
            color_model: Some(ColorModel::YCbCr),
            ..AvifOptions::default()
        }),
    },
    // ---- png (image crate) ------------------------------------------------------
    BuiltinProfile {
        preset_title: "PNG · Default",
        chip_title: "Default",
        description: "encoder defaults (balanced compression)",
        quality_dots: 5,
        size_tag: SizeTag::Typical,
        speed_tag: SpeedTag::Faster,
        encoder: EncoderConfig::Png(PngOptions::default()),
    },
    BuiltinProfile {
        preset_title: "PNG · Best",
        chip_title: "Best",
        description: "best compression + adaptive filtering — slow",
        quality_dots: 5,
        size_tag: SizeTag::Smaller,
        speed_tag: SpeedTag::Slower,
        encoder: EncoderConfig::Png(PngOptions {
            compression_type: Some(CompressionType::Best),
            filter_type: Some(FilterType::Adaptive),
        }),
    },
    // ---- jpeg (mozjpeg) -----------------------------------------------------------
    BuiltinProfile {
        preset_title: "JPEG · Default",
        chip_title: "Default",
        description: "mozjpeg defaults (the encoder has no options)",
        quality_dots: 5,
        size_tag: SizeTag::Typical,
        speed_tag: SpeedTag::Baseline,
        encoder: EncoderConfig::Jpeg,
    },
    // ---- jxl (libjxl) ---------------------------------------------------------------
    BuiltinProfile {
        preset_title: "JXL · Compact",
        chip_title: "Compact",
        description: "distance 2.0 · effort 8 — smallest, slower effort (whitepaper rate-distortion data: roughly twice the savings of d1.0)",
        quality_dots: 3,
        size_tag: SizeTag::Smallest,
        speed_tag: SpeedTag::Slower,
        encoder: EncoderConfig::Jxl(JxlOptions {
            distance: Some(2.0),
            effort: 8,
            ..JxlOptions::default()
        }),
    },
    BuiltinProfile {
        preset_title: "JXL · Visually lossless",
        chip_title: "Visually lossless",
        description: "distance 1.0 · effort 7 — Butteraugli 1.0, the canonical visually-lossless point (cjxl's default lossy setting)",
        quality_dots: 4,
        size_tag: SizeTag::Smaller,
        speed_tag: SpeedTag::Baseline,
        encoder: EncoderConfig::Jxl(JxlOptions {
            distance: Some(1.0),
            effort: 7,
            ..JxlOptions::default()
        }),
    },
    BuiltinProfile {
        preset_title: "JXL · Bit-exact",
        chip_title: "Bit-exact",
        description: "lossless · effort 7 — true lossless mode for archival",
        quality_dots: 5,
        size_tag: SizeTag::Larger,
        speed_tag: SpeedTag::Slower,
        encoder: EncoderConfig::Jxl(JxlOptions {
            lossless: true,
            effort: 7,
            ..JxlOptions::default()
        }),
    },
    // ---- oxipng -----------------------------------------------------------------------
    BuiltinProfile {
        preset_title: "Oxipng · Safe re-optimize",
        chip_title: "Default",
        description: "level 2 preset — conservative, lossless re-optimization (the CLI default; the literature on deeper levels is thin)",
        quality_dots: 5,
        size_tag: SizeTag::Typical,
        speed_tag: SpeedTag::Faster,
        encoder: EncoderConfig::Oxipng(OxipngOptions::default()),
    },
    BuiltinProfile {
        preset_title: "Oxipng · Deep",
        chip_title: "Deep",
        description: "level 4 + zopfli — smallest PNGs, much slower",
        quality_dots: 5,
        size_tag: SizeTag::Smallest,
        speed_tag: SpeedTag::Slowest,
        encoder: EncoderConfig::Oxipng(OxipngOptions {
            level: OxipngLevel::Four,
            zopfli: true,
            ..OxipngOptions::default()
        }),
    },
    // ---- webp-anim ----------------------------------------------------------------------
    BuiltinProfile {
        preset_title: "WebP-anim · Compact",
        chip_title: "Compact",
        description: "q75 — smaller animations, faster encode",
        quality_dots: 2,
        size_tag: SizeTag::Smallest,
        speed_tag: SpeedTag::Faster,
        encoder: EncoderConfig::WebpAnim(WebpAnimOptions {
            quality: 75.0,
            ..WebpAnimOptions::default()
        }),
    },
    BuiltinProfile {
        preset_title: "WebP-anim · Balanced",
        chip_title: "Balanced",
        description: "q85 — balanced animation quality",
        quality_dots: 3,
        size_tag: SizeTag::Smaller,
        speed_tag: SpeedTag::Baseline,
        encoder: EncoderConfig::WebpAnim(WebpAnimOptions {
            quality: 85.0,
            ..WebpAnimOptions::default()
        }),
    },
    // ---- apng ------------------------------------------------------------------------------
    BuiltinProfile {
        preset_title: "APNG · Default",
        chip_title: "Default",
        description: "encoder defaults (balanced compression)",
        quality_dots: 5,
        size_tag: SizeTag::Typical,
        speed_tag: SpeedTag::Faster,
        encoder: EncoderConfig::Apng(ApngOptions::default()),
    },
    BuiltinProfile {
        preset_title: "APNG · Best",
        chip_title: "Best",
        description: "best compression + adaptive filtering — slow",
        quality_dots: 5,
        size_tag: SizeTag::Smaller,
        speed_tag: SpeedTag::Slower,
        encoder: EncoderConfig::Apng(ApngOptions {
            compression_type: Some(CompressionType::Best),
            filter_type: Some(FilterType::Adaptive),
        }),
    },
    // ---- gif ---------------------------------------------------------------------------------
    BuiltinProfile {
        preset_title: "GIF · Default",
        chip_title: "Default",
        description: "palette speed 10 (the CLI default)",
        quality_dots: 5,
        size_tag: SizeTag::Typical,
        speed_tag: SpeedTag::Baseline,
        encoder: EncoderConfig::Gif(GifOptions::default()),
    }],
    )
}

/// All built-ins as full read-only [`Preset`] values (format-only scope:
/// no policies, plan 14 §5; `builtin: true` and never persisted).
#[must_use]
pub fn builtin() -> Vec<Preset> {
    profiles()
        .iter()
        .map(|profile| Preset {
            schema: PRESET_SCHEMA,
            title: profile.preset_title.to_string(),
            description: profile.description.to_string(),
            created_unix: 0,
            modified_unix: 0,
            core_version: crate::app::CORE_VERSION.to_string(),
            builtin: true,
            content: PresetContent {
                encoder: profile.encoder.clone(),
                policies: None,
                include_output_dir: false,
            },
        })
        .collect()
}

/// The profiles of one encoder kind (capability name like `"avif"`), in
/// table order; empty for unknown names.
#[must_use]
pub fn profiles_for(kind: &str) -> Vec<&'static BuiltinProfile> {
    profiles()
        .iter()
        .filter(|profile| crate::options::encoder_kind_name(&profile.encoder) == kind)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::{DESCRIPTION_MAX_CHARS, TITLE_MAX_CHARS};
    use crate::options::{ENCODER_NAMES, encoder_kind_name};

    #[test]
    fn every_builtin_is_a_known_variant_with_sane_metadata() {
        assert!(
            profiles().len() >= 11,
            "the plan-§5 table (11 profiles) is a subset of the full chip table"
        );
        let mut titles = Vec::new();
        for profile in profiles() {
            // known encoder kind + the exact option surface round-trips
            let kind = encoder_kind_name(&profile.encoder);
            assert!(
                ENCODER_NAMES.contains(&kind),
                "{}: unknown encoder kind {kind}",
                profile.preset_title
            );
            let json = serde_json::to_string(&profile.encoder).expect("serialize");
            let back: EncoderConfig = serde_json::from_str(&json).expect("deserialize back");
            assert_eq!(back, profile.encoder, "{}: serde round trip", profile.preset_title);
            assert!(
                (1..=5).contains(&profile.quality_dots),
                "{}: dot count out of range",
                profile.preset_title
            );
            assert!(
                !profile.description.is_empty()
                    && profile.description.chars().count() <= DESCRIPTION_MAX_CHARS,
                "{}: description bounds",
                profile.preset_title
            );
            assert!(
                !profile.preset_title.is_empty()
                    && profile.preset_title.chars().count() <= TITLE_MAX_CHARS,
            );
            titles.push(profile.preset_title);
        }
        assert_eq!(
            titles
                .iter()
                .rfind(|title| titles.iter().filter(|other| *other == *title).count() > 1),
            None,
            "duplicate built-in title"
        );
    }

    #[test]
    fn every_encoder_kind_has_profiles_for_the_chip_ladder() {
        for name in ENCODER_NAMES {
            let profiles = profiles_for(name);
            assert!(!profiles.is_empty(), "{name} lost its chip ladder");
            for profile in &profiles {
                assert_eq!(
                    encoder_kind_name(&profile.encoder),
                    name,
                    "profile {} carries a different kind",
                    profile.preset_title
                );
            }
        }
        assert!(profiles_for("nope").is_empty());
    }

    #[test]
    fn plan_5_numbers_match_the_verified_table() {
        // AVIF: q50/speed 6/8-bit/YCbCr · q62/speed 4/8-bit · q78/speed 3/10-bit
        let avif = profiles_for("avif");
        let avif_assert = |quality: f32, speed: u8, depth: Option<BitDepth>, title: &str| {
            let profile = avif
                .iter()
                .find(|profile| profile.preset_title == title)
                .unwrap_or_else(|| panic!("missing {title}"));
            let EncoderConfig::Avif(options) = &profile.encoder else {
                panic!("{title} carries the wrong variant");
            };
            assert_eq!(options.quality, quality);
            assert_eq!(options.speed, speed);
            assert_eq!(options.bit_depth, depth);
            assert_eq!(options.color_model, Some(ColorModel::YCbCr));
            assert_eq!(options.alpha_quality, AvifOptions::default().alpha_quality);
        };
        avif_assert(50.0, 6, Some(BitDepth::Eight), "AVIF · Compact");
        avif_assert(62.0, 4, Some(BitDepth::Eight), "AVIF · Balanced");
        avif_assert(78.0, 3, Some(BitDepth::Ten), "AVIF · High fidelity");

        // WebP: q70 / q80 / q90 / lossless
        let webp_qualities: Vec<(String, f32, bool)> = profiles_for("webp")
            .iter()
            .map(|profile| {
                let EncoderConfig::Webp(options) = &profile.encoder else {
                    panic!("{} carries the wrong variant", profile.preset_title);
                };
                (profile.preset_title.to_string(), options.quality, options.lossless)
            })
            .collect();
        assert_eq!(
            webp_qualities,
            vec![
                ("WebP · Compact".to_string(), 70.0, false),
                ("WebP · Balanced".to_string(), 80.0, false),
                ("WebP · High fidelity".to_string(), 90.0, false),
                ("WebP · Lossless".to_string(), 90.0, true),
            ]
        );

        // JXL: d1.0/e7 · d2.0/e8 · lossless/e7 — quality and distance are
        // mutually exclusive (never both set)
        for profile in profiles_for("jxl") {
            let EncoderConfig::Jxl(options) = &profile.encoder else {
                panic!("{} carries the wrong variant", profile.preset_title);
            };
            assert!(
                !(options.quality.is_some() && options.distance.is_some()),
                "{}: quality and distance are mutually exclusive",
                profile.preset_title
            );
            assert_eq!(
                options.intensity_target, None,
                "built-ins never pin HDR intensity"
            );
        }
        let jxl_value = |title: &str| {
            let profile = profiles_for("jxl")
                .into_iter()
                .find(|profile| profile.preset_title == title)
                .unwrap_or_else(|| panic!("missing {title}"));
            let EncoderConfig::Jxl(options) = profile.encoder.clone() else {
                unreachable!()
            };
            options
        };
        let visually = jxl_value("JXL · Visually lossless");
        assert_eq!(visually.distance, Some(1.0));
        assert_eq!(visually.effort, 7);
        assert!(!visually.lossless);
        let compact = jxl_value("JXL · Compact");
        assert_eq!(compact.distance, Some(2.0));
        assert_eq!(compact.effort, 8);
        let bit_exact = jxl_value("JXL · Bit-exact");
        assert!(bit_exact.lossless);
        assert_eq!(bit_exact.effort, 7);
        assert_eq!(bit_exact.distance, None);

        // Oxipng safe re-optimize: level 2 defaults
        let safe = profiles_for("oxipng")
            .into_iter()
            .find(|profile| profile.preset_title == "Oxipng · Safe re-optimize")
            .expect("missing safe re-optimize");
        assert_eq!(safe.encoder, EncoderConfig::Oxipng(OxipngOptions::default()));

        // alpha color mode stays auto on every AVIF profile
        for profile in profiles_for("avif") {
            let EncoderConfig::Avif(options) = &profile.encoder else {
                unreachable!()
            };
            assert_eq!(options.alpha_color_mode, None);
        }
    }

    #[test]
    fn builtin_presets_are_read_only_format_only_and_deserialize_back() {
        let builtins = builtin();
        assert_eq!(builtins.len(), profiles().len());
        for preset in &builtins {
            assert!(preset.builtin);
            assert!(preset.content.policies.is_none(), "built-ins are format-only");
            assert!(!preset.content.include_output_dir);
            assert_eq!(preset.schema, PRESET_SCHEMA);
            // the on-disk shape must stay loadable (skip_serializing_if on
            // `builtin` + `policies` must not break the round trip)
            let json = serde_json::to_string_pretty(preset).expect("serialize");
            let back: Preset = serde_json::from_str(&json).expect("deserialize back");
            assert!(!back.builtin, "builtin is never persisted");
            let mut back = back;
            back.builtin = true;
            assert_eq!(*preset, back);
        }
    }
}
