//! Entry point of the `byteshaver-gui` desktop application (plan WS8).
//!
//! Boots the egui/eframe runtime with the Concept-1 queue-centric single
//! window and hands over to [`app::App`]. The window title is `byteshaver`;
//! the default (and persisted) viewport size is restored from the settings
//! file before the event loop starts.
//!
//! All application modules live in the `byteshaver_gui` library crate
//! (`src/lib.rs`) so tests and the capture binary (plan 17) can reuse them.

#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use byteshaver_gui::{app, install_symbol_fonts, settings};

use eframe::egui;

fn main() -> eframe::Result<()> {
    // settings are loaded before the viewport is created so the persisted
    // window size can seed the native options
    let settings = settings::Settings::load_or_default();
    let initial_size = settings.window_size.unwrap_or([1100.0, 720.0]);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("byteshaver")
            .with_inner_size(egui::vec2(initial_size[0], initial_size[1])),
        ..Default::default()
    };

    eframe::run_native(
        "byteshaver",
        options,
        Box::new(move |creation_context| {
            install_symbol_fonts(&creation_context.egui_ctx);
            Ok(Box::new(app::App::with_settings(settings)))
        }),
    )
}

