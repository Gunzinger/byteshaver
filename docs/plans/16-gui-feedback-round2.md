# 16 — GUI feedback round 2: window fill, confetti origin, Windows reveal, action labels

**Size:** S (3 work packages) · **Status: implemented** on branch
`plans/gui-ux-improvements`. Follow-up to plan 15; issues F20–F23.

## F20 — dead grey space at the bottom of the preset-manage window and the visual-difference viewer

**Root causes:**
- Inspector: `MAX_CANVAS_HEIGHT = 560.0` (`gui/src/panels/inspector.rs:64`)
  caps the zoom/pan canvas — in a taller window everything below is dead
  space (and dead *input* space: the canvas is the interaction surface).
- Preset manage list: `max_height(available − MANAGE_BELOW_RESERVE)`
  with `MANAGE_BELOW_RESERVE = 150.0`
  (`gui/src/panels/options_panel.rs:79,1153`) over-reserves what the
  widgets below the list actually need → grey strip between list and
  buttons.
- Popup viewports open at their `default_size`; content shorter than the
  window shows native background below (preset save modal esp.).

**Fix:** the plan-09 doctrine is "bound by *available* height, never by
content" — apply it without reserve guessing:
- Inspector: drop `MAX_CANVAS_HEIGHT`; render the controls row first and
  give the canvas exactly `ui.available_height()` (which then is the
  remaining space — fills any window size, input surface grows too).
- Manage list (and, for consistency, the report file list): replace the
  fixed-reserve `max_height` with `.auto_shrink([false, true])` —
  the scroll area takes min(content, available) height, widgets below
  follow immediately, scrollbar appears only on overflow.
- Preset save modal: default viewport size tightened to fit its form.

## F21 — confetti should burst from the freshly converted rows

**Root cause:** plan 12 anchors the burst at the Convert button corner
(`viewport_size`-derived origin in `finish_job`).

**Fix:** the file table records the rect of each visible row's
size/ratio cell into `App` (`HashMap<PathBuf, RectPx>`, refreshed per
frame); on the celebration trigger, spawn bursts at the cells of rows
that finished `Encoded` in this run (up to the configured particle
budget distributed across origins; rows scrolled out of view simply
don't emit). `celebrate.rs` gains `burst_multi(count, &[RectPx], rng)`
(distribution pure + tested); the button-corner origin path is removed.

## F22 — "folder" action still opens Documents on Windows

**Root cause:** `Command::args([format!("/select,{}", path)])` goes
through the Windows CRT quoting rules — for any path containing spaces
the whole argument arrives quoted (`explorer "/select,C:\my dir\a.png"`),
which `explorer /select,` does not accept: it silently opens its default
view (Documents / Win11 Home). The `\\?\`-prefix strip from round 1 was
necessary but not sufficient.

**Fix:** on Windows build the argument with
`std::os::windows::process::CommandExt::raw_arg` in explorer's native
form: `/select,"<path>"` (quotes around the path only, never around
`/select,`). The pure argv string construction stays unit-tested
(cfg-gated); the `\\?\UNC\` → `\\` mapping is added to
`normalize_windows_select` while touching it.

## F23 — explicit action labels

"open" → **"open converted file"**, "folder" → **"open output folder"**
in the hover buttons, tooltips and the row context menu
(`gui/src/panels/file_table.rs`). Width impact checked (hover-revealed
buttons must not push the row layout; keep short-label fallback if the
actions area overflows — decision: buttons keep icons/short text, menu
carries the explicit wording, tooltip always explicit).

## Verification

`cargo test --workspace` + clippy clean; manual pass: tall inspector
window (canvas fills, pan/zoom works everywhere), manage window at
various sizes, confetti bursts on converted rows, Windows reveal with
spaces in the output path, menu wording.
