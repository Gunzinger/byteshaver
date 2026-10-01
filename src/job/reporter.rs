//! Event sinks for the headless job API: the [`Reporter`] trait plus the
//! built-in [`StdoutReporter`] (the classic CLI output), [`JsonlReporter`]
//! (one JSON object per event, `logs` feature) and combinators
//! ([`TeeReporter`], [`NullReporter`]).
//!
//! Reporters are shared between the pipeline's rayon workers, hence the
//! `Send + Sync` supertraits.

use std::path::PathBuf;
use std::sync::Mutex;
// file/JSON machinery only exists with the `logs` feature
#[cfg(feature = "logs")]
use std::fs;
#[cfg(feature = "logs")]
use std::io::{BufWriter, Write};
#[cfg(feature = "logs")]
use std::path::Path;

use indicatif::{HumanDuration, ProgressBar, ProgressStyle};

use crate::pipeline::Outcome;

/// One progress event emitted by the conversion core.
///
/// The pipeline emits, in order: zero or more [`JobEvent::Notice`] lines
/// (pre-flight information), [`JobEvent::Started`], then per work item a
/// [`JobEvent::FileStarted`]/[`JobEvent::FileFinished`] pair plus a
/// [`JobEvent::ProgressStats`] update, and finally [`JobEvent::Finished`].
/// Ad-hoc warnings (per-file errors, collisions, huge images, dropped
/// metadata) surface as [`JobEvent::Notice`].
#[derive(Clone, Debug, serde::Serialize)]
pub enum JobEvent {
    /// A batch of `total_files` work items is about to be processed.
    Started {
        /// Total number of work items (files, or HEIF sub-images).
        total_files: u64,
    },
    /// The work item at `index` started processing.
    FileStarted {
        /// Zero-based position in the (deterministic) work-item queue.
        index: u64,
        /// Input file path.
        path: PathBuf,
    },
    /// The work item at `index` finished with the given outcome.
    FileFinished {
        /// Zero-based position in the (deterministic) work-item queue.
        index: u64,
        /// Input file path.
        path: PathBuf,
        /// Conversion result of the item.
        outcome: Outcome,
    },
    /// A human-readable message that is not tied to one finished file
    /// (pre-flight lines, warnings). Replaces the pipeline's former direct
    /// `println!` calls.
    Notice {
        /// Related file, when the message concerns a single input.
        path: Option<PathBuf>,
        /// Message text (one line, without trailing newline).
        message: String,
    },
    /// Running totals after a finished work item; replaces the progress
    /// bar's message updates. Sizes count encoded + skipped files only
    /// (discarded and errored files do not contribute), matching the
    /// final statistics.
    ProgressStats {
        /// Total counted input bytes so far.
        input_bytes: u64,
        /// Total counted output bytes so far.
        output_bytes: u64,
        /// Files encoded successfully so far.
        ok: u64,
        /// Files skipped (existing output or kept preexisting) so far.
        skipped: u64,
        /// Files that failed so far.
        errors: u64,
    },
    /// The batch is complete; all [`JobEvent::FileFinished`] events have
    /// been delivered. Stdout- or file-writing reporters flush their final
    /// rendering (progress bar teardown, statistics block) here.
    Finished,
}

/// Consumer of [`JobEvent`]s (GUI, CLI, tests, log files).
///
/// Shared between the pipeline thread and its rayon workers, so
/// implementations must be `Send + Sync` (use interior mutability).
pub trait Reporter: Send + Sync {
    /// Handles a single event.
    fn on_event(&self, ev: JobEvent);
}

/// Reporter that discards every event (headless runs, tests).
///
/// Keeps a capture buffer (`Mutex<Vec<u8>>`) that stays empty by contract;
/// tests assert it is untouched to prove the core never prints on its own.
#[derive(Debug, Default)]
pub struct NullReporter {
    /// Always-empty capture buffer.
    captured: Mutex<Vec<u8>>,
}

impl NullReporter {
    /// Creates a null reporter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The (always empty) capture buffer of this reporter.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn captured(&self) -> std::sync::MutexGuard<'_, Vec<u8>> {
        self.captured.lock().expect("null reporter mutex poisoned")
    }
}

impl Reporter for NullReporter {
    fn on_event(&self, _ev: JobEvent) {
        // intentional: nothing is ever written
    }
}

/// Reporter fan-out: dispatches every event to a list of reporters
/// (e.g. the CLI's stdout bar plus a `--json-log` file).
#[derive(Default)]
pub struct TeeReporter {
    reporters: Vec<Box<dyn Reporter>>,
}

impl TeeReporter {
    /// Creates an empty tee; append receivers with [`TeeReporter::push`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a receiver.
    pub fn push(&mut self, reporter: Box<dyn Reporter>) {
        self.reporters.push(reporter);
    }

    /// Builds a tee from an iterator of receivers.
    pub fn with_reporters(reporters: impl IntoIterator<Item = Box<dyn Reporter>>) -> Self {
        Self {
            reporters: reporters.into_iter().collect(),
        }
    }
}

impl Reporter for TeeReporter {
    fn on_event(&self, ev: JobEvent) {
        for reporter in &self.reporters {
            reporter.on_event(ev.clone());
        }
    }
}

/// The classic CLI rendering: indicatif progress bar, inline notices and the
/// final "Encode statistics" block, byte-compatible with the pre-job-API
/// pipeline output (only the clear-line handling of warning lines is now
/// uniform).
pub struct StdoutReporter {
    /// Progress bar; created on [`JobEvent::Started`], torn down on
    /// [`JobEvent::Finished`].
    state: Mutex<StdoutState>,
}

/// Mutable rendering state of [`StdoutReporter`].
struct StdoutState {
    /// Active progress bar (`None` before `Started` / after `Finished`).
    bar: Option<ProgressBar>,
    /// Total input bytes counted so far (encoded + skipped items).
    input_bytes: u64,
    /// Total output bytes counted so far.
    output_bytes: u64,
    /// Preexisting bytes of the input side (skipped items).
    preexisting_input: u64,
    /// Preexisting bytes of the output side (skipped items).
    preexisting_output: u64,
    /// Encoded file count.
    ok: u64,
    /// Skipped file count (existing or kept outputs).
    skipped: u64,
    /// Error count.
    errors: u64,
    /// Collision count.
    collisions: u64,
    /// Discarded (larger-than-input) encode count.
    discarded: u64,
    /// Discarded input bytes.
    discarded_input: u64,
    /// Discarded output bytes.
    discarded_output: u64,
    /// Aborted item count.
    aborted: u64,
    /// Outputs that could not carry EXIF metadata.
    metadata_dropped: u64,
    /// Total work items (from `Started`).
    total_files: u64,
}

impl StdoutState {
    fn new() -> Self {
        StdoutState {
            bar: None,
            input_bytes: 0,
            output_bytes: 0,
            preexisting_input: 0,
            preexisting_output: 0,
            ok: 0,
            skipped: 0,
            errors: 0,
            collisions: 0,
            discarded: 0,
            discarded_input: 0,
            discarded_output: 0,
            aborted: 0,
            metadata_dropped: 0,
            total_files: 0,
        }
    }
}

/// humansize formatting used by the progress bar and the statistics block
/// (binary units, two decimals, no space).
fn size_format() -> humansize::FormatSizeOptions {
    humansize::FormatSizeOptions::from(humansize::BINARY)
        .decimal_places(2)
        .decimal_zeroes(2)
        .space_after_value(false)
}

impl StdoutReporter {
    /// Creates a reporter that renders the classic CLI output.
    #[must_use]
    pub fn new() -> Self {
        StdoutReporter {
            state: Mutex::new(StdoutState::new()),
        }
    }

    /// Prints a notice line; while the progress bar is live the line is
    /// cleared first (matching the pipeline's former inline prints).
    fn print_notice(message: &str, bar_active: bool) {
        if bar_active {
            // carriage return and clear line contents, mirroring the
            // historical per-file error/collision prints
            println!("\r\x1b[2K{message}");
        } else {
            println!("{message}");
        }
    }

    /// Renders the running progress-bar message from the accumulated
    /// counters (same math and format as the pipeline's former
    /// `pb.set_message` call).
    fn bar_message(state: &StdoutState) -> String {
        let fmt = size_format();
        if state.preexisting_input > 0 {
            format!(
                "{} ➜ {} ({} ➜ {} preexisting) | ✔ {} — {} ✖ {}",
                humansize::format_size(state.input_bytes, fmt),
                humansize::format_size(state.output_bytes, fmt),
                humansize::format_size(state.preexisting_input, fmt),
                humansize::format_size(state.preexisting_output, fmt),
                state.ok,
                state.skipped,
                state.errors
            )
        } else {
            format!(
                "{} ➜ {} | ✔ {} — {} ✖ {}",
                humansize::format_size(state.input_bytes, fmt),
                humansize::format_size(state.output_bytes, fmt),
                state.ok,
                state.skipped,
                state.errors
            )
        }
    }

    /// Tallies a finished outcome into the running counters (same bucketing
    /// as the pipeline's `RunStats`).
    fn tally(state: &mut StdoutState, outcome: &Outcome) {
        match outcome {
            Outcome::Encoded {
                input_size,
                output_size,
                metadata_dropped,
                ..
            } => {
                state.ok += 1;
                state.input_bytes += input_size;
                state.output_bytes += output_size;
                state.metadata_dropped += u64::from(*metadata_dropped);
            }
            Outcome::SkippedExisting {
                input_size,
                existing_size,
                ..
            }
            | Outcome::DiscardedLargerThanExisting {
                input_size,
                existing_size,
            } => {
                state.skipped += 1;
                state.input_bytes += input_size;
                state.output_bytes += existing_size;
                state.preexisting_input += input_size;
                state.preexisting_output += existing_size;
            }
            Outcome::SkippedCollision { .. } => state.collisions += 1,
            Outcome::DiscardedLargerThanInput {
                input_size,
                encoded_size,
            } => {
                state.discarded += 1;
                state.discarded_input += input_size;
                state.discarded_output += encoded_size;
            }
            Outcome::Error(_) => state.errors += 1,
            Outcome::Aborted => state.aborted += 1,
        }
    }

    /// Renders the final "Encode statistics" block (historical format).
    fn print_statistics(state: &StdoutState, elapsed: std::time::Duration) {
        let fmt = size_format();
        println!("Encode statistics:");
        println!("Time taken:  {}", HumanDuration(elapsed));
        println!("Input files: {}", state.total_files);
        println!("Successful:  {}", state.ok);
        println!("Skipped:     {}", state.skipped);
        if state.collisions > 0 {
            println!(
                "Collisions:  {} (skipped, the output path was already produced by another input)",
                state.collisions
            );
        }
        println!("Errors:      {}", state.errors);
        if state.metadata_dropped > 0 {
            println!(
                "Metadata:    {} outputs could not carry EXIF metadata (target format has no support)",
                state.metadata_dropped
            );
        }
        if state.discarded > 0 {
            println!(
                "Discarded:   {} (due to the encode being larger than the input; {} ➜ {})",
                state.discarded,
                humansize::format_size(state.discarded_input, fmt),
                humansize::format_size(state.discarded_output, fmt)
            );
            println!(
                "Please note that discarded in- and outputs do not count into the total in-/output statistics below."
            );
        }
        if state.input_bytes > 0 && state.output_bytes > 0 {
            println!(
                "Total input size:  {}",
                humansize::format_size(state.input_bytes, fmt)
            );
            println!(
                "Total output size: {}",
                humansize::format_size(state.output_bytes, fmt)
            );
            println!(
                "Total comp. ratio: {:.02}%",
                state.output_bytes as f64 / state.input_bytes as f64 * 100.0
            );
            if state.preexisting_input > 0 && state.preexisting_output > 0 {
                if state.input_bytes - state.preexisting_input > 0 {
                    println!(
                        "New encodes input size:  {}",
                        humansize::format_size(state.input_bytes - state.preexisting_input, fmt)
                    );
                    println!(
                        "New encodes output size: {}",
                        humansize::format_size(state.output_bytes - state.preexisting_output, fmt)
                    );
                    println!(
                        "New encodes comp. ratio: {:.02}%",
                        state.preexisting_output as f64 / state.preexisting_input as f64 * 100.0
                    );
                }
                println!(
                    "Preexisting input size:  {}",
                    humansize::format_size(state.preexisting_input, fmt)
                );
                println!(
                    "Preexisting output size: {}",
                    humansize::format_size(state.preexisting_output, fmt)
                );
                println!(
                    "Preexisting comp. ratio: {:.02}%",
                    state.preexisting_output as f64 / state.preexisting_input as f64 * 100.0
                );
            }
        } else if (state.ok + state.skipped + state.errors) > 1 {
            println!(
                "Input and output size could not be determined, please try using OS-native binaries."
            );
        }
    }
}

impl Default for StdoutReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Reporter for StdoutReporter {
    fn on_event(&self, ev: JobEvent) {
        let mut state = self.state.lock().expect("stdout reporter mutex poisoned");
        match ev {
            JobEvent::Notice { message, .. } => {
                Self::print_notice(&message, state.bar.is_some());
            }
            JobEvent::Started { total_files } => {
                state.total_files = total_files;
                let pb = ProgressBar::new(total_files);
                let style = ProgressStyle::with_template(
                    "[{elapsed_precise}/~{duration_precise} ({eta_precise} rem.)] {wide_bar:.cyan/blue} {pos:>7}/{len:7} | {msg}",
                )
                .unwrap();
                pb.set_style(style);
                state.bar = Some(pb);
            }
            JobEvent::FileFinished { outcome, .. } => {
                Self::tally(&mut state, &outcome);
                if let Some(pb) = &state.bar {
                    pb.inc(1);
                    pb.set_message(Self::bar_message(&state));
                }
            }
            JobEvent::Finished => {
                if let Some(pb) = state.bar.take() {
                    let elapsed = pb.elapsed();
                    // clear the remnants of the progress bar off the screen
                    pb.finish_with_message("finished!");
                    Self::print_statistics(&state, elapsed);
                }
            }
            // the stdout rendering derives everything from FileFinished
            JobEvent::FileStarted { .. } | JobEvent::ProgressStats { .. } => {}
        }
    }
}

/// Writes one JSON object per [`JobEvent`] to a file (JSON-lines).
///
/// Every line is flushed immediately so the log can be tailed while a batch
/// runs. Compile-time gated behind the `logs` feature.
#[cfg(feature = "logs")]
pub struct JsonlReporter {
    /// Buffered file writer; flushed after every event.
    writer: Mutex<BufWriter<fs::File>>,
}

#[cfg(feature = "logs")]
impl JsonlReporter {
    /// Creates (or truncates) the log file at `path`.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] when the file cannot be opened.
    pub fn new(path: &Path) -> std::io::Result<Self> {
        let file = fs::File::create(path)?;
        Ok(JsonlReporter {
            writer: Mutex::new(BufWriter::new(file)),
        })
    }
}

#[cfg(feature = "logs")]
impl Reporter for JsonlReporter {
    fn on_event(&self, ev: JobEvent) {
        let mut writer = self.writer.lock().expect("jsonl reporter mutex poisoned");
        if let Ok(line) = serde_json::to_string(&ev) {
            let _ = writeln!(writer, "{line}");
            let _ = writer.flush();
        }
    }
}
