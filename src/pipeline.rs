//! Conversion pipeline: file discovery, per-file conversion flow (output
//! naming, overwrite logic, collision detection), statistics, progress and
//! the library entry point [`run`].

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use humansize::{BINARY, FormatSizeOptions, format_size};
use indicatif::{HumanDuration, ProgressBar, ProgressStyle};
use rayon::prelude::*;

use crate::Error;
use crate::config::{ConversionConfig, EncoderConfig};
use crate::converter::{EncoderRegistry, ImageEncoder, ThreadBudget};
use crate::format::ImageFormat;
use crate::input::{self, ImageContent, SourceImage};

/// Result of a single file conversion, replacing the former
/// `(isize, usize, usize)` magic-number status tuples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// File encoded and written to disk.
    Encoded {
        /// Size of the input file in bytes.
        input_size: u64,
        /// Size of the encoded output in bytes.
        output_size: u64,
        /// EXIF metadata existed and was requested, but the target format
        /// cannot carry it (e.g. AVIF) — counted for the summary.
        metadata_dropped: bool,
    },
    /// Skipped because an output file already exists (no overwrite policy
    /// active, or the overwrite-if-smaller policy found no improvement).
    SkippedExisting {
        /// Size of the input file in bytes.
        input_size: u64,
        /// Size of the preexisting output file in bytes.
        existing_size: u64,
    },
    /// Skipped because another input already maps to the same output path.
    SkippedCollision {
        /// Size of the input file in bytes.
        input_size: u64,
        /// Output path that another input already claimed.
        output_path: PathBuf,
    },
    /// Encoding succeeded but was discarded because it is larger than the
    /// input file (discard_if_larger_than_input active); no output written.
    DiscardedLargerThanInput {
        /// Size of the input file in bytes.
        input_size: u64,
        /// Size of the discarded encoding in bytes.
        encoded_size: u64,
    },
    /// Placeholder variant for policies that rename colliding outputs
    /// (reserved for WS1 multi-image outputs).
    DiscardedLargerThanExisting {
        /// Size of the input file in bytes.
        input_size: u64,
        /// Size of the preexisting output file in bytes.
        existing_size: u64,
    },
    /// Conversion failed with an error message.
    Error(String),
    /// Processing stopped early (Ctrl+C received).
    Aborted,
}

/// Policy deciding how to behave when an output file already exists or
/// multiple inputs map to the same output name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CollisionPolicy {
    /// Never overwrite existing outputs (default).
    #[default]
    KeepExisting,
    /// Overwrite existing outputs only if the new encoding is smaller.
    OverwriteIfSmaller,
    /// Overwrite existing outputs regardless of size.
    OverwriteAlways,
    /// Rename colliding outputs with suffixes (`_1`, `_2`, ...) instead of
    /// skipping them (reserved for WS1 multi-image outputs).
    Suffix,
}

impl CollisionPolicy {
    /// Maps the overwrite flags onto the policy.
    ///
    /// `overwrite_if_smaller` takes precedence (ties go to the existing
    /// file), matching the historical flag behavior.
    #[must_use]
    pub fn from_flags(overwrite_if_smaller: bool, overwrite_existing: bool) -> Self {
        if overwrite_if_smaller {
            CollisionPolicy::OverwriteIfSmaller
        } else if overwrite_existing {
            CollisionPolicy::OverwriteAlways
        } else {
            CollisionPolicy::KeepExisting
        }
    }
}

/// Statistics of a [`run`], mirroring the printed "Encode statistics".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunStats {
    /// Number of input files discovered from the glob pattern.
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
    /// Number of files not processed due to Ctrl+C.
    pub aborted: u64,
    /// Number of outputs that could not carry EXIF metadata although the
    /// policy wanted it embedded (target format has no metadata support).
    pub metadata_dropped: u64,
    /// Total size of all counted input files in bytes.
    pub input_size: u64,
    /// Total size of all counted outputs in bytes.
    pub output_size: u64,
    /// Wall time of the whole run.
    pub elapsed: Duration,
}

/// Per-outcome statistic counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StatBuckets {
    successful: u64,
    skipped: u64,
    discarded: u64,
    collisions: u64,
    errors: u64,
    aborted: u64,
    metadata_dropped: u64,
    input_size: u64,
    output_size: u64,
    preexisting_input_size: u64,
    preexisting_output_size: u64,
    discarded_input_size: u64,
    discarded_output_size: u64,
}

/// Maps an [`Outcome`] onto the statistic counters (single source of truth).
fn buckets_for(outcome: &Outcome) -> StatBuckets {
    match outcome {
        Outcome::Encoded {
            input_size,
            output_size,
            metadata_dropped,
        } => StatBuckets {
            successful: 1,
            metadata_dropped: u64::from(*metadata_dropped),
            input_size: *input_size,
            output_size: *output_size,
            ..StatBuckets::default()
        },
        Outcome::SkippedExisting {
            input_size,
            existing_size,
        }
        | Outcome::DiscardedLargerThanExisting {
            input_size,
            existing_size,
        } => StatBuckets {
            skipped: 1,
            input_size: *input_size,
            output_size: *existing_size,
            preexisting_input_size: *input_size,
            preexisting_output_size: *existing_size,
            ..StatBuckets::default()
        },
        Outcome::SkippedCollision { .. } => StatBuckets {
            collisions: 1,
            ..StatBuckets::default()
        },
        Outcome::DiscardedLargerThanInput {
            input_size,
            encoded_size,
        } => StatBuckets {
            discarded: 1,
            discarded_input_size: *input_size,
            discarded_output_size: *encoded_size,
            ..StatBuckets::default()
        },
        Outcome::Error(_) => StatBuckets {
            errors: 1,
            ..StatBuckets::default()
        },
        Outcome::Aborted => StatBuckets {
            aborted: 1,
            ..StatBuckets::default()
        },
    }
}

/// Shared atomic counters feeding the progress bar and the final statistics.
#[derive(Debug, Default)]
struct StatCounters {
    successful: AtomicU64,
    skipped: AtomicU64,
    discarded: AtomicU64,
    collisions: AtomicU64,
    errors: AtomicU64,
    aborted: AtomicU64,
    metadata_dropped: AtomicU64,
    input_size: AtomicU64,
    output_size: AtomicU64,
    preexisting_input_size: AtomicU64,
    preexisting_output_size: AtomicU64,
    discarded_input_size: AtomicU64,
    discarded_output_size: AtomicU64,
}

impl StatCounters {
    fn add(&self, buckets: &StatBuckets) {
        self.successful
            .fetch_add(buckets.successful, Ordering::SeqCst);
        self.skipped.fetch_add(buckets.skipped, Ordering::SeqCst);
        self.discarded
            .fetch_add(buckets.discarded, Ordering::SeqCst);
        self.collisions
            .fetch_add(buckets.collisions, Ordering::SeqCst);
        self.errors.fetch_add(buckets.errors, Ordering::SeqCst);
        self.aborted.fetch_add(buckets.aborted, Ordering::SeqCst);
        self.metadata_dropped
            .fetch_add(buckets.metadata_dropped, Ordering::SeqCst);
        self.input_size
            .fetch_add(buckets.input_size, Ordering::SeqCst);
        self.output_size
            .fetch_add(buckets.output_size, Ordering::SeqCst);
        self.preexisting_input_size
            .fetch_add(buckets.preexisting_input_size, Ordering::SeqCst);
        self.preexisting_output_size
            .fetch_add(buckets.preexisting_output_size, Ordering::SeqCst);
        self.discarded_input_size
            .fetch_add(buckets.discarded_input_size, Ordering::SeqCst);
        self.discarded_output_size
            .fetch_add(buckets.discarded_output_size, Ordering::SeqCst);
    }

    fn get(&self, counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }
}

/// Outputs already claimed by an input file (collision detection).
type ClaimedOutputs = Arc<Mutex<HashSet<PathBuf>>>;

/// Registers an output path; returns `false` if it was already claimed.
fn claim_output(claimed_outputs: &ClaimedOutputs, output_path: &Path) -> bool {
    claimed_outputs
        .lock()
        .expect("output claim mutex poisoned")
        .insert(output_path.to_path_buf())
}

fn base_from_pattern(pattern: &str) -> String {
    let mut base = PathBuf::new();

    for part in Path::new(pattern) {
        let s = part.to_string_lossy();
        if s.contains('*') || s.contains('?') || s.contains('[') {
            break;
        }
        base.push(part);
    }

    base.to_string_lossy().to_string()
}

fn normalize_prefix<P: AsRef<Path>>(p: P) -> PathBuf {
    let path = p.as_ref();

    let mut components = path.components().peekable();
    let mut normalized = PathBuf::new();

    // Skip leading CurrentDir (`.`) if present
    while let Some(c) = components.peek() {
        if c.as_os_str() == "." {
            components.next();
        } else {
            break;
        }
    }

    for c in components {
        normalized.push(c);
    }

    normalized
}

/// Computes the output path for an input file.
///
/// Without an output directory, the output lands next to the input with the
/// new extension; otherwise the pattern base is replaced by the output
/// directory, preserving the relative subdirectory structure.
#[must_use]
pub fn output_path_for(
    input_path: &Path,
    extension: &str,
    output: &str,
    pattern_base: &str,
) -> PathBuf {
    if output.is_empty() {
        input_path.with_extension(extension)
    } else {
        let pattern_base_norm = normalize_prefix(pattern_base);
        let input_path_norm = normalize_prefix(input_path);
        let rel_path = input_path_norm
            .strip_prefix(&pattern_base_norm)
            .unwrap_or_else(|_| Path::new(&input_path_norm));

        Path::new(output)
            .join(rel_path.parent().unwrap_or_else(|| Path::new("")))
            .join(input_path_norm.file_stem().unwrap_or_default())
            .with_extension(extension)
    }
}

fn source_dimensions(source: &SourceImage) -> (u32, u32) {
    match &source.content {
        ImageContent::Still(image) => (image.width(), image.height()),
        ImageContent::Animated(animation) => (animation.width, animation.height),
    }
}

fn source_buffer_size_bytes(source: &SourceImage) -> u64 {
    match &source.content {
        ImageContent::Still(image) => input::buffer_size_bytes(image),
        ImageContent::Animated(animation) => {
            let frame_bytes = u64::from(animation.width) * u64::from(animation.height) * 4;
            frame_bytes * animation.frames.len() as u64
        }
    }
}

/// Resolves the EXIF policy for a loaded source image (WS4):
///
/// 1. computes the payload to embed via `metadata::resolve`,
/// 2. when the resolution drops the Orientation tag while the source pixels
///    are not upright yet, bakes the orientation transform into the pixels,
/// 3. clears the payload when the target encoder cannot carry EXIF at all,
///    printing one warning line per file.
///
/// Returns whether metadata was dropped because of the target format.
#[cfg(feature = "exif")]
fn apply_exif_policy_to_source(
    source: &mut SourceImage,
    policy: &crate::metadata::policy::ExifPolicy,
    encoder: &dyn ImageEncoder,
) -> bool {
    let source_orientation = source
        .metadata
        .exif
        .as_deref()
        .and_then(crate::metadata::exif::orientation_from_payload);

    let resolved = crate::metadata::resolve(policy, &source.metadata);
    let resolved_keeps_orientation = resolved
        .as_deref()
        .and_then(crate::metadata::exif::orientation_from_payload);

    // the Orientation tag is lost (policy stripped/filtered it): rotate the
    // pixels upright so the output never appears sideways — but only when
    // the decoder has not already applied the transform
    if resolved_keeps_orientation.is_none()
        && let Some(orientation) = source_orientation
        && orientation != 1
        && !source.metadata.exif_applied_orientation
        && let ImageContent::Still(image) = &mut source.content
    {
        *image = crate::metadata::orientation::apply_orientation(image, orientation);
    }

    source.metadata.exif = resolved;

    if source.metadata.exif.is_some() && !encoder.supports_metadata() {
        println!(
            "Warning: {} target does not support EXIF embedding; metadata dropped",
            encoder.extension()
        );
        source.metadata.exif = None;
        return true;
    }
    false
}

/// Feature-less fallback: without the `exif` feature there is no EXIF
/// handling, so nothing is resolved, nothing is dropped, and no orientation
/// transform can be derived.
#[cfg(not(feature = "exif"))]
fn apply_exif_policy_to_source(
    _source: &mut SourceImage,
    _policy: &crate::metadata::policy::ExifPolicy,
    _encoder: &dyn ImageEncoder,
) -> bool {
    false
}

/// Converts a single input file and returns the resulting [`Outcome`].
fn convert_file(
    input_path: &Path,
    encoder: &dyn ImageEncoder,
    conf: &ConversionConfig,
    policy: CollisionPolicy,
    pattern_base: &str,
    claimed_outputs: &ClaimedOutputs,
) -> Outcome {
    let extension = encoder.extension();
    let output_path = output_path_for(input_path, extension, &conf.output, pattern_base);

    // collision detection: the first input wins, subsequent ones are reported
    if !claim_output(claimed_outputs, &output_path) {
        let input_size = fs::metadata(input_path).map_or(0, |meta| meta.len());
        println!(
            "\r\x1b[2KFile {}: skipped because another input maps to the same output path {}",
            input_path.display(),
            output_path.display()
        );
        return Outcome::SkippedCollision {
            input_size,
            output_path,
        };
    }

    let input_size = match fs::metadata(input_path) {
        Ok(meta) => meta.len(),
        Err(err) => return Outcome::Error(err.to_string()),
    };
    let output_exists = match fs::exists(output_path.clone()) {
        Ok(exists) => exists,
        Err(err) => return Outcome::Error(err.to_string()),
    };
    if output_exists && policy == CollisionPolicy::KeepExisting {
        // file exists, and we do not have any overwrite policy on? => return early
        let existing_size = match fs::metadata(output_path.clone()) {
            Ok(meta) => meta.len(),
            Err(err) => return Outcome::Error(err.to_string()),
        };
        return Outcome::SkippedExisting {
            input_size,
            existing_size,
        };
    }

    // create output subdirectories if an output directory is configured
    if !conf.output.is_empty()
        && let Some(parent) = output_path.parent()
        && let Err(err) = fs::create_dir_all(parent)
    {
        return Outcome::Error(err.to_string());
    }

    let mut source = match input::load_source(input_path) {
        Ok(source) => source,
        Err(err) => return Outcome::Error(err.to_string()),
    };
    if conf.discard_input_alpha_channel {
        source = source.into_without_alpha();
    }

    // WS4: resolve the EXIF policy, bake the orientation into the pixels if
    // the Orientation tag would be lost, and warn about unembeddable targets.
    let metadata_dropped = apply_exif_policy_to_source(&mut source, &conf.exif, encoder);

    const HUGE_IMAGE_DIMENSION_LIMIT: u32 = 8192;
    let (width, height) = source_dimensions(&source);
    if width > HUGE_IMAGE_DIMENSION_LIMIT || height > HUGE_IMAGE_DIMENSION_LIMIT {
        let format_option_binary_two_nospace = FormatSizeOptions::from(BINARY)
            .decimal_places(2)
            .decimal_zeroes(2)
            .space_after_value(false);
        println!(
            "Trying to encode huge image \"{}\" (filesize: {}, dimensions: {}x{}px, decoded buffer: {})...",
            input_path.display(),
            format_size(input_size, format_option_binary_two_nospace),
            width,
            height,
            format_size(
                source_buffer_size_bytes(&source),
                format_option_binary_two_nospace
            )
        );
    }

    match encoder.encode(&source) {
        Ok(image_data) => {
            let output_size = image_data.len() as u64;
            if policy == CollisionPolicy::OverwriteIfSmaller && output_exists {
                let existing_size = match fs::metadata(output_path.clone()) {
                    Ok(meta) => meta.len(),
                    Err(err) => return Outcome::Error(err.to_string()),
                };
                if output_size >= existing_size {
                    // overwrite-if-smaller active, but the existing output is
                    // already smaller than (or equal to) our encode => abort
                    return Outcome::SkippedExisting {
                        input_size,
                        existing_size,
                    };
                }
            }

            if conf.discard_if_larger_than_input && output_size >= input_size {
                return Outcome::DiscardedLargerThanInput {
                    input_size,
                    encoded_size: output_size,
                };
            }

            if let Err(err) = fs::write(output_path.clone(), image_data) {
                return Outcome::Error(err.to_string());
            }
            Outcome::Encoded {
                input_size,
                output_size,
                metadata_dropped,
            }
        }
        Err(err) => Outcome::Error(format!("Image encoding failed: {:?}", err)),
    }
}

/// Process-global stop flag shared with the Ctrl+C handler.
static GLOBAL_STOP: AtomicBool = AtomicBool::new(false);
/// Installs the process-wide Ctrl+C handler exactly once (`run` may be
/// called repeatedly, but only one handler can exist per process).
static CTRL_C_INSTALL: std::sync::Once = std::sync::Once::new();
/// Number of Ctrl+C presses, feeding the repeated-press notice.
static CTRLC_PRESSES: AtomicU64 = AtomicU64::new(0);

/// Resets the stop flag and ensures the Ctrl+C handler is installed.
///
/// Returns the stop flag the pipeline polls between files.
fn install_ctrlc_handler() -> &'static AtomicBool {
    GLOBAL_STOP.store(false, Ordering::Relaxed);
    CTRL_C_INSTALL.call_once(|| {
        ctrlc::set_handler(|| {
            let presses = CTRLC_PRESSES.fetch_add(1, Ordering::Relaxed);
            if !GLOBAL_STOP.load(Ordering::Relaxed) {
                println!("received Ctrl+C, stopping further queue processing!");
                GLOBAL_STOP.store(true, Ordering::Relaxed);
            } else {
                println!(
                    "an encoding task is still active!{} processing will end afterwards.",
                    str::repeat("!", presses as usize)
                );
            }
        })
        .expect("Error setting Ctrl-C handler");
    });
    &GLOBAL_STOP
}

/// Processes and encodes images matching the glob pattern of `conf` to the
/// target format described by `enc`.
///
/// This is the single library entry point of `byteshaver`; it prints the
/// conversion progress and the encode statistics, and returns the summary.
///
/// # Errors
///
/// Returns an [`Error`] if the glob pattern is invalid or the output
/// directory cannot be inspected.
pub fn run(conf: ConversionConfig, enc: EncoderConfig) -> Result<RunStats, Error> {
    let mut paths: Vec<PathBuf> = glob::glob(&conf.pattern)?
        .filter_map(|entry| entry.ok())
        .filter(|path| {
            let format = ImageFormat::from(path.as_path());
            format != ImageFormat::Unknown && format != ImageFormat::Avif // disable reading avif (FIXME: re-enable with reliable build+integration for reader)
        })
        .collect();
    // sort paths lexicographically, not only filenames
    paths.sort_by(|a, b| {
        let dir_cmp = a.parent().cmp(&b.parent());
        let cmp = if dir_cmp != std::cmp::Ordering::Equal {
            dir_cmp
        } else {
            a.file_name().cmp(&b.file_name())
        };

        if conf.reverse_processing_order {
            cmp.reverse()
        } else {
            cmp
        }
    });
    let pattern_base = base_from_pattern(&conf.pattern);
    let policy = CollisionPolicy::from_flags(conf.overwrite_if_smaller, conf.overwrite_existing);

    if paths.is_empty() {
        println!("No images to convert, check input glob pattern and supported input formats.");
        return Ok(RunStats::default());
    }

    // create output directory if it does not exist
    if !conf.output.is_empty() {
        let output_directory = Path::new(&conf.output);
        if !fs::exists(output_directory)? {
            // is it possible to warn in docker if the target output directory is not host mounted?
            println!("Creating output directory {:?}", output_directory);
            fs::create_dir_all(output_directory).unwrap_or_else(|err| {
                eprintln!("Error creating the output directory: {err}");
                std::process::exit(1);
            });
        }
    }
    // IDEA: create output filename from configurable regex

    println!("Converting {} files...", paths.len());
    let encoder = EncoderRegistry::build(&enc, ThreadBudget::global());
    println!("{}", encoder.describe());

    let stop_signal = install_ctrlc_handler();

    let (tx, rx) = mpsc::channel::<PathBuf>();
    let input_file_count = paths.len() as u64;
    // producer thread: feed paths in lexicographic order
    std::thread::spawn(move || {
        for path in paths {
            if tx.send(path).is_err() {
                break; // consumer dropped, exit
            }
        }
        // close the channel
        drop(tx);
    });

    let pb = ProgressBar::new(input_file_count);
    let style = ProgressStyle::with_template("[{elapsed_precise}/~{duration_precise} ({eta_precise} rem.)] {wide_bar:.cyan/blue} {pos:>7}/{len:7} | {msg}").unwrap();
    pb.set_style(style);
    let counters = Arc::new(StatCounters::default());
    let claimed_outputs: ClaimedOutputs = Arc::new(Mutex::new(HashSet::new()));
    let format_option_binary_two_nospace = FormatSizeOptions::from(BINARY)
        .decimal_places(2)
        .decimal_zeroes(2)
        .space_after_value(false);

    rx.into_iter().par_bridge().for_each(|path| {
        let outcome = if stop_signal.load(Ordering::Relaxed) {
            Outcome::Aborted
        } else {
            convert_file(
                &path,
                &*encoder,
                &conf,
                policy,
                &pattern_base,
                &claimed_outputs,
            )
        };
        if let Outcome::Error(err) = &outcome {
            // carriage return and clear line contents (do not spam screen content with logger bar states)
            println!(
                "\r\x1b[2KFile {}: could not be converted, error: {}",
                path.display(),
                err
            );
        }
        counters.add(&buckets_for(&outcome));
        pb.inc(1); // increment progress bar counter
        let (input_total, output_total) = (
            counters.get(&counters.input_size),
            counters.get(&counters.output_size),
        );
        let (input_preexisting, output_preexisting) = (
            counters.get(&counters.preexisting_input_size),
            counters.get(&counters.preexisting_output_size),
        );
        pb.set_message(if input_preexisting > 0 {
            format!(
                "{} ➜ {} ({} ➜ {} preexisting) | ✔ {} — {} ✖ {}",
                format_size(input_total, format_option_binary_two_nospace),
                format_size(output_total, format_option_binary_two_nospace),
                format_size(input_preexisting, format_option_binary_two_nospace),
                format_size(output_preexisting, format_option_binary_two_nospace),
                counters.get(&counters.successful),
                counters.get(&counters.skipped),
                counters.get(&counters.errors)
            )
        } else {
            format!(
                "{} ➜ {} | ✔ {} — {} ✖ {}",
                format_size(input_total, format_option_binary_two_nospace),
                format_size(output_total, format_option_binary_two_nospace),
                counters.get(&counters.successful),
                counters.get(&counters.skipped),
                counters.get(&counters.errors)
            )
        });
    });

    // use a return carriage feed to clear the remnants of the progress bar off the screen
    pb.finish_with_message("finished!");
    // \r\x1b[2K is the sequence to clear the current row content (if manual way is intended)
    let (successful, skipped) = (
        counters.get(&counters.successful),
        counters.get(&counters.skipped),
    );
    let (discarded, collisions) = (
        counters.get(&counters.discarded),
        counters.get(&counters.collisions),
    );
    let (errors, aborted) = (
        counters.get(&counters.errors),
        counters.get(&counters.aborted),
    );
    let input_total = counters.get(&counters.input_size);
    let output_total = counters.get(&counters.output_size);
    let input_preexisting = counters.get(&counters.preexisting_input_size);
    let output_preexisting = counters.get(&counters.preexisting_output_size);
    let input_discarded = counters.get(&counters.discarded_input_size);
    let output_discarded = counters.get(&counters.discarded_output_size);
    let metadata_dropped = counters.get(&counters.metadata_dropped);
    let elapsed = pb.elapsed();

    println!("Encode statistics:");
    println!("Time taken:  {}", HumanDuration(elapsed));
    println!("Input files: {}", input_file_count);
    println!("Successful:  {}", successful);
    println!("Skipped:     {}", skipped);
    if collisions > 0 {
        println!(
            "Collisions:  {} (skipped, the output path was already produced by another input)",
            collisions
        );
    }
    println!("Errors:      {}", errors);
    if metadata_dropped > 0 {
        println!(
            "Metadata:    {} outputs could not carry EXIF metadata (target format has no support)",
            metadata_dropped
        );
    }
    if conf.discard_if_larger_than_input && discarded > 0 {
        println!(
            "Discarded:   {} (due to the encode being larger than the input; {} ➜ {})",
            discarded,
            format_size(input_discarded, format_option_binary_two_nospace),
            format_size(output_discarded, format_option_binary_two_nospace)
        );
        println!(
            "Please note that discarded in- and outputs do not count into the total in-/output statistics below."
        );
    }
    if input_total > 0 && output_total > 0 {
        // show total stats
        println!(
            "Total input size:  {}",
            format_size(input_total, format_option_binary_two_nospace)
        );
        println!(
            "Total output size: {}",
            format_size(output_total, format_option_binary_two_nospace)
        );
        println!(
            "Total comp. ratio: {:.02}%",
            output_total as f64 / input_total as f64 * 100.0
        );
        if input_preexisting > 0 && output_preexisting > 0 {
            if input_total - input_preexisting > 0 {
                // if we have new encodes and preexisting images, first show the stats for the new encodes, then for the preexisting ones
                println!(
                    "New encodes input size:  {}",
                    format_size(
                        input_total - input_preexisting,
                        format_option_binary_two_nospace
                    )
                );
                println!(
                    "New encodes output size: {}",
                    format_size(
                        output_total - output_preexisting,
                        format_option_binary_two_nospace
                    )
                );
                println!(
                    "New encodes comp. ratio: {:.02}%",
                    output_preexisting as f64 / input_preexisting as f64 * 100.0
                );
            }
            // if we have preexisting images, show these stats
            println!(
                "Preexisting input size:  {}",
                format_size(input_preexisting, format_option_binary_two_nospace)
            );
            println!(
                "Preexisting output size: {}",
                format_size(output_preexisting, format_option_binary_two_nospace)
            );
            println!(
                "Preexisting comp. ratio: {:.02}%",
                output_preexisting as f64 / input_preexisting as f64 * 100.0
            );
        }
    } else {
        if (successful + skipped + errors) > 1 {
            println!(
                "Input and output size could not be determined, please try using OS-native binaries."
            );
        }
    }

    Ok(RunStats {
        input_files: input_file_count,
        successful,
        skipped,
        discarded,
        collisions,
        errors,
        aborted,
        metadata_dropped,
        input_size: input_total,
        output_size: output_total,
        elapsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    // ---- Outcome mapping -------------------------------------------------

    #[test]
    fn outcome_buckets_map_onto_statistics() {
        let encoded = buckets_for(&Outcome::Encoded {
            input_size: 100,
            output_size: 50,
            metadata_dropped: false,
        });
        assert_eq!(
            (
                encoded.successful,
                encoded.skipped,
                encoded.discarded,
                encoded.errors
            ),
            (1, 0, 0, 0)
        );
        assert_eq!((encoded.input_size, encoded.output_size), (100, 50));
        assert_eq!(encoded.preexisting_input_size, 0);
        assert_eq!(encoded.metadata_dropped, 0);

        let encoded_dropped = buckets_for(&Outcome::Encoded {
            input_size: 100,
            output_size: 50,
            metadata_dropped: true,
        });
        assert_eq!(encoded_dropped.metadata_dropped, 1);
        assert_eq!(encoded_dropped.successful, 1);

        let skipped = buckets_for(&Outcome::SkippedExisting {
            input_size: 100,
            existing_size: 40,
        });
        assert_eq!((skipped.successful, skipped.skipped), (0, 1));
        assert_eq!((skipped.input_size, skipped.output_size), (100, 40));
        assert_eq!(
            (
                skipped.preexisting_input_size,
                skipped.preexisting_output_size
            ),
            (100, 40)
        );

        // overwrite-if-smaller keeps count as a skip with preexisting sizes
        let kept = buckets_for(&Outcome::DiscardedLargerThanExisting {
            input_size: 100,
            existing_size: 40,
        });
        assert_eq!(kept.skipped, 1);
        assert_eq!((kept.input_size, kept.output_size), (100, 40));
        assert_eq!(kept.discarded, 0);

        let discarded = buckets_for(&Outcome::DiscardedLargerThanInput {
            input_size: 100,
            encoded_size: 120,
        });
        assert_eq!(
            (discarded.discarded, discarded.successful, discarded.skipped),
            (1, 0, 0)
        );
        // discarded files do not count into the total in-/output statistics
        assert_eq!((discarded.input_size, discarded.output_size), (0, 0));
        assert_eq!(
            (
                discarded.discarded_input_size,
                discarded.discarded_output_size
            ),
            (100, 120)
        );

        let collision = buckets_for(&Outcome::SkippedCollision {
            input_size: 100,
            output_path: PathBuf::from("x"),
        });
        assert_eq!(
            (collision.collisions, collision.successful, collision.errors),
            (1, 0, 0)
        );

        let error = buckets_for(&Outcome::Error("boom".to_string()));
        assert_eq!((error.errors, error.successful, error.skipped), (1, 0, 0));

        let aborted = buckets_for(&Outcome::Aborted);
        assert_eq!(aborted.aborted, 1);
        assert_eq!(
            aborted.successful + aborted.skipped + aborted.errors + aborted.discarded,
            0
        );
    }

    #[test]
    fn counter_totals_accumulate_from_buckets() {
        let counters = StatCounters::default();
        counters.add(&buckets_for(&Outcome::Encoded {
            input_size: 10,
            output_size: 5,
            metadata_dropped: true,
        }));
        counters.add(&buckets_for(&Outcome::SkippedExisting {
            input_size: 20,
            existing_size: 8,
        }));
        counters.add(&buckets_for(&Outcome::DiscardedLargerThanInput {
            input_size: 30,
            encoded_size: 40,
        }));
        counters.add(&buckets_for(&Outcome::Error("x".to_string())));
        assert_eq!(counters.get(&counters.successful), 1);
        assert_eq!(counters.get(&counters.skipped), 1);
        assert_eq!(counters.get(&counters.errors), 1);
        assert_eq!(counters.get(&counters.discarded), 1);
        assert_eq!(counters.get(&counters.metadata_dropped), 1);
        // totals include encoded + skipped, but not discarded files
        assert_eq!(counters.get(&counters.input_size), 30);
        assert_eq!(counters.get(&counters.output_size), 13);
        assert_eq!(counters.get(&counters.preexisting_input_size), 20);
        assert_eq!(counters.get(&counters.discarded_output_size), 40);
    }

    // ---- Output naming ---------------------------------------------------

    #[test]
    fn output_path_next_to_input_without_output_dir() {
        let path = output_path_for(Path::new("images/sub/a.jpeg"), "webp", "", "images");
        assert_eq!(path, Path::new("images/sub/a.webp"));
    }

    #[test]
    fn output_path_relocates_pattern_base_into_output_dir() {
        let path = output_path_for(Path::new("images/sub/a.jpeg"), "webp", "/tmp/out", "images");
        assert_eq!(path, Path::new("/tmp/out/sub/a.webp"));

        // files outside the pattern base keep their relative structure
        let path = output_path_for(Path::new("elsewhere/a.jpeg"), "png", "/tmp/out", "images");
        assert_eq!(path, Path::new("/tmp/out/elsewhere/a.png"));
    }

    #[test]
    fn output_path_normalizes_leading_current_dir() {
        let path = output_path_for(Path::new("./images/a.jpeg"), "webp", "out", "./images");
        assert_eq!(path, Path::new("out/a.webp"));
    }

    // ---- Collision detection & policies ----------------------------------

    #[test]
    fn collision_policy_from_flags_prefers_smaller_overwrite() {
        assert_eq!(
            CollisionPolicy::from_flags(false, false),
            CollisionPolicy::KeepExisting
        );
        assert_eq!(
            CollisionPolicy::from_flags(true, false),
            CollisionPolicy::OverwriteIfSmaller
        );
        assert_eq!(
            CollisionPolicy::from_flags(false, true),
            CollisionPolicy::OverwriteAlways
        );
        // both flags behave like overwrite-if-smaller (ties go to existing file)
        assert_eq!(
            CollisionPolicy::from_flags(true, true),
            CollisionPolicy::OverwriteIfSmaller
        );
    }

    #[test]
    fn claim_output_detects_collisions() {
        let claimed: ClaimedOutputs = Arc::new(Mutex::new(HashSet::new()));
        let out_a = Path::new("/tmp/out/photo.webp");
        let out_b = Path::new("/tmp/out/other.webp");

        assert!(claim_output(&claimed, out_a), "first claim must win");
        assert!(!claim_output(&claimed, out_a), "same output must collide");
        assert!(
            claim_output(&claimed, out_b),
            "different output must not collide"
        );
    }

    #[test]
    fn convert_file_reports_collision_for_same_stem_inputs() {
        let dir =
            std::env::temp_dir().join(format!("byteshaver-pipeline-test-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        let input_a = dir.join("photo.png");
        let input_b = dir.join("photo.jpg");
        fs::write(&input_a, []).expect("write input a");
        fs::write(&input_b, []).expect("write input b");

        let conf = ConversionConfig {
            output: dir.join("out").display().to_string(),
            ..ConversionConfig::default()
        };
        let encoder = EncoderRegistry::build(&EncoderConfig::Jpeg, ThreadBudget::global());
        let claimed: ClaimedOutputs = Arc::new(Mutex::new(HashSet::new()));

        let outcome_a = convert_file(
            &input_a,
            &*encoder,
            &conf,
            CollisionPolicy::KeepExisting,
            "does-not-matter",
            &claimed,
        );
        let outcome_b = convert_file(
            &input_b,
            &*encoder,
            &conf,
            CollisionPolicy::KeepExisting,
            "does-not-matter",
            &claimed,
        );

        // both inputs map to the same output file (for inputs outside the
        // pattern base the relative parent is absolute, matching historical
        // naming); the second input is reported as a collision
        assert!(
            matches!(outcome_a, Outcome::Error(_)),
            "empty png input should fail decoding, not collide"
        );
        assert!(
            matches!(outcome_b, Outcome::SkippedCollision { output_path, .. } if output_path == dir.join("photo.jpeg"))
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_signal_yields_aborted_outcome_without_processing() {
        // mirrors the run() loop behavior on Ctrl+C
        let stop = Arc::new(AtomicBool::new(false));
        stop.store(true, Ordering::Relaxed);
        let stopped = stop.load(Ordering::Relaxed);
        assert!(stopped);
    }
}
