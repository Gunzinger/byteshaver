//! Headless job API (plan WS7): drives the conversion core from any
//! front-end (CLI, GUI, tests) without stdout, indicatif or signal coupling.
//!
//! A [`JobSpec`] describes one batch; [`JobHandle::start`] runs it on a
//! worker thread, streaming [`JobEvent`][reporter::JobEvent]s to a
//! [`Reporter`], and [`JobHandle::join`] collects the final
//! [`RunReport`]. Cancelation works by raising a [`StopFlag`] — the CLI
//! wires it to Ctrl+C, a GUI to its cancel button.

pub mod capabilities;
pub mod reporter;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::config::{ConversionConfig, EncoderConfig};
use crate::converter::ThreadBudget;
use crate::format::ImageFormat;
use crate::pipeline::{self, FileResult, RunStats};

pub use capabilities::{Capabilities, EncoderInfo, capabilities};
// `JsonlReporter` only exists with the `logs` feature
#[cfg(feature = "logs")]
pub use reporter::JsonlReporter;
pub use reporter::{JobEvent, NullReporter, Reporter, StdoutReporter, TeeReporter};

/// Selection of the inputs of a job.
///
/// `Pattern` is the classic CLI glob (discovery identical to former
/// pipeline behavior); `Files` is the drag-and-drop/file-picker variant
/// where directories are expanded recursively by the core using the same
/// supported-format filter as glob results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputSelection {
    /// Glob pattern to match images to convert (e.g. `images/**/*.png`).
    Pattern(String),
    /// Explicit files and/or directories; directories are walked
    /// recursively and filtered by supported input format. Nonexistent
    /// paths surface as per-file errors instead of aborting the batch.
    Files(Vec<PathBuf>),
}

/// Specification of one conversion batch.
#[derive(Clone, Debug)]
pub struct JobSpec {
    /// Inputs to convert.
    pub inputs: InputSelection,
    /// Output directory overriding `common.output` when set; `None` keeps
    /// the CLI behavior (output directory from `common.output`, empty
    /// meaning "next to the input").
    pub output: Option<PathBuf>,
    /// Target encoder with its options.
    pub encoder: EncoderConfig,
    /// Shared conversion configuration (EXIF policy, collision flags,
    /// animation guards, ...). Its `pattern` field is ignored — input
    /// selection happens via `inputs`.
    pub common: ConversionConfig,
}

impl JobSpec {
    /// Builds a spec from the CLI-style pair of configurations, selecting
    /// inputs via the pattern of `common` (the behavior of
    /// [`run`][crate::run]).
    #[must_use]
    pub fn from_conversion(common: ConversionConfig, encoder: EncoderConfig) -> Self {
        JobSpec {
            inputs: InputSelection::Pattern(common.pattern.clone()),
            output: None,
            encoder,
            common,
        }
    }
}

/// Cooperative cancelation flag (CLI: raised by the Ctrl+C handler; GUI:
/// raised by the cancel button).
///
/// Cheap to clone (shared `Arc<AtomicBool>`); the pipeline checks it once
/// per queued work item — already-running encodes always finish.
#[derive(Clone, Debug, Default)]
pub struct StopFlag(Arc<AtomicBool>);

impl StopFlag {
    /// Creates a lowered (running) flag.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether cancelation was requested.
    #[must_use]
    pub fn raised(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Requests cancelation of the running job.
    pub fn raise(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Per-process (or per-GUI-window) holder of the global threading budget.
///
/// Today this simply exposes [`ThreadBudget::global()`]; owning it here
/// keeps the door open for explicit budgets per session.
#[derive(Clone, Copy, Debug)]
pub struct Session {
    /// Thread budget applied to every job of this session.
    pub thread_budget: ThreadBudget,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// Detects a session from the machine's available parallelism.
    #[must_use]
    pub fn new() -> Self {
        Session {
            thread_budget: ThreadBudget::global(),
        }
    }
}

/// Refines the session budget for one job: with more than one work item
/// each in-flight encoder is capped to its share of the machine
/// ([`ThreadBudget::per_files`]), so concurrent encoders cannot
/// oversubscribe the cores; single-item jobs keep the session budget so
/// one big encode can use every core. Budgets with an explicitly
/// configured per-encoder cap are left untouched.
///
/// The count mirrors the pipeline's discovery (glob matches and recursive
/// directory expansion, supported formats only; HEIF sub-image expansion
/// is not mirrored — the budget only needs an approximate item count).
fn refine_budget(spec: &JobSpec, base: ThreadBudget) -> ThreadBudget {
    if base.threads_per_encoder.is_some() {
        return base;
    }
    let count = match &spec.inputs {
        InputSelection::Pattern(pattern) => glob::glob(pattern)
            .map(|paths| {
                paths
                    .filter_map(|entry| entry.ok())
                    .filter(|path| ImageFormat::from(path.as_path()) != ImageFormat::Unknown)
                    .count()
            })
            .unwrap_or(0),
        InputSelection::Files(files) => files.iter().map(|entry| count_input_entry(entry)).sum(),
    };
    if count > 1 {
        ThreadBudget::per_files(count)
    } else {
        base
    }
}

/// Work-item estimate of one [`InputSelection::Files`] entry: directories
/// are walked recursively (supported formats only), everything else
/// counts as one item.
fn count_input_entry(entry: &Path) -> usize {
    if entry.is_dir() {
        count_directory(entry)
    } else {
        1
    }
}

/// Number of supported image files below `dir`, recursively (mirrors the
/// pipeline's discovery filter; unreadable directories count as empty).
fn count_directory(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                count_directory(&path)
            } else if ImageFormat::from(path.as_path()) != ImageFormat::Unknown {
                1
            } else {
                0
            }
        })
        .sum()
}

/// Aggregated counters of a finished run (same math as the printed
/// "Encode statistics" block).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Totals {
    /// Number of work items discovered from the inputs.
    pub input_files: u64,
    /// Number of files successfully encoded and written.
    pub successful: u64,
    /// Number of files skipped because an output already existed.
    pub skipped: u64,
    /// Number of encodings discarded (larger than the input file).
    pub discarded: u64,
    /// Number of inputs skipped due to output name collisions.
    pub collisions: u64,
    /// Number of files that failed to convert.
    pub errors: u64,
    /// Number of items not processed due to a raised stop flag.
    pub aborted: u64,
    /// Total size of all counted input files in bytes.
    pub input_size: u64,
    /// Total size of all counted outputs in bytes.
    pub output_size: u64,
}

/// Result of a finished job.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    /// Wall time of the whole run.
    pub elapsed: Duration,
    /// Per-item results in queue order (the deterministic input order, not
    /// completion order).
    pub files: Vec<FileResult>,
    /// Aggregated counters of the run.
    pub totals: Totals,
    /// Number of outputs that could not carry EXIF metadata although the
    /// policy wanted it embedded.
    pub metadata_dropped_count: u64,
    /// Pre-flight failure message (invalid glob pattern, output directory
    /// error); `None` on successful runs.
    pub error: Option<String>,
}

impl From<RunStats> for RunReport {
    fn from(stats: RunStats) -> Self {
        RunReport {
            elapsed: stats.elapsed,
            files: stats.results,
            totals: Totals {
                input_files: stats.input_files,
                successful: stats.successful,
                skipped: stats.skipped,
                discarded: stats.discarded,
                collisions: stats.collisions,
                errors: stats.errors,
                aborted: stats.aborted,
                input_size: stats.input_size,
                output_size: stats.output_size,
            },
            metadata_dropped_count: stats.metadata_dropped,
            error: None,
        }
    }
}

impl From<Result<RunStats, crate::Error>> for RunReport {
    fn from(result: Result<RunStats, crate::Error>) -> Self {
        match result {
            Ok(stats) => RunReport::from(stats),
            Err(err) => RunReport {
                error: Some(err.to_string()),
                ..RunReport::default()
            },
        }
    }
}

impl RunReport {
    /// Rebuilds the legacy [`RunStats`] (the return type of
    /// [`run`][crate::run]); pre-flight failures map onto the error.
    ///
    /// # Errors
    ///
    /// Returns the pre-flight error (invalid pattern, output directory)
    /// when the run failed before processing started.
    pub fn into_stats(self) -> Result<RunStats, crate::Error> {
        if let Some(message) = self.error {
            return Err(crate::Error::from_string(message));
        }
        Ok(RunStats {
            input_files: self.totals.input_files,
            successful: self.totals.successful,
            skipped: self.totals.skipped,
            discarded: self.totals.discarded,
            collisions: self.totals.collisions,
            errors: self.totals.errors,
            aborted: self.totals.aborted,
            metadata_dropped: self.metadata_dropped_count,
            input_size: self.totals.input_size,
            output_size: self.totals.output_size,
            results: self.files,
            elapsed: self.elapsed,
        })
    }
}

/// Handle of a job started via [`JobHandle::start`]; join it to await the
/// result.
pub struct JobHandle {
    /// Worker thread executing the pipeline.
    thread: Option<std::thread::JoinHandle<Result<RunStats, crate::Error>>>,
}

impl JobHandle {
    /// Starts `spec` on a dedicated worker thread.
    ///
    /// Every [`JobEvent`] is dispatched to `reporter`
    /// until [`JobEvent::Finished`];
    /// `stop` is polled once per queued work item. The session provides
    /// the threading budget.
    pub fn start(
        spec: JobSpec,
        reporter: Box<dyn Reporter>,
        stop: StopFlag,
        session: &Session,
    ) -> JobHandle {
        let reporter: Arc<dyn Reporter> = Arc::from(reporter);
        let budget = session.thread_budget;
        let thread = std::thread::spawn(move || {
            // the budget is refined from the job's work-item count before
            // the pipeline builds its encoder (counting runs here, off
            // the caller's thread)
            let budget = refine_budget(&spec, budget);
            pipeline::execute(&spec, reporter, &stop, budget)
        });
        JobHandle {
            thread: Some(thread),
        }
    }

    /// Awaits the job and collects its report.
    ///
    /// A panicked worker yields an empty default report (with the panic
    /// surfaced as `error`), so a GUI never loses its process.
    #[must_use]
    pub fn join(mut self) -> RunReport {
        let result = match self.thread.take() {
            Some(thread) => match thread.join() {
                Ok(result) => result,
                Err(_panic) => Err(crate::Error::from_string(
                    "job worker thread panicked".to_string(),
                )),
            },
            None => Ok(RunStats::default()),
        };
        RunReport::from(result)
    }
}
