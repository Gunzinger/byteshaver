//! GUI settings persistence (plan WS8 §4.5 / task §6): output directory,
//! encoder kind + options, global policies and the window size, stored as
//! JSON at `<config_dir>/byteshaver-gui/settings.json` (via the `dirs`
//! crate: XDG config on Linux, AppData on Windows, Library on macOS).
//!
//! A corrupt or unreadable file silently falls back to defaults (no panic);
//! the defaults match the CLI's argument defaults so GUI and CLI agree
//! out of the box.

use std::path::PathBuf;

use byteshaver::config::{AnimatedInputPolicy, EncoderConfig, HeifImagePolicy};
use byteshaver::metadata::policy::{ExifPolicy, parse_tag_list};
use serde::{Deserialize, Serialize};

use crate::celebrate::ConfettiLevel;

/// Which existing outputs survive a run (maps onto the CLI's
/// `--overwrite-if-smaller` / `--overwrite-existing` flag pair).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollisionChoice {
    /// Never overwrite (CLI default).
    #[default]
    KeepExisting,
    /// Overwrite only when the new encoding is smaller.
    OverwriteIfSmaller,
    /// Overwrite unconditionally.
    OverwriteAlways,
}

impl CollisionChoice {
    /// Maps the choice onto the CLI flag pair (`overwrite_if_smaller`
    /// takes precedence, matching `CollisionPolicy::from_flags`).
    #[must_use]
    pub fn flags(self) -> (bool, bool) {
        match self {
            CollisionChoice::KeepExisting => (false, false),
            CollisionChoice::OverwriteIfSmaller => (true, false),
            CollisionChoice::OverwriteAlways => (false, true),
        }
    }
}

/// UI-editable EXIF policy: the policy mode plus the raw comma-separated
/// tag selector texts of `--exif-except` / `--exif-only`.
///
/// The selector text is kept verbatim (not pre-parsed) so invalid input
/// survives round-trips through the editor; parsing happens when building
/// the job spec and surfaces as a start-blocker message.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExifSettings {
    /// Selected policy mode.
    pub mode: ExifMode,
    /// Raw `--exif-except` tag selector text (comma separated).
    pub except_tags: String,
    /// Raw `--exif-only` tag selector text (comma separated).
    pub only_tags: String,
}

/// EXIF policy modes offered by the GUI (mirrors the CLI `--exif` flag).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExifMode {
    /// Remove all EXIF (CLI default).
    #[default]
    Strip,
    /// Copy EXIF verbatim.
    Keep,
    /// Keep all EXIF except the listed tags (`--exif filter --exif-except`).
    FilterExcept,
    /// Keep only the listed tags (`--exif filter --exif-only`).
    KeepOnly,
}

impl ExifSettings {
    /// Resolves the UI state into the core's [`ExifPolicy`].
    ///
    /// # Errors
    ///
    /// Returns the CLI's parse error message when a tag selector list is
    /// malformed (the message names the offending selector, like
    /// `--exif-list-tags` suggests).
    pub fn to_policy(&self) -> Result<ExifPolicy, String> {
        match self.mode {
            ExifMode::Strip => Ok(ExifPolicy::Strip),
            ExifMode::Keep => Ok(ExifPolicy::Keep),
            ExifMode::FilterExcept => Ok(ExifPolicy::FilterExcept(parse_tag_list(&split_tags(
                &self.except_tags,
            ))?)),
            ExifMode::KeepOnly => Ok(ExifPolicy::KeepOnly(parse_tag_list(&split_tags(
                &self.only_tags,
            ))?)),
        }
    }
}

/// Splits the comma-separated editor text into non-empty trimmed selectors.
fn split_tags(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

/// Persisted GUI state (see module docs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Output directory; `None` = same directory as the input (CLI default).
    pub output_dir: Option<String>,
    /// Selected target encoder with its options (serde of the core's
    /// [`EncoderConfig`], WS7 C6).
    pub encoder: EncoderConfig,
    /// EXIF policy editor state.
    pub exif: ExifSettings,
    /// Collision/overwrite policy.
    pub collision: CollisionChoice,
    /// CLI `--animated-input` mirror.
    pub animated_input: AnimatedInputPolicy,
    /// CLI `--heif-image-policy` mirror (only effective with `dec-heif`).
    pub heif_image_policy: HeifImagePolicy,
    /// CLI `--discard-if-larger-than-input` mirror.
    pub discard_if_larger_than_input: bool,
    /// CLI `--discard-input-alpha-channel` mirror.
    pub discard_input_alpha_channel: bool,
    /// CLI `--max-animation-memory` mirror (MiB).
    pub max_animation_memory_mib: u64,
    /// CLI `--reverse-processing-order` mirror.
    pub reverse_processing_order: bool,
    /// Confetti intensity of the post-run celebration (plan 12 §3,
    /// decision D1: default Regular).
    #[serde(default)]
    pub confetti: ConfettiLevel,
    /// Reduced-motion escape hatch: steady bar shades, no time-varying
    /// painting, and the confetti replaced by a fading text line (plan
    /// 12 §2/§3; egui cannot read the OS preference portably).
    #[serde(default)]
    pub reduced_motion: bool,
    /// Whether the "Target format & options" tree of the options panel was
    /// open at the end of the last session (seeds the collapse state).
    #[serde(default = "default_true")]
    pub options_tree_open: bool,
    /// Whether the "Output & global policies" tree of the options panel
    /// was open at the end of the last session (seeds the collapse state).
    #[serde(default = "default_true")]
    pub policies_tree_open: bool,
    /// Last window size in points (restored on startup).
    pub window_size: Option<[f32; 2]>,
    /// Last report-window geometry in points as `[x, y, width, height]`
    /// (outer-rect position, inner-rect size), restored when the report
    /// viewport opens.
    #[serde(default)]
    pub report_window_geometry: Option<[f32; 4]>,
}

/// Serde default for the tree collapse states (open, like the GUI default).
fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            output_dir: None,
            encoder: EncoderConfig::Webp(Default::default()),
            exif: ExifSettings::default(),
            collision: CollisionChoice::default(),
            animated_input: AnimatedInputPolicy::default(),
            heif_image_policy: HeifImagePolicy::default(),
            discard_if_larger_than_input: false,
            discard_input_alpha_channel: false,
            max_animation_memory_mib: 4096,
            reverse_processing_order: false,
            confetti: ConfettiLevel::default(),
            reduced_motion: false,
            options_tree_open: true,
            policies_tree_open: true,
            window_size: None,
            report_window_geometry: None,
        }
    }
}

impl Settings {
    /// Path of the settings file, `None` when the OS has no config dir.
    #[must_use]
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("byteshaver-gui").join("settings.json"))
    }

    /// Loads the settings, falling back to defaults for every error
    /// (missing file, unreadable, corrupt JSON, schema drift). The corrupt
    /// file is intentionally left on disk so a downgrade can still read it.
    #[must_use]
    pub fn load_or_default() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the settings to disk (creating the directory when needed);
    /// failures are reported on stderr but never fatal.
    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        let write = || -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(
                &path,
                serde_json::to_string_pretty(self).unwrap_or_default(),
            )?;
            Ok(())
        };
        if let Err(err) = write() {
            eprintln!("byteshaver-gui: could not save settings: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_json() {
        let settings = Settings {
            output_dir: Some("/tmp/out".to_string()),
            encoder: EncoderConfig::Webp(Default::default()),
            exif: ExifSettings {
                mode: ExifMode::FilterExcept,
                except_tags: "gps,Orientation".to_string(),
                only_tags: String::new(),
            },
            collision: CollisionChoice::OverwriteIfSmaller,
            animated_input: AnimatedInputPolicy::Error,
            heif_image_policy: HeifImagePolicy::All,
            discard_if_larger_than_input: true,
            discard_input_alpha_channel: true,
            max_animation_memory_mib: 1024,
            reverse_processing_order: true,
            confetti: ConfettiLevel::Excessive,
            reduced_motion: true,
            options_tree_open: false,
            policies_tree_open: true,
            window_size: Some([1100.0, 720.0]),
            report_window_geometry: Some([12.0, 34.0, 760.0, 480.0]),
        };
        let json = serde_json::to_string(&settings).expect("serialize settings");
        let parsed: Settings = serde_json::from_str(&json).expect("deserialize settings");
        assert_eq!(parsed, settings);
    }

    #[test]
    fn settings_without_report_geometry_load_from_the_old_schema() {
        // settings files written before the report viewport existed lack
        // the field; `#[serde(default)]` must keep them loadable
        let mut json = serde_json::to_value(Settings::default()).expect("serialize defaults");
        if let Some(object) = json.as_object_mut() {
            object.remove("report_window_geometry");
        }
        let parsed: Settings = serde_json::from_value(json).expect("old schema still loads");
        assert_eq!(parsed.report_window_geometry, None);
    }

    #[test]
    fn celebration_settings_default_when_absent_from_the_file() {
        // settings written by an older build lack the plan-12 fields
        let mut value = serde_json::to_value(Settings::default()).expect("serialize defaults");
        let object = value
            .as_object_mut()
            .expect("settings serialize to an object");
        object.remove("confetti");
        object.remove("reduced_motion");
        let parsed: Settings =
            serde_json::from_value(value).expect("deserialize without the new fields");
        assert_eq!(parsed.confetti, ConfettiLevel::Regular, "decision D1");
        assert!(!parsed.reduced_motion);
        assert_eq!(parsed, Settings::default());
    }

    #[test]
    fn collapse_tree_states_default_open_when_absent_and_round_trip() {
        let mut settings = Settings::default();
        assert!(settings.options_tree_open);
        assert!(settings.policies_tree_open);
        settings.options_tree_open = false;
        settings.policies_tree_open = false;
        let json = serde_json::to_string(&settings).expect("serialize settings");
        let parsed: Settings = serde_json::from_str(&json).expect("deserialize settings");
        assert!(!parsed.options_tree_open);
        assert!(!parsed.policies_tree_open);

        // a settings file written before the fields existed defaults to open
        let mut legacy = serde_json::to_value(&settings).expect("settings value");
        let object = legacy.as_object_mut().expect("settings object");
        object.remove("options_tree_open");
        object.remove("policies_tree_open");
        let parsed: Settings = serde_json::from_value(legacy).expect("legacy settings");
        assert!(parsed.options_tree_open);
        assert!(parsed.policies_tree_open);
    }

    #[test]
    fn corrupt_settings_fall_back_to_defaults() {
        let fallback: Settings = serde_json::from_str("{ not json !!!").unwrap_or_default();
        assert_eq!(fallback, Settings::default());
        // schema drift (wrong variant shape) also falls back
        let fallback: Settings =
            serde_json::from_str("{\"encoder\": {\"NoSuchEncoder\": 1}}").unwrap_or_default();
        assert_eq!(fallback, Settings::default());
    }

    #[test]
    fn collision_choice_maps_onto_cli_flag_pair() {
        assert_eq!(CollisionChoice::KeepExisting.flags(), (false, false));
        assert_eq!(CollisionChoice::OverwriteIfSmaller.flags(), (true, false));
        assert_eq!(CollisionChoice::OverwriteAlways.flags(), (false, true));
    }

    #[test]
    fn exif_settings_resolve_like_the_cli_flag_triple() {
        let strip = ExifSettings::default();
        assert_eq!(strip.to_policy().expect("strip policy"), ExifPolicy::Strip);

        let keep = ExifSettings {
            mode: ExifMode::Keep,
            ..ExifSettings::default()
        };
        assert_eq!(keep.to_policy().expect("keep policy"), ExifPolicy::Keep);

        let except = ExifSettings {
            mode: ExifMode::FilterExcept,
            except_tags: "gps, Orientation".to_string(),
            ..ExifSettings::default()
        };
        assert!(matches!(
            except.to_policy().expect("filter policy"),
            ExifPolicy::FilterExcept(_)
        ));

        // invalid selectors surface the core's parse error instead of panicking
        let bogus = ExifSettings {
            mode: ExifMode::KeepOnly,
            only_tags: "0xz!".to_string(),
            ..ExifSettings::default()
        };
        assert!(bogus.to_policy().is_err());
    }
}
