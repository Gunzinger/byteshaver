# 17 — README media capture pipeline: scripted screenshots & videos of the CLI and GUI

**Size:** L (7 phases, P0–P6) · **Status: implemented** on branch
`plans/media-capture-pipeline` (P0–P5 done; P6 polish partially: no GIF
variants wired into CI, no synthetic cursor, decorations skipped in favor of
chrome-less consistency). Decisions D1–D4 as planned; D5 resolved
cursor-free; D6 resolved as the manifest `stable`/`unstable` classification
(§9 addendum below); D7 resolved dispatch-only. Deviations that landed
differently than specced are noted in §17.

## §17 addendum — implementation deviations (2026-10-02)

- **Renderer**: `egui_kittest`'s `snapshot` feature only provides
  compare/save helpers; rendering requires the **`wgpu` feature** (software
  rasterizer via lavapipe — `mesa-vulkan-drivers`, the same setup rerun uses
  in CI). The GUI `capture` feature therefore pulls `egui_kittest/eframe,wgpu`.
- **Terminal engine**: VHS 0.12 turned out to have more sharp edges than
  §5 assumed, all now handled by the xtask and documented in
  `docs/media/tapes/README.md`:
  - `Output <dir>/` is applied as a rename that **fails silently** when the
    parent is missing or the target exists → the xtask pre-creates
    `out/<scene>/` and verifies artifacts after every tape;
  - **vhs exits 0 even on recording failure** → artifact verification is the
    real signal;
  - frames are recorded into a `MkdirTemp` dir and moved with an
    **error-ignored `os.Rename`** → cross-device moves silently lose all
    frames → the stage sets `TMPDIR` to a same-filesystem dir;
  - the ttyd **cursor paint races `Screenshot`** → terminal stills are built
    from the last `frame-text-*.png` (cursor-free, chrome-less) instead;
  - `Wait+Screen`/`Wait+Line` are unusable for summaries (scrollback/cursor
    line semantics) → tapes sync on a bare `Wait` (prompt returned).
- **Determinism (§9)**: three wall-clock leaks were found and closed —
  fixture **mtimes** (files *and* directories; the GUI "modified" column)
  are normalized to a fixed epoch at stage time; tapes use **fast fixtures**
  so `Time taken: 0 seconds` stays constant and clean their own outputs in
  the hidden setup (stable pre-state, `demo/` stays pristine for GUI
  scenes); and assets that genuinely embed wall-clock output (all demo
  videos: progress redraws; run-state GUI stills: `in 0.4s` durations) are
  classified **`unstable`** in the manifest — `--check` reports their drift
  as advisory instead of failing. Two consecutive full `--check` runs
  report zero stable drift.
- **Scenes**: `gui-tour` runs webp (not avif — the AVIF inspector beat needs
  `dec-heif` for re-decode) and was trimmed to ~113 frames to fit the size
  budget (videos encode `fps=15`, `q50`); `gui-running` may fall back to the
  fresh-report state on fast encoders (documented in the scene).
- **Scenes are Rust** (`build_scenes()` registry), not `scenes.ron` — the
  `Step` enum is serde-ready if data-driven scenes are ever needed.
- **The gui crate became lib + bin** (§6.1) with `install_symbol_fonts`
  honoring `BH_GUI_FONT_DIR`; `kittest_smoke.rs` guards the headless render
  path in CI.

## 0. Problem

The README ships three **static, stale** terminal screenshots inherited from the
imgc-rs era (`docs/img/webp_cmd.webp`, `clean_cmd.webp`, plus the unreferenced
`avif_cmd_TODO.webp` — all showing the old CLI), a commented-out
`<!-- TODO: real GUI screenshot -->` placeholder, and no video material at all.
Every UI/CLI change invalidates them by hand, so they rot. We need a
**code-defined, one-command, CI-automated** pipeline that captures terminal and
GUI material, post-processes it (image processing included), embeds it in the
README, and makes regeneration after updates trivial.

**Goal:** after any change, `cargo xtask media` (or a CI dispatch) regenerates
every asset deterministically-ish from committed scene definitions and opens a
"docs: refresh media" PR when outputs differ.

**Non-goals:**
- recording *manual* interactive sessions (the pipeline is script-first; VHS
  `record` may be used to *draft* tapes, never as the source of truth),
- Windows/macOS-flavored captures (Linux only; the renderers are
  platform-neutral where possible but only Linux is exercised),
- screenshots of the docker/HEIC flows beyond what the default-feature build
  covers (`dec-heif` needs native libs; scene left as an optional tape that
  requires a `dec-heif` build),
- video hosting outside the repo (assets are committed; GitHub renders animated
  WebP from repo files, verified in P0).

## 1. Resolved decisions (owner sign-off 2026-10-01)

| ID | Decision | Choice |
|----|----------|--------|
| D1 | Animated format committed to the repo | **Animated WebP** (matches existing `docs/img/*.webp`, far smaller than GIF; ffmpeg-encoded). GIFs optionally *also* generated for external use, not referenced by the README. |
| D2 | GUI capture technique | **Headless render via `egui_kittest`** (egui's official harness; runs the real `gui::app::App` with no display server, no GPU; deterministic; CI-friendly). Xvfb screen-recording is the documented escape hatch, not the primary path. |
| D3 | Terminal scripting/recording | **VHS** (charmbracelet) `.tape` files — typed sessions as code with pinned font/theme/geometry, `Wait /regex/` synchronization, PNG-frame output. |
| D4 | Automation level | **CI job + auto-PR**: manual `workflow_dispatch` (+ optional weekly schedule) regenerates everything in a hermetic runner env and opens a PR when `docs/img/`, goldens or README change. |

## 2. Architecture overview

```
                    cargo xtask media  (single entry point, also driven by CI)
                    ┌───────────────────────────────────────────────────────────┐
                    │ 1. stage: rsync docs/media/fixtures → target/media-stage   │
                    │    (copy pinned inputs; isolated XDG dirs; copies the      │
                    │     freshly built release binaries in as `byteshaver`)    │
                    │                                                           │
                    │ 2. terminal engine:  vhs docs/media/tapes/*.tape           │
                    │      → PNG frame sequences (+ .ascii golden text)          │
                    │                                                           │
                    │ 3. gui engine:  target/release/bh-gui-capture --scene …    │
                    │      (egui_kittest harness drives gui::app::App through    │
                    │       scripted states) → PNG stills + PNG frame sequences  │
                    │                                                           │
                    │ 4. post-process (ffmpeg + byteshaver itself):              │
                    │      2x→1x lanczos downscale, animated WebP encode,        │
                    │      still WebP via the byteshaver CLI (dogfooding),       │
                    │      optional GIF palette variant, decorations             │
                    │                                                           │
                    │ 5. publish: write docs/img/*, update docs/media/manifest   │
                    │    .json (sha256 + cache-bust version), rewrite README     │
                    │    image URLs' ?v= parameters for changed assets only     │
                    └───────────────────────────────────────────────────────────┘

CI: .github/workflows/media.yaml ── regenerates ──▶ git diff? ──▶ auto-PR
```

Design principles:
- **Media as code**: everything referenced by the README is generated from
  committed sources (`docs/media/`); nothing is edited by hand after generation.
- **One runner**: the xtask orchestrates external tools (vhs, ffmpeg) and the
  Rust capture binaries; scene definitions are data next to the code that
  consumes them.
- **Deterministic where it matters** (fonts, theme, geometry, fixture bytes,
  virtual time in the GUI harness), **tolerant where it can't be** (progress-bar
  wall-clock animation; see §9).
- **Cache-busting is part of the pipeline** (§8): GitHub's camo proxy caches
  images aggressively; a changed asset whose URL stays identical keeps showing
  the old pixels for a long time. The xtask bumps `?v=` query strings.

## 3. Repository layout (new/changed)

```
docs/
  img/                      # committed FINAL assets referenced by docs (webp/gif/png)
  media/                    # committed SOURCES ("media as code")
    README.md               # authoring guide: how to add a scene, tool versions
    manifest.json           # xtask-managed: { asset: { sha256, version } }
    theme/vhs.json          # VHS terminal theme (byteshaver brand palette)
    fonts/JetBrainsMono/    # terminal font (OFL, license file committed) +
    fonts/symbols/          #   Noto Sans Symbols 2 + DejaVu copies for the GUI
    fixtures/               # pinned demo inputs (see §5.1)
    tapes/                  # *.tape terminal scenes + _shared.tape
    gui/scenes.ron          # GUI scene definitions (see §6.3)
    golden/                 # committed .ascii terminal goldens + GUI still
                            #   sha256s (advisory freshness signal, §9)
  plans/17-media-capture-pipeline.md
xtask/                      # new workspace member (cargo xtask media)
gui/
  src/lib.rs                # NEW: crate restructured lib+bin (§6.1)
  src/main.rs               # thin binary, unchanged behavior
  src/capture.rs            # NEW, feature "capture": demo driver + kittest runner
  src/bin/bh-gui-capture.rs # NEW, feature "capture": CLI around capture.rs
.github/workflows/media.yaml
.cargo/config.toml          # [alias] media = "run --release -p xtask --"
Cargo.toml                  # workspace members += "xtask"
```

`target/media-stage/` (gitignored) is the scratch workspace every run rebuilds
from `docs/media/fixtures/` — captures never touch `examples/` or the repo tree,
so generated outputs never feed back into later runs.

## 4. xtask orchestration crate

Plain `cargo xtask` pattern (no external task runner; the repo has no justfile
and `.cargo/config.toml` already exists for rustflags).

Subcommands:
- `cargo xtask media` — full pipeline (stage → terminal → gui → post → publish).
  Flags: `--only terminal|gui|<scene>`, `--skip-video`, `--dry-run` (no writes
  outside `target/`), `--check` (regenerate into `target/`, compare hashes
  against `docs/img/` + manifest, exit 1 listing stale assets — no publish).
- `cargo xtask media readme` — only the §8 publish/rewrite step.
- `cargo xtask media deps` — print/assert the external tool versions (vhs,
  ffmpeg, ttyd) against `docs/media/README.md` pins; CI runs this first.

Implementation notes:
- Deps (keep the tree lean; all already in the workspace graph or tiny):
  `clap` (derive), `anyhow`, `serde`/`serde_json`, `sha2`, `xshell` (command
  plumbing; fall back to `std::process::Command` if xshell is unwanted).
- **Staging** (`target/media-stage/`): copy fixtures; write a `demo/` tree with
  realistic names (`photos/landscape.jpg`, `screens/shot-01.png`,
  `anim/loading.gif`, …) so queue tables and globs look natural on camera; copy
  `target/release/byteshaver` in; export isolated env for children:
  `HOME`/`XDG_CONFIG_HOME`/`XDG_CACHE_HOME` → stage dirs (GUI settings.json and
  anything the CLI caches must never leak between runs or read the developer's
  real config — this also guarantees the GUI always starts in first-run state).
- The xtask **must build** `byteshaver` (release, default features) and
  `byteshaver-gui --features capture` first; it uses the *same* `byteshaver`
  binary afterwards to optimize still PNGs (§7), so the tool documents itself.
- Exit codes: non-zero if any scene fails or if `--check` finds drift; print a
  per-scene table (scene → asset → status → Δbytes).

## 5. Terminal engine — VHS

### 5.1 Verified facts (2026-10)

- VHS (`charmbracelet/vhs`, MIT) requires `ttyd` + `ffmpeg` on `PATH`; install
  via brew/pacman/nix/deb packages or `go install …@<pin>`.
- `Output` supports **gif / mp4 / webm / a directory of PNG frames** — **no
  WebP output**. ⇒ tapes emit **PNG frame sequences only**; animated WebP is
  produced by our ffmpeg post-process (§7). This also keeps the encode single
  generation (no lossy mp4 intermediate).
- `Wait[/+Line/+Screen] /regex/` waits for screen content (default timeout 15s)
  — used instead of fixed sleeps to absorb encode-time variance.
- `Screenshot <path>.png` grabs the current frame → terminal **stills**.
- `Set Theme {json}`, `Set FontFamily/FontSize`, `Set Width/Height` or
  `Set Columns/Rows`, `Set Padding/Margin/MarginFill`, `Set WindowBar`,
  `Set BorderRadius`, `Set Framerate`, `Set TypingSpeed`, `Set CursorBlink`,
  `Set PlaybackSpeed`, `Set LoopOffset`, `Hide/Show`, `Env`, `Source <tape>`,
  `Require <prog>` — everything needed for consistent branding.
- `.ascii`/`.txt` `Output` produces a golden text dump of the final screen —
  the basis of the advisory freshness check (§9).
- Official CI path exists (`charmbracelet/vhs-action`), but we run the `vhs`
  binary directly from the xtask for one code path in CI and locally.

### 5.2 Tape conventions

- `docs/media/tapes/_shared.tape` is `Source`d by every scene and pins:
  `Set Shell "bash"`, `Set FontFamily "JetBrains Mono"` (from
  `docs/media/fonts/`, installed into the stage font dir via `FONTCONFIG_PATH`
  so runners need no font packages), `Set FontSize 22`, `Set Framerate 30`,
  `Set TypingSpeed 45ms`, `Set Theme {docs/media/theme/vhs.json contents}`,
  `Set Margin 16` + `Set MarginFill "#16181d"`, `Set WindowBar "Colorful"`,
  `Set CursorBlink false` (one less nondeterminism source), `Set Width/Height`
  per-scene override allowed, default 1000×560.
  (2x crispness trick: render at ~2x font/size, downscale in post — §7.)
- Every tape starts `Require byteshaver` and `Hide`/`Show` around setup so the
  staging `cd` is invisible.
- Completion sync: after each conversion `Enter`, `Wait+Line /Compression ratio:/`
  (or `/Successful:/`), then a short `Sleep 700ms` for the summary to settle.
  This is what makes re-capture robust against encoder timing changes.
- Output block per tape:
  ```
  Output ../stage/<scene>/frames/     # PNG sequence (primary)
  Output ../golden/<scene>.ascii      # committed golden (advisory)
  Screenshot ../stage/<scene>/still.png   # where a README still is wanted
  ```
  (paths relative to the tape's cwd; the xtask invokes vhs with
  `cwd = target/media-stage/tapes` so tapes stay repo-relative and portable.)

### 5.3 Initial terminal storyboard

| tape | content | asset |
|------|---------|-------|
| `cli-basic` | `./byteshaver "demo/**/*.png" webp -o out` — typing, progress, summary | `docs/img/cli-basic.webp` (hero candidate) + still |
| `cli-avif` | `… avif -q 85` recipe (replaces `avif_cmd_TODO.webp`) | `cli-avif.webp` |
| `cli-exif` | `… --exif filter --exif-except gps,GPSInfo` | `cli-exif.webp` |
| `cli-anim` | `… "anim/*.gif" webp-anim` | `cli-anim.webp` |
| `cli-clean` | `clean` command demo (replaces `clean_cmd.webp`) | `cli-clean.webp` |
| `cli-jxl` (opt) | jxl target with `--quality` | `cli-jxl.webp` |

Old `webp_cmd.webp` / `clean_cmd.webp` / `avif_cmd_TODO.webp` are **deleted** in
the same change; README links rewritten to the new names with relative paths
(no leading `/` — the current `/docs/img/…` form breaks anywhere but GitHub).

### 5.4 Fixture policy (`docs/media/fixtures/`)

- Committed, byte-pinned inputs: a handful of small photographs (reuse
  `examples/` images — already MIT-released in-repo), one tiny animated GIF,
  one animated WebP, one APNG, one JXL still, one PNG with EXIF (synthesized
  via `exiftool`-free Rust or committed binary — must contain GPS + Orientation
  so the exif scene shows meaningful output).
- Sizes chosen so a full scene encodes in **≤ ~3–4 s** (progress bar visible
  but the video stays short); count chosen so the summary line is interesting
  (~10–15 files).
- An `anim/` and a `photos/` subtree give the GUI table natural variety.
- Fixtures are **inputs only**; generated outputs live exclusively in the stage
  dir and never get committed.

## 6. GUI engine — headless `egui_kittest`

### 6.1 Verified facts (2026-10)

- Crate **`egui_kittest` 0.36.2** exists and tracks the egui line exactly
  (`egui ^0.36`, `eframe ^0.36`, `egui_extras ^0.36` — all present in
  `gui/Cargo.toml` after the pending 0.36 upgrade). Features: `eframe`
  (headless harness that runs a real `eframe::App`, constructing the
  `CreationContext` without a window/display), `snapshot` (PNG/image output +
  `dify` comparisons), optional `wgpu` (GPU-parity rendering; **off** — default
  software rasterizer is what we want in CI), `x11`. MSRV 1.95 ≤ workspace 1.98.
- The harness pumps the app's update loop with simulated time and exposes the
  app for mutation between steps, plus snapshot-to-image. Exact method names
  (`EframeHarness::builder()…build(...)`, `run`, `run_until`, `app_mut`,
  `snapshot`) are to be confirmed against the 0.36 docs during P0 — the plan's
  adapter trait (§6.2) isolates the call sites so API drift stays mechanical.
- The GUI's `App` is already driver-friendly (audited): `App::with_settings`,
  `queue.add_paths`, `select_encoder`, `apply_preset`, `build_job_spec`,
  `start_job`, `cancel_job`, `drain_events`, `open_inspector`,
  `measure_quality` — the demo driver needs **no synthetic mouse input** for
  the core story; kittest's widget-level interactions (click on chips/rows) are
  available for optional polish.
- Conversions run through the same headless core as the CLI (WS7 job API) in
  worker threads ⇒ the harness must pump until quiescent; same for thumbnail
  decode and DSSIM/PSNR metric workers.

### 6.2 Crate restructure + feature

- `gui/` becomes **lib + bin**: `gui/src/lib.rs` re-exports the existing
  modules (`app`, `panels`, `queue`, …); `gui/src/main.rs` shrinks to the
  current `main()` (fonts install + `run_native`). Zero behavior change;
  `cargo build -p byteshaver-gui` produces the same binary (release artifacts
  unaffected — verify the workflow's `-p byteshaver-gui` build still picks
  `[[bin]] byteshaver-gui`).
- New **`capture` cargo feature** (default off) in `gui/Cargo.toml`:
  `capture = ["dep:egui_kittest", "dep:serde", "dep:image"]`; adds
  - `gui/src/capture.rs` — the scene runner (§6.3) and the thin
    `CaptureHarness` wrapper isolating the egui_kittest API surface,
  - `gui/src/bin/bh-gui-capture.rs` — `--scene <id> [--frames-dir D]
    [--still P] [--list]` CLI used by the xtask.
  Nothing capture-related compiles into shipped binaries.
- Fonts: `main.rs::install_symbol_fonts` currently probes OS font paths. The
  lib gains an env override (`BH_GUI_FONT_DIR`, checked first) that the capture
  binary points at `docs/media/fonts/symbols/` (committed Noto Sans Symbols 2 +
  DejaVu copies, OFL) so status glyphs (⤷ ✂ ⏸ ⟳ ⤬ ⬇) render identically on
  runners and dev machines.
- Settings isolation: `Settings::load_or_default` runs with the staged
  `XDG_CONFIG_HOME`; scenes that want a non-default window size construct
  `Settings` with `window_size = Some([1280, 800])` (2× the README display
  width, fed to the harness as the viewport size; combined with
  `pixels_per_point = 2.0` this yields ~2560-px masters → 1280-px finals).
- **Viewports** (report window, inspector, preset modals are separate OS
  windows via `show_viewport_immediate`): a single-context harness snapshot
  captures the parent viewport only. Under `capture`, `viewports.rs` gains an
  adapter that renders the same panel content in an in-window `egui::Window`
  (with an egui-drawn title bar approximating the native chrome). Production
  behavior is untouched; captured stills show the in-window variant. If the
  in-window approximation proves visually dishonest, the fallback is the Xvfb
  escape hatch (§11) — but it should not be needed for these panels.
- **No native dialogs in scenes**: `rfd` pickers cannot be automated headless;
  scenes inject paths via the driver. Drag-and-drop hover visuals
  (`hovered_files` overlay) can be faked by injecting a `RawInput` with
  `hovered_files` if the harness allows; otherwise the tour video cuts from
  empty → populated queue (P6 polish decides; not blocking).

### 6.3 Scene runner (`capture.rs`)

Scenes are data (`docs/media/gui/scenes.ron`, parsed via serde into the gui
crate so adding a scene needs no Rust edit):

```ron
GuiTour: [
  Reset,                                  // fresh Settings + fixture queue state
  Snapshot("00-empty"),
  AddPaths(["demo/photos", "demo/anim", "demo/screens/logo.png"]),
  PumpUntilThumbs,  Snapshot("01-queue"),
  SelectEncoder("avif"), SetQuality(85),
  OpenOptions,      Snapshot("02-options"),
  Start,
  PumpUntil(JobActive),  Snapshot("03-running"),   // mid-progress frame
  PumpUntil(JobDone),    Snapshot("04-report"),    // confetti armed/frame chosen
  OpenInspector(row: 1), Snapshot("05-inspector"),
]
```

- `PumpUntil(cond)`: run the harness loop in small time steps (e.g. 1/30 s
  virtual time per frame — egui animations, the segmented progress shimmer and
  `celebrate.rs` confetti all key off context time, so virtual time yields
  smooth, reproducible animation), exiting when the predicate on `&App` holds
  (job idle, thumbnail cache quiescent, metrics workers drained).
- **Stills** = `Snapshot(id)` → PNG master (2×) in the stage dir.
- **Videos**: `--frames-dir` writes every pumped frame as a numbered PNG; the
  tour video is just a scene whose steps are all `Pump*` with snapshots off.
  Virtual-time stepping makes the video's duration a scene parameter
  (`Settle(millis)` steps), decoupled from real encode wall time (a 3-s real
  encode is stretched/compressed to the scripted beat).
- Quiescence detectors reuse existing state: `App` knows the run state
  (`start_job`/`drain_events` maintain it), `thumb.rs` the decode queue depth,
  `metrics/worker.rs` the pending metric count — tiny `pub(crate)` accessors if
  not already exposed.

### 6.4 Initial GUI storyboard

| scene | state | asset |
|-------|-------|-------|
| `gui-empty` | first-run drop zone | still `gui-empty.webp` |
| `gui-queue` | mixed-format queue, thumbnails, sorted by size | still |
| `gui-options` | encoder chips + preset dropdown + EXIF policy | still |
| `gui-running` | mid-conversion: segmented bar, per-row glyphs | still |
| `gui-report` | finished run: ratios, totals, confetti frame | still |
| `gui-inspector` | visual-diff viewer on one output | still |
| `gui-tour` | empty → queue → options → run → report (≈12 s @30fps) | `gui-tour.webp` |

README: GUI section gets a two-column block (tour video + one still) replacing
the TODO comment; a small media strip lands near the top (§8).

## 7. Post-processing (the "image processing" leg)

All post-processing is xtask-invoked `ffmpeg` (pinned flags) + the freshly
built `byteshaver` binary itself:

1. **Downscale**: masters are rendered at 2× (GUI `pixels_per_point = 2.0`;
   VHS via doubled font/geometry) → `scale=iw/2:ih/2:flags=lanczos`,
  un-sharp our terminal text slightly if needed (`unsharp=5:5:0.4`).
2. **Animated WebP** (frame dirs):
   `ffmpeg -framerate 30 -i frames/%04d.png -vf "<scale chain>"
    -c:v libwebp -lossless 0 -q:v 62 -loop 0 -an out.webp`
   (q/framerate per-scene overridable in the manifest; target ≤ ~1.2 MB per
   asset, hard budget 2 MB — the xtask prints sizes and warns).
3. **Still WebP**: run `byteshaver master.png webp -q 92` — the pipeline's own
   product optimizes the README's images (one real invocation per still,
   exercised in CI; a nice dogfooding line for the README itself).
4. **Optional GIF variant** (external use only):
   `palettegen/paletteuse` two-pass, `fps=12`, width ≤ 800.
5. **Decorations** (P6, off by default, per-asset flag in the manifest):
   terminal chrome via VHS `WindowBar`/`MarginFill` (native, preferred);
   GUI stills rounded corners + soft shadow via ffmpeg `pad`+`geq` alpha
   feather or a tiny `image`-crate pass inside xtask (Rust, testable).
   No caption bars initially — text in images rots fastest.

Masters (PNG frames + stills) stay in `target/` only; the repo commits finals +
goldens, keeping repo growth bounded (§10 budgets).

## 8. README integration & cache busting

- **Manifest** `docs/media/manifest.json`: `{ "cli-basic.webp": { "sha256": …,
  "version": 3, "generated_by": "…tool versions…" } }` — the xtask's source of
  truth for "what is current".
- **Publish step**: after regenerating, for each asset whose sha256 changed,
  bump its `version` and rewrite **every** reference in `README.md` (and
  `gui/README.md` if it ever grows images) from `path?v=<old>` to
  `?v=<new>`; unchanged assets keep their version. This defeats GitHub's camo
  image cache (the proxy key includes the query string) — the exact failure
  mode that would otherwise keep serving stale screenshots after regeneration.
  The rewrite is a strict, unit-tested regex pass (`docs?/img/<name>(?:\?v=\d+)?`),
  and it fails loudly on references to assets that don't exist (link lint).
- New README section order: a compact media strip after the intro (one terminal
  video, one GUI video), CLI examples with fresh stills/videos, GUI section
  with the tour. All paths relative (`docs/img/…`).
- Note documented in `docs/media/README.md`: crates.io does not render relative
  repo images (pre-existing behavior, unchanged by this plan).

## 9. Determinism & freshness policy

**Stable across regenerations:** fixture bytes, fonts, theme, geometry, typing
speeds, VHS `Wait`-anchored pacing, GUI virtual-time beats, still-image
compositions (same code path ⇒ pixel-identical stills; sha256-stable).
**Intentionally unstable:** progress-bar wall-clock animation inside videos,
spinner phases, encoder-version-driven output sizes/timings (that is the
*point* of refreshing), WebP container metadata.

Consequences:
- **Stills** are hash-comparable ⇒ `cargo xtask media --check` gives a hard
  stale/fresh signal for them; GUI still goldens (sha256 in manifest) catch
  accidental UI drift in CI *between* deliberate refreshes.
- **Videos** are not byte-stable; freshness is tracked by the manifest's
  `generated_by` (tool + crate versions) and the auto-PR diff (size changes).
- **`.ascii` goldens** (final terminal screen text) compare the *semantic*
  output of the CLI; progress artifacts may pollute them (spinner glyphs,
  partial redraws) ⇒ goldens are **advisory**: `--check` reports diffs without
  failing, and the CI job comment summarizes them. If they prove stable in P0,
  promote to hard-failing (D6).
- Freshness loop: PR merges that touch `src/**`, `gui/**` or CLI help don't
  block on media; the weekly/dispatched media job regenerates and the auto-PR
  makes the delta reviewable (one PR per concurrency group; stale open media
  PRs are superseded, not stacked).

## 10. CI — `.github/workflows/media.yaml`

Following `workflow.yaml` conventions (pinned major action tags, Swatinem
rust-cache, explicit permissions):

- **Triggers:** `workflow_dispatch` (inputs: `only` — scene filter, `dry_run`),
  optional `schedule: weekly` (D7; committed disabled-by-default via a comment
  until trust is earned), plus `workflow_dispatch` on **`check`** mode wired as
  a required-ish advisory job later.
- **Runner job `refresh`:** `runs-on: ubuntu-latest`, env `XDG_*` → stage;
  permissions `contents: write`, `pull-requests: write`.
  1. checkout; rust-toolchain (stable, match workflow.yaml);
     `Swatinem/rust-cache@v2`.
  2. apt: `ffmpeg`, `ttyd` (ubuntu carries both; versions recorded by
     `xtask media deps` into the PR body for traceability); fonts come from
     the repo (`FONTCONFIG_PATH` → `docs/media/fonts`).
  3. vhs: download the pinned release `.deb`/tarball from
     `charmbracelet/vhs` releases (fallback documented: `go install
     github.com/charmbracelet/vhs@<pin>`; the runner images carry Go).
  4. `cargo build --release` (CLI; default features incl. jxl — cmake/nasm
     present on the runner, same as the main build) and
     `cargo build --release -p byteshaver-gui --features capture`.
  5. `cargo xtask media --only <input>` (or `--check` for the advisory job).
  6. `peter-evans/create-pull-request@v7` (branch `docs/media-refresh`,
     title `docs: refresh README media`, labels `documentation`, `media`;
     body: per-scene table with Δbytes, tool versions, golden summary).
     `concurrency: media-refresh` collapses parallel runs.
- **Advisory job `check`** (optional separate dispatch): `--check` only, never
  opens a PR; posts a stale-asset list as a workflow summary.
- Docker fallback if the runner's ttyd/vhs combination misbehaves: run tapes
  inside `ghcr.io/charmbracelet/vhs` with the workspace mounted **and** a musl
  CLI build mounted in (the repo already produces musl binaries in CI, so the
  recipe exists) — documented in `docs/media/README.md`, not wired by default.

## 11. Risks & mitigations

| # | Risk | Mitigation |
|---|------|-----------|
| R1 | GitHub does not render animated WebP in READMEs (assumption D1) | **P0 spike**: push one ffmpeg-made animated WebP to a throwaway branch, eyeball the rendered README; fallback = GIF for animations (pipeline unchanged — swap §7.2 encode). |
| R2 | `egui_kittest` 0.36 eframe-harness API differs from the assumed shape | P0 spike renders the current `App` headlessly within a day; the `CaptureHarness` wrapper keeps drift mechanical. If the harness can't host our `App` at all, Xvfb path (below) is the fallback. |
| R3 | Report/inspector/modals live in separate viewports; harness captures parent only | `capture`-feature in-window rendering adapter (§6.2); escape hatch: `xvfb-run` + `ffmpeg -f x11grab` manual recipe documented in `docs/media/README.md` for hero shots. |
| R4 | Software rasterizer vs GPU glow rendering differs subtly (AA, blending) | Acceptable for docs; `wgpu` feature exists for a GPU-parity pass locally if ever needed. |
| R5 | VHS/ttyd version drift breaks tapes | `Require byteshaver` in tapes, `xtask media deps` pin check first in CI, golden `.ascii` diffs surface behavioral drift. |
| R6 | `.ascii` goldens flaky due to progress redraws | Advisory-only by default (D6); scenes can end with `clear`-free `Sleep` and compare tail lines only if promoted. |
| R7 | camo cache keeps serving stale pixels despite regeneration | `?v=` bump machinery (§8) is the designed countermeasure; verify once in P5 with a real refresh PR. |
| R8 | Repo bloat from committed media | §7 budgets (≤ ~1.2 MB/asset, ~10–14 assets ⇒ well under ~20 MB total), masters gitignored, one-time deletion of the three legacy webps (~300 KB back). |
| R9 | GUI capture builds eframe with x11/wayland features on a headless runner | Build-time only needs no display (already true for the existing CI GUI builds); runtime never opens a window. Verified in P0. |
| R10 | Long-term maintenance of scene definitions as the GUI evolves | Scenes are data + tiny predicates; `gui-tour` steps map 1:1 onto stable `App` methods that the GUI's own tests already exercise; stale scenes fail loudly (`--check`), not silently. |

## 12. Remaining decision points

- **D5 tour-video cursor**: show a synthetic cursor dot moving to where the
  driver acts (ffmpeg overlay), or cursor-free? *Recommend: cursor-free v1.*
- **D6 golden strictness**: advisory vs hard-failing `.ascii` goldens.
  *Recommend: advisory until proven stable (P0/P4 evidence).*
- **D7 schedule**: weekly auto-refresh PR vs dispatch-only.
  *Recommend: dispatch-only initially; enable weekly once two manual refreshes
  have been boring.*

## 13. Phased implementation

| Phase | Content | Size | Exit criteria |
|-------|---------|------|---------------|
| **P0** | Spikes: R1 (animated WebP on GitHub), R2 (kittest hosts the real `App`, still of the current drop zone), VHS smoke (one `cli-basic` tape → frames → webp locally), `.ascii` stability across 3 runs | S | all four spikes documented in this file's addendum; go/no-go per risk |
| **P1** | xtask skeleton: staging, env isolation, manifest read/write, `--check` hashing, `deps` command; fixtures tree | M | `cargo xtask media --dry-run` stages reproducibly; `--check` detects a doctored asset |
| **P2** | gui lib+bin split, `capture` feature, `CaptureHarness`, scene runner + quiescence predicates, 7 GUI stills, still post-process (downscale + byteshaver webp), README embed + `?v=` machinery + link lint, delete legacy webps | L | `cargo xtask media --only gui` regenerates stills pixel-identically twice; README renders with fresh stills; release build bit-for-bit unchanged vs pre-split (modulo build id) |
| **P3** | GUI video: virtual-time frame pump → frames dir → webp encode; `gui-tour` scene | M | `gui-tour.webp` ≤ budget, loop looks smooth, duration is a scene constant |
| **P4** | Terminal: `_shared.tape`, theme, fonts, six tapes, `Wait` sync, ascii goldens, still+video post-process | M | `cargo xtask media --only terminal` green locally and on the runner; goldens committed |
| **P5** | `media.yaml`: dispatch + auto-PR + advisory `check` job; `docs/media/README.md` authoring guide | M | one real refresh PR lands via CI with a meaningful diff table |
| **P6** | Polish: optional decorations (rounded corners/shadow), GIF variants, cursor (D5), media strip layout, `gui/README.md` stills | S | README reviewed; guide sufficient for a newcomer to add a scene unaided |

## 14. Touched files / ownership

| File | Change |
|------|--------|
| `Cargo.toml` | workspace members += `xtask` |
| `gui/Cargo.toml` | `[lib]` + `[[bin]] byteshaver-gui` + `[[bin]] bh-gui-capture` (feature-gated), `capture` feature, `egui_kittest` 0.36 dep |
| `gui/src/lib.rs`, `gui/src/main.rs` | new lib surface; main thins out |
| `gui/src/capture.rs`, `gui/src/bin/bh-gui-capture.rs` | new (feature-gated) |
| `gui/src/viewports.rs`, `gui/src/main.rs` (font dir env) | small `#[cfg(feature = "capture")]` adapters |
| `README.md` | media strip, GUI screenshot block, new asset links with `?v=`, legacy links removed |
| `.github/workflows/media.yaml` | new; `workflow.yaml` untouched |
| `.cargo/config.toml`, `.gitignore` | alias; `target/media-stage/` |
| `docs/img/*`, `docs/media/**` | generated finals / committed sources |

## 15. Verification

- `cargo test --workspace` extended with: manifest bump/rewrite unit tests
  (README regex), scene-parser round-trip, quiescence predicates (fake app
  states), version-param idempotence.
- Determinism: `cargo xtask media --only gui-stills` twice → identical sha256s;
  `--check` clean afterwards.
- Release parity: build `byteshaver-gui` with and without the restructure on
  the same commit; binaries behave identically (manual smoke: drop, convert,
  report, inspector).
- CI: advisory `check` green on a clean tree; doctored `docs/img` file ⇒
  `--check` fails with the asset named; dispatch job opens exactly one
  superseding PR per concurrency group.
- README review checklist: all links resolve (link lint), animations animate
  on github.com (R1), no asset exceeds budget, `?v=` bumped only for changed
  assets.

## 16. Verified ecosystem facts (2026-10)

- `egui_kittest` 0.36.2 (MIT OR Apache-2.0): `egui ^0.36` / `eframe ^0.36` /
  `egui_extras ^0.36`; features `eframe`, `snapshot` (image + dify), optional
  `wgpu`/`x11`; MSRV 1.95 (sparse-index dump inspected).
- VHS (MIT): outputs gif/mp4/webm/**PNG-frame dirs** (no WebP), `Wait /regex/`,
  `Screenshot`, `Source`, `Env`, `Require`, theme JSON, font/geometry/margin/
  windowbar/framerate controls; needs `ttyd` + `ffmpeg`; docker image
  `ghcr.io/charmbracelet/vhs`; CI action exists (README inspected 2026-10).
- ffmpeg (ubuntu runner / local): `libwebp` encoder with `-loop 0` animated
  WebP support, `palettegen/paletteuse`, `lanczos` scaling.
- GitHub README: animated WebP rendering assumed pending R1 spike; camo cache
  keyed by full source URL incl. query string (basis of `?v=` busting).
- GUI internals audited for driveability: `App::{with_settings, select_encoder,
  apply_preset, start_job, cancel_job, drain_events, open_inspector,
  measure_quality}`, `Queue::{add_paths, begin_run, finish_run}` exist with
  usable signatures (`gui/src/app.rs`, `gui/src/queue.rs`).
