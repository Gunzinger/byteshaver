//! Run footer: Convert/Cancel buttons, live progress and the aggregate
//! statistics (fed by `JobEvent::ProgressStats` while running and by the
//! final `RunReport` afterwards — identical math to the CLI summary).

use crate::app::{App, FooterStats};
use crate::queue::format_size;

/// Renders the footer panel (run controls + progress + totals).
pub fn show(app: &mut App, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("byteshaver-footer").show(ctx, |ui| {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            // run controls -------------------------------------------------
            let mut convert = ui.add_enabled(app.can_start(), egui::Button::new("▶ Convert"));
            if let Some(reason) = app.start_blocker() {
                convert = convert.on_disabled_hover_text(reason);
            }
            if convert.clicked() {
                app.start_job();
            }
            let cancel_enabled = app.running.as_ref().is_some_and(|job| !job.stop.raised());
            if ui
                .add_enabled(cancel_enabled, egui::Button::new("✕ Cancel"))
                .on_disabled_hover_text("no running job")
                .clicked()
            {
                app.cancel_job();
            }

            // progress ------------------------------------------------------
            if let Some(job) = &app.running {
                let (fraction, text) = match job.progress() {
                    Some(fraction) => (
                        fraction,
                        format!(
                            "{}/{} files",
                            job.finished_items,
                            job.total_items.unwrap_or(0)
                        ),
                    ),
                    None => (0.0, "discovering files…".to_string()),
                };
                let bar = egui::ProgressBar::new(fraction).text(text);
                ui.add_sized([ui.available_width() * 0.4, 18.0], bar);
            }

            // aggregate statistics ------------------------------------------
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let stats_text = if let Some(job) = &app.running {
                    stats_line(&job.stats, Some(job.started_at.elapsed()))
                } else if let Some(report) = &app.report {
                    stats_line(
                        &FooterStats {
                            input_bytes: report.totals.input_size,
                            output_bytes: report.totals.output_size,
                            ok: report.totals.successful,
                            skipped: report.totals.skipped,
                            errors: report.totals.errors,
                        },
                        Some(report.elapsed),
                    )
                } else {
                    String::new()
                };
                if !stats_text.is_empty() {
                    ui.label(egui::RichText::new(stats_text).weak());
                }
            });
        });
        ui.add_space(2.0);
        // start blockers visible in full when present
        if let Some(error) = &app.start_error {
            ui.colored_label(ui.visuals().error_fg_color, error.clone());
        }
        ui.add_space(2.0);
    });
}

/// Footer statistics line (same numbers as the CLI progress bar message).
fn stats_line(stats: &FooterStats, elapsed: Option<std::time::Duration>) -> String {
    let mut line = format!(
        "{} → {} | ✔ {} — {} ✖ {}",
        format_size(stats.input_bytes),
        format_size(stats.output_bytes),
        stats.ok,
        stats.skipped,
        stats.errors
    );
    if let Some(elapsed) = elapsed {
        let secs = elapsed.as_secs_f32();
        line.push_str(&format!(" · {secs:.1}s"));
    }
    line
}
