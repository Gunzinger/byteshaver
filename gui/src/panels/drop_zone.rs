//! The persistent drop-zone banner (plan WS8 §3 Concept-1): the whole
//! window is a drop target (handled in `App::update` via
//! `RawInput::dropped_files`/`hovered_files`); this banner is the always
//! visible affordance — highlighted while a drag hovers, with click-to-
//! browse (files) and add-folder buttons using the native `rfd` dialogs.
//!
//! The summary line aggregates the queue *including* dropped folders
//! (via their enqueue-time [`crate::queue::DirSummary`] scan): total
//! detected input size, image count and a per-format breakdown, visible
//! immediately after a drop.

use std::collections::BTreeMap;

use byteshaver::format::ImageFormat;

use crate::app::{App, INPUT_EXTENSIONS};
use crate::queue::{format_breakdown, format_size};

/// Height of the banner in points.
const BANNER_HEIGHT: f32 = 64.0;

/// Number of formats shown in the per-format breakdown before collapsing
/// the rest into `+ N more`.
const BREAKDOWN_LIMIT: usize = 6;

/// Aggregated queue totals across file rows and directory scans.
struct QueueTotals {
    file_count: u64,
    dir_count: u64,
    image_count: u64,
    total_bytes: u64,
    by_format: Vec<(String, u64)>,
}

impl QueueTotals {
    /// Folds the queue: file rows contribute their stat'ed size and
    /// sniffed format, directory rows their [`crate::queue::DirSummary`].
    fn of(app: &App) -> Self {
        let mut totals = QueueTotals {
            file_count: 0,
            dir_count: 0,
            image_count: 0,
            total_bytes: 0,
            by_format: Vec::new(),
        };
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for item in app.queue.items() {
            if item.is_dir {
                totals.dir_count += 1;
                if let Some(summary) = &item.summary {
                    totals.image_count += summary.images;
                    totals.total_bytes += summary.bytes;
                    for (format, count) in &summary.by_format {
                        *counts.entry(format.clone()).or_default() += count;
                    }
                }
            } else {
                totals.file_count += 1;
                if let Some(size) = item.input_size {
                    totals.total_bytes += size;
                }
                if item.source_format != ImageFormat::Unknown {
                    totals.image_count += 1;
                    *counts
                        .entry(item.source_format.extension().to_string())
                        .or_default() += 1;
                }
            }
        }
        totals.by_format = crate::queue::sort_counts(counts);
        totals
    }
}

/// Renders the drop-zone banner at the top of the central panel
/// (inside the shared `CentralPanel`, see [`super::show`]).
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let highlight = app.drag_hovered;
    let totals = QueueTotals::of(app);

    let frame = egui::Frame::new()
        .fill(if highlight {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().faint_bg_color
        })
        .stroke(if highlight {
            ui.visuals().selection.stroke
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke
        })
        .corner_radius(4.0)
        .inner_margin(8);

    frame.show(ui, |ui| {
        ui.set_min_height(BANNER_HEIGHT);
        ui.vertical_centered(|ui| {
            ui.add_space(4.0);
            if highlight {
                ui.heading("⬇ drop to add to the queue");
            } else {
                ui.heading("⬇ drop files or folders anywhere in this window");
            }
            ui.add_space(2.0);
            let summary = if app.queue.is_empty() {
                "…or use the buttons below to browse".to_string()
            } else {
                let mut parts = vec![format!("{} files queued", totals.file_count)];
                if totals.dir_count > 0 {
                    parts.push(format!(
                        "{} folders → {} images",
                        totals.dir_count, totals.image_count
                    ));
                }
                if totals.total_bytes > 0 {
                    parts.push(format_size(totals.total_bytes));
                }
                parts.join(" · ")
            };
            ui.label(egui::RichText::new(summary).weak());
            if !totals.by_format.is_empty() {
                ui.label(
                    egui::RichText::new(format_breakdown(&totals.by_format, BREAKDOWN_LIMIT))
                        .weak()
                        .size(12.0),
                );
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Add files…").clicked() {
                    browse_files(app);
                }
                if ui.button("Add folder…").clicked() {
                    browse_folder(app);
                }
                ui.add_enabled_ui(!app.queue.is_empty() && app.running.is_none(), |ui| {
                    if ui.button("Clear queue").clicked() {
                        app.queue.clear();
                    }
                });
            });
        });
    });
    ui.add_space(4.0);
}

/// Opens the native multi-file picker (extension filter mirrors the core's
/// `ImageFormat::from_extension` table) and enqueues the selection.
fn browse_files(app: &mut App) {
    let dialog = rfd::FileDialog::new().add_filter("Images", INPUT_EXTENSIONS);
    if let Some(paths) = dialog.pick_files() {
        app.queue.add_paths(paths);
    }
}

/// Opens the native folder picker; folders are enqueued as-is (the core
/// expands them recursively with the same supported-format filter).
fn browse_folder(app: &mut App) {
    if let Some(path) = rfd::FileDialog::new().pick_folder() {
        app.queue.add_paths(vec![path]);
    }
}
