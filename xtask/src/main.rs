//! xtask - repository automation for the byteshaver workspace.
//!
//! Currently hosts the README media pipeline (plan 17, phase P1): staging,
//! scene plumbing, fixture generation, post-processing helpers and the
//! publish/cache-busting machinery. The terminal (VHS) and GUI (kittest)
//! capture engines are separate phases (P2/P4) and plug into the stage
//! functions defined here; the plumbing (scene discovery, env isolation,
//! tool lookup) is already real so their integration stays a small diff.
//!
//! Everything runs through cargo aliases (see `.cargo/config.toml`):
//!
//! ```text
//! cargo media --help          # or: cargo xtask media --help
//! cargo media gen-fixtures
//! ```

use clap::{Parser, Subcommand};

mod fixtures;
mod manifest;
mod media;
mod post;
mod readme;
mod stage;
mod tools;
mod util;

/// Top-level CLI: `cargo media <COMMAND>` (the alias expands to
/// `cargo run --release -p xtask -- <COMMAND>`).
#[derive(Parser)]
#[command(
    name = "xtask",
    about = "Repository automation for byteshaver: README media pipeline (plan 17).",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// README media pipeline: stage -> terminal -> gui -> post -> publish.
    ///
    /// Without a subcommand this runs the full pipeline. `readme` and `deps`
    /// run a single maintenance step instead (pipeline flags are ignored).
    Media {
        #[command(subcommand)]
        sub: Option<MediaSub>,

        /// Restrict the run to named stages and/or scenes. Accepts a
        /// comma-separated list and/or repeated flags: `--only terminal,post`,
        /// `--only gui --only gui-report`, `--only cli-basic`.
        ///
        /// Known stage names: stage | terminal | gui | post | publish.
        /// Anything else is treated as a scene name (a `.tape` file stem for
        /// the terminal stage, a scene id from `bh-gui-capture --list` for
        /// the gui stage). Omitting --only runs everything.
        #[arg(long, value_name = "STAGE|SCENE", value_delimiter = ',')]
        only: Vec<String>,

        /// Skip every video-producing step (frame sequences); stills only.
        #[arg(long)]
        skip_video: bool,

        /// Do everything except writes outside target/: docs/img/, README
        /// and manifest stay untouched and publish only previews its result.
        /// The stage dir (inside target/) is still created/populated.
        #[arg(long)]
        dry_run: bool,

        /// Regenerate into target/ only and compare the results against the
        /// committed docs/img/ + docs/media/manifest.json. Prints a freshness
        /// table and exits 1 when anything is stale/missing. Never publishes.
        #[arg(long)]
        check: bool,

        /// Additionally produce palette-optimized GIF variants (fps=12,
        /// max width 800) next to animated WebPs. External use only - the
        /// README references WebP only. Off by default.
        #[arg(long)]
        gif: bool,
    },

    /// Deterministically (re)generate every file under docs/media/fixtures/.
    ///
    /// Photos are byte-copies from examples/, screens and animations are
    /// procedurally drawn with fixed seeds (no time, no randomness). Output
    /// is byte-identical across runs on the same toolchain; commit the
    /// results (they are pinned pipeline inputs, plan 17 §5.4).
    GenFixtures,
}

/// Single-step subcommands of `media`.
#[derive(Subcommand)]
enum MediaSub {
    /// Only the README rewrite pass: refresh every `docs/img/<asset>?v=`
    /// query string from the current manifest and lint that all references
    /// resolve. The pipeline's publish step runs the same pass internally.
    Readme,
    /// Check the external tools (vhs, ttyd, ffmpeg) and the built binaries;
    /// print found versions, exit non-zero listing the missing tools.
    Deps,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Media {
            sub,
            only,
            skip_video,
            dry_run,
            check,
            gif,
        } => {
            let opts = media::MediaOpts {
                only,
                skip_video,
                dry_run,
                check,
                gif,
            };
            match sub {
                None => media::run_pipeline(&opts),
                Some(MediaSub::Readme) => media::run_readme_only(),
                Some(MediaSub::Deps) => media::run_deps(),
            }
        }
        Command::GenFixtures => fixtures::gen_fixtures(),
    }
}
