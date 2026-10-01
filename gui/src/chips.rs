//! Interim "quality ladder" chip definitions for the options panel
//! (plan 13 §2B, approach B).
//!
//! A chip is a **named, complete encoder configuration**: selecting it
//! applies the full [`EncoderConfig`]. Which chip is active is derived —
//! never stored — by pure equality against the current config
//! ([`selected_chip`]), so editing any option while a chip is active
//! unhighlights it into the "Custom…" state.
//!
//! **Interim table**: this module is a stand-in for plan 14's preset
//! model. The [`Chip`] shape mirrors a `Preset` subset (title,
//! description, encoder) plus the qualitative trade-off metadata
//! (quality dots, size/speed tags) that plan 14's profile table will
//! provide; the table below migrates verbatim into
//! `presets::builtin()` once plan 14 lands. Descriptions and tags are
//! qualitative (no benchmark runs).

use byteshaver::config::{
    ApngOptions, AvifOptions, BitDepth, CompressionType, EncoderConfig, FilterType, GifOptions,
    JxlOptions, OxipngLevel, OxipngOptions, PngOptions, WebpAnimOptions, WebpOptions,
};

/// Qualitative expected-output-size tag of a chip (relative to the other
/// chips of the same encoder; no benchmark runs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeTag {
    /// Smallest output of the encoder's ladder.
    Smallest,
    /// Noticeably smaller than typical.
    Smaller,
    /// Typical size for the encoder.
    Typical,
    /// Larger (usually because quality or losslessness wins).
    Larger,
}

impl SizeTag {
    /// Short word rendered after the `▤` icon.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            SizeTag::Smallest => "smallest",
            SizeTag::Smaller => "smaller",
            SizeTag::Typical => "typical",
            SizeTag::Larger => "larger",
        }
    }
}

/// Qualitative encode-time tag of a chip (relative to the other chips of
/// the same encoder; no benchmark runs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedTag {
    /// Faster than typical.
    Faster,
    /// The encoder's typical/default speed.
    Baseline,
    /// Slower than typical.
    Slower,
    /// Slowest encode of the encoder's ladder.
    Slowest,
}

impl SpeedTag {
    /// Short word rendered after the `⚡` icon.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            SpeedTag::Faster => "faster",
            SpeedTag::Baseline => "baseline",
            SpeedTag::Slower => "slower",
            SpeedTag::Slowest => "slowest",
        }
    }
}

/// One quality-ladder chip: a curated, complete encoder configuration with
/// presentation metadata (interim data shape, see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    /// Chip title (short, unique per encoder).
    pub title: &'static str,
    /// One-line description with the concrete values (shown under the
    /// ladder for the selected chip and as hover text).
    pub description: &'static str,
    /// Relative quality as `✦` dot count (1–5).
    pub quality_dots: u8,
    /// Qualitative size tag (rendered after `▤`).
    pub size_tag: SizeTag,
    /// Qualitative encode-time tag (rendered after `⚡`).
    pub speed_tag: SpeedTag,
    /// The complete configuration this chip applies.
    pub encoder: EncoderConfig,
}

impl Chip {
    /// The `✦✦✦ ▤ smaller ⚡ baseline` icon row of the chip card.
    #[must_use]
    pub fn icon_row(&self) -> String {
        format!(
            "{} ▤ {} ⚡ {}",
            "✦".repeat(usize::from(self.quality_dots)),
            self.size_tag.label(),
            self.speed_tag.label()
        )
    }
}

/// The quality-ladder chips of an encoder (by capability name); empty for
/// unknown names. Built-in, read-only, ordered from compact to high.
#[must_use]
pub fn chips_for(name: &str) -> Vec<Chip> {
    match name {
        "webp" => vec![
            Chip {
                title: "Compact",
                description: "q70 — smaller files with mild softening",
                quality_dots: 2,
                size_tag: SizeTag::Smallest,
                speed_tag: SpeedTag::Faster,
                encoder: EncoderConfig::Webp(WebpOptions {
                    quality: 70.0,
                    ..WebpOptions::default()
                }),
            },
            Chip {
                title: "Balanced",
                description: "q80 — good default for web photos",
                quality_dots: 3,
                size_tag: SizeTag::Smaller,
                speed_tag: SpeedTag::Baseline,
                encoder: EncoderConfig::Webp(WebpOptions {
                    quality: 80.0,
                    ..WebpOptions::default()
                }),
            },
            Chip {
                title: "High",
                description: "q90 — the CLI default quality",
                quality_dots: 4,
                size_tag: SizeTag::Typical,
                speed_tag: SpeedTag::Baseline,
                encoder: EncoderConfig::Webp(WebpOptions::default()),
            },
            Chip {
                title: "Lossless",
                description: "byte-identical output, larger files",
                quality_dots: 5,
                size_tag: SizeTag::Larger,
                speed_tag: SpeedTag::Slower,
                encoder: EncoderConfig::Webp(WebpOptions {
                    lossless: true,
                    ..WebpOptions::default()
                }),
            },
        ],
        "webp-image" => vec![Chip {
            title: "Lossless",
            description: "image-crate lossless webp (no options)",
            quality_dots: 5,
            size_tag: SizeTag::Typical,
            speed_tag: SpeedTag::Baseline,
            encoder: EncoderConfig::WebpImage,
        }],
        "avif" => vec![
            Chip {
                title: "Compact",
                description: "q50 · speed 6 · 8-bit — smallest files, for thumbnails and bulk archives",
                quality_dots: 2,
                size_tag: SizeTag::Smallest,
                speed_tag: SpeedTag::Faster,
                encoder: EncoderConfig::Avif(AvifOptions {
                    quality: 50.0,
                    speed: 6,
                    ..AvifOptions::default()
                }),
            },
            Chip {
                title: "Balanced",
                description: "q62 · speed 4 · 8-bit — web photos, roughly half the bytes of JPEG q85 at equal perception",
                quality_dots: 3,
                size_tag: SizeTag::Smaller,
                speed_tag: SpeedTag::Baseline,
                encoder: EncoderConfig::Avif(AvifOptions {
                    quality: 62.0,
                    speed: 4,
                    ..AvifOptions::default()
                }),
            },
            Chip {
                title: "High",
                description: "q78 · speed 3 · 10-bit — near-transparent quality, slowest encode",
                quality_dots: 5,
                size_tag: SizeTag::Larger,
                speed_tag: SpeedTag::Slower,
                encoder: EncoderConfig::Avif(AvifOptions {
                    quality: 78.0,
                    speed: 3,
                    bit_depth: Some(BitDepth::Ten),
                    ..AvifOptions::default()
                }),
            },
        ],
        "png" => vec![
            Chip {
                title: "Default",
                description: "encoder defaults (balanced compression)",
                quality_dots: 5,
                size_tag: SizeTag::Typical,
                speed_tag: SpeedTag::Faster,
                encoder: EncoderConfig::Png(PngOptions::default()),
            },
            Chip {
                title: "Best",
                description: "best compression + adaptive filtering — slow",
                quality_dots: 5,
                size_tag: SizeTag::Smaller,
                speed_tag: SpeedTag::Slower,
                encoder: EncoderConfig::Png(PngOptions {
                    compression_type: Some(CompressionType::Best),
                    filter_type: Some(FilterType::Adaptive),
                }),
            },
        ],
        "jpeg" => vec![Chip {
            title: "Default",
            description: "mozjpeg defaults (the encoder has no options)",
            quality_dots: 5,
            size_tag: SizeTag::Typical,
            speed_tag: SpeedTag::Baseline,
            encoder: EncoderConfig::Jpeg,
        }],
        "jxl" => vec![
            Chip {
                title: "Compact",
                description: "distance 2.0 · effort 8 — smallest, slower effort",
                quality_dots: 3,
                size_tag: SizeTag::Smallest,
                speed_tag: SpeedTag::Slower,
                encoder: EncoderConfig::Jxl(JxlOptions {
                    distance: Some(2.0),
                    effort: 8,
                    ..JxlOptions::default()
                }),
            },
            Chip {
                title: "Balanced",
                description: "distance 1.0 (visually lossless) · effort 7",
                quality_dots: 4,
                size_tag: SizeTag::Smaller,
                speed_tag: SpeedTag::Baseline,
                encoder: EncoderConfig::Jxl(JxlOptions {
                    distance: Some(1.0),
                    effort: 7,
                    ..JxlOptions::default()
                }),
            },
            Chip {
                title: "High",
                description: "lossless · effort 7 — bit-exact output",
                quality_dots: 5,
                size_tag: SizeTag::Larger,
                speed_tag: SpeedTag::Slower,
                encoder: EncoderConfig::Jxl(JxlOptions {
                    lossless: true,
                    effort: 7,
                    ..JxlOptions::default()
                }),
            },
        ],
        "oxipng" => vec![
            Chip {
                title: "Default",
                description: "level 2 preset — the CLI default",
                quality_dots: 5,
                size_tag: SizeTag::Typical,
                speed_tag: SpeedTag::Faster,
                encoder: EncoderConfig::Oxipng(OxipngOptions::default()),
            },
            Chip {
                title: "Deep",
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
        ],
        "webp-anim" => vec![
            Chip {
                title: "Compact",
                description: "q75 — smaller animations, faster encode",
                quality_dots: 2,
                size_tag: SizeTag::Smallest,
                speed_tag: SpeedTag::Faster,
                encoder: EncoderConfig::WebpAnim(WebpAnimOptions {
                    quality: 75.0,
                    ..WebpAnimOptions::default()
                }),
            },
            Chip {
                title: "Balanced",
                description: "q85 — balanced animation quality",
                quality_dots: 3,
                size_tag: SizeTag::Smaller,
                speed_tag: SpeedTag::Baseline,
                encoder: EncoderConfig::WebpAnim(WebpAnimOptions {
                    quality: 85.0,
                    ..WebpAnimOptions::default()
                }),
            },
        ],
        "apng" => vec![
            Chip {
                title: "Default",
                description: "encoder defaults (balanced compression)",
                quality_dots: 5,
                size_tag: SizeTag::Typical,
                speed_tag: SpeedTag::Faster,
                encoder: EncoderConfig::Apng(ApngOptions::default()),
            },
            Chip {
                title: "Best",
                description: "best compression + adaptive filtering — slow",
                quality_dots: 5,
                size_tag: SizeTag::Smaller,
                speed_tag: SpeedTag::Slower,
                encoder: EncoderConfig::Apng(ApngOptions {
                    compression_type: Some(CompressionType::Best),
                    filter_type: Some(FilterType::Adaptive),
                }),
            },
        ],
        "gif" => vec![Chip {
            title: "Default",
            description: "palette speed 10 (the CLI default)",
            quality_dots: 5,
            size_tag: SizeTag::Typical,
            speed_tag: SpeedTag::Baseline,
            encoder: EncoderConfig::Gif(GifOptions::default()),
        }],
        _ => Vec::new(),
    }
}

/// Which chip of the ladder (if any) exactly matches the current config —
/// `None` means the "Custom…" state. Pure equality, so editing any option
/// while a chip is active moves the state to custom.
#[must_use]
pub fn selected_chip(chips: &[Chip], encoder: &EncoderConfig) -> Option<usize> {
    chips.iter().position(|chip| &chip.encoder == encoder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{ENCODER_NAMES, encoder_kind_name};

    #[test]
    fn every_encoder_has_a_ladder_of_matching_chips() {
        for name in ENCODER_NAMES {
            let chips = chips_for(name);
            assert!(!chips.is_empty(), "{name} has no chips");
            let mut titles = Vec::new();
            for (index, chip) in chips.iter().enumerate() {
                assert_eq!(
                    encoder_kind_name(&chip.encoder),
                    name,
                    "chip '{}' of {name} carries a different encoder kind",
                    chip.title
                );
                assert!(
                    (1..=5).contains(&chip.quality_dots),
                    "chip '{}' has an out-of-range dot count",
                    chip.title
                );
                assert!(
                    !chip.description.is_empty(),
                    "chip '{}' has no description",
                    chip.title
                );
                assert_eq!(
                    selected_chip(&chips, &chip.encoder),
                    Some(index),
                    "chip '{}' must be exactly selectable",
                    chip.title
                );
                titles.push(chip.title);
            }
            assert_eq!(
                titles
                    .iter()
                    .rfind(|title| titles.iter().filter(|other| *other == *title).count() > 1),
                None,
                "{name}: duplicate chip title"
            );
            // the icon row carries all three trade-off icons
            let icons = chips[0].icon_row();
            assert!(icons.contains('✦') && icons.contains('▤') && icons.contains('⚡'));
        }
        assert!(chips_for("nope").is_empty());
    }

    #[test]
    fn editing_any_option_unhighlights_the_chip_into_custom() {
        for name in ENCODER_NAMES {
            let chips = chips_for(name);
            let mut edited = chips[0].encoder.clone();
            // one distinctive edit per encoder that no other chip matches
            match &mut edited {
                EncoderConfig::Webp(options) => options.quality += 0.5,
                EncoderConfig::WebpImage => continue,
                EncoderConfig::Avif(options) => options.quality += 0.5,
                EncoderConfig::Png(options) => options.filter_type = Some(FilterType::Up),
                EncoderConfig::Jpeg => continue,
                EncoderConfig::Jxl(options) => {
                    options.advanced.push(("brotli_effort".to_owned(), 9));
                }
                EncoderConfig::Oxipng(options) => options.zopfli_iterations += 1,
                EncoderConfig::WebpAnim(options) => options.quality += 0.5,
                EncoderConfig::Apng(options) => options.filter_type = Some(FilterType::Up),
                EncoderConfig::Gif(options) => options.speed = Some(17),
            }
            assert_eq!(
                selected_chip(&chips, &edited),
                None,
                "{name}: an edited config must land in the custom state"
            );
        }
    }
}
