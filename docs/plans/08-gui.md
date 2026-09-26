# WS8 — GUI (desktop front-end for byteshaver)

**Depends on:** WS7 (headless job API). Everything UI-specific lives in a **separate
workspace crate** so the CLI binary, musl builds, docker images, and release pipeline
stay untouched.
**Hard requirement:** drag-and-drop of files (and folders) onto the window adds them to
the encoding queue. Nothing is implemented in this plan; it is a decision + build spec.

---

## 1. Shared architectural foundation

The GUI is a **pure consumer of `byteshaver` core** (post-WS7):

```
┌───────────────────────────── byteshaver-gui (own crate) ─────────────────────────────┐
│  UI state (queue items, options, settings)                                     │
│      │ builds JobSpec (InputSelection::Files / ::Pattern)                       │
│      ▼                                                                          │
│  Session::detect() ── StopFlag ── Box<dyn Reporter = ChannelReporter>           │
│      │                                                                          │
│      ▼                                     ┌──────────────────────────────┐     │
│  JobHandle::start(spec, …)  ── worker ────▶│ std::sync::mpsc<JobEvent>    │     │
│                                            └─────────────┬────────────────┘     │
│  UI thread drains events each frame / on wakeup ◀─────────┘                     │
│  (repaint via request_repaint / event-loop wakeup)                              │
└─────────────────────────────────────────────────────────────────────────────────┘
```

Key properties:
- **One line of UI code touches no encoder.** Everything goes through
  `JobSpec`/`JobEvent`/`Outcome`/`capabilities()`. New encoders from WS1–WS6 appear in
  the GUI automatically (registry-driven option panels; see §5.4).
- Cancellation = `StopFlag::raise()` from the Cancel button — no signal handling.
- The queue is **serialized by default** (one job at a time, files within a job
  parallelize via rayon as today). A `Session` per window owns the thread budget, so
  the GUI and a concurrently running CLI don't oversubscribe the machine only if
  they run as separate processes (acceptable; document it).

### Workspace layout

```toml
# root Cargo.toml becomes a workspace
[workspace]
members = [".", "gui"]
# root stays: lib byteshaver (core) + bin byteshaver (CLI)
```

- `gui/` = `byteshaver-gui` crate, `default-run` untouched; released as separate artifacts
  (`byteshaver-gui` desktop binaries; **docker images remain CLI-only**).
- Feature unification: `byteshaver-gui` depends on `byteshaver` with the same default features;
  platform-stubbed features (WS1/WS2/WS6 on windows-gnu/musl) show up as disabled
  entries in `capabilities()` — the GUI grays them out instead of breaking.

---

## 2. Framework options

### Comparison criteria (project-specific)

Static-linkability & dep weight (project ships static musl + upx'd binaries),
single-language CI, drag-and-drop quality, native look, packaging burden, licensing
compatibility with MIT, long-term maintenance risk, Linux/docker-first audience.

| | A. egui/eframe | B. Tauri 2 | C. iced | D. GTK4/Relm4 | E. Local web UI (axum + browser) | F. Slint |
|---|---|---|---|---|---|---|
| Language/toolchain | pure Rust | Rust + HTML/JS (+ node optional) | pure Rust | Rust + C runtime (gtk) | Rust + any web stack | Rust + .slint DSL |
| Drag & drop files | ✅ first-class: `RawInput::dropped_files`/`hovered_files` (winit; X11/macOS/Windows solid, Wayland good on recent winit — verify) | ✅ good: native drag-drop events with absolute paths (`onDragDropEvent`); Linux WebKitGTK quirks need `dragDropEnabled` config care | ⚠️ possible via window `FileDropped` events/subscription; thinner docs | ✅ best-in-class `GtkDropTarget` + FileChooser | ⚠️ browser gives file *contents*, not paths (upload model); File System Access API is Chromium-only | ⚠️ manual via winit/raw-window; least documented |
| Binary/dep weight | medium (GPU renderer bundled; ~10–15 MB) | small (system webview) | medium | huge (GTK stack), non-static | tiny (server only) | medium |
| Static musl build | ✅ (GL via software/llvmpipe possible; on musl needs care) | ❌ relies on system webview | ✅ | ❌ | ✅ | ✅ (mostly) |
| Native look & feel | ❌ (custom immediate-mode look; themable) | ✅ (HTML/CSS as native-looking as you make it) | ❌ | ✅ (native on Linux; foreign elsewhere) | ✅/❌ browser chrome | ◑ native-ish themes |
| CI/packaging burden | low (`cargo build`; optional bundlers) | medium–high (front-end build, bundler per OS, webview version matrix) | low | high (MSYS2/macOS GTK bundling pain) | very low | low–medium |
| License | MIT OR Apache-2.0 | MIT OR Apache-2.0 | MIT | LGPL-2.1+ (dynamic linking OK) | MIT | ⚠️ GPL-3 / royalty-free desktop / commercial — complicates MIT project |
| Maintenance risk | low (very active, huge community) | medium (two ecosystems + webview churn) | medium (API churn between versions) | medium (fine on Linux, painful cross-platform) | low | medium (smaller community) |
| Extra capability | same code can compile to wasm (limited: native encoders won't run in-browser) | richest UI potential (charts, i18n, theming ecosystems) | Elm architecture suits queue state | heavyweight widgets/accessibility | works headless/remote; perfect for docker users port-forwarding | good embedded story (irrelevant here) |

### Recommendation

1. **Primary: A — egui/eframe.** Best fit for this project's ethos (pure Rust,
   single toolchain, static-linkable, MIT, first-class file drop). The queue UI is
   tables + buttons + progress — exactly egui's sweet spot; no web stack in CI.
2. **If product polish beats simplicity: B — Tauri 2.** Choose when a designer-driven,
   pixel-polished, brandable UI matters more than CI simplicity, and the team accepts
   JS/TS + per-OS bundling. Same core integration (§1) applies unchanged.
3. **Complementary, not exclusive: E — local web UI** as a *Phase 3* add-on reusing the
   same `JobSpec` API (`byteshaver serve`): gives docker users a UI over port-forwarding.
   Do not make it the desktop answer (upload model ≠ local file paths).
4. **Rejected for this project:** D (GTK) — static/packaging story incompatible with
   the project's release engineering outside Linux; F (Slint) — license friction;
   Qt/cxx-qt, Flutter — C++/Dart toolchains disproportionate to scope; thin GUI
   spawning the CLI binary and parsing stdout — explicitly impossible (that coupling
   is what WS7 removes).

**Decision gate:** pick A or B before starting; §4–§6 are framework-agnostic, §7
details the egui path (with Tauri deltas noted).

---

## 3. General design suggestions (three concepts, one recommended)

### Concept 1 — Queue-centric single window (recommended)

One window, drag-and-drop anywhere; everything else is secondary.

```
┌───────────────────────────────────────────────────────────────────────┐
│  byteshaver                                        [Settings] [About]  ─ □ ✕ │
├───────────────────────────────────────────────────────────────────────┤
│  ┌─────────────────────────────────────────────────────────────────┐  │
│ │   ⬇  drop files or folders here                                 │  │
│ │      …or click to browse  ·  14 files queued · 128 MiB          │  │
│ └─────────────────────────────────────────────────────────────────┘  │
│  ┌─────────────────────────────────────────────────────────────────┐  │
│  │ ✓ thumbnail │ name        │ in → out │ status │ size │ ✕       │  │
│  │ ░░          │ photo1.jpg  │ jpg→avif │ queued │ 4.2M │         │  │
│  │ ░░          │ photo2.heic │ heic→avif│ done ✓ │ 3.1M→290K      │  │
│  │ ░░          │ anim.gif    │ gif→webp │ ⟳ 12/24 frames        │  │
│  └─────────────────────────────────────────────────────────────────┘  │
│  Output: [ same as input ▼] [browse]  Format: [AVIF ▼] [Options…]    │
│  Global: EXIF [strip ▼]  Collision [overwrite-if-smaller ▼]           │
│  ───────────────────────────────────────────────────────────────────  │
│  [▶ Convert]  [⏸ Pause]  [✕ Cancel]        ▓▓▓▓▓▓░░░░ 7/14 · ETA 40s │
│  128 MiB → 31 MiB (24%) · 7 ok · 0 skipped · 0 errors    [Report ▼]  │
└───────────────────────────────────────────────────────────────────────┘
```

- The **entire window is a drop target** (requirement), with a persistent, always
  visible drop zone banner; dropped folders expand recursively (core does it);
  unsupported extensions are added grayed-out with a reason tooltip instead of
  silently ignored.
- Format picker is populated from `capabilities()`; per-format options panels are
  generated from the `*Options` structs (§5.4); encoders compiled out (HEIF/JXL stubs)
  are visible but disabled with the `disabled_reason` tooltip.
- Status column maps 1:1 to `Outcome` (queued/running/done/skipped/discarded/error
  with message). Footer numbers are `RunReport`/`ProgressStats` values — identical math
  to the CLI summary.
- "Report" exports the JSONL log (WS7 `JsonlReporter`).

### Concept 2 — Three-step wizard (Files → Settings → Run/Results)

Simpler mental model, good for one-off conversions; weaker for iterative batch work
(changing one option re-walks the wizard). Suitable as a **first-run overlay** on top
of Concept 1 ("quick mode"), not as the main layout.

### Concept 3 — Dual-pane explorer + hot-folder dashboard

Left: source tree with pattern entry (CLI parity). Right: output preview + sizes.
Plus a "watch folder" mode (drop a folder, get a processing daemon). Power-user value,
but significantly more UI state; hot-folder watching is a *core* feature (fs watcher)
mispalced in the GUI — defer to a future core feature (`byteshaver watch`) that any UI could
use.

**Chosen direction:** Concept 1, adopting Concept 2's quick-mode overlay later and
Concept 3's watch-folder only after it exists in core.

---

## 4. UX specification (Concept 1, MVP scope)

1. **Adding files (the hard requirement)**
   - Drop anywhere → `InputSelection::Files` (directories preserved as-is; core
     expands). Also: click-to-browse (native file dialog: `rfd` crate under egui),
     Ctrl+O, "Add folder…" (recursive), paste (Ctrl+V of copied files).
   - Deduplicate against existing queue items (same canonical path).
   - Each queue item shows source format detection immediately (cheap header sniff,
     `ImageFormat::from`) — no decoding at add-time.
   - Optional "pattern" text field (collapsed under "advanced") for CLI parity.
2. **Configuration**
   - Global target format + options panel; per-item override via row context menu
     (v2).
   - Output directory: `same as input` (default) or browsed path; mirrors
     `ConversionConfig.output`.
   - Global policies as dropdowns mapping directly onto core enums: EXIF policy
     (WS4), collision/overwrite policy (WS0 §2.7), `--animated-input` (WS5),
     `--heif-image-policy` (WS1). Help icons quote the CLI flag names — CLI↔GUI
     terminology stays aligned.
3. **Running**
   - Start disabled when queue empty or no capable encoder selected.
   - Cancel = `StopFlag::raise()` (drains current file, marks rest cancelled — same
     semantics as CLI Ctrl+C). Pause = v2 (requires core support: stop-flag variant
     that pauses queue intake — small WS7 extension, listed there as follow-up).
   - Live per-file progress where available (frames for animations later; per-file is
     binary in MVP: running/done).
4. **Results**
   - Footer aggregates from `ProgressStats`; final `RunReport` opens a summary panel
     (sizes saved, failures with `Outcome::Error` strings, "open folder" buttons,
     export report). Failed rows keep their error text inline.
5. **Settings persistence**
   - `byteshaver-gui.json` in the OS config dir (`dirs`/`confy`): last output dir, chosen
     encoder + options, policies, window size. `EncoderConfig` serde (WS7 C6) makes
     this trivial. No registry/hidden dotfile hacks.
6. **Platform behaviors**
   - File associations & "open with byteshaver" installer tweaks: v2 (needs per-OS packaging
     work, list in §8).
   - Dark/light theme follows system where the framework allows; dark default.
   - Accessibility: keyboard-navigable queue, screen-reader labels (egui: weaker here —
     listed honestly as drawback; Tauri: inherits web a11y).

---

## 5. Implementation architecture (egui path)

### 5.1 Crate & deps

```
gui/
  Cargo.toml      # byteshaver-gui; deps: eframe, egui, rfd, byteshaver (path dep), serde, dirs
  src/main.rs     # eframe entry
  src/app.rs      # App state (queue, settings, job handle)
  src/queue.rs    # QueueItem model, dedup, expansion
  src/panels/     # drop_zone, file_table, options, footer, report, settings, about
  src/reporter.rs # ChannelReporter: forwards JobEvent into mpsc for the UI thread
```

Version pins left to implementation time (eframe current stable); `rfd` for native
dialogs; no async runtime.

### 5.2 Threading & event flow

- UI thread: egui loop at ~30 fps idle, `request_repaint_after(100ms)` while a job
  runs (or event-loop wakeup via `egui::Context::send_viewport_cmd`/waker).
- Worker: `JobHandle::start` (core already spawns threads); `ChannelReporter` pushes
  `JobEvent`s; UI drains with `try_recv` each frame → update `QueueItem` states.
- `Outcome` → row status mapping is a pure function (unit-tested).

### 5.3 Drag-and-drop specifics (egui)

- Each frame: `ctx.input(|i| i.raw.dropped_files.clone())`; non-empty → enqueue
  `DroppedFile.path`s (filter `None`-path entries, e.g. in-memory drags).
- `hovered_files` → highlight drop zone + window border.
- Extensions filtered with `ImageFormat::from_extension` (accept = supported input or
  `Unknown`→grayed with reason).
- Platform verification matrix added to the GUI test checklist: Windows 10/11,
  macOS, X11, Wayland (winit version note), 100+ files at once (UI virtualization:
  egui table with `ScrollArea` + row virtualization; 10k rows must stay smooth —
  test).

### 5.4 Option panels from core types

To avoid hand-writing a panel per encoder: a small `OptionsView` helper renders fields
via a `trait OptionsSchema` added in the **gui crate only** (implemented for each
`*Options` struct via a macro in gui, not in core — keeps core UI-agnostic). Fields,
ranges, and defaults come from the structs; enums render as dropdowns via
`strum`-style name tables. When WS2/WS3/WS5 land new options structs, the GUI adds one
impl block (single, predictable touch-point).

### 5.5 Packaging & release

> **IMPLEMENTED (supersedes the original text below):** the release pipeline builds
> GUI + CLI binaries for **linux-musl (static-pie) and windows-gnu** per CPU target.
> Validated locally: static musl GUI via alpine container with
> `--no-default-features --features x11` (x11-only windowing; the wayland feature is
> not target-gated and stays off); windows GUI with `--no-default-features` (winit
> auto-selects its windows backend). The musl builds run natively in an alpine
> container because vendored libjxl needs a musl C++ toolchain that
> ubuntu's `musl-tools` cross setup cannot provide. See `gui/README.md` and the
> `build_binaries`/`validate_docker` jobs in `.github/workflows/workflow.yaml`.

- Windows: `cargo build --release` + icon via `winresource` (already on the roadmap)
  → zip or NSIS/MSI via `cargo-wix` (v2).
- macOS: `.app` bundle (manual bundle script or `cargo-bundle`); unsigned initially,
  notarization as v2 task.
- Linux: AppImage (linuxdeploy recipe) + tarball; .deb v2.
- Release pipeline (workflow.yaml): new `build_gui` job matrix (same cpu targets as
  CLI, **no musl for GUI v1** — GL/software-rendering on musl is possible but
  untested; start with gnu Windows + gnu Linux + macOS). GUI artifacts attached to the
  same GitHub release; docker untouched.
- Binary size: expect +10–15 MB before upx; upx applies fine.

### 5.6 Tauri-delta (if option B chosen)

Same §1 integration; replaces §5.1/§5.5: `gui/` becomes `src-tauri/` + webview
front-end (`queue.ts` state mirroring §4); events cross IPC (`JobEvent` → JSON);
drag-drop via Tauri's native drop events (paths, not uploads); packaging via Tauri
bundler; CI adds node-free static frontend build. §4 UX spec unchanged.

---

## 6. Testing

- **Unit (gui crate):** queue dedup/expansion, Outcome→status mapping, options-schema
  rendering snapshot (values only), settings serde round-trip.
- **Integration (headless):** run the App with a test harness driving the queue model
  directly (bypass rendering): add 50 files → run → assert `RunReport` equals CLI
  result on the same inputs (golden parity test — GUI must produce identical
  conversions).
- **Manual checklist per OS:** drop of files/folders/mixed/unsupported/1000 files,
  drop during a running job, cancel mid-job, output-dir-permission error surfaces as
  dialog (no crash — regression test for C3), HiDPI scaling, long filenames truncation.
- **CI:** `cargo clippy`/`test` for gui crate; a `xvfb-run` smoke test on Linux that
  launches the app for 3s and exits 0 (catches GPU/font init issues).

---

## 7. Roadmap

| Phase | Content |
|-------|---------|
| GUI-MVP | Concept-1 layout, drag&drop + browse add, single global encoder + options panel, run/cancel, footer stats, report panel, settings persistence, per-OS packaging scripts |
| GUI v1.1 | per-item overrides, quick-mode wizard overlay, `--json-log` viewer, pause |
| GUI v2 | file associations/installers, thumbnail preview generation (reuse core decode), watch-folder (after core `byteshaver watch`), i18n |
| Complement | `byteshaver serve` web UI (option E) reusing JobSpec API for docker/remote users |

## 8. Risks

- **egui on Wayland/HiDPI quirks** → verification matrix in §6; fallback X11/wayland
  guidance in README.
- **GUI doubles release engineering surface** → GUI v1 ships gnu targets only; musl/
  docker untouched; CLI release cadence unaffected.
- **Scope creep toward file manager** → concepts 2/3 explicitly deferred; MVP scope is
  the table above.
- **Framework lock-in** → all product logic lives in core + thin `app.rs` state; the
  Tauri delta (§5.6) documents that switching frameworks is a front-end rewrite only,
  never a core change.
