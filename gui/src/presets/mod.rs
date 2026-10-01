//! Configuration presets (plan 14): a user-defined or built-in bundle of a
//! target format (with its full option surface) plus — optionally — the
//! output/global policy mirrors of [`crate::settings::PolicySet`].
//!
//! The module is **pure logic + file io, no egui**: the serde model
//! ([`Preset`]), the filename [`slugify`], validation, title dedup on
//! import, the per-file preset [`Store`] under
//! `<config>/byteshaver-gui/presets/` and the apply-path helpers
//! ([`apply_preview`], [`preset_matches`]). Rendering lives in the options
//! panel; the built-in table in [`crate::presets::builtin`].
//!
//! # File format & compatibility (plan 14 §2)
//!
//! One JSON file per preset, `schema` first. Loading isolates errors per
//! file: a corrupt or structurally unknown file (e.g. written by a build
//! with a future/removed encoder variant) never blocks the rest — it lands
//! in the [`StoreContents::unreadable`] list (kept with its raw JSON) that
//! the manage window shows grayed with the reason. Files with
//! `schema > PRESET_SCHEMA` parse structurally and stay visible read-only
//! (the "newer format" badge is derived via [`is_newer_format`]).
//!
//! # Privacy (plan 14 §2)
//!
//! `include_output_dir == false` (the default) → the preset's
//! `policies.output_dir` is **stripped before any write** ([`sanitized`];
//! the `Option` is also `skip_serializing_if`'d while unset), so neither
//! the user-dir file nor an export ever embeds a local path unless the
//! user explicitly opts in. Applying such a preset leaves the local output
//! directory untouched ([`preset_matches`] ignores the field accordingly).

mod builtin;

// nameability re-export (the profile type appears in the `profiles`
// signatures); nothing else names the path
#[allow(unused_imports)]
pub use builtin::BuiltinProfile;
pub use builtin::{builtin, profiles, profiles_for};

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use byteshaver::config::EncoderConfig;

use crate::settings::PolicySet;

/// On-disk schema version of the preset files this build writes.
pub const PRESET_SCHEMA: u32 = 1;

/// Maximum (trimmed) length of a preset title.
pub const TITLE_MAX_CHARS: usize = 64;
/// Maximum length of a preset description.
pub const DESCRIPTION_MAX_CHARS: usize = 280;
/// Maximum length of a filename slug.
pub const SLUG_MAX_CHARS: usize = 48;
/// Second extension of exported preset files (`<slug>.byteshaver-preset.
/// json`, plan 14 decision 3: recognizable *and* editor-openable).
pub const EXPORT_EXTENSION: &str = "byteshaver-preset.json";

/// A saved configuration: target format + optional output/global policies
/// under a title and description (plan 14 §1).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Preset {
    /// On-disk schema version (== [`PRESET_SCHEMA`] for files this build
    /// writes; a higher value marks a future file, see [`is_newer_format`]).
    pub schema: u32,
    /// Human title (1..=[`TITLE_MAX_CHARS`] chars after trimming, unique
    /// among the *user* presets; built-ins are read-only).
    pub title: String,
    /// One-line rationale, may be empty (≤ [`DESCRIPTION_MAX_CHARS`]).
    pub description: String,
    /// Creation time (unix seconds; `0` for built-ins).
    pub created_unix: u64,
    /// Last modification time (unix seconds).
    pub modified_unix: u64,
    /// Version of the core the preset was authored with.
    pub core_version: String,
    /// Whether this is a compiled-in built-in profile (read-only, never
    /// persisted — always skipped on serialization, defaults to `false`).
    #[serde(default, skip_serializing_if = "never_persisted")]
    pub builtin: bool,
    /// The saved configuration.
    pub content: PresetContent,
}

/// The configuration payload of a [`Preset`] (plan 14 §1).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PresetContent {
    /// Target encoder with its full option surface (core serde, verified
    /// round-trippable).
    pub encoder: EncoderConfig,
    /// Output/global policies when the preset is in "full" scope; `None`
    /// for format-only presets (applying leaves the policies untouched).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policies: Option<PolicySet>,
    /// Whether [`PresetContent::policies`] may embed the local output
    /// directory path. `false` (the default) → the path is stripped at
    /// write time (privacy, see the module docs).
    #[serde(default)]
    pub include_output_dir: bool,
}

/// `skip_serializing_if` helper: [`Preset::builtin`] is never written to
/// disk — built-ins are identified by living in the binary, user files
/// carry the `false` default via `#[serde(default)]`.
fn never_persisted(_: &bool) -> bool {
    true
}

/// Qualitative expected-output-size tag of a built-in profile (relative to
/// the other profiles of the same encoder; no benchmark runs).
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

/// Qualitative encode-time tag of a built-in profile (relative to the
/// other profiles of the same encoder; no benchmark runs).
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

// ---- slug / titles -----------------------------------------------------------

/// Maps a preset title onto a filename slug: lowercased ASCII, only
/// `[a-z0-9-_]`, runs of whitespace/separators collapsed, at most
/// [`SLUG_MAX_CHARS`] chars; non-ASCII characters are dropped. When
/// nothing survives (e.g. a purely non-ASCII title) the slug falls back to
/// `preset-<fnv1a hash>` so every title yields a stable, path-safe name.
#[must_use]
pub fn slugify(title: &str) -> String {
    let mut slug = String::new();
    for ch in title.chars() {
        let mapped = if ch.is_ascii_uppercase() {
            ch.to_ascii_lowercase()
        } else if ch.is_whitespace() {
            '-'
        } else {
            ch
        };
        let allowed =
            mapped.is_ascii_lowercase() || mapped.is_ascii_digit() || mapped == '-' || mapped == '_';
        if !allowed || (mapped == '-' && (slug.is_empty() || slug.ends_with('-'))) {
            continue;
        }
        if slug.len() >= SLUG_MAX_CHARS {
            break;
        }
        slug.push(mapped);
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        format!("preset-{:08x}", fnv1a(title))
    } else {
        slug
    }
}

/// FNV-1a (32-bit) — a tiny stable hash for the empty-slug fallback (std's
/// `DefaultHasher` is not guaranteed stable across toolchain releases, and
/// a slug must not change between builds of the app).
fn fnv1a(text: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Structural validation of a preset (title/description bounds; the schema
/// and encoder surface are validated by serde on load).
///
/// # Errors
///
/// A message naming the violated bound.
pub fn validate(preset: &Preset) -> Result<(), String> {
    let title = preset.title.trim();
    if title.is_empty() {
        return Err("the preset title is empty".to_string());
    }
    if title.chars().count() > TITLE_MAX_CHARS {
        return Err(format!(
            "the preset title is longer than {TITLE_MAX_CHARS} characters"
        ));
    }
    if preset.description.chars().count() > DESCRIPTION_MAX_CHARS {
        return Err(format!(
            "the description is longer than {DESCRIPTION_MAX_CHARS} characters"
        ));
    }
    Ok(())
}

/// Validation of the save dialog's draft fields (live, before building).
///
/// # Errors
///
/// "title required" / "too long" / "already exists" — rendered inline.
pub fn validate_draft(
    title: &str,
    description: &str,
    existing_titles: &[String],
) -> Result<(), String> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err("a title is required".to_string());
    }
    if trimmed.chars().count() > TITLE_MAX_CHARS {
        return Err(format!(
            "the title is longer than {TITLE_MAX_CHARS} characters"
        ));
    }
    if description.chars().count() > DESCRIPTION_MAX_CHARS {
        return Err(format!(
            "the description is longer than {DESCRIPTION_MAX_CHARS} characters"
        ));
    }
    if existing_titles.iter().any(|existing| existing == trimmed) {
        return Err(format!("a preset named {trimmed:?} already exists"));
    }
    Ok(())
}

/// Title of an imported preset that would collide with an existing one:
/// an `"(imported)"` suffix keeps both copies (silently overwriting a
/// user's preset is data loss, plan 14 §2), counted when needed.
#[must_use]
pub fn dedup_import_title(title: &str, existing: &[String]) -> String {
    if !existing.iter().any(|existing| existing == title) {
        return title.to_string();
    }
    let base = format!("{title} (imported)");
    let mut candidate = base.clone();
    let mut counter = 1;
    while existing.contains(&candidate) {
        counter += 1;
        candidate = format!("{base} {counter}");
    }
    candidate
}

/// Current unix time in seconds (`0` if the clock is before the epoch).
#[must_use]
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Builds a user preset from the save dialog's fields (pure): the scope
/// toggle decides whether policies ride along; the output directory is
/// only embedded on explicit opt-in (see [`sanitized`]).
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_preset(
    title: &str,
    description: &str,
    include_policies: bool,
    include_output_dir: bool,
    encoder: &EncoderConfig,
    policies: &PolicySet,
) -> Preset {
    let now = now_unix();
    let mut saved_policies = include_policies.then(|| policies.clone());
    if let Some(saved) = &mut saved_policies
        && !include_output_dir
    {
        saved.output_dir = None;
    }
    Preset {
        schema: PRESET_SCHEMA,
        title: title.trim().to_string(),
        description: description.trim().to_string(),
        created_unix: now,
        modified_unix: now,
        core_version: crate::app::CORE_VERSION.to_string(),
        builtin: false,
        content: PresetContent {
            encoder: encoder.clone(),
            policies: saved_policies,
            include_output_dir: include_policies && include_output_dir,
        },
    }
}

/// Privacy strip (plan 14 §2): the exact variant that may be written to
/// disk or exported — `builtin` forced `false` and the output directory
/// removed unless `include_output_dir` opts in. Idempotent.
#[must_use]
pub fn sanitized(preset: &Preset) -> Preset {
    let mut copy = preset.clone();
    copy.builtin = false;
    if !copy.content.include_output_dir && let Some(policies) = &mut copy.content.policies {
        policies.output_dir = None;
    }
    copy
}

/// The pretty JSON of an export (sanitized; see [`sanitized`]).
#[must_use]
pub fn export_json(preset: &Preset) -> String {
    serde_json::to_string_pretty(&sanitized(preset)).unwrap_or_default()
}

/// Suggested export file name: `<slug>.byteshaver-preset.json`.
#[must_use]
pub fn export_file_name(preset: &Preset) -> String {
    format!("{}.{}", slugify(&preset.title), EXPORT_EXTENSION)
}

/// Parses the text of an imported preset file (serde + structural
/// validation); imported presets are always user presets (`builtin`
/// forced off — a forged `builtin: true` must not become read-only).
///
/// # Errors
///
/// The serde or validation error message.
pub fn parse_preset_text(text: &str) -> Result<Preset, String> {
    let mut preset: Preset =
        serde_json::from_str(text).map_err(|err| format!("not a readable preset: {err}"))?;
    preset.builtin = false;
    validate(&preset)?;
    Ok(preset)
}

// ---- apply path (pure) -------------------------------------------------------

/// Whether a preset is exactly active for the given state: the encoder
/// must be equal, and — for full-scope presets — the policies too. The
/// output directory is ignored when the preset did not embed it (privacy
/// exclusions must not read as "modified").
#[must_use]
pub fn preset_matches(
    preset: &Preset,
    encoder: &EncoderConfig,
    policies: &PolicySet,
) -> bool {
    if preset.content.encoder != *encoder {
        return false;
    }
    let Some(saved) = &preset.content.policies else {
        return true;
    };
    if preset.content.include_output_dir {
        saved == policies
    } else {
        let mut without_output = saved.clone();
        without_output.output_dir = None;
        let mut current = policies.clone();
        current.output_dir = None;
        without_output == current
    }
}

/// The dropdown's apply-preview line (plan 14 §Risks): presets must never
/// apply policies the user forgot were embedded. Format-only presets
/// announce "applies format only"; full presets count how many of the
/// nine mirrors actually differ from the defaults.
#[must_use]
pub fn apply_preview(preset: &Preset) -> String {
    match &preset.content.policies {
        None => "applies format only".to_string(),
        Some(policies) => format!(
            "applies format + {} policies",
            policy_change_count(policies)
        ),
    }
}

/// How many of the nine policy mirrors differ from the defaults (the
/// meaningful payload size of a full-scope preset; also the default of the
/// save dialog's scope toggle — decision 1: full preset only when the
/// policies actually differ).
#[must_use]
pub fn policy_change_count(policies: &PolicySet) -> usize {
    let defaults = PolicySet::default();
    let mut changes = 0;
    if policies.output_dir != defaults.output_dir {
        changes += 1;
    }
    if policies.exif != defaults.exif {
        changes += 1;
    }
    if policies.collision != defaults.collision {
        changes += 1;
    }
    if policies.animated_input != defaults.animated_input {
        changes += 1;
    }
    if policies.heif_image_policy != defaults.heif_image_policy {
        changes += 1;
    }
    if policies.discard_if_larger_than_input != defaults.discard_if_larger_than_input {
        changes += 1;
    }
    if policies.discard_input_alpha_channel != defaults.discard_input_alpha_channel {
        changes += 1;
    }
    if policies.max_animation_memory_mib != defaults.max_animation_memory_mib {
        changes += 1;
    }
    if policies.reverse_processing_order != defaults.reverse_processing_order {
        changes += 1;
    }
    changes
}

/// Identity of the currently active preset (for the dropdown label and
/// the manage window's highlight).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresetRef {
    /// Index into the built-in table ([`builtin()`], stable per build).
    Builtin(usize),
    /// Title of a user preset (titles are the authoritative identity, §2).
    User(String),
}

/// Resolves a [`PresetRef`] against the built-in table and the loaded user
/// presets.
#[must_use]
pub fn find_preset<'a>(
    builtins: &'a [Preset],
    user: &'a [StoredPreset],
    reference: &PresetRef,
) -> Option<&'a Preset> {
    match reference {
        PresetRef::Builtin(index) => builtins.get(*index),
        PresetRef::User(title) => user
            .iter()
            .find(|stored| stored.preset.title == *title)
            .map(|stored| &stored.preset),
    }
}

/// `true` when the file was written by a *newer* app generation: the
/// preset stays visible but read-only (badge "newer format").
#[must_use]
pub fn is_newer_format(preset: &Preset) -> bool {
    preset.schema > PRESET_SCHEMA
}

/// `true` when the preset was authored by a different core version
/// (yellow warning badge + tooltip; presets still apply structurally).
#[must_use]
pub fn is_foreign_version(preset: &Preset) -> bool {
    preset.core_version != crate::app::CORE_VERSION
}

/// `YYYY-MM-DD` of a unix timestamp (the manage window's "modified"
/// column; no chrono dependency — Howard Hinnant's `civil_from_days`).
#[must_use]
pub fn format_date(unix: u64) -> String {
    let days = i64::try_from(unix / 86_400).unwrap_or(0) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

// ---- persistence (one file per preset, plan 14 §2) ---------------------------

/// One parseable preset loaded from the user directory.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredPreset {
    /// The parsed preset (`builtin` is always `false` here).
    pub preset: Preset,
    /// File name inside the store directory (delete/rename target).
    pub file_name: String,
}

/// A file in the store directory that could not be parsed — surfaced in
/// the manage window grayed with its reason (never blocks the rest).
#[derive(Clone, Debug)]
pub struct Unreadable {
    /// File name inside the store directory.
    pub file_name: String,
    /// Human-readable reason (serde error, bad JSON, …).
    pub reason: String,
    /// The raw JSON when the file *was* valid JSON but the preset model
    /// rejected it (e.g. an unknown encoder variant from a future build).
    pub raw_json: Option<serde_json::Value>,
}

/// Everything [`Store::load_all`] found on disk.
#[derive(Clone, Debug, Default)]
pub struct StoreContents {
    /// Parseable presets, sorted by file name.
    pub presets: Vec<StoredPreset>,
    /// Files that could not be parsed, sorted by file name.
    pub unreadable: Vec<Unreadable>,
}

/// The per-preset JSON store under
/// `<config>/byteshaver-gui/presets/` (plan 14 §2).
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// A store rooted at the given directory.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Store { dir }
    }

    /// The default store directory (`None` when the OS has no config dir),
    /// sibling of [`crate::settings::Settings::path`].
    #[must_use]
    pub fn default_dir() -> Option<PathBuf> {
        crate::settings::Settings::path().and_then(|settings_path| {
            settings_path
                .parent()
                .map(|parent| parent.join("presets"))
        })
    }

    /// The store directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Loads every `*.json` file in the directory, isolating errors per
    /// file (a missing directory is an empty store).
    #[must_use]
    pub fn load_all(&self) -> StoreContents {
        let mut contents = StoreContents::default();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return contents;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort();
        for path in paths {
            let file_name = path
                .file_name()
                .map_or_else(|| "?".to_string(), |name| name.to_string_lossy().into_owned());
            match Self::load_file(&path) {
                Ok(preset) => contents.presets.push(StoredPreset { preset, file_name }),
                Err(error) => contents.unreadable.push(Unreadable {
                    file_name,
                    reason: error.message,
                    raw_json: error.raw_json,
                }),
            }
        }
        contents
    }

    /// Parses one preset file; a `schema > PRESET_SCHEMA` file still
    /// parses structurally and is returned (read-only badge derived in the
    /// UI via [`is_newer_format`]).
    fn load_file(path: &Path) -> Result<Preset, LoadError> {
        let text = std::fs::read_to_string(path)
            .map_err(|err| LoadError::new(format!("unreadable file: {err}")))?;
        let raw: serde_json::Value = serde_json::from_str(&text)
            .map_err(|err| LoadError::new(format!("not valid JSON: {err}")))?;
        serde_json::from_value(raw.clone())
            .map_err(|err| LoadError::with_raw(format!("unknown or malformed preset: {err}"), raw))
    }

    /// Writes `preset` (sanitized) into the store: an existing file of the
    /// same title is overwritten in place (edits), otherwise a free
    /// `<slug>[-N].json` name is chosen (collision suffixes `-2`, `-3`,
    /// …). When `previous_title` differs from the new title the old file
    /// is removed (rename re-slugs). Writes are atomic (tmp + rename).
    ///
    /// # Errors
    ///
    /// Validation or filesystem failure messages (surfaced in the UI
    /// status area, never dialogs).
    pub fn upsert(&self, preset: &Preset, previous_title: Option<&str>) -> Result<(), String> {
        validate(preset)?;
        std::fs::create_dir_all(&self.dir).map_err(|err| format!("cannot create presets dir: {err}"))?;
        if let Some(previous) = previous_title
            && previous != preset.title
            && let Some(old_file) = self.find_file_name_by_title(previous)?
        {
            std::fs::remove_file(self.dir.join(old_file))
                .map_err(|err| format!("cannot remove the renamed preset file: {err}"))?;
        }
        let target = match self.find_file_name_by_title(&preset.title)? {
            Some(existing) => self.dir.join(existing),
            None => self.dir.join(self.free_file_name(&preset.title)?),
        };
        let json = serde_json::to_string_pretty(&sanitized(preset))
            .map_err(|err| format!("cannot serialize the preset: {err}"))?;
        write_atomic(&target, json.as_bytes())
    }

    /// Deletes the file carrying `title` (titles are authoritative).
    ///
    /// # Errors
    ///
    /// Unknown title or filesystem failure.
    pub fn delete_by_title(&self, title: &str) -> Result<(), String> {
        let Some(file_name) = self.find_file_name_by_title(title)? else {
            return Err(format!("no stored preset named {title:?}"));
        };
        std::fs::remove_file(self.dir.join(file_name))
            .map_err(|err| format!("cannot delete the preset file: {err}"))
    }

    /// Deletes one file of the unreadable list (cleanup of junk files).
    ///
    /// # Errors
    ///
    /// Filesystem failure.
    pub fn delete_file(&self, file_name: &str) -> Result<(), String> {
        std::fs::remove_file(self.dir.join(file_name))
            .map_err(|err| format!("cannot delete the file: {err}"))
    }

    /// Scans the store for the file whose *title* matches (the slug is
    /// only a stable filename — renames re-slug, titles don't change).
    fn find_file_name_by_title(&self, title: &str) -> Result<Option<String>, String> {
        self.load_all()
            .presets
            .into_iter()
            .find(|stored| stored.preset.title == title)
            .map(|stored| Ok(Some(stored.file_name)))
            .unwrap_or(Ok(None))
    }

    /// The first free `<slug>.json` / `<slug>-N>.json` file name.
    fn free_file_name(&self, title: &str) -> Result<String, String> {
        let base = slugify(title);
        let mut candidate = format!("{base}.json");
        let mut counter = 1;
        while self.dir.join(&candidate).exists() {
            counter += 1;
            candidate = format!("{base}-{counter}.json");
        }
        Ok(candidate)
    }
}

/// Internal load failure carrying the reason (and the raw JSON when the
/// file itself parsed but the model rejected it).
struct LoadError {
    message: String,
    raw_json: Option<serde_json::Value>,
}

impl LoadError {
    fn new(message: String) -> Self {
        LoadError {
            message,
            raw_json: None,
        }
    }

    fn with_raw(message: String, raw_json: serde_json::Value) -> Self {
        LoadError {
            message,
            raw_json: Some(raw_json),
        }
    }
}

/// Atomic file write (tmp + rename, like the settings save).
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|err| format!("cannot write the preset file: {err}"))?;
    std::fs::rename(&tmp, path).map_err(|err| format!("cannot finalize the preset file: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ExifSettings;
    use byteshaver::config::{EncoderConfig, WebpOptions};

    fn webp_encoder(quality: f32) -> EncoderConfig {
        EncoderConfig::Webp(WebpOptions {
            lossless: false,
            quality,
        })
    }

    /// A minimal valid user preset.
    fn sample_preset(title: &str) -> Preset {
        Preset {
            schema: PRESET_SCHEMA,
            title: title.to_string(),
            description: "test preset".to_string(),
            created_unix: 1_700_000_000,
            modified_unix: 1_700_000_100,
            core_version: crate::app::CORE_VERSION.to_string(),
            builtin: false,
            content: PresetContent {
                encoder: webp_encoder(80.0),
                policies: None,
                include_output_dir: false,
            },
        }
    }

    fn tmp_store(name: &str) -> (PathBuf, Store) {
        let dir = tempfile_dir(name);
        let store = Store::new(dir.clone());
        (dir, store)
    }

    fn tempfile_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "byteshaver-preset-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    // ---- slug matrix ---------------------------------------------------------

    #[test]
    fn slug_matrix_lowercases_strips_and_truncates() {
        assert_eq!(slugify("AVIF Compact"), "avif-compact");
        assert_eq!(slugify("  spaced   out  "), "spaced-out");
        assert_eq!(slugify("a_b-c"), "a_b-c");
        assert_eq!(slugify("Ähnlich schön"), "hnlich-schn", "non-ascii dropped");
        assert!(
            slugify("!!!").starts_with("preset-"),
            "punctuation-only titles fall back to the hash slug"
        );
        assert!(
            slugify("").starts_with("preset-"),
            "the empty title falls back too (every title yields a usable slug)"
        );
        let long = slugify("This is a very long preset title that goes on and on forever");
        assert_eq!(long.len(), SLUG_MAX_CHARS);
        assert!(!long.ends_with('-'));
        assert!(long.chars().all(|ch| ch.is_ascii_lowercase()
            || ch.is_ascii_digit()
            || ch == '-'
            || ch == '_'));
    }

    #[test]
    fn empty_slug_falls_back_to_a_stable_hash() {
        let slug = slugify("日本語のプリセット");
        assert!(slug.starts_with("preset-"), "got {slug}");
        assert_eq!(slug, slugify("日本語のプリセット"), "stable per title");
        assert_ne!(slug, slugify("别的标题"), "different titles hash apart");
        assert_ne!(
            slugify("!!!"),
            slugify("???"),
            "distinct titles get distinct fallback hashes (no file collisions)"
        );
    }

    #[test]
    fn same_title_updates_in_place_and_slug_collisions_get_suffixes() {
        let (dir, store) = tmp_store("collide");
        let mut preset = sample_preset("My Preset");
        store.upsert(&preset, None).expect("first save");
        assert!(dir.join("my-preset.json").exists());
        // editing (same title) reuses the exact file — never a -2 suffix
        preset.modified_unix += 1;
        store.upsert(&preset, None).expect("second save");
        assert_eq!(store.load_all().presets.len(), 1);

        // a stale foreign file occupying the slug (title lost) forces the
        // collision suffixes on the next save of that title
        std::fs::write(dir.join("my-preset.json"), "{ junk").expect("seed stale file");
        store.upsert(&preset, None).expect("third save");
        assert!(dir.join("my-preset-2.json").exists(), "collision suffix -2");
        std::fs::write(dir.join("my-preset-2.json"), "{ junk").expect("seed again");
        store.upsert(&preset, None).expect("fourth save");
        assert!(dir.join("my-preset-3.json").exists(), "collision suffix -3");
        assert_eq!(store.load_all().unreadable.len(), 2, "the stale files surface");
    }

    // ---- validation / titles ---------------------------------------------------

    #[test]
    fn validation_enforces_title_and_description_bounds() {
        let mut preset = sample_preset("Ok");
        assert!(validate(&preset).is_ok());
        preset.title = "   ".to_string();
        assert!(validate(&preset).is_err(), "blank title rejected");
        preset.title = "x".repeat(TITLE_MAX_CHARS + 1);
        assert!(validate(&preset).is_err(), "over-long title rejected");
        preset.title = "Ok".to_string();
        preset.description = "d".repeat(DESCRIPTION_MAX_CHARS + 1);
        assert!(validate(&preset).is_err(), "over-long description rejected");

        assert!(validate_draft("", "", &[]).is_err());
        assert!(validate_draft("Ok", "", &["Ok".to_string()]).is_err(), "duplicate title");
        assert!(validate_draft(" Ok ", "", &["Other".to_string()]).is_ok(), "trimmed");
    }

    #[test]
    fn import_titles_get_the_imported_suffix_never_overwrite() {
        let existing = vec!["Mine".to_string()];
        assert_eq!(dedup_import_title("New", &existing), "New");
        assert_eq!(dedup_import_title("Mine", &existing), "Mine (imported)");
        let crowded = vec!["Mine".to_string(), "Mine (imported)".to_string()];
        assert_eq!(dedup_import_title("Mine", &crowded), "Mine (imported) 2");
        assert_eq!(
            dedup_import_title("Mine (imported)", &crowded),
            "Mine (imported) (imported)",
            "an already-imported title is suffixed again, never overwritten"
        );
    }

    // ---- privacy ---------------------------------------------------------------

    #[test]
    fn privacy_skip_strips_the_output_dir_unless_opted_in() {
        let mut preset = sample_preset("Local");
        preset.content.policies = Some(PolicySet {
            output_dir: Some("/home/me/secret-out".to_string()),
            ..PolicySet::default()
        });
        preset.content.include_output_dir = false;
        let json = export_json(&preset);
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert!(
            !json.contains("secret-out"),
            "exported JSON must not embed the local path:\n{json}"
        );
        assert!(
            value["content"]["policies"]
                .as_object()
                .expect("policies object")
                .get("output_dir")
                .is_none(),
            "the output_dir key is skipped entirely"
        );
        preset.content.include_output_dir = true;
        let json = export_json(&preset);
        assert!(json.contains("secret-out"), "opt-in embeds the path");
    }

    // ---- round trip / store io --------------------------------------------------

    #[test]
    fn preset_round_trips_through_the_store() {
        let (_dir, store) = tmp_store("roundtrip");
        let mut preset = sample_preset("Round Trip");
        preset.description = "with unicode ✓ and quotes \"…".to_string();
        preset.content.policies = Some(PolicySet {
            exif: ExifSettings {
                mode: crate::settings::ExifMode::Keep,
                ..ExifSettings::default()
            },
            collision: crate::settings::CollisionChoice::OverwriteAlways,
            ..PolicySet::default()
        });
        preset.content.include_output_dir = true;
        preset.content.policies.as_mut().unwrap().output_dir = Some("/tmp/öut".to_string());
        store.upsert(&preset, None).expect("save");

        let contents = store.load_all();
        assert!(contents.unreadable.is_empty(), "{:?}", contents.unreadable);
        assert_eq!(contents.presets.len(), 1);
        let loaded = &contents.presets[0];
        assert_eq!(loaded.preset, preset, "save → load equality");
        assert_eq!(loaded.file_name, "round-trip.json");

        // file → import parse → identical (the sharing guarantee)
        let text = std::fs::read_to_string(store.dir().join(&loaded.file_name)).expect("read");
        let imported = parse_preset_text(&text).expect("import parses");
        assert_eq!(imported, preset);
    }

    #[test]
    fn unicode_titles_slug_via_hash_and_survive_a_round_trip() {
        let (_dir, store) = tmp_store("unicode");
        let preset = sample_preset("日本語のプリセット");
        store.upsert(&preset, None).expect("save");
        let contents = store.load_all();
        assert_eq!(contents.presets.len(), 1);
        assert_eq!(contents.presets[0].preset.title, "日本語のプリセット");
        assert_eq!(contents.presets[0].preset, preset);
    }

    #[test]
    fn rename_moves_the_file_and_keeps_the_store_consistent() {
        let (_dir, store) = tmp_store("rename");
        let mut preset = sample_preset("Before");
        store.upsert(&preset, None).expect("save");
        preset.title = "After".to_string();
        preset.modified_unix += 1;
        store.upsert(&preset, Some("Before")).expect("rename");
        let contents = store.load_all();
        assert_eq!(contents.presets.len(), 1);
        assert_eq!(contents.presets[0].file_name, "after.json");
        assert_eq!(contents.presets[0].preset.title, "After");
        assert!(store.delete_by_title("After").is_ok());
        assert!(store.load_all().presets.is_empty());
        assert!(store.delete_by_title("After").is_err(), "second delete fails");
    }

    #[test]
    fn corrupt_files_are_isolated_into_the_unreadable_list() {
        let (dir, store) = tmp_store("corrupt");
        store.upsert(&sample_preset("Fine"), None).expect("seed");
        std::fs::write(dir.join("broken.json"), "{ not json !!!").expect("seed junk");
        std::fs::write(dir.join("not-a-preset.json"), r#"{"hello": 1}"#).expect("seed junk");
        std::fs::write(dir.join("hidden.txt"), "ignored").expect("seed non-json");
        let contents = store.load_all();
        assert_eq!(contents.presets.len(), 1, "the good preset survives");
        assert_eq!(contents.unreadable.len(), 2, "non-json files are ignored");
        let names: Vec<&str> = contents
            .unreadable
            .iter()
            .map(|unreadable| unreadable.file_name.as_str())
            .collect();
        assert!(names.contains(&"broken.json"));
        assert!(names.contains(&"not-a-preset.json"));
        assert!(
            contents
                .unreadable
                .iter()
                .all(|unreadable| unreadable.raw_json.is_none()
                    || unreadable.file_name == "not-a-preset.json"),
            "valid JSON is kept raw for the reason display"
        );
    }

    #[test]
    fn forward_compat_fixtures_load_with_a_reason_or_a_badge() {
        let (dir, store) = tmp_store("forward");
        // future schema whose shape still parses → visible read-only
        let newer = sample_preset("From The Future");
        let mut newer = newer;
        newer.schema = PRESET_SCHEMA + 1;
        let newer_json = serde_json::to_string_pretty(&newer).expect("serialize");
        std::fs::write(dir.join("newer.json"), newer_json).expect("seed");
        // unknown encoder variant of a future build → unreadable with raw JSON
        std::fs::write(
            dir.join("future-encoder.json"),
            r#"{
                "schema": 1, "title": "Future Enc", "description": "",
                "created_unix": 0, "modified_unix": 0, "core_version": "9.9.9",
                "content": { "encoder": { "AvifNext": {} }, "include_output_dir": false }
            }"#,
        )
        .expect("seed");
        let contents = store.load_all();
        assert_eq!(contents.presets.len(), 1, "the newer-schema file stays visible");
        assert!(is_newer_format(&contents.presets[0].preset));
        assert_eq!(contents.unreadable.len(), 1);
        let unreadable = &contents.unreadable[0];
        assert_eq!(unreadable.file_name, "future-encoder.json");
        assert!(
            unreadable.raw_json.is_some() && !unreadable.reason.is_empty(),
            "raw JSON + reason kept for the grayed display"
        );
    }

    #[test]
    fn a_forged_builtin_flag_never_survives_an_import() {
        let (dir, store) = tmp_store("forged");
        let mut preset = sample_preset("Forged");
        preset.builtin = true;
        let json = serde_json::to_string_pretty(&preset).expect("serialize");
        std::fs::write(dir.join("forged.json"), json).expect("seed");
        let text = std::fs::read_to_string(dir.join("forged.json")).expect("read");
        let imported = parse_preset_text(&text).expect("parses");
        assert!(!imported.builtin, "imports are always user presets");
        // and the store's own load path neutralizes it as well
        let contents = store.load_all();
        assert!(!contents.presets[0].preset.builtin);
    }

    // ---- apply path helpers ------------------------------------------------------

    #[test]
    fn apply_preview_announces_the_scope() {
        let mut preset = sample_preset("Format only");
        assert_eq!(apply_preview(&preset), "applies format only");
        preset.content.policies = Some(PolicySet::default());
        assert_eq!(
            apply_preview(&preset),
            "applies format + 0 policies",
            "default policies carry no changes"
        );
        preset.content.policies = Some(PolicySet {
            collision: crate::settings::CollisionChoice::OverwriteAlways,
            output_dir: Some("/x".to_string()),
            max_animation_memory_mib: 512,
            ..PolicySet::default()
        });
        assert_eq!(apply_preview(&preset), "applies format + 3 policies");
    }

    #[test]
    fn policy_change_count_matches_the_nine_mirrors() {
        assert_eq!(policy_change_count(&PolicySet::default()), 0);
        let all = PolicySet {
            output_dir: Some("/x".to_string()),
            exif: ExifSettings {
                mode: crate::settings::ExifMode::Keep,
                ..ExifSettings::default()
            },
            collision: crate::settings::CollisionChoice::OverwriteIfSmaller,
            animated_input: byteshaver::config::AnimatedInputPolicy::Error,
            heif_image_policy: byteshaver::config::HeifImagePolicy::All,
            discard_if_larger_than_input: true,
            discard_input_alpha_channel: true,
            max_animation_memory_mib: 1,
            reverse_processing_order: true,
        };
        assert_eq!(policy_change_count(&all), 9);
    }

    #[test]
    fn preset_equality_tolerates_the_excluded_output_dir() {
        let mut preset = sample_preset("Match");
        preset.content.policies = Some(PolicySet {
            output_dir: None,
            collision: crate::settings::CollisionChoice::OverwriteAlways,
            ..PolicySet::default()
        });
        let mut current = PolicySet {
            output_dir: Some("/local/dir".to_string()),
            collision: crate::settings::CollisionChoice::OverwriteAlways,
            ..PolicySet::default()
        };
        assert!(
            preset_matches(&preset, &preset.content.encoder, &current),
            "the excluded output dir must not read as modified"
        );
        current.collision = crate::settings::CollisionChoice::KeepExisting;
        assert!(
            !preset_matches(&preset, &preset.content.encoder, &current),
            "a real policy drift is 'modified'"
        );
        current.collision = crate::settings::CollisionChoice::OverwriteAlways;
        assert!(
            !preset_matches(&preset, &webp_encoder(50.0), &current),
            "an encoder drift is 'modified'"
        );
        // format-only preset matches regardless of policies
        let mut format_only = sample_preset("Only");
        format_only.content.policies = None;
        assert!(preset_matches(&format_only, &format_only.content.encoder, &current));
    }

    // ---- misc helpers --------------------------------------------------------------

    #[test]
    fn dates_format_as_iso_days() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_700_000_000), "2023-11-14");
        assert_eq!(format_date(1_760_000_000), "2025-10-09");
    }

    #[test]
    fn build_preset_trims_and_normalizes_the_scope() {
        let policies = PolicySet {
            output_dir: Some("/tmp/out".to_string()),
            ..PolicySet::default()
        };
        let preset = build_preset(
            "  Padded  ",
            " desc ",
            true,
            false,
            &webp_encoder(90.0),
            &policies,
        );
        assert_eq!(preset.title, "Padded");
        assert_eq!(preset.description, "desc");
        assert_eq!(preset.schema, PRESET_SCHEMA);
        assert!(!preset.builtin);
        assert_eq!(preset.core_version, crate::app::CORE_VERSION);
        assert!(
            preset.content.policies.as_ref().unwrap().output_dir.is_none(),
            "no opt-in → path stripped at build time"
        );
        assert!(!preset.content.include_output_dir);

        let preset = build_preset(
            "Full",
            "",
            true,
            true,
            &webp_encoder(90.0),
            &policies,
        );
        assert_eq!(
            preset.content.policies.as_ref().unwrap().output_dir.as_deref(),
            Some("/tmp/out"),
            "opt-in embeds the path"
        );
        assert!(preset.content.include_output_dir);

        let preset = build_preset("Format", "", false, true, &webp_encoder(90.0), &policies);
        assert!(preset.content.policies.is_none());
        assert!(
            !preset.content.include_output_dir,
            "the dir opt-in is meaningless without policies"
        );
        let _ = export_file_name(&preset);
    }

    #[test]
    fn export_file_name_uses_the_double_extension() {
        let mut preset = sample_preset("My Export!");
        assert_eq!(
            export_file_name(&preset),
            format!("my-export.{EXPORT_EXTENSION}")
        );
        preset.title = "日本語".to_string();
        assert!(export_file_name(&preset).starts_with("preset-"));
    }
}
