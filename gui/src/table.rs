//! Pure column model of the queue file table (plan 11 §1): the column
//! set/order, the sort state and the **view-only** row permutation.
//!
//! No egui types live here — everything is plain data so the sort matrix
//! and serde persistence stay unit-testable headless. The rendering glue
//! is `panels/file_table.rs`.
//!
//! Sorting never reorders the queue itself: [`Queue::selection`]
//! (crate::queue::Queue::selection) keeps enqueue order and events address
//! rows by queue index; the table renders through the [`sort_indices`]
//! permutation ("sorting does not change conversion order").

use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::queue::{ItemStatus, QueueItem};

/// Every table column, in the order offered by the column chooser.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Column {
    /// Status glyph + label (+ note/error).
    Status,
    /// File name (full path in a tooltip).
    Name,
    /// Detected source format (extension sniff; `dir` for folders).
    SourceFormat,
    /// Merged `in → out` sizes (the CLI-report shape).
    MergedSize,
    /// Input size alone (offered for sorting by it).
    InputSize,
    /// Output size alone.
    OutputSize,
    /// `output / input` as percent with a ratio-band tint.
    Ratio,
    /// Target format of the last run this row entered (`webp`, `avif`, …).
    TargetFormat,
    /// Input file modification time (UTC).
    Modified,
    /// Pixel dimensions of the input (W×H).
    Dimensions,
    /// EXIF `Make` + `Model`.
    ExifCamera,
    /// EXIF `DateTimeOriginal`.
    ExifTaken,
    /// EXIF `ISOSpeed`.
    ExifIso,
    /// EXIF `ExposureTime` (`1/125` style).
    ExifExposure,
    /// Hover-revealed row actions (open output / show in folder / remove).
    Actions,
}

impl Column {
    /// Every column in declaration (chooser) order.
    pub const ALL: [Column; 15] = [
        Column::Status,
        Column::Name,
        Column::SourceFormat,
        Column::MergedSize,
        Column::InputSize,
        Column::OutputSize,
        Column::Ratio,
        Column::TargetFormat,
        Column::Modified,
        Column::Dimensions,
        Column::ExifCamera,
        Column::ExifTaken,
        Column::ExifIso,
        Column::ExifExposure,
        Column::Actions,
    ];

    /// Header label of the column.
    #[must_use]
    pub fn header(self) -> &'static str {
        match self {
            Column::Status => "status",
            Column::Name => "name",
            Column::SourceFormat => "format",
            Column::MergedSize => "size (in → out)",
            Column::InputSize => "input",
            Column::OutputSize => "output",
            Column::Ratio => "ratio",
            Column::TargetFormat => "target",
            Column::Modified => "modified",
            Column::Dimensions => "dimensions",
            Column::ExifCamera => "camera",
            Column::ExifTaken => "taken",
            Column::ExifIso => "iso",
            Column::ExifExposure => "exposure",
            Column::Actions => "Actions",
        }
    }

    /// Whether clicking the header sorts by this column.
    #[must_use]
    pub fn sortable(self) -> bool {
        !matches!(self, Column::Actions)
    }

    /// Whether the column is an EXIF/metadata field (chooser grouping).
    #[must_use]
    pub fn is_metadata(self) -> bool {
        matches!(
            self,
            Column::ExifCamera | Column::ExifTaken | Column::ExifIso | Column::ExifExposure
        )
    }
}

/// The visible columns in display order (persisted in the settings).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnState {
    /// Visible columns; order = display order.
    pub visible: Vec<Column>,
}

/// The pre-plan-15 default column order (F7 migration reference): exactly
/// what plan 11 §1 shipped.
pub const LEGACY_DEFAULT_COLUMNS: [Column; 8] = [
    Column::Status,
    Column::Name,
    Column::SourceFormat,
    Column::MergedSize,
    Column::Ratio,
    Column::TargetFormat,
    Column::Modified,
    Column::Actions,
];

/// The plan-15 default column order (F7): `Dimensions` moves up beside
/// the name so the pixel size reads with the file.
pub const DEFAULT_COLUMNS: [Column; 9] = [
    Column::Status,
    Column::Name,
    Column::Dimensions,
    Column::SourceFormat,
    Column::MergedSize,
    Column::Ratio,
    Column::TargetFormat,
    Column::Modified,
    Column::Actions,
];

impl Default for ColumnState {
    fn default() -> Self {
        // plan 15 F7: Dimensions joins the default set right after Name
        ColumnState {
            visible: DEFAULT_COLUMNS.to_vec(),
        }
    }
}

impl ColumnState {
    /// Whether `column` is currently visible.
    #[must_use]
    pub fn is_visible(&self, column: Column) -> bool {
        self.visible.contains(&column)
    }

    /// Toggles `column`: hidden columns are appended at the end of the
    /// display order (plan 11 §2 chooser semantics).
    pub fn toggle(&mut self, column: Column) {
        if let Some(position) = self.visible.iter().position(|&visible| visible == column) {
            self.visible.remove(position);
        } else {
            self.visible.push(column);
        }
    }
}

/// F7 migration of a persisted `table_columns` order: an order that is
/// exactly the **pre-plan-15 default** ([`LEGACY_DEFAULT_COLUMNS`]) is
/// silently upgraded to the new default; any other stored order (user
/// customization, hidden columns) is respected — `None` = keep as is.
#[must_use]
pub fn migrate_legacy_columns(visible: &[Column]) -> Option<Vec<Column>> {
    (visible == LEGACY_DEFAULT_COLUMNS.as_slice()).then(|| DEFAULT_COLUMNS.to_vec())
}

/// Moves one visible column (F8 header drag): `from` and `to` are
/// positions in the given order, with `to` the **insertion position**
/// (`to == len` appends at the end, `to == target` inserts before the
/// target column, `to == target + 1` after it). No-ops (self-drop, equal
/// indices, out-of-range `from`) return the order unchanged; `to` beyond
/// the end clamps to the end.
#[must_use]
pub fn reorder(mut visible: Vec<Column>, from: usize, to: usize) -> Vec<Column> {
    if from >= visible.len() || from == to {
        return visible;
    }
    let column = visible.remove(from);
    let insert_at = if from < to { to - 1 } else { to };
    visible.insert(insert_at.min(visible.len()), column);
    visible
}

/// Current sort of the table (view-only; persisted in the settings).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    /// Queue order.
    #[default]
    None,
    /// Sorted ascending by the column.
    Asc(Column),
    /// Sorted descending by the column.
    Desc(Column),
}

impl SortKey {
    /// Cycles the sort when `column`'s header is clicked:
    /// `None → Asc → Desc → None` (a different column starts fresh at
    /// `Asc`). Non-sortable columns reset to [`SortKey::None`].
    #[must_use]
    pub fn cycle(self, column: Column) -> Self {
        if !column.sortable() {
            return SortKey::None;
        }
        match self {
            SortKey::Asc(sorted) if sorted == column => SortKey::Desc(column),
            SortKey::Desc(sorted) if sorted == column => SortKey::None,
            _ => SortKey::Asc(column),
        }
    }

    /// The sorted column, if any.
    #[must_use]
    pub fn column(self) -> Option<Column> {
        match self {
            SortKey::None => None,
            SortKey::Asc(column) | SortKey::Desc(column) => Some(column),
        }
    }
}

/// Header arrow glyph for `column` (F11): `▲`/`▼` only when the active
/// sort is **exactly this column** — every other header renders without
/// an indicator (the old code showed the arrow on all columns).
#[must_use]
pub fn sort_indicator(sort: SortKey, column: Column) -> Option<&'static str> {
    match sort {
        SortKey::Asc(sorted) if sorted == column => Some("▲"),
        SortKey::Desc(sorted) if sorted == column => Some("▼"),
        _ => None,
    }
}

/// The view permutation: `order[view_row] = queue_index`.
///
/// `base_order` seeds the sort (the queue's own indices in display-first
/// position); ties fall back to that base order (stable sort). Columns
/// with missing values sort to the end.
#[must_use]
pub fn sort_indices(items: &[QueueItem], sort: SortKey, base_order: &[usize]) -> Vec<usize> {
    let Some(column) = sort.column() else {
        return base_order.to_vec();
    };
    let descending = matches!(sort, SortKey::Desc(_));
    let mut order = base_order.to_vec();
    order.sort_by(|&a, &b| {
        let ordering = compare(&items[a], &items[b], column);
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    order
}

/// Column comparison of two rows (missing values last, ties equal so the
/// stable sort keeps queue order).
fn compare(a: &QueueItem, b: &QueueItem, column: Column) -> Ordering {
    match column {
        Column::Name => name_of(a)
            .cmp(&name_of(b))
            .then_with(|| a.path.cmp(&b.path)),
        Column::SourceFormat => source_format_text(a).cmp(&source_format_text(b)),
        Column::MergedSize | Column::InputSize => cmp_option(a.input_size, b.input_size, u64::cmp),
        Column::OutputSize => cmp_option(a.output_size, b.output_size, u64::cmp),
        Column::Ratio => cmp_option(ratio_of(a), ratio_of(b), f32::total_cmp),
        Column::TargetFormat => cmp_option(a.converted_to, b.converted_to, |a, b| a.cmp(b)),
        Column::Modified => cmp_option(a.modified, b.modified, SystemTime::cmp),
        Column::Dimensions => cmp_option(pixels(a.dimensions), pixels(b.dimensions), u64::cmp),
        Column::Status => status_rank(a.status).cmp(&status_rank(b.status)),
        Column::ExifCamera => cmp_option(
            exif_text(a, |summary| &summary.camera),
            exif_text(b, |summary| &summary.camera),
            |a, b| a.cmp(b),
        ),
        Column::ExifTaken => cmp_option(
            exif_text(a, |summary| &summary.date_time_original),
            exif_text(b, |summary| &summary.date_time_original),
            |a, b| a.cmp(b),
        ),
        Column::ExifIso => cmp_option(exif_number(a), exif_number(b), u32::cmp),
        Column::ExifExposure => cmp_option(
            exposure_seconds_of(a),
            exposure_seconds_of(b),
            f64::total_cmp,
        ),
        Column::Actions => Ordering::Equal,
    }
}

/// `Option`-aware comparison: `None` sorts last (both directions are the
/// caller's reversal, so descending puts `None` first — standard table
/// behavior).
fn cmp_option<T, F>(a: Option<T>, b: Option<T>, compare: F) -> Ordering
where
    F: FnOnce(&T, &T) -> Ordering,
{
    match (a, b) {
        (Some(a), Some(b)) => compare(&a, &b),
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
    }
}

/// Case-insensitive file name (whole path as the tiebreaker in [`compare`]).
fn name_of(item: &QueueItem) -> String {
    item.path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| item.path.display().to_string().to_lowercase())
}

/// Source-format cell text (also the sort key).
fn source_format_text(item: &QueueItem) -> String {
    if item.is_dir {
        "dir".to_string()
    } else {
        item.source_format.extension().to_string()
    }
}

/// Status grouping order: in-flight first, then queued, results, and
/// errors/aborts last. All statuses share the column so rows of one group
/// tie (queue order within).
fn status_rank(status: ItemStatus) -> u8 {
    match status {
        ItemStatus::Running => 0,
        ItemStatus::Queued => 1,
        ItemStatus::Encoded => 2,
        ItemStatus::SkippedExisting | ItemStatus::SkippedCollision => 3,
        ItemStatus::DiscardedLargerThanInput => 4,
        ItemStatus::Aborted => 5,
        ItemStatus::Error => 6,
    }
}

/// `output / input` of a finished row (`None` when not computable).
fn ratio_of(item: &QueueItem) -> Option<f32> {
    let input = item.input_size?;
    let output = item.output_size?;
    if input == 0 {
        return None;
    }
    Some(output as f32 / input as f32)
}

/// Total pixel count (`None` for the unknown/failed `(0, 0)` sentinel).
fn pixels(dimensions: Option<(u32, u32)>) -> Option<u64> {
    let (width, height) = dimensions?;
    if width == 0 || height == 0 {
        return None;
    }
    Some(u64::from(width) * u64::from(height))
}

/// Reads an `Option<String>` field through the row's EXIF cache.
fn exif_text(
    item: &QueueItem,
    field: impl FnOnce(&byteshaver::metadata::exif::ExifSummary) -> &Option<String>,
) -> Option<String> {
    item.exif
        .as_ref()
        .and_then(|summary| summary.as_ref())
        .and_then(|summary| field(summary).clone())
}

/// ISO value through the row's EXIF cache.
fn exif_number(item: &QueueItem) -> Option<u32> {
    item.exif
        .as_ref()
        .and_then(|summary| summary.as_ref())
        .and_then(|summary| summary.iso)
}

/// Exposure time in seconds through the row's EXIF cache.
fn exposure_seconds_of(item: &QueueItem) -> Option<f64> {
    item.exif
        .as_ref()
        .and_then(|summary| summary.as_ref())
        .and_then(|summary| summary.exposure.as_deref())
        .and_then(exposure_seconds)
}

/// Parses a [`byteshaver::metadata::exif::ExifSummary::exposure`] text
/// (`1/125`, `1/2`, `2s`, `0`, `3/10`) into seconds for numeric sorting.
#[must_use]
pub fn exposure_seconds(text: &str) -> Option<f64> {
    if let Some((numerator, denominator)) = text.split_once('/') {
        let numerator: f64 = numerator.trim().parse().ok()?;
        let denominator: f64 = denominator.trim().parse().ok()?;
        if denominator == 0.0 {
            return None;
        }
        return Some(numerator / denominator);
    }
    let text = text.strip_suffix('s').unwrap_or(text);
    text.trim().parse().ok()
}

/// Formats a `SystemTime` as a `YYYY-MM-DD HH:MM` civil timestamp (UTC —
/// std has no portable local-time conversion; the column tooltip says so).
#[must_use]
pub fn format_system_time(time: SystemTime) -> String {
    let seconds = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    };
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60
    )
}

/// Days-since-epoch → civil `(year, month, day)` (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Formats an EXIF `DateTimeOriginal` (F6): the raw camera form
/// `2020:01:31 14:22:05` renders as `2020.01.31 14:22:05` (dots between
/// the date fields, colons in the time). Values without the EXIF colon
/// date pass through unchanged.
#[must_use]
pub fn format_exif_taken(raw: &str) -> String {
    let bytes = raw.as_bytes();
    if bytes.len() >= 10 && bytes[4] == b':' && bytes[7] == b':' {
        let (date, rest) = raw.split_at(10);
        let date = date.replace(':', ".");
        let rest = rest.trim();
        if rest.is_empty() {
            date
        } else {
            format!("{date} {rest}")
        }
    } else {
        raw.to_owned()
    }
}

/// Visible per-row quality-metric state (F19 display half): rendered in
/// the status cell instead of being tooltip-only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetricStatus {
    /// A measurement is in flight.
    Measuring,
    /// A finished reading (the formatted `engine score · reading` line).
    Reading(String),
    /// The measurement failed — the row shows the message in red.
    Error(String),
}

impl MetricStatus {
    /// The line rendered into the status cell.
    #[must_use]
    pub fn line(&self) -> &str {
        match self {
            MetricStatus::Measuring => "…measuring",
            MetricStatus::Reading(line) | MetricStatus::Error(line) => line,
        }
    }

    /// Whether the state is a failure (rendered in the error color).
    #[must_use]
    pub fn is_error(&self) -> bool {
        matches!(self, MetricStatus::Error(_))
    }
}

/// Assembles the visible metric state (F19): an error (when the metric
/// state reports one) wins over the pending marker, which wins over a
/// finished reading; with no pending request and no reading there is no
/// state.
#[must_use]
pub fn metric_status(
    pending: bool,
    reading: Option<&str>,
    error: Option<&str>,
) -> Option<MetricStatus> {
    if let Some(message) = error {
        return Some(MetricStatus::Error(message.to_owned()));
    }
    if pending {
        return Some(MetricStatus::Measuring);
    }
    reading.map(|line| MetricStatus::Reading(line.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A minimal in-memory row (bypasses the stat/format sniff of
    /// [`QueueItem::new`]).
    fn item(path: &str, patch: impl FnOnce(&mut QueueItem)) -> QueueItem {
        let mut item = QueueItem::new(PathBuf::from(path));
        patch(&mut item);
        item
    }

    fn order_of(indices: &[usize], items: &[QueueItem]) -> Vec<String> {
        indices
            .iter()
            .map(|&index| items[index].path.display().to_string())
            .collect()
    }

    fn base(len: usize) -> Vec<usize> {
        (0..len).collect()
    }

    const T0: SystemTime = UNIX_EPOCH;
    const T1: SystemTime = UNIX_EPOCH;
    // (T0/T1 equal on purpose: equal keys must tie to queue order)

    // ---- every column × direction ----------------------------------------

    #[test]
    fn size_columns_sort_numerically_with_missing_last() {
        let items = vec![
            item("/x/big.png", |row| {
                row.input_size = Some(300);
                row.output_size = Some(100);
            }),
            item("/x/small.png", |row| {
                row.input_size = Some(100);
            }),
            item("/x/unknown.png", |_| {}),
        ];
        let asc = sort_indices(&items, SortKey::Asc(Column::InputSize), &base(3));
        assert_eq!(
            order_of(&asc, &items),
            vec!["/x/small.png", "/x/big.png", "/x/unknown.png"]
        );
        let desc = sort_indices(&items, SortKey::Desc(Column::InputSize), &base(3));
        assert_eq!(
            order_of(&desc, &items),
            // descending reverses everything, missing values first
            // (standard table behavior, see cmp_option)
            vec!["/x/unknown.png", "/x/big.png", "/x/small.png"]
        );

        let out_asc = sort_indices(&items, SortKey::Asc(Column::OutputSize), &base(3));
        assert_eq!(
            order_of(&out_asc, &items),
            vec!["/x/big.png", "/x/small.png", "/x/unknown.png"]
        );

        // the merged column sorts by input size
        let merged = sort_indices(&items, SortKey::Asc(Column::MergedSize), &base(3));
        assert_eq!(
            order_of(&merged, &items),
            vec!["/x/small.png", "/x/big.png", "/x/unknown.png"]
        );
    }

    #[test]
    fn ratio_sorts_numerically() {
        let items = vec![
            item("/x/half.png", |row| {
                row.input_size = Some(100);
                row.output_size = Some(50);
            }),
            item("/x/grew.png", |row| {
                row.input_size = Some(100);
                row.output_size = Some(120);
            }),
            item("/x/none.png", |_| {}),
        ];
        let asc = sort_indices(&items, SortKey::Asc(Column::Ratio), &base(3));
        assert_eq!(
            order_of(&asc, &items),
            vec!["/x/half.png", "/x/grew.png", "/x/none.png"]
        );
    }

    #[test]
    fn name_sort_is_case_insensitive_lexicographic() {
        let items = vec![
            item("/x/banana.PNG", |_| {}),
            item("/x/Apple.png", |_| {}),
            item("/x/cherry.png", |_| {}),
        ];
        let asc = sort_indices(&items, SortKey::Asc(Column::Name), &base(3));
        assert_eq!(
            order_of(&asc, &items),
            vec!["/x/Apple.png", "/x/banana.PNG", "/x/cherry.png"]
        );
        let desc = sort_indices(&items, SortKey::Desc(Column::Name), &base(3));
        assert_eq!(
            order_of(&desc, &items),
            vec!["/x/cherry.png", "/x/banana.PNG", "/x/Apple.png"]
        );
    }

    #[test]
    fn modified_sorts_by_date() {
        let epoch = UNIX_EPOCH;
        let later = epoch + std::time::Duration::from_secs(86_400 * 10);
        let items = vec![
            item("/x/old.png", |row| row.modified = Some(epoch)),
            item("/x/new.png", |row| row.modified = Some(later)),
            item("/x/none.png", |_| {}),
        ];
        let asc = sort_indices(&items, SortKey::Asc(Column::Modified), &base(3));
        assert_eq!(
            order_of(&asc, &items),
            vec!["/x/old.png", "/x/new.png", "/x/none.png"]
        );
    }

    #[test]
    fn status_sort_groups_rows_and_ties_to_queue_order() {
        let items = vec![
            item("/x/encoded.png", |row| row.status = ItemStatus::Encoded),
            item("/x/error.png", |row| row.status = ItemStatus::Error),
            item("/x/queued.png", |_| {}),
            item("/x/encoded2.png", |row| row.status = ItemStatus::Encoded),
        ];
        let asc = sort_indices(&items, SortKey::Asc(Column::Status), &base(4));
        assert_eq!(
            order_of(&asc, &items),
            vec![
                "/x/queued.png",
                "/x/encoded.png",
                "/x/encoded2.png",
                "/x/error.png"
            ],
            "queued group first, encoded group ties keep queue order"
        );
    }

    #[test]
    fn dimensions_sorts_by_pixel_count_and_ignores_the_failed_sentinel() {
        let items = vec![
            item("/x/small.png", |row| row.dimensions = Some((10, 10))),
            item("/x/big.png", |row| row.dimensions = Some((4000, 3000))),
            item("/x/failed.png", |row| row.dimensions = Some((0, 0))),
        ];
        let asc = sort_indices(&items, SortKey::Asc(Column::Dimensions), &base(3));
        assert_eq!(
            order_of(&asc, &items),
            vec!["/x/small.png", "/x/big.png", "/x/failed.png"]
        );
    }

    #[test]
    fn target_format_sorts_by_extension() {
        let items = vec![
            item("/x/a.png", |row| row.converted_to = Some("webp")),
            item("/x/b.png", |row| row.converted_to = Some("avif")),
            item("/x/c.png", |_| {}),
        ];
        let asc = sort_indices(&items, SortKey::Asc(Column::TargetFormat), &base(3));
        assert_eq!(
            order_of(&asc, &items),
            vec!["/x/b.png", "/x/a.png", "/x/c.png"]
        );
    }

    #[test]
    fn exif_columns_sort_with_missing_cache_last() {
        let summary = |iso: u32, taken: &str, exposure: &str| {
            Some(byteshaver::metadata::exif::ExifSummary {
                camera: Some("TestCam".to_string()),
                date_time_original: Some(taken.to_string()),
                iso: Some(iso),
                exposure: Some(exposure.to_string()),
            })
        };
        let items = vec![
            item("/x/low.png", |row| {
                row.exif = Some(summary(100, "2020:01:01 00:00:00", "1/500"))
            }),
            item("/x/high.png", |row| {
                row.exif = Some(summary(1600, "2024:01:01 00:00:00", "1/60"))
            }),
            item("/x/noexif.png", |row| row.exif = Some(None)),
        ];
        let iso = sort_indices(&items, SortKey::Asc(Column::ExifIso), &base(3));
        assert_eq!(
            order_of(&iso, &items),
            vec!["/x/low.png", "/x/high.png", "/x/noexif.png"]
        );
        let taken = sort_indices(&items, SortKey::Asc(Column::ExifTaken), &base(3));
        assert_eq!(
            order_of(&taken, &items),
            vec!["/x/low.png", "/x/high.png", "/x/noexif.png"]
        );
        // exposure is numeric: 1/500 < 1/60
        let exposure = sort_indices(&items, SortKey::Asc(Column::ExifExposure), &base(3));
        assert_eq!(
            order_of(&exposure, &items),
            vec!["/x/low.png", "/x/high.png", "/x/noexif.png"]
        );
        let camera = sort_indices(&items, SortKey::Asc(Column::ExifCamera), &base(3));
        assert_eq!(
            order_of(&camera, &items),
            vec!["/x/low.png", "/x/high.png", "/x/noexif.png"]
        );
    }

    // ---- stability & base order -------------------------------------------

    #[test]
    fn ties_fall_back_to_base_order_and_none_is_identity() {
        let items = vec![
            item("/x/a.png", |_| {}),
            item("/x/b.png", |_| {}),
            item("/x/c.png", |_| {}),
        ];
        // equal keys (T0 == T1) → base order preserved
        let equal_items = vec![
            item("/x/a.png", |row| row.modified = Some(T0)),
            item("/x/b.png", |row| row.modified = Some(T1)),
        ];
        let order = sort_indices(&equal_items, SortKey::Asc(Column::Modified), &base(2));
        assert_eq!(order, vec![0, 1], "stable sort keeps the base order");

        // base_order seeds/preserves the permutation (view-only sorting)
        assert_eq!(sort_indices(&items, SortKey::None, &base(3)), vec![0, 1, 2]);
        assert_eq!(
            sort_indices(&items, SortKey::None, &[2, 0, 1]),
            vec![2, 0, 1]
        );
        // the queue itself is untouched by any sort
        assert_eq!(items[0].path, PathBuf::from("/x/a.png"));
    }

    #[test]
    fn actions_column_is_not_sortable() {
        assert!(!Column::Actions.sortable());
        assert_eq!(
            SortKey::Asc(Column::Name).cycle(Column::Actions),
            SortKey::None
        );
        assert_eq!(SortKey::None.cycle(Column::Actions), SortKey::None);
    }

    // ---- sort-key cycling --------------------------------------------------

    #[test]
    fn header_clicks_cycle_none_asc_desc_none() {
        let mut sort = SortKey::None;
        sort = sort.cycle(Column::Name);
        assert_eq!(sort, SortKey::Asc(Column::Name));
        assert_eq!(sort_indicator(sort, Column::Name), Some("▲"));
        sort = sort.cycle(Column::Name);
        assert_eq!(sort, SortKey::Desc(Column::Name));
        assert_eq!(sort_indicator(sort, Column::Name), Some("▼"));
        sort = sort.cycle(Column::Name);
        assert_eq!(sort, SortKey::None);
        // switching columns starts fresh at ascending
        let sort = SortKey::Desc(Column::Ratio);
        assert_eq!(sort.cycle(Column::Name), SortKey::Asc(Column::Name));
        assert_eq!(sort.column(), Some(Column::Ratio));
    }

    // ---- sort indicator (F11) ----------------------------------------------

    #[test]
    fn sort_indicator_shows_only_on_the_active_column() {
        assert_eq!(sort_indicator(SortKey::None, Column::Name), None);
        // the old bug: the arrow showed on every column
        assert_eq!(
            sort_indicator(SortKey::Desc(Column::Ratio), Column::Name),
            None,
            "no arrow on a column the sort is not keyed to"
        );
        assert_eq!(
            sort_indicator(SortKey::Asc(Column::Ratio), Column::Ratio),
            Some("▲")
        );
        assert_eq!(
            sort_indicator(SortKey::Desc(Column::Ratio), Column::Ratio),
            Some("▼")
        );
        // a non-sortable column never carries the arrow
        assert_eq!(
            sort_indicator(SortKey::Asc(Column::Name), Column::Actions),
            None
        );
    }

    // ---- column state --------------------------------------------------------

    #[test]
    fn column_state_toggles_and_defaults_match_the_plan() {
        let mut state = ColumnState::default();
        assert_eq!(
            state.visible,
            vec![
                Column::Status,
                Column::Name,
                Column::Dimensions,
                Column::SourceFormat,
                Column::MergedSize,
                Column::Ratio,
                Column::TargetFormat,
                Column::Modified,
                Column::Actions,
            ],
            "plan 15 F7 default order"
        );
        assert!(state.is_visible(Column::Name));
        // hide appends nothing, show appends at the end
        state.toggle(Column::Name);
        assert!(!state.is_visible(Column::Name));
        state.toggle(Column::ExifIso);
        assert_eq!(*state.visible.last().expect("appended"), Column::ExifIso);
        state.toggle(Column::Name);
        assert_eq!(*state.visible.last().expect("re-appended"), Column::Name);
    }

    // ---- legacy-default migration (F7) ---------------------------------------

    #[test]
    fn legacy_default_column_order_upgrades_to_the_new_default() {
        // exactly the old default → silently upgraded
        assert_eq!(
            migrate_legacy_columns(&LEGACY_DEFAULT_COLUMNS),
            Some(DEFAULT_COLUMNS.to_vec())
        );
        // the new default itself is respected (already migrated)
        assert_eq!(migrate_legacy_columns(&DEFAULT_COLUMNS), None);
        // any custom order is respected
        assert_eq!(
            migrate_legacy_columns(&[Column::Name, Column::Status, Column::Actions]),
            None
        );
        // a subset of the old default is a user choice (hidden columns), not
        // the default — respected
        assert_eq!(migrate_legacy_columns(&LEGACY_DEFAULT_COLUMNS[..4]), None);
    }

    // ---- header drag reorder (F8) --------------------------------------------

    #[test]
    fn reorder_moves_a_column_to_the_insertion_position() {
        let order = |from, to| reorder(vec![Column::Status, Column::Name, Column::Ratio], from, to);
        // insert-before the target (left half of the header)
        assert_eq!(
            order(0, 2),
            vec![Column::Name, Column::Status, Column::Ratio]
        );
        // insert-after the target (right half), to == len appends at the end
        assert_eq!(
            order(0, 3),
            vec![Column::Name, Column::Ratio, Column::Status]
        );
        // backwards move
        assert_eq!(
            order(2, 0),
            vec![Column::Ratio, Column::Status, Column::Name]
        );
        // self-drops and equal indices are no-ops
        assert_eq!(
            order(1, 1),
            vec![Column::Status, Column::Name, Column::Ratio]
        );
        assert_eq!(
            order(0, 0),
            vec![Column::Status, Column::Name, Column::Ratio]
        );
        // out-of-range from is a no-op, out-of-range to clamps to the end
        assert_eq!(
            order(3, 0),
            vec![Column::Status, Column::Name, Column::Ratio]
        );
        assert_eq!(
            order(0, 99),
            vec![Column::Name, Column::Ratio, Column::Status]
        );
    }

    #[test]
    fn column_model_round_trips_through_serde() {
        let state = ColumnState::default();
        let json = serde_json::to_string(&state).expect("serialize");
        assert_eq!(
            serde_json::from_str::<ColumnState>(&json).expect("deserialize"),
            state
        );
        let sort = SortKey::Desc(Column::Modified);
        let json = serde_json::to_string(&sort).expect("serialize");
        assert_eq!(
            serde_json::from_str::<SortKey>(&json).expect("deserialize"),
            sort
        );
        // metadata grouping flags (chooser sub-header)
        assert!(Column::ExifCamera.is_metadata() && Column::ExifExposure.is_metadata());
        assert!(!Column::Name.is_metadata());
        // every column has a header label and appears exactly once
        let mut seen = Vec::new();
        for column in Column::ALL {
            assert!(!column.header().is_empty(), "{column:?} needs a header");
            assert!(!seen.contains(&column));
            seen.push(column);
        }
    }

    // ---- helpers --------------------------------------------------------------

    #[test]
    fn exposure_seconds_parses_camera_styles() {
        assert_eq!(exposure_seconds("1/125"), Some(0.008));
        assert_eq!(exposure_seconds("1/2"), Some(0.5));
        assert_eq!(exposure_seconds("2s"), Some(2.0));
        assert_eq!(exposure_seconds("2"), Some(2.0));
        assert_eq!(exposure_seconds("0"), Some(0.0));
        assert_eq!(exposure_seconds("3/10"), Some(0.3));
        assert_eq!(exposure_seconds("1/0"), None);
        assert_eq!(exposure_seconds("bogus"), None);
    }

    #[test]
    fn system_time_formats_as_utc_civil_timestamp() {
        assert_eq!(format_system_time(UNIX_EPOCH), "1970-01-01 00:00");
        // 2024-05-01 12:34:56 UTC
        let time = UNIX_EPOCH + std::time::Duration::from_secs(1_714_566_896);
        assert_eq!(format_system_time(time), "2024-05-01 12:34");
        // before the epoch clamps instead of panicking
        let before = UNIX_EPOCH - std::time::Duration::from_secs(86_400 * 5);
        assert_eq!(format_system_time(before), "1969-12-27 00:00");
    }

    // ---- EXIF taken timestamp (F6) -------------------------------------------

    #[test]
    fn exif_taken_renders_as_dotted_civil_timestamp() {
        assert_eq!(
            format_exif_taken("2020:01:31 14:22:05"),
            "2020.01.31 14:22:05"
        );
        assert_eq!(
            format_exif_taken("2024:12:01 07:03:59"),
            "2024.12.01 07:03:59"
        );
        // date-only values keep the dotted form without a trailing space
        assert_eq!(format_exif_taken("2020:01:31"), "2020.01.31");
        // anything not in the EXIF colon-date form passes through unchanged
        assert_eq!(
            format_exif_taken("2020-01-31 14:22:05"),
            "2020-01-31 14:22:05"
        );
        assert_eq!(format_exif_taken(""), "");
    }

    // ---- visible metric state (F19 display half) ------------------------------

    #[test]
    fn metric_state_prefers_error_then_pending_then_reading() {
        assert_eq!(
            metric_status(true, None, None),
            Some(MetricStatus::Measuring)
        );
        assert_eq!(
            metric_status(false, Some("dssim 0.0120 · excellent match"), None),
            Some(MetricStatus::Reading(
                "dssim 0.0120 · excellent match".to_owned()
            ))
        );
        assert_eq!(
            metric_status(false, None, None),
            None,
            "no state without a pending request or a reading"
        );
        // errors shadow everything and render red in the row
        assert_eq!(
            metric_status(
                true,
                Some("dssim 0.0120 · excellent match"),
                Some("could not decode")
            ),
            Some(MetricStatus::Error("could not decode".to_owned()))
        );
        // rendering hooks
        assert_eq!(MetricStatus::Measuring.line(), "…measuring");
        assert_eq!(
            MetricStatus::Reading("psnr 41.2dB".to_owned()).line(),
            "psnr 41.2dB"
        );
        assert!(MetricStatus::Error("boom".to_owned()).is_error());
        assert!(!MetricStatus::Measuring.is_error());
        assert!(!MetricStatus::Reading("x".to_owned()).is_error());
    }
}
