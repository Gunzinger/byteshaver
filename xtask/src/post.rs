//! Post-processing of capture artifacts into final assets (plan 17 §7).
//!
//! This is the "image processing" leg of the pipeline: ffmpeg for the
//! downscales, compositing and the animated WebP encode, plus the freshly
//! built `byteshaver` binary itself for the still WebPs (dogfooding - the
//! pipeline's own product optimizes the README's images).
//!
//! # Artifact layout contract (agreed with the capture engines)
//!
//! Three artifact flavors exist; every one maps to final assets under
//! `<stage>/final/`:
//!
//! ```text
//! A. GUI stills (bh-gui-capture, 2x HiDPI masters):
//!    out/<snapshot-name>.png
//!      -> ffmpeg scale=iw/2:ih/2:flags=lanczos -> post-tmp/<name>.png
//!      -> bin/byteshaver post-tmp/<name>.png webp -q 92 -o final
//!      => final/<snapshot-name>.webp
//!    Tour-beat stills (`gui-tour-*`) are development artifacts and are
//!    skipped (the tour lives on as its video).
//!
//! B. GUI videos (bh-gui-capture --frames-dir, 2x masters, %04d from 0000):
//!    out/<scene-id>/0000.png, 0001.png, ...
//!      -> ffmpeg -start_number 0 -framerate <scene fps>
//!              -vf scale=iw/2:ih/2:flags=lanczos
//!              -c:v libwebp -lossless 0 -q:v 62 -loop 0
//!      => final/<scene-id>.webp
//!
//! C. Terminal scenes (VHS tapes, 1x):
//!    tapes/out/<scene>/still.png          (decorated: margin + window bar)
//!      -> byteshaver webp -q 92           => final/<scene>.webp
//!    tapes/out/<scene>/frames/frame-text-%05d.png
//!                       frames/frame-cursor-%05d.png
//!      (vhs emits two interleaved sequences; the cursor is composited
//!       over the text layer; no downscale - the frames are already 1x)
//!      => final/<scene>-demo.webp
//! ```
//!
//! Master PNGs stay in `target/` only; the repo commits finals (plan 17 §7).

use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::media::MediaOpts;
use crate::stage::Stage;
use crate::tools;
use crate::util;

/// Runs post-processing over every artifact flavor (see the module docs).
pub fn run(stage: &Stage, opts: &MediaOpts) -> Result<()> {
    // Tools are checked once, upfront: when artifacts exist, a missing
    // ffmpeg is a hard error, not a skip.
    let ffmpeg = tools::lookup("ffmpeg");
    let byteshaver = stage.bin("byteshaver");
    let any_artifacts = util::list_files(&stage.out_dir())?.len() > 0
        || util::list_dirs(&stage.out_dir())?.len() > 0
        || stage
            .tapes_dir()
            .join("out")
            .is_dir();
    if !any_artifacts {
        println!("[post] no capture artifacts under out/ or tapes/out/ (capture stages produced nothing) - nothing to post-process");
        return Ok(());
    }
    let ffmpeg = ffmpeg.ok_or_else(|| {
        anyhow::anyhow!(
            "[post] 'ffmpeg' not found{} - required to post-process the capture artifacts",
            tools_path_hint()
        )
    })?;
    if !byteshaver.is_file() {
        bail!(
            "[post] staged byteshaver missing at {} - run the 'stage' phase first",
            byteshaver.display()
        );
    }

    // A. GUI stills: flat out/*.png (2x masters)
    for name in util::list_files(&stage.out_dir())? {
        if !name.ends_with(".png") {
            continue;
        }
        if is_tour_beat(&name) {
            println!(
                "[post] still '{name}': tour beat snapshot - skipped (the tour lives on as its video)"
            );
            continue;
        }
        let scene = name.trim_end_matches(".png").to_string();
        process_still_scaled(stage, &ffmpeg, &byteshaver, &stage.out_dir().join(&name), &scene)
            .with_context(|| format!("post-processing gui still '{name}'"))?;
    }

    // B. GUI videos: out/<scene>/NNNN.png (%04d from 0000, 2x masters)
    for scene in util::list_dirs(&stage.out_dir())? {
        if scene == "capture-run" {
            // the scenes' conversion *outputs* (what the demo runs encoded),
            // not capture artifacts - never post-processed
            continue;
        }
        if opts.skip_video {
            println!("[post] scene '{scene}': frames present but --skip-video set - skipped");
            continue;
        }
        encode_gui_frames(stage, &ffmpeg, &scene)
            .with_context(|| format!("post-processing gui frames of scene '{scene}'"))?;
        if opts.gif {
            encode_gif(stage, &ffmpeg, &scene)
                .with_context(|| format!("encoding GIF variant of scene '{scene}'"))?;
        }
    }

    // C. Terminal scenes: tapes/out/<scene>/{still.png,frames/}
    let tapes_out = stage.tapes_dir().join("out");
    if tapes_out.is_dir() {
        for scene in util::list_dirs(&tapes_out)? {
            let scene_dir = tapes_out.join(&scene);
            let still = scene_dir.join("still.png");
            if still.is_file() {
                process_still_1x(stage, &byteshaver, &still, &scene)
                    .with_context(|| format!("post-processing terminal still '{scene}'"))?;
            }
            let frames = scene_dir.join("frames");
            if frames.is_dir() {
                if opts.skip_video {
                    println!(
                        "[post] scene '{scene}': terminal frames present but --skip-video set - skipped"
                    );
                } else {
                    encode_terminal_frames(stage, &ffmpeg, &scene)
                        .with_context(|| format!("post-processing terminal frames of '{scene}'"))?;
                }
            }
        }
    }
    Ok(())
}

/// Tour-beat stills (`gui-tour-00-empty.png`, ...) are intermediate
/// development artifacts, not README assets.
fn is_tour_beat(file_name: &str) -> bool {
    file_name.starts_with("gui-tour-")
}

/// GUI still (2x master): downscale + dogfooded WebP => `final/<scene>.webp`.
fn process_still_scaled(
    stage: &Stage,
    ffmpeg: &Path,
    byteshaver: &Path,
    master: &Path,
    scene: &str,
) -> Result<()> {
    let master_rel = master
        .strip_prefix(&stage.root)
        .unwrap_or(master)
        .to_string_lossy()
        .into_owned();
    // intermediate named `<scene>.png`: byteshaver's `-o` is a flat output
    // directory naming outputs after the input stem => `final/<scene>.webp`
    let scaled_rel = format!("post-tmp/{scene}.png");
    tools::run(
        stage
            .env
            .command(ffmpeg)
            .args(["-y", "-i"])
            .arg(&master_rel)
            .args(["-vf", "scale=iw/2:ih/2:flags=lanczos"])
            .arg(&scaled_rel)
            .current_dir(&stage.root),
    )?;
    dogfood_webp(stage, byteshaver, &scaled_rel, scene)?;
    println!("[post] gui still '{scene}' -> final/{scene}.webp");
    Ok(())
}

/// Terminal still (1x, decorated by VHS): no downscale, straight through
/// the byteshaver binary => `final/<scene>.webp`.
fn process_still_1x(
    stage: &Stage,
    byteshaver: &Path,
    master: &Path,
    scene: &str,
) -> Result<()> {
    let scaled_rel = format!("post-tmp/{scene}.png");
    std::fs::copy(master, stage.root.join(&scaled_rel))
        .with_context(|| format!("copying terminal still of '{scene}' into post-tmp"))?;
    dogfood_webp(stage, byteshaver, &scaled_rel, scene)?;
    println!("[post] terminal still '{scene}' -> final/{scene}.webp");
    Ok(())
}

/// The dogfooding step shared by both still flavors: the freshly built
/// byteshaver binary produces the final WebP (plan 17 §7.3). Runs with
/// cwd = stage so the relative paths hold; `--overwrite-existing` keeps
/// re-used stage dirs (--dry-run, --only post) from silently skipping.
fn dogfood_webp(stage: &Stage, byteshaver: &Path, scaled_rel: &str, scene: &str) -> Result<()> {
    tools::run(
        stage
            .env
            .command(byteshaver)
            .arg(scaled_rel)
            .args(["webp", "-q", "92", "--overwrite-existing", "-o", "final"])
            .current_dir(&stage.root),
    )
    .with_context(|| format!("byteshaver webp encode of '{scene}'"))?;
    Ok(())
}

/// GUI video: 2x `%04d` frames from `0000` => `final/<scene>.webp`.
fn encode_gui_frames(stage: &Stage, ffmpeg: &Path, scene: &str) -> Result<()> {
    tools::run(
        stage
            .env
            .command(ffmpeg)
            .args(["-y", "-framerate", "30", "-start_number", "0", "-i"])
            .arg(format!("out/{scene}/%04d.png"))
            // fps=15 + q50: README videos are size-budgeted (<= ~1.5 MB);
            // the tour beats are mostly cuts, 15 fps keeps them smooth
            // enough while halving the frame count that the confetti
            // particles (worst case for inter-frame compression) cost
            .args(["-vf", "fps=15,scale=iw/2:ih/2:flags=lanczos"])
            .args(["-c:v", "libwebp", "-lossless", "0", "-q:v", "50", "-loop", "0", "-an"])
            .arg(format!("final/{scene}.webp"))
            .current_dir(&stage.root),
    )?;
    println!("[post] gui frames '{scene}' -> final/{scene}.webp");
    Ok(())
}

/// Optional palette-optimized GIF variant of a GUI video (external use
/// only; fps=12, max width 800). Two-pass palettegen/paletteuse via the
/// filter-graph split.
fn encode_gif(stage: &Stage, ffmpeg: &Path, scene: &str) -> Result<()> {
    tools::run(
        stage
            .env
            .command(ffmpeg)
            .args(["-y", "-framerate", "30", "-start_number", "0", "-i"])
            .arg(format!("out/{scene}/%04d.png"))
            .args([
                "-vf",
                // single-quoted min() keeps the graph parser happy; passed
                // verbatim through exec (no shell), ffmpeg unquotes itself
                "fps=12,scale='min(iw,800)':-2:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse",
            ])
            .arg(format!("final/{scene}.gif"))
            .current_dir(&stage.root),
    )?;
    println!("[post] gui frames '{scene}' -> final/{scene}.gif");
    Ok(())
}

/// Terminal video: vhs emits two interleaved `%05d` sequences (text layer +
/// transparent cursor overlay), both starting at `00001`. The cursor is
/// composited over the text layer; frames are already 1x, so no downscale.
/// => `final/<scene>-demo.webp` (the still keeps `final/<scene>.webp`).
fn encode_terminal_frames(stage: &Stage, ffmpeg: &Path, scene: &str) -> Result<()> {
    let text = format!("tapes/out/{scene}/frames/frame-text-%05d.png");
    let cursor_path = stage
        .tapes_dir()
        .join(format!("out/{scene}/frames/frame-cursor-%05d.png"));
    let has_cursor = cursor_path.is_file();
    let mut cmd = stage.env.command(ffmpeg);
    cmd.args(["-y", "-framerate", "30", "-start_number", "1", "-i", &text]);
    if has_cursor {
        cmd.args(["-framerate", "30", "-start_number", "1", "-i"])
            .arg(format!("tapes/out/{scene}/frames/frame-cursor-%05d.png"))
            .args(["-filter_complex", "[0:v][1:v]overlay=format=auto"]);
    }
    // fps=15 + q50: same size budget as the gui videos (module docs);
    // terminal motion is typing + scrolling, 15 fps reads fine
    cmd.args(["-vf", "fps=15"]);
    cmd.args(["-c:v", "libwebp", "-lossless", "0", "-q:v", "50", "-loop", "0", "-an"])
        .arg(format!("final/{scene}-demo.webp"))
        .current_dir(&stage.root);
    tools::run(&mut cmd)?;
    println!("[post] terminal frames '{scene}' -> final/{scene}-demo.webp");
    Ok(())
}

/// Human-facing hint appended when a tool is missing.
fn tools_path_hint() -> String {
    tools_path_hint_from(
        std::env::var_os(tools::MEDIA_TOOLS_PATH_VAR)
            .as_deref()
            .map(|value| value.to_string_lossy().into_owned()),
    )
}

/// Pure core of [`tools_path_hint`] (env-free, so it is unit-testable
/// without the unsafe `set_var` dance of edition 2024).
fn tools_path_hint_from(media_tools_path: Option<String>) -> String {
    match media_tools_path {
        Some(value) => format!(
            " (searched PATH and {}={})",
            tools::MEDIA_TOOLS_PATH_VAR,
            value
        ),
        None => " (searched PATH)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_path_hint_formats_media_tools_path() {
        let hint = tools_path_hint_from(Some("/tmp/media-tools/bin".to_string()));
        assert!(hint.contains("MEDIA_TOOLS_PATH=/tmp/media-tools/bin"), "{hint}");
        assert!(hint.starts_with(" (searched"), "{hint}");
        // unset variable -> generic hint
        assert_eq!(tools_path_hint_from(None), " (searched PATH)");
    }
}
