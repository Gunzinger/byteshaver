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

## CI additions (not yet wired)

The release pipeline (`.github/workflows`) is intentionally untouched by
WS8. When GUI artifacts should be built on CI, add a job along these lines
(gnu targets only for v1; musl/docker stay CLI-only per plan WS8 §5.5):

```yaml
build-gui:
  strategy:
    matrix:
      include:
        - { os: ubuntu-24.04,  target: x86_64-unknown-linux-gnu }
        - { os: windows-latest, target: x86_64-pc-windows-msvc }
        - { os: macos-14,      target: aarch64-apple-darwin }
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    # no apt packages required to *compile* (x11/wayland/GL are dlopened at
    # runtime; libxkbcommon is not needed for the build)
    - run: cargo build --release -p byteshaver-gui
    # optional smoke test on linux:
    # - run: xvfb-run -a timeout 3 target/release/byteshaver-gui; test $? -eq 124
    - uses: actions/upload-artifact@v4
      with:
        name: byteshaver-gui-${{ matrix.target }}
        path: target/release/byteshaver-gui*
```

Also worth adding once the job exists: `cargo clippy -p byteshaver-gui
--all-targets -- -D warnings` and `cargo test -p byteshaver-gui` (currently
covered by the workspace-wide runs).

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

## Deferred (GUI v1.1+/v2, see plan WS8 §7)

packaging scripts (AppImage/.app/NSIS, icons), per-item encoder overrides,
quick-mode wizard overlay, pause, thumbnails, i18n, file associations,
`byteshaver watch` hot folders, a Linux `xvfb-run` smoke test in CI.
