//! Panel renderers of the Concept-1 queue-centric single window
//! (plan WS8 §3/§4). Every module here is **rendering only**: they read
//! app state, dispatch user intent back onto `App` methods and never own
//! product logic, which keeps the state machine in `app`/`queue` testable
//! headless.
//!
//! Layout (top to bottom): header bar, drop-zone banner + file table
//! (central), options & policies panel, run footer. Optional windows:
//! report and about.

pub mod about;
pub mod drop_zone;
pub mod file_table;
pub mod footer;
pub mod options_panel;
pub mod report;

use crate::app::App;

/// Renders one full frame: all panels plus the optional windows.
///
/// Panel order: header (top), footer + options (bottom), central panel
/// with the drop-zone banner and the queue table. Both central pieces
/// render inside **one** `CentralPanel` (egui gives each `CentralPanel`
/// the whole remaining rect, so a second one would be invisible).
pub fn show(app: &mut App, ctx: &egui::Context) {
    header(app, ctx);
    footer::show(app, ctx);
    options_panel::show(app, ctx);
    egui::CentralPanel::default().show(ctx, |ui| {
        drop_zone::show(app, ui);
        file_table::show(app, ui);
    });
    report::show_window(app, ctx);
    about::show_window(app, ctx);
}

/// Header bar: app title plus report/about toggles.
fn header(app: &mut App, ctx: &egui::Context) {
    egui::TopBottomPanel::top("byteshaver-header").show(ctx, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.set_height(24.0);
            ui.heading("byteshaver");
            ui.weak(format!("v{}", crate::app::CORE_VERSION));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("About").clicked() {
                    app.show_about = !app.show_about;
                }
                if ui
                    .add_enabled(app.report.is_some(), egui::Button::new("Report"))
                    .on_disabled_hover_text("no finished run yet")
                    .clicked()
                {
                    app.show_report = !app.show_report;
                }
            });
        });
    });
}
