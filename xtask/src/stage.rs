//! Staging: `target/media-stage/` is the scratch workspace every pipeline
//! run rebuilds from the committed inputs (plan 17 §2 step 1, §4).
//!
//! Layout:
//!
//! ```text
//! target/media-stage/
//!   home/                  isolated HOME/XDG tree for every child process
//!     .config/ .cache/ .local/share/ .local/share/flatpak/
//!   bin/byteshaver         freshly built release CLI (copied in)
//!   bin/bh-gui-capture     GUI capture binary, when built (plan 17 P2)
//!   demo/                  fixtures copy (photos/ anim/ screens/ ...)
//!   tapes/                 copy of docs/media/tapes/ (vhs cwd)
//!   out/<scene>/...        capture artifacts (masters; see post.rs contract)
//!   final/                 post-processed, would-be-published assets
//!   post-tmp/              scratch for post-processing intermediates
//! ```
//!
//! Captures never touch `examples/` or the repo tree, so generated outputs
//! never feed back into later runs.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::tools;
use crate::util::{self, repo_root};

/// Stage location relative to the workspace root.
pub const STAGE_REL: &str = "target/media-stage";

/// Env overlay applied to every child process (vhs, ffmpeg, byteshaver,
/// bh-gui-capture): isolated HOME/XDG dirs under the stage so neither the
/// developer's real config/cache leaks into captures nor anything a child
/// writes leaks out, and a PATH that starts with the staged binaries (plus
/// the MEDIA_TOOLS_PATH dirs, so e.g. vhs finds a pinned ffmpeg first).
#[derive(Debug, Clone)]
pub struct StageEnv {
    home: PathBuf,
    path_prepend: Vec<PathBuf>,
}

impl StageEnv {
    /// The staged isolated HOME (e.g. for FONT_HOME in the tape font
    /// setup).
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Same-filesystem scratch dir handed to children as `TMPDIR` (see the
    /// comment in [`Self::command`]).
    pub fn tmp_dir(&self) -> PathBuf {
        self.home.parent().unwrap_or(&self.home).join("tmp")
    }

    /// Wraps a program in a `Command` with the overlay applied. Callers set
    /// cwd explicitly (byteshaver/ffmpeg run in the stage root, vhs in
    /// `<stage>/tapes`, so tape/asset paths stay relative and portable).
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new(program);
        cmd.env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_DATA_DIRS", xdg_data_dirs(&self.home))
            // TMPDIR on the same filesystem as the stage: vhs records tape
            // frames into a MkdirTemp dir and moves them to the output with
            // an error-ignored os.Rename - a cross-device rename (tmpfs
            // /tmp -> ext4 target/) fails silently and the frames vanish
            // (plan 17 §5, vhs 0.12 evaluator.go)
            .env("TMPDIR", self.tmp_dir());
        // PATH = staged bin + pinned tool dirs + whatever the parent had.
        // Everything else in the parent env stays (overlay semantics).
        let mut dirs = self.path_prepend.clone();
        if let Some(existing) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(&dirs) {
            cmd.env("PATH", joined);
        }
        cmd
    }
}

/// `XDG_DATA_DIRS` per the spec: the staged flatpak dir plus the two
/// conventional system dirs (kept so apps still find baseline data).
fn xdg_data_dirs(home: &Path) -> std::ffi::OsString {
    let dirs = [
        home.join(".local/share/flatpak"),
        PathBuf::from("/usr/local/share"),
        PathBuf::from("/usr/share"),
    ];
    std::env::join_paths(dirs).unwrap_or_default()
}

/// A materialized stage dir plus its env overlay.
#[derive(Debug, Clone)]
pub struct Stage {
    pub root: PathBuf,
    pub env: StageEnv,
}

impl Stage {
    /// Absolute path of the stage dir for the current workspace.
    pub fn path() -> PathBuf {
        repo_root().join(STAGE_REL)
    }

    /// Wipes and recreates the stage. Default for full runs: captures must
    /// start from the committed inputs only (plan 17 §3).
    pub fn rebuild() -> Result<Stage> {
        util::remove_dir_if_exists(&Self::path())?;
        Self::ensure()
    }

    /// Creates the stage only when missing. Used by `--dry-run` so a dry run
    /// never destroys a previous full run's artifacts, and by `--only`
    /// invocations that reuse an existing stage.
    pub fn ensure() -> Result<Stage> {
        let root = Self::path();
        // The isolated HOME tree is created eagerly: children expect the
        // XDG dirs to exist before they write into them.
        for sub in [
            "home/.config",
            "home/.cache",
            "home/.local/share",
            "home/.local/share/flatpak",
            "bin",
            "demo",
            "tapes",
            "tmp",
            "out",
            "final",
            "post-tmp",
        ] {
            util::ensure_dir(&root.join(sub))?;
        }
        let mut path_prepend = vec![root.join("bin")];
        path_prepend.extend(tools::media_tools_dirs());
        let env = StageEnv {
            home: root.join("home"),
            path_prepend,
        };
        Ok(Stage { root, env })
    }

    pub fn bin(&self, name: &str) -> PathBuf {
        self.root.join("bin").join(name)
    }

    pub fn demo_dir(&self) -> PathBuf {
        self.root.join("demo")
    }

    pub fn tapes_dir(&self) -> PathBuf {
        self.root.join("tapes")
    }

    pub fn out_dir(&self) -> PathBuf {
        self.root.join("out")
    }

    pub fn final_dir(&self) -> PathBuf {
        self.root.join("final")
    }

    /// Copies `docs/media/fixtures/**` under `demo/`, preserving the natural
    /// tree (photos/, anim/, screens/) so queue tables and globs look right
    /// on camera.
    pub fn copy_fixtures(&self) -> Result<()> {
        let fixtures = repo_root().join("docs/media/fixtures");
        util::copy_tree(&fixtures, &self.demo_dir())
            .context("staging docs/media/fixtures into demo/")?;
        Ok(())
    }

    /// Copies a built binary into the stage's `bin/` (world-executable, like
    /// cargo leaves it, so `Require byteshaver` in tapes keeps working).
    pub fn copy_binary(&self, src: &Path, name: &str) -> Result<()> {
        let dst = self.bin(name);
        std::fs::copy(src, &dst).with_context(|| {
            format!("staging binary {} -> {}", src.display(), dst.display())
        })?;
        Ok(())
    }
}
