//! Application state of the GUI (plan WS8 §1/§5): the queue, the selected
//! encoder options and global policies (persisted via [`crate::settings`]),
//! the running job and the event flow between the conversion worker and the
//! UI thread.
//!
//! # Threading model
//!
//! - The UI thread owns everything; egui calls [`App::update`] each frame.
//! - Starting a job builds a [`JobSpec`], then hands it to
//!   [`JobHandle::start`][byteshaver::job::JobHandle::start] together with a
//!   [`ChannelReporter`][crate::reporter::ChannelReporter], a fresh
//!   [`StopFlag`][byteshaver::job::StopFlag] and a
//!   [`Session`][byteshaver::job::Session] — the core spawns its own worker
//!   (plus rayon file parallelism).
//! - Every frame the UI drains the `std::sync::mpsc` receiver and repaints
//!   at most 100 ms later while a job runs; the final
//!   [`JobEvent::Finished`] triggers `JobHandle::join` (non-blocking in
//!   practice: `Finished` is the pipeline's last emission).
//! - Cancellation is `StopFlag::raise()` from the Cancel button (identical
//!   semantics to the CLI's Ctrl+C: the current file finishes, the rest of
//!   the queue is reported as aborted).
//!
//! All product logic (spec building, event application) lives in pure
//! functions here — the `panels` modules only render state, which keeps
//! everything unit-testable headless (the egui rendering itself cannot run
//! without a display server).

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use byteshaver::config::{ConversionConfig, EncoderConfig};
use byteshaver::job::{
    Capabilities, InputSelection, JobEvent, JobHandle, JobSpec, Reporter, RunReport, Session,
    StopFlag,
};
use byteshaver::metadata::policy::ExifPolicy;
use byteshaver::pipeline::Outcome;

use crate::celebrate::{Confetti, RectPx, TextFlourish};
use crate::metrics::{MetricMode, MetricState};
use crate::options;
use crate::presets::PresetRef;
use crate::queue::Queue;
use crate::reporter::ChannelReporter;
use crate::settings::{OutputMode, Settings};
use crate::thumb::{ThumbKey, ThumbState};

/// Version of the core the GUI is released with.
///
/// The gui crate is versioned in lockstep with `byteshaver` (identical
/// `CARGO_PKG_VERSION`), so this is the core version; shown in the About
/// panel next to `capabilities()`.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Input extensions offered by the file-picker filter. Mirrors the
/// extension table of `byteshaver::format::ImageFormat::from_extension`
/// (which stays the authoritative run-time check).
pub const INPUT_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "dds", "exr", "farbfeld", "ff", "gif", "heic", "heif", "hif", "ico", "jpeg",
    "jpg", "jxl", "png", "pjpeg", "pnm", "qoi", "tga", "tif", "tiff", "webp", "x-png",
];

/// Pending editor state for the JXL advanced `--setting` list (the
/// `JxlOptions.advanced` entries are plain `(String, i64)` pairs, so the
/// "add row" input needs somewhere to live between frames).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JxlAdvancedDraft {
    /// Text of the setting id to add.
    pub new_id: String,
    /// Numeric value of the setting to add.
    pub new_value: i64,
}

/// Running totals of the active job (fed by
/// [`JobEvent::ProgressStats`], same math as the CLI footer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FooterStats {
    /// Counted input bytes so far (encoded + skipped files).
    pub input_bytes: u64,
    /// Counted output bytes so far.
    pub output_bytes: u64,
    /// Files encoded successfully so far.
    pub ok: u64,
    /// Files skipped so far (existing outputs).
    pub skipped: u64,
    /// Files failed so far.
    pub errors: u64,
}

/// State of the currently running job (UI side).
pub struct RunningJob {
    /// Worker handle; taken and joined as soon as
    /// [`JobEvent::Finished`] arrives.
    pub handle: Option<JobHandle>,
    /// Cancel button target.
    pub stop: StopFlag,
    /// Live totals.
    pub stats: FooterStats,
    /// Total work items (from [`JobEvent::Started`]).
    pub total_items: Option<u64>,
    /// Finished work items so far.
    pub finished_items: u64,
    /// Paths currently being worked on (FileStarted without a matching
    /// FileFinished) — the truthful "active" set (plan 12 §1):
    /// `Queue::begin_run` marks every row Running, only this set knows
    /// which ones are actually in flight.
    pub active: std::collections::HashSet<PathBuf>,
    /// Per-work-item segment states of the progress bar, indexed by the
    /// events' `index` (sized on [`JobEvent::Started`], plan 12 §2).
    pub segments: Vec<SegState>,
    /// Per-work-item input paths for segment tooltips (sized like
    /// [`RunningJob::segments`]).
    pub seg_paths: Vec<Option<PathBuf>>,
    /// When the job started (for the elapsed display).
    pub started_at: Instant,
}

impl RunningJob {
    /// Progress fraction in `0.0..=1.0` (`None` before discovery reports).
    #[must_use]
    pub fn progress(&self) -> Option<f32> {
        let total = self.total_items?;
        if total == 0 {
            return Some(1.0);
        }
        Some(self.finished_items as f32 / total as f32)
    }
}

/// Paint class of one progress-bar segment (egui-free; the footer maps it
/// onto concrete colors, plan 12 §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegTone {
    /// Not started yet (faint gray).
    Pending,
    /// Currently being converted (selection blue + pulse/shimmer).
    Active,
    /// Encoded at ≤ 80 % ratio (green tint).
    Good,
    /// Encoded at 80–100 % ratio (soft green-gray).
    Neutral,
    /// Output larger than the input (amber).
    Grew,
    /// Conversion failed (red).
    Error,
    /// Skipped, collided or aborted (dark gray).
    Skipped,
}

/// State of one progress-bar segment, indexed by the work-item index of
/// the [`JobEvent`] stream (plan 12 §2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SegState {
    /// Waiting to be worked on.
    Pending,
    /// Inside the job's active set (FileStarted without FileFinished).
    Active,
    /// Finished: result class plus the `output / input` ratio when known.
    Done {
        /// Result class (color band).
        tone: SegTone,
        /// `output / input` fraction (`None` when the input had no size).
        ratio: Option<f32>,
    },
}

impl SegState {
    /// The segment's paint class.
    #[must_use]
    pub fn tone(self) -> SegTone {
        match self {
            SegState::Pending => SegTone::Pending,
            SegState::Active => SegTone::Active,
            SegState::Done { tone, .. } => tone,
        }
    }

    /// Maps a finished [`Outcome`] onto its segment state. The ratio bands
    /// come from [`crate::ratio`] (plan 10 §phase 1: ≤ 80 % good, 80–100 %
    /// neutral, above 100 % grew; discarded-larger counts as grew).
    #[must_use]
    pub fn from_outcome(outcome: &Outcome) -> Self {
        match outcome {
            Outcome::Encoded {
                input_size,
                output_size,
                ..
            } => {
                let ratio = ratio_of(*input_size, *output_size);
                let tone = ratio.map_or(SegTone::Neutral, ratio_tone);
                SegState::Done { tone, ratio }
            }
            Outcome::DiscardedLargerThanInput {
                input_size,
                encoded_size,
            } => SegState::Done {
                tone: SegTone::Grew,
                ratio: ratio_of(*input_size, *encoded_size),
            },
            Outcome::Error(_) => SegState::Done {
                tone: SegTone::Error,
                ratio: None,
            },
            Outcome::SkippedExisting { .. }
            | Outcome::SkippedCollision { .. }
            | Outcome::DiscardedLargerThanExisting { .. }
            | Outcome::Aborted => SegState::Done {
                tone: SegTone::Skipped,
                ratio: None,
            },
        }
    }
}

/// `output / input` as a fraction (`None` for a zero-size input).
fn ratio_of(input: u64, output: u64) -> Option<f32> {
    crate::ratio::ratio_fraction(input, output)
}

/// Ratio band of a compression fraction as a segment paint class — a thin
/// bridge onto [`crate::ratio::hint_for_ratio`], the single home of the
/// plan-10 bands (≤ 0.8 good / ≤ 1.0 neutral / > 1.0 grew).
#[must_use]
pub fn ratio_tone(ratio: f32) -> SegTone {
    match crate::ratio::hint_for_ratio(ratio) {
        crate::ratio::Hint::Good => SegTone::Good,
        crate::ratio::Hint::Neutral => SegTone::Neutral,
        crate::ratio::Hint::Grew => SegTone::Grew,
    }
}

/// The GUI application state.
pub struct App {
    /// The conversion queue.
    pub queue: Queue,
    /// Persisted configuration (encoder options, policies, output dir).
    pub settings: Settings,
    /// Compile-time capabilities of the linked core (encoder list, HEIF
    /// input availability).
    pub capabilities: Capabilities,
    /// Per-window thread budget.
    pub session: Session,
    /// The active job, if any.
    pub running: Option<RunningJob>,
    /// UI-side end of the event channel of the active job.
    event_rx: Option<Receiver<JobEvent>>,
    /// All events of the current/last run (JSONL export source).
    pub event_log: Vec<JobEvent>,
    /// Final report of the last run (`None` while a job runs or when no
    /// run happened yet).
    pub report: Option<RunReport>,
    /// Whether the report viewport is open (its own OS window, plan 09);
    /// closing it via the OS title bar flips this off.
    pub show_report: bool,
    /// Whether the about window is open.
    pub show_about: bool,
    /// Pre-flight/start errors (invalid EXIF selectors, encoder disabled).
    pub start_error: Option<String>,
    /// Whether a drag is currently over the window (drop-zone highlight).
    pub drag_hovered: bool,
    /// Draft state of the JXL advanced settings editor.
    pub jxl_draft: JxlAdvancedDraft,
    /// Confetti burst of the last finished run (`None` = no party /
    /// expired; plan 12 §3).
    pub confetti: Option<Confetti>,
    /// Reduced-motion text flourish of the last finished run (plan 12
    /// §3).
    pub flourish: Option<TextFlourish>,
    /// Last known viewport size in points (refreshed every frame; anchors
    /// the confetti emission at the Convert-button corner).
    pub viewport_size: [f32; 2],
    /// Thumbnail/EXIF background worker: request dedup, texture cache
    /// and the pause flag (plan 11 §6; paused while a job runs).
    pub thumbs: ThumbState,
    /// Quality-metric background worker (plan 10 §phase 2): measurement
    /// requests/results cache, same pause policy as the thumbs worker.
    pub metrics: MetricState,
    /// Open visual difference inspector (plan 10 §phase 3): `None` when
    /// closed — buffers and textures are dropped with it.
    pub inspector: Option<crate::panels::inspector::InspectorState>,
    /// Last row-action failure (spawn errors surface as row tooltips,
    /// plan 11 §5 — never dialogs).
    pub action_error: Option<String>,
    /// The compiled-in built-in presets (plan 14 §5), loaded once.
    pub builtins: Vec<crate::presets::Preset>,
    /// User presets loaded from the preset store (plan 14 §2).
    pub user_presets: Vec<crate::presets::StoredPreset>,
    /// Store files that could not be parsed (plan 14 §2; surfaced grayed
    /// with a reason in the manage window).
    pub unreadable_presets: Vec<crate::presets::Unreadable>,
    /// The preset store (`None` when the OS has no config dir; tests
    /// inject a temp-dir store).
    pub preset_store: Option<crate::presets::Store>,
    /// The currently active preset (drives the dropdown label and its
    /// "·modified" dimming, plan 14 §4).
    pub active_preset: Option<crate::presets::PresetRef>,
    /// Whether the manage-presets window is open (plan 14 §4).
    pub show_preset_manager: bool,
    /// Open "save current as preset" modal draft (plan 14 §4).
    pub preset_save: Option<PresetSaveDraft>,
    /// Last preset-operation status/error line (manage window + save
    /// modal; never dialogs, plan 14 §2).
    pub preset_status: Option<String>,
    /// The explicit half of the chip-ladder tri-state (plan 15 F1): set
    /// when the user clicks the "⚙ Custom…" card, cleared whenever a
    /// preset/chip is applied (or the encoder is switched). With it unset
    /// the selection is pure equality against the current chip ladder
    /// (a matching chip highlights; drift highlights Custom as the
    /// *derived* state) — see `crate::chips::chip_selection`.
    pub custom_chip_explicit: bool,
    settings_dirty: bool,
}

/// Draft state of the "save current as preset" modal (plan 14 §4): the
/// title/description fields plus the two privacy-relevant toggles. The
/// scope toggle defaults to a *full* preset only when the policies
/// actually differ from the defaults (plan 14 decision 1) — see
/// [`PresetSaveDraft::from_current`].
#[derive(Clone, Debug, PartialEq)]
pub struct PresetSaveDraft {
    /// Title field (required, unique — validated live via
    /// `crate::presets::validate_draft`).
    pub title: String,
    /// Description field (optional).
    pub description: String,
    /// Scope toggle: also save the output/global policies.
    pub include_policies: bool,
    /// Opt-in to embed the local output directory path (privacy, plan 14
    /// §2; only enabled when a directory is set).
    pub include_output_dir: bool,
}

impl PresetSaveDraft {
    /// Seeds the draft from the current state: the scope toggle is on when
    /// the policies differ from the defaults (decision 1); the output-dir
    /// opt-in always starts off.
    #[must_use]
    pub fn from_current(app: &App) -> Self {
        PresetSaveDraft {
            title: String::new(),
            description: String::new(),
            include_policies: crate::presets::policy_change_count(&app.settings.policies) > 0,
            include_output_dir: false,
        }
    }
}

impl App {
    /// Builds the app with the given (already loaded) settings.
    #[must_use]
    pub fn with_settings(settings: Settings) -> Self {
        let metric_engine = settings.metric_engine;
        let preset_store = crate::presets::Store::default_dir().map(crate::presets::Store::new);
        let (user_presets, unreadable_presets) = match &preset_store {
            Some(store) => {
                let contents = store.load_all();
                (contents.presets, contents.unreadable)
            }
            None => (Vec::new(), Vec::new()),
        };
        App {
            queue: Queue::new(),
            settings,
            capabilities: byteshaver::job::capabilities(),
            session: Session::new(),
            running: None,
            event_rx: None,
            event_log: Vec::new(),
            report: None,
            show_report: false,
            show_about: false,
            start_error: None,
            drag_hovered: false,
            jxl_draft: JxlAdvancedDraft::default(),
            confetti: None,
            flourish: None,
            viewport_size: [1100.0, 720.0],
            thumbs: ThumbState::new(),
            metrics: {
                let mut metrics = MetricState::new();
                metrics.engine = metric_engine;
                metrics
            },
            inspector: None,
            action_error: None,
            builtins: crate::presets::builtin(),
            user_presets,
            unreadable_presets,
            preset_store,
            active_preset: None,
            show_preset_manager: false,
            preset_save: None,
            preset_status: None,
            custom_chip_explicit: false,
            settings_dirty: false,
        }
    }

    /// Marks the settings as changed (panels call this after edits); the
    /// next `update` saves them to disk.
    pub fn mark_settings_dirty(&mut self) {
        self.settings_dirty = true;
    }

    /// Whether the settings changed since the last save.
    #[must_use]
    pub fn settings_dirty(&self) -> bool {
        self.settings_dirty
    }

    /// Saves the settings and clears the dirty flag.
    pub fn save_settings(&mut self) {
        self.settings.save();
        self.settings_dirty = false;
    }

    /// Whether the selected encoder is compiled into this build.
    #[must_use]
    pub fn encoder_available(&self) -> bool {
        options::encoder_enabled(
            &self.capabilities,
            options::encoder_kind_name(&self.settings.encoder),
        )
    }

    /// Why the selected encoder is unavailable (`None` when available).
    #[must_use]
    pub fn encoder_unavailable_reason(&self) -> Option<&'static str> {
        options::encoder_disabled_reason(
            &self.capabilities,
            options::encoder_kind_name(&self.settings.encoder),
        )
    }

    /// Selects the encoder by capability name, resetting its options to
    /// the CLI defaults. Returns `false` (keeping the old selection) when
    /// the name is unknown. Clears the explicit chip-custom state (the
    /// switch is a configuration choice, plan 15 F1).
    pub fn select_encoder(&mut self, name: &str) -> bool {
        let Some(config) = options::default_encoder_config(name) else {
            return false;
        };
        if self.settings.encoder != config {
            self.settings.encoder = config;
            self.custom_chip_explicit = false;
            self.mark_settings_dirty();
        }
        true
    }

    /// Output extension of the selected encoder (`"webp"`, `"avif"`, …),
    /// from the capability registry with the encoder kind name as the
    /// fallback — the Target-format column's value (plan 11 §3).
    #[must_use]
    pub fn target_extension(&self) -> Option<&'static str> {
        let kind = options::encoder_kind_name(&self.settings.encoder);
        Some(
            self.capabilities
                .encoders
                .iter()
                .find(|info| info.name == kind)
                .map_or(kind, |info| info.extension),
        )
    }

    // ---- presets (plan 14) ---------------------------------------------------

    /// Applies a preset to the current state (pure, plan 14 §4): the
    /// encoder is replaced (resetting the JXL draft state) and — for
    /// full-scope presets — the policies too. The output *mode* of a full
    /// preset always applies (plan 15 F16); the output directory *string*
    /// is **kept** when the preset excluded it (privacy exclusions must
    /// not wipe the local setting, and switching modes never clears the
    /// stored path). Format-only presets never touch mode/dir at all.
    /// Queue, table and window state are never touched. Settings are
    /// marked dirty once; the explicit chip-custom state is cleared (the
    /// chip ladder re-derives from the applied config, plan 15 F1).
    ///
    /// # Errors
    ///
    /// A "newer format" preset (schema beyond this build's
    /// [`crate::presets::PRESET_SCHEMA`]) is rejected; applying is
    /// structural otherwise (serde validated the file on load).
    pub fn apply_preset(&mut self, preset: &crate::presets::Preset) -> Result<(), String> {
        if crate::presets::is_newer_format(preset) {
            return Err(format!(
                "{:?} uses a newer preset format (schema {}) and cannot be applied",
                preset.title, preset.schema
            ));
        }
        self.settings.encoder = preset.content.encoder.clone();
        self.jxl_draft = JxlAdvancedDraft::default();
        self.custom_chip_explicit = false;
        if let Some(saved) = &preset.content.policies {
            let mut next = saved.clone();
            if !preset.content.include_output_dir {
                next.output_dir = self.settings.policies.output_dir.clone();
            }
            self.settings.policies = next;
        }
        self.mark_settings_dirty();
        self.active_preset = Some(if preset.builtin {
            PresetRef::Builtin(
                self.builtins
                    .iter()
                    .position(|builtin| builtin.title == preset.title)
                    .unwrap_or_default(),
            )
        } else {
            PresetRef::User(preset.title.clone())
        });
        Ok(())
    }

    /// The dropdown's active-preset label: the preset title, dimmed with a
    /// "·modified" suffix when the current state drifted from it (pure
    /// equality via [`crate::presets::preset_matches`], plan 14 §4).
    #[must_use]
    pub fn active_preset_label(&self) -> Option<String> {
        let preset = crate::presets::find_preset(
            &self.builtins,
            &self.user_presets,
            self.active_preset.as_ref()?,
        )?;
        if crate::presets::preset_matches(preset, &self.settings.encoder, &self.settings.policies)
        {
            Some(preset.title.clone())
        } else {
            Some(format!("{} ·modified", preset.title))
        }
    }

    /// Builds the preset the save modal would write (pure wrapper around
    /// [`crate::presets::build_preset`]; the timestamps/version are filled
    /// in there).
    #[must_use]
    pub fn preset_from_current(&self, draft: &PresetSaveDraft) -> crate::presets::Preset {
        crate::presets::build_preset(
            &draft.title,
            &draft.description,
            draft.include_policies,
            draft.include_output_dir,
            &self.settings.encoder,
            &self.settings.policies,
        )
    }

    /// Validates + persists a preset built from the save-modal draft.
    ///
    /// # Errors
    ///
    /// Validation (empty/duplicate/over-long title) or store failure.
    pub fn save_preset_from_current(&mut self, draft: &PresetSaveDraft) -> Result<(), String> {
        let titles: Vec<String> = self
            .user_presets
            .iter()
            .map(|stored| stored.preset.title.clone())
            .collect();
        crate::presets::validate_draft(&draft.title, &draft.description, &titles)?;
        let preset = self.preset_from_current(draft);
        match &self.preset_store {
            Some(store) => store.upsert(&preset, None)?,
            None => return Err("no preset directory is available".to_string()),
        }
        self.reload_presets();
        self.active_preset = Some(PresetRef::User(preset.title.clone()));
        Ok(())
    }

    /// Duplicates any preset (built-ins included — the supported way to
    /// edit them, plan 14 §2) under a fresh `"(copy)"` title.
    ///
    /// # Errors
    ///
    /// Store failure.
    pub fn preset_duplicate(&mut self, source: &crate::presets::Preset) -> Result<(), String> {
        let existing: Vec<String> = self
            .user_presets
            .iter()
            .map(|stored| stored.preset.title.clone())
            .collect();
        let base = format!("{} (copy)", source.title);
        let mut title = base.clone();
        let mut counter = 1;
        while existing.contains(&title) {
            counter += 1;
            title = format!("{base} {counter}");
        }
        let now = crate::presets::now_unix();
        let mut copy = source.clone();
        copy.builtin = false;
        copy.title = title;
        copy.created_unix = now;
        copy.modified_unix = now;
        match &self.preset_store {
            Some(store) => store.upsert(&copy, None)?,
            None => return Err("no preset directory is available".to_string()),
        }
        self.reload_presets();
        Ok(())
    }

    /// Renames a user preset (re-slugging the file, plan 14 §2).
    ///
    /// # Errors
    ///
    /// Unknown title, validation, or store failure.
    pub fn preset_rename(&mut self, old_title: &str, new_title: &str) -> Result<(), String> {
        let other_titles: Vec<String> = self
            .user_presets
            .iter()
            .map(|stored| stored.preset.title.clone())
            .filter(|title| title != old_title)
            .collect();
        crate::presets::validate_draft(new_title, "", &other_titles)?;
        let mut preset = self
            .user_presets
            .iter()
            .find(|stored| stored.preset.title == old_title)
            .map(|stored| stored.preset.clone())
            .ok_or_else(|| format!("no preset named {old_title:?}"))?;
        preset.title = new_title.trim().to_string();
        preset.modified_unix = crate::presets::now_unix();
        match &self.preset_store {
            Some(store) => store.upsert(&preset, Some(old_title))?,
            None => return Err("no preset directory is available".to_string()),
        }
        if self.active_preset == Some(PresetRef::User(old_title.to_string())) {
            self.active_preset = Some(PresetRef::User(preset.title.clone()));
        }
        self.reload_presets();
        Ok(())
    }

    /// Edits a user preset's description in place.
    ///
    /// # Errors
    ///
    /// Unknown title, over-long description, or store failure.
    pub fn preset_edit_description(
        &mut self,
        title: &str,
        description: &str,
    ) -> Result<(), String> {
        let mut preset = self
            .user_presets
            .iter()
            .find(|stored| stored.preset.title == title)
            .map(|stored| stored.preset.clone())
            .ok_or_else(|| format!("no preset named {title:?}"))?;
        if description.chars().count() > crate::presets::DESCRIPTION_MAX_CHARS {
            return Err(format!(
                "the description is longer than {} characters",
                crate::presets::DESCRIPTION_MAX_CHARS
            ));
        }
        preset.description = description.trim().to_string();
        preset.modified_unix = crate::presets::now_unix();
        match &self.preset_store {
            Some(store) => store.upsert(&preset, None)?,
            None => return Err("no preset directory is available".to_string()),
        }
        self.reload_presets();
        Ok(())
    }

    /// Deletes a user preset from the store.
    ///
    /// # Errors
    ///
    /// Unknown title or store failure.
    pub fn preset_delete(&mut self, title: &str) -> Result<(), String> {
        match &self.preset_store {
            Some(store) => store.delete_by_title(title)?,
            None => return Err("no preset directory is available".to_string()),
        }
        if self.active_preset == Some(PresetRef::User(title.to_string())) {
            self.active_preset = None;
        }
        self.reload_presets();
        Ok(())
    }

    /// Writes a preset's sanitized JSON to an export target.
    ///
    /// # Errors
    ///
    /// Filesystem failure.
    pub fn preset_export(
        &self,
        preset: &crate::presets::Preset,
        target: std::path::PathBuf,
    ) -> Result<(), String> {
        std::fs::write(target, crate::presets::export_json(preset))
            .map_err(|err| format!("cannot write the preset file: {err}"))
    }

    /// Imports preset files (multi-select, plan 14 §3): each file is
    /// validated, its title deduped, then stored. Returns the number of
    /// imported presets.
    ///
    /// # Errors
    ///
    /// No store available, or a per-file problem summary (all other files
    /// are still imported).
    pub fn import_presets(&mut self, paths: &[std::path::PathBuf]) -> Result<usize, String> {
        let Some(store) = self.preset_store.clone() else {
            return Err("no preset directory is available".to_string());
        };
        let mut imported = 0;
        let mut errors = Vec::new();
        for path in paths {
            let file_name = path
                .file_name()
                .map_or_else(|| "?".to_string(), |name| name.to_string_lossy().into_owned());
            let outcome = std::fs::read_to_string(path)
                .map_err(|err| err.to_string())
                .and_then(|text| crate::presets::parse_preset_text(&text));
            match outcome {
                Ok(mut preset) => {
                    let titles: Vec<String> = store
                        .load_all()
                        .presets
                        .into_iter()
                        .map(|stored| stored.preset.title)
                        .collect();
                    preset.title = crate::presets::dedup_import_title(&preset.title, &titles);
                    match store.upsert(&preset, None) {
                        Ok(()) => imported += 1,
                        Err(err) => errors.push(format!("{file_name}: {err}")),
                    }
                }
                Err(err) => errors.push(format!("{file_name}: {err}")),
            }
        }
        self.reload_presets();
        if errors.is_empty() {
            Ok(imported)
        } else {
            Err(format!(
                "imported {imported} preset(s); problem(s): {}",
                errors.join("; ")
            ))
        }
    }

    /// Re-reads only the unreadable list (after a junk-file cleanup).
    pub fn reload_unreadable_only(&mut self) {
        if let Some(store) = &self.preset_store {
            self.unreadable_presets = store.load_all().unreadable;
        }
    }

    /// Re-reads the user preset list from the store (after every mutation)
    /// and drops a dangling active reference.
    fn reload_presets(&mut self) {
        let Some(store) = &self.preset_store else {
            return;
        };
        let contents = store.load_all();
        self.user_presets = contents.presets;
        self.unreadable_presets = contents.unreadable;
        if let Some(PresetRef::User(title)) = &self.active_preset
            && !self
                .user_presets
                .iter()
                .any(|stored| &stored.preset.title == title)
        {
            self.active_preset = None;
        }
    }

    // ---- job spec construction -------------------------------------------

    /// Resolves the EXIF policy from the settings (may fail on invalid
    /// tag selectors; the message blocks starting a job).
    ///
    /// # Errors
    ///
    /// The CLI's selector parse error message.
    pub fn exif_policy(&self) -> Result<ExifPolicy, String> {
        self.settings.policies.exif.to_policy()
    }

    /// Builds the shared [`ConversionConfig`] from the global policy
    /// mirrors (pure; unit-tested against `ConversionConfig::from_args`).
    ///
    /// # Errors
    ///
    /// The EXIF selector parse error.
    pub fn build_conversion_config(&self) -> Result<ConversionConfig, String> {
        let (overwrite_if_smaller, overwrite_existing) = self.settings.policies.collision.flags();
        Ok(ConversionConfig {
            // input selection happens via the spec's InputSelection
            pattern: String::new(),
            // overridden by the spec's output directory
            output: String::new(),
            reverse_processing_order: self.settings.policies.reverse_processing_order,
            overwrite_if_smaller,
            overwrite_existing,
            discard_if_larger_than_input: self
                .settings
                .policies
                .discard_if_larger_than_input,
            discard_input_alpha_channel: self.settings.policies.discard_input_alpha_channel,
            exif: self.exif_policy()?,
            heif_image_policy: self.settings.policies.heif_image_policy,
            animated_input: self.settings.policies.animated_input,
            max_animation_memory_mib: self.settings.policies.max_animation_memory_mib,
        })
    }

    /// Builds the [`JobSpec`] of the current queue + settings (pure; the
    /// `Files` selection mirrors the CLI's `JobSpec::from_conversion`
    /// semantics, with the output directory overriding `common.output`).
    ///
    /// # Errors
    ///
    /// The EXIF selector parse error, or the plan-15 F17 output blocker
    /// (directory mode without a folder — never a silent same-as-input
    /// fallback).
    pub fn build_job_spec(&self) -> Result<JobSpec, String> {
        if let Some(message) = self.output_blocker() {
            return Err(message);
        }
        let common = self.build_conversion_config()?;
        let mut encoder = self.settings.encoder.clone();
        inject_exif_policy(&mut encoder, &common.exif);
        Ok(JobSpec {
            inputs: InputSelection::Files(self.queue.selection()),
            output: match self.settings.policies.output_mode {
                OutputMode::SameAsInput => None,
                OutputMode::Directory => Some(PathBuf::from(&self.settings.policies.output_dir)),
            },
            encoder,
            common,
        })
    }

    /// Whether a job can start right now (queue non-empty, no job running,
    /// encoder compiled in, EXIF selectors valid, output target resolvable).
    #[must_use]
    pub fn can_start(&self) -> bool {
        self.start_blocker().is_none()
    }

    /// The plan-15 F17 output blocker (its own accessor so the footer can
    /// render it in the error line, not only as the button tooltip):
    /// directory mode with a blank path — never a silent same-as-input
    /// fallback.
    #[must_use]
    pub fn output_blocker(&self) -> Option<String> {
        (self.settings.policies.output_mode == OutputMode::Directory
            && self.settings.policies.output_dir.trim().is_empty())
        .then(|| {
            "output: directory mode is selected but no folder is set — pick one or switch to \
             'same as input'"
                .to_string()
        })
    }

    /// The reason a job cannot start right now (`None` when it can);
    /// rendered as the Convert button's disabled tooltip by the footer.
    #[must_use]
    pub fn start_blocker(&self) -> Option<String> {
        if self.running.is_some() {
            return Some("a job is already running".to_string());
        }
        if self.queue.is_empty() {
            return Some("the queue is empty".to_string());
        }
        if !self.encoder_available() {
            let reason = self
                .encoder_unavailable_reason()
                .unwrap_or("encoder not available in this build");
            return Some(format!("selected encoder is unavailable: {reason}"));
        }
        if let Some(message) = self.output_blocker() {
            return Some(message);
        }
        self.exif_policy().err()
    }

    // ---- job lifecycle ----------------------------------------------------

    /// Starts a conversion job over the whole queue. A start attempt while
    /// blocked is surfaced in the footer error line via `start_error`
    /// (plan 15 F17) instead of failing silently — the button is normally
    /// disabled, so this is the defense-in-depth path.
    ///
    /// # Panics
    ///
    /// Panics if the internal event channel is closed unexpectedly (the
    /// receiver always lives on the same app instance).
    pub fn start_job(&mut self) {
        if self.running.is_some() {
            return;
        }
        if let Some(message) = self.start_blocker() {
            self.start_error = Some(message);
            return;
        }
        let spec = match self.build_job_spec() {
            Ok(spec) => spec,
            Err(message) => {
                self.start_error = Some(message);
                return;
            }
        };
        self.start_error = None;
        self.report = None;
        self.event_log.clear();
        // a new run retires the previous celebration
        self.confetti = None;
        self.flourish = None;
        let target = self.target_extension();
        self.queue.begin_run(target);
        let (tx, rx) = mpsc::channel::<JobEvent>();
        let reporter: Box<dyn Reporter> = Box::new(ChannelReporter::new(tx));
        let stop = StopFlag::new();
        self.event_rx = Some(rx);
        self.running = Some(RunningJob {
            handle: Some(JobHandle::start(
                spec,
                reporter,
                stop.clone(),
                &self.session,
            )),
            stop,
            stats: FooterStats::default(),
            total_items: None,
            finished_items: 0,
            active: std::collections::HashSet::new(),
            segments: Vec::new(),
            seg_paths: Vec::new(),
            started_at: Instant::now(),
        });
    }

    /// Requests cancelation of the running job (drains the current file,
    /// marks the rest aborted — CLI Ctrl+C semantics).
    pub fn cancel_job(&mut self) {
        if let Some(job) = &self.running {
            job.stop.raise();
        }
    }

    /// Requests a quality measurement for a converted pair (plan 10
    /// §phase 2 "Measure quality"): no-op when metrics are `Off`. The
    /// decode+compare run on the metric worker; the result lands in
    /// [`App::metrics`] (rendered by the file table + report).
    pub fn measure_quality(&mut self, input: &std::path::Path, output: &std::path::Path) {
        if self.settings.quality_metric == MetricMode::Off {
            return;
        }
        self.metrics.request_measure(
            input.to_path_buf(),
            output.to_path_buf(),
            self.settings.metric_engine,
            self.settings.metric_max_edge_clamped(),
        );
    }

    /// Opens the visual difference inspector for a converted pair (plan
    /// 10 §phase 3): the bounded decode (2048 px) runs on the metric
    /// worker — the UI thread only uploads the returned buffers as
    /// textures when they arrive.
    pub fn open_inspector(&mut self, input: &std::path::Path, output: &std::path::Path) {
        let input = input.to_path_buf();
        let output = output.to_path_buf();
        self.metrics.request_inspect(input.clone(), output.clone());
        self.inspector = Some(crate::panels::inspector::InspectorState::new(input, output));
    }
    /// Drains the event channel of the running job into the UI state.
    pub fn drain_events(&mut self) {
        loop {
            let event = {
                let Some(rx) = &self.event_rx else {
                    return;
                };
                match rx.try_recv() {
                    Ok(event) => event,
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
                }
            };
            self.apply_event(event);
        }
    }

    /// Applies one event to the UI state (pure state machine; called from
    /// [`App::drain_events`] only).
    fn apply_event(&mut self, event: JobEvent) {
        self.event_log.push(event.clone());
        match event {
            JobEvent::Started { total_files } => {
                if let Some(job) = self.running.as_mut() {
                    job.total_items = Some(total_files);
                    // per-item segments are sized on discovery (plan 12 §2)
                    let total = total_files as usize;
                    job.segments = vec![SegState::Pending; total];
                    job.seg_paths = vec![None; total];
                }
            }
            JobEvent::FileStarted { index, path } => {
                if let Some(job) = self.running.as_mut() {
                    job.active.insert(path.clone());
                    if let Some(segment) = job.segments.get_mut(index as usize) {
                        *segment = SegState::Active;
                    }
                    if let Some(slot) = job.seg_paths.get_mut(index as usize) {
                        *slot = Some(path.clone());
                    }
                }
                self.queue.apply_file_started(&path);
            }
            JobEvent::FileFinished {
                index,
                path,
                outcome,
            } => {
                if let Some(job) = self.running.as_mut() {
                    job.finished_items += 1;
                    job.active.remove(&path);
                    let state = SegState::from_outcome(&outcome);
                    if let Some(segment) = job.segments.get_mut(index as usize) {
                        *segment = state;
                    }
                    if let Some(slot) = job.seg_paths.get_mut(index as usize) {
                        *slot = Some(path.clone());
                    }
                }
                if !self.queue.apply_file_finished(&path, &outcome) {
                    // discovered by directory/HEIF expansion: show as row
                    self.queue.push_discovered(path, &outcome);
                }
            }
            JobEvent::Notice { path, message } => {
                if let Some(path) = path {
                    self.queue_note(&path, message);
                }
            }
            JobEvent::ProgressStats {
                input_bytes,
                output_bytes,
                ok,
                skipped,
                errors,
            } => {
                if let Some(job) = self.running.as_mut() {
                    job.stats = FooterStats {
                        input_bytes,
                        output_bytes,
                        ok,
                        skipped,
                        errors,
                    };
                }
            }
            JobEvent::Finished => {
                self.finish_job();
            }
        }
    }

    /// Attaches a per-file notice (collision/error warnings) to its queue
    /// row as the note text.
    fn queue_note(&mut self, path: &std::path::Path, message: String) {
        let Some(item) = self
            .queue
            .items_mut()
            .iter_mut()
            .find(|item| item.path == path)
        else {
            return;
        };
        // never overwrite the error text with a notice
        if item.error.is_none() {
            item.note = Some(message);
        }
    }

    /// Fills the lazy per-row data the file table requested for its
    /// visible slice (plan 11 §4/§6): thumbnail requests go to the
    /// background worker, EXIF summaries ride the same worker, dimensions
    /// are read right here (a header-only stat, ~µs — safe on the UI
    /// thread, cached on the row afterwards).
    pub fn request_row_data(
        &mut self,
        visible_keys: &[ThumbKey],
        need_dimensions: &[PathBuf],
        need_exif: &[PathBuf],
    ) {
        self.thumbs.request_thumbs(visible_keys);
        for path in need_exif {
            self.thumbs.request_exif(path.clone());
        }
        for path in need_dimensions {
            let Some(item) = self
                .queue
                .items_mut()
                .iter_mut()
                .find(|item| &item.path == path)
            else {
                continue;
            };
            if item.dimensions.is_none() {
                item.dimensions = Some(crate::thumb::read_dimensions(path).unwrap_or((0, 0)));
            }
        }
    }

    /// Completes the running job: joins the worker (instant after
    /// `Finished`), collects the report, resets the queue run state and
    /// arms the celebration (plan 12 §3).
    fn finish_job(&mut self) {
        let Some(mut job) = self.running.take() else {
            return;
        };
        self.event_rx = None;
        let report = job
            .handle
            .take()
            .map(byteshaver::job::JobHandle::join)
            .unwrap_or_default();
        job.active.clear(); // plan 12 §1: the active set dies with the run
        self.queue.finish_run();
        if let Some(message) = &report.error {
            self.start_error = Some(message.clone());
        }
        self.celebrate_run(&report);
        // plan 10 §phase 2 AutoAfterRun: enqueue measurements for every
        // successfully encoded file (the metric worker just un-paused)
        let auto_pairs =
            crate::metrics::auto_measure_pairs(&report, self.settings.quality_metric);
        self.report = Some(report);
        for (input, output) in auto_pairs {
            self.metrics.request_measure(
                input,
                output,
                self.settings.metric_engine,
                self.settings.metric_max_edge_clamped(),
            );
        }
    }

    /// Decides and arms the post-run celebration (plan 12 §3): a
    /// compression-ratio-scaled confetti burst emitted at the
    /// Convert-button corner — or, under `reduced_motion`, a fading text
    /// flourish with the same percentage. Pure trigger decision in
    /// [`crate::celebrate::should_celebrate`].
    fn celebrate_run(&mut self, report: &RunReport) {
        let summary = crate::celebrate::RunSummary {
            successful: report.totals.successful,
            input_size: report.totals.input_size,
            output_size: report.totals.output_size,
            input_files: report.totals.input_files,
            aborted: report.totals.aborted,
            failed: report.error.is_some(),
        };
        if !crate::celebrate::should_celebrate(&summary) {
            return;
        }
        // should_celebrate guarantees input_size > output_size ≥ 0
        let ratio = report.totals.output_size as f32 / report.totals.input_size as f32;
        if self.settings.reduced_motion {
            let percent = ((1.0 - ratio) * 100.0).round().clamp(0.0, 100.0) as u32;
            self.flourish = Some(TextFlourish::new(percent));
        } else {
            let count = crate::celebrate::confetti_count(self.settings.confetti, ratio);
            if count == 0 {
                return;
            }
            let [_, height] = self.viewport_size;
            let origin = RectPx {
                x: 12.0,
                y: (height - 84.0).max(0.0),
                w: 240.0,
                h: 48.0,
            };
            // deterministic seed per run → identical replays
            let totals = &report.totals;
            let seed = totals.input_size
                ^ totals.output_size.rotate_left(17)
                ^ totals.successful.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            self.confetti = Some(Confetti::burst(
                count,
                origin,
                &mut crate::celebrate::Lcg::new(seed),
            ));
        }
    }
}

/// Injects the global EXIF policy into the encoder options that consume it
/// (oxipng, jxl) — byte-for-byte the CLI's logic in `main.rs`.
///
/// Deliberately **not** `#[cfg(feature = …)]`-gated: the cfg switches
/// belong to the `byteshaver` dependency (whose default features this
/// crate always enables), not to `byteshaver-gui` — gating on gui-local
/// features would silently compile the injection away.
pub fn inject_exif_policy(encoder: &mut EncoderConfig, policy: &ExifPolicy) {
    if let EncoderConfig::Oxipng(oxipng_options) = encoder {
        oxipng_options.exif_policy = policy.clone();
    }
    if let EncoderConfig::Jxl(jxl_options) = encoder {
        jxl_options.exif_policy = policy.clone();
    }
}

impl eframe::App for App {
    /// Renders one frame: reads drag-and-drop input, drains job events,
    /// draws all panels and handles settings persistence.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. drag & drop (the whole window is a drop target)
        let (dropped, hovered) = ctx.input(|input| {
            (
                input.raw.dropped_files.clone(),
                input.raw.hovered_files.clone(),
            )
        });
        self.drag_hovered = !hovered.is_empty();
        if !dropped.is_empty() {
            let paths = dropped
                .into_iter()
                .filter_map(|file| file.path)
                .collect::<Vec<_>>();
            if !paths.is_empty() {
                self.queue.add_paths(paths);
            }
        }

        // 2. job events first, so panels render fresh state
        if self.running.is_some() {
            self.drain_events();
        }

        // 2.5 thumbnail/EXIF worker results: textures land in the LRU
        // cache, summaries onto their queue rows (plan 11 §6); the worker
        // pauses while a job runs (decode would contend with rayon)
        {
            let queue = &mut self.queue;
            self.thumbs.poll(ctx, &mut |path, summary| {
                if let Some(item) = queue.items_mut().iter_mut().find(|item| item.path == path) {
                    item.exif = Some(summary);
                }
            });
        }
        self.thumbs.set_paused(self.running.is_some());

        // 2.6 metric worker results (plan 10 §phase 2/3): measurements
        // land in the cache, inspector buffers become textures in the
        // callback; the worker pauses while a job runs (same policy as
        // the thumbnail worker). An engine-settings change invalidates
        // the cache so rows/aggregate never mix engines silently.
        if self.metrics.engine != self.settings.metric_engine {
            self.metrics.engine = self.settings.metric_engine;
            self.metrics.clear_results();
        }
        self.metrics.poll(|input, output, a, b| {
            let Some(state) = &mut self.inspector else {
                return; // window closed meanwhile → buffers dropped
            };
            if !state.matches(&input, &output) {
                return; // stale result of an earlier pair
            }
            match (a, b) {
                (Some(a), Some(b)) => state.receive(ctx, a, b),
                _ => state.receive_failed(),
            }
        });
        self.metrics.set_paused(self.running.is_some());

        // 3. panels
        crate::panels::show(self, ctx);

        // 4. keep repainting while a job runs
        if self.running.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // 5. remember the window size for the next launch (and keep the
        // confetti origin anchored to the current window)
        let size = ctx.input(|input| input.viewport().inner_rect.map(|rect| rect.size()));
        if let Some([width, height]) = size.map(|size| [size.x, size.y]) {
            self.viewport_size = [width, height];
            if self.settings.window_size != Some([width, height]) {
                self.settings.window_size = Some([width, height]);
                self.settings_dirty = true;
            }
        }

        // 6. persist changed settings (cheap JSON write, at most per change);
        // the per-frame window-size capture above keeps the persisted size
        // current without a separate on_exit hook
        if self.settings_dirty() {
            self.save_settings();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::celebrate::ConfettiLevel;
    use crate::presets::{Preset, PresetContent};
    use crate::queue::ItemStatus;
    use crate::settings::{CollisionChoice, ExifMode, ExifSettings, OutputMode, PolicySet};
    use byteshaver::cli::CliArgs;
    use byteshaver::config::ConversionConfig;
    use byteshaver::pipeline::Outcome;
    use clap::Parser;
    use std::path::Path;
    use std::time::Duration;

    fn default_app() -> App {
        App::with_settings(Settings::default())
    } // (App::new is exercised through main; tests build explicitly)

    // ---- spec construction vs CLI semantics -------------------------------

    #[test]
    fn default_policies_match_conversion_config_from_args() {
        let app = App::with_settings(Settings::default());
        let mut gui_config = app.build_conversion_config().expect("valid defaults");
        let args = CliArgs::parse_from(["byteshaver", "pattern", "webp"]);
        let mut cli_config = ConversionConfig::from_args(&args);
        // input selection differs by design: the GUI uses InputSelection::Files,
        // the CLI the glob pattern — `pattern` is unused by the GUI
        gui_config.pattern = String::new();
        cli_config.pattern = String::new();
        assert_eq!(
            gui_config, cli_config,
            "GUI defaults must equal CLI defaults"
        );
    }

    #[test]
    fn policy_mirrors_map_onto_the_cli_flag_values() {
        let mut app = default_app();
        app.settings.policies.collision = CollisionChoice::OverwriteIfSmaller;
        app.settings.policies.animated_input = byteshaver::config::AnimatedInputPolicy::Error;
        app.settings.policies.discard_if_larger_than_input = true;
        app.settings.policies.discard_input_alpha_channel = true;
        app.settings.policies.max_animation_memory_mib = 512;
        app.settings.policies.reverse_processing_order = true;
        app.settings.policies.exif = ExifSettings {
            mode: ExifMode::FilterExcept,
            except_tags: "gps".to_string(),
            only_tags: String::new(),
        };

        let mut gui_config = app.build_conversion_config().expect("valid settings");
        let args = CliArgs::parse_from([
            "byteshaver",
            "pattern",
            "--overwrite-if-smaller",
            "--animated-input",
            "error",
            "--discard-if-larger-than-input",
            "--discard-input-alpha-channel",
            "--max-animation-memory",
            "512",
            "--reverse-processing-order",
            "--exif",
            "filter",
            "--exif-except",
            "gps",
            "webp",
        ]);
        let mut cli_config = ConversionConfig::from_args(&args);
        gui_config.pattern = String::new();
        cli_config.pattern = String::new();
        assert_eq!(
            gui_config, cli_config,
            "every GUI policy must map onto its CLI flag"
        );
    }

    #[test]
    fn job_spec_uses_files_selection_and_output_override() {
        let mut app = default_app();
        app.queue.add_paths(vec![PathBuf::from("/x/a.png")]);
        app.settings.policies.output_mode = OutputMode::Directory;
        app.settings.policies.output_dir = "/tmp/out".to_string();
        let spec = app.build_job_spec().expect("valid settings");
        assert_eq!(
            spec.inputs,
            InputSelection::Files(vec![PathBuf::from("/x/a.png")])
        );
        assert_eq!(spec.output, Some(PathBuf::from("/tmp/out")));
        // the common config stays CLI-default-shaped (pattern/output unused)
        assert_eq!(spec.common.pattern, "");
        assert_eq!(spec.common.output, "");

        // same-as-input mode means "same as input" (CLI default); the
        // stored path text survives the switch untouched (plan 15 F16)
        app.settings.policies.output_mode = OutputMode::SameAsInput;
        let spec = app.build_job_spec().expect("valid settings");
        assert_eq!(spec.output, None);

        // plan 15 F17: directory mode with a blank path is an error, never
        // a silent same-as-input fallback
        app.settings.policies.output_mode = OutputMode::Directory;
        app.settings.policies.output_dir = "   ".to_string();
        let err = app
            .build_job_spec()
            .expect_err("blank directory must not fall back");
        assert!(err.contains("directory mode"), "{err}");
    }

    #[test]
    fn output_blocker_matrix_covers_mode_and_path() {
        let mut app = default_app();
        app.queue.add_paths(vec![PathBuf::from("/x/a.png")]);
        let blocker = |app: &App| app.start_blocker().expect("blocked");

        // same as input (with or without a stored path) never blocks
        app.settings.policies.output_mode = OutputMode::SameAsInput;
        app.settings.policies.output_dir = String::new();
        assert!(app.output_blocker().is_none());
        assert!(app.can_start());
        app.settings.policies.output_dir = "/kept".to_string();
        assert!(app.output_blocker().is_none());

        // directory mode: blank/whitespace paths block, non-blank passes
        app.settings.policies.output_mode = OutputMode::Directory;
        app.settings.policies.output_dir = String::new();
        assert!(app.output_blocker().is_some());
        app.settings.policies.output_dir = "  ".to_string();
        let message = blocker(&app);
        assert!(
            message.contains("directory mode is selected but no folder is set"),
            "{message}"
        );
        assert!(!app.can_start());
        app.settings.policies.output_dir = "/real/dir".to_string();
        assert!(app.output_blocker().is_none());
        assert!(app.can_start());

        // a start attempt while blocked surfaces the message (plan 15 F17)
        app.settings.policies.output_dir = String::new();
        app.start_job();
        assert!(app.running.is_none(), "the blocked start never runs");
        assert_eq!(app.start_error.as_deref(), Some(&*blocker(&app)));
    }

    #[test]
    fn exif_policy_is_injected_into_oxipng_and_jxl_options() {
        let mut app = default_app();
        app.settings.policies.exif = ExifSettings {
            mode: ExifMode::FilterExcept,
            except_tags: "gps,Orientation".to_string(),
            only_tags: String::new(),
        };

        app.select_encoder("oxipng");
        let spec = app.build_job_spec().expect("valid settings");
        let EncoderConfig::Oxipng(options) = &spec.encoder else {
            panic!("expected oxipng config");
        };
        assert_eq!(
            options.exif_policy,
            app.exif_policy().expect("valid selectors"),
            "oxipng options must carry the global policy (main.rs behavior)"
        );

        app.select_encoder("jxl");
        let spec = app.build_job_spec().expect("valid settings");
        let EncoderConfig::Jxl(options) = &spec.encoder else {
            panic!("expected jxl config");
        };
        assert_eq!(
            options.exif_policy,
            app.exif_policy().expect("valid selectors")
        );

        // unchanged when the encoder ignores metadata policies
        app.select_encoder("webp");
        let spec = app.build_job_spec().expect("valid settings");
        assert!(matches!(spec.encoder, EncoderConfig::Webp(_)));
    }

    #[test]
    fn invalid_exif_selectors_block_the_start() {
        let mut app = default_app();
        app.settings.policies.exif = ExifSettings {
            mode: ExifMode::KeepOnly,
            only_tags: "NoSuchTag!!!".to_string(),
            except_tags: String::new(),
        };
        app.queue.add_paths(vec![PathBuf::from("/x/a.png")]);
        assert!(!app.can_start());
        assert!(app.build_job_spec().is_err());
    }

    #[test]
    fn start_is_blocked_on_empty_queue_and_unknown_encoder() {
        let mut app = default_app();
        assert!(!app.can_start(), "empty queue");
        app.queue.add_paths(vec![PathBuf::from("/x/a.png")]);
        assert!(app.can_start());
        // a settings file written by a build with more features can select
        // an encoder this build does not have; simulate by patching caps
        app.capabilities.encoders.retain(|info| info.name != "webp");
        app.settings.encoder = EncoderConfig::Webp(Default::default());
        // encoder_kind_name still resolves; simulate a truly unknown kind by
        // clearing the whole registry
        app.capabilities.encoders.clear();
        assert!(!app.can_start(), "no capable encoder selected");
        assert!(!app.select_encoder("nope"));
    }

    // ---- event application -------------------------------------------------

    #[test]
    fn events_update_footer_and_queue_rows_in_order() {
        let mut app = default_app();
        app.queue.add_paths(vec![PathBuf::from("/x/a.png")]);
        app.start_job();
        assert!(app.running.is_some());
        assert_eq!(app.queue.items()[0].status, ItemStatus::Running);

        app.apply_event(JobEvent::Started { total_files: 1 });
        app.apply_event(JobEvent::FileStarted {
            index: 0,
            path: PathBuf::from("/x/a.png"),
        });
        app.apply_event(JobEvent::FileFinished {
            index: 0,
            path: PathBuf::from("/x/a.png"),
            outcome: Outcome::Encoded {
                input_size: 10,
                output_size: 5,
                metadata_dropped: false,
                output_path: PathBuf::from("/x/a.webp"),
            },
        });
        app.apply_event(JobEvent::ProgressStats {
            input_bytes: 10,
            output_bytes: 5,
            ok: 1,
            skipped: 0,
            errors: 0,
        });
        let job = app.running.as_ref().expect("job running");
        assert_eq!(job.finished_items, 1);
        assert_eq!(job.stats.ok, 1);
        assert_eq!(job.progress(), Some(1.0));
        // plan 11 §5: the outcome's real on-disk output path lands on the
        // row and enables the open/reveal actions
        assert_eq!(
            app.queue.items()[0].output_path,
            Some(PathBuf::from("/x/a.webp"))
        );
        assert_eq!(app.queue.items()[0].converted_to, Some("webp"));

        app.apply_event(JobEvent::Finished);
        assert!(app.running.is_none());
        let report = app.report.expect("report collected");
        assert_eq!(
            report.totals.successful, 0,
            "join of a never-started worker"
        );
        assert_eq!(app.queue.items()[0].status, ItemStatus::Encoded);
    }

    #[test]
    fn notices_attach_to_rows_and_unknown_paths_become_discovered_rows() {
        let mut app = default_app();
        app.queue.add_paths(vec![PathBuf::from("/x/dir")]);
        app.start_job();

        app.apply_event(JobEvent::Started { total_files: 1 });
        app.apply_event(JobEvent::Notice {
            path: Some(PathBuf::from("/x/dir/inner.png")),
            message: "collision-ish warning".to_string(),
        });
        app.apply_event(JobEvent::FileFinished {
            index: 0,
            path: PathBuf::from("/x/dir/inner.png"),
            outcome: Outcome::Encoded {
                input_size: 4,
                output_size: 2,
                metadata_dropped: false,
                output_path: PathBuf::from("/x/out/inner.webp"),
            },
        });
        assert_eq!(app.queue.len(), 2, "discovered rows are appended");
        assert!(app.queue.items()[1].discovered);
        assert_eq!(app.queue.items()[1].status, ItemStatus::Encoded);
    }

    // ---- plan 12 §1/§2: active set + per-item segments ----------------------

    fn encoded_outcome(input: u64, output: u64) -> Outcome {
        Outcome::Encoded {
            input_size: input,
            output_size: output,
            metadata_dropped: false,
            output_path: PathBuf::from("/x/out.img"),
        }
    }

    #[test]
    fn active_set_tracks_started_without_matching_finished() {
        let mut app = default_app();
        app.queue
            .add_paths(vec![PathBuf::from("/x/a.png"), PathBuf::from("/x/b.png")]);
        app.start_job();
        app.apply_event(JobEvent::Started { total_files: 3 });
        // rayon parallelism: two files in flight at once coexist
        app.apply_event(JobEvent::FileStarted {
            index: 0,
            path: PathBuf::from("/x/a.png"),
        });
        app.apply_event(JobEvent::FileStarted {
            index: 1,
            path: PathBuf::from("/x/b.png"),
        });
        {
            let job = app.running.as_ref().expect("job running");
            assert_eq!(job.active.len(), 2);
            assert!(job.active.contains(Path::new("/x/a.png")));
            assert!(job.active.contains(Path::new("/x/b.png")));
            assert_eq!(
                job.segments,
                vec![SegState::Active, SegState::Active, SegState::Pending]
            );
            assert_eq!(job.seg_paths[1].as_deref(), Some(Path::new("/x/b.png")));
        }
        // finishes remove exactly their own path
        app.apply_event(JobEvent::FileFinished {
            index: 0,
            path: PathBuf::from("/x/a.png"),
            outcome: encoded_outcome(100, 40),
        });
        let job = app.running.as_ref().expect("job running");
        assert_eq!(job.active.len(), 1);
        assert!(job.active.contains(Path::new("/x/b.png")));
        assert!(!job.active.contains(Path::new("/x/a.png")));
        assert_eq!(
            job.segments[0],
            SegState::Done {
                tone: SegTone::Good,
                ratio: Some(0.4),
            }
        );
        assert_eq!(job.segments[2], SegState::Pending);
        assert_eq!(job.finished_items, 1);
    }

    #[test]
    fn aborted_outcome_leaves_the_active_set_and_maps_to_skipped() {
        let mut app = default_app();
        app.queue.add_paths(vec![PathBuf::from("/x/a.png")]);
        app.start_job();
        app.apply_event(JobEvent::Started { total_files: 1 });
        app.apply_event(JobEvent::FileStarted {
            index: 0,
            path: PathBuf::from("/x/a.png"),
        });
        app.apply_event(JobEvent::FileFinished {
            index: 0,
            path: PathBuf::from("/x/a.png"),
            outcome: Outcome::Aborted,
        });
        let job = app.running.as_ref().expect("job running");
        assert!(job.active.is_empty(), "aborted files are no longer active");
        assert_eq!(job.segments[0].tone(), SegTone::Skipped);
    }

    #[test]
    fn outcome_maps_onto_segment_state_bands() {
        let done = |tone, ratio| SegState::Done { tone, ratio };
        // ratio bands: ≤ 80 % good, 80–100 % neutral, > 100 % grew
        assert_eq!(
            SegState::from_outcome(&encoded_outcome(100, 80)),
            done(SegTone::Good, Some(0.8))
        );
        assert_eq!(
            SegState::from_outcome(&encoded_outcome(100, 81)),
            done(SegTone::Neutral, Some(0.81))
        );
        assert_eq!(
            SegState::from_outcome(&encoded_outcome(100, 100)),
            done(SegTone::Neutral, Some(1.0))
        );
        assert_eq!(
            SegState::from_outcome(&encoded_outcome(100, 101)),
            done(SegTone::Grew, Some(1.01))
        );
        assert_eq!(
            SegState::from_outcome(&Outcome::DiscardedLargerThanInput {
                input_size: 100,
                encoded_size: 120,
            }),
            done(SegTone::Grew, Some(1.2)),
            "discarded-larger counts as grew (plan 10 §phase 1)"
        );
        // skipped / collision / aborted share the dark-gray tone
        assert_eq!(
            SegState::from_outcome(&Outcome::SkippedExisting {
                input_size: 100,
                existing_size: 50,
                output_path: PathBuf::from("/x/old.img"),
            }),
            done(SegTone::Skipped, None)
        );
        assert_eq!(
            SegState::from_outcome(&Outcome::SkippedCollision {
                input_size: 100,
                output_path: PathBuf::from("x"),
            }),
            done(SegTone::Skipped, None)
        );
        assert_eq!(
            SegState::from_outcome(&Outcome::DiscardedLargerThanExisting {
                input_size: 100,
                existing_size: 50,
            }),
            done(SegTone::Skipped, None)
        );
        assert_eq!(
            SegState::from_outcome(&Outcome::Aborted),
            done(SegTone::Skipped, None)
        );
        // errors are red
        assert_eq!(
            SegState::from_outcome(&Outcome::Error("boom".to_string())),
            done(SegTone::Error, None)
        );
    }

    #[test]
    fn ratio_tone_bands_at_boundaries() {
        assert_eq!(ratio_tone(0.0), SegTone::Good);
        assert_eq!(ratio_tone(0.79), SegTone::Good);
        assert_eq!(ratio_tone(0.8), SegTone::Good);
        assert_eq!(ratio_tone(0.81), SegTone::Neutral);
        assert_eq!(ratio_tone(1.0), SegTone::Neutral);
        assert_eq!(ratio_tone(1.001), SegTone::Grew);
        // non-finite ratios never crash the band lookup
        assert_eq!(ratio_tone(f32::NAN), SegTone::Neutral);
        assert_eq!(ratio_tone(f32::INFINITY), SegTone::Neutral);
    }

    // ---- plan 12 §3: celebration trigger ------------------------------------

    fn run_report(successful: u64, input: u64, output: u64) -> RunReport {
        RunReport {
            elapsed: Duration::ZERO,
            files: Vec::new(),
            totals: byteshaver::job::Totals {
                input_files: successful,
                successful,
                skipped: 0,
                discarded: 0,
                collisions: 0,
                errors: 0,
                aborted: 0,
                input_size: input,
                output_size: output,
            },
            metadata_dropped_count: 0,
            error: None,
        }
    }

    #[test]
    fn celebration_arms_per_report_and_settings() {
        // Regular + a real gain → confetti (ratio 0.4 → factor 1.12 → 134)
        let mut app = default_app();
        app.celebrate_run(&run_report(2, 100, 40));
        let confetti = app.confetti.as_ref().expect("confetti burst");
        assert_eq!(confetti.visible(), 134);
        assert!(app.flourish.is_none());

        // Off → nothing at all
        let mut app = default_app();
        app.settings.confetti = ConfettiLevel::Off;
        app.celebrate_run(&run_report(1, 100, 40));
        assert!(app.confetti.is_none());
        assert!(app.flourish.is_none());

        // reduced motion → the text flourish with the saved percentage
        let mut app = default_app();
        app.settings.reduced_motion = true;
        app.celebrate_run(&run_report(1, 100, 38));
        let flourish = app.flourish.as_ref().expect("flourish");
        assert_eq!(flourish.text, "✔ 62 % saved — nice run.");
        assert!(app.confetti.is_none());

        // grew / no gain / nothing successful → no celebration
        let mut app = default_app();
        app.celebrate_run(&run_report(1, 100, 120));
        assert!(app.confetti.is_none() && app.flourish.is_none());
        app.celebrate_run(&run_report(1, 100, 100));
        assert!(app.confetti.is_none() && app.flourish.is_none());
        app.celebrate_run(&run_report(0, 100, 40));
        assert!(app.confetti.is_none() && app.flourish.is_none());

        // pre-flight failure (report.error) → no celebration
        let mut failed = run_report(1, 100, 40);
        failed.error = Some("pre-flight broke".to_string());
        app.celebrate_run(&failed);
        assert!(app.confetti.is_none() && app.flourish.is_none());

        // fully aborted run → no celebration (partial aborts still do)
        let mut aborted = run_report(0, 100, 40);
        aborted.totals.aborted = 3;
        aborted.totals.input_files = 3;
        app.celebrate_run(&aborted);
        assert!(app.confetti.is_none() && app.flourish.is_none());
        let mut partial = run_report(1, 100, 40);
        partial.totals.aborted = 1;
        partial.totals.input_files = 2;
        app.celebrate_run(&partial);
        assert!(app.confetti.is_some(), "partial success still celebrates");
    }

    // ---- files-vs-pattern run parity (headless, uses the real core) --------

    /// Minimal valid 8x8 RGB PNG used as test fixture (no image dependency
    /// in the gui crate).
    const TEST_PNG: [u8; 74] = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 8, 0, 0, 0, 8, 8, 2,
        0, 0, 0, 75, 109, 41, 220, 0, 0, 0, 17, 73, 68, 65, 84, 120, 218, 99, 120, 32, 160, 128,
        21, 49, 12, 45, 9, 0, 194, 78, 68, 1, 198, 78, 54, 47, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
        96, 130,
    ];

    /// Runs a job headless and returns the report.
    fn run(spec: JobSpec) -> RunReport {
        let session = Session::new();
        JobHandle::start(
            spec,
            Box::new(byteshaver::job::NullReporter::new()),
            StopFlag::new(),
            &session,
        )
        .join()
    }

    #[test]
    fn files_selection_matches_pattern_discovery_output_trees() {
        let base =
            std::env::temp_dir().join(format!("byteshaver-gui-parity-{}", std::process::id()));
        let input_dir = base.join("photos");
        let subdir = input_dir.join("sub");
        std::fs::create_dir_all(&subdir).expect("create fixture dirs");
        std::fs::write(subdir.join("img.png"), TEST_PNG).expect("write fixture png");
        let out_pattern = base.join("out-pattern");
        let out_files = base.join("out-files");

        let mut app = default_app();
        app.select_encoder("webp");
        app.settings.policies.output_mode = OutputMode::Directory;
        app.settings.policies.output_dir = out_files.display().to_string();
        // enqueue the directory only — the core must expand it recursively
        app.queue.add_paths(vec![input_dir.clone()]);

        // GUI-shaped run: InputSelection::Files([directory])
        let spec_files = app.build_job_spec().expect("valid settings");
        assert!(matches!(spec_files.inputs, InputSelection::Files(_)));
        let report_files = run(spec_files);
        assert!(report_files.error.is_none(), "{:?}", report_files.error);
        assert_eq!(report_files.totals.successful, 1);
        assert_eq!(report_files.totals.input_files, 1);

        // CLI-shaped run: the classic glob pattern over the same tree
        let mut cli_app = default_app();
        cli_app.select_encoder("webp");
        let mut common = cli_app.build_conversion_config().expect("valid defaults");
        common.pattern = format!("{}/**/*.png", input_dir.display());
        common.output = out_pattern.display().to_string();
        let mut encoder = EncoderConfig::Webp(Default::default());
        inject_exif_policy(&mut encoder, &common.exif);
        let spec_pattern = JobSpec::from_conversion(common, encoder);
        let report_pattern = run(spec_pattern);
        assert!(report_pattern.error.is_none(), "{:?}", report_pattern.error);
        assert_eq!(report_pattern.totals.successful, 1);

        // identical output trees (same files, byte-identical encodes)
        assert_eq!(
            report_files.totals.output_size, report_pattern.totals.output_size,
            "aggregate output size must match"
        );
        let files_a = report_files.files;
        let files_b = report_pattern.files;
        assert_eq!(files_a.len(), files_b.len());
        for (result_a, result_b) in files_a.iter().zip(files_b.iter()) {
            assert_eq!(
                result_a.path.file_name(),
                result_b.path.file_name(),
                "relative output naming must match"
            );
            let bytes_a = std::fs::read(&result_a.path).expect("read files-run output");
            let bytes_b = std::fs::read(&result_b.path).expect("read pattern-run output");
            assert_eq!(bytes_a, bytes_b, "encodes must be byte-identical");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- settings persistence ----------------------------------------------

    #[test]
    fn settings_dirty_flag_round_trips() {
        let mut app = default_app();
        assert!(!app.settings_dirty());
        app.mark_settings_dirty();
        assert!(app.settings_dirty());
        app.save_settings();
        assert!(!app.settings_dirty());
    }

    #[test]
    fn encoder_selection_resets_options_and_marks_dirty() {
        let mut app = default_app();
        assert!(matches!(app.settings.encoder, EncoderConfig::Webp(_)));
        assert!(app.select_encoder("avif"));
        assert!(matches!(app.settings.encoder, EncoderConfig::Avif(_)));
        assert!(app.settings_dirty());
        assert!(!app.select_encoder("nope"));
        assert!(matches!(app.settings.encoder, EncoderConfig::Avif(_)));
    }

    // ---- plan 14: presets (apply path, active tracking, store ops) ------------

    fn builtin_preset(title: &str) -> Preset {
        crate::presets::builtin()
            .into_iter()
            .find(|preset| preset.title == title)
            .unwrap_or_else(|| panic!("missing built-in {title}"))
    }

    fn full_preset(include_output_dir: bool) -> Preset {
        let mut preset = builtin_preset("WebP · Balanced");
        preset.builtin = false;
        preset.title = "Full Shape".to_string();
        preset.content.policies = Some(PolicySet {
            // saved while directory output was selected (the mode always
            // applies on apply, plan 15 F16; the path rides only on opt-in)
            output_mode: OutputMode::Directory,
            collision: CollisionChoice::OverwriteIfSmaller,
            discard_if_larger_than_input: true,
            max_animation_memory_mib: 512,
            ..PolicySet::default()
        });
        preset.content.include_output_dir = include_output_dir;
        if include_output_dir
            && let Some(policies) = &mut preset.content.policies
        {
            policies.output_mode = OutputMode::Directory;
            policies.output_dir = "/preset/dir".to_string();
        }
        preset
    }

    #[test]
    fn applying_a_format_only_preset_touches_only_the_encoder() {
        let mut app = default_app();
        app.settings.policies.collision = CollisionChoice::OverwriteAlways;
        app.settings.policies.output_mode = OutputMode::Directory;
        app.settings.policies.output_dir = "/keep/me".to_string();
        let preset = builtin_preset("AVIF · Balanced");
        let encoder_before = app.settings.encoder.clone();

        app.apply_preset(&preset).expect("applies");
        assert_eq!(app.settings.encoder, preset.content.encoder);
        assert_eq!(
            app.settings.policies.collision,
            CollisionChoice::OverwriteAlways,
            "format-only presets leave the policies untouched"
        );
        assert_eq!(
            app.settings.policies.output_mode,
            OutputMode::Directory,
            "format-only presets never touch the output mode (plan 15 F16)"
        );
        assert_eq!(app.settings.policies.output_dir, "/keep/me");
        assert_ne!(app.settings.encoder, encoder_before);
        assert!(app.settings_dirty(), "dirty marked exactly once");
        app.save_settings();
        assert!(!app.settings_dirty());
    }

    #[test]
    fn applying_a_full_preset_applies_the_mode_and_keeps_the_excluded_dir() {
        let mut app = default_app();
        app.settings.policies.output_mode = OutputMode::Directory;
        app.settings.policies.output_dir = "/local/dir".to_string();

        // exclusion (the default save shape): mode applies, string stays
        let preset = full_preset(false);
        app.apply_preset(&preset).expect("applies");
        assert_eq!(
            app.settings.policies.collision,
            CollisionChoice::OverwriteIfSmaller
        );
        assert!(app.settings.policies.discard_if_larger_than_input);
        assert_eq!(
            app.settings.policies.output_mode,
            OutputMode::Directory,
            "the preset's output mode always applies (plan 15 F16)"
        );
        assert_eq!(
            app.settings.policies.output_dir, "/local/dir",
            "a privacy-excluded dir must not wipe the local setting"
        );

        // opt-in: the embedded mode+dir replace the local ones
        let preset = full_preset(true);
        app.apply_preset(&preset).expect("applies");
        assert_eq!(app.settings.policies.output_mode, OutputMode::Directory);
        assert_eq!(app.settings.policies.output_dir, "/preset/dir");
    }

    #[test]
    fn a_same_as_input_preset_switches_the_mode_without_losing_the_path() {
        let mut app = default_app();
        app.settings.policies.output_mode = OutputMode::Directory;
        app.settings.policies.output_dir = "/local/dir".to_string();

        // a full preset saved while "same as input" was selected (the path
        // was never embedded): applying flips the mode back but must not
        // destroy the locally stored directory text (plan 15 F16)
        let mut preset = builtin_preset("WebP · Balanced");
        preset.content.policies = Some(PolicySet::default());
        app.apply_preset(&preset).expect("applies");
        assert_eq!(app.settings.policies.output_mode, OutputMode::SameAsInput);
        assert_eq!(
            app.settings.policies.output_dir, "/local/dir",
            "mode switches never clear the stored path"
        );
    }

    #[test]
    fn applying_a_preset_clears_the_explicit_custom_state() {
        let mut app = default_app();
        app.custom_chip_explicit = true;
        let preset = builtin_preset("AVIF · Balanced");
        app.apply_preset(&preset).expect("applies");
        assert!(
            !app.custom_chip_explicit,
            "the chip ladder re-derives from the applied config (plan 15 F1)"
        );
    }

    #[test]
    fn applying_a_preset_resets_the_jxl_draft_and_tracks_the_ref() {
        let mut app = default_app();
        app.jxl_draft = JxlAdvancedDraft {
            new_id: "leftover".to_string(),
            new_value: 7,
        };
        let preset = builtin_preset("JXL · Visually lossless");
        app.apply_preset(&preset).expect("applies");
        assert_eq!(app.jxl_draft, JxlAdvancedDraft::default(), "draft state reset");
        let index = app
            .builtins
            .iter()
            .position(|builtin| builtin.title == "JXL · Visually lossless")
            .expect("builtin present");
        assert_eq!(app.active_preset, Some(PresetRef::Builtin(index)));
        assert_eq!(
            app.active_preset_label().as_deref(),
            Some("JXL · Visually lossless")
        );

        // drift → "·modified"
        if let EncoderConfig::Jxl(options) = &mut app.settings.encoder {
            options.effort = 2;
        }
        assert_eq!(
            app.active_preset_label().as_deref(),
            Some("JXL · Visually lossless ·modified")
        );
        // back to an exact match → clean label
        if let EncoderConfig::Jxl(options) = &mut app.settings.encoder {
            options.effort = 7;
        }
        assert_eq!(
            app.active_preset_label().as_deref(),
            Some("JXL · Visually lossless")
        );
    }

    #[test]
    fn newer_format_presets_are_rejected_by_the_apply_path() {
        let mut app = default_app();
        let mut preset = builtin_preset("WebP · Compact");
        preset.schema = crate::presets::PRESET_SCHEMA + 1;
        let encoder_before = app.settings.encoder.clone();
        assert!(app.apply_preset(&preset).is_err());
        assert_eq!(app.settings.encoder, encoder_before, "state untouched");
        assert!(app.active_preset.is_none());
    }

    #[test]
    fn preset_save_rename_edit_delete_round_trip_through_the_store() {
        let mut app = default_app();
        let dir = std::env::temp_dir().join(format!(
            "byteshaver-gui-app-preset-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        app.preset_store = Some(crate::presets::Store::new(dir.clone()));
        app.settings.policies.collision = CollisionChoice::OverwriteAlways;

        // save (scope toggle default per decision 1: policies differ → full)
        let draft = PresetSaveDraft::from_current(&app);
        assert!(draft.include_policies, "policies differ from the defaults");
        assert!(!draft.include_output_dir, "privacy opt-in starts off");
        let err = app
            .save_preset_from_current(&draft)
            .expect_err("an empty title is rejected");
        assert!(err.contains("title"));

        // actually save with a proper title
        app.active_preset = None;
        let draft = PresetSaveDraft {
            title: "My Webp".to_string(),
            description: "test".to_string(),
            ..PresetSaveDraft::from_current(&app)
        };
        app.save_preset_from_current(&draft).expect("saved");
        assert_eq!(app.user_presets.len(), 1);
        assert_eq!(app.user_presets[0].file_name, "my-webp.json");
        assert_eq!(app.active_preset, Some(PresetRef::User("My Webp".to_string())));

        // duplicate title rejected
        let err = app
            .save_preset_from_current(&PresetSaveDraft {
                title: "My Webp".to_string(),
                ..draft.clone()
            })
            .expect_err("duplicate title");
        assert!(err.contains("already exists"));

        // rename re-slugs
        app.preset_rename("My Webp", "Renamed Preset").expect("renamed");
        assert_eq!(app.user_presets[0].file_name, "renamed-preset.json");
        assert_eq!(
            app.active_preset,
            Some(PresetRef::User("Renamed Preset".to_string())),
            "the active ref follows the rename"
        );

        // description edit
        app.preset_edit_description("Renamed Preset", "new text")
            .expect("edited");
        assert_eq!(app.user_presets[0].preset.description, "new text");

        // duplicate from a builtin (the supported edit path for built-ins)
        let source = builtin_preset("AVIF · Compact");
        app.preset_duplicate(&source).expect("duplicated");
        assert_eq!(app.user_presets.len(), 2);
        let copy = app
            .user_presets
            .iter()
            .find(|stored| stored.preset.title == "AVIF · Compact (copy)")
            .expect("the copy exists (found by title, files load sorted)");
        assert!(!copy.preset.builtin);

        // delete clears a dangling active ref
        app.preset_delete("Renamed Preset").expect("deleted");
        assert_eq!(app.user_presets.len(), 1);
        assert!(app.active_preset.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preset_import_dedups_titles_and_reports_per_file_errors() {
        let mut app = default_app();
        let dir = std::env::temp_dir().join(format!(
            "byteshaver-gui-app-import-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        app.preset_store = Some(crate::presets::Store::new(dir.clone()));

        let mut preset = builtin_preset("WebP · Compact");
        preset.builtin = false;
        let json = serde_json::to_string_pretty(&preset).expect("serialize");
        let good = dir.join("one.json");
        std::fs::write(&good, &json).expect("seed import 1");
        let same_title = dir.join("two.json");
        std::fs::write(&same_title, json).expect("seed import 2 (same title)");
        let broken = dir.join("broken.json");
        std::fs::write(&broken, "{ not json").expect("seed junk");

        let message = app
            .import_presets(&[good, same_title, broken])
            .expect_err("the broken file surfaces as a summary error");
        assert!(
            message.starts_with("imported 2 preset(s)"),
            "the two good files still imported: {message}"
        );
        let titles: Vec<String> = app
            .user_presets
            .iter()
            .map(|stored| stored.preset.title.clone())
            .collect();
        assert!(titles.contains(&"WebP · Compact".to_string()));
        assert!(
            titles.contains(&"WebP · Compact (imported)".to_string()),
            "the colliding import keeps both (dedup on import)"
        );

        // clean import reports the count
        let good = dir.join("one.json");
        let result = app
            .import_presets(&[good.clone(), good])
            .expect("clean batch");
        assert_eq!(result, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preset_export_writes_sanitized_json() {
        let app = default_app();
        let mut preset = full_preset(false);
        if let Some(policies) = &mut preset.content.policies {
            policies.output_mode = OutputMode::Directory;
            policies.output_dir = "/local/secret".to_string();
        }
        let target = std::env::temp_dir().join(format!(
            "byteshaver-gui-app-export-{}.json",
            std::process::id()
        ));
        app.preset_export(&preset, target.clone()).expect("exported");
        let text = std::fs::read_to_string(&target).expect("read export");
        assert!(
            !text.contains("/local/secret"),
            "excluded dirs never reach the file"
        );
        let parsed: Preset = serde_json::from_str(&text).expect("re-importable");
        assert_eq!(parsed.title, preset.title);
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn preset_content_shapes_round_trip_through_serde() {
        for scope in [false, true] {
            let mut preset = full_preset(scope);
            preset.schema = crate::presets::PRESET_SCHEMA;
            preset.core_version = crate::app::CORE_VERSION.to_string();
            preset.created_unix = 1;
            preset.modified_unix = 2;
            let json = serde_json::to_string(&preset).expect("serialize");
            let parsed: Preset = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, preset);
        }
    }

    #[test]
    fn save_draft_defaults_follow_decision_1() {
        // default policies → format-only default
        let app = default_app();
        let draft = PresetSaveDraft::from_current(&app);
        assert!(!draft.include_policies);
        // drifted policies → full-preset default
        let mut app = default_app();
        app.settings.policies.reverse_processing_order = true;
        let draft = PresetSaveDraft::from_current(&app);
        assert!(draft.include_policies);
        assert!(!draft.include_output_dir);
    }

    #[test]
    fn preset_from_current_builds_both_scope_shapes() {
        let mut app = default_app();
        app.select_encoder("jxl");
        app.settings.policies.exif = ExifSettings {
            mode: ExifMode::Keep,
            ..ExifSettings::default()
        };
        app.settings.policies.output_mode = OutputMode::Directory;
        app.settings.policies.output_dir = "/x".to_string();
        let format_only = app.preset_from_current(&PresetSaveDraft {
            title: "Only format".to_string(),
            description: String::new(),
            include_policies: false,
            include_output_dir: false,
        });
        assert!(format_only.content.policies.is_none());
        let full = app.preset_from_current(&PresetSaveDraft {
            title: "Full".to_string(),
            description: String::new(),
            include_policies: true,
            include_output_dir: false,
        });
        assert_eq!(
            full.content
                .policies
                .as_ref()
                .expect("full scope")
                .output_dir,
            "",
            "no opt-in → the path is cleared at build time (mode stays)"
        );
        // and the state after an apply matches the preset (equality check;
        // the label only resolves for *stored* presets, so compare directly)
        app.apply_preset(&full).expect("applies");
        assert!(
            crate::presets::preset_matches(&full, &app.settings.encoder, &app.settings.policies),
            "the excluded output dir must not read as modified"
        );
    }

    #[test]
    fn a_preset_content_with_an_unknown_encoder_would_not_parse() {
        // forward-compat guarantee at the model level (plan 14 §2)
        let json = r#"{
            "schema": 1, "title": "X", "description": "", "created_unix": 0,
            "modified_unix": 0, "core_version": "9.9.9",
            "content": { "encoder": { "NotAnEncoder": {} }, "include_output_dir": false }
        }"#;
        let parsed: Result<Preset, _> = serde_json::from_str(json);
        assert!(parsed.is_err(), "unknown variants fail serde (unreadable list)");
        // while the empty-content guard still trips
        let json = r#"{
            "schema": 1, "title": "X", "description": "", "created_unix": 0,
            "modified_unix": 0, "core_version": "9.9.9"
        }"#;
        let parsed: Result<Preset, _> = serde_json::from_str(json);
        assert!(parsed.is_err(), "content is mandatory");
    }

    #[test]
    fn preset_content_carries_the_documented_fields() {
        // a structural smoke test of the plan-§1 shape
        let preset = full_preset(true);
        let PresetContent { encoder, policies, include_output_dir } = preset.content;
        assert!(matches!(encoder, EncoderConfig::Webp(_)));
        assert!(policies.is_some());
        assert!(include_output_dir);
    }
}
