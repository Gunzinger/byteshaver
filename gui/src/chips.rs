//! The quality-ladder chip ladder of the options panel (plan 13 §2B,
//! approach B) — now sourced from the built-in preset table (plan 14
//! §5/integration): every built-in profile of the selected encoder
//! becomes a chip.
//!
//! A chip is a **named, complete encoder configuration**: selecting it
//! applies the full [`EncoderConfig`] via the backing built-in preset
//! ([`Chip::preset_title`]). Which control is active follows the plan-15
//! F1 **tri-state model** ([`ChipSelection`], computed by the pure
//! [`chip_selection`]): a matching chip highlights, an explicit click on
//! the "⚙ Custom…" card highlights Custom *even when the config still
//! equals a chip*, and a config that drifted off every chip highlights
//! Custom as the *derived* state. The explicit half lives in
//! [`crate::app::App::custom_chip_explicit`] and is cleared by every
//! preset/chip application.
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
    /// The `▤ smaller ⚡ baseline` icon row of the chip card (the ✦ quality
    /// dots are gone since plan 15 F3 — they rendered badly at card size).
    #[must_use]
    pub fn icon_row(&self) -> String {
        format!("▤ {} ⚡ {}", self.size_tag.label(), self.speed_tag.label())
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
            size_tag: profile.size_tag,
            speed_tag: profile.speed_tag,
            encoder: profile.encoder.clone(),
            preset_title: profile.preset_title,
        })
        .collect()
}

/// Which ladder control is active (plan 15 F1's tri-state selection
/// model): one chip, Custom clicked explicitly, or Custom as the derived
/// result of editing a chip's config. Only [`ChipSelection::Selected`]
/// highlights a chip card; both custom states highlight the Custom card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipSelection {
    /// The chip at this ladder index is active (config equality).
    Selected(usize),
    /// The user clicked the "⚙ Custom…" card — Custom highlights even
    /// while the config still equals a chip.
    CustomExplicit,
    /// The config drifted off every chip (a manual edit while a chip was
    /// active) — Custom highlights as the derived state.
    CustomDerived,
}

impl ChipSelection {
    /// Whether the "⚙ Custom…" card should be highlighted.
    #[must_use]
    pub fn is_custom(self) -> bool {
        matches!(self, ChipSelection::CustomExplicit | ChipSelection::CustomDerived)
    }

    /// The highlighted chip, if any.
    #[must_use]
    pub fn selected_index(self) -> Option<usize> {
        match self {
            ChipSelection::Selected(index) => Some(index),
            ChipSelection::CustomExplicit | ChipSelection::CustomDerived => None,
        }
    }
}

/// Computes the tri-state selection (pure): an explicit Custom click wins
/// over everything; otherwise a config-equal chip is selected; otherwise
/// the state is derived custom. `custom_explicit` is the flag the panel
/// sets on a Custom click and that
/// [`crate::app::App::apply_preset`] clears.
#[must_use]
pub fn chip_selection(
    chips: &[Chip],
    encoder: &EncoderConfig,
    custom_explicit: bool,
) -> ChipSelection {
    if custom_explicit {
        return ChipSelection::CustomExplicit;
    }
    match chips.iter().position(|chip| &chip.encoder == encoder) {
        Some(index) => ChipSelection::Selected(index),
        None => ChipSelection::CustomDerived,
    }
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
                    !chip.description.is_empty(),
                    "chip '{}' has no description",
                    chip.title
                );
                assert_eq!(
                    chip_selection(&chips, &chip.encoder, false),
                    ChipSelection::Selected(index),
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
            // plan 15 F3: the icon row carries the trade-off icons, no ✦
            let icons = chips[0].icon_row();
            assert!(icons.contains('▤') && icons.contains('⚡'));
            assert!(!icons.contains('✦'), "quality dots are gone");
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
            let selection = chip_selection(&chips, &edited, false);
            assert!(
                selection.is_custom(),
                "{name}: an edited config must land in the custom state"
            );
            assert_eq!(
                selection,
                ChipSelection::CustomDerived,
                "{name}: the drift state is the derived custom"
            );
            assert_eq!(selection.selected_index(), None);
        }
    }

    // ---- plan 15 F1: the tri-state selection matrix -------------------------

    #[test]
    fn clicking_custom_highlights_custom_even_while_the_config_matches_a_chip() {
        // the root cause of F1: an untouched preset config equals the chip,
        // so the old equality-only derivation never highlighted Custom
        for name in ENCODER_NAMES {
            let chips = chips_for(name);
            assert_eq!(
                chip_selection(&chips, &chips[0].encoder, true),
                ChipSelection::CustomExplicit,
                "{name}: the explicit click must win over chip equality"
            );
            assert!(chip_selection(&chips, &chips[0].encoder, true).is_custom());
        }
    }

    #[test]
    fn selecting_a_chip_rehighlights_the_chip_again() {
        for name in ENCODER_NAMES {
            let chips = chips_for(name);
            // user was in the explicit custom state, then clicks chip 0
            let selection = chip_selection(&chips, &chips[0].encoder, false);
            assert_eq!(selection, ChipSelection::Selected(0));
            assert_eq!(selection.selected_index(), Some(0));
            assert!(!selection.is_custom());
        }
    }

    #[test]
    fn the_selection_of_an_unknown_config_is_derived_custom() {
        let chips = chips_for("webp");
        let mut edited = chips[0].encoder.clone();
        if let EncoderConfig::Webp(options) = &mut edited {
            options.quality += 0.5;
        }
        // the derived state persists while the explicit flag is unset —
        // the explicit flag only ever comes from a Custom click
        assert_eq!(
            chip_selection(&chips, &edited, false),
            ChipSelection::CustomDerived
        );
        // and an empty ladder (unknown encoder) is always custom
        assert!(chip_selection(&[], &edited, false).is_custom());
    }
}
