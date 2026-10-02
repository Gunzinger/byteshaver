//! Scene runner of the headless GUI capture pipeline (plan 17 §6, P2).
//!
//! Drives the real [`crate::app::App`] inside an `egui_kittest` harness
//! (wgpu on the software rasterizer — no display server, plan 17 D2)
//! through scripted [`Step`] sequences and renders PNG stills / frame
//! sequences for the README media pipeline.
//!
//! # Virtual time
//!
//! The harness pumps the app with fixed `step_dt` frames; the runner pins
//! egui's absolute clock by setting [`egui::RawInput::time`] before every
//! step, so animations (egui tweens, the progress shimmer, the plan-12
//! confetti) advance deterministically at the scene's frame rate and the
//! encoded video's duration is a scene constant — decoupled from the
//! real wall-clock time the encoder workers take (plan 17 §6.3).
//!
//! # Frame contract
//!
//! - `Pump`/`Stable`/`HoverAt` frames are the scripted video beats and
//!   are dumped to the frames dir when configured (`NNNN.png`, starting
//!   at `0000`).
//! - `PumpUntil` waits for a [`Predicate`] on app state (the encode
//!   workers run at wall-clock speed) and dumps **no** frames: the
//!   video's running segment length comes from the scripted `Pump` beats
//!   around it, not from the encoder's real duration. The (undumped)
//!   settle frames `Stable` runs through `run_ok` are invisible beats
//!   too.
//! - `Snapshot` renders a still and (when a snapshot dir is configured)
//!   writes it as `<dir>/<name>.png`; snapshots are also kept in memory
//!   for in-process tests.
//!
//! # Determinism
//!
//! Scenes run against a freshly constructed [`App`] built from
//! [`Settings::default()`] (never `load_or_default`) with the window size
//! injected; `crate::viewports` is switched into its in-canvas embedded
//! window mode (plan 17 §6.2) so popups render into the single harness
//! canvas only while open. The harness zeroes egui's `animation_time`
//! for snapshot stability; [`Step::Animate`] restores it for video
//! scenes.

use std::path::PathBuf;

use anyhow::{Context as _, bail, ensure};

use egui_kittest::Harness;

use crate::app::App;
use crate::settings::{OutputMode, Settings};

/// Upper bound for one [`Predicate`] wait and for the harness's settle
/// loop: 2 virtual minutes at 30 fps. A conversion that outlasts this is
/// an error, never a hang (plan 17 §6.3 LIMIT).
const PUMP_LIMIT: u64 = 60 * 120;

/// Default frame duration of a scene (30 fps, the plan 17 §5.2/§7 video
/// pipeline assumption).
const DEFAULT_DT_MS: u32 = 1000 / 30;

/// Animation time restored by [`Step::Animate`] / [`SceneRunner::
/// set_animate`] (the harness zeroes `animation_time` at construction for
/// snapshot determinism; egui's default is 1/12 s — a touch slower than
/// the app feels in production, but smooth and deterministic on video).
const ANIMATION_TIME: f32 = 0.15;

/// CLI flag of the per-encoder quality option in [`crate::options`]
/// (shared by webp/avif/jxl/webp-anim — the exact row
/// [`Step::SetQuality`] drives).
const QUALITY_CLI_FLAG: &str = "-q, --quality";

/// Output directory root the run scenes write their conversions into
/// (relative to the capture process cwd = the xtask stage dir, plan 17
/// §4: outputs stay inside the stage and never feed back into later
/// runs). Each run scene gets its own subdir (see [`run_output_dir`]) —
/// re-running scenes in one stage dir must not collide with previous
/// outputs, which would turn every row into "skipped (existing output
/// kept)" and kill the confetti/report story.
const RUN_OUTPUT_ROOT: &str = "out/capture-run";

/// The per-scene run output directory (see [`RUN_OUTPUT_ROOT`]).
fn run_output_dir(scene: &str) -> PathBuf {
    PathBuf::from(RUN_OUTPUT_ROOT).join(scene)
}

/// One scripted action of a capture scene (plan 17 §6.3). Steps drive
/// [`App`] through its public API only — no synthetic mouse clicks for
/// the core story, no UI-thread-unsafe shortcuts. The enum is
/// serde-derivable so data-driven scene files (plan 17 §6.3 `scenes.ron`)
/// can parse into it later; v1 ships a built-in Rust registry (see
/// [`scenes`]).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Step {
    /// Fresh first-run state. Each scene already runs against a freshly
    /// constructed [`App`] (see [`SceneRunner::new`]), so this is a no-op
    /// kept for symmetry with the plan's data-driven scene sketch (§6.3)
    /// and for future mid-scene resets.
    Reset,
    /// Enqueues one input path (file or directory). Errors when the path
    /// does not exist — scenes must never silently capture a broken
    /// queue. Paths are relative to the capture process cwd (the xtask
    /// stage dir).
    AddPath(PathBuf),
    /// Enqueues several input paths (same rules as [`Step::AddPath`]).
    AddPaths(Vec<PathBuf>),
    /// Selects the target encoder by capability name (webp, avif, …),
    /// resetting its options to the CLI defaults. Errors when the name is
    /// unknown.
    SelectEncoder(String),
    /// Sets the encoder's `--quality` option (clamped to its schema
    /// range). Errors when the selected encoder has no quality option.
    SetQuality(f32),
    /// `Some(dir)`: directory output mode with the given folder (created
    /// on demand by the conversion core); `None`: back to "same as
    /// input".
    SetOutputDir(Option<PathBuf>),
    /// Applies a built-in preset by title (plan 14 §4). Errors when no
    /// builtin carries the title.
    ApplyPreset(String),
    /// Starts a conversion over the whole queue. Errors with the footer's
    /// blocker message when the run cannot start (empty queue, encoder
    /// unavailable, output target unresolved, …) instead of capturing a
    /// stuck error line.
    Start,
    /// Requests cancelation of the running job (CLI Ctrl+C semantics).
    Cancel,
    /// Opens the visual difference inspector on queue row `index`
    /// (bounds-guarded). Errors when the row has no converted output yet
    /// — open the inspector after a finished run.
    OpenInspector(usize),
    /// Closes the inspector (drops its buffers and textures).
    CloseInspector,
    /// Opens the run-report popup.
    OpenReport,
    /// Closes the run-report popup.
    CloseReport,
    /// Opens the About popup.
    OpenAbout,
    /// Closes the About popup.
    CloseAbout,
    /// Opens the manage-presets popup.
    OpenPresetManager,
    /// Closes the manage-presets popup.
    ClosePresetManager,
    /// Moves the synthetic cursor to a logical position (kittest draws it
    /// into the render, e.g. hovering the Convert button for a beat).
    HoverAt {
        /// Logical x position (points).
        x: f32,
        /// Logical y position (points).
        y: f32,
    },
    /// Advances the scene by `frames` frames at the configured frame
    /// duration (a scripted video beat).
    Pump {
        /// Number of frames to advance.
        frames: u32,
    },
    /// Advances until `predicate` holds on the app state (bounded by
    /// [`PUMP_LIMIT`]). No frames are dumped during the wait — see the
    /// module docs' frame contract.
    PumpUntil(Predicate),
    /// Advances `frames` frames and then lets the harness settle (drain
    /// animations/repaints) while the app is idle. When a job is still
    /// running the settle is skipped: the app repaints continuously then,
    /// and `run_ok` would only spin against its step budget.
    Stable {
        /// Number of frames to advance before settling.
        frames: u32,
    },
    /// Renders the current frame as a still named `name` (written to the
    /// configured snapshot dir and kept in memory).
    Snapshot {
        /// Still name; the file becomes `<snapshot-dir>/<name>.png`.
        name: String,
    },
    /// Enables/disables egui animations (the harness zeroes
    /// `animation_time` at construction; video scenes restore it so
    /// tweens/confetti actually animate).
    Animate(bool),
    /// Changes the scene's frame duration from here on (also the fps the
    /// dumped frame sequence encodes at — reported by `bh-gui-capture`).
    FrameRate {
        /// Frame duration in milliseconds.
        dt_ms: u32,
    },
    /// Ends the scene early; remaining steps (if any) are skipped.
    Quit,
}

/// Wait condition of [`Step::PumpUntil`] (pure predicates on [`App`],
/// unit-tested against fake states).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Predicate {
    /// A run has started and is still active.
    JobActive,
    /// The run finished: the worker joined and the report was collected
    /// (the `finish_job` path; `App::start_job` cleared the previous
    /// report, so this is the real completion signal).
    JobDone,
    /// The thumbnail/EXIF worker drained every request (plan 17 §6.3
    /// quiescence). Only meaningful while no job runs — the worker pauses
    /// during conversions.
    ThumbsSettled,
    /// The metric worker drained every request (measurements and
    /// inspector decode pairs alike).
    MetricsSettled,
}

impl Predicate {
    /// Evaluates the predicate on the app state (pure; called once per
    /// pumped frame by [`Step::PumpUntil`]).
    #[must_use]
    pub fn holds(self, app: &App) -> bool {
        match self {
            Predicate::JobActive => app.running.is_some(),
            Predicate::JobDone => app.running.is_none() && app.report.is_some(),
            Predicate::ThumbsSettled => app.thumbs.pending_count() == 0,
            Predicate::MetricsSettled => app.metrics.pending_count() == 0,
        }
    }
}

/// A named scene: id (CLI/xtask handle), description and the scripted
/// steps (plan 17 §6.4 storyboard).
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// Stable scene id (`gui-empty`, `gui-tour`, …).
    pub id: &'static str,
    /// One-line description (`--list` output).
    pub description: &'static str,
    /// Whether the scene is a video scene (frame sequence for the animated
    /// WebP encode). The xtask media pipeline passes `--frames-dir` only
    /// for video scenes; still scenes dump nothing, so the post stage never
    /// mistakes their settle frames for video beats.
    pub video: bool,
    /// The scripted steps.
    pub steps: Vec<Step>,
}

/// Options for constructing a [`SceneRunner`].
#[derive(Clone, Debug)]
pub struct SceneOptions {
    /// Logical window size in points (the harness's screen rect).
    pub window_size: [f32; 2],
    /// Device pixel ratio: masters are `window_size * pixels_per_point`
    /// px (2.0 = HiDPI masters for the 2x→1x lanczos downscale, plan 17
    /// §7).
    pub pixels_per_point: f32,
    /// When set, [`Step::Snapshot`] stills are written here as
    /// `<dir>/<name>.png`.
    pub snapshot_dir: Option<PathBuf>,
    /// When set, every scripted frame is dumped here as
    /// `<dir>/<scene-id>/NNNN.png` (the runner appends the scene id, so
    /// scenes never collide inside one invocation; see the module docs'
    /// frame contract).
    pub frames_dir: Option<PathBuf>,
}

impl Default for SceneOptions {
    fn default() -> Self {
        SceneOptions {
            window_size: [1100.0, 720.0],
            pixels_per_point: 2.0,
            snapshot_dir: None,
            frames_dir: None,
        }
    }
}

/// Owns the harness and the virtual clock; executes [`Step`] sequences
/// against the app. One runner = one scene = one fresh first-run [`App`]
/// (plan 17 §4: settings are never loaded from disk, so the developer's
/// config cannot leak into captures).
pub struct SceneRunner {
    harness: Harness<'static, App>,
    /// Virtual egui clock (seconds); pinned via `RawInput::time` per step.
    clock: f64,
    /// Current frame duration in seconds (mutable via [`Step::FrameRate`]).
    dt: f32,
    /// Current frame rate metadata for the frame-sequence consumer.
    fps: u32,
    /// Scene id (frames subdir naming + CLI summary).
    scene_id: String,
    /// Directory the scripted frames are dumped into (`None` = no dump).
    frames_dir: Option<PathBuf>,
    /// Frames dumped so far (`NNNN.png` counter).
    frame_count: u64,
    /// Stills rendered so far, in snapshot order.
    snapshots: Vec<(String, image::RgbaImage)>,
    snapshot_dir: Option<PathBuf>,
    /// Set by [`Step::Quit`].
    finished: bool,
    /// Animation flag mirror of [`SceneRunner::animate`].
    animate: bool,
}

impl SceneRunner {
    /// Builds the harness around a fresh first-run [`App`] and switches
    /// the app's popup host into its capture (in-canvas embedded window)
    /// mode before the first frame renders (plan 17 §6.2).
    pub fn new(scene_id: &str, options: &SceneOptions) -> Self {
        let [width, height] = options.window_size;
        let settings = Settings {
            window_size: Some([width, height]),
            ..Settings::default()
        };
        let harness: Harness<'static, App> = Harness::builder()
            .with_size(egui::vec2(width, height))
            .with_pixels_per_point(options.pixels_per_point)
            .with_step_dt(DEFAULT_DT_MS as f32 / 1000.0)
            .with_max_steps(PUMP_LIMIT)
            .build_eframe(|cc| {
                crate::install_symbol_fonts(&cc.egui_ctx);
                let mut app = App::with_settings(settings);
                app.capture_embedded_windows = true;
                app
            });
        // continue the virtual clock where the harness's construction
        // frames left off (time never jumps backwards within a scene)
        let clock = harness.ctx.input(|input| input.time);
        // the frames subdir is created eagerly so a dump failure surfaces
        // before the scene runs, not halfway through a video
        let frames_dir = options.frames_dir.as_ref().map(|dir| {
            let dir = dir.join(scene_id);
            std::fs::create_dir_all(&dir)
                .unwrap_or_else(|err| panic!("cannot create the frames dir {}: {err}", dir.display()));
            dir
        });
        SceneRunner {
            harness,
            clock,
            dt: DEFAULT_DT_MS as f32 / 1000.0,
            fps: 30,
            scene_id: scene_id.to_string(),
            frames_dir,
            frame_count: 0,
            snapshots: Vec::new(),
            snapshot_dir: options.snapshot_dir.clone(),
            finished: false,
            animate: false,
        }
    }

    /// Whether egui animations are currently enabled (off by default —
    /// the harness zeroes `animation_time` for deterministic stills).
    /// Mirrors the runner's own flag (the style value is written through
    /// [`Context::all_styles_mut`][egui::Context::all_styles_mut], which
    /// has no read accessor).
    #[must_use]
    pub fn animate(&self) -> bool {
        self.animate
    }

    /// Enables/disables egui animations (restores/zeroes
    /// `animation_time`; see [`Step::Animate`]).
    pub fn set_animate(&mut self, animate: bool) {
        let time = if animate { ANIMATION_TIME } else { 0.0 };
        self.harness.ctx.all_styles_mut(|style| style.animation_time = time);
        self.animate = animate;
    }

    /// The scene id (frames subdir naming).
    #[must_use]
    pub fn scene_id(&self) -> &str {
        &self.scene_id
    }

    /// The app state (read-only inspection for tests).
    #[must_use]
    pub fn state(&self) -> &App {
        self.harness.state()
    }

    /// The app state (mutable; lets tests pre-seed state before running).
    #[must_use]
    pub fn state_mut(&mut self) -> &mut App {
        self.harness.state_mut()
    }

    /// The stills rendered so far (in snapshot order).
    #[must_use]
    pub fn snapshots(&self) -> &[(String, image::RgbaImage)] {
        &self.snapshots
    }

    /// Frames dumped so far.
    #[must_use]
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// The fps the dumped frame sequence encodes at (reported by the CLI
    /// for the post-processing step).
    #[must_use]
    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// Renders the current frame (the root canvas) as PNG-ready RGBA.
    ///
    /// # Errors
    ///
    /// When the software rasterizer fails (wgpu device errors).
    pub fn render(&mut self) -> anyhow::Result<image::RgbaImage> {
        self.harness.render().map_err(anyhow::Error::msg)
    }

    /// Number of currently visible egui area layers (windows, tooltips,
    /// menus) in the harness context. The capture adapter's artifact
    /// assertion is built on this: the pre-adapter behavior hosted every
    /// popup through `show_viewport_immediate` regardless of open state,
    /// which the single-canvas harness turns into one stacked empty
    /// `egui::Window` shell per popup — visible areas that must not
    /// exist (plan 17 §6.2). The adapted in-canvas popups leave zero
    /// extra areas while closed.
    #[must_use]
    pub fn visible_area_count(&self) -> usize {
        self.harness
            .ctx
            .memory(|mem| mem.areas().visible_layer_ids().len())
    }

    /// Runs a full step list until it ends or a [`Step::Quit`] fires.
    ///
    /// # Errors
    ///
    /// The first failing step (see [`Step`] variants) — scenes fail
    /// loudly, never silently capture a wrong state (plan 17 R10).
    pub fn run(&mut self, steps: &[Step]) -> anyhow::Result<()> {
        for step in steps {
            if self.finished {
                break;
            }
            self.run_step(step)?;
        }
        Ok(())
    }

    /// Executes one step.
    ///
    /// # Errors
    ///
    /// See the [`Step`] variant docs; predicate waits are bounded by
    /// [`PUMP_LIMIT`].
    pub fn run_step(&mut self, step: &Step) -> anyhow::Result<()> {
        match step {
            Step::Reset => {} // fresh harness per scene (module docs)
            Step::AddPath(path) => {
                add_paths_checked(self.harness.state_mut(), std::iter::once(path))?;
            }
            Step::AddPaths(paths) => add_paths_checked(self.harness.state_mut(), paths.iter())?,
            Step::SelectEncoder(name) => {
                let app = self.harness.state_mut();
                ensure!(
                    app.select_encoder(name),
                    "SelectEncoder: unknown encoder {name:?} (capability names: see the About panel)"
                );
            }
            Step::SetQuality(quality) => set_quality(self.harness.state_mut(), *quality)?,
            Step::SetOutputDir(dir) => {
                let app = self.harness.state_mut();
                match dir {
                    Some(dir) => {
                        app.settings.policies.output_mode = OutputMode::Directory;
                        app.settings.policies.output_dir = dir.display().to_string();
                    }
                    None => {
                        app.settings.policies.output_mode = OutputMode::SameAsInput;
                    }
                }
                app.mark_settings_dirty();
            }
            Step::ApplyPreset(title) => {
                let app = self.harness.state_mut();
                let preset = app
                    .builtins
                    .iter()
                    .find(|preset| preset.title == title.as_str())
                    .cloned()
                    .with_context(|| format!("ApplyPreset: no builtin preset named {title:?}"))?;
                app.apply_preset(&preset)
                    .map_err(anyhow::Error::msg)
                    .with_context(|| format!("ApplyPreset: applying {title:?} failed"))?;
            }
            Step::Start => {
                let app = self.harness.state_mut();
                if let Some(blocker) = app.start_blocker() {
                    bail!("Start blocked: {blocker}");
                }
                app.start_job();
            }
            Step::Cancel => self.harness.state_mut().cancel_job(),
            Step::OpenInspector(index) => {
                let app = self.harness.state_mut();
                let item = app.queue.items().get(*index).with_context(|| {
                    format!(
                        "OpenInspector: row {index} is out of bounds (queue has {} rows)",
                        app.queue.len()
                    )
                })?;
                let input = item.path.clone();
                let output = item.output_path.clone().with_context(|| {
                    format!(
                        "OpenInspector: row {index} ({}) has no converted output yet — \
                         open the inspector after a finished run",
                        input.display()
                    )
                })?;
                app.open_inspector(&input, &output);
            }
            Step::CloseInspector => self.harness.state_mut().inspector = None,
            Step::OpenReport => self.harness.state_mut().show_report = true,
            Step::CloseReport => self.harness.state_mut().show_report = false,
            Step::OpenAbout => self.harness.state_mut().show_about = true,
            Step::CloseAbout => self.harness.state_mut().show_about = false,
            Step::OpenPresetManager => self.harness.state_mut().show_preset_manager = true,
            Step::ClosePresetManager => self.harness.state_mut().show_preset_manager = false,
            Step::HoverAt { x, y } => {
                self.harness.hover_at(egui::pos2(*x, *y));
                // one frame so the queued pointer event lands before any
                // subsequent snapshot (the synthetic cursor appears then)
                self.pump_frame(true)?;
            }
            Step::Pump { frames } => {
                for _ in 0..*frames {
                    self.pump_frame(true)?;
                }
            }
            Step::PumpUntil(predicate) => {
                let mut steps = 0u64;
                loop {
                    // no frame dump during the wait (module docs' frame
                    // contract: the video's beat length is scripted, not
                    // the encoder's real duration)
                    self.pump_frame(false)?;
                    steps += 1;
                    if predicate.holds(self.harness.state()) {
                        break;
                    }
                    ensure!(
                        steps <= PUMP_LIMIT,
                        "PumpUntil({predicate:?}) exceeded {PUMP_LIMIT} frames — \
                         the waited-for state never materialized"
                    );
                }
            }
            Step::Stable { frames } => {
                for _ in 0..*frames {
                    self.pump_frame(true)?;
                }
                // settle animations/repaints while idle (settle frames are
                // not dumped); during a run the app repaints continuously
                // and `run_ok` would only spin against the step budget
                if self.harness.state().running.is_none() {
                    self.harness.run_ok();
                }
            }
            Step::Snapshot { name } => {
                let image = self.harness.render().map_err(anyhow::Error::msg)?;
                if let Some(dir) = &self.snapshot_dir {
                    let path = dir.join(format!("{name}.png"));
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).with_context(|| {
                            format!("creating the stills dir {}", parent.display())
                        })?;
                    }
                    image
                        .save(&path)
                        .with_context(|| format!("writing the still {}", path.display()))?;
                }
                self.snapshots.push((name.clone(), image));
            }
            Step::Animate(animate) => self.set_animate(*animate),
            Step::FrameRate { dt_ms } => {
                ensure!(*dt_ms > 0, "FrameRate: dt_ms must be positive");
                self.dt = *dt_ms as f32 / 1000.0;
                self.fps = (1000.0 / *dt_ms as f32).round().max(1.0) as u32;
            }
            Step::Quit => self.finished = true,
        }
        Ok(())
    }

    /// One pumped frame: advance the virtual clock, run the app frame and
    /// — when `dump` is set and a frames dir is configured — render and
    /// write the frame as `NNNN.png`.
    fn pump_frame(&mut self, dump: bool) -> anyhow::Result<()> {
        self.clock += f64::from(self.dt);
        self.harness.input_mut().time = Some(self.clock);
        self.harness.step();
        if dump
            && let Some(dir) = &self.frames_dir
        {
            let path = dir.join(format!("{:04}.png", self.frame_count));
            let image = self.harness.render().map_err(anyhow::Error::msg)?;
            image
                .save(&path)
                .with_context(|| format!("writing the frame {}", path.display()))?;
            self.frame_count += 1;
        }
        Ok(())
    }
}

/// Enqueues paths after checking they exist ([`Step::AddPaths`] contract:
/// a missing fixture is a scene error, never a silently broken queue).
fn add_paths_checked<'a>(
    app: &mut App,
    paths: impl IntoIterator<Item = &'a PathBuf>,
) -> anyhow::Result<()> {
    let paths: Vec<&PathBuf> = paths.into_iter().collect();
    for path in &paths {
        ensure!(
            path.exists(),
            "AddPaths: input path {} does not exist (paths are relative to the capture \
             process cwd, the xtask stage dir)",
            path.display()
        );
    }
    app.queue.add_paths(paths.into_iter().cloned());
    Ok(())
}

/// Sets the selected encoder's `--quality` option through the pure
/// [`crate::options`] schema (the same setter the options editor grid
/// uses — no per-encoder special casing here).
fn set_quality(app: &mut App, quality: f32) -> anyhow::Result<()> {
    let encoder_name = crate::options::encoder_kind_name(&app.settings.encoder);
    let row = crate::options::encoder_rows(&app.settings.encoder)
        .into_iter()
        .find(|row| row.cli_flag == QUALITY_CLI_FLAG)
        .with_context(|| format!("SetQuality: encoder {encoder_name:?} has no --quality option"))?;
    let (set, range) = match row.control {
        crate::options::ControlSpec::Slider { set, range, .. }
        | crate::options::ControlSpec::Drag { set, range, .. } => (set, range),
        other => bail!(
            "SetQuality: the --quality row of {encoder_name:?} is not numeric ({other:?})"
        ),
    };
    let clamped = f64::from(quality).clamp(range.0, range.1);
    set(
        &mut app.settings.encoder,
        crate::options::OptionValue::Number(clamped),
    );
    app.mark_settings_dirty();
    Ok(())
}

// ---- built-in scene registry (plan 17 §6.4) -------------------------------

/// Fixture paths the scenes reference, relative to the capture process
/// cwd (the xtask stage dir, plan 17 §4 — the `demo/` fixture tree is
/// staged there; these exact names are the P1/P2 contract).
const DEMO_PHOTOS: &str = "demo/photos";
const DEMO_ANIM: &str = "demo/anim";
const DEMO_LOGO: &str = "demo/screens/logo.png";
const DEMO_DASHBOARD: &str = "demo/screens/dashboard.png";

/// The built-in v1 scene registry (plan 17 §6.4 storyboard). Encoder
/// note: the run/inspector scenes select **webp** — this build cannot
/// decode AVIF input back (dec-heif is off, `src/input/mod.rs` routes
/// AVIF to libheif), so an AVIF run would leave the visual difference
/// inspector stuck on its decode-failure caption. `gui-options` still
/// selects AVIF (a pure settings state, no decode involved).
pub fn scenes() -> &'static [Scene] {
    use std::sync::LazyLock;
    static SCENES: LazyLock<Vec<Scene>> = LazyLock::new(build_scenes);
    &SCENES
}

/// Looks a scene up by id.
#[must_use]
pub fn scene(id: &str) -> Option<&'static Scene> {
    scenes().iter().find(|scene| scene.id == id)
}

/// Builds the storyboard of plan 17 §6.4 (unit-checked for unique ids and
/// unique snapshot names below).
fn build_scenes() -> Vec<Scene> {
    let paths = |names: &[&str]| names.iter().map(PathBuf::from).collect::<Vec<PathBuf>>();
    vec![
        // first-run drop zone (still `gui-empty`)
        Scene {
            id: "gui-empty",
            description: "first-run drop zone",

            video: false,
            steps: vec![
                Step::Stable { frames: 20 },
                Step::Snapshot {
                    name: "gui-empty".to_string(),
                },
            ],
        },
        // mixed-format queue with thumbnails (still `gui-queue`)
        Scene {
            id: "gui-queue",
            description: "mixed-format queue, thumbnails settled",

            video: false,
            steps: vec![
                Step::AddPaths(paths(&[DEMO_PHOTOS, DEMO_ANIM, DEMO_LOGO, DEMO_DASHBOARD])),
                Step::PumpUntil(Predicate::ThumbsSettled),
                Step::Stable { frames: 15 },
                Step::Snapshot {
                    name: "gui-queue".to_string(),
                },
            ],
        },
        // encoder chips + preset dropdown + EXIF policy with AVIF at 85
        // selected (still `gui-options`; the options panel is always
        // visible at the bottom of the main window)
        Scene {
            id: "gui-options",
            description: "options panel with AVIF · quality 85 selected",

            video: false,
            steps: vec![
                Step::AddPaths(paths(&[DEMO_LOGO, DEMO_DASHBOARD])),
                Step::SelectEncoder("avif".to_string()),
                Step::SetQuality(85.0),
                Step::Stable { frames: 15 },
                Step::Snapshot {
                    name: "gui-options".to_string(),
                },
            ],
        },
        // mid-conversion: segmented bar, per-row glyphs (still
        // `gui-running`). The fixture files are tiny, so the run may
        // already be finished when the snapshot lands (the pumps run in
        // microseconds while the encode workers take milliseconds) — in
        // that case the still shows the fresh report instead, which is
        // the documented acceptable fallback for this scene.
        Scene {
            id: "gui-running",
            description: "mid-conversion run state",

            video: false,
            steps: vec![
                Step::AddPaths(paths(&[DEMO_DASHBOARD, DEMO_LOGO, DEMO_PHOTOS])),
                Step::PumpUntil(Predicate::ThumbsSettled),
                Step::SelectEncoder("webp".to_string()),
                Step::SetOutputDir(Some(run_output_dir("gui-running"))),
                Step::Start,
                Step::Pump { frames: 2 },
                Step::Snapshot {
                    name: "gui-running".to_string(),
                },
                Step::PumpUntil(Predicate::JobDone),
                Step::Stable { frames: 5 },
            ],
        },
        // finished run: ratios, totals, confetti + report (still
        // `gui-report`)
        Scene {
            id: "gui-report",
            description: "finished run with confetti and report",

            video: false,
            steps: vec![
                Step::AddPaths(paths(&[DEMO_PHOTOS, DEMO_LOGO, DEMO_DASHBOARD])),
                Step::SelectEncoder("webp".to_string()),
                Step::SetOutputDir(Some(run_output_dir("gui-report"))),
                Step::Start,
                Step::PumpUntil(Predicate::JobDone),
                // confetti is armed by finish_job: snapshot it mid-flight
                Step::Pump { frames: 6 },
                Step::OpenReport,
                Step::Stable { frames: 10 },
                Step::Snapshot {
                    name: "gui-report".to_string(),
                },
            ],
        },
        // visual difference inspector on one output (still
        // `gui-inspector`)
        Scene {
            id: "gui-inspector",
            description: "visual difference inspector on a converted pair",

            video: false,
            steps: vec![
                Step::AddPaths(paths(&[DEMO_DASHBOARD, DEMO_LOGO])),
                Step::SelectEncoder("webp".to_string()),
                Step::SetOutputDir(Some(run_output_dir("gui-inspector"))),
                Step::Start,
                Step::PumpUntil(Predicate::JobDone),
                Step::OpenInspector(0),
                // the inspector's bounded decode pair runs on the metric
                // worker; settled = buffers received, textures uploaded
                Step::PumpUntil(Predicate::MetricsSettled),
                Step::Stable { frames: 10 },
                Step::Snapshot {
                    name: "gui-inspector".to_string(),
                },
            ],
        },
        // the tour video (video scene): empty → queue → options → run →
        // report (confetti) → inspector. The frame beats are the scripted
        // Pump/Stable steps; the PumpUntil waits dump nothing (module
        // docs), so the video's length is this step list, not the encoder's
        // real duration. Beats are trimmed deliberately (~115 frames):
        // README assets are size-budgeted (plan 17 §7), and the confetti
        // particles are the worst case for inter-frame compression.
        Scene {
            id: "gui-tour",
            description: "tour video: empty, queue, options, run, report, inspector",

            video: true,
            steps: vec![
                Step::Animate(true),
                Step::Stable { frames: 12 },
                Step::Snapshot {
                    name: "gui-tour-00-empty".to_string(),
                },
                Step::AddPaths(paths(&[DEMO_PHOTOS, DEMO_LOGO, DEMO_ANIM])),
                Step::PumpUntil(Predicate::ThumbsSettled),
                Step::Stable { frames: 10 },
                Step::Snapshot {
                    name: "gui-tour-01-queue".to_string(),
                },
                Step::SelectEncoder("avif".to_string()),
                Step::SetQuality(85.0),
                Step::Stable { frames: 10 },
                Step::Snapshot {
                    name: "gui-tour-02-options".to_string(),
                },
                // the run itself uses webp (see the registry docs: the
                // inspector beat needs a decodable output)
                Step::SetOutputDir(Some(run_output_dir("gui-tour"))),
                Step::SelectEncoder("webp".to_string()),
                Step::Start,
                Step::PumpUntil(Predicate::JobActive),
                Step::Pump { frames: 8 },
                Step::Snapshot {
                    name: "gui-tour-03-running".to_string(),
                },
                Step::PumpUntil(Predicate::JobDone),
                Step::Pump { frames: 8 },
                Step::OpenReport,
                Step::Stable { frames: 10 },
                Step::Snapshot {
                    name: "gui-tour-04-report".to_string(),
                },
                // row 1 is `demo/screens/logo.png` (the two directory rows
                // sit at 0 and 2 and never carry a converted output)
                Step::OpenInspector(1),
                Step::PumpUntil(Predicate::MetricsSettled),
                Step::Stable { frames: 10 },
                Step::Snapshot {
                    name: "gui-tour-05-inspector".to_string(),
                },
                // tail: confetti burst and fade-out
                Step::Pump { frames: 45 },
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteshaver::config::EncoderConfig;
    use byteshaver::job::RunReport;

    /// A real tiny PNG on disk (queue rows stat/sniff at enqueue; the
    /// run scenes need a decodable file).
    fn tiny_png(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "byteshaver-capture-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{name}.png"));
        image::RgbaImage::from_pixel(24, 16, image::Rgba([40, 120, 200, 255]))
            .save(&path)
            .expect("write fixture png");
        path
    }

    // ---- predicates (plan 17 §15: quiescence predicates, fake states) ------

    #[test]
    fn job_predicates_track_the_run_lifecycle() {
        let mut app = App::with_settings(Settings::default());
        assert!(!Predicate::JobActive.holds(&app));
        assert!(!Predicate::JobDone.holds(&app), "no report yet");
        // running state (as after start_job)
        app.start_job(); // blocked (empty queue) — simulate instead:
        assert!(app.running.is_none());
        app.queue.add_paths(vec![PathBuf::from("/x/a.png")]);
        app.start_job();
        assert!(Predicate::JobActive.holds(&app), "run started");
        assert!(!Predicate::JobDone.holds(&app), "still running");
        // the finished state (as left by finish_job): worker joined,
        // report collected
        app.running = None;
        app.report = Some(RunReport::default());
        assert!(Predicate::JobDone.holds(&app), "run finished");
        assert!(!Predicate::JobActive.holds(&app));
    }

    #[test]
    fn worker_predicates_track_pending_requests() {
        let fixture = tiny_png("pred-worker");
        let mut app = App::with_settings(Settings::default());
        assert!(Predicate::ThumbsSettled.holds(&app));
        assert!(Predicate::MetricsSettled.holds(&app), "nothing requested");
        // a Manual-mode measure request parks a pending metric job
        let output = fixture.with_extension("webp");
        std::fs::write(&output, b"not a real webp").expect("stat-able output");
        app.measure_quality(&fixture, &output);
        assert!(!Predicate::MetricsSettled.holds(&app), "one in flight");
        assert!(Predicate::ThumbsSettled.holds(&app), "unrelated worker");
        let _ = std::fs::remove_dir_all(fixture.parent().unwrap());
    }

    // ---- step helpers -------------------------------------------------------

    #[test]
    fn add_paths_rejects_missing_fixtures() {
        let mut app = App::with_settings(Settings::default());
        let err = add_paths_checked(&mut app, std::iter::once(&PathBuf::from("demo/missing.png")))
            .expect_err("missing path");
        assert!(err.to_string().contains("does not exist"), "{err}");
        assert!(app.queue.is_empty(), "nothing enqueued on failure");
    }

    #[test]
    fn add_paths_enqueues_existing_fixtures() {
        let fixture = tiny_png("add");
        let mut app = App::with_settings(Settings::default());
        add_paths_checked(&mut app, std::iter::once(&fixture)).expect("existing path");
        assert_eq!(app.queue.len(), 1);
        let _ = std::fs::remove_dir_all(fixture.parent().unwrap());
    }

    #[test]
    fn set_quality_drives_the_options_schema_and_clamps() {
        let mut app = App::with_settings(Settings::default());
        // webp default: quality 80
        set_quality(&mut app, 85.0).expect("webp has --quality");
        let EncoderConfig::Webp(options) = &app.settings.encoder else {
            panic!("expected webp config");
        };
        assert_eq!(options.quality, 85.0);
        // avif carries the same flag
        assert!(app.select_encoder("avif"));
        set_quality(&mut app, 10.0).expect("avif has --quality");
        // clamping: webp quality range is 0..=100
        app.select_encoder("webp");
        set_quality(&mut app, 500.0).expect("clamped, not rejected");
        let EncoderConfig::Webp(options) = &app.settings.encoder else {
            panic!("expected webp config");
        };
        assert_eq!(options.quality, 100.0);
        // an encoder without a quality row errors clearly
        app.select_encoder("oxipng");
        let err = set_quality(&mut app, 85.0).expect_err("oxipng has no --quality");
        assert!(err.to_string().contains("no --quality"), "{err}");
    }

    #[test]
    fn unknown_encoder_and_preset_are_clear_errors() {
        let mut runner = SceneRunner::new("test", &SceneOptions::default());
        let err = runner
            .run_step(&Step::SelectEncoder("nope".to_string()))
            .expect_err("unknown encoder");
        assert!(err.to_string().contains("unknown encoder"), "{err}");
        let err = runner
            .run_step(&Step::ApplyPreset("Nope".to_string()))
            .expect_err("unknown preset");
        assert!(err.to_string().contains("Nope"), "{err}");
    }

    #[test]
    fn start_reports_its_blocker_instead_of_running() {
        let mut runner = SceneRunner::new("test", &SceneOptions::default());
        let err = runner.run_step(&Step::Start).expect_err("empty queue");
        assert!(err.to_string().contains("queue is empty"), "{err}");
        assert!(runner.state().running.is_none());
        assert!(runner.state().start_error.is_none(), "no UI-side error");
    }

    #[test]
    fn open_inspector_bounds_and_output_guards() {
        let mut runner = SceneRunner::new("test", &SceneOptions::default());
        let err = runner
            .run_step(&Step::OpenInspector(3))
            .expect_err("out of bounds");
        assert!(err.to_string().contains("out of bounds"), "{err}");
        let fixture = tiny_png("inspect");
        runner.state_mut().queue.add_paths(vec![fixture.clone()]);
        let err = runner
            .run_step(&Step::OpenInspector(0))
            .expect_err("no output yet");
        assert!(err.to_string().contains("no converted output"), "{err}");
        let _ = std::fs::remove_dir_all(fixture.parent().unwrap());
    }

    #[test]
    fn popup_flags_and_quit_work() {
        let mut runner = SceneRunner::new("test", &SceneOptions::default());
        runner
            .run(&[
                Step::OpenReport,
                Step::OpenAbout,
                Step::OpenPresetManager,
                Step::Quit,
                Step::OpenInspector(99), // skipped by Quit
            ])
            .expect("steps run");
        let app = runner.state();
        assert!(app.show_report && app.show_about && app.show_preset_manager);
    }

    #[test]
    fn animate_toggles_the_style_time() {
        let mut runner = SceneRunner::new("test", &SceneOptions::default());
        assert!(!runner.animate(), "harness zeroes animations");
        runner.set_animate(true);
        assert!(runner.animate());
        runner.set_animate(false);
        assert!(!runner.animate());
    }

    // ---- registry ------------------------------------------------------------

    #[test]
    fn registry_ids_are_unique_and_known() {
        let ids: Vec<&str> = scenes().iter().map(|scene| scene.id).collect();
        let expected = [
            "gui-empty",
            "gui-queue",
            "gui-options",
            "gui-running",
            "gui-report",
            "gui-inspector",
            "gui-tour",
        ];
        for id in expected {
            assert!(ids.contains(&id), "missing scene {id}");
            assert!(scene(id).is_some(), "lookup by id {id}");
        }
        for (index, id) in ids.iter().enumerate() {
            assert!(!ids[index + 1..].contains(id), "duplicate id {id}");
        }
        assert!(scene("no-such-scene").is_none());
    }

    #[test]
    fn registry_steps_are_non_empty_and_snapshot_names_unique_per_scene() {
        for scene in scenes() {
            assert!(!scene.steps.is_empty(), "{} has no steps", scene.id);
            assert!(
                !scene.description.is_empty(),
                "{} has no description",
                scene.id
            );
            let mut names: Vec<&str> = Vec::new();
            for step in &scene.steps {
                if let Step::Snapshot { name } = step {
                    assert!(
                        !names.contains(&name.as_str()),
                        "{} snapshots {name:?} twice",
                        scene.id
                    );
                    names.push(name);
                }
            }
        }
    }

    #[test]
    fn tour_contains_the_storyboard_beats_and_fixture_names() {
        let tour = scene("gui-tour").expect("tour registered");
        let has = |needle: &Step| tour.steps.iter().any(|step| step == needle);
        assert!(has(&Step::Animate(true)));
        assert!(has(&Step::PumpUntil(Predicate::JobActive)));
        assert!(has(&Step::PumpUntil(Predicate::JobDone)));
        assert!(has(&Step::OpenInspector(1)));
        let added = tour
            .steps
            .iter()
            .find_map(|step| match step {
                Step::AddPaths(paths) => Some(paths.clone()),
                _ => None,
            })
            .expect("tour adds paths");
        for name in ["demo/photos", "demo/screens/logo.png", "demo/anim"] {
            assert!(
                added.iter().any(|path| path == std::path::Path::new(name)),
                "tour must add {name}"
            );
        }
    }
}
