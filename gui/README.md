# byteshaver-gui

Desktop GUI for [byteshaver](../README.md) (plan WS8): a queue-centric
single window (egui/eframe) where the entire window is a drag-and-drop
target for files *and* folders.

## Build & run

```bash
cargo build --release -p byteshaver-gui
target/release/byteshaver-gui
```

The crate is a workspace member (`gui/`) and consumes the core via a path
dependency on `byteshaver` — the exact same library the CLI binary uses, so
GUI conversions are byte-identical to CLI runs of the same settings.

## Runtime requirements

| Platform | Requirements |
|----------|--------------|
| Linux (X11) | GL (libGL/libEGL); `xdg-desktop-portal` for native file dialogs |
| Linux (Wayland) | same as X11; drag-and-drop quality depends on the compositor/winit pairing (see plan WS8 §8 risk matrix) |
| Windows | nothing beyond the standard runtime (Common-Controls v6 via `rfd`) |
| macOS | nothing beyond the standard runtime |

Compile-time requirements are the same as the core crate (cmake + C++
compiler + nasm for the vendored libjxl when the default `jxl` feature is
active). **No GTK development packages** are needed: `rfd` uses the
`xdg-portal` backend by default on Linux.

## Feature contract

`byteshaver-gui` depends on `byteshaver` **with its default features**
(`opt-oxipng`, `exif`, `jxl`, `anim-webp`, `anim-apng`, `logs`). The options
editors and the EXIF policy injection reference those types/fields
unconditionally (Rust `cfg(feature = …)` in the gui crate cannot observe a
dependency's features). Building the gui against a feature-reduced core is
therefore not supported; use `capabilities()`-driven grayed-out entries
instead (they already cover feature-stubbed *platforms* at runtime).

## Architecture (plan WS8)

```
src/
  main.rs        eframe entry, window title/size (persisted)
  app.rs         App state: queue, settings, running job, event application,
                 JobSpec building (pure, unit-tested incl. CLI parity)
  queue.rs       QueueItem/Queue: dedup (canonical paths), Outcome→status map
  reporter.rs    ChannelReporter: JobEvent → std mpsc for the UI thread
  options.rs     per-encoder options editors + capability-driven enablement
  settings.rs    settings.json persistence (corrupt → defaults, no panic)
  panels/        drop_zone, file_table, footer, options_panel, report, about
```

Threading: the UI thread owns all state; `JobHandle::start` runs the core's
worker (rayon-parallel files); `JobEvent`s cross to the UI via
`std::sync::mpsc` and are drained with `try_recv` each frame, with
`request_repaint_after(100ms)` while a job runs. Cancelation is
`StopFlag::raise()` (same semantics as CLI Ctrl+C).

All product logic lives outside the rendering closures so the headless test
suite (`cargo test -p byteshaver-gui`) can cover it — the egui rendering
itself cannot run without a display server.

## Release packaging (implemented in CI)

The release pipeline builds **separate CLI and GUI binaries for Linux (musl,
static-pie) and Windows (gnu)** for every CPU target (x86-64-v3/v4, znver3,
znver5). Build configurations per platform:

| Artifact | Build | Notes |
|----------|-------|-------|
| `byteshaver-<ver>[-cpu]` (Linux) | alpine container, `cargo build --release -p byteshaver` | native musl toolchain (gcc/g++/make/cmake) so the vendored libjxl builds; validated static-pie |
| `byteshaver-gui-<ver>[-cpu]` (Linux) | alpine container, `-p byteshaver-gui --no-default-features --features x11` | x11-only windowing (no wayland system libraries); validated static-pie |
| `byteshaver-<ver>[-cpu].exe` (Windows) | ubuntu host, `--target x86_64-pc-windows-gnu -p byteshaver` | full features; the libjxl cmake cross needs the target-suffixed `CC/CXX/AR` env vars set by the workflow |
| `byteshaver-gui-<ver>[-cpu].exe` (Windows) | ubuntu host, `--no-default-features` | winit auto-selects its windows backend; eframe's wayland/x11 features are not target-gated and must stay off |

All artifacts are UPX-packed and shipped with a `.sha256` checksum to the
GitHub release. The docker images are the only artifacts that carry `dec-heif`
(the `libheif`/codec libraries have no static archives); a dedicated
`validate_docker` CI job builds both images and proves the feature with an
end-to-end decode of a generated real HEIC before publication.

## Deferred (GUI v1.1+/v2, see plan WS8 §7)

packaging scripts (AppImage/.app/NSIS, icons), per-item encoder overrides,
quick-mode wizard overlay, pause, i18n, file associations,
`byteshaver watch` hot folders, a Linux `xvfb-run` smoke test in CI.
(Implemented since: thumbnails, sortable/configurable file table, presets,
quality metrics + difference inspector, segmented progress bar + confetti,
independent report viewport — see `docs/plans/09-*.md` through `14-*.md`.)
