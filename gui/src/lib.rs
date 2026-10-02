//! Library crate of `byteshaver-gui` (plan WS8, plan 17 capture pipeline).
//!
//! Exposes the application modules so that:
//! - the thin `byteshaver-gui` binary (`src/main.rs`) can boot the app,
//! - integration tests and the feature-gated `bh-gui-capture` capture binary
//!   (plan 17 §6) can drive the real [`app::App`] headlessly.

#![deny(missing_docs)]

pub mod app;
/// Headless capture pipeline (plan 17 §6, feature `capture`): the scene
/// runner that drives [`app::App`] through `egui_kittest`, the viewport
/// adapter switch and the built-in scene registry; the `bh-gui-capture`
/// binary (`src/bin/bh-gui-capture.rs`) is its CLI.
#[cfg(feature = "capture")]
pub mod capture;
pub mod celebrate;
pub mod chips;
pub mod metrics;
pub mod options;
pub mod panels;
pub mod platform;
pub mod presets;
pub mod queue;
pub mod ratio;
pub mod reporter;
pub mod settings;
pub mod table;
pub mod thumb;
pub mod viewports;

use eframe::egui;

/// Installs symbol/emoji fonts as the *last* fallback for both font
/// families. egui's bundled fonts (Ubuntu/Hack/NotoEmoji) miss glyphs for
/// several status symbols this GUI uses (⤷ ✂ ⏸ ⟳ ⤬ ⬇), which render as
/// tofu boxes — notably on Windows. Missing font files are silently
/// skipped (the defaults stay in effect), so this is best-effort.
///
/// Candidate resolution order:
/// 1. every `*.ttf`/`*.otf` file in the directory named by the
///    `BH_GUI_FONT_DIR` environment variable (plan 17 §6.2: the capture
///    pipeline points this at the committed `docs/media/fonts/symbols/`
///    copies so headless renders are independent of the host's fonts),
/// 2. the usual per-OS system font paths.
pub fn install_symbol_fonts(ctx: &egui::Context) {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(dir) = std::env::var("BH_GUI_FONT_DIR") {
        let dir = std::path::PathBuf::from(dir);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut paths: Vec<std::path::PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    matches!(
                        path.extension().and_then(|ext| ext.to_str()),
                        Some("ttf") | Some("otf")
                    )
                })
                .collect();
            paths.sort();
            candidates.extend(
                paths
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned()),
            );
        }
    }
    candidates.extend(
        if cfg!(target_os = "windows") {
            vec![
                "C:\\Windows\\Fonts\\seguisym.ttf", // Segoe UI Symbol: ── misc symbols/dingbats
                "C:\\Windows\\Fonts\\seguiemj.ttf", // Segoe UI Emoji
            ]
        } else if cfg!(target_os = "macos") {
            vec![
                "/System/Library/Fonts/Apple Symbols.ttf",
                "/System/Library/Fonts/Symbol.ttf",
            ]
        } else {
            vec![
                "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            ]
        }
        .into_iter()
        .map(str::to_owned),
    );

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
