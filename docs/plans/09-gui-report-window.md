# 09 — GUI: run-report window fixes (independent OS window, working resize)

**Size:** S · **Depends on:** nothing · **Parallelizable with:** all other GUI plans
(owns `gui/src/panels/report.rs` exclusively; one tiny shared touch in
`gui/src/app.rs` for window-geometry persistence).

## Problem (user report)

The run-report window is buggy to interact with:

1. it appears vertically always bigger than intended,
2. it is trapped inside the main application window and cannot be dragged
   outside it,
3. attempting to vertically resize it makes it snap back to full expansion.

## Root-cause analysis (current code)

`gui/src/panels/report.rs:15-19` renders the report as an `egui::Window`
inside the **root viewport**:

- **Trapped:** an `egui::Window` is an in-viewport container clipped to the
  main window's rect. It can never leave the OS window — that is a
  framework property, not a sizing bug. Fixing "drag it outside" requires a
  **separate native viewport**.
- **Snap-back expansion:** the per-file list is
  `egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(...)`
  (`report.rs:43-45`) inside a window that otherwise auto-sizes to its
  content. `auto_shrink[1] = false` makes the scroll area request the
  **full content height** (thousands of rows × 20 px), so the window's
  minimum size equals the whole list: any drag-resize smaller than that is
  immediately re-expanded. The notices block at the bottom
  (`max_height(120)`) is already bounded correctly — the file table is not.

## Design

Two independent fixes; both ship together (fix 1 alone also benefits from
fix 2, and fix 2 alone is the graceful-degradation path if multi-viewport
proves broken on a target platform).

### Fix 1 — render the report in its own OS viewport

egui 0.32 (the pinned line, see `gui/Cargo.toml:30-44`) supports multiple
native viewports via `Context::show_viewport_immediate` (verified in
`egui-0.32.3/src/context.rs:3920`: callback signature
`FnMut(&Context, ViewportClass)`; **when the backend cannot do multiple
viewports, egui calls the callback with `ViewportClass::Embedded`** —
that is the runtime fallback signal, no manual constant needed). Spawn a
real OS window:

```rust
// in panels::show, replacing report::show_window(app, ctx)
if app.show_report && app.report.is_some() {
    ctx.show_viewport_immediate(
        egui::ViewportId::new("run-report"),
        egui::ViewportBuilder::default()
            .with_title("byteshaver — run report")
            .with_inner_size([760.0, 480.0]),
        |vctx, class| {
            if class == egui::ViewportClass::Embedded {
                // backend without multi-viewport support: graceful
                // degradation — a plain (bounded!) egui::Window
                report::show_embedded_window(app, vctx);
            } else {
                report::show_viewport_contents(app, vctx);
                vctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        },
    );
}
```

Properties gained: draggable outside the main window, natively resizable,
minimize/maximize, survives overlapping the main window. The contents
function is the existing `Window` body (totals block, export button, file
table, notices), rendered into a `egui::CentralPanel` of the viewport's
own context.

State: `App` keeps owning everything (`show_report` toggles the viewport's
existence; closing via the OS title bar is detected through
`vctx.input(|i| i.viewport().close_requested())` inside the viewport
callback and flips `app.show_report = false`).

Notes / caveats:

- **Immediate vs deferred:** immediate mode reuses the existing `&mut App`
  borrow with zero new threading; deferred is only needed for render-when
  minimized — not our case. Immediate it is.
- **Repaint cadence:** the report is static after a run finishes; the
  viewport requests its own repaints at a lazy 200 ms while visible. While a
  job runs and the report is open (currently impossible — report only
  exists after a run; keep that invariant), nothing changes.
- **Close semantics:** opening a report for a *new* run while the old
  viewport is open simply re-renders the same viewport id with fresh
  contents (no window churn).

### Fix 2 — bounded inner layout (also the fallback)

Restructure the body so every scroll area has an explicit height bound:

```rust
// after the totals block + export row:
ui.separator();
let remaining = ui.available_height();          // window inner rect minus headers
egui::ScrollArea::vertical()
    .max_height(remaining - NOTICES_RESERVE)    // e.g. 150 px for the notices block
    .auto_shrink([false, false])
    .show_rows(ui, ROW_HEIGHT, rows, |ui, range| { ... });
```

The window now auto-sizes to `default_size` (not to 10 000 rows), and
drag-resize works because the scroll area's min height is bounded. The
notices block keeps its `max_height(120)`.

The `Embedded` fallback path renders this same bounded layout inside a
plain `egui::Window` (the trap remains on such backends, but sizing
becomes sane) — automatically, via the `ViewportClass` check above.

### Geometry persistence

Persist report-window size/position like the main window
(`app.rs:511-517` does this for `settings.window_size`): read
`ctx.input(|i| i.viewport().inner_rect)` / `outer_rect` inside the viewport
callback, store into `Settings::report_window_geometry: Option<[f32; 4]>`
(x, y, w, h; serde field with `#[serde(default)]` for old settings files),
restore via `ViewportBuilder::with_position`/`with_inner_size`.

## Tasks

1. `settings.rs`: add `report_window_geometry: Option<[f32; 4]>`
   (`#[serde(default)]`), round-trip test.
2. `report.rs`: extract `show_viewport_contents(app, &Context)` from the
   current `Window` body; bound the file-table scroll area
   (`max_height(remaining)`), keep ROW_HEIGHT virtualization.
3. `panels/mod.rs` + `app.rs`: spawn the viewport when `show_report` &&
   report exists; handle close-requested; persist geometry.
4. Manual test checklist (below); `cargo clippy -p byteshaver-gui` and
   `cargo test -p byteshaver-gui` green.

## Testing

- Unit (headless, existing pattern): geometry serde round-trip; the
  "report rows → bounded height" math stays trivial (no logic extracted —
  rendering only, consistent with the crate's testability doctrine).
- Manual matrix: Linux X11 + Wayland, Windows 10/11: drag outside main
  window, resize below content height (must not snap back), maximize,
  close via title bar (button state resets), reopen after new run,
  geometry restored across restart, JSONL export from the viewported
  window.

## Risks

| Risk | Mitigation |
|------|------------|
| Multi-viewport bugs on Wayland/winit combinations | release builds are X11-only (gui/README release table); `ViewportClass::Embedded` auto-fallback covers backends without support; desktop default build keeps wayland — test matrix below |
| Immediate viewport starving when parent stops repainting | viewport requests its own repaints; parent also repaints 100 ms while a job runs |
| Always-on-top annoyance | not set; plain window |
| Viewport creation cost per open | single persistent viewport id; egui reuses the OS window |
