//! Background metric worker (plan 10 §phase 2): one dedicated thread fed
//! through a `std::sync::mpsc` channel, **one job at a time, paused while
//! a conversion job runs** — the same policy as the thumbnail worker
//! (decode would contend with rayon for no user-visible benefit).
//!
//! Results are drained each frame from the UI thread
//! ([`MetricState::poll`]): measurement scores land in a per-input-path
//! map (invalidated by the output's mtime), decode/compare failures land
//! in a parallel error map so rows render an explicit failure (plan 15
//! F19), inspector buffers are handed to the callback for texture upload.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, SystemTime};

use image::RgbaImage;

use super::decode;
use super::{
    MetricEngineChoice, MetricResult, QualityMetric, DISPLAY_MAX_EDGE, clamp_metric_max_edge,
};

/// Worker poll interval while paused / idle.
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// One queued measurement entry (per input path).
#[derive(Clone, Debug, PartialEq)]
pub struct MetricEntry {
    /// Output mtime at measurement time — a re-run writes a newer file,
    /// and the stale entry is dropped from dedup (re-measurable).
    pub output_mtime: Option<SystemTime>,
    /// The comparison result.
    pub result: MetricResult,
}

/// A failed measurement (plan 15 F19): decode/compare errors are stored
/// per input path so rows render an explicit failure instead of silently
/// staying empty. Invalidated by the output's mtime like results.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricError {
    /// Output mtime at failure time (cache invalidation).
    pub output_mtime: Option<SystemTime>,
    /// Human-readable failure reason (unsupported format, decode caps, …).
    pub message: String,
}

/// Work item handed to the metric thread.
pub enum MetricJob {
    /// Compare a converted pair with the selected engine.
    Measure {
        /// Input path (bounded-decoded).
        input: PathBuf,
        /// Converted output path (bounded-decoded, geometry-matched).
        output: PathBuf,
        /// Engine selected at request time.
        engine: MetricEngineChoice,
        /// Longest edge to decode at (settings' clamped value).
        max_edge: u32,
    },
    /// Decode a pair for the visual difference inspector (display bound).
    Inspect {
        /// Input path.
        input: PathBuf,
        /// Converted output path.
        output: PathBuf,
    },
}

/// Result produced by the metric thread.
pub enum MetricOutcome {
    /// `Err(reason)` = unmeasurable — the reason is stored per row and
    /// rendered (plan 15 F19), never silently dropped.
    Measured {
        /// Input path (map key).
        input: PathBuf,
        /// Output mtime at measurement time (cache invalidation).
        output_mtime: Option<SystemTime>,
        /// The comparison result, or the failure reason.
        result: Result<MetricResult, String>,
    },
    /// Decoded inspector pair (`None`s = decode failed — the inspector
    /// shows an explanatory message, never an error path).
    Inspected {
        /// Input path (matches the open inspector).
        input: PathBuf,
        /// Output path.
        output: PathBuf,
        /// Bounded RGBA of the input.
        a: Option<RgbaImage>,
        /// Bounded RGBA of the output.
        b: Option<RgbaImage>,
    },
}

/// UI-side handle of the metric worker: request dedup, mtime-aware
/// result cache and the pause flag. Dropping it shuts the worker down.
pub struct MetricState {
    jobs: Sender<MetricJob>,
    results: Receiver<MetricOutcome>,
    paused: Arc<AtomicBool>,
    /// Input paths with an in-flight request (dedup).
    pending: HashSet<PathBuf>,
    /// Measured results, keyed by input path.
    measured: HashMap<PathBuf, MetricEntry>,
    /// Stored failures, keyed by input path (plan 15 F19: errors are
    /// surfaced, not dropped; mtime-invalidated like results).
    errors: HashMap<PathBuf, MetricError>,
    /// Engine the cached results were measured with (app-mirrored from
    /// the settings; a change invalidates the cache).
    pub engine: MetricEngineChoice,
}

impl Default for MetricState {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricState {
    /// Spawns the worker thread (once per app).
    #[must_use]
    pub fn new() -> Self {
        let (jobs, job_rx) = mpsc::channel::<MetricJob>();
        let (result_tx, results) = mpsc::channel::<MetricOutcome>();
        let paused = Arc::new(AtomicBool::new(false));
        std::thread::Builder::new()
            .name("byteshaver-metrics".to_string())
            .spawn({
                let paused = Arc::clone(&paused);
                move || worker_loop(job_rx, result_tx, paused)
            })
            .expect("spawn metric worker");
        MetricState {
            jobs,
            results,
            paused,
            pending: HashSet::new(),
            measured: HashMap::new(),
            errors: HashMap::new(),
            engine: MetricEngineChoice::default(),
        }
    }

    /// Pause flag toggled by the app each frame (see module docs).
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// Cached entry for an input path.
    #[must_use]
    pub fn cached(&self, input: &Path) -> Option<&MetricEntry> {
        self.measured.get(input)
    }

    /// Stored failure for an input path (plan 15 F19: a decode/compare
    /// error is an explicit per-row state, not a silent nothing).
    // rendered by the file table's metric text (plan 15 WP B); unused
    // until that lands
    #[allow(dead_code)]
    #[must_use]
    pub fn error(&self, input: &Path) -> Option<&MetricError> {
        self.errors.get(input)
    }

    /// Whether `input` has a request in flight.
    #[must_use]
    pub fn is_pending(&self, input: &Path) -> bool {
        self.pending.contains(input)
    }

    /// Requests a measurement unless one is in flight or a fresh result
    /// for the current output mtime is already cached. `max_edge` is the
    /// settings' clamped [`clamp_metric_max_edge`] value.
    pub fn request_measure(
        &mut self,
        input: PathBuf,
        output: PathBuf,
        engine: MetricEngineChoice,
        max_edge: u32,
    ) {
        if self.pending.contains(&input) {
            return;
        }
        let mtime = file_mtime(&output);
        if let Some(entry) = self.measured.get(&input) {
            if entry.output_mtime == mtime {
                return; // fresh cache hit, nothing changed on disk
            }
            self.measured.remove(&input); // stale output → re-measure
        }
        if let Some(error) = self.errors.get(&input) {
            if error.output_mtime == mtime {
                return; // already failed for this exact output
            }
            self.errors.remove(&input); // stale failure → re-measure
        }
        if self
            .jobs
            .send(MetricJob::Measure {
                input: input.clone(),
                output,
                engine,
                max_edge: clamp_metric_max_edge(max_edge),
            })
            .is_err()
        {
            return; // worker gone (app shutting down)
        }
        self.pending.insert(input);
    }

    /// Requests the bounded decode pair of the open inspector (deduped;
    /// [`super::DISPLAY_MAX_EDGE`] display bound, separate from the
    /// metric's settings bound).
    pub fn request_inspect(&mut self, input: PathBuf, output: PathBuf) {
        if self.pending.contains(&input) {
            return;
        }
        if self
            .jobs
            .send(MetricJob::Inspect {
                input: input.clone(),
                output,
            })
            .is_err()
        {
            return;
        }
        self.pending.insert(input);
    }

    /// Drops every cached result (e.g. when the engine choice changed so
    /// the map cannot mix engines for the aggregate display).
    pub fn clear_results(&mut self) {
        self.measured.clear();
        self.errors.clear();
    }

    /// The aggregate line for the report totals (see
    /// [`super::aggregate_line`]).
    #[must_use]
    pub fn aggregate(&self) -> Option<String> {
        super::aggregate_line(&self.measured)
    }

    /// Drains the worker results: measurements land in the result map (or
    /// the error map, plan 15 F19), inspector buffers go to
    /// `apply_inspection` (the app uploads the textures).
    pub fn poll(
        &mut self,
        mut apply_inspection: impl FnMut(PathBuf, PathBuf, Option<RgbaImage>, Option<RgbaImage>),
    ) {
        loop {
            match self.results.try_recv() {
                Ok(MetricOutcome::Measured {
                    input,
                    output_mtime,
                    result,
                }) => {
                    self.pending.remove(&input);
                    match result {
                        Ok(result) => {
                            self.errors.remove(&input);
                            self.measured
                                .insert(input, MetricEntry { output_mtime, result });
                        }
                        Err(message) => {
                            self.measured.remove(&input);
                            self.errors.insert(
                                input,
                                MetricError {
                                    output_mtime,
                                    message,
                                },
                            );
                        }
                    }
                }
                Ok(MetricOutcome::Inspected { input, output, a, b }) => {
                    self.pending.remove(&input);
                    apply_inspection(input, output, a, b);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }
}

/// Output file mtime (`None` when unstatable).
fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

/// Metric-thread loop: sleeps while paused, otherwise takes one job at a
/// time with a timeout; exits when all senders are gone (app dropped).
fn worker_loop(
    jobs: Receiver<MetricJob>,
    results: Sender<MetricOutcome>,
    paused: Arc<AtomicBool>,
) {
    loop {
        if paused.load(Ordering::Relaxed) {
            std::thread::sleep(POLL_INTERVAL);
            continue;
        }
        match jobs.recv_timeout(POLL_INTERVAL) {
            Ok(job) => {
                let outcome = match job {
                    MetricJob::Measure {
                        input,
                        output,
                        engine,
                        max_edge,
                    } => {
                        let result = match decode::load_pair(&input, &output, max_edge) {
                            Ok((a, b)) => {
                                let metric: Box<dyn QualityMetric> = engine.engine();
                                Ok(metric.compare(&a, &b))
                            }
                            Err(reason) => Err(reason),
                        };
                        MetricOutcome::Measured {
                            output_mtime: file_mtime(&output),
                            input,
                            result,
                        }
                    }
                    MetricJob::Inspect { input, output } => {
                        let (a, b) = match decode::load_pair(&input, &output, DISPLAY_MAX_EDGE) {
                            Ok((a, b)) => (Some(a), Some(b)),
                            Err(_) => (None, None),
                        };
                        MetricOutcome::Inspected { input, output, a, b }
                    }
                };
                if results.send(outcome).is_err() {
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_edge_is_below_the_metric_default() {
        assert!(DISPLAY_MAX_EDGE <= clamp_metric_max_edge(4096));
    }

    #[test]
    fn measure_request_dedups_and_mtime_invalidates() {
        let mut state = MetricState::new();
        let input = PathBuf::from("/x/a.png");
        let output = PathBuf::from("/x/a.webp");
        // missing files: the request is accepted (the worker reports
        // unmeasurable later); a second request is deduped while pending
        state.request_measure(input.clone(), output.clone(), MetricEngineChoice::Psnr, 4096);
        assert!(state.is_pending(&input));
        state.request_measure(input.clone(), output.clone(), MetricEngineChoice::Psnr, 4096);
        assert!(state.is_pending(&input), "in-flight requests are deduped");
        // fresh entry → no re-request (after the pending entry resolved)
        state.measured.insert(
            input.clone(),
            MetricEntry {
                output_mtime: file_mtime(&output),
                result: MetricResult {
                    engine: "psnr",
                    score: 40.0,
                    pretty: "40.0dB".to_string(),
                },
            },
        );
        state.pending.remove(&input);
        state.request_measure(input.clone(), output.clone(), MetricEngineChoice::Psnr, 4096);
        assert!(!state.is_pending(&input), "fresh cache hit skips the request");
        // stale mtime → re-request (the recorded mtime differs from the
        // file's current one)
        state.measured.get_mut(&input).expect("entry").output_mtime = Some(SystemTime::UNIX_EPOCH);
        state.request_measure(input.clone(), output.clone(), MetricEngineChoice::Psnr, 4096);
        assert!(state.is_pending(&input), "stale output triggers re-measure");
        assert!(!state.measured.contains_key(&input), "stale entry dropped");
        // engine mirror is plain data (app syncs it from the settings)
        state.engine = MetricEngineChoice::Psnr;
        assert_eq!(state.engine, MetricEngineChoice::Psnr);
        state.clear_results();
        assert!(state.cached(&input).is_none());
    }

    #[test]
    fn poll_routes_measured_and_inspected_results() {
        // drain a real worker: feed it a measure job for a missing file
        // (unmeasurable → stored error, plan 15 F19) and verify pending clears
        let mut state = MetricState::new();
        let input = PathBuf::from("/definitely/missing/input.png");
        let output = PathBuf::from("/definitely/missing/output.webp");
        state.request_measure(input.clone(), output.clone(), MetricEngineChoice::Psnr, 4096);
        state.set_paused(false);
        // poll may need a beat for the worker; bounded wait loop
        for _ in 0..100 {
            state.poll(|_, _, _, _| panic!("no inspection was requested"));
            if !state.is_pending(&input) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!state.is_pending(&input));
        assert!(state.cached(&input).is_none(), "failure → no result entry");
        let error = state.error(&input).expect("failure → stored error entry");
        assert!(error.message.contains("input.png"), "{}", error.message);
    }

    /// Real files: a junk output measures as an explicit error; a re-request
    /// for the same output is deduped (cached failure); rewriting the output
    /// (newer mtime) makes it re-measurable.
    #[test]
    fn decode_errors_surface_and_remeasure_on_a_newer_output() {
        let dir = std::env::temp_dir().join(format!("byteshaver-metric-error-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let input = dir.join("in.png");
        let output = dir.join("out.png");
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([1, 2, 3, 255]),
        ))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode fixture");
        std::fs::write(&input, &png).expect("write input");
        std::fs::write(&output, b"not an image at all").expect("write junk output");

        let mut state = MetricState::new();
        state.set_paused(false);
        state.request_measure(input.clone(), output.clone(), MetricEngineChoice::Psnr, 4096);
        for _ in 0..100 {
            state.poll(|_, _, _, _| panic!("no inspection was requested"));
            if !state.is_pending(&input) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let error = state.error(&input).expect("junk output → stored error");
        assert!(error.message.to_lowercase().contains("format"), "{}", error.message);
        // a second request for the same output is a cached-failure hit
        state.request_measure(input.clone(), output.clone(), MetricEngineChoice::Psnr, 4096);
        assert!(!state.is_pending(&input), "failure deduped like a result");

        // rewriting the output (newer mtime) drops the stale failure and
        // the re-request is accepted
        std::thread::sleep(Duration::from_millis(5));
        std::fs::write(&output, &png).expect("rewrite output as a real png");
        state.request_measure(input.clone(), output.clone(), MetricEngineChoice::Psnr, 4096);
        assert!(state.is_pending(&input), "stale error → re-measure");
        for _ in 0..100 {
            state.poll(|_, _, _, _| panic!("no inspection was requested"));
            if !state.is_pending(&input) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(state.error(&input).is_none(), "failure replaced by a result");
        assert!(state.cached(&input).is_some(), "now it measured");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
