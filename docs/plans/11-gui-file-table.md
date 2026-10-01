# 11 — GUI: file-table overhaul (sortable columns, configurable fields, output actions, thumbnails)

**Size:** L · **Depends on:** nothing (owns the shared core amendment
below) · **Parallelizable with:** all others after the core amendment lands
first as an isolated commit.

## Goal

Replace the hand-rolled `ScrollArea::show_rows` queue table
(`gui/src/panels/file_table.rs`) with a proper list that has:

1. **sortable columns** (click header: asc/desc/none, indicator glyph),
2. a **target format** column filled after a run (shown right after the
   achieved size),
3. **configurable data columns** (modified date, dimensions, EXIF fields
   such as camera / date-taken / ISO …) via a column-chooser that
   persists,
4. per-row **"open output" / "show in folder"** actions once converted
   (needs a small core amendment: outcomes must carry the output path),
5. **thumbnails** with a thoroughly considered overhead budget
   (background decode, visible-rows-only, LRU cache, off by default —
   see §6).

## 0. Core amendment (lands first, isolated commit)

`Outcome::Encoded` (`src/pipeline.rs:37-45`) reports sizes but **not where
the output was written**. The GUI needs the path for open/reveal and the
inspector (plan 10). Amend:

```rust
pub enum Outcome {
    Encoded {
        input_size: u64,
        output_size: u64,
        metadata_dropped: bool,
        output_path: PathBuf,   // NEW: actual written file (HEIF multi-image suffix included)
    },
    SkippedExisting { input_size: u64, existing_size: u64,
                      output_path: PathBuf },  // NEW (the kept preexisting file)
    ...
}
```

- Construction sites: `src/pipeline.rs` only (`claim_output` already holds
  the path). `Discarded*`/`SkippedCollision` unchanged (no file kept /
  path already present respectively).
- Ripples: CLI stdout reporter ignores it (no output change); the gui
  parity tests that construct `Outcome::Encoded` (`gui/src/app.rs:707`,
  `gui/src/queue.rs:504-510`) add `PathBuf::from("…")`; JSONL event schema
  gains a field — additive, same policy as previous `JobEvent` growth.
- Alternative rejected: GUI-side path recomputation (duplicating
  `output_path_for` + pattern-base resolution for `InputSelection::Files`)
  — fragile against core changes (HEIF `_N` suffixes, collision renames).

## 1. Table widget foundation

**Decision point D1 — egui_extras vs hand-rolled.**

| | A. `egui_extras::TableBuilder` (recommended) | B. extend hand-rolled rows |
|---|---|---|
| sortable headers, column resize, striped rows | built-in (`resizable(true)`, header widgets are plain buttons) | manual hit-testing per header cell |
| virtualization | `body.rows(h, n, …)` renders visible rows only (thumbnails hook in cleanly) | `show_rows` (already there) |
| new dependency | `egui_extras = "0.32"` matching the pinned egui line (verify lockstep availability at impl time; it exists for every egui release line historically) | none |
| release size | negligible (pure egui layer) | — |

Both plans 11 and 12's segmented bar stay custom-painted; the dependency is
table-only. **Recommendation: A**, with B as documented fallback if the
version pairing fails.

Column model (pure, `gui/src/table.rs` — new, no egui types):

```rust
pub enum Column { Status, Name, SourceFormat, InputSize, OutputSize,
                  TargetFormat, Modified, Dimensions, Ratio,
                  ExifCamera, ExifTaken, ExifIso, ExifExposure, Actions }
pub struct ColumnState { pub visible: Vec<Column>, /* order = display order */ }
pub enum SortKey { None, Asc(Column), Desc(Column) }
pub fn sort_indices(items: &[QueueItem], sort: SortKey, base_order: &[usize]) -> Vec<usize>;
```

- **Sort semantics** (pure, unit-tested): numeric for sizes/ratio/iso,
  date for modified/taken, case-insensitive lexicographic for name
  (natural sort for digit runs noted as a follow-up, not in scope),
  status-grouping for status, stable (ties fall back to queue order).
- **Sorting is view-only**: `Queue::selection()` keeps enqueue order;
  `begin_run`/events address rows by queue index, the table renders
  through the `sort_indices` permutation. Explicit tooltip: "sorting does
  not change conversion order".
- Column widths: user-resizable via the table; visible set + widths +
  sort persisted in `Settings` (`#[serde(default)]` → old settings files
  keep working).
- Default visible set: exactly today's columns **plus** Ratio and
  TargetFormat (see §3–4): `Status, Name, SourceFormat, InputSize→OutputSize
  (one merged column), Ratio, TargetFormat, Modified, Actions`. The merged
  size column keeps the established `in → out` string (familiar from the
  CLI report); OutputSize alone is offered as an alternative column for
  users who sort by it.

## 2. Header row & column chooser

- Header cells: `ui.add(SelectableLabel…)` or plain buttons; on click
  cycle `None → Asc → Desc → None`; indicator `▲ / ▼` (ASCII-safe, the
  symbol-font fallback in `main.rs:55-90` covers these).
- A `Columns ▾` button in the table header area opens a checkbox list of
  all `Column` variants (EXIF ones grouped under a "metadata" sub-header).
  Checkbox toggles visibility at the end of the order; **reordering is
  drag handles inside the chooser** (`egui::drag_source` pattern from the
  egui demo; if considered scope creep for v1, ship visibility-only and
  note drag-reorder as follow-up — decision point D2).

## 3. Target-format column

- `QueueItem` gains `converted_to: Option<&'static str>` (extension or
  encoder name, e.g. `avif`).
- Set from `options::encoder_kind_name(&settings.encoder)` +
  `info.extension` in `Queue::begin_run` (a run uses one global encoder);
  cleared on `begin_run` of a later run (rows carry only the latest run's
  result, same lifecycle as `output_size` today).
- Rendered after the achieved size (merged-size column) as planned: `…
  | 4.2MiB → 1.3MiB | avif | ✔ done | …`; empty for non-finished rows.

## 4. Extra data columns (file date, EXIF/metadata)

- **Modified**: `std::fs::metadata().modified()` — already stat'ed at
  enqueue; store `SystemTime` on `QueueItem` (extend `QueueItem::new`).
- **Dimensions** (W×H): `image::ImageReader::into_dimensions()` is a
  header-only read (~µs, no full decode) — safe on the UI thread at
  enqueue or lazily.
- **EXIF fields** (Camera = Make+Model, DateTimeOriginal, ISO,
  ExposureTime): read **once per file** on the background worker (§6's
  shared worker), cached in `HashMap<PathBuf, Option<ExifSummary>>` on
  `App`. Parsing lives in **core** as a new tiny public helper
  (`src/metadata/exif.rs` already has the reader; expose e.g.
  `pub fn summary(path) -> Option<ExifSummary>` with a serde-plain
  struct) so the GUI does not add a direct `kamadak-exif` dependency and
  duplicate tag tables (decision point D3 — alternative: gui-local dep;
  core helper keeps parsing in one place and benefits the job API).
  - Feature gate: `exif` is a default core feature (gui builds with it);
    without it, EXIF columns are absent from the chooser
    (`cfg`-free: core helper returns `None`s — the gui crate cannot see
    dependency cfgs, same doctrine as gui/README "Feature contract").
  - Rows that already finished keep whatever was cached; a re-run
    re-reads only missing entries.

## 5. Per-row output actions

- `QueueItem` gains `output_path: Option<PathBuf>` filled from the amended
  `Outcome` in `apply_outcome` (`queue.rs:290-329`).
- **Actions column** (or hover-revealed at row end, decision D4 — hover
  reveal keeps the table calm; recommended): two small buttons
  - `open output` — open the file with the OS default viewer,
  - `show in folder` — reveal+select in the OS file manager.
- Enabled only when `output_path.is_some()` + status ∈ {Encoded,
  SkippedExisting}; otherwise disabled with tooltip ("not converted yet"
  / "no output was written").
- Platform helpers in new `gui/src/platform.rs` (pure-ish, `#[cfg]`-split,
  unit-testable command construction):
  - Windows: `explorer /select,"<path>"` for reveal; `cmd /C start ""` or
    `rfd`-independent `start` for open.
  - macOS: `open -R <path>` / `open <path>`.
  - Linux: `dbus-send`/`gdbus` `org.freedesktop.FileManager1.ShowItems`
    first, `xdg-open <parent>` fallback for reveal; `xdg-open <path>` for
    open.
  - Prefer vetting the `opener`/`open` crates first (MIT; `opener` gained
    a `reveal` API — verify at impl time); hand-roll only what's missing.
    Errors surface as row tooltips, never dialogs.
- Same actions appear in the row context menu (shared with plan 10) and in
  the report window rows (plan 09's viewport).

## 6. Thumbnails (overhead considered)

**Default: off** (`settings.thumbnails: Off | OnAdd | OnConvert`,
recommended default `OnConvert` per the request: thumbnails appear "once
conversion starts" — i.e. rows that entered a run; `OnAdd` shows them from
enqueue).

Overhead budget (the analysis the request asks for):

| Cost | Measure |
|------|---------|
| Decode time | one dedicated worker thread (`std::thread` + mpsc), requests queued LRU-prioritized; typical 24 MP JPEG ≈ 80–150 ms, PNG ≈ 250–500 ms, WebP ≈ 100 ms — never on the UI thread |
| Contended with conversion | worker **paused while a job runs** (checks `App::running` between items; a job saturating rayon makes thumbnail progress moot anyway) |
| Memory (RAM) | bounded decode via `image::io::Limits` (512 MiB alloc cap) then immediate downscale to the thumb size; transient ≈ limits cap worst-case, typically ≤ 100 MiB live |
| Memory (cache) | thumb 48×48 (96×96 @2x texture for HiDPI) ≈ 9–37 KiB each; **LRU cap 1024 entries ≈ ≤ 40 MiB** textures; eviction drops the `TextureHandle` |
| GPU upload | one 48–96 px texture per visible row; `ctx.load_texture` ≈ 0.1 ms — invisible |
| Requests | **visible rows only**: the table body callback knows the visible range each frame; enqueues (re-)requests with a small hysteresis so fast scrolling doesn't thrash decode |
| Failure modes | corrupt/undecodable → placeholder glyph + tooltip; format not compiled in (HEIF) → placeholder + reason (pattern already exists for unsupported rows) |

Rendering: 48 px row height while thumbnails are on (22 px otherwise);
`body.rows(row_height, …)` receives the height so virtualization math
stays exact. Cache key: `(path, mtime)` — re-conversion (newer mtime)
invalidates.

`gui/Cargo.toml` gains a direct `image = "0.25"` dependency (same version
line as core; core's re-exports are intentionally not used crate-wide) —
shared with plan 10's metrics; whichever plan lands first adds it.

## File-ownership map

| File | Owner |
|------|-------|
| `src/pipeline.rs` (Outcome amendment + tests) | this plan, first commit only |
| `gui/src/table.rs`, `gui/src/platform.rs`, `gui/src/thumb.rs` (new) | this plan |
| `gui/src/panels/file_table.rs` | this plan (full rewrite) |
| `gui/src/queue.rs` | this plan (row model fields; 10 appends ratio helpers) |
| `gui/src/settings.rs` | additive fields `#[serde(default)]` (this + 12 + 14 append) |
| `gui/Cargo.toml` | this plan adds `egui_extras`, `image`; 10 adds `dssim` |

## Testing

- Headless: `sort_indices` (each column × direction, stability, ties,
  mixed unknown values), column-state serde round-trip + migration from a
  settings file without the new fields, `converted_to` lifecycle across
  two runs, `output_path` extraction from all `Outcome` variants,
  platform command construction (`cfg`-gated per OS, assert argv shape).
- Integration (existing harness): convert a fixture → assert the row
  carries the real on-disk `output_path`; HEIF `_1` suffix case if the
  feature build allows.
- Manual: 10 000-row queue sorts and scrolls smoothly; thumbnails stream
  in during idle after a run; hover actions on Windows/macOS/X11 open the
  right things; column chooser changes persist across restart.

## Decision points

1. **D1** `egui_extras` 0.32 adoption (recommended) vs hand-rolled.
2. **D2** column reorder UX now (drag handles) vs visibility-only v1.
3. **D3** EXIF summary via new core helper (recommended) vs gui-local
   `kamadak-exif` dep.
4. **D4** actions column vs hover-revealed buttons (recommended: hover).

## Risks

| Risk | Mitigation |
|------|------------|
| `egui_extras` 0.32 pairing unavailable | fallback B is fully specced (sort state + manual headers on top of current `show_rows`) |
| Outcome amendment churns CLI/JSONL | additive enum field, isolated first commit, JSONL schema note in PR description |
| Thumbnail decode stalls on network mounts | limits + worker isolation; `Off` default escape hatch |
| EXIF reads on huge queues | lazy (only when an EXIF column is visible), background, cached, skip-on-error |
