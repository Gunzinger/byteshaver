//! Queue model of the GUI (plan WS8 §1/§3): the list of files (and
//! directories) scheduled for conversion, their per-row status and sizes.
//!
//! All state transitions are pure functions of the incoming
//! [`JobEvent`][byteshaver::job::JobEvent] stream — no rendering state lives
//! here, which keeps every code path unit-testable headless (egui rendering
//! itself cannot be exercised without a display server).
//!
//! ## Directory handling
//!
//! Directories are enqueued as-is and stay untouched: the core expands them
//! recursively in `InputSelection::Files` discovery (same supported-format
//! filter as glob results). While a job runs, work items discovered under a
//! directory (or multi-image HEIF containers) arrive as [`JobEvent::
//! FileFinished`] events whose path has no dedicated queue row; the app
//! appends them as *discovered* rows (flagged, transient). They are dropped
//! again when the next run starts so the queue keeps representing the
//! user's input selection, not the last run's expansion.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use byteshaver::format::ImageFormat;
use byteshaver::pipeline::Outcome;

/// Status of one queue row, mapped 1:1 from the core's [`Outcome`] plus the
/// two UI-only states [`ItemStatus::Queued`] and [`ItemStatus::Running`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemStatus {
    /// Waiting for a job to pick it up.
    #[default]
    Queued,
    /// Inside the currently running job (not yet finished).
    Running,
    /// [`Outcome::Encoded`]: written to disk.
    Encoded,
    /// [`Outcome::SkippedExisting`] / [`Outcome::DiscardedLargerThanExisting`]:
    /// a preexisting output was kept.
    SkippedExisting,
    /// [`Outcome::SkippedCollision`]: another input claimed the output path.
    SkippedCollision,
    /// [`Outcome::DiscardedLargerThanInput`]: encode succeeded but was
    /// discarded (no output written).
    DiscardedLargerThanInput,
    /// [`Outcome::Error`]: failed; the message is kept on the row.
    Error,
    /// [`Outcome::Aborted`]: not processed because cancelation was requested.
    Aborted,
}

impl ItemStatus {
    /// Pure [`Outcome`] → row status mapping (unit-tested).
    #[must_use]
    pub fn from_outcome(outcome: &Outcome) -> Self {
        match outcome {
            Outcome::Encoded { .. } => ItemStatus::Encoded,
            Outcome::SkippedExisting { .. } | Outcome::DiscardedLargerThanExisting { .. } => {
                ItemStatus::SkippedExisting
            }
            Outcome::SkippedCollision { .. } => ItemStatus::SkippedCollision,
            Outcome::DiscardedLargerThanInput { .. } => ItemStatus::DiscardedLargerThanInput,
            Outcome::Error(_) => ItemStatus::Error,
            Outcome::Aborted => ItemStatus::Aborted,
        }
    }

    /// Short glyph used in the status column of the file table.
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            ItemStatus::Queued => "…",
            ItemStatus::Running => "⟳",
            ItemStatus::Encoded => "✔",
            ItemStatus::SkippedExisting => "—",
            ItemStatus::SkippedCollision => "⤷",
            ItemStatus::DiscardedLargerThanInput => "✂",
            ItemStatus::Error => "✖",
            ItemStatus::Aborted => "⏸",
        }
    }

    /// Status-column glyph during a run, refined by the job's active set
    /// (plan 12 §1): `Queue::begin_run` marks *every* row
    /// [`ItemStatus::Running`], so only the cross-check against
    /// `RunningJob::active` makes the glyph truthful — actively worked
    /// rows keep `⟳`, rows still waiting in this run show the weak
    /// pending `…`. Non-running rows delegate to [`ItemStatus::glyph`].
    // consumed by plan 11's file-table rewrite; unused until then
    #[allow(dead_code)]
    #[must_use]
    pub fn glyph_while_running(self, active: bool) -> &'static str {
        match self {
            ItemStatus::Running if active => "⟳",
            ItemStatus::Running => "…",
            other => other.glyph(),
        }
    }

    /// Human-readable status text for the file table and report panel.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ItemStatus::Queued => "queued",
            ItemStatus::Running => "running",
            ItemStatus::Encoded => "done",
            ItemStatus::SkippedExisting => "skipped (existing output kept)",
            ItemStatus::SkippedCollision => "skipped (output collision)",
            ItemStatus::DiscardedLargerThanInput => "discarded (larger than input)",
            ItemStatus::Error => "error",
            ItemStatus::Aborted => "not processed (canceled)",
        }
    }
}

/// Aggregated preview of a directory's convertible image content,
/// computed at enqueue time by a metadata-only recursive walk (one `stat`
/// per entry, no file reading) so dropping a folder shows its totals
/// immediately. Discovery filtering mirrors the core (`ImageFormat`
/// extension sniff); the conversion itself still hands the directory to
/// the core, which re-discovers with identical semantics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirSummary {
    /// Number of image files found (supported extensions, symlink targets
    /// excluded).
    pub images: u64,
    /// Total size in bytes of the found image files.
    pub bytes: u64,
    /// Per-format image counts (key: canonical extension, e.g. `jpeg`),
    /// sorted by count descending (ties: extension ascending).
    pub by_format: Vec<(String, u64)>,
}

impl DirSummary {
    /// Walks `dir` recursively and summarizes its convertible content.
    /// Runs synchronously on the calling (UI) thread — stat-only, so this
    /// stays responsive for typical photo libraries.
    #[must_use]
    pub fn scan(dir: &Path) -> Self {
        let mut summary = DirSummary::default();
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        Self::scan_recursive(dir, &mut summary, &mut counts);
        summary.by_format = sort_counts(counts);
        summary
    }

    fn scan_recursive(dir: &Path, summary: &mut Self, counts: &mut BTreeMap<String, u64>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            // file_type does not follow symlinks: symlinked files/dirs are
            // skipped (cycle-safe; the core's glob discovery expands them,
            // but the preview deliberately stays conservative)
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                Self::scan_recursive(&path, summary, counts);
            } else if file_type.is_file() {
                let format = ImageFormat::from(path.as_path());
                if format == ImageFormat::Unknown {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                summary.images += 1;
                summary.bytes += meta.len();
                *counts.entry(format.extension().to_string()).or_default() += 1;
            }
        }
    }

    /// Per-format counts as `JPEG 210 · PNG 100`, most common first;
    /// entries beyond `max` collapse into `+ N more`.
    #[must_use]
    pub fn breakdown(&self, max: usize) -> String {
        format_breakdown(&self.by_format, max)
    }
}

/// Sorts per-format counts by count descending (ties: key ascending).
#[must_use]
pub fn sort_counts(counts: BTreeMap<String, u64>) -> Vec<(String, u64)> {
    let mut entries: Vec<(String, u64)> = counts.into_iter().collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries
}

/// Formats per-format counts as `JPEG 210 · PNG 100` (most common first),
/// collapsing entries beyond `max` into `+ N more`. Keys are uppercased
/// for display.
#[must_use]
pub fn format_breakdown(counts: &[(String, u64)], max: usize) -> String {
    let shown = counts
        .iter()
        .take(max)
        .map(|(format, count)| format!("{} {}", format.to_uppercase(), count));
    let mut text: Vec<String> = shown.collect();
    if counts.len() > max {
        text.push(format!("+ {} more", counts.len() - max));
    }
    text.join(" · ")
}

/// One row of the conversion queue.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueItem {
    /// Path as enqueued (kept verbatim; the core resolves it).
    pub path: PathBuf,
    /// Cheap header/extension sniff via [`ImageFormat::from`] — no decoding
    /// at add-time. [`ImageFormat::Unknown`] for directories and
    /// unrecognized extensions.
    pub source_format: ImageFormat,
    /// Current row status.
    pub status: ItemStatus,
    /// Error text from [`Outcome::Error`].
    pub error: Option<String>,
    /// Additional grayed-out reason (unsupported extension, HEIF without
    /// `dec-heif`, directory expansion note). Purely informational; such
    /// rows are still converted (and surface real per-file errors).
    pub note: Option<String>,
    /// Input file size in bytes (`None` when not statable).
    pub input_size: Option<u64>,
    /// Output size in bytes of the last conversion.
    pub output_size: Option<u64>,
    /// Whether EXIF was requested but the target cannot carry it.
    pub metadata_dropped: bool,
    /// Directories are passed to the core as-is (recursive expansion there).
    pub is_dir: bool,
    /// Aggregated image content of a directory row (see [`DirSummary`]);
    /// `None` for file rows.
    pub summary: Option<DirSummary>,
    /// Transient row appended from a directory/HEIF expansion during a run
    /// (not part of the user's queue selection; dropped on the next run).
    pub discovered: bool,
}

impl QueueItem {
    /// Builds a fresh queued row for `path` (stat + format sniff).
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        let is_dir = path.is_dir();
        let source_format = if is_dir {
            ImageFormat::Unknown
        } else {
            ImageFormat::from(path.as_path())
        };
        let input_size = std::fs::metadata(&path).map(|meta| meta.len()).ok();
        let summary = if is_dir {
            Some(DirSummary::scan(&path))
        } else {
            None
        };
        QueueItem {
            path,
            source_format,
            status: ItemStatus::Queued,
            error: None,
            note: None,
            input_size,
            output_size: None,
            metadata_dropped: false,
            is_dir,
            summary,
            discovered: false,
        }
    }

    /// Whether this row is expected to be convertible in this build
    /// (supported extension and, for HEIF containers, a `dec-heif` build).
    /// Unsupported rows are still enqueued — they surface as per-file
    /// errors at run time (documented decision, plan WS8 task §4).
    #[must_use]
    pub fn is_supported(&self, heif_input_enabled: bool) -> bool {
        if self.is_dir {
            return true;
        }
        if self.source_format == ImageFormat::Unknown {
            return false;
        }
        if self.source_format == ImageFormat::Heif && !heif_input_enabled {
            return false;
        }
        true
    }

    /// Human-readable reason for an unsupported row (`None` if supported).
    #[must_use]
    pub fn unsupported_reason(&self, heif_input_enabled: bool) -> Option<String> {
        if self.is_supported(heif_input_enabled) {
            return None;
        }
        if self.source_format == ImageFormat::Heif {
            Some(
                "HEIF/HEIC/AVIF container input requires the \"dec-heif\" feature \
                 (not compiled into this build); the run reports a per-file error"
                    .to_string(),
            )
        } else {
            Some("unsupported input extension; the run reports a per-file error".to_string())
        }
    }

    /// Applies a finished outcome to the row (status, sizes, error text).
    pub fn apply_outcome(&mut self, outcome: &Outcome) {
        self.status = ItemStatus::from_outcome(outcome);
        self.metadata_dropped = false;
        self.error = None;
        self.note = None;
        match outcome {
            Outcome::Encoded {
                input_size,
                output_size,
                metadata_dropped,
                ..
            } => {
                self.input_size = Some(*input_size);
                self.output_size = Some(*output_size);
                self.metadata_dropped = *metadata_dropped;
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
                self.input_size = Some(*input_size);
                self.output_size = Some(*existing_size);
            }
            Outcome::SkippedCollision { input_size, .. } => {
                self.input_size = Some(*input_size);
            }
            Outcome::DiscardedLargerThanInput {
                input_size,
                encoded_size,
            } => {
                self.input_size = Some(*input_size);
                self.output_size = Some(*encoded_size);
            }
            Outcome::Error(message) => self.error = Some(message.clone()),
            Outcome::Aborted => {}
        }
    }
}

/// The conversion queue: ordered rows with canonical-path deduplication.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Queue {
    items: Vec<QueueItem>,
}

impl Queue {
    /// Creates an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read access to the rows.
    #[must_use]
    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    /// Mutable access to the rows (notice attachment from the app).
    pub(crate) fn items_mut(&mut self) -> &mut Vec<QueueItem> {
        &mut self.items
    }

    /// Number of rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the queue has no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether `path` is already enqueued (canonicalized comparison; paths
    /// that cannot be canonicalized — e.g. not-yet-existing files — compare
    /// verbatim).
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        let key = canonical_key(path);
        self.items.iter().any(|item| item.path == key)
    }

    /// Adds paths (files **and** directories — directories are passed to
    /// the core as-is and expanded recursively there), deduplicating
    /// against existing rows via their canonical path. Returns the number
    /// of rows actually added.
    pub fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> usize {
        let mut added = 0;
        for path in paths {
            let key = canonical_key(&path);
            if self.contains(&key) {
                continue;
            }
            self.items.push(QueueItem::new(key));
            added += 1;
        }
        added
    }

    /// Removes the row at `index` (no-op when out of bounds).
    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
        }
    }

    /// Removes every row.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Removes only the transient rows discovered during a run.
    pub fn clear_discovered(&mut self) {
        self.items.retain(|item| !item.discovered);
    }

    /// The plain input selection for a job spec (queue paths in order,
    /// directories included — the core expands them).
    #[must_use]
    pub fn selection(&self) -> Vec<PathBuf> {
        self.items.iter().map(|item| item.path.clone()).collect()
    }

    /// Marks every row running (start of a job) and drops the previous
    /// run's discovered rows.
    pub fn begin_run(&mut self) {
        self.clear_discovered();
        for item in &mut self.items {
            item.status = ItemStatus::Running;
            item.error = None;
            item.note = None;
            item.output_size = None;
            item.metadata_dropped = false;
        }
    }

    /// Applies a [`byteshaver::job::JobEvent::FileStarted`] event: flips the
    /// matching row to [`ItemStatus::Running`].
    pub fn apply_file_started(&mut self, path: &Path) {
        if let Some(item) = self.items.iter_mut().find(|item| item.path == path) {
            item.status = ItemStatus::Running;
        }
    }

    /// Applies a [`byteshaver::job::JobEvent::FileFinished`] event: updates
    /// the matching running row, or returns `false` when the path belongs
    /// to a directory/HEIF expansion without a dedicated row (the caller
    /// then appends a discovered row).
    pub fn apply_file_finished(&mut self, path: &Path, outcome: &Outcome) -> bool {
        let running = self
            .items
            .iter_mut()
            .find(|item| item.path == path && item.status == ItemStatus::Running);
        let Some(item) = running else {
            return false;
        };
        item.apply_outcome(outcome);
        true
    }

    /// Appends a discovered (directory/HEIF expansion) result row.
    pub fn push_discovered(&mut self, path: PathBuf, outcome: &Outcome) {
        let mut item = QueueItem::new(path);
        item.discovered = true;
        item.apply_outcome(outcome);
        self.items.push(item);
    }

    /// After a job: rows that never received an event (directories, which
    /// the core expanded instead) return to the queued state with an
    /// informational note.
    pub fn finish_run(&mut self) {
        for item in &mut self.items {
            if item.status == ItemStatus::Running && item.is_dir {
                item.status = ItemStatus::Queued;
                item.note =
                    Some("expanded recursively (results appear as discovered rows)".to_string());
            }
        }
    }
}

/// Canonicalized dedup key: resolves symlinks/`..` segments; falls back to
/// the verbatim path when the file does not exist.
fn canonical_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Binary-unit size formatting matching the CLI's humansize style
/// (two decimals, no space), e.g. `783.00KiB`.
#[must_use]
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes}B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2}{}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome_encoded() -> Outcome {
        Outcome::Encoded {
            input_size: 100,
            output_size: 50,
            metadata_dropped: true,
            output_path: PathBuf::from("/x/a.out"),
        }
    }

    // ---- Outcome → status mapping ---------------------------------------

    #[test]
    fn outcome_maps_onto_row_status_one_to_one() {
        assert_eq!(
            ItemStatus::from_outcome(&outcome_encoded()),
            ItemStatus::Encoded
        );
        assert_eq!(
            ItemStatus::from_outcome(&Outcome::SkippedExisting {
                input_size: 10,
                existing_size: 5,
                output_path: PathBuf::from("/x/a.out"),
            }),
            ItemStatus::SkippedExisting
        );
        assert_eq!(
            ItemStatus::from_outcome(&Outcome::DiscardedLargerThanExisting {
                input_size: 10,
                existing_size: 5
            }),
            ItemStatus::SkippedExisting,
            "overwrite-if-smaller keeps count as a skip, matching the CLI summary"
        );
        assert_eq!(
            ItemStatus::from_outcome(&Outcome::SkippedCollision {
                input_size: 10,
                output_path: PathBuf::from("x")
            }),
            ItemStatus::SkippedCollision
        );
        assert_eq!(
            ItemStatus::from_outcome(&Outcome::DiscardedLargerThanInput {
                input_size: 10,
                encoded_size: 12
            }),
            ItemStatus::DiscardedLargerThanInput
        );
        assert_eq!(
            ItemStatus::from_outcome(&Outcome::Error("boom".to_string())),
            ItemStatus::Error
        );
        assert_eq!(
            ItemStatus::from_outcome(&Outcome::Aborted),
            ItemStatus::Aborted
        );
    }

    #[test]
    fn encoded_outcome_fills_row_sizes_and_error_clears_them() {
        let mut item = QueueItem::new(PathBuf::from("/definitely/not/here.png"));
        item.apply_outcome(&outcome_encoded());
        assert_eq!(item.status, ItemStatus::Encoded);
        assert_eq!(item.input_size, Some(100));
        assert_eq!(item.output_size, Some(50));
        assert!(item.metadata_dropped);

        item.apply_outcome(&Outcome::Error("nope".to_string()));
        assert_eq!(item.status, ItemStatus::Error);
        assert_eq!(item.error.as_deref(), Some("nope"));
        assert!(!item.metadata_dropped);
    }

    // ---- Glyph truthfulness (plan 12 §1) ------------------------------------

    #[test]
    fn running_glyph_distinguishes_active_from_pending_rows() {
        // actively worked rows keep the spinner…
        assert_eq!(ItemStatus::Running.glyph_while_running(true), "⟳");
        // …rows still waiting in this run show the pending dots
        assert_eq!(ItemStatus::Running.glyph_while_running(false), "…");
        // non-running statuses delegate unchanged
        assert_eq!(ItemStatus::Queued.glyph_while_running(false), "…");
        assert_eq!(ItemStatus::Queued.glyph_while_running(true), "…");
        assert_eq!(ItemStatus::Encoded.glyph_while_running(true), "✔");
        assert_eq!(ItemStatus::Aborted.glyph_while_running(false), "⏸");
    }

    // ---- Add / dedup / remove / clear ------------------------------------

    #[test]
    fn add_is_idempotent_per_canonical_path() {
        let mut queue = Queue::new();
        let tmp = std::env::temp_dir().join(format!("byteshaver-gui-queue-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let file = tmp.join("a.png");
        std::fs::write(&file, [0]).expect("write temp file");

        assert_eq!(queue.add_paths(vec![file.clone()]), 1);
        // the same path again
        assert_eq!(queue.add_paths(vec![file.clone()]), 0);
        // a different spelling of the same canonical path
        let dotted = tmp.join(".").join("a.png");
        assert_eq!(queue.add_paths(vec![dotted]), 0);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.items()[0].path, file);

        // a directory is added as-is (core expands it)
        assert_eq!(queue.add_paths(vec![tmp.clone()]), 1);
        assert!(queue.items()[1].is_dir);
        assert_eq!(queue.items()[1].source_format, ImageFormat::Unknown);

        // cleanup (best effort)
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn remove_and_clear_work() {
        let mut queue = Queue::new();
        queue.add_paths(vec![PathBuf::from("/x/a.png"), PathBuf::from("/x/b.png")]);
        queue.remove(0);
        assert_eq!(queue.items()[0].path, PathBuf::from("/x/b.png"));
        queue.remove(9);
        assert_eq!(queue.len(), 1);
        queue.clear();
        assert!(queue.is_empty());
    }

    // ---- Run-state transitions -------------------------------------------

    #[test]
    fn run_lifecycle_marks_rows_and_handles_discovered_results() {
        // a real directory so the queue marks it is_dir (the core expands it)
        let dir = std::env::temp_dir().join(format!("byteshaver-gui-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let file = dir.join("a.png");

        let mut queue = Queue::new();
        queue.add_paths(vec![dir.clone(), file.clone()]);
        assert!(queue.items()[0].is_dir);
        queue.begin_run();
        assert!(
            queue
                .items()
                .iter()
                .all(|item| item.status == ItemStatus::Running)
        );

        // a queue row finishes directly
        assert!(queue.apply_file_finished(&file, &outcome_encoded()));
        assert_eq!(queue.items()[1].status, ItemStatus::Encoded);

        // a file discovered under the directory has no row yet
        let inner = dir.join("sub/b.jpg");
        assert!(!queue.apply_file_finished(&inner, &Outcome::Aborted));
        queue.push_discovered(inner.clone(), &Outcome::Aborted);
        assert!(queue.items()[2].discovered);
        assert_eq!(queue.items()[2].status, ItemStatus::Aborted);

        // finishing the run reverts the untouched directory row and notes
        // the expansion; the next run drops discovered rows again
        queue.finish_run();
        assert_eq!(queue.items()[0].status, ItemStatus::Queued);
        assert!(queue.items()[0].note.is_some());
        assert_eq!(queue.items()[1].status, ItemStatus::Encoded);

        queue.begin_run();
        assert_eq!(queue.len(), 2, "discovered rows are dropped");
        assert_eq!(queue.items()[1].status, ItemStatus::Running);

        let _ = std::fs::remove_dir(&dir);
    }

    // ---- Support flags ----------------------------------------------------

    #[test]
    fn unsupported_extensions_are_flagged_but_still_enqueued() {
        let item = QueueItem::new(PathBuf::from("/x/readme.txt"));
        assert!(!item.is_supported(true));
        assert_eq!(
            item.unsupported_reason(true).as_deref(),
            Some("unsupported input extension; the run reports a per-file error")
        );

        let heic = QueueItem::new(PathBuf::from("/x/photo.heic"));
        assert_eq!(heic.source_format, ImageFormat::Heif);
        assert!(heic.is_supported(true));
        assert!(heic.unsupported_reason(true).is_none());
        assert!(!heic.is_supported(false));
        assert!(
            heic.unsupported_reason(false)
                .is_some_and(|reason| reason.contains("dec-heif"))
        );

        // a real directory is always passed on (core expands it)
        let dir_path =
            std::env::temp_dir().join(format!("byteshaver-gui-supp-{}", std::process::id()));
        std::fs::create_dir_all(&dir_path).expect("create temp dir");
        let dir = QueueItem::new(dir_path.clone());
        assert!(dir.is_dir, "test requires a real directory");
        assert!(dir.is_supported(false), "directories are always passed on");
        let _ = std::fs::remove_dir(&dir_path);
    }

    // ---- Directory content scan -------------------------------------------

    #[test]
    fn dir_summary_counts_images_sizes_and_formats() {
        let base = std::env::temp_dir().join(format!("byteshaver-gui-scan-{}", std::process::id()));
        let nested = base.join("nested");
        std::fs::create_dir_all(&nested).expect("create fixture dirs");
        std::fs::write(base.join("a.png"), [0u8; 10]).expect("write png");
        std::fs::write(base.join("b.jpg"), [0u8; 20]).expect("write jpg");
        std::fs::write(base.join("c.jpg"), [0u8; 30]).expect("write jpg 2");
        std::fs::write(base.join("ignored.txt"), [0u8; 999]).expect("write txt");
        std::fs::write(nested.join("d.webp"), [0u8; 40]).expect("write webp");
        // a symlink must not count (or loop)
        #[cfg(unix)]
        std::os::unix::fs::symlink(&base, base.join("loop")).expect("create symlink");

        let item = QueueItem::new(base.clone());
        assert!(item.is_dir);
        let summary = item.summary.expect("dir rows carry a summary");
        assert_eq!(summary.images, 4, "png + 2 jpg + webp, no txt/symlink");
        assert_eq!(summary.bytes, 100);
        assert_eq!(
            summary.by_format,
            vec![
                ("jpeg".to_string(), 2),
                ("png".to_string(), 1),
                ("webp".to_string(), 1),
            ],
            "sorted by count desc, ties by extension"
        );
        assert_eq!(summary.breakdown(2), "JPEG 2 · PNG 1 · + 1 more");
        assert_eq!(summary.breakdown(10), "JPEG 2 · PNG 1 · WEBP 1");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn empty_and_missing_directories_yield_empty_summaries() {
        assert_eq!(
            DirSummary::scan(Path::new("/definitely/not/here")),
            DirSummary::default()
        );

        let dir =
            std::env::temp_dir().join(format!("byteshaver-gui-scan-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        assert_eq!(DirSummary::scan(&dir), DirSummary::default());
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn file_rows_have_no_summary() {
        let item = QueueItem::new(PathBuf::from("/definitely/not/here.png"));
        assert!(!item.is_dir);
        assert_eq!(item.summary, None);
    }

    #[test]
    fn breakdown_formatting_sorts_and_truncates() {
        let counts = vec![
            ("webp".to_string(), 5),
            ("jpeg".to_string(), 10),
            ("png".to_string(), 7),
        ];
        // format_breakdown takes pre-sorted input; sorting is sort_counts' job
        assert_eq!(format_breakdown(&counts, 2), "WEBP 5 · JPEG 10 · + 1 more");
        let sorted = sort_counts(counts.into_iter().collect());
        assert_eq!(
            sorted,
            vec![
                ("jpeg".to_string(), 10),
                ("png".to_string(), 7),
                ("webp".to_string(), 5),
            ]
        );
    }

    #[test]
    fn size_format_matches_cli_style() {
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(1024 * 783 / 2), "391.50KiB");
        assert_eq!(format_size(2 * 1024 * 1024 + 5), "2.00MiB");
    }
}
