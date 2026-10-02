//! Post-processing of capture artifacts into final assets (plan 17 §7).
//!
//! This is the "image processing" leg of the pipeline: ffmpeg for the
//! 2x->1x lanczos downscale and the animated WebP encode, plus the freshly
//! built `byteshaver` binary itself for the still WebPs (dogfooding - the
//! pipeline's own product optimizes the README's images).
//!
//! # Artifact layout contract (agreed with the capture engines)
//!
//! Capture stages (terminal tapes in P4, `bh-gui-capture` in P2) drop their
//! masters under `<stage>/out/<scene>/`; the post stage turns each scene
//! into exactly one final asset `<stage>/final/<scene>.webp`:
//!
//! ```text
//! <stage>/out/<scene>/still.png           # 2x master still (single PNG)
//!     -> ffmpeg scale=iw/2:ih/2:flags=lanczos -> post-tmp/<scene>.png
//!     -> bin/byteshaver post-tmp/<scene>.png webp -q 92 -o final
//!     => final/<scene>.webp
//!
//! <stage>/out/<scene>/frames/%04d.png     # 2x frame sequence, numbered
//!                                         # from 0001, contiguous
//!     -> ffmpeg -framerate 30 -i out/<scene>/frames/%04d.png
//!              -vf scale=iw/2:ih/2:flags=lanczos
//!              -c:v libwebp -lossless 0 -q:v 62 -loop 0 -an
//!     => final/<scene>.webp
//! ```
//!
//! The `%04d` numbering (starting at `0001`, zero-padded, no gaps) is the
//! ffmpeg image2 sequence convention and is emitted by both capture
//! engines: VHS PNG-frame output and the `bh-gui-capture --frames-dir`
//! writer. Any `out/<scene>/` dir with neither `still.png` nor `frames/` is
//! reported and skipped rather than guessed at.
//!
//! Masters stay in `target/` only; the repo commits finals (plan 17 §7).

use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::media::MediaOpts;
use crate::stage::Stage;
use crate::tools;
use crate::util;

/// Runs post-processing over every scene dir in `<stage>/out/`.
pub fn run(stage: &Stage, opts: &MediaOpts) -> Result<()> {
    let scenes = util::list_dirs(&stage.out_dir())?;
    if scenes.is_empty() {
        println!("[post] no capture artifacts under out/ (terminal/gui stages produced nothing) - nothing to post-process");
        return Ok(());
    }

    // Tools are checked once, upfront: artifacts exist, so a missing ffmpeg
    // is a hard error, not a skip.
    let ffmpeg = tools::lookup("ffmpeg").ok_or_else(|| {
        anyhow::anyhow!(
            "[post] 'ffmpeg' not found{} - required to post-process {} scene(s)",
            tools_path_hint(),
            scenes.len()
        )
    })?;
    let byteshaver = stage.bin("byteshaver");
    if !byteshaver.is_file() {
        bail!(
            "[post] staged byteshaver missing at {} - run the 'stage' phase first",
            byteshaver.display()
        );
    }

    for scene in scenes {
        let scene_dir = stage.out_dir().join(&scene);
        let frames = scene_dir.join("frames");
        if frames.is_dir() {
            if opts.skip_video {
                println!("[post] scene '{scene}': frames present but --skip-video set - skipped");
                continue;
            }
            encode_frames(stage, &ffmpeg, &scene)
                .with_context(|| format!("post-processing frames of scene '{scene}'"))?;
            if opts.gif {
                encode_gif(stage, &ffmpeg, &scene)
                    .with_context(|| format!("encoding GIF variant of scene '{scene}'"))?;
            }
        } else if scene_dir.join("still.png").is_file() {
            process_still(stage, &ffmpeg, &byteshaver, &scene)
                .with_context(|| format!("post-processing still of scene '{scene}'"))?;
        } else {
            println!(
                "[post] scene '{scene}': neither still.png nor frames/ present - unknown layout, skipped"
            );
        }
    }
    Ok(())
}

/// Still pipeline (see the module docs for the exact command set).
fn process_still(
    stage: &Stage,
    ffmpeg: &Path,
    byteshaver: &Path,
    scene: &str,
) -> Result<()> {
    // 1) 2x -> 1x lanczos downscale into the scratch dir. The intermediate
    //    is named `<scene>.png` on purpose: byteshaver's `-o` is a flat
    //    output directory and names outputs after the input stem, so this
    //    yields `final/<scene>.webp` directly.
    let scaled_rel = format!("post-tmp/{scene}.png");
    tools::run(
        stage
            .env
            .command(ffmpeg)
            .args(["-y", "-i"])
            .arg(format!("out/{scene}/still.png"))
            .args(["-vf", "scale=iw/2:ih/2:flags=lanczos"])
            .arg(&scaled_rel)
            .current_dir(&stage.root),
    )?;
    // 2) dogfood: the pipeline's own CLI produces the final WebP. Runs with
    //    cwd = stage so the paths above stay relative (plan 17 §7.3).
    tools::run(
        stage
            .env
            .command(byteshaver)
            .arg(&scaled_rel)
            .args(["webp", "-q", "92", "-o", "final"])
            .current_dir(&stage.root),
    )?;
    println!("[post] scene '{scene}': still -> final/{scene}.webp");
    Ok(())
}

/// Animated WebP from a `%04d`-numbered frame sequence (plan 17 §7.2).
fn encode_frames(stage: &Stage, ffmpeg: &Path, scene: &str) -> Result<()> {
    tools::run(
        stage
            .env
            .command(ffmpeg)
            .args(["-y", "-framerate", "30", "-i"])
            .arg(format!("out/{scene}/frames/%04d.png"))
            .args(["-vf", "scale=iw/2:ih/2:flags=lanczos"])
            .args(["-c:v", "libwebp", "-lossless", "0", "-q:v", "62", "-loop", "0", "-an"])
            .arg(format!("final/{scene}.webp"))
            .current_dir(&stage.root),
    )?;
    println!("[post] scene '{scene}': frames -> final/{scene}.webp");
    Ok(())
}

/// Optional palette-optimized GIF variant (external use only; fps=12, max
/// width 800). Two-pass palettegen/paletteuse via the filter-graph split.
fn encode_gif(stage: &Stage, ffmpeg: &Path, scene: &str) -> Result<()> {
    tools::run(
        stage
            .env
            .command(ffmpeg)
            .args(["-y", "-framerate", "30", "-i"])
            .arg(format!("out/{scene}/frames/%04d.png"))
            .args([
                "-vf",
                // single-quoted min() keeps the graph parser happy; passed
                // verbatim through exec (no shell), ffmpeg unquotes itself
                "fps=12,scale='min(iw,800)':-2:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse",
            ])
            .arg(format!("final/{scene}.gif"))
            .current_dir(&stage.root),
    )?;
    println!("[post] scene '{scene}': frames -> final/{scene}.gif");
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
