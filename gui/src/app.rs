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

use crate::options;
use crate::queue::Queue;
use crate::reporter::ChannelReporter;
use crate::settings::Settings;

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
    /// Whether the report window is open.
    pub show_report: bool,
    /// Whether the about window is open.
    pub show_about: bool,
    /// Pre-flight/start errors (invalid EXIF selectors, encoder disabled).
    pub start_error: Option<String>,
    /// Whether a drag is currently over the window (drop-zone highlight).
    pub drag_hovered: bool,
    /// Draft state of the JXL advanced settings editor.
    pub jxl_draft: JxlAdvancedDraft,
    settings_dirty: bool,
}

impl App {
    /// Builds the app with the given (already loaded) settings.
    #[must_use]
    pub fn with_settings(settings: Settings) -> Self {
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
    /// the name is unknown.
    pub fn select_encoder(&mut self, name: &str) -> bool {
        let Some(config) = options::default_encoder_config(name) else {
            return false;
        };
        if self.settings.encoder != config {
            self.settings.encoder = config;
            self.mark_settings_dirty();
        }
        true
    }

    // ---- job spec construction -------------------------------------------

    /// Resolves the EXIF policy from the settings (may fail on invalid
    /// tag selectors; the message blocks starting a job).
    ///
    /// # Errors
    ///
    /// The CLI's selector parse error message.
    pub fn exif_policy(&self) -> Result<ExifPolicy, String> {
        self.settings.exif.to_policy()
    }

    /// Builds the shared [`ConversionConfig`] from the global policy
    /// mirrors (pure; unit-tested against `ConversionConfig::from_args`).
    ///
    /// # Errors
    ///
    /// The EXIF selector parse error.
    pub fn build_conversion_config(&self) -> Result<ConversionConfig, String> {
        let (overwrite_if_smaller, overwrite_existing) = self.settings.collision.flags();
        Ok(ConversionConfig {
            // input selection happens via the spec's InputSelection
            pattern: String::new(),
            // overridden by the spec's output directory
            output: String::new(),
            reverse_processing_order: self.settings.reverse_processing_order,
            overwrite_if_smaller,
            overwrite_existing,
            discard_if_larger_than_input: self.settings.discard_if_larger_than_input,
            discard_input_alpha_channel: self.settings.discard_input_alpha_channel,
            exif: self.exif_policy()?,
            heif_image_policy: self.settings.heif_image_policy,
            animated_input: self.settings.animated_input,
            max_animation_memory_mib: self.settings.max_animation_memory_mib,
        })
    }

    /// Builds the [`JobSpec`] of the current queue + settings (pure; the
    /// `Files` selection mirrors the CLI's `JobSpec::from_conversion`
    /// semantics, with the output directory overriding `common.output`).
    ///
    /// # Errors
    ///
    /// The EXIF selector parse error.
    pub fn build_job_spec(&self) -> Result<JobSpec, String> {
        let common = self.build_conversion_config()?;
        let mut encoder = self.settings.encoder.clone();
        inject_exif_policy(&mut encoder, &common.exif);
        Ok(JobSpec {
            inputs: InputSelection::Files(self.queue.selection()),
            output: self
                .settings
                .output_dir
                .as_deref()
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            encoder,
            common,
        })
    }

    /// Whether a job can start right now (queue non-empty, no job running,
    /// encoder compiled in, EXIF selectors valid).
    #[must_use]
    pub fn can_start(&self) -> bool {
        self.start_blocker().is_none()
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
        self.exif_policy().err()
    }

    // ---- job lifecycle ----------------------------------------------------

    /// Starts a conversion job over the whole queue.
    ///
    /// # Panics
    ///
    /// Panics if the internal event channel is closed unexpectedly (the
    /// receiver always lives on the same app instance).
    pub fn start_job(&mut self) {
        if self.running.is_some() {
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
        self.queue.begin_run();
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
                }
            }
            JobEvent::FileStarted { path, .. } => {
                self.queue.apply_file_started(&path);
            }
            JobEvent::FileFinished { path, outcome, .. } => {
                if let Some(job) = self.running.as_mut() {
                    job.finished_items += 1;
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

    /// Completes the running job: joins the worker (instant after
    /// `Finished`), collects the report and resets the queue run state.
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
        self.queue.finish_run();
        if let Some(message) = &report.error {
            self.start_error = Some(message.clone());
        }
        self.report = Some(report);
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

        // 3. panels
        crate::panels::show(self, ctx);

        // 4. keep repainting while a job runs
        if self.running.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // 5. remember the window size for the next launch
        let size = ctx.input(|input| input.viewport().inner_rect.map(|rect| rect.size()));
        if let Some([width, height]) = size.map(|size| [size.x, size.y])
            && self.settings.window_size != Some([width, height])
        {
            self.settings.window_size = Some([width, height]);
            self.settings_dirty = true;
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
    use crate::queue::ItemStatus;
    use crate::settings::{CollisionChoice, ExifMode, ExifSettings};
    use byteshaver::cli::CliArgs;
    use byteshaver::config::ConversionConfig;
    use byteshaver::pipeline::Outcome;
    use clap::Parser;

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
        app.settings.collision = CollisionChoice::OverwriteIfSmaller;
        app.settings.animated_input = byteshaver::config::AnimatedInputPolicy::Error;
        app.settings.discard_if_larger_than_input = true;
        app.settings.discard_input_alpha_channel = true;
        app.settings.max_animation_memory_mib = 512;
        app.settings.reverse_processing_order = true;
        app.settings.exif = ExifSettings {
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
        app.settings.output_dir = Some("/tmp/out".to_string());
        let spec = app.build_job_spec().expect("valid settings");
        assert_eq!(
            spec.inputs,
            InputSelection::Files(vec![PathBuf::from("/x/a.png")])
        );
        assert_eq!(spec.output, Some(PathBuf::from("/tmp/out")));
        // the common config stays CLI-default-shaped (pattern/output unused)
        assert_eq!(spec.common.pattern, "");
        assert_eq!(spec.common.output, "");

        // empty output string means "same as input" (CLI default)
        app.settings.output_dir = None;
        let spec = app.build_job_spec().expect("valid settings");
        assert_eq!(spec.output, None);
    }

    #[test]
    fn exif_policy_is_injected_into_oxipng_and_jxl_options() {
        let mut app = default_app();
        app.settings.exif = ExifSettings {
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
        app.settings.exif = ExifSettings {
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
            },
        });
        assert_eq!(app.queue.len(), 2, "discovered rows are appended");
        assert!(app.queue.items()[1].discovered);
        assert_eq!(app.queue.items()[1].status, ItemStatus::Encoded);
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
        app.settings.output_dir = Some(out_files.display().to_string());
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
}
