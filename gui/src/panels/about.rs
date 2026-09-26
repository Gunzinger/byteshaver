//! About window: core version (lockstep with the gui crate) plus the
//! capability summary — every registry encoder with its `describe()` line
//! and compile-time availability, and the HEIF input status. The same
//! information a fresh CLI build prints, rendered as the GUI's
//! "which encoders does this binary have" reference.

use crate::app::{App, CORE_VERSION};

/// Renders the about window when open.
pub fn show_window(app: &mut App, ctx: &egui::Context) {
    let mut open = app.show_about;
    egui::Window::new("About byteshaver")
        .open(&mut open)
        .default_width(560.0)
        .show(ctx, |ui| {
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

            egui::ScrollArea::vertical()
                .id_salt("about-encoders")
                .max_height(260.0)
                .show(ui, |ui| {
                    for info in &app.capabilities.encoders {
                        ui.horizontal_top(|ui| {
                            let status = if info.enabled {
                                egui::RichText::new("✔")
                                    .color(egui::Color32::from_rgb(90, 190, 110))
                            } else {
                                egui::RichText::new("✖").color(ui.visuals().weak_text_color())
                            };
                            ui.monospace(status);
                            let mut label =
                                egui::RichText::new(format!("{} (.{})", info.name, info.extension))
                                    .monospace();
                            if !info.enabled {
                                label = label.weak();
                            }
                            let response = ui.label(label);
                            if !info.enabled {
                                response
                                    .on_hover_text(info.disabled_reason.unwrap_or("not available"));
                            }
                            if info.supports_animation {
                                ui.weak("anim");
                            }
                            if info.supports_metadata {
                                ui.weak("exif");
                            }
                        });
                        ui.weak(egui::RichText::new(info.description.as_str()).size(12.0));
                        ui.add_space(2.0);
                    }
                });
            ui.separator();
            ui.weak("GUI and CLI consume the same byteshaver core; conversions are identical.");
        });
    app.show_about = open;
}
