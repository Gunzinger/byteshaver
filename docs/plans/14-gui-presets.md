# 14 — GUI: configuration presets (user-defined + literature-based built-in profiles)

**Size:** L · **Depends on:** nothing hard; integrates with 13 (approach
B's chips consume built-in profiles; either order works) ·
**Parallelizable with:** all others (owns new `gui/src/presets/` and a
small options-panel dropdown hook).

## Goal

Users repeatedly need the same (encoder + options + output/global
policies) combinations. This plan adds:

1. **User presets**: save the *entire effective configuration* — target
   format with its full option surface **and** output/global policies —
   under a **title + description**, apply, edit, duplicate, delete.
2. **Persistence & sharing designed thoroughly**: stable file format,
   import/export as single shareable files, safe merging, forward/backward
   compatibility, privacy of local paths.
3. **Built-in read-only profiles** for avif, webp and jxl, derived from
   published data and literature (§5), included by default.

## 1. Preset model

```rust
// gui/src/presets/mod.rs  (new; serde types + logic only — no egui)
pub const PRESET_SCHEMA: u32 = 1;

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct Preset {
    pub schema: u32,                 // == PRESET_SCHEMA
    pub title: String,               // 1..=64 chars, trimmed, unique per scope (§3)
    pub description: String,         // ≤ 280 chars, may be empty
    pub created_unix: u64,
    pub modified_unix: u64,
    pub core_version: String,        // semver of the core it was authored with
    pub builtin: bool,               // built-ins are read-only, never persisted to user dir
    pub content: PresetContent,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct PresetContent {
    pub encoder: EncoderConfig,      // core serde (already round-trippable, verified)
    pub policies: PolicySet,         // output_dir, collision, exif(ExifSettings),
                                     // animated_input, heif_image_policy,
                                     // discard_if_larger_than_input,
                                     // discard_input_alpha_channel,
                                     // max_animation_memory_mib,
                                     // reverse_processing_order
                                     // == mirrors of Settings, reused 1:1
    pub include_output_dir: bool,    // privacy guard, see §2
}
```

- `PolicySet` is factored out of `Settings` (settings.rs fields move into
  it; `Settings` embeds it) — one type, used by settings, presets and the
  apply-path; existing settings files stay loadable (`#[serde(default)]`
  + `#[serde(flatten)]`-free explicit migration test).
- **Scope rule**: a preset may cover *only* the encoder (policies absent
  → `policies: None` variant? no — keep one shape, add
  `PresetContent { encoder, policies: Option<PolicySet> }`): "format-only"
  presets apply the encoder and leave policies untouched; full presets
  apply both. UI toggle at save time ("also save output & global
  policies", default on when policies differ from defaults). This
  matches the request's "presets of target format configurations *and*
  output/global policies" — both shapes exist.

## 2. Persistence

- **Location**: `Settings::path()` sibling dir: `<config>/byteshaver-gui/
  presets/<slug>.json` — **one file per preset** (not one big json):
  sharing = send one file; deletion = unlink; partial corruption of one
  preset never breaks the rest; matches the title-bearing sharing model.
- **Title ↔ filename**: `slug = lowercase, [a-z0-9-_], collapsed
  whitespace, max 48 chars`; collisions get `-2`, `-3` suffixes; the
  **title inside the file is authoritative**, the slug is only a stable
  filename (renames re-slug; import of a file whose title exists →
  "(imported)" suffix on the *title*, keeping both, since silently
  overwriting a user's preset is data loss). Slug fn is pure + tested
  (unicode → ascii-ish per title rules: keep it lazy — non-ascii chars
  are dropped; if the slug would be empty, fall back to `preset-<hash>`).
- **Writes**: atomic (`write tmp + rename`), like settings save; failures
  surface in the UI status area, never dialogs.
- **Privacy**: `include_output_dir == false` (the default when saving) →
  the preset's `policies.output_dir` is **not** exported/embedded at all
  (serde skip) so shared files never leak local paths; applying such a
  preset leaves the local output dir untouched. Explicit opt-in required
  to embed a path.
- **Loading & compatibility**:
  - corrupt/unknown file → skipped, listed in a "unreadable presets"
    note (the settings.rs corrupt→default doctrine, per-file).
  - `schema > PRESET_SCHEMA` (future file in older app) → visible but
    read-only with a "newer format" badge.
  - unknown `EncoderConfig` variant (preset from a build with a future/
    removed encoder) → visible, grayed with reason (the capabilities
    pattern), never blocks the rest.
  - `core_version` mismatch (older/newer) → yellow warning badge +
    tooltip; presets apply structurally (serde did the validation), a
    drifted *default* inside a known variant is fine (values are
    explicit).
- **Built-ins**: compiled into the binary (`presets::builtin() ->
  Vec<Preset>`, source: a const table in `gui/src/presets/builtin.rs`
  generated from §5) — no runtime files, update with the app, `builtin:
  true` → UI renders them in a separate "Built-in" group; *duplicate*
  makes them editable user presets.

## 3. Sharing

- **Export**: preset row → "Export…" → `rfd` save dialog
  (`<slug>.byteshaver-preset.json`, dual extension so the file is both
  recognizable and openable in editors). File = the exact on-disk JSON
  (schema included).
- **Import**: "Import preset…" → `rfd` open (`.json` filter) → validate →
  title dedup (§2) → save to user dir → appears immediately. Multi-select
  import supported (a preset pack is just N files; a `.zip` bundle is
  explicitly rejected as v2 scope).
- Round-trip guarantee is a unit test: `Preset → file → import → Preset`
  equality, including the privacy skip and unicode titles.

## 4. UI integration

- **Preset dropdown** in the options panel header row (left of the
  encoder picker): `[Preset: ● Balanced AVIF ▾]`
  - grouped menu: Built-ins / User presets / `Save current as preset…` /
    `Manage presets…` / `Import…`
  - active preset shown by title; dims when the config is edited away
    from it ("● Balanced AVIF ·modified") — equality check is the
    `PresetContent` compare against `App` state (pure fn, tested).
- **Save dialog**: a small modal (`egui::Window`, bounded like 09's fix):
  title (required), description (optional, multiline), scope toggle
  (format-only vs + policies), include-output-dir opt-in (disabled with
  privacy note unless an output dir is set).
- **Manage window**: list with title, description preview, scope badge,
  modified date; actions: apply, duplicate, rename (re-slug), edit
  description, export, delete (delete asks via inline confirm swap, no
  dialog).
- Keyboard: preset dropdown focusable; no further chords (v1).

## 5. Built-in profiles (from published data & literature)

Each built-in ships title, one-line description (with the *why*), full
`PresetContent` (format-only scope, no policies), and a `sources` note
kept in `builtin.rs` doc-comments. **Numbers below are candidate values
from the cited literature; the implementation task re-verifies each
source before baking them in** (house style: verified-ecosystem-facts
sections carry dates; facts as of 2026-10).

### AVIF (ravif)

| profile | candidate params | rationale (source to verify) |
|---------|------------------|------------------------------|
| **AVIF · Compact** | quality 50, speed 6, 8-bit, YCbCr | Netflix tech-blog AVIF guidance & Cloudinary/Squoosh-era comparisons: AVIF q~50 matches JPEG q~75–80 perception at ~half the bytes; speed 6 keeps encode times interactive |
| **AVIF · Balanced** | quality 62, speed 4, 8-bit, YCbCr | ravif author (kornelski) recommends quality ~60–70 as the useful band; speed 4 = batch-friendly sweet spot |
| **AVIF · High fidelity** | quality 78, speed 3, ten-bit (auto→ten where source allows), YCbCr | ten-bit reduces banding at high quality (AOM/AV1 10-bit guidance); near-visually-lossless tier |

### WebP (libwebp via `webp` crate)

| profile | candidate params | rationale |
|---------|------------------|-----------|
| **WebP · Compact** | quality 70 | Google's WebP study: q~70–75 retains strong fidelity for photos |
| **WebP · Balanced** | quality 80 | libwebp's own default (75) nudged to 80 for web photos; widely cited sweet spot |
| **WebP · High fidelity** | quality 90 | visually-safe re-encode tier; beyond 90 returns diminish sharply |
| (**WebP · Lossless** | lossless on) | for pixel art / screenshots; note libwebp lossless ≈ 25–35 % smaller than PNG (Google study) |

### JPEG XL (libjxl)

| profile | candidate params | rationale |
|---------|------------------|-----------|
| **JXL · Visually lossless** | distance 1.0, effort 7 | libjxl docs: d1.0 ≈ Butteraugli 1.0, the canonical visually-lossless operating point (cjxl's default lossy point) |
| **JXL · Compact** | distance 2.0, effort 8 | JPEG XL whitepaper/ICASSP rate-distortion data: ~2× the savings of d1.0 with small perceptual cost |
| **JXL · Bit-exact** | lossless, effort 7 | true lossless mode for archival |

Verification task checklist (impl time): libjxl `doc/` + `cjxl --help`
distance semantics; kornelski ravif README quality guidance; Google
WebP compression study (developers.google.com/speed/webp); Netflix
"AVIF for next-generation image coding" (netflixtechblog); Cloudinary
AVIF study 2020. Any number that fails verification ships with the
verified value and a comment citing the check.

## 6. Tasks (ordered)

1. `PolicySet` extraction from `Settings` (+ migration tests).
2. `gui/src/presets/mod.rs`: model, slug/dedup, atomic io, validation,
   import/export — pure, fully unit-tested (temp-dir io tests following
   `settings.rs` patterns).
3. `builtin.rs` table + a test asserting every builtin's encoder is a
   known variant and its numbers match the (verified) table.
4. UI: preset dropdown, save modal, manage window, apply path
   (`App::apply_preset` — pure, tested: assert full `Settings` state
   after applying each scope shape).
5. 13-integration: expose built-ins as chips if approach B was chosen.

## Testing

- Round-trip file io, slug/dedup matrix (unicode, empty-after-slug,
  collision suffixes), privacy skip (no `output_dir` in exported JSON),
  schema/variant forward-compat fixtures (hand-written JSON strings),
  apply-path state assertions, builtin sanity, settings migration from
  pre-PolicySet files.
- Manual: export on one machine/import on another (paths privacy noted),
  editing a builtin duplicate, preset dropdown dimming on edit.

## Decision points

1. Scope-toggle default at save time (proposed: full preset when policies
   differ from defaults, format-only otherwise).
2. Whether `jpeg`/`png`/`oxipng`/anim encoders get built-ins too (proposed:
   oxipng "safe re-optimize" only, others none — literature thin, GUI
   defaults already sane).
3. Export extension (`byteshaver-preset.json` proposed).

## Risks

| Risk | Mitigation |
|------|------------|
| EncoderConfig serde drift across core versions breaks old presets | schema + core_version guards, structural apply, grayed-not-blocked UX |
| Preset applies policies user forgot were embedded | apply preview line in the dropdown ("applies format + 5 policies"), scope badge everywhere |
| Literature numbers contested | per-profile verification checklist; descriptions carry hedged language |
| Filename slug abuse (title → path) | strict allow-list slug fn, tested |
