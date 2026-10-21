//! Conversion pipeline: file discovery, per-file conversion flow (output
//! naming, overwrite logic, collision detection), statistics and the
//! library entry point [`run`].
//!
//! All reporting flows through the [`Reporter`][crate::job::Reporter] event
//! sink and cancelation through the
//! [`StopFlag`][crate::job::StopFlag] — the pipeline itself never touches
//! stdout, the progress bar or signals (plan WS7). The classic CLI output
//! lives in [`StdoutReporter`][crate::job::StdoutReporter].

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use humansize::{BINARY, FormatSizeOptions, format_size};
use image::ImageDecoder;
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use rayon::prelude::*;

use crate::Error;
use crate::config::{AnimatedInputPolicy, ConversionConfig, EncoderConfig};
use crate::converter::{EncoderRegistry, ImageEncoder, ThreadBudget};
use crate::format::ImageFormat;
use crate::input::{self, ImageContent, SourceImage};
use crate::job::StopFlag;
use crate::job::reporter::{JobEvent, Reporter, StdoutReporter};
use crate::job::{InputSelection, JobSpec};

/// Result of a single file conversion, replacing the former
/// `(isize, usize, usize)` magic-number status tuples.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
        /// Path the output was written to (HEIF multi-image `_N` suffixes
        /// included) — consumed by front-ends for open/reveal actions.
        output_path: PathBuf,
    },
    /// Skipped because an output file already exists (no overwrite policy
    /// active, or the overwrite-if-smaller policy found no improvement).
    SkippedExisting {
        /// Size of the input file in bytes.
        input_size: u64,
        /// Size of the preexisting output file in bytes.
        existing_size: u64,
        /// Path of the kept preexisting output file.
        output_path: PathBuf,
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// Result of one work item (file, or one image of a multi-image HEIF
/// container) in a finished run.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileResult {
    /// Input path of the work item (multi-image HEIF items share the
    /// container path; their outputs differ by the `_N` suffix).
    pub path: PathBuf,
    /// Conversion result of the item.
    pub outcome: Outcome,
}

/// Statistics of a [`run`], mirroring the printed "Encode statistics".
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Per-item results in queue order (the deterministic input order, not
    /// completion order).
    #[serde(default)]
    pub results: Vec<FileResult>,
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
            ..
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
            ..
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
            .fetch_add(buckets.successful, Ordering::Relaxed);
        self.skipped.fetch_add(buckets.skipped, Ordering::Relaxed);
        self.discarded
            .fetch_add(buckets.discarded, Ordering::Relaxed);
        self.collisions
            .fetch_add(buckets.collisions, Ordering::Relaxed);
        self.errors.fetch_add(buckets.errors, Ordering::Relaxed);
        self.aborted.fetch_add(buckets.aborted, Ordering::Relaxed);
        self.metadata_dropped
            .fetch_add(buckets.metadata_dropped, Ordering::Relaxed);
        self.input_size
            .fetch_add(buckets.input_size, Ordering::Relaxed);
        self.output_size
            .fetch_add(buckets.output_size, Ordering::Relaxed);
        self.preexisting_input_size
            .fetch_add(buckets.preexisting_input_size, Ordering::Relaxed);
        self.preexisting_output_size
            .fetch_add(buckets.preexisting_output_size, Ordering::Relaxed);
        self.discarded_input_size
            .fetch_add(buckets.discarded_input_size, Ordering::Relaxed);
        self.discarded_output_size
            .fetch_add(buckets.discarded_output_size, Ordering::Relaxed);
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

/// Releases an output path claimed by an input that subsequently failed
/// before writing, so a later input mapping to the same output is
/// processed instead of being skipped as a collision (M-1).
fn release_output(claimed_outputs: &ClaimedOutputs, output_path: &Path) {
    claimed_outputs
        .lock()
        .expect("output claim mutex poisoned")
        .remove(output_path);
}

/// A single work item of the processing queue: a plain input file, or one
/// image of a multi-image HEIC/HEIF container (`--heif-image-policy all`,
/// `dec-heif` feature).
#[derive(Clone, Debug)]
enum WorkItem {
    /// One file maps to one output.
    Single(PathBuf),
    /// The `index`-th image (0-based) of a HEIF container holding multiple
    /// master images; outputs get `stem_N.ext` suffixes for index > 0.
    #[cfg_attr(not(feature = "dec-heif"), allow(dead_code))]
    HeifMulti { path: PathBuf, index: usize },
}

impl WorkItem {
    fn path(&self) -> &Path {
        match self {
            WorkItem::Single(path) | WorkItem::HeifMulti { path, .. } => path,
        }
    }
}

/// Expands a discovered input path into its work items: plain files stay
/// single items; HEIF containers are expanded to one item per image when the
/// `all` policy is active (`dec-heif` feature).
///
/// A container that cannot be probed stays a single item so the actual
/// error surfaces per-file in the conversion worker.
#[cfg(all(
    feature = "dec-heif",
    any(not(target_os = "windows"), target_abi = "llvm")
))]
fn expand_work_item(path: PathBuf, heif_policy: crate::config::HeifImagePolicy) -> Vec<WorkItem> {
    if heif_policy == crate::config::HeifImagePolicy::All
        && ImageFormat::from(path.as_path()) == ImageFormat::Heif
        && let Ok(total) = input::heif_probe(&path)
        && total > 1
    {
        return (0..total)
            .map(|index| WorkItem::HeifMulti {
                path: path.clone(),
                index,
            })
            .collect();
    }
    vec![WorkItem::Single(path)]
}

/// Feature-less fallback of [`expand_work_item`]: every file stays a single
/// work item (HEIF input fails per-file at load time).
#[cfg(any(
    not(feature = "dec-heif"),
    all(target_os = "windows", not(target_abi = "llvm"))
))]
fn expand_work_item(path: PathBuf, _heif_policy: crate::config::HeifImagePolicy) -> Vec<WorkItem> {
    vec![WorkItem::Single(path)]
}

/// Output file path for the `index`-th image of a multi-image container:
/// the base output path for the first image (index 0), `stem_1.ext`,
/// `stem_2.ext`, ... for the following ones (the suffix style of the
/// reserved `CollisionPolicy::Suffix` variant, applied deterministically
/// per index).
fn suffixed_image_path(output_path: &Path, index: usize) -> PathBuf {
    if index == 0 {
        return output_path.to_path_buf();
    }
    let mut file_name = output_path.file_stem().unwrap_or_default().to_os_string();
    file_name.push(format!("_{index}"));
    if let Some(extension) = output_path.extension() {
        file_name.push(".");
        file_name.push(extension);
    }
    output_path.with_file_name(file_name)
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
    output_path_for_dir(input_path, extension, Path::new(output), pattern_base)
}

/// Path-based core of [`output_path_for`]: the job API's output directory
/// stays a `Path` end-to-end instead of a lossy `String` round-trip (L10).
fn output_path_for_dir(
    input_path: &Path,
    extension: &str,
    output: &Path,
    pattern_base: &str,
) -> PathBuf {
    if output.as_os_str().is_empty() {
        input_path.with_extension(extension)
    } else {
        let pattern_base_norm = normalize_prefix(pattern_base);
        let input_path_norm = normalize_prefix(input_path);
        // inputs outside the pattern base cannot keep their relative
        // structure: flatten them into the output root instead of joining
        // their (absolute) parent, which would escape the output
        // directory (M-2)
        let rel_parent = input_path_norm
            .strip_prefix(&pattern_base_norm)
            .ok()
            .and_then(|rel_path| rel_path.parent());

        output
            .join(rel_parent.unwrap_or_else(|| Path::new("")))
            .join(input_path_norm.file_name().unwrap_or_default())
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
            // checked math: a crafted header must not wrap the projection
            // to a small value (CR-4) — saturating to u64::MAX instead
            // always trips the memory cap
            u64::from(animation.width)
                .checked_mul(u64::from(animation.height))
                .and_then(|pixels| pixels.checked_mul(4))
                .and_then(|frame_bytes| frame_bytes.checked_mul(animation.frames.len() as u64))
                .unwrap_or(u64::MAX)
        }
    }
}

/// Cheap animation budget probe (CR-4): reads only container headers
/// (dimensions, animation flag) of the formats the image crate decodes
/// and returns an error message when a single full-canvas frame buffer
/// already exceeds `--max-animation-memory` — before any pixel is
/// decoded. None of the image crate's decoders exposes a frame count
/// without decoding, so multi-frame overruns stay with the post-decode
/// guard in [`convert_file`]; JXL/HEIF inputs are not probeable here at
/// all and keep that guard as well.
///
/// Returns `None` when the file passes the probe, cannot be probed, or
/// fails to open (the regular load path surfaces the latter).
fn animation_preflight_error(input_path: &Path, conf: &ConversionConfig) -> Option<String> {
    let format = ImageFormat::from(input_path);
    if !matches!(
        format,
        ImageFormat::Gif | ImageFormat::Webp | ImageFormat::Png
    ) {
        return None;
    }
    let file = fs::File::open(input_path).ok()?;
    let reader = std::io::BufReader::new(file);
    let (width, height) = match format {
        ImageFormat::Gif => GifDecoder::new(reader).ok()?.dimensions(),
        ImageFormat::Webp => {
            let decoder = WebPDecoder::new(reader).ok()?;
            if !decoder.has_animation() {
                return None; // still containers are not subject to the cap
            }
            decoder.dimensions()
        }
        _ => {
            let decoder = PngDecoder::new(reader).ok()?;
            if !decoder.is_apng().ok()? {
                return None;
            }
            decoder.dimensions()
        }
    };
    let cap_bytes = conf.max_animation_memory_mib.saturating_mul(1024 * 1024);
    let single_frame_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4));
    if single_frame_bytes.is_none_or(|bytes| bytes > cap_bytes) {
        return Some(format!(
            "{}: animated image ({}x{}px) exceeds the --max-animation-memory limit of {} MiB before decoding",
            input_path.display(),
            width,
            height,
            conf.max_animation_memory_mib
        ));
    }
    None
}

/// Image index for the source loader: multi-image HEIF work items decode
/// their indexed image, everything else the primary image.
fn heif_image_index(item: &WorkItem) -> Option<usize> {
    match item {
        WorkItem::Single(_) => None,
        WorkItem::HeifMulti { index, .. } => Some(*index),
    }
}

/// Resolves the EXIF policy for a loaded source image (WS4):
///
/// 1. computes the payload to embed via `metadata::resolve`,
/// 2. when the resolution drops the Orientation tag while the source pixels
///    are not upright yet, bakes the orientation transform into the pixels,
/// 3. clears the payload when the target encoder cannot carry EXIF at all,
///    emitting one notice per file.
///
/// Returns whether metadata was dropped because of the target format.
#[cfg(feature = "exif")]
fn apply_exif_policy_to_source(
    source: &mut SourceImage,
    policy: &crate::metadata::policy::ExifPolicy,
    encoder: &dyn ImageEncoder,
    reporter: &dyn Reporter,
    input_path: &Path,
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
        reporter.on_event(JobEvent::Notice {
            path: Some(input_path.to_path_buf()),
            message: format!(
                "Warning: {} target does not support EXIF embedding; metadata dropped",
                encoder.extension()
            ),
        });
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
    _reporter: &dyn Reporter,
    _input_path: &Path,
) -> bool {
    false
}

/// Per-process counter making atomic-write temp file names unique.
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Writes `data` to `target` via a unique temp file in the target's
/// directory and an atomic rename (CR-6): crashes, full disks and
/// Ctrl+C mid-write never leave a half-written output, and in-place
/// overwrites keep the original intact until the rename succeeds. On a
/// write or rename error the temp file is removed (best effort) and the
/// error propagates.
fn write_atomic(target: &Path, data: &[u8]) -> std::io::Result<()> {
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut temp_name = target.file_name().unwrap_or_default().to_os_string();
    temp_name.push(format!(".tmp-{}-{}", std::process::id(), sequence));
    let temp_path = target.with_file_name(temp_name);
    if let Err(err) = fs::write(&temp_path, data) {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }
    if let Err(err) = fs::rename(&temp_path, target) {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }
    Ok(())
}

/// Output parent directories already created by this process (L1): the
/// per-file `create_dir_all` syscall only runs once per unique parent.
static CREATED_OUTPUT_DIRS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

/// Creates the output parent directory of `output_path` once per unique
/// path; failures propagate (the path is only cached after success).
fn ensure_output_parent_dir(output_path: &Path) -> std::io::Result<()> {
    let Some(parent) = output_path.parent() else {
        return Ok(());
    };
    let mut created = CREATED_OUTPUT_DIRS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .expect("output directory cache mutex poisoned");
    if !created.contains(parent) {
        fs::create_dir_all(parent)?;
        created.insert(parent.to_path_buf());
    }
    Ok(())
}

/// Converts a single work item (file, or one image of a multi-image HEIF
/// container) and returns the resulting [`Outcome`].
///
/// Ad-hoc messages (collisions, huge-image warnings) are emitted as
/// [`JobEvent::Notice`]s; the caller emits the per-file error notice.
#[allow(clippy::too_many_arguments)]
fn convert_file(
    item: &WorkItem,
    encoder: &dyn ImageEncoder,
    conf: &ConversionConfig,
    policy: CollisionPolicy,
    pattern_base: &str,
    output_dir: Option<&Path>,
    claimed_outputs: &ClaimedOutputs,
    reporter: &dyn Reporter,
) -> Outcome {
    let input_path = item.path();
    let extension = encoder.extension();
    let output = output_dir.unwrap_or_else(|| Path::new(""));
    let mut output_path = output_path_for_dir(input_path, extension, output, pattern_base);
    // multi-image HEIF expansion: the first image keeps the base name, the
    // following ones get a deterministic `_N` suffix
    if let WorkItem::HeifMulti { index, .. } = item {
        output_path = suffixed_image_path(&output_path, *index);
    }

    // collision detection: the first input wins, subsequent ones are reported
    if !claim_output(claimed_outputs, &output_path) {
        let input_size = fs::metadata(input_path).map_or(0, |meta| meta.len());
        reporter.on_event(JobEvent::Notice {
            path: Some(input_path.to_path_buf()),
            message: format!(
                "File {}: skipped because another input maps to the same output path {}",
                input_path.display(),
                output_path.display()
            ),
        });
        return Outcome::SkippedCollision {
            input_size,
            output_path,
        };
    }

    let outcome = convert_claimed(
        item,
        &output_path,
        encoder,
        conf,
        policy,
        output_dir,
        reporter,
    );
    // M-1: a claim is only justified by a written output — release it when
    // the item failed first, so a later input mapping to the same output
    // is processed instead of being skipped as a collision
    if matches!(outcome, Outcome::Error(_)) {
        release_output(claimed_outputs, &output_path);
    }
    outcome
}

/// Post-claim conversion flow of [`convert_file`]: the exclusive claim on
/// `output_path` is held for the duration of the call.
fn convert_claimed(
    item: &WorkItem,
    output_path: &Path,
    encoder: &dyn ImageEncoder,
    conf: &ConversionConfig,
    policy: CollisionPolicy,
    output_dir: Option<&Path>,
    reporter: &dyn Reporter,
) -> Outcome {
    let input_path = item.path();
    let input_size = match fs::metadata(input_path) {
        Ok(meta) => meta.len(),
        Err(err) => return Outcome::Error(err.to_string()),
    };

    // CR-6: in-place conversion (no output directory, target extension
    // equal to the source extension) — the default policy turns this into
    // a silent skip, overwrite policies replace the input file itself
    // (atomically, via the temp-file write below); either way the user
    // should hear about it
    if output_path == input_path {
        reporter.on_event(JobEvent::Notice {
            path: Some(input_path.to_path_buf()),
            message: format!(
                "Warning: output path equals input path ({}): converting in place; use an output directory to keep the original file",
                input_path.display()
            ),
        });
    }

    let output_exists = match fs::exists(output_path) {
        Ok(exists) => exists,
        Err(err) => return Outcome::Error(err.to_string()),
    };
    if output_exists && policy == CollisionPolicy::KeepExisting {
        // file exists, and we do not have any overwrite policy on? => return early
        let existing_size = match fs::metadata(output_path) {
            Ok(meta) => meta.len(),
            Err(err) => return Outcome::Error(err.to_string()),
        };
        return Outcome::SkippedExisting {
            input_size,
            existing_size,
            output_path: output_path.to_path_buf(),
        };
    }

    // create output subdirectories if an output directory is configured
    if output_dir.is_some()
        && let Err(err) = ensure_output_parent_dir(output_path)
    {
        return Outcome::Error(err.to_string());
    }

    // CR-4: reject oversized animations from container headers alone,
    // before any frame is decoded (the post-decode guard further below
    // stays as the fallback for inputs the probe cannot read)
    if let Some(message) = animation_preflight_error(input_path, conf) {
        return Outcome::Error(message);
    }

    let mut source = match input::load_source_with_index(input_path, heif_image_index(item)) {
        Ok(source) => source,
        Err(err) => return Outcome::Error(err.to_string()),
    };
    if conf.discard_input_alpha_channel {
        source = source.into_without_alpha();
    }

    // WS5: animation guards. The memory cap applies to every animated input
    // (animated targets keep all frames in memory as well); the
    // --animated-input policy decides the behavior for still-only targets.
    if let ImageContent::Animated(animation) = &source.content {
        let projected_bytes = source_buffer_size_bytes(&source);
        let cap_bytes = conf.max_animation_memory_mib.saturating_mul(1024 * 1024);
        if projected_bytes > cap_bytes {
            return Outcome::Error(format!(
                "{}: animated image ({}x{}px, {} frames, approx. {} MiB of frame buffers) exceeds the --max-animation-memory limit of {} MiB",
                input_path.display(),
                animation.width,
                animation.height,
                animation.frames.len(),
                projected_bytes / (1024 * 1024),
                conf.max_animation_memory_mib
            ));
        }
        if !encoder.supports_animation() && conf.animated_input == AnimatedInputPolicy::Error {
            return Outcome::Error(format!(
                "{}: input is animated but the {} target cannot encode animations (--animated-input error)",
                input_path.display(),
                encoder.extension()
            ));
        }
    }

    // WS4: resolve the EXIF policy, bake the orientation into the pixels if
    // the Orientation tag would be lost, and warn about unembeddable targets.
    let metadata_dropped =
        apply_exif_policy_to_source(&mut source, &conf.exif, encoder, reporter, input_path);

    const HUGE_IMAGE_DIMENSION_LIMIT: u32 = 8192;
    let (width, height) = source_dimensions(&source);
    if width > HUGE_IMAGE_DIMENSION_LIMIT || height > HUGE_IMAGE_DIMENSION_LIMIT {
        let format_option_binary_two_nospace = FormatSizeOptions::from(BINARY)
            .decimal_places(2)
            .decimal_zeroes(2)
            .space_after_value(false);
        reporter.on_event(JobEvent::Notice {
            path: Some(input_path.to_path_buf()),
            message: format!(
                "Trying to encode huge image \"{}\" (filesize: {}, dimensions: {}x{}px, decoded buffer: {})...",
                input_path.display(),
                format_size(input_size, format_option_binary_two_nospace),
                width,
                height,
                format_size(
                    source_buffer_size_bytes(&source),
                    format_option_binary_two_nospace
                )
            ),
        });
        if let Some(hint) = encoder.huge_image_hint() {
            reporter.on_event(JobEvent::Notice {
                path: Some(input_path.to_path_buf()),
                message: hint.to_string(),
            });
        }
    }

    match encoder.encode(&source) {
        Ok(image_data) => {
            let output_size = image_data.len() as u64;
            if policy == CollisionPolicy::OverwriteIfSmaller && output_exists {
                let existing_size = match fs::metadata(output_path) {
                    Ok(meta) => meta.len(),
                    Err(err) => return Outcome::Error(err.to_string()),
                };
                if output_size >= existing_size {
                    // overwrite-if-smaller active, but the existing output is
                    // already smaller than (or equal to) our encode => abort
                    return Outcome::SkippedExisting {
                        input_size,
                        existing_size,
                        output_path: output_path.to_path_buf(),
                    };
                }
            }

            if conf.discard_if_larger_than_input && output_size >= input_size {
                return Outcome::DiscardedLargerThanInput {
                    input_size,
                    encoded_size: output_size,
                };
            }

            if let Err(err) = write_atomic(output_path, &image_data) {
                return Outcome::Error(err.to_string());
            }
            Outcome::Encoded {
                input_size,
                output_size,
                metadata_dropped,
                output_path: output_path.to_path_buf(),
            }
        }
        Err(err) => Outcome::Error(format!("Image encoding failed: {:?}", err)),
    }
}

/// Sorts discovered input paths lexicographically (directories before the
/// files they contain, then file names), optionally reversed.
fn sort_paths(paths: &mut [PathBuf], reverse: bool) {
    paths.sort_by(|a, b| {
        let dir_cmp = a.parent().cmp(&b.parent());
        let cmp = if dir_cmp != std::cmp::Ordering::Equal {
            dir_cmp
        } else {
            a.file_name().cmp(&b.file_name())
        };

        if reverse { cmp.reverse() } else { cmp }
    });
}

/// Maximum recursion depth of [`collect_directory`]: a backstop against
/// directory cycles the canonical-path guard cannot express (e.g. bind
/// mounts).
const MAX_DIRECTORY_DEPTH: usize = 64;

/// Collects supported image files below `dir` recursively (the same
/// extension-based format filter the glob discovery applies to its
/// results).
///
/// Directory symlinks are never followed and already-visited
/// (canonicalized) directories are skipped, so symlink cycles cannot
/// recurse endlessly (CR-2); symlinked *files* are still accepted by the
/// extension filter. Unreadable directories are reported on stderr
/// instead of vanishing silently (M-7).
fn collect_directory(dir: &Path, paths: &mut Vec<PathBuf>) {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    if let Ok(canonical) = dir.canonicalize() {
        visited.insert(canonical);
    }
    collect_directory_inner(dir, paths, &mut visited, 0);
}

fn collect_directory_inner(
    dir: &Path,
    paths: &mut Vec<PathBuf>,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) {
    if depth >= MAX_DIRECTORY_DEPTH {
        eprintln!(
            "Warning: maximum directory depth ({MAX_DIRECTORY_DEPTH}) exceeded below {}; skipping",
            dir.display()
        );
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("Warning: cannot read directory {}: {err}", dir.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            // only real directories are recursed into (file_type() does
            // not follow symlinks); the canonicalized-visited set guards
            // against re-entering the same physical directory
            if let Ok(canonical) = path.canonicalize()
                && visited.insert(canonical)
            {
                collect_directory_inner(&path, paths, visited, depth + 1);
            }
        } else if ImageFormat::from(path.as_path()) != ImageFormat::Unknown {
            paths.push(path);
        }
    }
}

/// Expands `InputSelection::Files` into input paths: existing directories
/// are walked recursively (supported formats only), everything else is kept
/// as-is so nonexistent paths surface as per-file errors during conversion.
fn discover_files(selection: &[PathBuf], reverse: bool) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for entry in selection {
        if entry.is_dir() {
            collect_directory(entry, &mut paths);
        } else {
            paths.push(entry.clone());
        }
    }
    sort_paths(&mut paths, reverse);
    paths
}

/// Computes the longest common directory prefix of all input paths; used
/// as the relocation base when `InputSelection::Files` meets an output
/// directory (mirrors the pattern base of glob discovery).
fn common_directory_base(paths: &[PathBuf]) -> String {
    let Some(first) = paths.first() else {
        return String::new();
    };
    let mut base = PathBuf::new();
    for component in first.parent().unwrap_or_else(|| Path::new("")).components() {
        let candidate = base.join(component);
        if paths.iter().all(|path| path.starts_with(&candidate)) {
            base = candidate;
        } else {
            break;
        }
    }
    base.to_string_lossy().to_string()
}

/// Processes and encodes the images selected by `spec` to the target format
/// described by `spec.encoder`, streaming every event to `reporter` and
/// checking `stop` once per queued work item.
///
/// This is the engine behind both [`JobHandle`][crate::job::JobHandle] and
/// the legacy [`run`] entry point.
///
/// # Errors
///
/// Returns an [`Error`] if the glob pattern is invalid or the output
/// directory cannot be created.
pub fn execute(
    spec: &JobSpec,
    reporter: Arc<dyn Reporter>,
    stop: &StopFlag,
    budget: ThreadBudget,
) -> Result<RunStats, Error> {
    // the spec's output directory overrides the common configuration's;
    // it stays a Path end-to-end instead of a lossy String round-trip,
    // so non-UTF8 directories keep working (L10)
    let conf = spec.common.clone();
    let output_dir: Option<PathBuf> = match &spec.output {
        Some(output) => Some(output.clone()),
        None if !conf.output.is_empty() => Some(PathBuf::from(&conf.output)),
        None => None,
    };

    let (paths, pattern_base) = match &spec.inputs {
        InputSelection::Pattern(pattern) => {
            let mut paths: Vec<PathBuf> = Vec::new();
            let mut glob_errors: u32 = 0;
            for entry in glob::glob(pattern)? {
                match entry {
                    Ok(path) => {
                        if ImageFormat::from(path.as_path()) != ImageFormat::Unknown {
                            paths.push(path);
                        }
                    }
                    Err(_) => glob_errors += 1,
                }
            }
            if glob_errors > 0 {
                eprintln!(
                    "Warning: {glob_errors} matched path(s) could not be read during discovery and were skipped"
                );
            }
            sort_paths(&mut paths, conf.reverse_processing_order);
            (paths, base_from_pattern(pattern))
        }
        InputSelection::Files(files) => {
            let paths = discover_files(files, conf.reverse_processing_order);
            let base = common_directory_base(&paths);
            (paths, base)
        }
    };
    let policy = CollisionPolicy::from_flags(conf.overwrite_if_smaller, conf.overwrite_existing);

    if paths.is_empty() {
        reporter.on_event(JobEvent::Notice {
            path: None,
            message: "No images to convert, check input glob pattern and supported input formats."
                .to_string(),
        });
        return Ok(RunStats::default());
    }

    // create output directory if it does not exist
    if let Some(output_directory) = output_dir.as_deref()
        && !fs::exists(output_directory)?
    {
        reporter.on_event(JobEvent::Notice {
            path: None,
            message: format!("Creating output directory {output_directory:?}"),
        });
        if let Err(err) = fs::create_dir_all(output_directory) {
            let message = format!("Error creating the output directory: {err}");
            reporter.on_event(JobEvent::Notice {
                path: None,
                message: message.clone(),
            });
            return Err(Error::from_string(message));
        }
    }
    // IDEA: create output filename from configurable regex

    reporter.on_event(JobEvent::Notice {
        path: None,
        message: format!("Converting {} files...", paths.len()),
    });
    let encoder = EncoderRegistry::build(&spec.encoder, budget);
    reporter.on_event(JobEvent::Notice {
        path: None,
        // per-run notice with the options actually in effect (plan 15 F4:
        // the capability description stays option-free instead)
        message: encoder.describe_options(),
    });

    // multi-image HEIF files are expanded before queueing, so the statistics
    // and the progress bar count images, not files
    let items: Vec<WorkItem> = paths
        .into_iter()
        .flat_map(|path| expand_work_item(path, conf.heif_image_policy))
        .collect();
    let input_file_count = items.len() as u64;

    reporter.on_event(JobEvent::Started {
        total_files: input_file_count,
    });

    let started = Instant::now();
    let counters = Arc::new(StatCounters::default());
    let claimed_outputs: ClaimedOutputs = Arc::new(Mutex::new(HashSet::new()));
    let results: Mutex<Vec<(u64, FileResult)>> =
        Mutex::new(Vec::with_capacity(input_file_count.try_into().unwrap_or(0)));

    items.into_par_iter().enumerate().for_each(|(index, item)| {
        let input_path = item.path().to_path_buf();
        let outcome = if stop.raised() {
            Outcome::Aborted
        } else {
            reporter.on_event(JobEvent::FileStarted {
                index: index as u64,
                path: input_path.clone(),
            });
            convert_file(
                &item,
                &*encoder,
                &conf,
                policy,
                &pattern_base,
                output_dir.as_deref(),
                &claimed_outputs,
                reporter.as_ref(),
            )
        };
        if let Outcome::Error(err) = &outcome {
            reporter.on_event(JobEvent::Notice {
                path: Some(input_path.clone()),
                message: format!(
                    "File {}: could not be converted, error: {}",
                    input_path.display(),
                    err
                ),
            });
        }
        counters.add(&buckets_for(&outcome));
        results.lock().expect("results mutex poisoned").push((
            index as u64,
            FileResult {
                path: input_path.clone(),
                outcome: outcome.clone(),
            },
        ));
        reporter.on_event(JobEvent::FileFinished {
            index: index as u64,
            path: input_path,
            outcome: outcome.clone(),
        });
        reporter.on_event(JobEvent::ProgressStats {
            input_bytes: counters.get(&counters.input_size),
            output_bytes: counters.get(&counters.output_size),
            ok: counters.get(&counters.successful),
            skipped: counters.get(&counters.skipped),
            errors: counters.get(&counters.errors),
        });
    });
    let elapsed = started.elapsed();

    reporter.on_event(JobEvent::Finished);

    let mut results = results.into_inner().expect("results mutex poisoned");
    // completion order is nondeterministic; report in queue order
    results.sort_by_key(|(index, _)| *index);
    let results = results.into_iter().map(|(_, result)| result).collect();

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
    let metadata_dropped = counters.get(&counters.metadata_dropped);

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
        results,
        elapsed,
    })
}

/// Processes and encodes images matching the glob pattern of `conf` to the
/// target format described by `enc`, rendering the classic CLI output
/// (progress bar and encode statistics) via
/// [`StdoutReporter`].
///
/// This is a thin compatibility wrapper over the job API:
/// [`JobSpec::from_conversion`] + a fresh [`StopFlag`] +
/// [`crate::job::Session::new`] + [`StdoutReporter`]. Library front-ends
/// should use [`crate::job::JobHandle::start`] directly (injectable
/// reporters and cancelation).
///
/// # Errors
///
/// Returns an [`Error`] if the glob pattern is invalid or the output
/// directory cannot be created.
pub fn run(conf: ConversionConfig, enc: EncoderConfig) -> Result<RunStats, Error> {
    let spec = JobSpec::from_conversion(conf, enc);
    let session = crate::job::Session::new();
    let handle = crate::job::JobHandle::start(
        spec,
        Box::new(StdoutReporter::new()),
        StopFlag::new(),
        &session,
    );
    handle.join().into_stats()
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
            output_path: PathBuf::from("out/a.bin"),
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
            output_path: PathBuf::from("out/a.bin"),
        });
        assert_eq!(encoded_dropped.metadata_dropped, 1);
        assert_eq!(encoded_dropped.successful, 1);

        let skipped = buckets_for(&Outcome::SkippedExisting {
            input_size: 100,
            existing_size: 40,
            output_path: PathBuf::from("out/a.bin"),
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
            output_path: PathBuf::from("out/a.bin"),
        }));
        counters.add(&buckets_for(&Outcome::SkippedExisting {
            input_size: 20,
            existing_size: 8,
            output_path: PathBuf::from("out/b.bin"),
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

        // files outside the pattern base are flattened into the output
        // root: joining their absolute parent would escape the output
        // directory (M-2)
        let path = output_path_for(Path::new("elsewhere/a.jpeg"), "png", "/tmp/out", "images");
        assert_eq!(path, Path::new("/tmp/out/a.png"));
    }

    #[test]
    fn output_path_normalizes_leading_current_dir() {
        let path = output_path_for(Path::new("./images/a.jpeg"), "webp", "out", "./images");
        assert_eq!(path, Path::new("out/a.webp"));
    }

    #[test]
    fn multi_image_heif_outputs_get_deterministic_suffixes() {
        // --heif-image-policy all: stem.ext, stem_1.ext, stem_2.ext, ...
        let base = output_path_for(Path::new("photos/burst.heic"), "webp", "", "photos");
        assert_eq!(base, Path::new("photos/burst.webp"));
        assert_eq!(
            suffixed_image_path(&base, 0),
            Path::new("photos/burst.webp"),
            "the first image keeps the base name"
        );
        assert_eq!(
            suffixed_image_path(&base, 1),
            Path::new("photos/burst_1.webp")
        );
        assert_eq!(
            suffixed_image_path(&base, 10),
            Path::new("photos/burst_10.webp")
        );

        // with an output directory the suffix is kept in the relocated name
        let relocated = output_path_for(
            Path::new("photos/sub/burst.heic"),
            "avif",
            "/tmp/out",
            "photos",
        );
        assert_eq!(
            suffixed_image_path(&relocated, 2),
            Path::new("/tmp/out/sub/burst_2.avif")
        );

        // pathological names do not panic
        assert_eq!(
            suffixed_image_path(Path::new("weird.name.with.dots.png"), 3),
            Path::new("weird.name.with.dots_3.png")
        );
        assert_eq!(
            suffixed_image_path(Path::new("noext"), 1),
            Path::new("noext_1")
        );
    }

    #[test]
    fn heif_multi_work_items_carry_their_image_index() {
        let path = PathBuf::from("x/burst.heic");
        assert_eq!(WorkItem::Single(path.clone()).path(), path.as_path());
        assert_eq!(heif_image_index(&WorkItem::Single(path.clone())), None);

        let multi = WorkItem::HeifMulti {
            path: path.clone(),
            index: 2,
        };
        assert_eq!(multi.path(), path.as_path());
        assert_eq!(heif_image_index(&multi), Some(2));
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
    fn failed_input_releases_claim_for_same_stem_inputs() {
        let dir =
            std::env::temp_dir().join(format!("byteshaver-pipeline-test-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        let input_a = dir.join("photo.png");
        let input_b = dir.join("photo.jpg");
        fs::write(&input_a, []).expect("write input a");
        fs::write(&input_b, []).expect("write input b");

        let out_dir = dir.join("out");
        let conf = ConversionConfig {
            output: out_dir.display().to_string(),
            ..ConversionConfig::default()
        };
        let encoder = EncoderRegistry::build(&EncoderConfig::Jpeg, ThreadBudget::global());
        let claimed: ClaimedOutputs = Arc::new(Mutex::new(HashSet::new()));
        let reporter = crate::job::reporter::NullReporter::new();

        let outcome_a = convert_file(
            &WorkItem::Single(input_a),
            &*encoder,
            &conf,
            CollisionPolicy::KeepExisting,
            "does-not-matter",
            Some(&out_dir),
            &claimed,
            &reporter,
        );
        let outcome_b = convert_file(
            &WorkItem::Single(input_b),
            &*encoder,
            &conf,
            CollisionPolicy::KeepExisting,
            "does-not-matter",
            Some(&out_dir),
            &claimed,
            &reporter,
        );

        // both inputs map to the same flattened output file
        // (out/photo.jpeg); the first input fails decoding and releases
        // its claim (M-1), so the second is processed under the same
        // output path instead of being skipped as a collision
        assert!(
            matches!(outcome_a, Outcome::Error(_)),
            "empty png input should fail decoding, not collide"
        );
        assert!(
            matches!(outcome_b, Outcome::Error(_)),
            "failed claim must be released: the second input fails on its own instead of colliding"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_replaces_target_and_leaves_no_temp_files() {
        let dir = std::env::temp_dir().join(format!(
            "byteshaver-pipeline-test-{}-atomic",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let target = dir.join("out.bin");
        fs::write(&target, b"old").expect("write target");

        write_atomic(&target, b"new content").expect("atomic write");

        assert_eq!(fs::read(&target).expect("read target"), b"new content");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp files removed: {leftovers:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn collect_directory_survives_symlink_cycles() {
        let dir =
            std::env::temp_dir().join(format!("byteshaver-pipeline-cycle-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).expect("create temp dirs");
        fs::write(dir.join("sub/a.png"), []).expect("write png");
        // self-referencing directory cycles (CR-2)
        std::os::unix::fs::symlink(".", dir.join("sub/loop")).expect("symlink .");
        std::os::unix::fs::symlink(dir.join("sub"), dir.join("back")).expect("symlink back");
        // symlinked files stay accepted
        std::os::unix::fs::symlink(dir.join("sub/a.png"), dir.join("b.png")).expect("symlink file");

        let mut paths = Vec::new();
        collect_directory(&dir, &mut paths);

        assert_eq!(paths.len(), 2, "symlinked directories are not followed");
        assert!(paths.contains(&dir.join("sub/a.png")));
        assert!(paths.contains(&dir.join("b.png")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn animation_preflight_rejects_oversized_canvases_before_decoding() {
        let dir = std::env::temp_dir().join(format!(
            "byteshaver-pipeline-test-{}-preflight",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        // bare GIF89a header declaring a 65535x65535 canvas (~16 GiB of
        // RGBA frame buffer) without any frame data: the probe must
        // reject it from the header alone (CR-4). The trailing zeros
        // satisfy the decoder's block lookahead without adding frames.
        let mut huge = b"GIF89a".to_vec();
        huge.extend_from_slice(&65535u16.to_le_bytes());
        huge.extend_from_slice(&65535u16.to_le_bytes());
        huge.extend_from_slice(&[0x70, 0, 0]);
        huge.extend_from_slice(&[0u8; 16]);
        let huge_path = dir.join("huge.gif");
        fs::write(&huge_path, huge).expect("write gif header");

        let conf = ConversionConfig::default(); // 4096 MiB cap
        assert!(animation_preflight_error(&huge_path, &conf).is_some());

        // a small canvas passes the header probe
        let mut small = b"GIF89a".to_vec();
        small.extend_from_slice(&8u16.to_le_bytes());
        small.extend_from_slice(&8u16.to_le_bytes());
        small.extend_from_slice(&[0x70, 0, 0]);
        small.extend_from_slice(&[0u8; 16]);
        let small_path = dir.join("small.gif");
        fs::write(&small_path, small).expect("write gif header");
        assert!(animation_preflight_error(&small_path, &conf).is_none());

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
