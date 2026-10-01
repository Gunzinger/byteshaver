//! Report window: per-file results of the last run (status mapped from
//! `Outcome`, sizes, error text), the aggregated totals and the JSONL
//! export — one JSON object per retained [`JobEvent`], byte-compatible
//! with the core's `JsonlReporter` format (WS7 `--json-log`).

use byteshaver::job::{JobEvent, RunReport};
use byteshaver::pipeline::Outcome;

use crate::app::App;
use crate::queue::{ItemStatus, format_size};

/// Renders the report window when open.
pub fn show_window(app: &mut App, ctx: &egui::Context) {
    let mut open = app.show_report;
    egui::Window::new("Run report")
        .open(&mut open)
        .default_width(720.0)
        .default_height(420.0)
        .show(ctx, |ui| {
            let Some(report) = app.report.clone() else {
                ui.label("No finished run yet.");
                return;
            };
            totals_block(ui, &report);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Export JSONL…").clicked() {
                    export_jsonl(app);
                }
                ui.weak(format!(
                    "{} events retained for export",
                    app.event_log.len()
                ));
            });
            ui.add_space(6.0);
            ui.separator();

            // per-file results (virtualized; the row count can be large).
            // Rows are exactly ROW_HEIGHT tall (no per-row separator) and
            // use proportional widths so no column is clipped off-window.
            let rows = report.files.len();
            const ROW_HEIGHT: f32 = 20.0;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, ROW_HEIGHT, rows, |ui, range| {
                    for index in range {
                        let Some(result) = report.files.get(index) else {
                            continue;
                        };
                        let status = ItemStatus::from_outcome(&result.outcome);
                        ui.horizontal(|ui| {
                            ui.set_min_height(ROW_HEIGHT - 4.0);
                            ui.monospace(
                                egui::RichText::new(status.glyph()).color(status_color(ui, status)),
                            );
                            let spacing = ui.spacing().item_spacing.x * 3.0;
                            let sizes_width = 120.0;
                            let status_width = (ui.available_width() * 0.32).clamp(170.0, 260.0);
                            let width = (ui.available_width()
                                - sizes_width
                                - status_width
                                - spacing
                                - 14.0)
                                .max(80.0);
                            let name = result
                                .path
                                .file_name()
                                .map(|name| name.to_string_lossy().to_string())
                                .unwrap_or_else(|| result.path.display().to_string());
                            ui.add_sized(
                                [width, ROW_HEIGHT],
                                egui::Label::new(egui::RichText::new(&name).weak().monospace())
                                    .truncate()
                                    .selectable(false),
                            )
                            .on_hover_text(result.path.display().to_string());
                            let sizes = match &result.outcome {
                                Outcome::Encoded {
                                    input_size,
                                    output_size,
                                    ..
                                } => {
                                    format!(
                                        "{} → {}",
                                        format_size(*input_size),
                                        format_size(*output_size)
                                    )
                                }
                                Outcome::SkippedExisting {
                                    input_size,
                                    existing_size,
                                    ..
                                }
                                | Outcome::DiscardedLargerThanExisting {
                                    input_size,
                                    existing_size,
                                } => format!(
                                    "{} ≈ {}",
                                    format_size(*input_size),
                                    format_size(*existing_size)
                                ),
                                Outcome::SkippedCollision { input_size, .. } => {
                                    format_size(*input_size)
                                }
                                Outcome::DiscardedLargerThanInput {
                                    input_size,
                                    encoded_size,
                                } => format!(
                                    "{} ⤬ {}",
                                    format_size(*input_size),
                                    format_size(*encoded_size)
                                ),
                                Outcome::Error(_) | Outcome::Aborted => "—".to_string(),
                            };
                            ui.add_sized(
                                [sizes_width, ROW_HEIGHT],
                                egui::Label::new(egui::RichText::new(sizes).size(12.0)),
                            );
                            let status_color = if let Outcome::Error(_) = &result.outcome {
                                ui.visuals().error_fg_color
                            } else {
                                status_color(ui, status)
                            };
                            let status_label = egui::Label::new(
                                egui::RichText::new(status.label())
                                    .size(12.0)
                                    .color(status_color),
                            )
                            .truncate()
                            .selectable(false);
                            let status_response =
                                ui.add_sized([status_width, ROW_HEIGHT], status_label);
                            if let Outcome::Error(message) = &result.outcome {
                                status_response.on_hover_text(message.clone());
                            }
                        });
                    }
                });

            ui.add_space(6.0);
            ui.separator();
            ui.heading("Notices");
            let notices: Vec<String> = app
                .event_log
                .iter()
                .filter_map(|event| match event {
                    JobEvent::Notice { message, .. } => Some(message.clone()),
                    _ => None,
                })
                .collect();
            egui::ScrollArea::vertical()
                .id_salt("report-notices")
                .max_height(120.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if notices.is_empty() {
                        ui.weak("(none)");
                    }
                    for notice in notices {
                        ui.label(egui::RichText::new(notice).weak().size(12.0));
                    }
                });
        });
    app.show_report = open;
}

/// Aggregated totals (the "Encode statistics" numbers).
fn totals_block(ui: &mut egui::Ui, report: &RunReport) {
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{} files · ✔ {} ok · — {} skipped · ⤷ {} collisions · ✂ {} discarded · ✖ {} errors · ⏸ {} aborted",
            report.totals.input_files,
            report.totals.successful,
            report.totals.skipped,
            report.totals.collisions,
            report.totals.discarded,
            report.totals.errors,
            report.totals.aborted
        ));
    });
    ui.label(format!(
        "{} → {} ({:.02}%) in {:.1}s",
        format_size(report.totals.input_size),
        format_size(report.totals.output_size),
        if report.totals.input_size > 0 {
            report.totals.output_size as f64 / report.totals.input_size as f64 * 100.0
        } else {
            0.0
        },
        report.elapsed.as_secs_f32()
    ));
    if report.metadata_dropped_count > 0 {
        ui.weak(format!(
            "{} outputs could not carry EXIF metadata (target format has no support)",
            report.metadata_dropped_count
        ));
    }
    if let Some(error) = &report.error {
        ui.colored_label(ui.visuals().error_fg_color, error.clone());
    }
}

/// Writes the retained events of the run as JSON-lines (same schema as the
/// core's `JsonlReporter`).
fn export_jsonl(app: &App) {
    let Some(path) = rfd::FileDialog::new()
        .set_file_name("byteshaver-report.jsonl")
        .save_file()
    else {
        return;
    };
    let mut out = String::new();
    for event in &app.event_log {
        if let Ok(line) = serde_json::to_string(event) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    if let Err(err) = std::fs::write(&path, out) {
        eprintln!("byteshaver-gui: could not write JSONL report: {err}");
    }
}

/// Report-row status color.
fn status_color(ui: &egui::Ui, status: ItemStatus) -> egui::Color32 {
    match status {
        ItemStatus::Encoded => egui::Color32::from_rgb(90, 190, 110),
        ItemStatus::Error => ui.visuals().error_fg_color,
        _ => ui.visuals().weak_text_color(),
    }
}
