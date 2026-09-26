//! The persistent drop-zone banner (plan WS8 §3 Concept-1): the whole
//! window is a drop target (handled in `App::update` via
//! `RawInput::dropped_files`/`hovered_files`); this banner is the always
//! visible affordance — highlighted while a drag hovers, with click-to-
//! browse (files) and add-folder buttons using the native `rfd` dialogs.

use crate::app::{App, INPUT_EXTENSIONS};
use crate::queue::format_size;

/// Height of the banner in points.
const BANNER_HEIGHT: f32 = 64.0;

/// Renders the drop-zone banner at the top of the central panel
/// (inside the shared `CentralPanel`, see [`super::show`]).
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let highlight = app.drag_hovered;
    let queued_bytes: u64 = app
        .queue
        .items()
        .iter()
        .filter_map(|item| item.input_size)
        .sum();
    let file_count = app.queue.items().iter().filter(|item| !item.is_dir).count();
    let dir_count = app.queue.items().iter().filter(|item| item.is_dir).count();

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
                let mut summary = format!("{file_count} files queued");
                if dir_count > 0 {
                    summary.push_str(&format!(" · {dir_count} folders (expanded recursively)"));
                }
                if queued_bytes > 0 {
                    summary.push_str(&format!(" · {}", format_size(queued_bytes)));
                }
                summary
            };
            ui.label(egui::RichText::new(summary).weak());
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
