# 13 — GUI: target-format options presentation (three candidate approaches) & auto-sizing collapsible sections

**Size:** M · **Depends on:** nothing (integrates best after 14's preset
model exists — approach B consumes presets — but both orderings work;
see decision gate) · **Parallelizable with:** all others (owns
`gui/src/panels/options_panel.rs` and `gui/src/options.rs` editors).

## Goal

1. Make the target-format options easier to parse and more pleasing:
   present **three complete, implementable approaches** (§2) from which
   **one is selected** (decision gate at the end) — with aligned layout,
   a restore-defaults affordance, and sensible icons indicating
   quality/size/time trade-offs.
2. When the two `CollapsingHeader` trees ("Target format & options",
   "Output & global policies") collapse, the containing bottom panel
   **automatically shrinks** so the file table gets the space back; when
   expanded it grows again to reveal everything (§4).

Only the chosen approach of §2 gets implemented; the others remain in this
document as decision record.

## 1. What exists today (baseline & constraints)

- `panels/options_panel.rs:20-47`: a resizable `TopBottomPanel`
  (`default_height(220)`) wraps a `ScrollArea` around two always-rendered
  `CollapsingHeader`s. Collapsing a header does **not** shrink the panel —
  the empty space stays (the §4 complaint).
- `options.rs` holds one hand-written editor per encoder: label + control
  rows (`ui.horizontal`), e.g. jxl renders ~12 unaligned rows + an
  advanced-settings list. No grouping, no alignment, no reset affordance.
- Available primitives (egui 0.32, verified): `egui::Grid` (two-column
  alignment), `egui::Slider::text`, `CollapsingHeader`,
  `Context::animate_value_with_time` (`context.rs:3025`), and the
  symbol-font fallback machinery (`main.rs:55-90`) that makes unicode
  glyphs safe.
- Restore-defaults is nearly free: `options::default_encoder_config(name)`
  (`options.rs:64-78`) is the exact per-encoder CLI default, already
  unit-tested against the CLI.

Cross-cutting pieces **all three approaches share** (implement once):

- **Restore defaults button**: per-encoder `↺ defaults` (tooltip lists
  what changes: "resets N options"; uses the diff between current and
  default config — pure helper `option_diff(current, default) -> Vec<&'static str>`,
  unit-tested). Confirmation only when the diff is non-empty and the
  encoder was manually touched (cheap guard against rage-clicks; no
  dialog, an "undo" toast-style label instead — egui-native, no new dep).
- **Trade-off icons** where *sensible* only (rule: icon must encode
  information, never decorate): quality `✦` (higher = better), size
  `▤` (estimated output size direction), speed/time `⚡`. Concretely:
  quality sliders get a quality icon at the good end; speed/effort
  controls get ⚡ with "slower" tooltips; size-affecting toggles (zopfli,
  minimize-size) get ▤. No icons on pure bookkeeping options (interlace
  keep/none/adam7). Unicode glyphs chosen from the sets the
  symbol-font fallback covers (seguisym/DejaVu/Noto Symbols).
- **Units & ranges in labels** (`quality (0–100)`), every control keeps
  its CLI-flag tooltip (existing pattern).

## 2. The three candidate approaches

### Approach A — "Aligned form" (evolutionary; lowest risk)

Keep the two-collapsing-tree structure; rewrite each encoder editor into
an `egui::Grid` two-column form with grouped sub-headers and per-group
reset:

```
┌ Target format & options ────────────────────────────── ↺ defaults ┐
│ Format: [avif (.avif) ▾]  still-image AV1 encoder (ravif)         │
│ ── Quality ────────────────────────────────────────────────────   │
│   quality ✦        [────●────────────] 62        (CLI: -q)        │
│   alpha quality ✦  [──────●──────────] 90                            │
│ ── Speed / size ⚡ ───────────────────────────────────────────      │
│   speed            [4 ▾]   1 = slowest/best, 10 = fastest           │
│ ── Color ─────────────────────────────────────────────────────      │
│   bit depth        [auto ▾]    color model  [auto ▾]                │
└──────────────────────────────────────────────────────────────────┘
```

- Pros: smallest diff; keeps muscle memory; groups give the parseability
  win; per-group reset chips possible later.
- Cons: still a long form for jxl/oxipng; no guidance *which* values are
  sensible; presets (14) only appear as a separate dropdown.

### Approach B — "Quality ladder + custom disclosure" (recommended)

A row of **chips** — curated configurations, each with name, one-line
description and the three trade-off icons — plus `Custom…` expanding the
full aligned form of approach A:

```
┌ Target format & options ──────────────────────────────────────────┐
│ Format: [avif (.avif) ▾]                                           │
│ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌────────────┐ │
│ │ ● Compact    │ │ ● Balanced   │ │ ● High       │ │ ⚙ Custom…  │ │
│ │ ✦✦ ▤ smallest│ │ ✦✦✦ ▤ smaller │ │ ✦✦✦✦✦ ▤ ~50% │ │  full form │ │
│ │ ⚡ faster     │ │ ⚡ baseline   │ │ ⚡ slower     │ │            │ │
│ └──────────────┘ └──────────────┘ └──────────────┘ └────────────┘ │
│  Balanced: q62 · speed 4 · 8-bit · YCbCr — "web photos, ~half the  │
│  bytes of JPEG q85 at equal perception" [adjust ▾]                 │
└────────────────────────────────────────────────────────────────────┘
```

- Chips per encoder are **named parameter sets**; built-in chips come from
  plan 14's literature-backed profiles (avif/webp/jxl get 3, other
  formats get 1–2 sensible ones). Selecting a chip applies the config;
  the detail line shows the concrete values; `adjust ▾` opens the aligned
  form pre-filled (dirty state = chip unhighlights into "custom").
- Icons: dot-count `✦` scale for relative quality, `▤` for expected size
  (qualitative — no benchmark runs), `⚡` for relative encode time —
  sourced from the profile metadata table (14), not guessed at runtime.
- Pros: fastest path to a good outcome for most users; the numbers users
  actually should pick are one click away; the full surface stays
  reachable; presets get a first-class home.
- Cons: largest surface area of the three; depends on 14's profile data
  (or a small interim table shipped here and migrated).

### Approach C — "Comparison matrix" (power-tool)

A compact table listing every option of the selected encoder **against
the built-in profiles side-by-side**, current selection highlighted,
clicking a column header applies that profile:

```
│ option        │ Compact │ Balanced │ High   │ Custom │
│ quality ✦     │   50    │    62    │   78   │  62 ↔  │
│ speed ⚡       │    6    │    4     │    3   │   4 ↔  │
│ bit depth     │   8     │    8     │  auto  │  8  ↔  │
│ est. size ▤   │  ▬▬▬    │  ▬▬▬▬    │ ▬▬▬▬▬▬ │        │
│ est. time ⚡   │  ▬▬     │  ▬▬▬     │ ▬▬▬▬▬  │        │
```

- Pros: everything at once; comparing profiles is the point; delta cells
  (↔) show what the custom config changed vs the hovered profile.
- Cons: dense; weak for encoders with 1–2 profiles; a table of 2 rows
  (jpeg/webp-image) looks silly; the most code (custom table widget or
  egui_extras — coordination with 11).

### Comparison & decision gate

| Criterion | A | B | C |
|-----------|---|---|---|
| parseability for newcomers | ◑ | ✅ | ◑ |
| power-user speed | ◑ | ◑ | ✅ |
| implementation risk | low | medium | high |
| dependency on 14 (presets) | none | strong | strong |
| fits all 10 encoders | ✅ | ✅ | ◑ |

**Recommendation: B** (with A's aligned grid as its "Custom…" interior —
so B subsumes A's diff; C recorded as a possible v2 power toggle).
**Owner decision point D1: pick A, B or C before implementation starts.**
If 14 has not landed: ship B with a small in-crate table of chip
definitions (data shape = 14's `Preset`, migrated verbatim later).

## 3. Scope common to the chosen approach

- Rewrite `options.rs` editors: each `*_ui` becomes
  (a) a `Vec<OptionRow>` schema (label, control spec, group, cli-flag,
  trade-off icon, tooltip) — pure data, unit-testable — and (b) a single
  grid renderer consuming rows. This kills the per-encoder layout drift
  and gives 14 a reflection surface ("what options exist on this
  encoder").
- The JXL advanced `--setting` list and the oxipng multi-selects stay in a
  collapsed "Advanced" sub-header inside the form.
- Persist collapsed-state of the two trees + sub-headers (see §4) so the
  panel opens as left.

## 4. Auto-sizing collapsible sections (approach-independent)

Problem: `TopBottomPanel::resizable(true).default_height(220)` keeps its
height regardless of the trees' collapse state. Desired behavior: panel
height tracks content (collapsed → small, expanded → fits everything,
capped), animated smoothly.

Design (egui 0.32-verified mechanism):

1. Replace `resizable` with `TopBottomPanel::exact_height(ctx, h_each_frame)`.
2. Each frame render the content *inside* the panel, then read its
   measured height (`ui.min_rect().height()`) and store it as the
   animation **target** for the next frame.
3. `h_each_frame = ctx.animate_value_with_time(id, target, 0.20)` —
   smooth ease in/out; collapse shrinks, expand grows, both animated
   (respecting `reduced_motion` from plan 12: jump instantly then).
4. First-frame seeding: `animate_value_with_time` starts at 220 (current
   default); one-frame lag is invisible.
5. **Cap**: target = `min(measured, 60 % of viewport height)`; beyond the
   cap an internal `ScrollArea` (bounded, unlike today's) keeps the rest
   reachable — large jxl forms on small windows stay usable.
6. Persist the two tree collapse states in `Settings`
   (`#[serde(default = …)]` open) and seed/sync them through
   `egui::containers::collapsing_header::CollapsingState` (verified in
   0.32: `load_with_open_default`, `is_open`, `set_open`, `store`):
   startup seeds the state from settings (only when settings carry an
   explicit value), each frame's user toggles are read back from
   `CollapsingState` into `Settings` for the next launch — egui memory
   wins within a session, settings win at startup.
7. The freed space flows to the central panel automatically (egui panel
   layout) → the file table grows; `file_table` needs no change
   (it fills the remaining rect).

Edge cases: window resize during animation (target recomputed each frame
— fine); user drag-resize is intentionally *removed* (height is no longer
user-owned; decision point D2 — keep a manual resize handle as an
override, proposed: no, content-driven only).

## Testing

- Headless: `option_diff` per encoder (touch each field in a test,
  expect its label), OptionRow schema completeness per encoder (every
  `EncoderConfig` variant produces ≥ 1 row and every row's cli-flag
  tooltip string exists in the CLI help snapshot), chip-application
  logic for B (chip → `EncoderConfig` equality), panel-height target
  computation (collapsed/expanded/cap).
- Manual: collapse/expand both trees — table grows/shrinks smoothly; a
  768 px-tall window with jxl selected stays scrollable inside the panel;
  states survive restart; `↺ defaults` restores and the undo label
  appears.

## Decision points

1. **D1** choose approach A/B/C (recommendation: **B**).
2. **D2** drop the manual panel resize handle (recommended) vs keep as
   override.
3. B only: interim in-crate chip table vs hard dependency on 14 landing
   first.

## Risks

| Risk | Mitigation |
|------|------------|
| Height measurement feedback loop (panel sized by content that reacts to panel size) | content doesn't depend on panel height (only the capped ScrollArea does, and only at the cap) — verified by manual matrix; fall back to step-wise 8 px snapping if oscillation appears |
| B depends on 14's data | interim table migration path specced |
| Unicode icons missing on exotic fonts | glyph allow-list = fonts verified in `main.rs` fallback + `☐`-class ASCII fallbacks (`*`, `#`) defined per icon |
| egui_extras (C only) adds coupling to 11 | C explicitly lists it as a con |
