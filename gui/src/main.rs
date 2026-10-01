//! Entry point of the `byteshaver-gui` desktop application (plan WS8).
//!
//! Boots the egui/eframe runtime with the Concept-1 queue-centric single
//! window and hands over to [`app::App`]. The window title is `byteshaver`;
//! the default (and persisted) viewport size is restored from the settings
//! file before the event loop starts.

#![deny(missing_docs)]
// GUI application: no console subsystem in release builds on Windows
// (otherwise launching from Explorer allocates an empty terminal window
// next to the GUI window). Debug builds keep the console so `eprintln!`
// diagnostics stay visible during development.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod celebrate;
mod chips;
mod options;
mod panels;
mod queue;
mod reporter;
mod settings;

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

/// Installs system symbol/emoji fonts as the *last* fallback for both font
/// families. egui's bundled fonts (Ubuntu/Hack/NotoEmoji) miss glyphs for
/// several status symbols this GUI uses (⤷ ✂ ⏸ ⟳ ⤬ ⬇), which render as
/// tofu boxes — notably on Windows. Missing font files are silently
/// skipped (the defaults stay in effect), so this is best-effort.
fn install_symbol_fonts(ctx: &egui::Context) {
    let candidates: &[&str] = if cfg!(target_os = "windows") {
        &[
            "C:\\Windows\\Fonts\\seguisym.ttf", // Segoe UI Symbol: ── misc symbols/dingbats
            "C:\\Windows\\Fonts\\seguiemj.ttf", // Segoe UI Emoji
        ]
    } else if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/Apple Symbols.ttf",
            "/System/Library/Fonts/Symbol.ttf",
        ]
    } else {
        &[
            "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ]
    };

    let mut fonts = egui::FontDefinitions::default();
    for (index, path) in candidates.iter().enumerate() {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        let name = format!("byteshaver-symbols-{index}");
        fonts.font_data.insert(
            name.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(data)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            if let Some(list) = fonts.families.get_mut(&family) {
                list.push(name.clone());
            }
        }
    }
    ctx.set_fonts(fonts);
}
