//! Headless render smoke test for the capture pipeline (plan 17 §6, P0 spike).
//!
//! Boots the real [`byteshaver_gui::app::App`] in an `egui_kittest` harness
//! (wgpu on the software rasterizer / lavapipe — no display server, no
//! window) and renders one settled empty-state frame to a PNG. This guards
//! the exact rendering path `bh-gui-capture` uses for the README assets.
//!
//! The output image is written to `target/kittest-spike/gui-empty.png`
//! (never committed); the assertions are pixel-statistics based so the test
//! stays independent of minor style tweaks.

use std::path::PathBuf;

use byteshaver_gui::{app::App, install_symbol_fonts, settings::Settings};
use egui_kittest::Harness;

/// Renders the empty first-run state and saves it under
/// `target/kittest-spike/`.
#[test]
fn empty_state_renders_headlessly() {
    let settings = Settings {
        window_size: Some([1100.0, 720.0]),
        ..Settings::default()
    };

    let mut harness: Harness<App> = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .with_pixels_per_point(1.0)
        .build_eframe(|cc| {
            install_symbol_fonts(&cc.egui_ctx);
            App::with_settings(settings)
        });

    // settle animations/repaints; a small step budget keeps this bounded
    harness.run_steps(30);

    let image = harness
        .render()
        .expect("software rasterizer must render the empty state");

    // sanity: the frame is the requested size and not blank
    assert_eq!(image.width(), 1100, "render width must match the harness size");
    assert_eq!(image.height(), 720, "render height must match the harness size");
    let non_background = image
        .pixels()
        .filter(|px| px.0[0] < 250 || px.0[1] < 250 || px.0[2] < 250)
        .count();
    assert!(
        non_background > 10_000,
        "expected substantial ui content, got {non_background} non-background pixels"
    );

    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("kittest-spike/gui-empty.png");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    image.save(&out).expect("save spike png");
    println!("wrote {}", out.display());
}
