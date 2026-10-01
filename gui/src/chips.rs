//! The quality-ladder chip ladder of the options panel (plan 13 §2B,
//! approach B) — now sourced from the built-in preset table (plan 14
//! §5/integration): every built-in profile of the selected encoder
//! becomes a chip.
//!
//! A chip is a **named, complete encoder configuration**: selecting it
//! applies the full [`EncoderConfig`] via the backing built-in preset
//! ([`Chip::preset_title`]). Which chip is active is derived — never
//! stored — by pure equality against the current config
//! ([`selected_chip`]), so editing any option while a chip is active
//! unhighlights it into the "Custom…" state.
//!
//! This module is the thin render-model layer over
//! [`crate::presets::profiles_for`]; the preset payloads live in
//! [`crate::presets::builtin`].

use byteshaver::config::EncoderConfig;

use crate::presets::profiles_for;

pub use crate::presets::{SizeTag, SpeedTag};

/// One quality-ladder chip: a curated, complete encoder configuration with
/// presentation metadata (plan 14 §5's built-in profiles in ladder shape).
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
    /// Title of the built-in preset backing this chip (the apply target;
    /// applying routes through [`crate::app::App::apply_preset`] so the
    /// active-preset tracking stays in sync).
    pub preset_title: &'static str,
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
    profiles_for(name)
        .into_iter()
        .map(|profile| Chip {
            title: profile.chip_title,
            description: profile.description,
            quality_dots: profile.quality_dots,
            size_tag: profile.size_tag,
            speed_tag: profile.speed_tag,
            encoder: profile.encoder.clone(),
            preset_title: profile.preset_title,
        })
        .collect()
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
                assert!(
                    crate::presets::profiles()
                        .iter()
                        .any(|profile| profile.preset_title == chip.preset_title),
                    "chip '{}' must be backed by a built-in preset",
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
        use byteshaver::config::FilterType;
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
