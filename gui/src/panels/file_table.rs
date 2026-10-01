//! Queue table (plan 11): an [`egui_extras::TableBuilder`] with sortable
//! headers (click cycles asc/desc/none, ▲/▼ indicator — plan 15 F11:
//! only on the sorted column), a persisted column chooser, drag-to-reorder
//! headers (plan 15 F8), horizontal scrolling (plan 15 F9), centered
//! numeric cells (plan 15 F10), hover-revealed row actions (open output /
//! show in folder / remove) and background thumbnails.
//!
//! Rows render through the **view-only** [`table::sort_indices`]
//! permutation — the queue itself keeps enqueue order (`Queue::selection`
//! and the job events address rows by queue index), so sorting never
//! changes conversion order (explicit header tooltip).
//!
//! Quality metrics (plan 10 §phase 2, plan 15 F19): the per-row metric
//! state is rendered visibly in the status cell — pending `…measuring`,
//! the reading, or the error in red — with the full line in the tooltip.
//!
//! Thumbnails (plan 11 §6): when [`ThumbMode`] is on, the row height
//! grows to [`THUMB_ROW_HEIGHT`] and the Status cell shows a decoded
//! texture from the [`crate::thumb`] worker — requests go out for the
//! visible rows only (the virtualized `body.rows` callback yields the
//! visible slice each frame), results are LRU-cached on the app, the
//! worker is paused while a job runs, and undecodable files render a
//! placeholder glyph with a tooltip instead of an error.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use byteshaver::metadata::exif::ExifSummary;
use egui::TextureHandle;
use egui_extras::TableBuilder;

use crate::app::App;
use crate::celebrate::RectPx;
use crate::platform;
use crate::queue::{ItemStatus, format_size};
use crate::table::{self, Column, SortKey};
use crate::thumb::{ThumbEntry, ThumbKey, ThumbMode};

/// Compact row height (thumbnails off).
const ROW_HEIGHT: f32 = 22.0;
/// Row height while thumbnails are on (48 px texture + padding).
const THUMB_ROW_HEIGHT: f32 = 56.0;
/// Rendered edge of a thumbnail texture.
const THUMB_PX: f32 = 48.0;
/// Header row height.
const HEADER_HEIGHT: f32 = 24.0;

/// Thumbnail state of one visible row for this frame.
enum ThumbCell<'a> {
    /// Decoded texture.
    Ready(&'a TextureHandle),
    /// Decoding in flight / requested.
    Pending,
    /// Undecodable — permanent placeholder.
    Failed,
    /// Thumbnails off or the row is not eligible (never ran, directory).
    Hidden,
}

/// Renders the queue table in the space below the drop-zone banner
/// (inside the shared `CentralPanel`, see [`super::show`]).
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    // plan 15 F7: a persisted order that is exactly the pre-plan-15
    // default is silently upgraded to the new default (anything else is
    // respected); idempotent — after the upgrade the check no longer fires
    if let Some(upgraded) = table::migrate_legacy_columns(&app.settings.table_columns.visible) {
        app.settings.table_columns.visible = upgraded;
        app.mark_settings_dirty();
    }
    if app.queue.is_empty() {
        app.celebrate_cell_rects.clear();
        return;
    }
    chooser_bar(app, ui);
    let columns = app.settings.table_columns.visible.clone();
    // at least one data cell is required (the row hover detection and the
    // whole-row context menu anchor on a data cell's rect)
    if columns.iter().all(|&column| column == Column::Actions) {
        app.celebrate_cell_rects.clear();
        ui.weak("all columns hidden — reopen them from Columns ▾");
        return;
    }
    render_table(app, ui, &columns);
}

/// `Columns ▾` chooser (visibility toggles; metadata columns grouped
/// under their own sub-header) plus the view-only sort hint.
fn chooser_bar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.menu_button("Columns ▾", |ui| {
            for column in Column::ALL {
                if !column.is_metadata() {
                    column_checkbox(app, ui, column);
                }
            }
            ui.separator();
            ui.weak("metadata");
            for column in Column::ALL {
                if column.is_metadata() {
                    column_checkbox(app, ui, column);
                }
            }
        });
        if app.settings.table_sort.column().is_some() && ui.button("reset sort").clicked() {
            app.settings.table_sort = SortKey::None;
            app.mark_settings_dirty();
        }
        ui.weak("sort is view-only")
            .on_hover_text("sorting does not change conversion order");
    });
}

/// One chooser checkbox (hidden columns re-appear at the end of the
/// display order).
fn column_checkbox(app: &mut App, ui: &mut egui::Ui, column: Column) {
    let mut visible = app.settings.table_columns.is_visible(column);
    if ui
        .add(egui::Checkbox::new(&mut visible, column.header()))
        .changed()
    {
        app.settings.table_columns.toggle(column);
        app.mark_settings_dirty();
    }
}

/// Initial/resizable width hint of a column (fixed-ish widths keep the
/// layout stable while the row virtualization scrolls; the name column
/// takes the remainder). The initial width doubles as the **minimum**
/// (plan 15 F9): columns can grow with their content or be resized wider,
/// so the table can overflow and the horizontal scrollbar appears.
fn table_column(column: Column) -> egui_extras::Column {
    let width = match column {
        Column::Status => 150.0,
        Column::Name => return egui_extras::Column::remainder().resizable(true),
        Column::SourceFormat => 64.0,
        Column::MergedSize => 120.0,
        Column::InputSize | Column::OutputSize => 90.0,
        Column::Ratio => 64.0,
        Column::TargetFormat => 64.0,
        Column::Modified => 130.0,
        Column::Dimensions => 90.0,
        Column::ExifCamera | Column::ExifTaken => 140.0,
        Column::ExifIso => 56.0,
        Column::ExifExposure => 72.0,
        Column::Actions => 110.0,
    };
    egui_extras::Column::initial(width)
        .resizable(true)
        .at_least(width)
}

/// Drag payload of a header cell (plan 15 F8): the position of the
/// dragged column in the visible order at drag start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeaderDrag {
    from: usize,
}

/// Header cell (plan 15 F5/F8/F11): sortable columns cycle the sort on
/// click and carry the ▲/▼ indicator only while they are the sorted
/// column; the actions column keeps a plain non-sortable title. Every
/// header is a drag source/target that reorders the visible columns.
fn header_cell(
    app: &mut App,
    ui: &mut egui::Ui,
    position: usize,
    column: Column,
    sort: SortKey,
    column_count: usize,
) {
    let response = if column.sortable() {
        let arrow = table::sort_indicator(sort, column);
        let label = match arrow {
            Some(arrow) => format!("{} {arrow}", column.header()),
            None => column.header().to_owned(),
        };
        let mut text = egui::RichText::new(label);
        if arrow.is_some() {
            text = text.strong();
        }
        let response = ui.add(
            egui::Button::selectable(arrow.is_some(), text).sense(egui::Sense::click_and_drag()),
        );
        if response.clicked() {
            app.settings.table_sort = sort.cycle(column);
            app.mark_settings_dirty();
        }
        response.on_hover_text(format!(
            "sort by {} — click cycles ascending / descending / off\n\
             drag to move the column\n\
             sorting does not change conversion order",
            column.header()
        ))
    } else {
        ui.add(
            egui::Label::new(egui::RichText::new(column.header()).weak())
                .sense(egui::Sense::click_and_drag()),
        )
        .on_hover_text("row actions — drag to move the column")
    };

    // plan 15 F8: drag-to-reorder. The click/sort split comes from egui's
    // click-and-drag sense: a decisive drag suppresses the click (sort) and
    // starts the drag payload instead.
    response.dnd_set_drag_payload(HeaderDrag { from: position });

    let pointer_pos = ui.input(|input| input.pointer.latest_pos());
    if let Some(drag) = response.dnd_hover_payload::<HeaderDrag>() {
        // insertion marker on the covered half (self-drops show none)
        if drag.from != position
            && let Some(pos) = pointer_pos
        {
            let after = pos.x >= response.rect.center().x;
            let x = if after {
                response.rect.right()
            } else {
                response.rect.left()
            };
            ui.painter().line_segment(
                [
                    egui::pos2(x, response.rect.top()),
                    egui::pos2(x, response.rect.bottom()),
                ],
                egui::Stroke::new(2.0_f32, ui.visuals().selection.stroke.color),
            );
        }
    }
    if let Some(drag) = response.dnd_release_payload::<HeaderDrag>() {
        let after = pointer_pos.is_some_and(|pos| pos.x >= response.rect.center().x);
        let to = (position + usize::from(after)).min(column_count);
        let reordered = table::reorder(app.settings.table_columns.visible.clone(), drag.from, to);
        if reordered != app.settings.table_columns.visible {
            app.settings.table_columns.visible = reordered;
            app.mark_settings_dirty();
        }
    }
}

/// Cheap display snapshot of one row (cloned out of the queue borrow so
/// the remove button, action errors and the lazy-data requests can mutate
/// the app afterwards).
struct RowSnapshot {
    path: PathBuf,
    name: String,
    format: String,
    sizes: String,
    sizes_hover: &'static str,
    status: ItemStatus,
    status_text: String,
    error: Option<String>,
    hover: String,
    unsupported: bool,
    unsupported_reason: Option<String>,
    is_dir: bool,
    input_size: Option<u64>,
    output_size: Option<u64>,
    ratio: Option<f32>,
    converted_to: Option<&'static str>,
    modified: Option<SystemTime>,
    dimensions: Option<(u32, u32)>,
    exif: Option<Option<ExifSummary>>,
    output_path: Option<PathBuf>,
}

impl RowSnapshot {
    fn of(item: &crate::queue::QueueItem, heif_enabled: bool) -> Self {
        let name = item
            .path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| item.path.display().to_string());
        let format = if item.is_dir {
            "dir".to_string()
        } else {
            item.source_format.extension().to_string()
        };
        // directory rows show the scanned content instead of the
        // (meaningless) directory inode size
        let sizes = if let Some(summary) = &item.summary {
            format_size(summary.bytes)
        } else {
            size_column(item.input_size, item.output_size, item.status)
        };
        let sizes_hover = if item.summary.is_some() {
            "total size of the contained images (enqueue-time scan)"
        } else {
            "input size → output size"
        };
        let mut status_text = if let Some(summary) = &item.summary {
            format!("→ {} images", summary.images)
        } else {
            item.status.label().to_string()
        };
        if let Some(note) = &item.note {
            status_text.push_str(" · ");
            status_text.push_str(note);
        }
        // tooltip: full path plus the per-format breakdown of scanned
        // directories and the unsupported-extension reason
        let mut hover = item.path.display().to_string();
        if let Some(summary) = &item.summary {
            hover.push('\n');
            hover.push_str(&summary.breakdown(10));
        }
        let unsupported_reason = item.unsupported_reason(heif_enabled);
        if let Some(reason) = &unsupported_reason {
            hover.push('\n');
            hover.push_str(reason);
        }
        let ratio = match (item.input_size, item.output_size) {
            (Some(input), Some(output)) if input > 0 => Some(output as f32 / input as f32),
            _ => None,
        };
        RowSnapshot {
            path: item.path.clone(),
            name,
            format,
            sizes,
            sizes_hover,
            status: item.status,
            status_text,
            error: item.error.clone(),
            hover,
            unsupported: unsupported_reason.is_some(),
            unsupported_reason,
            is_dir: item.is_dir,
            input_size: item.input_size,
            output_size: item.output_size,
            ratio,
            converted_to: item.converted_to,
            modified: item.modified,
            dimensions: item.dimensions,
            exif: item.exif.clone(),
            output_path: item.output_path.clone(),
        }
    }
}

fn render_table(app: &mut App, ui: &mut egui::Ui, columns: &[Column]) {
    let running = app.running.is_some();
    let heif_enabled = app.capabilities.heif_input_enabled;
    let sort = app.settings.table_sort;
    let mode = app.settings.thumbnails;
    let row_height = if mode == ThumbMode::Off {
        ROW_HEIGHT
    } else {
        THUMB_ROW_HEIGHT
    };
    let base: Vec<usize> = (0..app.queue.len()).collect();
    let order = table::sort_indices(app.queue.items(), sort, &base);
    // pointer position of the frame (read once up front: the body closure
    // must not re-borrow the table's `Ui`)
    let pointer_y = ui
        .input(|input| input.pointer.latest_pos())
        .map(|pos| pos.y);

    // thumbnail texture snapshot for the visible rows (the body closure
    // only gets an immutable queue borrow, so the LRU-promoting cache
    // lookups and the texture handle clones happen up front — driven by
    // the previous frame's visible slice, one frame of latency for newly
    // visible rows which render the decoding placeholder meanwhile)
    let mut textures: HashMap<PathBuf, TextureHandle> = HashMap::new();
    let mut thumb_failed: HashSet<PathBuf> = HashSet::new();
    for key in &app.thumbs.take_visible() {
        match app.thumbs.cached(key) {
            Some(ThumbEntry::Ready(texture)) => {
                textures.insert(key.0.clone(), texture.clone());
            }
            Some(ThumbEntry::Failed) => {
                thumb_failed.insert(key.0.clone());
            }
            None => {}
        }
    }
    let mut visible_keys: Vec<ThumbKey> = Vec::new();

    let exif_requested = columns.iter().any(|column| column.is_metadata());
    let dimensions_requested = columns.contains(&Column::Dimensions);
    let mut remove_index: Option<usize> = None;
    let mut action_error: Option<String> = app.action_error.clone();
    let mut need_dimensions: Vec<PathBuf> = Vec::new();
    let mut need_exif: Vec<PathBuf> = Vec::new();
    // plan 10 §phase 2: "Measure quality" requests collected by the row
    // context menus, applied after the table borrow ends
    let mut measure_requests: Vec<(PathBuf, PathBuf)> = Vec::new();
    let metric_off = app.settings.quality_metric == crate::metrics::MetricMode::Off;
    // plan 10 §phase 3: "Inspect visual difference" requests likewise
    let mut inspect_requests: Vec<(PathBuf, PathBuf)> = Vec::new();
    // plan 16 F21: screen-space rect of each visible row's size/ratio
    // cell, swapped into `App::celebrate_cell_rects` after the render
    // (fresh every frame; rows scrolled out of view simply drop out)
    let mut cell_rects: Vec<(PathBuf, RectPx)> = Vec::new();

    // plan 15 F9: egui_extras 0.32's TableBuilder hard-codes its scroll
    // area to `[false, vscroll]`, so the horizontal scroll bar lives on
    // this outer scroll area — header and body overflow (and scroll)
    // together, and the bar appears once the columns exceed the viewport.
    egui::ScrollArea::new([true, false])
        .id_salt("byteshaver-file-table-hscroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let builder = TableBuilder::new(ui)
                .id_salt("byteshaver-file-table")
                .striped(true)
                .vscroll(true)
                .auto_shrink([false, false])
                .resizable(true)
                // click sense so the whole-row context menu works (cells default
                // to hover-only, and Response::context_menu keys off
                // secondary_clicked)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
            let builder = columns.iter().fold(builder, |builder, &column| {
                builder.column(table_column(column))
            });

            let table = builder.header(HEADER_HEIGHT, |mut header| {
                for (position, &column) in columns.iter().enumerate() {
                    header.col(|ui| {
                        header_cell(app, ui, position, column, sort, columns.len());
                    });
                }
            });

            table.body(|body| {
                body.rows(row_height, order.len(), |mut row| {
                    let Some(&queue_index) = order.get(row.index()) else {
                        return;
                    };
                    let Some(item) = app.queue.items().get(queue_index) else {
                        return;
                    };
                    let snapshot = RowSnapshot::of(item, heif_enabled);
                    // plan 10 §phase 2 + plan 15 F19: the per-row metric
                    // state renders visibly in the status cell (pending
                    // marker / reading / error), full line in the tooltip
                    let pending = app.metrics.is_pending(&snapshot.path);
                    let metric_error = app.metrics.error(&snapshot.path);
                    let metric_line = if pending {
                        None
                    } else if let Some(error) = metric_error {
                        Some(error.message.clone())
                    } else {
                        app.metrics.cached(&snapshot.path).map(|entry| {
                            format!(
                                "{} · {}",
                                entry.result.pretty,
                                entry.result.interpretation()
                            )
                        })
                    };
                    let metric = table::metric_status(
                        pending,
                        metric_line.as_deref(),
                        metric_error.map(|error| error.message.as_str()),
                    );
                    let active = app
                        .running
                        .as_ref()
                        .is_some_and(|job| job.active.contains(&item.path));
                    let thumb_cell = if let Some(texture) = textures.get(&item.path) {
                        ThumbCell::Ready(texture)
                    } else if thumb_failed.contains(&item.path) {
                        ThumbCell::Failed
                    } else if thumb_eligible(mode, item) {
                        ThumbCell::Pending
                    } else {
                        ThumbCell::Hidden
                    };
                    visible_keys.push((item.path.clone(), item.modified));
                    let mut row_remove: Option<usize> = None;
                    let mut row_top: Option<f32> = None;
                    // this row's size/ratio cell (celebration origin,
                    // plan 16 F21)
                    let mut celebrate_rect: Option<egui::Rect> = None;

                    for &column in columns {
                        if column == Column::Actions {
                            // hover reveal: the pointer's y band identifies the
                            // row (the actions cell sits outside the data cells'
                            // union response, so their rect is the anchor)
                            let hovered = row_top.is_some_and(|top| {
                                pointer_y.is_some_and(|y| (top..=top + row_height).contains(&y))
                            });
                            row.col(|ui| {
                                actions_cell(
                                    ui,
                                    &snapshot,
                                    running,
                                    hovered,
                                    queue_index,
                                    &mut row_remove,
                                    &mut action_error,
                                );
                            });
                        } else {
                            let (_, response) = row.col(|ui| {
                                data_cell(
                                    ui,
                                    column,
                                    &snapshot,
                                    active,
                                    &thumb_cell,
                                    metric.clone(),
                                );
                            });
                            row_top.get_or_insert(response.rect.top());
                            if column == Column::MergedSize {
                                celebrate_rect = Some(response.rect);
                            }
                        }
                    }

                    if let Some(rect) = celebrate_rect {
                        cell_rects.push((snapshot.path.clone(), RectPx::from_egui(rect)));
                    }

                    // right-click menu (plan 10 §phase 2/3)
                    row.response().context_menu(|ui| {
                        output_action_buttons(ui, &snapshot, &mut action_error, true);
                        ui.separator();
                        measure_quality_button(
                            ui,
                            &snapshot,
                            running,
                            metric_off,
                            &mut measure_requests,
                        );
                        inspect_difference_button(
                            ui,
                            &snapshot,
                            heif_enabled,
                            running,
                            &mut inspect_requests,
                        );
                        ui.separator();
                        ui.add_enabled_ui(!running, |ui| {
                            if ui.button("✕ remove from queue").clicked() {
                                row_remove = Some(queue_index);
                                ui.close();
                            }
                        });
                    });

                    if row_remove.is_some() {
                        remove_index = row_remove;
                    }

                    // lazy data: dimensions read on-demand (header-only stat),
                    // EXIF via the shared background worker — once per row
                    if dimensions_requested && snapshot.dimensions.is_none() && !snapshot.is_dir {
                        need_dimensions.push(snapshot.path.clone());
                    }
                    if exif_requested && snapshot.exif.is_none() && !snapshot.is_dir {
                        need_exif.push(snapshot.path.clone());
                    }
                });
            });
        });

    if let Some(index) = remove_index {
        app.queue.remove(index);
    }
    for (input, output) in measure_requests {
        app.measure_quality(&input, &output);
    }
    for (input, output) in inspect_requests {
        app.open_inspector(&input, &output);
    }
    app.action_error = action_error;
    app.thumbs.set_visible(visible_keys.clone());
    app.request_row_data(&visible_keys, &need_dimensions, &need_exif);
    // plan 16 F21: per-frame refresh — the map is always exactly this
    // frame's visible size/ratio cells
    app.celebrate_cell_rects = cell_rects.into_iter().collect();
}

/// Whether a row shows a thumbnail under the current mode (plan 11 §6:
/// `OnConvert` = rows that entered a run, `OnAdd` = every file row;
/// directory rows never decode).
fn thumb_eligible(mode: ThumbMode, item: &crate::queue::QueueItem) -> bool {
    if item.is_dir {
        return false;
    }
    match mode {
        ThumbMode::Off => false,
        ThumbMode::OnAdd => true,
        ThumbMode::OnConvert => item.status != ItemStatus::Queued,
    }
}

/// Centers one line of cell text (plan 15 F10): the ratio, dimensions,
/// format and target cells share the size column's `add_sized` approach
/// with an explicit center-aligned label.
fn centered_cell(ui: &mut egui::Ui, text: egui::RichText) -> egui::Response {
    ui.add_sized(
        [ui.available_width(), ROW_HEIGHT],
        egui::Label::new(text)
            .halign(egui::Align::Center)
            .selectable(false),
    )
}

/// Renders one data cell.
fn data_cell(
    ui: &mut egui::Ui,
    column: Column,
    snapshot: &RowSnapshot,
    active: bool,
    thumb_cell: &ThumbCell<'_>,
    metric: Option<table::MetricStatus>,
) {
    match column {
        Column::Status => {
            // thumbnail (when decoded) left of the status glyph
            match thumb_cell {
                ThumbCell::Ready(texture) => {
                    ui.add(
                        egui::Image::new(*texture)
                            .max_size(egui::vec2(THUMB_PX, THUMB_PX))
                            .corner_radius(3.0),
                    )
                    .on_hover_text(&snapshot.name);
                }
                ThumbCell::Pending => {
                    ui.label(egui::RichText::new("∅").weak())
                        .on_hover_text("decoding thumbnail…");
                }
                ThumbCell::Failed => {
                    ui.label(egui::RichText::new("␥").weak())
                        .on_hover_text("thumbnail unavailable (undecodable or unsupported format)");
                }
                ThumbCell::Hidden => {}
            }
            status_content(ui, snapshot, active, metric);
        }
        Column::Name => {
            let mut name_text = egui::RichText::new(&snapshot.name).monospace();
            if snapshot.unsupported {
                name_text = name_text.weak();
            }
            ui.add(egui::Label::new(name_text).truncate().selectable(false))
                .on_hover_text(snapshot.hover.clone());
        }
        Column::SourceFormat => {
            centered_cell(
                ui,
                egui::RichText::new(&snapshot.format)
                    .monospace()
                    .weak()
                    .size(12.0),
            )
            .on_hover_text("detected source format (extension sniff)");
        }
        Column::MergedSize => {
            ui.add_sized(
                [ui.available_width(), ROW_HEIGHT],
                egui::Label::new(egui::RichText::new(&snapshot.sizes).size(12.0))
                    .halign(egui::Align::Center)
                    .selectable(false),
            )
            .on_hover_text(snapshot.sizes_hover);
        }
        Column::InputSize => {
            ui.monospace(
                snapshot
                    .input_size
                    .map_or_else(|| "—".to_string(), format_size),
            )
            .on_hover_text("input file size");
        }
        Column::OutputSize => {
            ui.monospace(
                snapshot
                    .output_size
                    .map_or_else(|| "—".to_string(), format_size),
            )
            .on_hover_text("output size of the last run");
        }
        Column::Ratio => ratio_cell(ui, snapshot),
        Column::TargetFormat => {
            if let Some(target) = snapshot.converted_to {
                centered_cell(
                    ui,
                    egui::RichText::new(target).monospace().weak().size(12.0),
                )
                .on_hover_text("target format of the last run");
            }
        }
        Column::Modified => {
            if let Some(modified) = snapshot.modified {
                ui.monospace(egui::RichText::new(table::format_system_time(modified)).size(12.0))
                    .on_hover_text("file modification time at enqueue (UTC)");
            }
        }
        Column::Dimensions => match snapshot.dimensions {
            Some((0, 0)) => {
                centered_cell(ui, egui::RichText::new("—").weak())
                    .on_hover_text("dimensions unreadable");
            }
            Some((width, height)) => {
                centered_cell(
                    ui,
                    egui::RichText::new(format!("{width}×{height}"))
                        .monospace()
                        .size(12.0),
                );
            }
            None => {}
        },
        Column::ExifCamera => {
            exif_cell(ui, snapshot, |summary| summary.camera.clone())
                .on_hover_text("EXIF Make + Model");
        }
        Column::ExifTaken => {
            exif_cell(ui, snapshot, |summary| {
                summary
                    .date_time_original
                    .as_deref()
                    .map(table::format_exif_taken)
            })
            .on_hover_text("EXIF DateTimeOriginal (shown as yyyy.mm.dd hh:mm:ss)");
        }
        Column::ExifIso => {
            exif_cell(ui, snapshot, |summary| {
                summary.iso.map(|iso| iso.to_string())
            })
            .on_hover_text("EXIF ISO speed");
        }
        Column::ExifExposure => {
            exif_cell(ui, snapshot, |summary| summary.exposure.clone())
                .on_hover_text("EXIF exposure time");
        }
        Column::Actions => {}
    }
}

/// The EXIF cell text from the row cache: `…` = not read yet, `—` = read
/// but the file carries no such field.
fn exif_cell(
    ui: &mut egui::Ui,
    snapshot: &RowSnapshot,
    field: impl FnOnce(&ExifSummary) -> Option<String>,
) -> egui::Response {
    let text = match &snapshot.exif {
        None => "…".to_string(),
        Some(None) => "—".to_string(),
        Some(Some(summary)) => field(summary).unwrap_or_else(|| "—".to_string()),
    };
    ui.add(egui::Label::new(egui::RichText::new(text).size(12.0)).selectable(false))
}

/// Ratio cell: percent text with a subtle cell tint from the shared
/// plan-10 ratio bands ([`crate::ratio::hint_for_ratio`]: ≤ 0.8 good /
/// ≤ 1.0 neutral / > 1.0 grew) — the percent text always carries the
/// information, the color is redundancy (accessibility: never color-only).
fn ratio_cell(ui: &mut egui::Ui, snapshot: &RowSnapshot) {
    let Some(ratio) = snapshot.ratio else {
        return;
    };
    let hint = match (snapshot.input_size, snapshot.output_size) {
        (Some(input), Some(output)) => crate::ratio::ratio_hint(input, output),
        _ => return,
    };
    let color = crate::ratio::hint_color(hint, ui.visuals());
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(rect, 3.0, color.gamma_multiply(0.25));
    centered_cell(
        ui,
        egui::RichText::new(format!("{:.0} %", (ratio * 100.0).round()))
            .size(12.0)
            .color(color),
    )
    .on_hover_text(format!(
        "output / input = {ratio:.2} — ≤ 80 % shades green, > 100 % amber"
    ));
}

/// The row's status glyph (refined by the run's active set, plan 12 §1)
/// plus the status label/note — with the plan-15 F19 quality-metric state
/// rendered visibly after it (`…measuring` / `dssim 0.0120 · excellent
/// match` / the error, in red); the full reading stays in the tooltip.
fn status_content(
    ui: &mut egui::Ui,
    snapshot: &RowSnapshot,
    active: bool,
    metric: Option<table::MetricStatus>,
) -> egui::Response {
    let glyph_color = status_color(ui, snapshot.status, snapshot.unsupported);
    ui.label(egui::RichText::new(snapshot.status.glyph_while_running(active)).color(glyph_color));
    // plan 15 F19: visible metric state in the row, not tooltip-only
    let mut status_text = snapshot.status_text.clone();
    if let Some(metric) = &metric {
        status_text.push_str(" · ");
        status_text.push_str(metric.line());
    }
    let metric_error = metric.as_ref().is_some_and(table::MetricStatus::is_error);
    let mut status_rich = egui::RichText::new(&status_text).size(12.0);
    if snapshot.error.is_some() || metric_error {
        status_rich = status_rich.color(ui.visuals().error_fg_color);
    }
    let status_label = egui::Label::new(status_rich).truncate().selectable(false);
    let response = ui.add_sized([ui.available_width().max(40.0), ROW_HEIGHT], status_label);
    // tooltip: error/reason first, the metric reading appended below it
    // (information in text, never color-only)
    let mut tooltip = String::new();
    if let Some(error) = &snapshot.error {
        tooltip.push_str(error);
    } else if let Some(reason) = &snapshot.unsupported_reason {
        tooltip.push_str(reason);
    }
    if let Some(metric) = &metric {
        if !tooltip.is_empty() {
            tooltip.push('\n');
        }
        tooltip.push_str("quality: ");
        tooltip.push_str(metric.line());
    }
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// The plan-10 "Measure quality" context-menu action: enabled iff the row
/// has a written output, metrics are not `Off` and no job is running.
/// The request is collected into `requests` (applied after the table
/// borrow ends) — the comparison itself runs on the metric worker.
fn measure_quality_button(
    ui: &mut egui::Ui,
    snapshot: &RowSnapshot,
    running: bool,
    metric_off: bool,
    requests: &mut Vec<(PathBuf, PathBuf)>,
) {
    let enabled = snapshot.output_path.is_some() && !metric_off && !running;
    let hint = if metric_off {
        "quality metrics are switched off (settings)"
    } else if running {
        "a conversion job is running"
    } else if snapshot.output_path.is_none() {
        "no output was written"
    } else {
        "compare input and output (bounded decode) — the reading shows in the status cell"
    };
    let button = ui.add_enabled(enabled, egui::Button::new("Measure quality"));
    let button = if enabled {
        button.on_hover_text(hint)
    } else {
        button.on_disabled_hover_text(hint)
    };
    if button.clicked()
        && let Some(output) = &snapshot.output_path
    {
        requests.push((snapshot.path.clone(), output.clone()));
        ui.close();
    }
}

/// The plan-10 "Inspect visual difference…" context-menu action: enabled
/// iff the row has a written output and no job is running; HEIF *input*
/// rows are grayed with the queue's unsupported-extension reason when
/// `dec-heif` is missing. The bounded decode runs on the metric worker.
fn inspect_difference_button(
    ui: &mut egui::Ui,
    snapshot: &RowSnapshot,
    heif_enabled: bool,
    running: bool,
    requests: &mut Vec<(PathBuf, PathBuf)>,
) {
    let heif_blocked = !heif_enabled
        && snapshot
            .unsupported_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("dec-heif"));
    let enabled = snapshot.output_path.is_some() && !running && !heif_blocked;
    let hint = if heif_blocked {
        snapshot.unsupported_reason.clone().unwrap_or_default()
    } else if running {
        "a conversion job is running".to_string()
    } else if snapshot.output_path.is_none() {
        "no output was written".to_string()
    } else {
        "bounded decode of both files (2048 px) in a swipe/side-by-side/difference view".to_string()
    };
    let button = ui.add_enabled(enabled, egui::Button::new("Inspect visual difference…"));
    let button = if enabled {
        button.on_hover_text(hint)
    } else {
        button.on_disabled_hover_text(hint)
    };
    if button.clicked()
        && let Some(output) = &snapshot.output_path
    {
        requests.push((snapshot.path.clone(), output.clone()));
        ui.close();
    }
}

/// Hover-revealed row actions (plan 11 §5 decision D4): open output /
/// show in folder / remove — the output actions enabled only for rows
/// with a written output.
#[allow(clippy::too_many_arguments)]
fn actions_cell(
    ui: &mut egui::Ui,
    snapshot: &RowSnapshot,
    running: bool,
    hovered: bool,
    queue_index: usize,
    remove_slot: &mut Option<usize>,
    error_slot: &mut Option<String>,
) {
    if !hovered {
        ui.weak("⋯").on_hover_text("row actions");
        return;
    }
    output_action_buttons(ui, snapshot, error_slot, false);
    ui.add_enabled_ui(!running, |ui| {
        if ui
            .add(egui::Button::new("✕").small())
            .on_hover_text("remove from the queue")
            .on_disabled_hover_text("a job is running")
            .clicked()
        {
            *remove_slot = Some(queue_index);
        }
    });
}

/// Labels + tooltips of the two output actions (plan 16 F23): the row
/// context menu carries the explicit full wording; the hover-revealed
/// buttons stay compact (the actions column is narrow) while their
/// tooltips always spell the action out.
struct OutputActionLabels {
    open: &'static str,
    folder: &'static str,
    open_tooltip: &'static str,
    folder_tooltip: &'static str,
}

#[must_use]
fn output_action_labels(in_menu: bool) -> OutputActionLabels {
    if in_menu {
        OutputActionLabels {
            open: "open converted file",
            folder: "open output folder",
            open_tooltip: "open converted file with the OS default viewer",
            folder_tooltip: "open output folder in the file manager",
        }
    } else {
        OutputActionLabels {
            open: "open file",
            folder: "output folder",
            open_tooltip: "open converted file (with the OS default viewer)",
            folder_tooltip: "open output folder (reveal it in the file manager)",
        }
    }
}

/// The two output actions (shared by the hover reveal and the context
/// menu — `in_menu` picks the label set, see [`output_action_labels`]):
/// enabled iff a run actually wrote an output for this row; otherwise a
/// disabled tooltip explains why. Spawn errors surface as row tooltips
/// via [`App::action_error`], never dialogs.
fn output_action_buttons(
    ui: &mut egui::Ui,
    snapshot: &RowSnapshot,
    error_slot: &mut Option<String>,
    in_menu: bool,
) {
    let labels = output_action_labels(in_menu);
    let action_error = error_slot.clone();
    let convertible = matches!(
        snapshot.status,
        ItemStatus::Encoded | ItemStatus::SkippedExisting
    );
    let enabled = convertible && snapshot.output_path.is_some();
    let disabled_reason = if convertible {
        "no output was written"
    } else {
        "not converted yet"
    };
    ui.add_enabled_ui(enabled, |ui| {
        let open = ui
            .button(labels.open)
            .on_hover_text(labels.open_tooltip)
            .on_disabled_hover_text(disabled_reason);
        if open.clicked()
            && let Some(output) = &snapshot.output_path
        {
            match platform::open_path(output) {
                Ok(()) => *error_slot = None,
                Err(message) => *error_slot = Some(message),
            }
        }
        let reveal = ui
            .button(labels.folder)
            .on_hover_text(labels.folder_tooltip)
            .on_disabled_hover_text(disabled_reason);
        if reveal.clicked()
            && let Some(output) = &snapshot.output_path
        {
            match platform::reveal_in_folder(output) {
                Ok(()) => *error_slot = None,
                Err(message) => *error_slot = Some(message),
            }
        }
    });
    if let Some(message) = action_error {
        ui.weak("⚠").on_hover_text(message);
    }
}

/// "in → out" column text of a row.
fn size_column(input: Option<u64>, output: Option<u64>, status: ItemStatus) -> String {
    let Some(input) = input else {
        return "—".to_string();
    };
    let in_text = format_size(input);
    match (output, status) {
        (Some(output), ItemStatus::Encoded) => format!("{in_text} → {}", format_size(output)),
        (Some(output), ItemStatus::SkippedExisting) => {
            format!("{in_text} ≈ {}", format_size(output))
        }
        (Some(output), ItemStatus::DiscardedLargerThanInput) => {
            format!("{in_text} ⤬ {}", format_size(output))
        }
        _ => in_text,
    }
}

/// Status glyph color (weak for queued/unsupported, error red, success
/// green).
fn status_color(ui: &egui::Ui, status: ItemStatus, unsupported: bool) -> egui::Color32 {
    match status {
        ItemStatus::Queued if unsupported => ui.visuals().weak_text_color(),
        ItemStatus::Queued => ui.visuals().text_color(),
        ItemStatus::Running => ui.visuals().selection.stroke.color,
        ItemStatus::Encoded => egui::Color32::from_rgb(90, 190, 110),
        ItemStatus::Error => ui.visuals().error_fg_color,
        _ => ui.visuals().weak_text_color(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- plan 16 F23: explicit action labels ---------------------------------

    #[test]
    fn output_action_labels_carry_the_explicit_wording() {
        // context menu: the full wording
        let menu = output_action_labels(true);
        assert_eq!(menu.open, "open converted file");
        assert_eq!(menu.folder, "open output folder");
        assert!(menu.open_tooltip.contains("open converted file"));
        assert!(menu.folder_tooltip.contains("open output folder"));

        // hover buttons: compact but explicit, tooltips always spell it out
        let hover = output_action_labels(false);
        assert_eq!(hover.open, "open file");
        assert_eq!(hover.folder, "output folder");
        assert!(hover.open_tooltip.contains("open converted file"));
        assert!(hover.folder_tooltip.contains("open output folder"));

        // no fallback to the old bare labels anywhere
        assert_ne!(hover.open, "open");
        assert_ne!(hover.folder, "folder");
        assert_ne!(menu.open, hover.open);
        assert_ne!(menu.folder, hover.folder);
    }
}
