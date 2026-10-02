//! `bh-gui-capture` — headless GUI capture CLI (plan 17 §6, P2).
//!
//! Runs the built-in capture scenes of [`byteshaver_gui::capture`]
//! (egui_kittest harness, software rasterizer — no display server) and
//! writes PNG stills plus optional per-frame PNG sequences for the README
//! media pipeline. Invoked by `cargo xtask media` from the staged fixture
//! directory (plan 17 §4); every scene runs against a fresh first-run
//! [`byteshaver_gui::settings::Settings`]-default app, never the
//! developer's on-disk config.
//!
//! Artifact layout (the xtask contract):
//! - stills: `<out-dir>/<snapshot-name>.png` (master resolution =
//!   logical window size × `--points-per-pixel`)
//! - frames: `<frames-dir>/<scene-id>/NNNN.png` (numbering starts at
//!   `0000`, fps printed per scene for the ffmpeg encode step)
//!
//! Config isolation: unless the caller already provides
//! `XDG_CONFIG_HOME`/`XDG_CACHE_HOME` (the xtask does, plan 17 §4), both
//! are pointed at a per-process scratch dir so scene-triggered settings
//! saves and the preset store never touch the developer's real GUI state
//! (Linux — the pipeline is Linux-only, plan 17 §0).

#![deny(missing_docs)]

use std::path::PathBuf;

use clap::Parser;

use byteshaver_gui::capture::{Scene, SceneOptions, SceneRunner, scenes};

/// CLI of the capture binary.
#[derive(Parser, Debug)]
#[command(
    name = "bh-gui-capture",
    about = "Headless GUI scene capture for the byteshaver README media pipeline (plan 17).",
    version
)]
struct Args {
    /// List the available scene ids with their descriptions and exit.
    #[arg(long)]
    list: bool,
    /// Scene id to run (repeatable; see --list).
    #[arg(long = "scene", value_name = "ID")]
    scenes: Vec<String>,
    /// Run every registered scene.
    #[arg(long)]
    all: bool,
    /// Directory the still PNGs are written to (as <name>.png).
    #[arg(long, value_name = "DIR", default_value = "out/gui")]
    out_dir: PathBuf,
    /// Enables per-frame dumps under <DIR>/<scene-id>/NNNN.png for the
    /// whole scene duration.
    #[arg(long, value_name = "DIR")]
    frames_dir: Option<PathBuf>,
    /// Logical window size, as WxH in points (1100x720).
    #[arg(long, value_name = "WxH", default_value = "1100x720")]
    window_size: String,
    /// Device pixel ratio of the masters (logical size × this = master
    /// pixels; 2.0 = HiDPI masters for the 2x→1x downscale).
    #[arg(
        long = "points-per-pixel",
        visible_alias = "ppp",
        value_name = "F",
        default_value_t = 2.0
    )]
    points_per_pixel: f32,
    /// Directory with symbol fonts, handed to the GUI via BH_GUI_FONT_DIR
    /// before the app is created (committed Noto Sans Symbols 2 + DejaVu
    /// copies for reproducible glyph rendering).
    #[arg(long, value_name = "DIR")]
    fonts_dir: Option<PathBuf>,
    /// Restore egui animations for the scenes (the harness zeroes
    /// animation_time for deterministic stills; video scenes enable this
    /// themselves via the Animate step).
    #[arg(long)]
    animate: bool,
}

fn main() {
    let args = Args::parse();

    if args.list {
        for scene in scenes() {
            // the [video] tag is part of the xtask contract: the media
            // pipeline passes --frames-dir only for video scenes
            let tag = if scene.video { " [video]" } else { "" };
            println!("{:<14}{} {}", scene.id, tag, scene.description);
        }
        return;
    }

    let window_size = parse_window_size(&args.window_size).unwrap_or_else(|err| {
        eprintln!("bh-gui-capture: {err}");
        std::process::exit(2);
    });

    // font override must happen before the first harness/app construction
    if let Some(dir) = &args.fonts_dir {
        if !dir.is_dir() {
            eprintln!(
                "bh-gui-capture: fonts dir {} does not exist",
                dir.display()
            );
            std::process::exit(2);
        }
        // single-threaded startup: process-env mutation is sound here;
        // `install_symbol_fonts` reads the variable when the app is built
        // SAFETY: single-threaded startup, before any other thread
        // (let alone the harness workers) exists.
        unsafe {
            std::env::set_var("BH_GUI_FONT_DIR", dir);
        }
    }
    isolate_config_dirs();

    let selected = select_scenes(&args).unwrap_or_else(|err| {
        eprintln!("bh-gui-capture: {err}");
        std::process::exit(2);
    });
    if selected.is_empty() {
        eprintln!(
            "bh-gui-capture: nothing to do — pass --scene <id> (repeatable) or --all; \
             --list shows the registry"
        );
        std::process::exit(2);
    }

    let options = SceneOptions {
        window_size,
        pixels_per_point: args.points_per_pixel,
        snapshot_dir: Some(args.out_dir.clone()),
        frames_dir: args.frames_dir.clone(),
    };

    let mut failures = 0;
    for scene in selected {
        match run_scene(scene, &options, args.animate) {
            Ok(result) => {
                println!(
                    "scene {}: {} still(s) in {}, {} frame(s) @ {} fps (master {}x{} px)",
                    scene.id,
                    result.stills.len(),
                    args.out_dir.display(),
                    result.frames,
                    result.fps,
                    window_size[0] * args.points_per_pixel,
                    window_size[1] * args.points_per_pixel,
                );
                for name in result.stills {
                    println!("  still: {}", args.out_dir.join(format!("{name}.png")).display());
                }
            }
            Err(err) => {
                failures += 1;
                eprintln!("scene {}: FAILED: {err:#}", scene.id);
            }
        }
    }
    if failures > 0 {
        std::process::exit(1);
    }
}

/// Result summary of one scene run (the CLI's per-scene report line).
struct SceneResult {
    /// Snapshot names rendered by the scene (stills under `--out-dir`).
    stills: Vec<String>,
    /// Frames dumped under `--frames-dir/<scene-id>/`.
    frames: u64,
    /// fps the dumped frame sequence encodes at (FrameRate steps may
    /// change it mid-scene; the post-processing step reads this).
    fps: u32,
}

/// Runs one scene in a fresh harness.
fn run_scene(scene: &Scene, options: &SceneOptions, animate: bool) -> anyhow::Result<SceneResult> {
    let mut runner = SceneRunner::new(scene.id, options);
    if animate {
        runner.set_animate(true);
    }
    runner.run(&scene.steps)?;
    Ok(SceneResult {
        stills: runner.snapshots().iter().map(|(name, _)| name.clone()).collect(),
        frames: runner.frame_count(),
        fps: runner.fps(),
    })
}

/// Parses the `WxH` window-size argument.
fn parse_window_size(text: &str) -> Result<[f32; 2], String> {
    let (width, height) = text
        .split_once('x')
        .ok_or_else(|| format!("invalid --window-size {text:?}: expected WxH (e.g. 1100x720)"))?;
    let width: f32 = width
        .trim()
        .parse()
        .map_err(|_| format!("invalid --window-size width in {text:?}"))?;
    let height: f32 = height
        .trim()
        .parse()
        .map_err(|_| format!("invalid --window-size height in {text:?}"))?;
    if width <= 0.0 || height <= 0.0 {
        return Err(format!("--window-size must be positive, got {text:?}"));
    }
    Ok([width, height])
}

/// Resolves the requested scene set (deduplicated, registry order).
fn select_scenes(args: &Args) -> Result<Vec<&'static Scene>, String> {
    let registry = scenes();
    let mut selected: Vec<&'static Scene> = Vec::new();
    let push = |id: &str, selected: &mut Vec<&'static Scene>| -> Result<(), String> {
        let scene = registry
            .iter()
            .find(|scene| scene.id == id)
            .ok_or_else(|| format!("unknown scene {id:?} (--list shows the registry)"))?;
        if !selected.iter().any(|selected| selected.id == scene.id) {
            selected.push(scene);
        }
        Ok(())
    };
    if args.all {
        selected.extend(registry.iter());
    }
    for id in &args.scenes {
        push(id, &mut selected)?;
    }
    Ok(selected)
}

/// Keeps the capture run from touching the developer's real GUI state:
/// scene-triggered edits mark the settings dirty and `App::save_settings`
/// writes them to the OS config dir; the preset store reads the same
/// place. When the caller (xtask, plan 17 §4) has not already provided
/// isolated `XDG_CONFIG_HOME`/`XDG_CACHE_HOME`, both are pointed at a
/// per-process scratch directory. Linux-only in practice (the pipeline is
/// Linux-only, plan 17 §0); on other desktops run captures with explicit
/// isolated XDG vars.
fn isolate_config_dirs() {
    let scratch = std::env::temp_dir().join(format!("bh-gui-capture-{}", std::process::id()));
    for var in ["XDG_CONFIG_HOME", "XDG_CACHE_HOME"] {
        if std::env::var_os(var).is_none() {
            let dir = scratch.join(var.to_ascii_lowercase());
            if std::fs::create_dir_all(&dir).is_ok() {
                // SAFETY: single-threaded startup, before the harness
                // spawns its workers.
                unsafe {
                    std::env::set_var(var, &dir);
                }
            }
        }
    }
}
