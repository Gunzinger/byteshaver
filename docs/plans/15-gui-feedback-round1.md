# 15 — GUI feedback round 1: adjustments & bug fixes

**Size:** M (4 work packages) · **Status: implemented** on branch
`plans/gui-ux-improvements` (all F1–F19; notable deviations: the
persistent-viewport host degrades to on-demand creation on Wayland —
winit's `set_visible` is a no-op there, so hidden windows would linger;
egui_extras 0.32 has no `hscroll` builder, so the horizontal scrollbar
wraps the table in an outer horizontally-scrolling area; preset ×
OutputMode semantics are documented in `presets/mod.rs` — full presets
apply the mode, the path string is kept locally unless opted in).

## Work packages / ownership

| WP | Scope | Files (owned) | Runs |
|----|-------|---------------|------|
| **A** — decode + reveal + descriptions | F4, F14, F15, F19 (root causes) | `src/input/`, `src/job/capabilities.rs`, `gui/src/metrics/**`, `gui/src/thumb.rs`, `gui/src/platform.rs`, small `gui/src/app.rs` metric-state touches | first (others build on its APIs) |
| **B** — file table | F5–F11, F19 (display half) | `gui/src/table.rs`, `gui/src/panels/file_table.rs` | parallel after A |
| **C** — options/presets/output/button | F1–F3, F16–F18 | `gui/src/chips.rs`, `gui/src/panels/options_panel.rs` (chip/form/dropdown regions), `gui/src/app.rs` (output mode, button state, blocker), `gui/src/panels/footer.rs` (button), `gui/src/settings.rs` (output mode) | parallel after A |
| **D** — windows | F12, F13 | `gui/src/viewports.rs` (new), `gui/src/panels/{report,about,inspector,options_panel}.rs` (window-host regions), `gui/src/panels/mod.rs` | last (needs C's preset windows) |

---

## F1 — preset → "Custom…" not highlighted; options hidden behind preset selection

**Symptoms:** selecting a chip then clicking "Custom…" does not highlight
Custom (only manual edits do); when a preset is selected the options form
stays collapsed, hiding what the preset contains.

**Fix (chips/options_panel):**
- Clicking the Custom control sets the *explicit* custom state
  immediately (the current implementation derives "custom" purely from
  `EncoderConfig != chip` equality; an untouched preset config equals the
  chip, so Custom never highlights). Add a tri-state selection model:
  `ChipSelected(index) | CustomExplicit | CustomDerived` where a manual
  edit maps `ChipSelected → CustomDerived`; only `CustomExplicit`/derived
  highlight the Custom control. Clicking Custom = `CustomExplicit`.
- The encoder-options collapsible ("Encoder options", see F2) is
  **open by default in every state** — preset or custom. Chip selection
  must NOT collapse it and must not auto-toggle it. Collapse/expand is
  exclusively user-driven (CollapsingState; the settings-persisted state
  stays, but the default when unset is open).
- The form must visibly reflect the applied preset values (it already
  renders from the same `EncoderConfig` — verify no cached schema state
  prevents this).

**Accept:** select preset → form open showing preset values, chip
highlighted; click Custom → Custom highlighted, values unchanged; edit →
chip unhighlights; collapse stays where the user put it across restarts.

## F2 — collapsible title inconsistency ("adjust ▾" / "⚙ Custom…")

**Symptoms:** title differs by entry path; "▾" in the title doubles the
CollapsingHeader's own arrow.

**Fix:** one constant title for the disclosure in all states:
**"Encoder options"** (no glyph arrows in the text). The "⚙ Custom…"
chip stays as the chip label only, never as the collapsible title.

**Accept:** identical title regardless of entry path; single arrow.

## F3 — ✦ quality dots render badly

**Fix:** remove the star/dot scale from chips and the preset detail line.
Keep the one-line textual description and the ⚡ (speed) / ▤ (size) emoji
indicators. Drop `quality_dots` from the render model (keep the field in
the data if plan-14 builtin table carries it, but stop rendering it).

**Accept:** no ✦ anywhere in the options panel; description + ⚡/▤ remain.

## F4 — format description embeds default options

**Symptoms:** picker description reads `Using "libjxl" (…) with options:
lossless=false quality=- …`, goes stale when settings change.

**Root cause:** core `EncoderInfo::description` = `encoder.describe()`
(`src/job/capabilities.rs`), which serializes the *default* options.

**Fix (core):** `describe()` implementations render only the encoder
identity: library/binding name (+ version line where already static) and
one-line character ("still-image AV1 encoder (ravif)"). No per-option
dump. Check CLI usages of `describe()`/capabilities output for text
expectations before changing (tests may pin strings — update them).

**Accept:** description stable while options change; no `quality=` text.

## F5 — Actions column has no header title

**Fix:** give it a header ("Actions") consistent with other headers
(non-sortable).

## F6 — "Taken" timestamp format

**Fix:** format `yyyy.mm.dd hh:mm:ss` (dots between date fields, colons
for time) in the EXIF taken formatter + its tests; tooltip mentions the
format.

## F7 — Dimensions column position/default

**Fix:** default column order becomes `Status, Name, Dimensions,
SourceFormat, MergedSize, Ratio, TargetFormat, Modified, Actions`.
Migration: a persisted `table_columns` that equals the **old** default
exactly is silently upgraded to the new default; any other stored order
is respected. Test both.

## F8 — column reordering via drag & drop headers

**Fix (table.rs/file_table.rs):** drag table headers to reorder visible
columns (egui: `ui.drag_drag_reorder`? — use the egui 0.32 drag-reorder
pattern: `egui::DragAndDrop`/`response.dnd_release_payload` + drag state,
or `ui.allocate_ui` with `dragged` ordering as in the egui demo
`drag_and_drop`). Persist the resulting order in `settings.table_columns`
(mark dirty). Header drag must not conflict with click-to-sort: drag
starts after a small movement threshold, click still cycles sort.

**Accept:** dragging a header reorders; order persists; sort-click still
works; hidden columns unaffected (chooser appends at the end as today).

## F9 — horizontal scroll indicator for the file list

**Fix:** enable the table's horizontal scrolling
(`TableBuilder`/`ScrollArea` `hscroll`) with sensible per-column minimum
widths so overflow is possible; egui then shows the horizontal scrollbar
(the indicator). Verify columns keep resizable behavior.

## F10 — center text in Ratio / Dimensions / Format / Target columns

**Fix:** render those cells with centered layout (matching the merged
size column's alignment).

## F11 — sort indicator on all columns

**Root cause:** header renderer decides the arrow from a stale/mismatched
comparison (shows on every column). **Fix:** arrow (▲/▼) only when
`SortKey` matches that exact column; test the pure predicate.

## F12 — report window flash on Windows; all popups should be independent windows

**Analysis:** the report viewport is created on demand
(`show_viewport_immediate` spawns a native winit window each time the
report opens). On Windows, winit window creation paints 1–2 default
(background/window-class) frames before egui's first paint — the flash.

**Fix (WP D):** persistent viewport host — new `gui/src/viewports.rs`
managing named viewports that are created **once** at startup and toggled
with `ViewportBuilder::with_visible(open)`; content renders every frame
for open ones (cheap), hidden ones skip painting where the API allows.
Creation flash is gone because creation happens before first paint of the
main window. `ViewportClass::Embedded` fallback per plan 09 unchanged
(embedded → bounded `egui::Window`).
Convert **all** popups to hosted viewports: report (already a viewport —
migrate onto the host), About, visual-difference inspector, preset save
modal, preset manage window.

**Accept:** no flash opening report repeatedly; About/inspector/preset
windows can be dragged outside the main window; closing via title bar
respects each window's state flag; embedded fallback still compiles.

## F13 — preset manage window vertical ceiling

**Fix:** remove the fixed `MANAGE_LIST_MAX_HEIGHT` ceiling; bound scroll
areas by `ui.available_height()` (dynamic, grows with the window) per the
plan-09 bounded-layout doctrine. Applies to the save modal too.

## F14 — "show in folder" opens Documents with a custom output folder

**Root cause (platform.rs):** two defects:
1. Windows: `explorer /select,<path>` fails silently (→ Explorer opens
   its default view, Win11 "Home"/Documents) when the path is not a
   canonical backslash path.
2. Linux: `gdbus` is spawned detached and its exit status is never
   observed — when no `FileManager1` service is registered, gdbus exits
   non-zero after we already returned success, and the `xdg-open <parent>`
   fallback never runs (it only covers a *missing gdbus binary*).

**Fix:**
- Windows: normalize the path (canonicalize; fallback: lexical
  absolutize) and force backslash separators before building
  `/select,<path>`; on spawn failure open the parent.
- Linux: spawn gdbus detached but watch it: a watcher thread polls
  `try_wait` for ~1 s; non-zero exit / failed spawn → `xdg-open <parent>`.
  Keep `file_uri` but feed it a canonicalized absolute path.
- Both paths unit-tested at the argv/normalization level (pure fns).

**Accept (manual, per OS):** reveal selects the written file in the
custom output folder; no Documents/`Home` fallback.

## F15 — inspector: "could not decode this pair (unsupported or unreadable file)" for jxl outputs

**Root cause:** `gui/src/metrics/decode.rs` decodes via the `image`
crate only — which has **no JXL/AVIF decoders**. The core decodes jxl
input via `jxl-oxide` (`src/input/jxl.rs`) and HEIF/AVIF via libheif
(`dec-heif` builds). Any jxl (and avif) *output* therefore fails to
decode in the inspector **and** in the quality metrics **and** as a
thumbnail source.

**Fix (core + gui):** new public core helper (in `src/input/`, e.g.
`pub fn decode_rgba(path: &Path) -> Result<image::RgbaImage, Error>`):
dispatches through the same input decoders the pipeline uses
(image-crate formats, jxl-oxide, HEIF when compiled in; animated input →
first frame, consistent with non-animated targets). `metrics/decode.rs`
and `thumb.rs` delegate to it (delete the gui-local image-only decode).
HEIF in stub builds keeps failing with the existing honest message.

**Accept:** inspector + measure-quality + thumbnails work for jxl
outputs in default builds; avif outputs work in `dec-heif` builds and
show the dec-heif reason otherwise.

## F16 — output "same as input" ↔ "directory:" toggle clears the path

**Fix (settings/app/options_panel):** replace
`policies.output_dir: Option<String>` with
`output_mode: OutputMode { SameAsInput, Directory }` (persisted) plus
`policies.output_dir: String` (kept verbatim; the `None = same as
input` encoding disappears). Toggling modes never touches the stored
string; `build_job_spec` uses mode+string. Legacy settings migration:
`output_dir: null → SameAsInput`, `output_dir: "…" → Directory`;
fixture-tested (both layouts, like the plan-14 migration).

## F17 — "directory:" with empty path silently behaves as same-as-input

**Fix:** when mode is `Directory` and the trimmed path is empty:
`start_blocker()` returns "output: directory mode is selected but no
folder is set — pick one or switch to 'same as input'"; the Convert
button is disabled with that tooltip and the message renders in the
footer error line on the start attempt. (No silent fallback.)

## F18 — Convert button state colors (low saturation)

**Fix (footer + pure helper):** `convert_button_colors(state)` →
- idle & ready: muted green fill
- blocked (blocker reason): muted gray-red tint, tooltip keeps reason
- running: muted amber (button shows Cancel semantics already separate —
  keep Convert disabled while running as today, tint communicates state)
- disabled-for-other-reasons keeps weak styling.
Colors derived from `ui.visuals()` where possible (dark/light aware);
pure mapping unit-tested. Saturation deliberately low (alpha ~
0.15–0.25 fills) but distinct.

## F19 — "Measure quality" appears to do nothing

**Root causes:** (a) F15 — the output decode fails for jxl/avif, the
worker stores/forgets an error that is not surfaced; (b) the result only
renders inside the status-cell tooltip — no visible per-row state.

**Fix:** (a) via F15's core decode; surface metric *errors* in the same
per-row text (not silently dropped). (b) WP B renders a visible metric
state per row (pending "…measuring", result `dssim 0.012 · psnr 41.2dB`,
error in red within the cell/tooltip) — coordinate with the existing
`metric_text` hook in `file_table.rs`. Verify the worker unpauses after
a job (`paused` flag refresh path in `App::update`) and that a manual
request while `Manual` mode is set enqueues immediately.

**Accept:** click Measure → row shows measuring → result or explicit
error; works for jxl outputs.

---

## Verification

- `cargo test --workspace`, `cargo clippy --workspace --all-targets`
  clean (pre-existing core `build.rs` warning excepted).
- Manual pass per feedback item on Linux + Windows (reveal, flash,
  centering, drag-reorder, toggles, colors).
- No regressions in the plan-09…14 behaviors (presets round-trip,
  segmented bar, viewport report).
