//! The `media` pipeline (plan 17 §2/§4): `cargo media` runs
//!
//! ```text
//! stage -> terminal -> gui -> post -> publish
//! ```
//!
//! - **stage**: builds the release CLI, rebuilds `target/media-stage/` from
//!   scratch, copies fixtures + binaries in, and hands every child process
//!   an isolated `StageEnv`.
//! - **terminal**: VHS tape runs (P4 owns the tape set). The plumbing is
//!   real - scene discovery over `docs/media/tapes/*.tape`, staging, env,
//!   the `vhs <tape>` invocation with cwd `<stage>/tapes` - but degrades to
//!   a clear "not available" notice while vhs/ttyd/ffmpeg or the tapes are
//!   missing.
//! - **gui**: `bh-gui-capture` runs (P2 owns the engine). Discovery via
//!   `--list` is implemented; the per-scene call is a marked stub until the
//!   capture binary lands.
//! - **post**: real post-processing over the artifacts (see `post.rs` for
//!   the layout contract).
//! - **publish**: copies `final/` into `docs/img/`, bumps manifest versions
//!   for changed bytes, rewrites README `?v=` queries, lints image links.
//!   `--check` replaces publish with a freshness table (no writes); with
//!   stub capture stages producing nothing, "nothing produced" is reported
//!   as *skipped*, not failed.

use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::time::Instant;

use crate::manifest::Manifest;
use crate::post;
use crate::readme;
use crate::stage::Stage;
use crate::tools;
use crate::util;

/// CLI options of the `media` command (see main.rs for the help texts).
pub struct MediaOpts {
    pub only: Vec<String>,
    pub skip_video: bool,
    pub dry_run: bool,
    pub check: bool,
    pub gif: bool,
}

/// Pipeline stage names, in run order.
const STAGE_NAMES: [&str; 5] = ["stage", "terminal", "gui", "post", "publish"];

/// What a `--only` list selects: explicit stage names, scene names, or both.
#[derive(Default)]
struct Selection {
    stages: Option<HashSet<String>>,
    scenes: Option<HashSet<String>>,
}

impl Selection {
    fn is_empty(&self) -> bool {
        self.stages.is_none() && self.scenes.is_none()
    }

    fn wants_stage(&self, name: &str) -> bool {
        match &self.stages {
            None => true,
            Some(stages) => stages.contains(name),
        }
    }

    /// Scene filter for tape/scene discovery (`None` = all scenes).
    fn scene_filter(&self) -> Option<&HashSet<String>> {
        self.scenes.as_ref()
    }
}

impl MediaOpts {
    fn selection(&self) -> Selection {
        let mut stages = HashSet::new();
        let mut scenes = HashSet::new();
        for item in &self.only {
            let name = item.trim();
            if name.is_empty() {
                continue;
            }
            if STAGE_NAMES.contains(&name) {
                stages.insert(name.to_string());
            } else {
                scenes.insert(name.to_string());
            }
        }
        Selection {
            stages: (!stages.is_empty()).then_some(stages),
            scenes: (!scenes.is_empty()).then_some(scenes),
        }
    }
}

/// Runs the full pipeline (no subcommand).
pub fn run_pipeline(opts: &MediaOpts) -> Result<()> {
    let repo = util::repo_root();
    let started = Instant::now();
    println!("byteshaver media pipeline (plan 17) - root: {}", repo.display());
    let sel = opts.selection();
    if !sel.is_empty() {
        println!("--only: {}", opts.only.join(", "));
    }
    if opts.skip_video {
        println!("--skip-video: frame sequences are not post-processed");
    }
    if opts.dry_run {
        println!("mode: dry-run (no writes outside target/)");
    }
    if opts.check {
        println!("mode: check (regenerate into target/, compare, no publish)");
    }

    // ---- 1. stage ---------------------------------------------------------
    let stage = if sel.wants_stage("stage") {
        build_release_byteshaver(&repo)?;
        // dry-run must not destroy a previous full run's artifacts, so it
        // only ensures the dirs and overwrites the inputs it stages.
        let stage = if opts.dry_run {
            Stage::ensure()?
        } else {
            Stage::rebuild()?
        };
        stage.copy_fixtures()?;
        stage.copy_binary(&repo.join("target/release/byteshaver"), "byteshaver")?;
        let capture = repo.join("target/release/bh-gui-capture");
        if capture.is_file() {
            stage.copy_binary(&capture, "bh-gui-capture")?;
        } else {
            println!(
                "[stage] note: target/release/bh-gui-capture does not exist yet (plan 17 P2, \
                 in flight) - the gui stage will be skipped"
            );
        }
        stage
    } else {
        // --only post/publish etc. reuse the existing stage; it must exist
        // already, otherwise there is nothing to work on.
        if !Stage::path().is_dir() {
            bail!(
                "stage dir {} does not exist - run without --only (or include 'stage') first",
                Stage::path().display()
            );
        }
        Stage::ensure()?
    };

    // ---- 2. terminal ------------------------------------------------------
    if sel.wants_stage("terminal") {
        run_terminal(&repo, &stage, &sel)?;
    }

    // ---- 3. gui -----------------------------------------------------------
    if sel.wants_stage("gui") {
        run_gui(&repo, &stage, &sel)?;
    }

    // ---- 4. post ----------------------------------------------------------
    if sel.wants_stage("post") {
        post::run(&stage, opts)?;
    }

    // ---- 5. publish (or check instead of it) ------------------------------
    if opts.check {
        run_check(&repo, &stage)?;
    } else if sel.wants_stage("publish") {
        run_publish(&repo, &stage, opts)?;
    }

    println!("done in {:.1}s", started.elapsed().as_secs_f64());
    Ok(())
}

/// Builds the release CLI once per run (the pipeline dogfoods this exact
/// binary for still post-processing). Honors the `CARGO` env override.
fn build_release_byteshaver(repo: &Path) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    println!("[stage] building release CLI ({cargo} build --release -p byteshaver)...");
    let status = std::process::Command::new(&cargo)
        .args(["build", "--release", "-p", "byteshaver"])
        .current_dir(repo)
        .status()
        .with_context(|| format!("spawning {cargo} (set CARGO to override)"))?;
    if !status.success() {
        bail!("cargo build --release -p byteshaver failed ({status})");
    }
    Ok(())
}

/// Terminal engine plumbing (plan 17 §5). The vhs invocation is implemented
/// fully (it is mechanical) and guarded by the tool check; the tape set
/// itself arrives with P4.
fn run_terminal(repo: &Path, stage: &Stage, sel: &Selection) -> Result<()> {
    let tapes_src = repo.join("docs/media/tapes");
    // scene discovery: every *.tape not starting with '_' (_shared.tape is
    // a Source partial, not a scene)
    let mut tapes: Vec<String> = Vec::new();
    if tapes_src.is_dir() {
        for entry in std::fs::read_dir(&tapes_src)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_file() && name.ends_with(".tape") && !name.starts_with('_') {
                tapes.push(name.trim_end_matches(".tape").to_string());
            }
        }
        tapes.sort();
    }
    if tapes.is_empty() {
        println!(
            "[terminal] no scenes yet: docs/media/tapes/*.tape is empty (plan 17 P4 owns the \
             tape set) - nothing to do"
        );
        return Ok(());
    }
    let tapes = match sel.scene_filter() {
        Some(filter) => tapes
            .into_iter()
            .filter(|tape| filter.contains(tape))
            .collect::<Vec<_>>(),
        None => tapes,
    };
    if tapes.is_empty() {
        println!("[terminal] --only filter matched no tape - skipped");
        return Ok(());
    }

    // prerequisites: vhs plus its ttyd/ffmpeg backends (plan 17 §5.1)
    let Some(vhs) = tools::lookup("vhs") else {
        println!(
            "[terminal] not available: 'vhs' not found{} - skipping (install \
             https://github.com/charmbracelet/vhs, or point MEDIA_TOOLS_PATH at a bin dir \
             containing it)",
            tools_hint()
        );
        return Ok(());
    };
    for prereq in ["ttyd", "ffmpeg"] {
        if tools::lookup(prereq).is_none() {
            println!(
                "[terminal] not available: vhs requires '{prereq}'{hint} - skipping",
                hint = tools_hint()
            );
            return Ok(());
        }
    }

    // vhs runs with cwd = <stage>/tapes against a stage copy, so the
    // repo-relative paths inside the tapes (Source, Output, Screenshot)
    // stay portable between the repo and the stage (plan 17 §5.2).
    util::copy_tree(&tapes_src, &stage.tapes_dir())?;

    // pre-create each scene's output parent: vhs 0.12 applies `Output <dir>/`
    // as a rename that fails *silently* when the parent is missing (the
    // ascii file output would create it, but only after the frames rename
    // already failed) - see docs/media/tapes/README.md
    for tape in &tapes {
        util::ensure_dir(&stage.tapes_dir().join("out").join(tape))?;
    }

    // fonts: install the committed JetBrains Mono copies into the stage's
    // isolated HOME so fontconfig resolves the pinned family without any
    // system font packages (plan 17 §5.2; script documented in
    // docs/media/tapes/setup-fonts.sh)
    let setup_fonts = stage.tapes_dir().join("setup-fonts.sh");
    if setup_fonts.is_file() {
        let font_src = repo.join("docs/media/fonts/JetBrainsMono");
        tools::run(
            stage
                .env
                .command("bash")
                .arg(&setup_fonts)
                .env("FONT_HOME", stage.env.home().join(".fonts"))
                .env("BH_TAPE_FONT_DIR", &font_src),
        )
        .context("installing tape fonts into the staged HOME")?;
    }

    for tape in &tapes {
        println!("[terminal] vhs {tape}.tape");
        // VHS_NO_SANDBOX=1: vhs (go-rod) only passes --no-sandbox to its
        // headless chromium when set; without it the sandboxed browser
        // fails in root-less/containerized environments
        tools::run(
            stage
                .env
                .command(&vhs)
                .arg(format!("{tape}.tape"))
                .env("VHS_NO_SANDBOX", "1")
                .current_dir(stage.tapes_dir()),
        )
        .with_context(|| format!("vhs run for tape '{tape}'"))?;
        // vhs exits 0 even when the recording failed (e.g. `Require` not
        // met, chromium crash), so the artifacts are the real signal
        verify_tape_artifacts(stage, tape)?;
    }
    Ok(())
}

/// Verifies a tape produced its outputs: `out/<tape>/<tape>.ascii`, the
/// `Screenshot` still and a non-empty `frames/` dir (all three are part of
/// the tape conventions, docs/media/tapes/README.md).
fn verify_tape_artifacts(stage: &Stage, tape: &str) -> Result<()> {
    let scene_dir = stage.tapes_dir().join("out").join(tape);
    let mut missing = Vec::new();
    if !scene_dir.join(format!("{tape}.ascii")).is_file() {
        missing.push(format!("{}.ascii", tape));
    }
    if !scene_dir.join("still.png").is_file() {
        missing.push("still.png".to_string());
    }
    let frames = scene_dir.join("frames");
    let frame_count = match util::list_files(&frames) {
        Ok(files) => files.len(),
        Err(_) => 0,
    };
    if frame_count == 0 {
        missing.push("frames/ (empty or missing)".to_string());
    }
    if !missing.is_empty() {
        bail!(
            "[terminal] tape '{tape}' produced no {} - vhs exits 0 on failure, so this is \
             a real recording error (see the vhs output above)",
            missing.join(", ")
        );
    }
    println!("[terminal] tape '{tape}': {frame_count} frame(s) + still + ascii");
    Ok(())
}

/// GUI engine (plan 17 §6): builds `bh-gui-capture` (release, `capture`
/// feature) when missing, then runs each registered scene headlessly in the
/// stage root. Stills land as `out/<name>.png`; video scenes additionally
/// dump their frame beats under `out/<scene-id>/NNNN.png` (post.rs owns the
/// layouts).
fn run_gui(repo: &Path, stage: &Stage, sel: &Selection) -> Result<()> {
    // build the capture binary on demand (the wgpu-backed software
    // rasterizer makes this a hefty first build, so it happens only when
    // the gui stage actually runs)
    let capture_src = repo.join("target/release/bh-gui-capture");
    let staged = stage.bin("bh-gui-capture");
    if !staged.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        println!(
            "[gui] building capture binary ({cargo} build --release -p byteshaver-gui \
             --features capture)..."
        );
        let status = std::process::Command::new(&cargo)
            .args([
                "build",
                "--release",
                "-p",
                "byteshaver-gui",
                "--features",
                "capture",
            ])
            .current_dir(repo)
            .status()
            .with_context(|| format!("spawning {cargo} (set CARGO to override)"))?;
        if !status.success() {
            bail!("cargo build -p byteshaver-gui --features capture failed ({status})");
        }
        stage.copy_binary(&capture_src, "bh-gui-capture")?;
    }

    // scene discovery contract (`--list`): `<id> [ [video]] <description>`
    // per line; '#' lines are comments
    let listing = tools::run(
        stage
            .env
            .command(&staged)
            .arg("--list")
            .current_dir(&stage.root),
    )
    .context("bh-gui-capture --list failed")?;
    let mut scenes: Vec<(String, bool)> = listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut parts = line.split_whitespace();
            let id = parts.next().unwrap_or_default().to_string();
            let video = parts.next() == Some("[video]");
            (id, video)
        })
        .filter(|(id, _)| !id.is_empty())
        .collect();
    if let Some(filter) = sel.scene_filter() {
        scenes.retain(|(id, _)| filter.contains(id));
    }
    if scenes.is_empty() {
        println!("[gui] no scenes to capture (--list output empty or filtered out) - skipped");
        return Ok(());
    }

    // committed symbol-font copies so glyph rendering does not depend on
    // the host's font packages (plan 17 §6.2)
    let fonts_dir = repo.join("docs/media/fonts/symbols");

    for (scene, video) in &scenes {
        println!("[gui] bh-gui-capture --scene {scene}{}", if *video { " (+frames)" } else { "" });
        // wipe the scene's previous artifacts first: a re-capture that
        // produces fewer frames than the last run would otherwise leave
        // stale trailing frames behind that the post stage would encode,
        // and stale conversion outputs would make the run report show
        // "skipped" rows (collision policy keeps existing files)
        let scene_frame_dir = stage.out_dir().join(scene);
        if scene_frame_dir.is_dir() {
            std::fs::remove_dir_all(&scene_frame_dir)
                .with_context(|| format!("clearing stale frames of scene '{scene}'"))?;
        }
        let scene_run_dir = stage.out_dir().join("capture-run").join(scene);
        if scene_run_dir.is_dir() {
            std::fs::remove_dir_all(&scene_run_dir)
                .with_context(|| format!("clearing stale conversion outputs of scene '{scene}'"))?;
        }
        let mut cmd = stage.env.command(&staged);
        cmd.arg("--scene")
            .arg(scene)
            .arg("--out-dir")
            .arg(stage.out_dir())
            .current_dir(&stage.root);
        if *video {
            cmd.arg("--frames-dir").arg(stage.out_dir());
        }
        if fonts_dir.is_dir() {
            cmd.arg("--fonts-dir").arg(&fonts_dir);
        }
        tools::run(&mut cmd).with_context(|| format!("gui capture scene '{scene}'"))?;
    }
    Ok(())
}

/// Publish: `final/` -> `docs/img/` + manifest version bumps + README `?v=`
/// rewrite + link lint. In `--dry-run` mode nothing outside target/ is
/// written; the lint runs read-only against the on-disk state plus the
/// would-be-published names.
fn run_publish(repo: &Path, stage: &Stage, opts: &MediaOpts) -> Result<()> {
    let docs_img = repo.join("docs/img");
    let finals = util::list_files(&stage.final_dir())?;
    let manifest_path = repo.join("docs/media/manifest.json");
    let mut manifest = Manifest::load_or_default(&manifest_path)?;

    let mut bumped: Vec<String> = Vec::new();
    for name in &finals {
        let sha = util::sha256_file(&stage.final_dir().join(name))?;
        if manifest.upsert(name, sha) {
            bumped.push(name.clone());
        }
    }

    if finals.is_empty() {
        println!(
            "[publish] nothing was produced (stub capture stages) - docs/img and manifest stay \
             untouched"
        );
    } else if opts.dry_run {
        println!(
            "[publish] dry-run: would write {} asset(s) into docs/img/ ({}): {} version bump(s)",
            finals.len(),
            finals.join(", "),
            bumped.len()
        );
    } else {
        util::ensure_dir(&docs_img)?;
        for name in &finals {
            std::fs::copy(stage.final_dir().join(name), docs_img.join(name))
                .with_context(|| format!("publishing {name}"))?;
        }
        manifest.generated_by = generated_by(repo);
        manifest.save(&manifest_path)?;
        println!(
            "[publish] wrote {} asset(s) to docs/img/, {} version bump(s): {}",
            finals.len(),
            bumped.len(),
            if bumped.is_empty() {
                "none".to_string()
            } else {
                bumped.join(", ")
            }
        );
        // rewrite every tracked reference to its (possibly new) version;
        // unchanged assets keep their version, so their URLs are stable
        rewrite_readmes(repo, &manifest)?;
    }

    // link lint in every mode (plan 17 §8): a docs/img reference that does
    // not resolve is a hard error. In dry-run mode, references to assets
    // this run *would* publish are accepted as a preview.
    let mut extra = BTreeSet::new();
    if opts.dry_run {
        extra.extend(finals.iter().cloned());
    }
    lint_readmes(repo, &docs_img, &extra)?;
    Ok(())
}

/// `--check`: no publish. Compares the regenerated finals against the
/// committed `docs/img/` bytes and the manifest records; prints a freshness
/// table and fails when anything is stale/missing/doctored. Assets that the
/// (stub) stages did not produce are reported as skipped, not failed.
fn run_check(repo: &Path, stage: &Stage) -> Result<()> {
    let docs_img = repo.join("docs/img");
    let manifest_path = repo.join("docs/media/manifest.json");
    let manifest = Manifest::load_or_default(&manifest_path)?;

    let mut committed: BTreeMap<String, String> = BTreeMap::new();
    for name in util::list_files(&docs_img)? {
        committed.insert(name.clone(), util::sha256_file(&docs_img.join(&name))?);
    }
    let mut regenerated: BTreeMap<String, String> = BTreeMap::new();
    for name in util::list_files(&stage.final_dir())? {
        regenerated.insert(name.clone(), util::sha256_file(&stage.final_dir().join(&name))?);
    }

    let names: BTreeSet<String> = committed
        .keys()
        .chain(regenerated.keys())
        .cloned()
        .collect();

    println!();
    println!(
        "{:<26} {:<16} {:<18} {:<18}",
        "asset", "state", "committed sha", "new sha"
    );
    let mut fresh = 0_usize;
    let mut stale = 0_usize;
    let mut missing = 0_usize;
    let mut skipped = 0_usize;
    let mut untracked = 0_usize;
    let mut drift = 0_usize;
    for name in &names {
        let regen = regenerated.get(name);
        let com = committed.get(name);
        let record = manifest.assets.get(name);
        // strongest signal first: the committed bytes no longer match the
        // manifest record -> doctored asset or a hand-edited docs/img
        // (plan 17 P1 exit criterion); fails the check regardless of
        // freshness
        let drifted = matches!((com, record), (Some(old), Some(rec)) if old != &rec.sha256);
        let state = if drifted {
            "manifest-drift"
        } else if let (Some(new), Some(old)) = (regen, com) {
            if new == old {
                "fresh"
            } else {
                "stale"
            }
        } else if regen.is_some() {
            // produced but not committed: docs is behind the pipeline
            "missing"
        } else if record.is_some() {
            // tracked asset that this run did not regenerate: e.g. a stub
            // capture stage - explicitly not a failure (plan 17 P1)
            "skipped"
        } else {
            // committed but neither regenerated nor tracked (legacy assets)
            "untracked"
        };
        match state {
            "fresh" => fresh += 1,
            "stale" => stale += 1,
            "missing" => missing += 1,
            "skipped" => skipped += 1,
            "untracked" => untracked += 1,
            _ => drift += 1,
        }
        println!(
            "{:<26} {:<16} {:<18} {:<18}",
            name,
            state,
            com.map(|s| short_sha(s)).unwrap_or_else(|| "-".into()),
            regen.map(|s| short_sha(s)).unwrap_or_else(|| "-".into()),
        );
    }
    println!();
    println!(
        "[check] {fresh} fresh, {stale} stale, {missing} missing, {drift} manifest-drift, \
         {skipped} skipped, {untracked} untracked"
    );
    let problems = stale + missing + drift;
    if problems > 0 {
        bail!("[check] {problems} asset(s) not fresh - regenerate and publish");
    }
    println!("[check] no drift detected (nothing to publish)");
    Ok(())
}

/// 16-hex-char sha prefix for the table (full hashes live in the manifest).
fn short_sha(sha: &str) -> String {
    sha.chars().take(16).collect()
}

/// `media readme`: only the README rewrite pass, using the current manifest.
pub fn run_readme_only() -> Result<()> {
    let repo = util::repo_root();
    let manifest_path = repo.join("docs/media/manifest.json");
    if !manifest_path.is_file() {
        bail!(
            "no manifest at {} - run `cargo xtask media` first (the manifest is written by \
             the publish step)",
            manifest_path.display()
        );
    }
    let manifest = Manifest::load(&manifest_path)?;
    rewrite_readmes(&repo, &manifest)?;
    lint_readmes(&repo, &repo.join("docs/img"), &BTreeSet::new())?;
    println!("[readme] done ({} tracked asset(s))", manifest.assets.len());
    Ok(())
}

/// `media deps`: external tool + binary check. Exits non-zero listing the
/// missing tools (CI runs this first, plan 17 §4).
pub fn run_deps() -> Result<()> {
    let repo = util::repo_root();
    let dirs = tools::media_tools_dirs();
    if dirs.is_empty() {
        println!("external tools (search order: PATH):");
    } else {
        let joined = dirs
            .iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(":");
        println!(
            "external tools (search order: {}={} then PATH):",
            tools::MEDIA_TOOLS_PATH_VAR,
            joined
        );
    }
    let mut missing: Vec<&'static str> = Vec::new();
    for (name, purpose) in [
        (
            "vhs",
            "terminal capture (https://github.com/charmbracelet/vhs)",
        ),
        ("ttyd", "vhs prerequisite (e.g. apt install ttyd)"),
        ("ffmpeg", "vhs prerequisite + post-processing (e.g. apt install ffmpeg)"),
    ] {
        match tools::lookup(name) {
            Some(path) => println!(
                "  {:<8} found    {} ({})",
                name,
                path.display(),
                tools::version_of(&path)
            ),
            None => {
                println!("  {:<8} MISSING  ({})", name, purpose);
                missing.push(name);
            }
        }
    }
    println!("binaries:");
    for (rel, note) in [
        (
            "target/release/byteshaver",
            "the media pipeline builds it on demand",
        ),
        (
            "target/release/bh-gui-capture",
            "plan 17 P2; the gui stage skips until it exists",
        ),
    ] {
        if repo.join(rel).is_file() {
            println!("  {:<32} found", rel);
        } else {
            println!("  {:<32} not built yet ({})", rel, note);
        }
    }
    if missing.is_empty() {
        println!("all tools present");
        Ok(())
    } else {
        bail!("missing tool(s): {}", missing.join(", "));
    }
}

/// Rewrites both READMEs against the manifest's versions (idempotent; only
/// writes files whose content actually changed).
fn rewrite_readmes(repo: &Path, manifest: &Manifest) -> Result<()> {
    let versions = manifest.versions();
    for rel in ["README.md", "gui/README.md"] {
        let path = repo.join(rel);
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {rel}"))?;
        let rewritten = readme::rewrite(&text, &versions);
        if rewritten != text {
            std::fs::write(&path, rewritten).with_context(|| format!("rewriting {rel}"))?;
            println!("[publish] refreshed image references in {rel}");
        }
    }
    Ok(())
}

/// Link lint over both READMEs (hard error on any dangling docs/img ref).
fn lint_readmes(repo: &Path, docs_img: &Path, extra_present: &BTreeSet<String>) -> Result<()> {
    for rel in ["README.md", "gui/README.md"] {
        let path = repo.join(rel);
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
        readme::lint(rel, &text, docs_img, extra_present)?;
    }
    println!("[lint] all docs/img references resolve");
    Ok(())
}

/// Provenance line for the manifest: `xtask <git describe|dev>; vhs <ver>;
/// ffmpeg <ver>` (plan 17 §8). Tool state at publish time is part of the
/// manifest on purpose: video bytes are not stable across toolchains, so
/// `generated_by` is their freshness signal (plan 17 §9).
fn generated_by(repo: &Path) -> String {
    let describe = std::process::Command::new("git")
        .args(["describe", "--tags", "--always"])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|desc| !desc.is_empty())
        .unwrap_or_else(|| "dev".to_string());
    let vhs = tools::lookup("vhs")
        .map(|path| tools::version_of(&path))
        .unwrap_or_else(|| "missing".to_string());
    let ffmpeg = tools::lookup("ffmpeg")
        .map(|path| tools::version_of(&path))
        .unwrap_or_else(|| "missing".to_string());
    format!("xtask {describe}; vhs {vhs}; ffmpeg {ffmpeg}")
}

/// Cosmetic suffix for "tool not found" messages.
fn tools_hint() -> String {
    match std::env::var_os(tools::MEDIA_TOOLS_PATH_VAR) {
        Some(value) => format!(
            " (searched PATH and {}={})",
            tools::MEDIA_TOOLS_PATH_VAR,
            value.to_string_lossy()
        ),
        None => " (searched PATH)".to_string(),
    }
}


