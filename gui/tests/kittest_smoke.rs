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

/// Runs the `gui-empty` capture scene end-to-end through the scene runner
/// (only built with `--features capture`, like `bh-gui-capture`) and
/// guards the plan-17 §6.2 viewport adapter: closed popups must not paint
/// stacked embedded window shells into the single harness canvas. Before
/// the adapter, every `show_viewport_immediate` call became an empty
/// `egui::Window` shell in the harness — one visible egui area per hosted
/// popup, five shells stacked over the main window. The adapted in-canvas
/// popups leave exactly the app's own base layer (measured: 1 clean vs 6
/// broken on egui 0.36), so the assertion is on the visible-area count —
/// crisp and independent of where the shells would happen to paint.
#[cfg(feature = "capture")]
#[test]
fn gui_empty_scene_renders_without_viewport_artifacts() {
    use byteshaver_gui::capture::{SceneOptions, SceneRunner, scene};

    let scene = scene("gui-empty").expect("gui-empty is registered");
    let mut runner = SceneRunner::new(
        scene.id,
        &SceneOptions {
            window_size: [1100.0, 720.0],
            pixels_per_point: 1.0,
            snapshot_dir: None,
            frames_dir: None,
        },
    );
    runner.run(&scene.steps).expect("scene executes in-memory");
    let image = runner.render().expect("the scene renders");

    assert_eq!(image.width(), 1100, "render width matches the harness size");
    assert_eq!(image.height(), 720, "render height matches the harness size");

    // no popup shells: the empty state hosts all five popups closed (see
    // `viewports::PopupKind`); the single remaining visible area is the
    // app's own base layer
    let areas = runner.visible_area_count();
    assert!(
        areas <= 2,
        "expected only the app's own area layer, got {areas} visible areas — \
         stacked viewport shells are leaking into the capture canvas"
    );
}
