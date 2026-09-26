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
            } => {
                self.input_size = Some(*input_size);
                self.output_size = Some(*output_size);
                self.metadata_dropped = *metadata_dropped;
            }
            Outcome::SkippedExisting {
                input_size,
                existing_size,
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
                existing_size: 5
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

    #[test]
    fn size_format_matches_cli_style() {
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(1024 * 783 / 2), "391.50KiB");
        assert_eq!(format_size(2 * 1024 * 1024 + 5), "2.00MiB");
    }
}
