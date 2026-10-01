//! GUI settings persistence (plan WS8 §4.5 / task §6): output directory,
//! encoder kind + options, global policies and the window size, stored as
//! JSON at `<config_dir>/byteshaver-gui/settings.json` (via the `dirs`
//! crate: XDG config on Linux, AppData on Windows, Library on macOS).
//!
//! A corrupt or unreadable file silently falls back to defaults (no panic);
//! the defaults match the CLI's argument defaults so GUI and CLI agree
//! out of the box.
//!
//! # Output-mode migration (plan 15 F16)
//!
//! Files written before plan 15 encoded "same as input" as
//! `output_dir: null`/absent and "directory" as `output_dir: "…"` (an
//! `Option<String>`). The new shape stores an explicit
//! [`OutputMode`] plus the path as a plain `String`; the custom
//! [`Deserialize`] impl of [`PolicySet`] accepts both shapes (fixture
//! tested) so old settings **and** old preset files keep loading.

use std::path::PathBuf;

use byteshaver::config::{AnimatedInputPolicy, EncoderConfig, HeifImagePolicy};
use byteshaver::metadata::policy::{ExifPolicy, parse_tag_list};
use serde::{Deserialize, Serialize};

use crate::celebrate::ConfettiLevel;
use crate::table::{ColumnState, SortKey};
use crate::thumb::ThumbMode;

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

/// Where the conversions of a run are written (plan 15 F16; maps onto the
/// CLI's `-o` flag being set or not).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputMode {
    /// Write every output next to its input (CLI default, `-o` unset).
    #[default]
    SameAsInput,
    /// Write every output into [`PolicySet::output_dir`].
    Directory,
}

/// The output/global policy mirrors shared by [`Settings`] and the
/// configuration presets (plan 14 §1): every field maps 1:1 onto a CLI
/// flag, so presets can carry them verbatim.
///
/// The output target is *mode + path* (plan 15 F16): toggling the mode in
/// the UI never clears the stored path (the old `Option<String>` encoding
/// destroyed it on every switch).
///
/// Privacy (plan 14 §2, adapted by plan 15 F16): an empty
/// [`PolicySet::output_dir`] is never serialized (`skip_serializing_if`);
/// a non-empty path is only written when a preset explicitly opts in via
/// `include_output_dir` — see `crate::presets`, which clears the field
/// before writing otherwise. The *mode* always rides along.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PolicySet {
    /// Where outputs are written (CLI `-o` set or not).
    pub output_mode: OutputMode,
    /// The output directory path (kept verbatim while the user edits, even
    /// when [`OutputMode::SameAsInput`] is selected — switching modes must
    /// not lose the text). Never serialized while empty (privacy, plan 14
    /// §2). Empty + [`OutputMode::Directory`] is a *start-blocker*
    /// (plan 15 F17), never a silent same-as-input fallback.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output_dir: String,
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
}

impl Default for PolicySet {
    fn default() -> Self {
        PolicySet {
            output_mode: OutputMode::SameAsInput,
            output_dir: String::new(),
            exif: ExifSettings::default(),
            collision: CollisionChoice::default(),
            animated_input: AnimatedInputPolicy::default(),
            heif_image_policy: HeifImagePolicy::default(),
            discard_if_larger_than_input: false,
            discard_input_alpha_channel: false,
            max_animation_memory_mib: 4096,
            reverse_processing_order: false,
        }
    }
}

impl PolicySet {
    /// Whether a concrete output directory is configured ([`OutputMode::
    /// Directory`] with a non-blank path) — the save modal's condition for
    /// offering the embed-the-path opt-in (plan 14 §2 privacy).
    #[must_use]
    pub fn has_output_directory(&self) -> bool {
        self.output_mode == OutputMode::Directory && !self.output_dir.trim().is_empty()
    }
}

impl<'de> Deserialize<'de> for PolicySet {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Every field optional so old files load: the pre-plan-15 shape
        /// has no `output_mode` and encodes the mode *in* `output_dir`
        /// (`null`/absent = same as input, a string = directory).
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct PolicySetDe {
            output_mode: Option<OutputMode>,
            output_dir: Option<String>,
            exif: Option<ExifSettings>,
            collision: Option<CollisionChoice>,
            animated_input: Option<AnimatedInputPolicy>,
            heif_image_policy: Option<HeifImagePolicy>,
            discard_if_larger_than_input: Option<bool>,
            discard_input_alpha_channel: Option<bool>,
            max_animation_memory_mib: Option<u64>,
            reverse_processing_order: Option<bool>,
        }
        let de = PolicySetDe::deserialize(deserializer)?;
        // migration (plan 15 F16): an explicit mode wins; without one the
        // legacy `Option<String>` decides (`null` and absent both mean
        // "same as input")
        let (output_mode, output_dir) = match de.output_mode {
            Some(mode) => (mode, de.output_dir.unwrap_or_default()),
            None => match de.output_dir {
                Some(dir) => (OutputMode::Directory, dir),
                None => (OutputMode::SameAsInput, String::new()),
            },
        };
        Ok(PolicySet {
            output_mode,
            output_dir,
            exif: de.exif.unwrap_or_default(),
            collision: de.collision.unwrap_or_default(),
            animated_input: de.animated_input.unwrap_or_default(),
            heif_image_policy: de.heif_image_policy.unwrap_or_default(),
            discard_if_larger_than_input: de.discard_if_larger_than_input.unwrap_or_default(),
            discard_input_alpha_channel: de.discard_input_alpha_channel.unwrap_or_default(),
            max_animation_memory_mib: de
                .max_animation_memory_mib
                .unwrap_or(PolicySet::default().max_animation_memory_mib),
            reverse_processing_order: de.reverse_processing_order.unwrap_or_default(),
        })
    }
}

/// Persisted GUI state (see module docs).
///
/// The nine policy mirrors live in the embedded [`PolicySet`] (plan 14
/// §1). Serialization writes the new shape (`"policies": {…}`); the
/// custom [`Deserialize`] impl additionally accepts the pre-plan-14
/// layout where the nine fields sit at the top level (see [`SettingsDe`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Settings {
    /// Selected target encoder with its options (serde of the core's
    /// [`EncoderConfig`], WS7 C6).
    pub encoder: EncoderConfig,
    /// Output/global policy mirrors (plan 14 §1).
    pub policies: PolicySet,
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
    /// Visible file-table columns in display order (plan 11 §1; old
    /// settings files keep the default column set).
    #[serde(default)]
    pub table_columns: ColumnState,
    /// Current file-table sort (view-only; plan 11 §1).
    #[serde(default)]
    pub table_sort: SortKey,
    /// When thumbnails are decoded (plan 11 §6; default `OnConvert`).
    #[serde(default)]
    pub thumbnails: ThumbMode,
    /// When quality metrics are computed (plan 10 §phase 2, decision D2:
    /// default `Manual` — no surprise CPU cost).
    #[serde(default)]
    pub quality_metric: crate::metrics::MetricMode,
    /// Which metric engine measures (plan 10 §phase 2, decision D1).
    #[serde(default)]
    pub metric_engine: crate::metrics::MetricEngineChoice,
    /// Longest edge metric decodes are bounded to in px (plan 10 §phase 2
    /// memory guardrail; clamped to 256..=16384 wherever it is used).
    #[serde(default = "default_metric_max_edge")]
    pub metric_max_edge: u32,
}

/// Serde default for [`Settings::metric_max_edge`].
fn default_metric_max_edge() -> u32 {
    crate::metrics::DEFAULT_METRIC_MAX_EDGE
}

/// Serde default for the tree collapse states (open, like the GUI default).
fn default_true() -> bool {
    true
}

/// Deserialization intermediate of [`Settings`]: every field optional so
/// both the current layout (nine policy mirrors embedded under
/// `"policies"`, plan 14 §1) and the pre-plan-14 layout (the nine fields
/// at the top level) load; missing fields fall back to their defaults.
#[derive(Deserialize, Default)]
#[serde(default)]
struct SettingsDe {
    encoder: Option<EncoderConfig>,
    policies: Option<PolicySet>,
    // pre-plan-14 flat layout (top-level policy mirrors)
    output_dir: Option<String>,
    exif: Option<ExifSettings>,
    collision: Option<CollisionChoice>,
    animated_input: Option<AnimatedInputPolicy>,
    heif_image_policy: Option<HeifImagePolicy>,
    discard_if_larger_than_input: Option<bool>,
    discard_input_alpha_channel: Option<bool>,
    max_animation_memory_mib: Option<u64>,
    reverse_processing_order: Option<bool>,
    // everything after the policy block (unchanged by the refactor)
    confetti: Option<ConfettiLevel>,
    reduced_motion: Option<bool>,
    options_tree_open: Option<bool>,
    policies_tree_open: Option<bool>,
    window_size: Option<[f32; 2]>,
    report_window_geometry: Option<[f32; 4]>,
    table_columns: Option<ColumnState>,
    table_sort: Option<SortKey>,
    thumbnails: Option<ThumbMode>,
    quality_metric: Option<crate::metrics::MetricMode>,
    metric_engine: Option<crate::metrics::MetricEngineChoice>,
    metric_max_edge: Option<u32>,
}

impl From<SettingsDe> for Settings {
    fn from(de: SettingsDe) -> Self {
        // an old file carries the nine mirrors at the top level; a file
        // with an explicit `policies` object wins over any stray flat
        // fields (both shapes never coexist in practice)
        let policies = de.policies.unwrap_or_else(|| {
            // legacy flat layout (plan 15 F16 migration): the pre-plan-14
            // `output_dir` encodes the mode — `Some(path)` → directory
            let mut legacy = PolicySet::default();
            if let Some(dir) = de.output_dir {
                legacy.output_mode = OutputMode::Directory;
                legacy.output_dir = dir;
            }
            PolicySet {
                output_mode: legacy.output_mode,
                output_dir: legacy.output_dir,
                exif: de.exif.unwrap_or_default(),
                collision: de.collision.unwrap_or_default(),
                animated_input: de.animated_input.unwrap_or_default(),
                heif_image_policy: de.heif_image_policy.unwrap_or_default(),
                discard_if_larger_than_input: de.discard_if_larger_than_input.unwrap_or_default(),
                discard_input_alpha_channel: de.discard_input_alpha_channel.unwrap_or_default(),
                max_animation_memory_mib: de
                    .max_animation_memory_mib
                    .unwrap_or(PolicySet::default().max_animation_memory_mib),
                reverse_processing_order: de.reverse_processing_order.unwrap_or_default(),
            }
        });
        Settings {
            encoder: de
                .encoder
                .unwrap_or_else(|| EncoderConfig::Webp(Default::default())),
            policies,
            confetti: de.confetti.unwrap_or_default(),
            reduced_motion: de.reduced_motion.unwrap_or_default(),
            options_tree_open: de.options_tree_open.unwrap_or_else(default_true),
            policies_tree_open: de.policies_tree_open.unwrap_or_else(default_true),
            window_size: de.window_size,
            report_window_geometry: de.report_window_geometry,
            table_columns: de.table_columns.unwrap_or_default(),
            table_sort: de.table_sort.unwrap_or_default(),
            thumbnails: de.thumbnails.unwrap_or_default(),
            quality_metric: de.quality_metric.unwrap_or_default(),
            metric_engine: de.metric_engine.unwrap_or_default(),
            metric_max_edge: de
                .metric_max_edge
                .unwrap_or_else(default_metric_max_edge),
        }
    }
}

impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(SettingsDe::deserialize(deserializer)?.into())
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            encoder: EncoderConfig::Webp(Default::default()),
            policies: PolicySet::default(),
            confetti: ConfettiLevel::default(),
            reduced_motion: false,
            options_tree_open: true,
            policies_tree_open: true,
            window_size: None,
            report_window_geometry: None,
            table_columns: ColumnState::default(),
            table_sort: SortKey::default(),
            thumbnails: ThumbMode::default(),
            quality_metric: crate::metrics::MetricMode::default(),
            metric_engine: crate::metrics::MetricEngineChoice::default(),
            metric_max_edge: crate::metrics::DEFAULT_METRIC_MAX_EDGE,
        }
    }
}

impl Settings {
    /// Path of the settings file, `None` when the OS has no config dir.
    #[must_use]
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("byteshaver-gui").join("settings.json"))
    }

    /// The metric decode edge clamped to the sane range (plan 10 §phase 2:
    /// a hand-edited settings file cannot defeat the memory guardrail).
    #[must_use]
    pub fn metric_max_edge_clamped(&self) -> u32 {
        crate::metrics::clamp_metric_max_edge(self.metric_max_edge)
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
            encoder: EncoderConfig::Webp(Default::default()),
            policies: PolicySet {
                output_mode: OutputMode::Directory,
                output_dir: "/tmp/out".to_string(),
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
            },
            confetti: ConfettiLevel::Excessive,
            reduced_motion: true,
            options_tree_open: false,
            policies_tree_open: true,
            window_size: Some([1100.0, 720.0]),
            report_window_geometry: Some([12.0, 34.0, 760.0, 480.0]),
            table_columns: crate::table::ColumnState {
                visible: vec![
                    crate::table::Column::Status,
                    crate::table::Column::Name,
                    crate::table::Column::Dimensions,
                    crate::table::Column::Actions,
                ],
            },
            table_sort: crate::table::SortKey::Asc(crate::table::Column::Modified),
            thumbnails: crate::thumb::ThumbMode::OnAdd,
            quality_metric: crate::metrics::MetricMode::AutoAfterRun,
            metric_engine: crate::metrics::MetricEngineChoice::Psnr,
            metric_max_edge: 8192,
        };
        let json = serde_json::to_string(&settings).expect("serialize settings");
        let parsed: Settings = serde_json::from_str(&json).expect("deserialize settings");
        assert_eq!(parsed, settings);
    }

    // ---- plan 14 §1: PolicySet extraction + layout migration -----------------

    #[test]
    fn pre_policieset_settings_files_load_from_the_flat_layout() {
        // settings written before plan 14 carry the nine policy mirrors at
        // the top level; they must migrate into the embedded PolicySet
        let old = serde_json::json!({
            "output_dir": "/tmp/legacy-out",
            "encoder": { "Webp": { "lossless": false, "quality": 80.0 } },
            "exif": { "mode": "FilterExcept", "except_tags": "gps", "only_tags": "" },
            "collision": "OverwriteIfSmaller",
            "animated_input": "Error",
            "heif_image_policy": "All",
            "discard_if_larger_than_input": true,
            "discard_input_alpha_channel": true,
            "max_animation_memory_mib": 512,
            "reverse_processing_order": true,
            "reduced_motion": true,
            "window_size": [1100.0, 720.0],
        });
        let parsed: Settings = serde_json::from_value(old).expect("flat layout loads");
        assert_eq!(parsed.policies.output_mode, OutputMode::Directory);
        assert_eq!(parsed.policies.output_dir, "/tmp/legacy-out");
        assert_eq!(parsed.policies.exif.mode, ExifMode::FilterExcept);
        assert_eq!(parsed.policies.exif.except_tags, "gps");
        assert_eq!(parsed.policies.collision, CollisionChoice::OverwriteIfSmaller);
        assert_eq!(
            parsed.policies.animated_input,
            AnimatedInputPolicy::Error
        );
        assert_eq!(parsed.policies.heif_image_policy, HeifImagePolicy::All);
        assert!(parsed.policies.discard_if_larger_than_input);
        assert!(parsed.policies.discard_input_alpha_channel);
        assert_eq!(parsed.policies.max_animation_memory_mib, 512);
        assert!(parsed.policies.reverse_processing_order);
        assert!(parsed.reduced_motion, "non-policy fields migrate too");
        assert_eq!(
            parsed.encoder,
            EncoderConfig::Webp(byteshaver::config::WebpOptions {
                lossless: false,
                quality: 80.0
            })
        );
    }

    #[test]
    fn flat_layout_defaults_apply_where_the_file_is_silent() {
        // an old file missing most fields still loads with defaults
        let old = serde_json::json!({
            "encoder": { "Jpeg": null },
        });
        let parsed: Settings = serde_json::from_value(old).expect("minimal flat file loads");
        assert_eq!(parsed.policies, PolicySet::default());
    }

    #[test]
    fn embedded_policies_shape_serializes_and_wins_over_flat_fields() {
        let mut settings = Settings::default();
        settings.policies.output_mode = OutputMode::Directory;
        settings.policies.output_dir = "/tmp/new-shape".to_string();
        settings.policies.collision = CollisionChoice::OverwriteAlways;
        let value = serde_json::to_value(&settings).expect("serialize new shape");
        let object = value.as_object().expect("settings object");
        assert!(
            object.contains_key("policies"),
            "the embedded policy object is the new on-disk shape"
        );
        assert!(
            !object.contains_key("output_dir"),
            "the nine mirrors no longer sit at the top level"
        );
        let policies_object = object["policies"].as_object().expect("policy object");
        assert!(
            policies_object.contains_key("output_mode"),
            "the mode is persisted explicitly (plan 15 F16)"
        );
        assert_eq!(policies_object["output_mode"], "Directory");
        assert_eq!(policies_object["output_dir"], "/tmp/new-shape");
        // an empty output_dir is skipped on serialization (privacy, plan 14 §2)
        let defaults = serde_json::to_value(Settings::default()).expect("serialize defaults");
        let default_policies = defaults["policies"].as_object().expect("policy object");
        assert!(
            !default_policies.contains_key("output_dir"),
            "no path is written while unset"
        );
        assert_eq!(default_policies["output_mode"], "SameAsInput");
        let parsed: Settings = serde_json::from_value(value).expect("new shape loads");
        assert_eq!(parsed, settings);

        // a hypothetical file carrying both shapes: embedded wins
        let mut both = serde_json::to_value(&settings).expect("new shape");
        both["output_dir"] = serde_json::json!("/tmp/stale-flat");
        let parsed: Settings = serde_json::from_value(both).expect("both shapes load");
        assert_eq!(parsed.policies.output_dir, "/tmp/new-shape");
        assert_eq!(parsed.policies.output_mode, OutputMode::Directory);
    }

    // ---- plan 15 F16: output-mode migration fixtures -------------------------

    #[test]
    fn legacy_output_dir_string_migrates_to_directory_mode() {
        // old settings: a `Some(path)` in the embedded policy object
        let old = serde_json::json!({
            "encoder": { "Jpeg": null },
            "policies": {
                "output_dir": "/tmp/old-out",
                "collision": "KeepExisting",
            },
        });
        let parsed: Settings = serde_json::from_value(old).expect("old embedded shape loads");
        assert_eq!(parsed.policies.output_mode, OutputMode::Directory);
        assert_eq!(parsed.policies.output_dir, "/tmp/old-out");
    }

    #[test]
    fn legacy_absent_and_null_output_dirs_migrate_to_same_as_input() {
        // old settings: `None` was skipped entirely on serialization …
        let absent = serde_json::json!({
            "encoder": { "Jpeg": null },
            "policies": { "collision": "OverwriteAlways" },
        });
        let parsed: Settings = serde_json::from_value(absent).expect("absent output_dir");
        assert_eq!(parsed.policies.output_mode, OutputMode::SameAsInput);
        assert_eq!(parsed.policies.output_dir, "");

        // … and a hand-edited file may carry an explicit `null`
        let null = serde_json::json!({
            "encoder": { "Jpeg": null },
            "policies": { "output_dir": null },
        });
        let parsed: Settings = serde_json::from_value(null).expect("null output_dir");
        assert_eq!(parsed.policies.output_mode, OutputMode::SameAsInput);
        assert_eq!(parsed.policies.output_dir, "");
    }

    #[test]
    fn legacy_flat_output_dir_string_migrates_to_directory_mode() {
        // pre-plan-14 flat layout with a set output directory
        let old = serde_json::json!({
            "encoder": { "Jpeg": null },
            "output_dir": "/tmp/flat-out",
        });
        let parsed: Settings = serde_json::from_value(old).expect("flat legacy loads");
        assert_eq!(parsed.policies.output_mode, OutputMode::Directory);
        assert_eq!(parsed.policies.output_dir, "/tmp/flat-out");

        // pre-plan-14 flat layout with the field absent (the None skip)
        let old = serde_json::json!({ "encoder": { "Jpeg": null } });
        let parsed: Settings = serde_json::from_value(old).expect("flat legacy loads");
        assert_eq!(parsed.policies.output_mode, OutputMode::SameAsInput);
    }

    #[test]
    fn both_output_modes_round_trip_through_the_new_shape() {
        for (mode, dir) in [
            (OutputMode::SameAsInput, ""),
            (OutputMode::SameAsInput, "/kept-while-same-as-input"),
            (OutputMode::Directory, ""),
            (OutputMode::Directory, "/tmp/real-dir"),
        ] {
            let mut settings = Settings::default();
            settings.policies.output_mode = mode;
            settings.policies.output_dir = dir.to_string();
            let json = serde_json::to_string(&settings).expect("serialize");
            let parsed: Settings = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed.policies.output_mode, mode, "mode survives: {json}");
            assert_eq!(parsed.policies.output_dir, dir, "path survives: {json}");
        }
        // the Directory+empty state (plan 15 F17 blocker) round trips too —
        // the mode is explicit so it must not degrade to same-as-input
        let mut blocked = Settings::default();
        blocked.policies.output_mode = OutputMode::Directory;
        let json = serde_json::to_string(&blocked).expect("serialize");
        let parsed: Settings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.policies.output_mode, OutputMode::Directory);
        assert!(!json.contains("output_dir"), "empty path stays private");
    }

    #[test]
    fn legacy_preset_policy_files_migrate_the_output_mode() {
        // a preset file written before plan 15 embeds the legacy Option
        let json = r#"{
            "schema": 1, "title": "Legacy Out", "description": "",
            "created_unix": 0, "modified_unix": 0, "core_version": "0.1.0",
            "content": {
                "encoder": { "Jpeg": null },
                "policies": { "output_dir": "/preset/dir", "collision": "KeepExisting" },
                "include_output_dir": true
            }
        }"#;
        let parsed: crate::presets::Preset = serde_json::from_str(json).expect("legacy preset");
        let policies = parsed.content.policies.as_ref().expect("policies");
        assert_eq!(policies.output_mode, OutputMode::Directory);
        assert_eq!(policies.output_dir, "/preset/dir");

        // … and the privacy-stripped shape (`output_dir` absent/null) too
        let json = r#"{
            "schema": 1, "title": "Legacy Stripped", "description": "",
            "created_unix": 0, "modified_unix": 0, "core_version": "0.1.0",
            "content": {
                "encoder": { "Jpeg": null },
                "policies": { "collision": "OverwriteAlways" },
                "include_output_dir": false
            }
        }"#;
        let parsed: crate::presets::Preset = serde_json::from_str(json).expect("legacy preset");
        let policies = parsed.content.policies.as_ref().expect("policies");
        assert_eq!(policies.output_mode, OutputMode::SameAsInput);
        assert_eq!(policies.output_dir, "");
    }

    #[test]
    fn policy_set_defaults_match_the_cli_defaults() {
        let policies = PolicySet::default();
        assert_eq!(policies.output_mode, OutputMode::SameAsInput);
        assert_eq!(policies.output_dir, "");
        assert!(!policies.has_output_directory());
        assert_eq!(policies.collision, CollisionChoice::KeepExisting);
        assert_eq!(
            policies.animated_input,
            AnimatedInputPolicy::FirstFrame
        );
        assert_eq!(policies.heif_image_policy, HeifImagePolicy::Primary);
        assert!(!policies.discard_if_larger_than_input);
        assert!(!policies.discard_input_alpha_channel);
        assert_eq!(policies.max_animation_memory_mib, 4096);
        assert!(!policies.reverse_processing_order);
        assert_eq!(Settings::default().policies, policies);
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
    fn table_settings_default_when_absent_from_the_file() {
        // settings written before plan 11 lack the file-table fields
        let mut value = serde_json::to_value(Settings::default()).expect("serialize defaults");
        let object = value
            .as_object_mut()
            .expect("settings serialize to an object");
        object.remove("table_columns");
        object.remove("table_sort");
        object.remove("thumbnails");
        let parsed: Settings =
            serde_json::from_value(value).expect("deserialize without the new fields");
        assert_eq!(parsed.table_columns, crate::table::ColumnState::default());
        assert_eq!(parsed.table_sort, crate::table::SortKey::None);
        assert_eq!(parsed.thumbnails, crate::thumb::ThumbMode::OnConvert);
        assert_eq!(parsed, Settings::default());
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
    fn collapse_tree_states_default_open_when_absent_and_round_trip() {        let mut settings = Settings::default();
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

    // ---- plan 10 §phase 2: metric settings ----------------------------------

    #[test]
    fn metric_settings_default_to_manual_dssim_4096_and_round_trip() {
        let settings = Settings::default();
        assert_eq!(settings.quality_metric, crate::metrics::MetricMode::Manual, "decision D2");
        assert_eq!(
            settings.metric_engine,
            crate::metrics::MetricEngineChoice::Dssim,
            "decision D1"
        );
        assert_eq!(settings.metric_max_edge, 4096);
        assert_eq!(settings.metric_max_edge_clamped(), 4096);

        let settings = Settings {
            quality_metric: crate::metrics::MetricMode::AutoAfterRun,
            metric_engine: crate::metrics::MetricEngineChoice::Psnr,
            metric_max_edge: 8192,
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).expect("serialize settings");
        let parsed: Settings = serde_json::from_str(&json).expect("deserialize settings");
        assert_eq!(parsed, settings);
    }

    #[test]
    fn metric_settings_default_when_absent_from_the_file() {
        // settings written before plan 10 lack the three fields
        let mut value = serde_json::to_value(Settings::default()).expect("serialize defaults");
        let object = value
            .as_object_mut()
            .expect("settings serialize to an object");
        object.remove("quality_metric");
        object.remove("metric_engine");
        object.remove("metric_max_edge");
        let parsed: Settings =
            serde_json::from_value(value).expect("deserialize without the new fields");
        assert_eq!(parsed, Settings::default());
        assert_eq!(parsed.quality_metric, crate::metrics::MetricMode::Manual);
        assert_eq!(parsed.metric_engine, crate::metrics::MetricEngineChoice::Dssim);
        assert_eq!(parsed.metric_max_edge, 4096);
    }

    #[test]
    fn hand_edited_metric_edge_is_clamped() {
        // a huge value in the file stays stored but is clamped at use
        let mut value =
            serde_json::to_value(Settings::default()).expect("serialize defaults");
        value["metric_max_edge"] = serde_json::json!(1_000_000);
        let parsed: Settings = serde_json::from_value(value).expect("deserialize");
        assert_eq!(parsed.metric_max_edge, 1_000_000, "stored verbatim");
        assert_eq!(
            parsed.metric_max_edge_clamped(),
            16_384,
            "used clamped (guardrail)"
        );
    }
}
