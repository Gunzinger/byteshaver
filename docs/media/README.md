# media as code - the README's generated assets

Everything the README embeds from `docs/img/` is generated from the sources in
this directory (plan `17-media-capture-pipeline`): terminal sessions are
scripted VHS tapes, GUI shots are headless egui_kittest scenes, post-processing
is pinned ffmpeg + the byteshaver CLI itself. **Never edit `docs/img/` (or the
`?v=` numbers in the READMEs) by hand** - regenerate instead; hand edits are
detected by `--check` as `manifest-drift`.

Masters (PNG frames/stills) stay in `target/media-stage/` (gitignored); the
repo commits only finals (`docs/img/`) and the manifest.

## Regeneration

    cargo xtask media        # stage -> terminal -> gui -> post -> publish

The `.cargo/config.toml` alias `media = "run --release -p xtask --"` is a
drop-in for `xtask` itself, so the full pipeline is `cargo media media` -
but top-level commands save typing: `cargo media gen-fixtures`.

Stages (run in order; each is also a `--only` name):

| stage | what it does | sources |
|-------|--------------|---------|
| `stage` | rebuilds `target/media-stage/` from the pinned fixtures, copies the freshly built release binaries in, isolates `HOME`/`XDG_*`/`TMPDIR` for every child process | `docs/media/fixtures/` |
| `terminal` | renders every `tapes/*.tape` (vhs) into frame dirs, stills and `.ascii` goldens | `docs/media/tapes/`, `theme/`, `fonts/JetBrainsMono/` |
| `gui` | runs each registered capture scene headlessly (kittest, software Vulkan; builds `bh-gui-capture --features capture` on demand) | `gui/src/capture.rs`, `fonts/symbols/` |
| `post` | downscales (2x masters -> 1x lanczos), composites, encodes videos (ffmpeg) and stills (byteshaver webp) | - |
| `publish` | writes `docs/img/`, bumps manifest versions, rewrites README `?v=` params, lints image links | `docs/media/manifest.json` |

Common invocations:

| command | effect |
|---------|--------|
| `cargo xtask media` | regenerate + publish everything |
| `cargo xtask media --only gui` | only the GUI stage (still + video scenes) |
| `cargo xtask media --only terminal` | only the vhs tapes |
| `cargo xtask media --only gui-tour` | a single scene (tape stem or gui scene id) |
| `cargo xtask media --only terminal,post` | stages/scenes combine (comma-separated or repeated flags) |
| `cargo xtask media --skip-video` | stills only (no frame sequences) |
| `cargo xtask media --check` | regenerate into `target/` only, print a freshness table, exit 1 on stale/missing stable assets (never publishes) |
| `cargo xtask media --dry-run` | everything except writes outside `target/` (publish previews) |
| `cargo xtask media --gif` | additionally emit palette-optimized GIF variants (fps=12, max width 800; external use only) |
| `cargo xtask media readme` | only the `?v=` rewrite + link lint against the current manifest |
| `cargo xtask media deps` | print tool/binary versions, exit non-zero listing what is missing (CI runs this first) |
| `cargo xtask gen-fixtures` | deterministically regenerate `docs/media/fixtures/` (see its README) |

`--check` reports committed assets the run did not regenerate as `skipped`
(not failures); `stale`, `missing` and `manifest-drift` on **stable**
assets fail the run - see [freshness classes](#freshness-classes).

## Tool bootstrap

The pipeline needs `vhs`, `ttyd`, `ffmpeg` and a `chrome` binary (vhs renders
through a headless Chromium via go-rod). Do not apt-install them;
`docs/media/bootstrap-tools.sh` installs pinned releases, user-local and
idempotent, into `target/media-tools/bin`:

| tool | pin | source |
|------|-----|--------|
| ffmpeg (+ffprobe) | 7.0.2 | johnvansickle.com static builds |
| ttyd | 1.7.7 | tsl0922/ttyd releases |
| vhs | 0.12.1 | charmbracelet/vhs releases |
| chrome | snapshot 1321438 | chromium-browser-snapshots (the revision vhs's go-rod would auto-fetch) |

It rewrites `target/media-tools/VERSIONS.txt` on every run (versions parsed
from the binaries themselves). Re-download with `--force`; bump pins by
editing the `*_VERSION` variables in the script and record the bump in
`docs/media/fonts/SOURCES.md`.

The xtask resolves tools from the **`MEDIA_TOOLS_PATH`** env var first (a
colon-separated bin-dir list), then `PATH`:

    export MEDIA_TOOLS_PATH="$PWD/target/media-tools/bin"

`cargo xtask media deps` shows what was found where.

## Scene authoring

### Terminal scenes (vhs tapes)

Add a `.tape` to `docs/media/tapes/` (scene id = file stem). Conventions:

- first line `Source "_shared.tape"` (quoted - unquoted `_` paths fail to
  lex); it pins shell, font, theme, geometry, framerate and `Require`.
- hidden setup block (`Hide` ... `Show`), outputs into `out/<scene>/`
  (frames dir + `.ascii` golden + `Screenshot .../still.png`),
- sync conversions with a **bare `Wait`** (prompt regex), never
  `Wait+Screen` (scrollback quirk), end with `Screenshot` + `Sleep 700ms`.

Full conventions, output layout and the vhs 0.12 quirk list:
[`tapes/README.md`](tapes/README.md).

### GUI scenes

Add a `Scene` to `build_scenes()` in `gui/src/capture.rs` (discovery:
`bh-gui-capture --list`). Steps are a small enum: `Snapshot` (still),
`AddPaths`, `SelectEncoder`, `SetQuality`, `SetOutputDir`, `Start`,
`PumpUntil` (quiescence predicate), `Pump`/`Stable` (timed frames),
`Animate`, `OpenReport`/`OpenInspector`. A scene with `video: true`
additionally dumps every pumped frame (`[video]` marker in `--list`) and
becomes a video asset; still scenes become still assets. Scenes run against a
fresh first-run `App` with the staged fixtures - the input contract
(`demo/photos|anim|screens`, EXIF-bearing `logo.png`) is pinned in
[`fixtures/README.md`](fixtures/README.md).

## Post-processing and size budget

- videos: `fps=15`, `libwebp -lossless 0 -q:v 50 -loop 0`; GUI masters are
  2x and downscaled with lanczos, terminal frames are already 1x (cursor
  layer composited over the text layer).
- stills: byteshaver encodes them itself (`webp -q 92`) - the pipeline
  dogfoods its own product on every run. Terminal stills are built from the
  **last `frame-text-*.png`** (pure text layer, no cursor): vhs's
  `Screenshot` composite races the ttyd cursor paint and flip-flops between
  two sha256s, the text layer is byte-stable. The cost: terminal stills are
  chrome-less (no window bar/margin) - same raw look as the videos.
- budget: **<= ~1.5 MB per asset**. An asset pushing ~2 MB is over budget:
  trim the scene (fewer/shorter beats, smaller fixture set), do not crank
  encoder quality down.

## Freshness classes

The manifest marks each asset `stable` or `unstable` (`stable: false`):

- **stable** (hard-checked by `cargo xtask media --check`): static-state
  stills - terminal summaries (fast fixtures keep `Time taken: 0 seconds`)
  and GUI states without a run (empty/queue/options). These must regenerate
  byte-identically; a drift is a real regression.
- **unstable** (advisory): everything embedding wall-clock output - all
  demo videos (progress-bar redraw timing) and the run-state GUI stills
  (`in 0.4s` job durations). They drift by design; their freshness signal
  is the manifest's `generated_by` line and the auto-PR diff.

Supporting determinism: staged fixtures get a fixed mtime (files *and*
directories - the GUI's "modified" column shows them, and git checkout
times differ per machine), and each tape's hidden setup removes its own
outputs so the terminal pre-state is always identical.

## Publish and cache busting

`docs/media/manifest.json` tracks per asset `{sha256, version, stable}` plus a
`generated_by` provenance line (`xtask <git describe>; vhs <ver>; ffmpeg
<ver>`). On publish:

1. finals are copied into `docs/img/`,
2. every asset whose sha256 changed gets its `version` bumped,
3. every `docs/img/<asset>?v=N` reference in `README.md` and
   `gui/README.md` is rewritten to the new version - changed assets only, so
   unchanged URLs stay camo-cache-stable,
4. the link lint hard-fails on any `docs/img/` reference that does not
   resolve.

Note: crates.io does not render relative repo images (pre-existing behavior,
unchanged by this pipeline).

## CI

`.github/workflows/media.yaml` (dispatch-only; the D7 weekly schedule is a
commented template in the file):

| job | mode | behavior |
|-----|------|----------|
| `refresh` | `cargo xtask media` (+ inputs) | opens/upserts the `docs/media-refresh` PR (`docs: refresh README media`) when `docs/img`, the manifest or a README changed; uploads `docs/img/` + the run log as artifacts; with `dry_run` it uploads target/ previews instead and never opens a PR |
| `check` | `cargo xtask media --check` | advisory freshness table in the workflow summary; fails on stale assets, never writes the repo |

Inputs: `only` (comma-separated stages/scenes, empty = everything),
`dry_run` (boolean). Concurrency group `media-refresh` serializes runs.

    gh workflow run media.yaml                      # full refresh + PR
    gh workflow run media.yaml -f only=gui          # one stage
    gh workflow run media.yaml -f dry_run=true      # preview, artifacts only
    gh workflow run media.yaml --ref <branch>       # run against another branch

CI bootstraps the same pinned tools via `bootstrap-tools.sh` (no apt vhs/ttyd
drift) and installs `mesa-vulkan-drivers` for the lavapipe-backed headless GUI
capture.

## Troubleshooting

- **vhs exits 0 even when the recording failed** (missing `Require`
  program, chromium crash, timed-out Wait). Judge by artifacts, not the exit
  code - the xtask verifies frames + still + `.ascii` per tape and fails
  with the missing pieces named.
- **Frames vanish silently when running vhs by hand**: vhs applies
  `Output <dir>/` as an error-ignored `os.Rename` from its internal tmp dir.
  It fails without a peep when the parent directory is missing, when the
  target frames dir already exists (stale frames are kept, this run's are
  discarded), and on a cross-device rename (`TMPDIR` on another filesystem
  than the output - e.g. tmpfs `/tmp` vs ext4 `target/`). The xtask guards
  all three (pre-created `out/<scene>/`, wiped stale dirs, staged `TMPDIR`
  on the stage filesystem); manually, `rm -rf out/<scene>` first and keep
  `TMPDIR` on the output's filesystem.
- **Fonts**: manual vhs runs need the pinned JetBrains Mono installed once:
  `bash docs/media/tapes/setup-fonts.sh` (into `$FONT_HOME`, default
  `~/.fonts`). The pipeline does this automatically into the staged HOME; the
  GUI scenes use the committed `fonts/symbols/` via `--fonts-dir`. No system
  font packages are involved anywhere.
- **Browser sandbox**: vhs only passes `--no-sandbox` to Chromium when
  `VHS_NO_SANDBOX` is set; containers/CI without user namespaces need
  `export VHS_NO_SANDBOX=1` (the xtask sets it for its own vhs children;
  the bootstrap-provided `chrome` is found via `PATH`).
- **Headless GUI capture fails to create a device** on bare machines: the
  capture renders via wgpu + lavapipe (software Vulkan), so a Vulkan loader
  must exist: `sudo apt-get install mesa-vulkan-drivers`.
