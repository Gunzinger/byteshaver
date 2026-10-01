//! Report window: per-file results of the last run (status mapped from
//! `Outcome`, sizes, error text), the aggregated totals and the JSONL
//! export — one JSON object per retained [`JobEvent`], byte-compatible
//! with the core's `JsonlReporter` format (WS7 `--json-log`).
//!
//! The report renders in its **own OS viewport** (plan 09), so it can be
//! dragged outside the main window, resized and maximized. When the
//! backend cannot spawn multiple viewports, egui invokes the viewport
//! callback with [`egui::ViewportClass::Embedded`] and the body degrades
//! to a plain `egui::Window` inside the main viewport. In both shapes
//! every scroll area carries an explicit height bound — an unbounded
//! `auto_shrink` list made the old in-viewport window expand to the full
//! row count and snap back on every resize attempt.

use std::time::Duration;

use byteshaver::job::{JobEvent, RunReport};
use byteshaver::pipeline::Outcome;

use crate::app::App;
use crate::queue::{ItemStatus, format_size};
use crate::settings::Settings;

/// Stable viewport id: re-opening re-renders the same viewport, so egui
/// reuses the OS window instead of churning a new one per run.
const VIEWPORT_ID: &str = "run-report";

/// OS window title of the report viewport.
const WINDOW_TITLE: &str = "byteshaver — run report";

/// Default inner size, used before any geometry has been persisted.
const DEFAULT_INNER_SIZE: [f32; 2] = [760.0, 480.0];

/// Height reserved below the file table for the separator, the "Notices"
/// heading and the notices block (which itself caps at 120 px).
const NOTICES_RESERVE: f32 = 150.0;

/// Lower bound of the file-table scroll height so the table stays usable
/// in a heavily shrunk window (`available_height` can go negative there).
const MIN_TABLE_HEIGHT: f32 = 60.0;

/// Max height of the notices scroll area.
const NOTICES_MAX_HEIGHT: f32 = 120.0;

/// Exact height of one file-table row (virtualization granularity).
const ROW_HEIGHT: f32 = 20.0;

/// Lazy self-repaint cadence of the report viewport (the report is static
/// after a run finishes).
const REPAINT_INTERVAL: Duration = Duration::from_millis(200);

/// Spawns the report viewport when open (called every pass from
/// `panels::show`). On backends without multi-viewport support egui calls
/// the callback with `ViewportClass::Embedded` and the body renders in a
/// plain window inside the main viewport instead.
pub fn show_window(app: &mut App, ctx: &egui::Context) {
    let builder = viewport_builder(&app.settings);
    ctx.show_viewport_immediate(
        egui::ViewportId::from_hash_of(VIEWPORT_ID),
        builder,
        move |vctx, class| {
            if class == egui::ViewportClass::Embedded {
                // graceful degradation: a plain (bounded!) egui::Window —
                // still trapped in the main window, but sized sanely
                show_embedded_window(app, vctx);
            } else {
                show_viewport_contents(app, vctx);
                vctx.request_repaint_after(REPAINT_INTERVAL);
            }
        },
    );
}

/// Builds the viewport builder: title, default size and the geometry
/// persisted from a previous session, if any.
fn viewport_builder(settings: &Settings) -> egui::ViewportBuilder {
    let mut builder = egui::ViewportBuilder::default()
        .with_title(WINDOW_TITLE)
        .with_inner_size(DEFAULT_INNER_SIZE);
    if let Some([x, y, width, height]) = settings.report_window_geometry {
        builder = builder
            .with_position(egui::Pos2::new(x, y))
            .with_inner_size([width, height]);
    }
    builder
}

/// Renders the contents of the standalone viewport: the report body in a
/// `CentralPanel` of the viewport's own context, plus the OS-title-bar
/// close handling and the geometry persistence.
pub fn show_viewport_contents(app: &mut App, ctx: &egui::Context) {
    // closing via the OS title bar hides the viewport for good (it is not
    // spawned again while `show_report` is false, so the header button
    // reflects the reset state)
    if ctx.input(|input| input.viewport().close_requested()) {
        app.show_report = false;
        return;
    }
    persist_geometry(app, ctx);
    egui::CentralPanel::default().show(ctx, |ui| report_body(app, ui));
}

/// Embedded fallback for backends without multi-viewport support: the same
/// bounded body in a plain `egui::Window` (still clipped to the main
/// window, but with sane sizing).
pub fn show_embedded_window(app: &mut App, ctx: &egui::Context) {
    let mut open = app.show_report;
    egui::Window::new("Run report")
        .open(&mut open)
        .default_width(DEFAULT_INNER_SIZE[0])
        .default_height(DEFAULT_INNER_SIZE[1])
        .show(ctx, |ui| report_body(app, ui));
    app.show_report = open;
}

/// Stores the current viewport rect into the settings — position from the
/// outer rect, size from the inner (content) rect — like the main-window
/// size in `App::update`. Wayland reports no rects; nothing is persisted
/// there.
fn persist_geometry(app: &mut App, ctx: &egui::Context) {
    let (inner, outer) = ctx.input(|input| {
        let viewport = input.viewport();
        (viewport.inner_rect, viewport.outer_rect)
    });
    let Some(size) = inner.map(|rect| rect.size()) else {
        return;
    };
    let Some(outer) = outer else {
        return;
    };
    let geometry = [outer.left(), outer.top(), size.x, size.y];
    if app.settings.report_window_geometry != Some(geometry) {
        app.settings.report_window_geometry = Some(geometry);
        app.mark_settings_dirty();
    }
}

/// The report body shared by the viewport and the embedded fallback:
/// totals block, export row, height-bounded file table, notices block.
fn report_body(app: &mut App, ui: &mut egui::Ui) {
    let Some(report) = app.report.clone() else {
        ui.label("No finished run yet.");
        return;
    };
    // plan 10 §phase 2: aggregated quality metrics over the measured
    // files (mean per engine), if any were measured
    let metric_line = app.metrics.aggregate();
    totals_block(ui, &report, metric_line.as_deref());
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
    // The scroll height is explicitly **bounded** (window height minus
    // the blocks below it): an unbounded `auto_shrink` list requested the
    // full content height, so the window's minimum size equaled the whole
    // list and every drag-resize snapped back (the plan-09 bug).
    let rows = report.files.len();
    let remaining = ui.available_height();
    egui::ScrollArea::vertical()
        .max_height((remaining - NOTICES_RESERVE).max(MIN_TABLE_HEIGHT))
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
                    let width =
                        (ui.available_width() - sizes_width - status_width - spacing - 14.0)
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
                        Outcome::SkippedCollision { input_size, .. } => format_size(*input_size),
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
                    let status_response = ui.add_sized([status_width, ROW_HEIGHT], status_label);
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
        .max_height(NOTICES_MAX_HEIGHT)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if notices.is_empty() {
                ui.weak("(none)");
            }
            for notice in notices {
                ui.label(egui::RichText::new(notice).weak().size(12.0));
            }
        });
}

/// Aggregated totals (the "Encode statistics" numbers) plus the optional
/// plan-10 quality-metric mean line.
fn totals_block(ui: &mut egui::Ui, report: &RunReport, metric_line: Option<&str>) {
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
    // plan 10 §phase 1: the percentage comes from the shared ratio
    // helper, so the report shows the exact string the footer shows
    ui.label(format!(
        "{} → {} ({}) in {:.1}s",
        format_size(report.totals.input_size),
        format_size(report.totals.output_size),
        crate::ratio::ratio_label(report.totals.input_size, report.totals.output_size),
        report.elapsed.as_secs_f32()
    ));
    if report.metadata_dropped_count > 0 {
        ui.weak(format!(
            "{} outputs could not carry EXIF metadata (target format has no support)",
            report.metadata_dropped_count
        ));
    }
    if let Some(line) = metric_line {
        ui.weak(line);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_builder_restores_persisted_geometry() {
        let plain = viewport_builder(&Settings::default());
        assert_eq!(plain.title.as_deref(), Some(WINDOW_TITLE));
        assert_eq!(plain.position, None);
        assert_eq!(
            plain.inner_size,
            Some(egui::vec2(DEFAULT_INNER_SIZE[0], DEFAULT_INNER_SIZE[1]))
        );

        let settings = Settings {
            report_window_geometry: Some([12.0, 34.0, 800.0, 600.0]),
            ..Settings::default()
        };
        let restored = viewport_builder(&settings);
        assert_eq!(restored.position, Some(egui::Pos2::new(12.0, 34.0)));
        assert_eq!(restored.inner_size, Some(egui::vec2(800.0, 600.0)));
    }
}
