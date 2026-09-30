# 09 — Application icon & release build metadata (version, commit hash)

**Status:** plan (not implemented). §1 records a shipped bug fix.
**Depends on:** nothing (independent of WS0–WS8); touches `build.rs`, `gui/`,
`.github/workflows/workflow.yaml`, `Dockerfile`.

---

## 1. Problem: released GUI binaries report `0.0.0-git`  *(fixed)*

**Root cause.** The release pipeline patches the placeholder version with sed —
but only in the **root** manifest:

```yaml
# .github/workflows/workflow.yaml (before the fix)
sed -i "s/0\\.0\\.0-git/${VERSION}/" Cargo.toml   # root crate only!
sed -i "s/0\\.0\\.0-git/${VERSION}/" Cargo.lock
```

`gui/Cargo.toml` keeps `version = "0.0.0-git"`. The GUI derives **everything it
displays** from its own crate version:

- `gui/src/app.rs:49` — `CORE_VERSION = env!("CARGO_PKG_VERSION")` (the *gui*
  crate's version, under the lockstep assumption),
- rendered in the header (`gui/src/panels/mod.rs:44`, `v0.0.0-git`) and the
  About window (`gui/src/panels/about.rs:18`, `core version 0.0.0-git`).

So on every tag build the Windows/musl GUI binaries — including the line
labelled *core version* — showed the placeholder. (The CLI's `--version` was
correct on tag builds, because the root manifest *was* patched.)

**Fix applied in this workflow step** (anchored so only package `version`
lines match; `Cargo.lock` has exactly one matching line per workspace crate):

```yaml
sed -i "s/^version = \"0\\.0\\.0-git\"/version = \"${VERSION}\"/" Cargo.toml gui/Cargo.toml Cargo.lock
grep -n '^version' Cargo.toml gui/Cargo.toml
grep -n -A1 '^name = "byteshaver' Cargo.lock
```

Verified locally against a copy of both manifests and the lock file (all four
spots patched; YAML parses). The old `head -n 5` check never even displayed the
version line (line 7) — the new greps show exactly the patched lines.

**Residual design smell (addressed in §3.4):** the About panel's *core version*
is the GUI crate's version, correct only while the lockstep sed succeeds for
both manifests. The core crate should expose its own version instead.

---

## 2. Goals / non-goals

**Goals**
1. Released binaries carry verifiable build metadata: crate version, **git
   commit**, build date, build profile.
2. The metadata is visible where users look: `byteshaver --version`, GUI About
   panel, Windows Explorer file properties.
3. The GUI gets an application icon: window/taskbar icon at runtime and a
   real `.exe` icon (plus `VERSIONINFO`) on Windows.
4. Zero-risk to the static musl CLI builds and the release pipeline's caching.

**Non-goals**
- Installer packages (.msi/.deb/AppImage) — noted in §7 as future work.
- macOS builds/bundles (no macOS target exists today; §6 records what an
  `.icns` would need if that ever changes).
- Reproducible-builds certification (but §3.3 keeps the door open via
  `SOURCE_DATE_EPOCH`).

---

## 3. Part A — build metadata (version, commit, date, profile)

### 3.1 Mechanism choice

| Option | Verdict |
|---|---|
| **Extend the existing `build.rs`** (env vars set by CI, `git` fallback locally, generated `build_info.rs`) | **recommended.** No new dependencies; values are injected by the environment so it works in the alpine container, the mingw cross build, `cargo publish`, and distro repackaging alike; graceful `unknown` fallbacks for `crates.io` tarballs / docs.rs. |
| `vergen` (+ `vergen-gitcl`) | Works, but adds build-dependencies and wants a `.git` dir at build time — absent for published-crate builds and hostile in shallow/worktree checkouts. Still needs the same env plumbing for the container anyway. |
| `shadow-rs` | Richest output, but drags in `clap` + more deps for what is ~15 lines of code here; same git-at-build-time caveats. |

**Precedent in this repo:** `build.rs` already generates `versions.rs`
(dependency table consumed by `src/converter/mod.rs:33`). Adding a sibling
`build_info.rs` is the smallest change consistent with the existing pattern.

### 3.2 Build-script design

In `build.rs` (root crate only — the GUI reads the values through the core
crate, see §3.4):

```rust
use std::process::Command;

fn git_fallback() -> Option<String> {
    // only meaningful for local builds; CI always injects the env var
    Command::new("git").args(["rev-parse", "--short=9", "HEAD"]).output().ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

// in main():
let commit = std::env::var("BYTESAVER_BUILD_COMMIT").ok()
    .filter(|s| !s.is_empty())
    .or_else(git_fallback)
    .unwrap_or_else(|| "unknown".into());
let date = std::env::var("BYTESAVER_BUILD_DATE").ok()
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| "unknown".into());
// write build_info.rs: BUILD_COMMIT, BUILD_DATE, BUILD_PROFILE
println!("cargo:rerun-if-env-changed=BYTESAVER_BUILD_COMMIT");
println!("cargo:rerun-if-env-changed=BYTESAVER_BUILD_DATE");
```

Details that matter:

- **`cargo:rerun-if-env-changed` is what makes the cache story safe**: the
  alpine build-tree cache (`actions/cache`, keyed on `Cargo.lock` only) stays
  valid across releases; cargo re-runs `build.rs` and recompiles just the two
  workspace crates because the env changed. Without this, a restored tree
  would silently keep the previous release's commit hash.
- Commit values: full `${{ github.sha }}` (40 hex) for auditability; display
  short form (first 9 chars) in UIs.
- Generate a ready-made display string too, so the CLI doesn't need runtime
  formatting:

  ```rust
  pub const LONG_VERSION: &str = "0.5.3 (commit a1b2c3d4e, built 2026-09-27, profile release)";
  ```
  `CARGO_PKG_VERSION` is available to build scripts, so this can be composed
  entirely in `build.rs`.

### 3.3 CI wiring (all four build environments)

| Environment | Change |
|---|---|
| **workflow-level env** (windows-gnu host build, `cargo publish`) | add to the top-level `env:` block: `BYTESAVER_BUILD_COMMIT: ${{ github.sha }}` and `BYTESAVER_BUILD_DATE: ${{ github.event.head_commit.timestamp }}` |
| **alpine container** (`docker run`, workflow.yaml §build linux) | the `docker run` line must forward them explicitly: `-e BYTESAVER_BUILD_COMMIT -e BYTESAVER_BUILD_DATE` (docker does not inherit env into containers) |
| **docker images** (`Dockerfile`) | add `ARG BYTESAVER_BUILD_COMMIT=unknown` + `ENV BYTESAVER_BUILD_COMMIT=$BYTESAVER_BUILD_COMMIT` (same for date) before the build stage, and pass `build-args:` in `docker/build-push-action`. `validate_docker` builds without the args → `unknown`, which is exactly what the smoke tests should assert-agnostic. |
| **workflow_dispatch fallback** | keep `VERSION=0.0.0-dispatch`; commit sha is still injected → artifacts remain attributable |

Reproducibility note: embedding a wall-clock date breaks byte-identical
rebuilds. The design above only embeds what CI provides; `unknown` defaults
mean local/offline builds stay deterministic, and a future strict mode can map
`SOURCE_DATE_EPOCH` → `BYTESAVER_BUILD_DATE`.

### 3.4 Display surfaces

1. **Core crate exports** (`src/lib.rs`, generated by `build.rs`):

   ```rust
   include!(concat!(env!("OUT_DIR"), "/build_info.rs")); // BUILD_COMMIT, BUILD_DATE, BUILD_PROFILE, LONG_VERSION
   pub const VERSION: &str = env!("CARGO_PKG_VERSION");  // authoritative core version
   ```

2. **CLI** (`src/cli.rs`): keep `-V` short & script-stable, make `--version`
   verbose — clap 4 shows `version` for `-V` and `long_version` for `--version`:

   ```rust
   #[command(version, long_version = byteshaver::LONG_VERSION, about, long_about = None)]
   ```

3. **GUI About** (`gui/src/panels/about.rs`) — replace the lockstep assumption
   with the real core version, and show metadata:

   ```rust
   format!("gui {} · core {} · commit {} (built {}, {})",
       env!("CARGO_PKG_VERSION"), byteshaver::VERSION,
       short_commit, byteshaver::BUILD_DATE, byteshaver::BUILD_PROFILE)
   ```

   The header (`gui/src/panels/mod.rs:44`) keeps the short `v{version}`.

4. **Optional:** one `build` event line in the `--json-log` stream carrying the
   same fields (helps bug reports); out of scope for v1.

---

## 4. Part B — Windows `.exe` icon + `VERSIONINFO` resource

### 4.1 Approach

Use **`winresource`** (maintained fork of `winres`) as a `[build-dependencies]`
entry in the root crate — it compiles a `.res` into the PE during the normal
cargo build and needs only `windres` from binutils, no SDK:

```toml
[build-dependencies]
winresource = "0.4"   # windows-resource embedding; used only for windows targets
```

Why not `embed-resource`/`winres`: `embed-resource` shells out to `rc.exe`
(Windows SDK) or `windres` but is more moving parts; `winres` is unmaintained.
`winresource` is the smallest maintained path and is what winres forks into.

### 4.2 Build-script guard (critical for the other targets)

The same `build.rs` serves musl, gnu-linux, and windows targets. The resource
step must be strictly windows-target-gated and **non-fatal**:

```rust
if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
    if let Err(err) = windows_resources() {
        println!("cargo:warning=skipping windows resources: {err}");
    }
}
```

```rust
fn windows_resources() -> std::io::Result<()> {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/windows/byteshaver.ico");
    res.set("ProductName", "byteshaver");
    res.set("FileDescription", "byteshaver — batch image converter");
    res.set("FileVersion", env!("CARGO_PKG_VERSION")); // build script sees CARGO_PKG_* env
    res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    res.set("LegalCopyright", "MIT License");
    res.set("OriginalFilename", "byteshaver.exe");
    res.set("Comments", &format!("commit {}, built {}", commit, date)); // from §3.2
    res.compile()
}
```

- Cross-compiling from ubuntu, `winresource` invokes `x86_64-w64-mingw32-windres`.
  That binary comes from `binutils-mingw-w64-x86-64`, which the CI apt line
  already pulls in transitively via `gcc-mingw-w64-x86-64-win32` — add it
  **explicitly** to the `apt-get install` list so it can't silently disappear.
- Guarding on `CARGO_CFG_TARGET_OS` (not `cfg!(windows)`) is what keeps the
  alpine/musl and docker builds untouched — no windres needed there, static-pie
  layout unaffected.
- `FileVersion`/`ProductVersion` must be numeric-compatible strings ("0.5.3");
  put the commit hash in `Comments` where Explorer shows free text.
- The GUI binary already sets `windows_subsystem = "windows"` in release
  (`gui/src/main.rs:13`) — the resource embeds fine alongside it.

### 4.3 GUI build note

`byteshaver-gui` has **no build script**. Two options:

- **(a) recommended:** let the root crate's mechanism serve the GUI by adding a
  tiny `gui/build.rs` calling the same helper (duplicated ~20 lines, or a
  shared `xtask`-style module later). The GUI's `.ico` gets
  `OriginalFilename = byteshaver-gui.exe`, `FileDescription` = GUI description.
- (b) skip VERSIONINFO for the GUI in v1 and only ship the runtime icon (§5);
  Explorer then shows a default icon — not acceptable for a desktop app.

### 4.4 UPX interplay

Both windows binaries are UPX-packed in CI (`upx --best`). UPX preserves the
PE resource section — icon and `VERSIONINFO` survive packing. Add a validation
step after packing:

```bash
file target/x86_64-pc-windows-gnu/release/byteshaver-gui.exe  # still PE32+
# explorer-properties equivalent on linux:
strings target/x86_64-pc-windows-gnu/release/byteshaver-gui.exe | grep -m1 ProductName
```

---

## 5. Part C — GUI runtime icon (window / taskbar)

### 5.1 What eframe 0.32 offers

`egui::ViewportBuilder::with_icon(egui::IconData { width, height, rgba })` and
`.with_taskbar_icon(...)`, set in `gui/src/main.rs:34` where the viewport is
already configured (`.with_title`, `.with_inner_size`).

```rust
let icon = load_icon(include_bytes!("../assets/byteshaver-256.png"));
viewport: egui::ViewportBuilder::default()
    .with_title("byteshaver")
    .with_icon(icon.clone())
    .with_taskbar_icon(icon)
    ...
```

Decoding: add `image` to `gui` with `default-features = false, features =
["png"]` — the exact same crate/version is already in the workspace graph via
the core, so the lock file doesn't grow; only the GUI's compile unit includes
png decoding (once, at startup, on a 256px file — negligible).

### 5.2 Platform matrix

| Platform | Window icon | Taskbar icon | Notes |
|---|---|---|---|
| Windows | `with_icon` (winit) — plus the embedded `.res` icon (§4) as fallback | `with_taskbar_icon`; exe resource also feeds Explorer | fully covered |
| X11 | `with_icon` (WM `_NET_WM_ICON`) | from window icon on most WMs | covered by the same code path |
| Wayland | **not supported by the protocol** — winit silently ignores it; compositors derive taskbar icons from the `.desktop` file's `Icon=` | desktop entry + hicolor icon install | needs packaging (§7); document a manual install snippet for the static binary until then |

So: §5.1 is a strict improvement everywhere that matters today; the Wayland
taskbar gap is a packaging concern, not a code concern.

### 5.3 Asset pipeline (source of truth → deliverables)

Chosen SVG (§8) is the single source of truth. Derive and **commit**:

```
gui/assets/byteshaver-256.png      # runtime icon (RGBA, 256×256)
assets/windows/byteshaver.ico      # 16/24/32/48/64/128/256 bundled
docs/img/icon.svg                  # master SVG (also used by README/docs)
```

Generation (any one of; document the chosen command in the asset dir):

```bash
rsvg-convert -w 256 -h 256 icon.svg -o byteshaver-256.png   # or inkscape / magick
icotool -c -o byteshaver.ico t16.png t24.png t32.png t48.png t64.png t128.png t256.png  # icoutils
```

No build-time SVG tooling in CI — assets are committed, keeping the release
pipeline dependency-free.

---

## 6. macOS note (for the record)

No macOS target is built today. If one appears: `.icns` via `iconutil`, bundle
identifier + `LSApplicationCategory` in an `Info.plist`, code-signing before
notarization — all packaging-layer work, none of it affects §3–§5.

---

## 7. Linux packaging metadata (follow-up, not v1)

For a polished Linux story (esp. Wayland taskbar icon, §5.2):

- `.desktop` file (`Icon=byteshaver`, `Exec=byteshaver-gui`) + 512px PNG
  installed to `/usr/share/icons/hicolor/512x512/apps/` — via a future `.deb`,
  PKGBUILD, or AppImage (AppImage additionally wants a
  `DirIcon`/`appimaged`-friendly layout).
- The docker images already get OCI labels (`org.opencontainers.image.*`) via
  `docker/metadata-action`; the commit ARG from §3.3 can additionally become an
  image label (`org.opencontainers.image.revision=${{ github.sha }}`) — one
  line in `metadata-action`'s `labels:` input.

---

## 8. Icon decision

Six proposals live in `docs/img/icon-proposals/` (open `index.html` for a
contact sheet with dark/light and 256/64/32/16px renderings). All proposals
are **pixel art**: authored on a 16×16 grid (32px cells, flat palette, hard
edges, `shape-rendering="crispEdges"` — no gradients, no anti-aliased
curves), so every size from the 512px tile down to a 16px favicon scales
crisply. The grids live in `docs/img/icon-proposals/generate.py`, which
regenerates all six SVGs (+ a QA contact PNG) after edits.

| # | File | Concept | Small-size legibility | Verdict |
|---|---|---|---|---|
| 1 | `01-razor-slice.svg` | pixel photo card sliced by a steel razor diagonal, trimmed pixels drifting, amber spark | good — pixel grid stays crisp at 16px | **recommended primary**: most distinctive, literal "shaving bytes" story |
| 2 | `02-nested-shrink.svg` | dashed + solid pixel frames collapsing into a teal core, amber chevron | good | good abstract-logo alternative; least domain-specific |
| 3 | `03-pixel-shave.svg` | pixel block grid with a diagonal bite shaved out, steel staircase blade | **best geometric** | **runner-up / small-size variant source** |
| 4 | `04-squeeze.svg` | chunky teal arrows pressing a tiny pixel photo | good | friendliest; slightly generic |
| 5 | `05-blade-badge.svg` | pixel safety-razor blade badge (center slot, side notches) | excellent silhouette | best pure favicon/taskbar shape; weak "compress" story |
| 6 | `06-before-after.svg` | big washed-out frame → amber arrow → small vivid pixel tile | medium | best for README header marketing, not an app icon |

**Recommendation:** ship concept **01** (razor slice). The pixel-art grid
removes the old large-vs-small artwork split — the same 16×16 grid stays
crisp from 512px down to 16px, so no separate small-size variant is needed.
Decide by the classic tests: squint at 32px, render the silhouette in solid
black (must stay recognizable), check dark and light desktop backgrounds
(`index.html` shows both).

---

## 9. Rollout checklist

1. Pick the icon (§8); generate + commit the three assets (§5.3).
2. `build.rs`: add build-info generation + `rerun-if-env-changed` (§3.2);
   export from `src/lib.rs` (§3.4.1); regenerate `build_info.rs` alongside the
   existing `versions.rs`.
3. CLI: `long_version` in `src/cli.rs` (§3.4.2) — keep `-V` short.
4. GUI: About panel metadata line + header unchanged (§3.4.3);
   `with_icon`/`with_taskbar_icon` + `image` png dep (§5.1).
5. Windows resources: `winresource` build-dep, guarded compile for both crates
   (§4.2/§4.3); add `binutils-mingw-w64-x86-64` to the CI apt line.
6. CI: workflow-level env, `docker run -e` forwarding, `Dockerfile`
   ARG/ENV + `build-args:` (§3.3); optional `org.opencontainers.image.revision`
   label (§7).
7. **Verification**
   - local: `cargo run -p byteshaver -q -- --version` (long line), `-V` (short);
     `cargo clippy -p byteshaver -p byteshaver-gui` clean.
   - simulate CI locally: `BYTESAVER_BUILD_COMMIT=$(git rev-parse HEAD)
     cargo build ...` then `grep -a "$(git rev-parse --short=9 HEAD)" bin`.
   - CI `workflow_dispatch` run: check the windows exe's Explorer properties
     (ProductName/FileVersion/Comments), `byteshaver -V` inside both docker
     images, and confirm the linux GUI About panel shows commit + date.
   - negative check: no release artifact contains the placeholder —
     `grep -a '0\.0\.0-git' <binary>` must find nothing.
