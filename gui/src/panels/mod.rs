//! Panel renderers of the Concept-1 queue-centric single window
//! (plan WS8 §3/§4). Every module here is **rendering only**: they read
//! app state, dispatch user intent back onto `App` methods and never own
//! product logic, which keeps the state machine in `app`/`queue` testable
//! headless.
//!
//! Layout (top to bottom): header bar, drop-zone banner + file table
//! (central), options & policies panel, run footer. Optional windows:
//! about; the report renders in its own OS viewport (plan 09).

pub mod about;
pub mod drop_zone;
pub mod file_table;
pub mod footer;
pub mod inspector;
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
    // the run report lives in its own OS viewport (plan 09); starting a
    // new run clears `report`, which hides the viewport until the run
    // finishes, then re-renders the same viewport id with fresh contents
    if app.show_report && app.report.is_some() {
        report::show_window(app, ctx);
    }
    // the visual difference inspector (plan 10 §phase 3): a bounded
    // window whose state (buffers + textures) lives on the app and is
    // dropped when the window closes
    if app.inspector.is_some() {
        inspector::InspectorState::show(app, ctx);
    }
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
