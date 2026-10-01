//! About window: core version (lockstep with the gui crate) plus the
//! capability summary — every registry encoder with its `describe()` line
//! and compile-time availability, and the HEIF input status. The same
//! information a fresh CLI build prints, rendered as the GUI's
//! "which encoders does this binary have" reference.
//!
//! Hosted by [`crate::viewports`] (plan 15 F12): an independent OS
//! viewport with an `Embedded` fallback (`egui::Window`, plan 09
//! doctrine).

use crate::app::{App, CORE_VERSION};
use crate::viewports::PopupKind;

/// Renders the embedded fallback window (`ViewportClass::Embedded`
/// backends, plan 09 doctrine).
pub fn show_embedded_window(app: &mut App, ctx: &egui::Context) {
    let mut open = app.show_about;
    egui::Window::new(PopupKind::About.title())
        .open(&mut open)
        .default_width(PopupKind::About.default_size()[0])
        .show(ctx, |ui| about_body(app, ui));
    app.show_about = open;
}

/// Renders the contents of the standalone viewport (called by
/// [`crate::viewports`] while open): the About body in a `CentralPanel`
/// of the viewport's own context, plus the OS-title-bar close handling.
pub fn show_viewport_contents(app: &mut App, ctx: &egui::Context) {
    // closing via the OS title bar flips the flag off; the host hides the
    // viewport on the next frame and the header button reflects the state
    if ctx.input(|input| input.viewport().close_requested()) {
        app.show_about = false;
        return;
    }
    egui::CentralPanel::default().show(ctx, |ui| about_body(app, ui));
}

/// The window body shared by the viewport and the embedded fallback.
fn about_body(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("byteshaver-gui");
    ui.label(format!(
        "core version {CORE_VERSION} · desktop front-end (egui/eframe)"
    ));
    ui.weak("A configurable and efficient batch image converter.");
    ui.hyperlink_to(
        "github.com/Gunzinger/byteshaver",
        "https://github.com/Gunzinger/byteshaver",
    );
    ui.separator();

    ui.heading("Build capabilities");
    ui.label(if app.capabilities.heif_input_enabled {
        "HEIF/HEIC/AVIF container input: enabled (dec-heif)"
    } else {
        "HEIF/HEIC/AVIF container input: disabled (dec-heif not compiled in)"
    });
    ui.add_space(4.0);

    // bounded by the window: whatever height is left minus the trailing
    // separator/line (the encoder list can grow with the registry — no
    // fixed cap, plan 15 F13 doctrine; the floor keeps tiny windows sane)
    egui::ScrollArea::vertical()
        .id_salt("about-encoders")
        .max_height((ui.available_height() - 40.0).max(120.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for info in &app.capabilities.encoders {
                encoder_row(ui, info);
                ui.add_space(2.0);
            }
        });
    ui.separator();
    ui.weak("GUI and CLI consume the same byteshaver core; conversions are identical.");
}

/// One encoder row of the capabilities list.
fn encoder_row(ui: &mut egui::Ui, info: &byteshaver::job::EncoderInfo) {
    ui.horizontal_top(|ui| {
        let status = if info.enabled {
            egui::RichText::new("✔").color(egui::Color32::from_rgb(90, 190, 110))
        } else {
            egui::RichText::new("✖").color(ui.visuals().weak_text_color())
        };
        ui.monospace(status);
        let mut label =
            egui::RichText::new(format!("{} (.{})", info.name, info.extension)).monospace();
        if !info.enabled {
            label = label.weak();
        }
        let response = ui.label(label);
        if !info.enabled {
            response.on_hover_text(info.disabled_reason.unwrap_or("not available"));
        }
        if info.supports_animation {
            ui.weak("anim");
        }
        if info.supports_metadata {
            ui.weak("exif");
        }
    });
    ui.weak(egui::RichText::new(info.description.as_str()).size(12.0));
}
