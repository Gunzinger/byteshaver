//! Virtualized queue table (plan WS8 §5.3): `ScrollArea::show_rows`
//! renders only the visible slice, keeping the UI smooth at thousands of
//! rows. Columns: status glyph, file name (full path in a tooltip), source
//! format, in → out sizes, status label + note/error, remove button.
//!
//! Rows must occupy *exactly* [`ROW_HEIGHT`]: `show_rows` scrolls by a
//! fixed multiple of it, so anything taller (e.g. a separator per row)
//! makes the content drift against the scrollbar. Column widths are
//! proportional to the available width — fixed pixel columns used to
//! overflow narrow windows, clipping the right-hand columns with no way
//! to scroll them into view.

use crate::app::App;
use crate::queue::{ItemStatus, format_size};

/// Uniform row height used by the row virtualization.
const ROW_HEIGHT: f32 = 22.0;

/// Renders the queue table in the space below the drop-zone banner
/// (inside the shared `CentralPanel`, see [`super::show`]).
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if app.queue.is_empty() {
        return;
    }
    let running = app.running.is_some();
    let heif_enabled = app.capabilities.heif_input_enabled;
    let total = app.queue.len();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, total, |ui, range| {
            for index in range {
                let Some(item) = app.queue.items().get(index) else {
                    continue;
                };
                // clone the cheap display data out of the borrow so the
                // remove button can mutate the queue afterwards
                let name = item
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| item.path.display().to_string());
                let full_path = item.path.display().to_string();
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
                let status = item.status;
                let mut status_text = if let Some(summary) = &item.summary {
                    format!("→ {} images", summary.images)
                } else {
                    status.label().to_string()
                };
                if let Some(note) = &item.note {
                    status_text.push_str(" · ");
                    status_text.push_str(note);
                }
                let error = item.error.clone();
                let supported = item.is_supported(heif_enabled);
                let unsupported_reason = item.unsupported_reason(heif_enabled);
                // tooltip: full path plus the per-format breakdown for
                // scanned directories
                let mut hover = full_path.clone();
                if let Some(summary) = &item.summary {
                    hover.push('\n');
                    hover.push_str(&summary.breakdown(10));
                }
                if let Some(reason) = &unsupported_reason {
                    hover.push('\n');
                    hover.push_str(reason);
                }

                ui.horizontal(|ui| {
                    ui.set_min_height(ROW_HEIGHT - 4.0);
                    let glyph_color = status_color(ui, status, supported);
                    ui.add(
                        egui::Label::new(egui::RichText::new(status.glyph()).color(glyph_color))
                            .selectable(false),
                    );
                    // column widths: proportional so every column stays
                    // visible at any window size (name takes the rest)
                    let spacing = ui.spacing().item_spacing.x * 5.0;
                    let status_width = (ui.available_width() * 0.26).clamp(140.0, 290.0);
                    let fixed = status_width + 110.0 + 44.0 + 16.0 + spacing;
                    let name_width = (ui.available_width() - fixed).max(60.0);

                    let mut name_text = egui::RichText::new(&name).monospace();
                    if unsupported_reason.is_some() {
                        name_text = name_text.weak();
                    }
                    let name_label = egui::Label::new(name_text).truncate().selectable(false);
                    ui.add_sized([name_width, ROW_HEIGHT], name_label)
                        .on_hover_text(hover);

                    ui.monospace(egui::RichText::new(format).weak().size(12.0))
                        .on_hover_text("detected source format (extension sniff)");
                    ui.add_sized(
                        [110.0, ROW_HEIGHT],
                        egui::Label::new(egui::RichText::new(sizes).size(12.0)),
                    )
                    .on_hover_text(sizes_hover);
                    let mut status_rich = egui::RichText::new(&status_text).size(12.0);
                    if error.is_some() {
                        status_rich = status_rich.color(ui.visuals().error_fg_color);
                    }
                    let status_label = egui::Label::new(status_rich).truncate().selectable(false);
                    let status_response = ui.add_sized([status_width, ROW_HEIGHT], status_label);
                    if let Some(error) = &error {
                        status_response.on_hover_text(error.clone());
                    } else if let Some(reason) = &unsupported_reason {
                        status_response.on_hover_text(reason.clone());
                    }

                    ui.add_enabled_ui(!running, |ui| {
                        if ui
                            .add_enabled(true, egui::Button::new("✕").small())
                            .on_disabled_hover_text("a job is running")
                            .clicked()
                        {
                            app.queue.remove(index);
                        }
                    });
                });
                // no per-row separator: rows must stay exactly ROW_HEIGHT
                // tall or show_rows' scroll math drifts against the content
            }
        });
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
fn status_color(ui: &egui::Ui, status: ItemStatus, supported: bool) -> egui::Color32 {
    match status {
        ItemStatus::Queued if !supported => ui.visuals().weak_text_color(),
        ItemStatus::Queued => ui.visuals().text_color(),
        ItemStatus::Running => ui.visuals().selection.stroke.color,
        ItemStatus::Encoded => egui::Color32::from_rgb(90, 190, 110),
        ItemStatus::Error => ui.visuals().error_fg_color,
        _ => ui.visuals().weak_text_color(),
    }
}
