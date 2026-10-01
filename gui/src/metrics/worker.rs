//! Background metric worker (plan 10 §phase 2): one dedicated thread fed
//! through a `std::sync::mpsc` channel, **one job at a time, paused while
//! a conversion job runs** — the same policy as the thumbnail worker
//! (decode would contend with rayon for no user-visible benefit).
//!
//! Results are drained each frame from the UI thread
//! ([`MetricState::poll`]): measurement scores land in a per-input-path
//! map (invalidated by the output's mtime), inspector buffers are handed
//! to the callback for texture upload.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, SystemTime};

use super::decode;
use super::{MetricEngineChoice, MetricResult, QualityMetric, clamp_metric_max_edge};

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
}

/// Result produced by the metric thread.
pub enum MetricOutcome {
    /// `None` = unmeasurable (decode failure etc.) — surfaced as a row
    /// tooltip, never an error path.
    Measured {
        /// Input path (map key).
        input: PathBuf,
        /// Output mtime at measurement time (cache invalidation).
        output_mtime: Option<SystemTime>,
        /// The comparison result.
        result: Option<MetricResult>,
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

    /// Drops every cached result (e.g. when the engine choice changed so
    /// the map cannot mix engines for the aggregate display).
    pub fn clear_results(&mut self) {
        self.measured.clear();
    }

    /// The aggregate line for the report totals (see
    /// [`super::aggregate_line`]).
    #[must_use]
    pub fn aggregate(&self) -> Option<String> {
        super::aggregate_line(&self.measured)
    }

    /// Drains the worker results: measurements land in the map.
    pub fn poll(&mut self) {
        loop {
            match self.results.try_recv() {
                Ok(MetricOutcome::Measured {
                    input,
                    output_mtime,
                    result,
                }) => {
                    self.pending.remove(&input);
                    if let Some(result) = result {
                        self.measured
                            .insert(input, MetricEntry { output_mtime, result });
                    }
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
                        let pair = decode::load_pair(&input, &output, max_edge);
                        let result = pair.map(|(a, b)| {
                            let metric: Box<dyn QualityMetric> = engine.engine();
                            metric.compare(&a, &b)
                        });
                        MetricOutcome::Measured {
                            output_mtime: file_mtime(&output),
                            input,
                            result,
                        }
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
        // (unmeasurable → no entry) and verify pending clears
        let mut state = MetricState::new();
        let input = PathBuf::from("/definitely/missing/input.png");
        let output = PathBuf::from("/definitely/missing/output.webp");
        state.request_measure(input.clone(), output.clone(), MetricEngineChoice::Psnr, 4096);
        state.set_paused(false);
        // poll may need a beat for the worker; bounded wait loop
        for _ in 0..100 {
            state.poll();
            if !state.is_pending(&input) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!state.is_pending(&input));
        assert!(state.cached(&input).is_none(), "failure → no entry");
    }
}
