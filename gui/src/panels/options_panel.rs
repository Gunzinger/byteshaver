//! Options panel (bottom, auto-sizing per plan 13 §4): target format
//! picker (capability-driven, with disabled entries grayed out + reason
//! tooltip), the preset dropdown (plan 14 §4: grouped built-ins/user
//! presets + save/manage/import, "·modified" dimming), the quality-ladder
//! chips ([`crate::chips`], sourced from the built-in profiles), the
//! per-encoder options editor ([`crate::options`]) behind a "Custom…"/
//! "adjust ▾" disclosure, the per-encoder "↺ defaults" button with undo
//! notice and the global policy mirrors (output directory, EXIF,
//! collisions, animation guards — every dropdown maps 1:1 onto a CLI
//! flag).
//!
//! This module also renders the two preset windows (registered from
//! `panels::show`, both bounded like report.rs's doctrine): the "save
//! current as preset" modal ([`show_save_window`]) and the manage window
//! ([`show_manage_window`]) with apply/duplicate/rename/edit/export/
//! delete(inline confirm) actions.
//!
//! The panel height tracks its content: the measured content height of
//! each frame becomes the next frame's animation target (capped at 60 % of
//! the viewport; beyond the cap the internal `ScrollArea` keeps the rest
//! reachable). The two tree collapse states persist in
//! [`crate::settings::Settings`]; the "Custom…" disclosure and the
//! "Advanced" sub-header ride on egui's own persisted memory.

use std::time::Instant;

use egui::containers::collapsing_header::CollapsingState;

use crate::app::{App, PresetSaveDraft};
use crate::chips::{self, Chip};
use crate::options;
use crate::presets::{self, Preset, PresetRef};

/// Height animation duration (s); plan 12 will route this through a
/// `reduced_motion` setting (jump instantly) — one-line togglable here.
const PANEL_ANIMATION_TIME: f32 = 0.20;
/// Fraction of the viewport height the panel may occupy (beyond it the
/// internal ScrollArea scrolls).
const PANEL_HEIGHT_CAP: f32 = 0.6;
/// Panel height before the first content measurement (the old
/// `default_height`).
const PANEL_SEED_HEIGHT: f32 = 220.0;
/// Vertical frame margin/padding added on top of the measured content
/// height (`Frame::side_top_panel`'s 2+2 pt margin plus breathing room).
const PANEL_VERTICAL_PADDING: f32 = 6.0;

/// Fade duration of the "reset N options (undo?)" notice (s).
const RESET_NOTICE_FADE_SECS: f32 = 4.0;

/// egui temp-memory id of the last measured content height.
const MEASURED_HEIGHT_ID: &str = "byteshaver-options-content-height";
/// egui animation id of the animated panel height.
const ANIMATED_HEIGHT_ID: &str = "byteshaver-options-panel-height";
/// egui temp-memory id of the pending reset notice/undo state.
const RESET_NOTICE_ID: &str = "byteshaver-reset-notice";
/// egui persisted-memory ids of the two trees.
const TARGET_TREE_ID: &str = "byteshaver-tree-target-format";
const POLICIES_TREE_ID: &str = "byteshaver-tree-policies";
/// egui persisted-memory id of the custom-form disclosure.
const CUSTOM_FORM_ID: &str = "byteshaver-custom-form";
/// egui temp-memory ids of the manage window's inline editing/confirm
/// states (never persisted).
const RENAME_EDIT_ID: &str = "byteshaver-preset-rename";
const DESCRIPTION_EDIT_ID: &str = "byteshaver-preset-description-edit";
/// Draft text slots of the inline editors (kept next to their triggers).
const RENAME_DRAFT_ID: &str = "byteshaver-preset-rename-draft";
const DESCRIPTION_DRAFT_ID: &str = "byteshaver-preset-description-draft";
const DELETE_CONFIRM_ID: &str = "byteshaver-preset-delete-confirm";
/// Bounded-window geometry (plan 09's doctrine): the manage list scrolls
/// inside an explicit height bound.
const MANAGE_LIST_MAX_HEIGHT: f32 = 320.0;
const MANAGE_WINDOW_WIDTH: f32 = 600.0;
const SAVE_WINDOW_WIDTH: f32 = 420.0;

/// Undo/fade state of the last "↺ defaults" click (kept in egui temp
/// memory; never persisted).
#[derive(Clone)]
struct ResetNotice {
    /// When the reset happened (drives the fade).
    at: Instant,
    /// How many options the reset changed (notice text).
    count: usize,
    /// The pre-reset config restored by "undo".
    undo: byteshaver::config::EncoderConfig,
}

/// What the user clicked in the chip ladder this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LadderAction {
    /// Nothing clicked.
    None,
    /// Apply the chip at this index.
    Apply(usize),
    /// Open the custom-form disclosure (the "⚙ Custom…" card).
    OpenCustom,
}

/// Next-frame panel height target: the measured content height plus the
/// panel's vertical padding, capped at [`PANEL_HEIGHT_CAP`] of the
/// viewport. Pure (unit-tested for collapsed/expanded/cap).
fn panel_height_target(measured_content_height: f32, viewport_height: f32) -> f32 {
    (measured_content_height + PANEL_VERTICAL_PADDING).min(viewport_height * PANEL_HEIGHT_CAP)
}

/// Renders the bottom options & policies panel (auto-sizing).
pub fn show(app: &mut App, ctx: &egui::Context) {
    let viewport_height = ctx.screen_rect().height();
    let measure_id = egui::Id::new(MEASURED_HEIGHT_ID);
    let measured = ctx
        .data_mut(|data| data.get_temp::<f32>(measure_id))
        .unwrap_or(PANEL_SEED_HEIGHT);
    let target = panel_height_target(measured, viewport_height);
    let height = ctx.animate_value_with_time(
        egui::Id::new(ANIMATED_HEIGHT_ID),
        target,
        PANEL_ANIMATION_TIME,
    );

    egui::TopBottomPanel::bottom("byteshaver-options")
        .exact_height(height)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    target_format_tree(ui, app);
                    ui.add_space(4.0);
                    policies_tree(ui, app);
                    // content height of this frame = next frame's target
                    let content_height = ui.min_rect().height();
                    ctx.data_mut(|data| data.insert_temp(measure_id, content_height));
                });
        });
}

/// Which persisted tree a `collapsing_tree` call renders (selects the
/// [`crate::settings::Settings`] field that seeds/absorbs the state).
#[derive(Clone, Copy)]
enum SettingsTree {
    /// "Target format & options".
    Options,
    /// "Output & global policies".
    Policies,
}

/// Renders one tree with a persisted collapse state: egui memory wins
/// within a session, [`crate::settings::Settings`] seeds the state at
/// startup, and each frame's toggle is read back into the settings.
fn collapsing_tree(
    ui: &mut egui::Ui,
    app: &mut App,
    tree: SettingsTree,
    id_salt: &'static str,
    title: &str,
    header_extra: impl FnOnce(&mut egui::Ui, &mut App),
    body: impl FnOnce(&mut egui::Ui, &mut App),
) {
    let settings_open = match tree {
        SettingsTree::Options => app.settings.options_tree_open,
        SettingsTree::Policies => app.settings.policies_tree_open,
    };
    let id = egui::Id::new(id_salt);
    let state = CollapsingState::load_with_default_open(ui.ctx(), id, settings_open);
    let header = state.show_header(ui, |ui| {
        ui.strong(title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            header_extra(ui, app);
        });
    });
    let open = header.is_open();
    let _ = header.body(|ui| body(ui, app));
    if open != settings_open {
        match tree {
            SettingsTree::Options => app.settings.options_tree_open = open,
            SettingsTree::Policies => app.settings.policies_tree_open = open,
        }
        app.mark_settings_dirty();
    }
}

/// The "Target format & options" tree: encoder picker, quality ladder and
/// the custom disclosure with the aligned options form.
fn target_format_tree(ui: &mut egui::Ui, app: &mut App) {
    collapsing_tree(
        ui,
        app,
        SettingsTree::Options,
        TARGET_TREE_ID,
        "Target format & options",
        |ui, app| {
            reset_notice_ui(ui, app);
            reset_defaults_button(ui, app);
        },
        |ui, app| {
            encoder_picker(ui, app);
            ui.add_space(4.0);
            custom_disclosure(ui, app);
            ui.add_space(2.0);
        },
    );
}

/// The "Output & global policies" tree.
fn policies_tree(ui: &mut egui::Ui, app: &mut App) {
    collapsing_tree(
        ui,
        app,
        SettingsTree::Policies,
        POLICIES_TREE_ID,
        "Output & global policies",
        |_, _| {},
        |ui, app| {
            output_dir_ui(ui, app);
            ui.separator();
            collision_ui(ui, app);
            exif_ui(ui, app);
            ui.separator();
            animation_ui(ui, app);
            misc_ui(ui, app);
        },
    );
}

/// Quality-ladder chips plus the "⚙ Custom…"/"adjust ▾" disclosure around
/// the full options form. The ladder is skipped for encoders without chip
/// definitions (unknown names).
fn custom_disclosure(ui: &mut egui::Ui, app: &mut App) {
    let name = options::encoder_kind_name(&app.settings.encoder);
    let ladder = chips::chips_for(name);
    if ladder.is_empty() {
        editor_body(ui, app);
        return;
    }

    let selected = chips::selected_chip(&ladder, &app.settings.encoder);
    let mut action = LadderAction::None;
    ui.horizontal_wrapped(|ui| {
        for (index, chip) in ladder.iter().enumerate() {
            if chip_card(ui, chip, selected == Some(index)) && action == LadderAction::None {
                action = LadderAction::Apply(index);
            }
        }
        if custom_card(ui, selected.is_none()) {
            action = LadderAction::OpenCustom;
        }
    });
    if let Some(index) = selected {
        let chip = &ladder[index];
        ui.weak(format!("{}: {}", chip.title, chip.description));
    }
    if let LadderAction::Apply(index) = action {
        // chips route through the backing built-in preset so the active-
        // preset tracking (plan 14 §4) stays in sync with the ladder
        let preset_title = ladder[index].preset_title;
        let backing = app
            .builtins
            .iter()
            .find(|preset| preset.title == preset_title)
            .cloned();
        if let Some(preset) = backing
            && !presets::preset_matches(
                &preset,
                &app.settings.encoder,
                &app.settings.policies,
            )
        {
            let _ = app.apply_preset(&preset);
        }
    }

    // the form opens automatically in the custom state, closes when a chip
    // is applied, and can be toggled via "adjust ▾" while a chip is active
    let is_custom = chips::selected_chip(&ladder, &app.settings.encoder).is_none();
    let disclosure_id = egui::Id::new(CUSTOM_FORM_ID);
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), disclosure_id, false);
    match action {
        LadderAction::Apply(_) => state.set_open(false),
        LadderAction::OpenCustom => state.set_open(true),
        LadderAction::None => {
            if is_custom {
                state.set_open(true);
            }
        }
    }
    let header = state.show_header(ui, |ui| {
        if is_custom {
            ui.strong("⚙ Custom…");
        } else {
            ui.weak("adjust ▾");
        }
    });
    let _ = header.body(|ui| {
        ui.add_space(2.0);
        editor_body(ui, app);
    });
}

/// The per-encoder options editor; marks the settings dirty on any change.
fn editor_body(ui: &mut egui::Ui, app: &mut App) {
    let before = app.settings.encoder.clone();
    let (encoder, draft) = (&mut app.settings.encoder, &mut app.jxl_draft);
    options::show_encoder_options(ui, encoder, draft);
    if app.settings.encoder != before {
        app.mark_settings_dirty();
    }
}

/// One chip card of the ladder (selected = filled with the selection
/// color); returns whether it was clicked.
fn chip_card(ui: &mut egui::Ui, chip: &Chip, selected: bool) -> bool {
    let visuals = ui.visuals();
    let (fill, stroke) = if selected {
        (visuals.selection.bg_fill, visuals.selection.stroke)
    } else {
        (
            visuals.faint_bg_color,
            visuals.widgets.noninteractive.bg_stroke,
        )
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_min_width(120.0);
            ui.vertical(|ui| {
                ui.strong(chip.title);
                ui.weak(chip.icon_row());
            });
        })
        .response
        .interact(egui::Sense::click())
        .on_hover_text(format!("{} — {}", chip.title, chip.description))
        .clicked()
}

/// The trailing "⚙ Custom…" card of the ladder; returns whether it was
/// clicked (opens the full form).
fn custom_card(ui: &mut egui::Ui, selected: bool) -> bool {
    let visuals = ui.visuals();
    let (fill, stroke) = if selected {
        (visuals.selection.bg_fill, visuals.selection.stroke)
    } else {
        (
            visuals.faint_bg_color,
            visuals.widgets.noninteractive.bg_stroke,
        )
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_min_width(120.0);
            ui.vertical(|ui| {
                ui.strong("⚙ Custom…");
                ui.weak("full form");
            });
        })
        .response
        .interact(egui::Sense::click())
        .on_hover_text("open the full option form (leaves the chip ladder)")
        .clicked()
}

/// Per-encoder "↺ defaults": restores [`options::default_encoder_config`]
/// and arms the undo notice; the tooltip lists exactly what changes.
fn reset_defaults_button(ui: &mut egui::Ui, app: &mut App) {
    let current = app.settings.encoder.clone();
    let Some(default) = options::default_encoder_config(options::encoder_kind_name(&current))
    else {
        return;
    };
    let changed = options::option_diff(&current, &default);
    let tooltip = if changed.is_empty() {
        "already at the CLI defaults".to_owned()
    } else {
        format!("resets {} option(s): {}", changed.len(), changed.join(", "))
    };
    let response = ui.button("↺ defaults").on_hover_text(tooltip);
    if response.clicked() && !changed.is_empty() {
        let undo = current;
        app.settings.encoder = default;
        app.mark_settings_dirty();
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                egui::Id::new(RESET_NOTICE_ID),
                ResetNotice {
                    at: Instant::now(),
                    count: changed.len(),
                    undo,
                },
            );
        });
    }
}

/// The fading "reset N options (undo?)" label next to the defaults button.
fn reset_notice_ui(ui: &mut egui::Ui, app: &mut App) {
    let Some(notice) = ui
        .ctx()
        .data_mut(|data| data.get_temp::<ResetNotice>(egui::Id::new(RESET_NOTICE_ID)))
    else {
        return;
    };
    let elapsed = notice.at.elapsed().as_secs_f32();
    if elapsed >= RESET_NOTICE_FADE_SECS {
        ui.ctx()
            .data_mut(|data| data.remove::<ResetNotice>(egui::Id::new(RESET_NOTICE_ID)));
        return;
    }
    let alpha = 1.0 - elapsed / RESET_NOTICE_FADE_SECS;
    let color = ui.visuals().text_color().gamma_multiply(alpha);
    ui.label(egui::RichText::new(format!("reset {} option(s)", notice.count)).color(color));
    if ui.small_button("undo").clicked() {
        app.settings.encoder = notice.undo.clone();
        app.mark_settings_dirty();
        ui.ctx()
            .data_mut(|data| data.remove::<ResetNotice>(egui::Id::new(RESET_NOTICE_ID)));
    }
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(120));
}

/// Capability-driven encoder picker: every registry encoder is listed;
/// compiled-out ones are visible but disabled with the `disabled_reason`
/// tooltip (plan WS8 §1).
fn encoder_picker(ui: &mut egui::Ui, app: &mut App) {
    let entries: Vec<(String, String, bool, Option<String>)> = app
        .capabilities
        .encoders
        .iter()
        .map(|info| {
            (
                info.name.to_string(),
                format!("{} (.{})", info.name, info.extension),
                info.enabled,
                info.disabled_reason.map(str::to_string),
            )
        })
        .collect();
    let current = options::encoder_kind_name(&app.settings.encoder).to_string();
    let current_enabled = options::encoder_enabled(&app.capabilities, &current);
    let current_label = entries
        .iter()
        .find(|(name, _, _, _)| *name == current)
        .map(|(_, label, _, _)| label.clone())
        .unwrap_or_else(|| current.clone());

    ui.horizontal(|ui| {
        preset_dropdown(ui, app);
        ui.separator();
        ui.label("Format:");
        let response = egui::ComboBox::from_id_salt("encoder-picker")
            .selected_text(current_label)
            .width(160.0)
            .show_ui(ui, |ui| {
                for (name, label, enabled, reason) in &entries {
                    let mut item =
                        ui.add_enabled(*enabled, egui::Button::selectable(false, label.clone()));
                    if let Some(reason) = reason {
                        item = item.on_disabled_hover_text(reason.clone());
                    }
                    if *enabled && item.clicked() {
                        app.select_encoder(name);
                    }
                }
            });
        if !current_enabled && let Some(reason) = app.encoder_unavailable_reason() {
            response.response.on_hover_text(format!(
                "the selected encoder is not available in this build: {reason}"
            ));
        }
        let description = app
            .capabilities
            .encoders
            .iter()
            .find(|info| info.name == current)
            .map(|info| info.description.clone())
            .unwrap_or_default();
        ui.weak(description);
    });
}

/// Output directory: "same as input" (default) or a browsed path; maps
/// onto the spec's `output` override (CLI `-o`).
fn output_dir_ui(ui: &mut egui::Ui, app: &mut App) {
    let mut same_as_input = app.settings.policies.output_dir.is_none();
    ui.horizontal(|ui| {
        ui.label("Output:");
        if ui
            .radio_value(&mut same_as_input, true, "same as input")
            .changed()
        {
            app.settings.policies.output_dir = None;
            app.mark_settings_dirty();
        }
        if ui
            .radio_value(&mut same_as_input, false, "directory:")
            .changed()
        {
            app.settings.policies.output_dir = Some(String::new());
            app.mark_settings_dirty();
        }
        if !same_as_input {
            if let Some(dir) = app.settings.policies.output_dir.as_mut() {
                let response = ui.add(
                    egui::TextEdit::singleline(dir)
                        .hint_text("/path/to/output")
                        .desired_width(280.0),
                );
                if response.changed() {
                    app.mark_settings_dirty();
                }
            }
            if ui.button("Browse…").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_folder()
            {
                app.settings.policies.output_dir = Some(path.display().to_string());
                app.mark_settings_dirty();
            }
        }
    });
}

/// Collision/overwrite policy (CLI `--overwrite-if-smaller` /
/// `--overwrite-existing`).
fn collision_ui(ui: &mut egui::Ui, app: &mut App) {
    use crate::settings::CollisionChoice;
    ui.horizontal(|ui| {
        ui.label("Collision:")
            .on_hover_text("behavior when an output file already exists (CLI: --overwrite-if-smaller / --overwrite-existing)");
        let mut choice = app.settings.policies.collision;
        egui::ComboBox::from_id_salt("collision-policy")
            .selected_text(match choice {
                CollisionChoice::KeepExisting => "keep existing",
                CollisionChoice::OverwriteIfSmaller => "overwrite if smaller",
                CollisionChoice::OverwriteAlways => "overwrite always",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut choice,
                    CollisionChoice::KeepExisting,
                    "keep existing (default)",
                );
                ui.selectable_value(
                    &mut choice,
                    CollisionChoice::OverwriteIfSmaller,
                    "overwrite if smaller",
                );
                ui.selectable_value(
                    &mut choice,
                    CollisionChoice::OverwriteAlways,
                    "overwrite always",
                );
            });
        if choice != app.settings.policies.collision {
            app.settings.policies.collision = choice;
            app.mark_settings_dirty();
        }
    });
}

/// EXIF policy editor (CLI `--exif` / `--exif-except` / `--exif-only`).
fn exif_ui(ui: &mut egui::Ui, app: &mut App) {
    use crate::settings::ExifMode;
    ui.horizontal(|ui| {
        ui.label("EXIF:")
            .on_hover_text("how EXIF metadata of the source is treated (CLI: --exif)");
        let mut mode = app.settings.policies.exif.mode;
        egui::ComboBox::from_id_salt("exif-policy")
            .selected_text(match mode {
                ExifMode::Strip => "strip (default)",
                ExifMode::Keep => "keep",
                ExifMode::FilterExcept => "filter: keep except tags",
                ExifMode::KeepOnly => "filter: keep only tags",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut mode, ExifMode::Strip, "strip (default)");
                ui.selectable_value(&mut mode, ExifMode::Keep, "keep");
                ui.selectable_value(&mut mode, ExifMode::FilterExcept, "filter: keep except tags");
                ui.selectable_value(&mut mode, ExifMode::KeepOnly, "filter: keep only tags");
            });
        if mode != app.settings.policies.exif.mode {
            app.settings.policies.exif.mode = mode;
            app.mark_settings_dirty();
        }
        let (enabled, text, field) = match mode {
            ExifMode::FilterExcept => (true, "except tags:", 0),
            ExifMode::KeepOnly => (true, "only tags:", 1),
            _ => (false, "", 0),
        };
        if enabled {
            let field = if field == 0 {
                &mut app.settings.policies.exif.except_tags
            } else {
                &mut app.settings.policies.exif.only_tags
            };
            let response = ui
                .add(
                    egui::TextEdit::singleline(field)
                        .hint_text("gps, Orientation, DateTimeOriginal")
                        .desired_width(260.0),
                )
                .on_hover_text("comma-separated tag names, IFD wildcards (ifd0/exif/gps/…) or numeric tags (0x8825); see --exif-list-tags");
            if response.changed() {
                app.mark_settings_dirty();
            }
        } else {
            ui.weak(text);
        }
    });
}

/// Animation guards (CLI `--animated-input`, `--max-animation-memory`) and
/// the HEIF multi-image policy (only effective with `dec-heif`).
fn animation_ui(ui: &mut egui::Ui, app: &mut App) {
    use byteshaver::config::{AnimatedInputPolicy, HeifImagePolicy};
    ui.horizontal(|ui| {
        ui.label("Animated input:")
            .on_hover_text("behavior when animated input meets a target that cannot encode animations (CLI: --animated-input)");
        let mut policy = app.settings.policies.animated_input;
        egui::ComboBox::from_id_salt("animated-input")
            .selected_text(match policy {
                AnimatedInputPolicy::FirstFrame => "encode first frame",
                AnimatedInputPolicy::Error => "error",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut policy,
                    AnimatedInputPolicy::FirstFrame,
                    "encode first frame (default)",
                );
                ui.selectable_value(&mut policy, AnimatedInputPolicy::Error, "error");
            });
        if policy != app.settings.policies.animated_input {
            app.settings.policies.animated_input = policy;
            app.mark_settings_dirty();
        }

        ui.label("animation memory cap:")
            .on_hover_text("hard cap for decoded animation memory in MiB (CLI: --max-animation-memory)");
        let mut mib = app.settings.policies.max_animation_memory_mib;
        if ui.add(egui::DragValue::new(&mut mib).range(1..=1_048_576).suffix(" MiB")).changed() {
            app.settings.policies.max_animation_memory_mib = mib;
            app.mark_settings_dirty();
        }

        ui.add_enabled_ui(app.capabilities.heif_input_enabled, |ui| {
            ui.label("HEIF multi-image:")
                .on_hover_text("how HEIC/HEIF files with multiple images are treated (CLI: --heif-image-policy; requires the dec-heif feature)");
            let mut heif = app.settings.policies.heif_image_policy;
            let combo = egui::ComboBox::from_id_salt("heif-policy")
                .selected_text(match heif {
                    HeifImagePolicy::Primary => "primary image",
                    HeifImagePolicy::All => "all images",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut heif, HeifImagePolicy::Primary, "primary image");
                    ui.selectable_value(&mut heif, HeifImagePolicy::All, "all images (stem_1, stem_2, …)");
                });
            if heif != app.settings.policies.heif_image_policy {
                app.settings.policies.heif_image_policy = heif;
                app.mark_settings_dirty();
            }
            if !app.capabilities.heif_input_enabled {
                combo.response.on_disabled_hover_text(
                    "this build was compiled without the \"dec-heif\" feature",
                );
            }
        });
    });
}

/// Remaining conversion flags of the CLI.
fn misc_ui(ui: &mut egui::Ui, app: &mut App) {    let mut discard_larger = app.settings.policies.discard_if_larger_than_input;
    if ui
        .checkbox(&mut discard_larger, "discard if larger than input")
        .on_hover_text("CLI: --discard-if-larger-than-input")
        .changed()
    {
        app.settings.policies.discard_if_larger_than_input = discard_larger;
        app.mark_settings_dirty();
    }
    let mut discard_alpha = app.settings.policies.discard_input_alpha_channel;
    if ui
        .checkbox(&mut discard_alpha, "discard input alpha channel")
        .on_hover_text("CLI: --discard-input-alpha-channel")
        .changed()
    {
        app.settings.policies.discard_input_alpha_channel = discard_alpha;
        app.mark_settings_dirty();
    }
    let mut reverse = app.settings.policies.reverse_processing_order;
    if ui
        .checkbox(&mut reverse, "reverse processing order")
        .on_hover_text("CLI: --reverse-processing-order")
        .changed()
    {
        app.settings.policies.reverse_processing_order = reverse;
        app.mark_settings_dirty();
    }
}

// ---- presets (plan 14 §4) -----------------------------------------------------

/// One prepared entry of the preset dropdown menu (snapshot built before
/// the menu closure so the app is not mutably borrowed while rendering).
struct PresetMenuEntry {
    reference: PresetRef,
    title: String,
    preview: String,
    enabled: bool,
    /// Grayed-out reason (encoder not compiled in / newer format).
    reason: Option<String>,
    /// Extra badge text (foreign core version).
    badge: Option<String>,
}

/// The preset dropdown, left of the encoder picker (plan 14 §4): grouped
/// Built-ins / User presets plus the Save / Manage / Import actions. The
/// active preset is shown in the button and dims to "·modified" when the
/// state drifts ([`App::active_preset_label`]).
fn preset_dropdown(ui: &mut egui::Ui, app: &mut App) {
    ui.label("Preset:");
    let label = app
        .active_preset_label()
        .unwrap_or_else(|| "—".to_string());
    let button_label = egui::RichText::new(format!("● {label} ▾"));
    let builtin_entries: Vec<PresetMenuEntry> = app
        .builtins
        .iter()
        .enumerate()
        .map(|(index, preset)| builtin_menu_entry(app, index, preset))
        .collect();
    let user_entries: Vec<PresetMenuEntry> =
        app.user_presets.iter().map(user_menu_entry).collect();

    let mut chosen: Option<PresetRef> = None;
    let mut open_save = false;
    let mut open_manage = false;
    let mut import_clicked = false;

    let dropdown_response = ui.button(button_label);
    egui::Popup::menu(&dropdown_response).show(|ui| {
        ui.set_min_width(280.0);
        ui.weak("Built-in");
        for entry in &builtin_entries {
            menu_entry_ui(ui, entry, &mut chosen);
        }
        ui.separator();
        ui.weak(if user_entries.is_empty() {
            "User presets (none yet)"
        } else {
            "User presets"
        });
        for entry in &user_entries {
            menu_entry_ui(ui, entry, &mut chosen);
        }
        ui.separator();
        if ui.button("Save current as preset…").clicked() {
            open_save = true;
        }
        if ui.button("Manage presets…").clicked() {
            open_manage = true;
        }
        if ui.button("Import…").clicked() {
            import_clicked = true;
        }
    });

    if let Some(reference) = chosen
        && let Some(preset) =
            presets::find_preset(&app.builtins, &app.user_presets, &reference).cloned()
    {
        match app.apply_preset(&preset) {
            Ok(()) => app.preset_status = None,
            Err(message) => app.preset_status = Some(message),
        }
    }
    if open_save {
        app.preset_save = Some(PresetSaveDraft::from_current(app));
    }
    if open_manage {
        app.show_preset_manager = true;
    }
    if import_clicked {
        import_presets_dialog(app);
    }
}

/// One dropdown entry (enabled or grayed with its reason; hovering shows
/// the apply-preview and any badge).
fn menu_entry_ui(ui: &mut egui::Ui, entry: &PresetMenuEntry, chosen: &mut Option<PresetRef>) {
    let mut item = ui.add_enabled(
        entry.enabled,
        egui::Button::selectable(false, entry.title.clone()),
    );
    let mut hover = format!("{} — {}", entry.preview, entry.title);
    if let Some(badge) = &entry.badge {
        hover = format!("{hover} (authored for core {badge})");
    }
    item = item.on_hover_text(hover);
    if let Some(reason) = &entry.reason {
        item = item.on_disabled_hover_text(reason);
    }
    if entry.enabled && item.clicked() {
        *chosen = Some(entry.reference.clone());
    }
}

/// Builds a dropdown entry for a built-in (grayed with the capability
/// reason when the encoder is not compiled into this build).
fn builtin_menu_entry(app: &App, index: usize, preset: &Preset) -> PresetMenuEntry {
    let kind = options::encoder_kind_name(&preset.content.encoder);
    let enabled = options::encoder_enabled(&app.capabilities, kind)
        && !presets::is_newer_format(preset);
    let reason = if presets::is_newer_format(preset) {
        Some("newer preset format — cannot be applied".to_string())
    } else {
        options::encoder_disabled_reason(&app.capabilities, kind)
            .map(|reason| format!("this build cannot encode {kind}: {reason}"))
    };
    PresetMenuEntry {
        reference: PresetRef::Builtin(index),
        title: preset.title.clone(),
        preview: presets::apply_preview(preset),
        enabled,
        reason,
        badge: presets::is_foreign_version(preset).then(|| preset.core_version.clone()),
    }
}

/// Builds a dropdown entry for a user preset (newer-format files are
/// visible but cannot be applied).
fn user_menu_entry(stored: &presets::StoredPreset) -> PresetMenuEntry {
    let preset = &stored.preset;
    let enabled = !presets::is_newer_format(preset);
    let reason = presets::is_newer_format(preset)
        .then(|| "newer preset format — cannot be applied".to_string());
    PresetMenuEntry {
        reference: PresetRef::User(preset.title.clone()),
        title: preset.title.clone(),
        preview: presets::apply_preview(preset),
        enabled,
        reason,
        badge: presets::is_foreign_version(preset).then(|| preset.core_version.clone()),
    }
}

/// File-picker import flow (plan 14 §3): multi-select `.json`, per-file
/// errors collect into the status line — never dialogs.
fn import_presets_dialog(app: &mut App) {
    let Some(paths) = rfd::FileDialog::new()
        .add_filter("byteshaver presets", &["json"])
        .pick_files()
    else {
        return;
    };
    match app.import_presets(&paths) {
        Ok(0) => app.preset_status = Some("no presets imported".to_string()),
        Ok(count) => {
            app.preset_status = Some(format!("imported {count} preset(s)"));
        }
        Err(message) => app.preset_status = Some(message),
    }
}

/// The bounded "save current as preset" modal (plan 14 §4): title
/// (required + unique, validated live), description, the scope toggle
/// (default per decision 1) and the output-dir opt-in (disabled with a
/// privacy note unless a directory is set).
pub fn show_save_window(app: &mut App, ctx: &egui::Context) {
    let Some(draft) = app.preset_save.clone() else {
        return;
    };
    let mut open = true;
    let mut draft = draft;
    let mut cancel = false;
    let mut save: Option<PresetSaveDraft> = None;

    egui::Window::new("Save current as preset")
        .open(&mut open)
        .default_width(SAVE_WINDOW_WIDTH)
        .show(ctx, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut draft.title)
                    .hint_text("title (required)")
                    .desired_width(f32::INFINITY),
            );
            ui.add(
                egui::TextEdit::multiline(&mut draft.description)
                    .hint_text("description (optional, one line about the why)")
                    .desired_width(f32::INFINITY)
                    .desired_rows(2),
            );
            ui.add_space(4.0);
            let policy_changes = presets::policy_change_count(&app.settings.policies);
            if ui
                .checkbox(
                    &mut draft.include_policies,
                    format!("also save output & global policies ({policy_changes} changed)"),
                )
                .on_hover_text(
                    "off = format-only preset: applying it leaves your policies untouched",
                )
                .changed()
                && !draft.include_policies
            {
                draft.include_output_dir = false;
            }
            let has_output_dir = app.settings.policies.output_dir.is_some();
            ui.add_enabled_ui(draft.include_policies && has_output_dir, |ui| {
                let checkbox = ui.checkbox(
                    &mut draft.include_output_dir,
                    "include the output directory path",
                );
                if !has_output_dir {
                    checkbox.on_disabled_hover_text(
                        "no output directory is set (\"same as input\") — nothing to embed",
                    );
                } else if let Some(dir) = app.settings.policies.output_dir.as_deref() {
                    checkbox.on_hover_text(format!(
                        "off (recommended): the preset file never embeds local paths; \
                         on: the file will contain {dir:?}"
                    ));
                }
            });
            ui.add_space(4.0);
            let titles: Vec<String> = app
                .user_presets
                .iter()
                .map(|stored| stored.preset.title.clone())
                .collect();
            if let Err(message) = presets::validate_draft(&draft.title, &draft.description, &titles)
            {
                ui.colored_label(ui.visuals().warn_fg_color, message);
            }
            if let Some(status) = &app.preset_status {
                ui.colored_label(ui.visuals().error_fg_color, status);
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = Some(draft.clone());
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

    if let Some(draft) = save {
        match app.save_preset_from_current(&draft) {
            Ok(()) => {
                app.preset_save = None;
                app.preset_status = Some(format!("saved preset {:?}", draft.title.trim()));
            }
            Err(message) => app.preset_status = Some(message),
        }
    } else if cancel {
        app.preset_save = None;
        app.preset_status = None;
    }
    if !open {
        app.preset_save = None;
    }
}

/// A prepared row of the manage window (snapshot; see [`PresetMenuEntry`]).
struct ManageRow {
    preset: Preset,
    scope: String,
    modified: String,
    read_only: bool,
    foreign: Option<String>,
    newer: bool,
}

/// The bounded manage-presets window (plan 14 §4): grouped list with
/// title, description preview, scope badge and modified date; actions
/// apply / duplicate / rename / edit description / export / delete
/// (delete confirms inline). Follows report.rs's bounded-layout doctrine:
/// the list scrolls inside an explicit height bound.
pub fn show_manage_window(app: &mut App, ctx: &egui::Context) {
    if !app.show_preset_manager {
        return;
    }
    let mut open = true;
    let mut import_clicked = false;

    // snapshots for the action pass (the closure borrows ui/app-data only)
    let builtin_rows: Vec<ManageRow> = app
        .builtins
        .iter()
        .map(manage_row_of)
        .collect();
    let user_rows: Vec<ManageRow> = app
        .user_presets
        .iter()
        .map(|stored| manage_row_of(&stored.preset))
        .collect();
    let unreadable: Vec<presets::Unreadable> = app.unreadable_presets.clone();

    // action intents collected while drawing
    let mut unreadable_remove: Option<String> = None;
    let mut apply: Option<PresetRef> = None;
    let mut duplicate: Option<Preset> = None;
    let mut export: Option<Preset> = None;
    let mut delete: Option<String> = None;
    let mut delete_confirmed: Option<String> = None;
    let mut rename_start: Option<String> = None;
    let mut rename_submit: Option<(String, String)> = None; // (old, new)
    let mut description_start: Option<String> = None;
    let mut description_submit: Option<(String, String)> = None; // (title, text)

    egui::Window::new("Manage presets")
        .open(&mut open)
        .default_width(MANAGE_WINDOW_WIDTH)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Import…").clicked() {
                    import_clicked = true;
                }
                ui.weak(format!(
                    "{} user preset(s) in {}",
                    user_rows.len(),
                    app.preset_store
                        .as_ref()
                        .map_or_else(|| "?".to_string(), |store| store.dir().display().to_string())
                ));
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .max_height(MANAGE_LIST_MAX_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.strong("Built-in (read-only)");
                    for row in &builtin_rows {
                        manage_row_ui(
                            ui,
                            row,
                            &mut apply,
                            &mut duplicate,
                            &mut export,
                            &mut rename_start,
                            &mut description_start,
                            &mut delete,
                        );
                        ui.separator();
                    }
                    ui.strong("User presets");
                    if user_rows.is_empty() {
                        ui.weak("(none yet — save the current configuration as a preset)");
                    }
                    for row in &user_rows {
                        manage_row_ui(
                            ui,
                            row,
                            &mut apply,
                            &mut duplicate,
                            &mut export,
                            &mut rename_start,
                            &mut description_start,
                            &mut delete,
                        );
                        ui.separator();
                    }
                    if !unreadable.is_empty() {
                        ui.strong("Unreadable files");
                        for unreadable in &unreadable {
                            ui.horizontal_wrapped(|ui| {
                                let name = ui
                                    .label(
                                        egui::RichText::new(unreadable.file_name.clone())
                                            .weak()
                                            .strikethrough(),
                                    )
                                    .on_hover_text(unreadable.reason.clone());
                                if let Some(raw) = &unreadable.raw_json {
                                    let preview: String =
                                        raw.to_string().chars().take(400).collect();
                                    name.on_hover_text(format!(
                                        "raw content (kept for recovery):\n{preview}"
                                    ));
                                }
                                if ui.button("Remove file").clicked() {
                                    unreadable_remove = Some(unreadable.file_name.clone());
                                }
                            });
                        }
                    }
                });

            // inline rename editor (draft prefilled with the current title)
            let rename = ui
                .ctx()
                .data_mut(|data| data.get_temp::<String>(egui::Id::new(RENAME_EDIT_ID)));
            if let Some(old) = rename {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("New title:");
                    let mut draft = ui
                        .ctx()
                        .data_mut(|data| data.get_temp::<String>(egui::Id::new(RENAME_DRAFT_ID)))
                        .unwrap_or_else(|| old.clone());
                    let response = ui.text_edit_singleline(&mut draft);
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(egui::Id::new(RENAME_DRAFT_ID), draft.clone())
                    });
                    if ui.button("Rename").clicked() || response.lost_focus() {
                        rename_submit = Some((old.clone(), draft));
                        ui.ctx().data_mut(|data| {
                            data.remove::<String>(egui::Id::new(RENAME_EDIT_ID));
                            data.remove::<String>(egui::Id::new(RENAME_DRAFT_ID));
                        });
                    }
                    if ui.button("Cancel").clicked() {
                        ui.ctx().data_mut(|data| {
                            data.remove::<String>(egui::Id::new(RENAME_EDIT_ID));
                            data.remove::<String>(egui::Id::new(RENAME_DRAFT_ID));
                        });
                    }
                });
            }

            // inline description editor (draft prefilled with the current text)
            let description_edit = ui
                .ctx()
                .data_mut(|data| data.get_temp::<String>(egui::Id::new(DESCRIPTION_EDIT_ID)));
            if let Some(title) = description_edit {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(format!("Description of {title:?}:"));
                    let mut draft = ui
                        .ctx()
                        .data_mut(|data| {
                            data.get_temp::<String>(egui::Id::new(DESCRIPTION_DRAFT_ID))
                        })
                        .unwrap_or_else(|| {
                            app_user_description(&user_rows, &title)
                        });
                    ui.add(
                        egui::TextEdit::multiline(&mut draft)
                            .desired_width(340.0)
                            .desired_rows(2),
                    );
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(egui::Id::new(DESCRIPTION_DRAFT_ID), draft.clone());
                    });
                    if ui.button("Save").clicked() {
                        description_submit = Some((title.clone(), draft.clone()));
                        ui.ctx().data_mut(|data| {
                            data.remove::<String>(egui::Id::new(DESCRIPTION_EDIT_ID));
                            data.remove::<String>(egui::Id::new(DESCRIPTION_DRAFT_ID));
                        });
                    }
                    if ui.button("Cancel").clicked() {
                        ui.ctx().data_mut(|data| {
                            data.remove::<String>(egui::Id::new(DESCRIPTION_EDIT_ID));
                            data.remove::<String>(egui::Id::new(DESCRIPTION_DRAFT_ID));
                        });
                    }
                });
            }

            // inline delete confirm swap
            let confirming = ui
                .ctx()
                .data_mut(|data| data.get_temp::<String>(egui::Id::new(DELETE_CONFIRM_ID)));
            if let Some(title) = confirming {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("delete {title:?} permanently?"),
                    );
                    if ui.button("Yes, delete").clicked() {
                        delete_confirmed = Some(title.clone());
                        ui.ctx()
                            .data_mut(|data| data.remove::<String>(egui::Id::new(DELETE_CONFIRM_ID)));
                    }
                    if ui.button("Keep").clicked() {
                        ui.ctx()
                            .data_mut(|data| data.remove::<String>(egui::Id::new(DELETE_CONFIRM_ID)));
                    }
                });
            }

            if let Some(status) = &app.preset_status {
                ui.separator();
                ui.weak(status);
            }
        });

    // ---- action pass (app mutated outside the window closure) -----------------
    if let Some(reference) = apply
        && let Some(preset) =
            presets::find_preset(&app.builtins, &app.user_presets, &reference).cloned()
    {
        match app.apply_preset(&preset) {
            Ok(()) => app.preset_status = Some(format!("applied {:?}", preset.title)),
            Err(message) => app.preset_status = Some(message),
        }
    }
    if let Some(source) = duplicate {
        match app.preset_duplicate(&source) {
            Ok(()) => {}
            Err(message) => app.preset_status = Some(message),
        }
    }
    if let Some(preset) = export {
        let suggested = presets::export_file_name(&preset);
        if let Some(target) = rfd::FileDialog::new()
            .add_filter("byteshaver presets", &["json"])
            .set_file_name(suggested)
            .save_file()
        {
            match app.preset_export(&preset, target) {
                Ok(()) => app.preset_status = Some("preset exported".to_string()),
                Err(message) => app.preset_status = Some(message),
            }
        }
    }
    if let Some(title) = rename_start {
        ui_context_set(ctx, RENAME_EDIT_ID, title);
    }
    if let Some((old, new)) = rename_submit {
        match app.preset_rename(&old, &new) {
            Ok(()) => {}
            Err(message) => app.preset_status = Some(message),
        }
    }
    if let Some(title) = description_start {
        ui_context_set(ctx, DESCRIPTION_EDIT_ID, title);
    }
    if let Some((title, text)) = description_submit {
        match app.preset_edit_description(&title, &text) {
            Ok(()) => {}
            Err(message) => app.preset_status = Some(message),
        }
    }
    if let Some(title) = delete {
        ui_context_set(ctx, DELETE_CONFIRM_ID, title);
    }
    if let Some(file_name) = unreadable_remove
        && let Some(store) = &app.preset_store
    {
        match store.delete_file(&file_name) {
            Ok(()) => {
                app.reload_unreadable_only();
                app.preset_status = Some(format!("removed {file_name}"));
            }
            Err(message) => app.preset_status = Some(message),
        }
    }
    if let Some(title) = delete_confirmed {
        match app.preset_delete(&title) {
            Ok(()) => app.preset_status = Some(format!("deleted {title:?}")),
            Err(message) => app.preset_status = Some(message),
        }
    }
    if import_clicked {
        import_presets_dialog(app);
    }
    app.show_preset_manager = open;
}

/// Small helper for the temp-memory one-shot states (rename/description/
/// delete-confirm) — sets `id_salt` to `value` and clears its draft slot.
fn ui_context_set(ctx: &egui::Context, id_salt: &str, value: String) {
    ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new(id_salt), value);
    });
}

/// Builds the manage-window row model of one preset (scope badge, modified
/// date, read-only/badges; pure).
fn manage_row_of(preset: &Preset) -> ManageRow {
    let scope = match &preset.content.policies {
        None => "format only".to_string(),
        Some(policies) => {
            format!("format + {} policies", presets::policy_change_count(policies))
        }
    };
    ManageRow {
        preset: preset.clone(),
        scope,
        modified: if preset.builtin {
            "built-in".to_string()
        } else {
            format!("modified {}", presets::format_date(preset.modified_unix))
        },
        read_only: preset.builtin || presets::is_newer_format(preset),
        foreign: presets::is_foreign_version(preset).then(|| preset.core_version.clone()),
        newer: presets::is_newer_format(preset),
    }
}

/// One manage-window row: title + badges, description preview + meta, and
/// the action buttons (read-only rows keep apply/duplicate/export).
#[allow(clippy::too_many_arguments)]
fn manage_row_ui(
    ui: &mut egui::Ui,
    row: &ManageRow,
    apply: &mut Option<PresetRef>,
    duplicate: &mut Option<Preset>,
    export: &mut Option<Preset>,
    rename_start: &mut Option<String>,
    description_start: &mut Option<String>,
    delete: &mut Option<String>,
) {
    ui.vertical(|ui| {
        ui.horizontal_wrapped(|ui| {
            let title_response = ui.strong(row.preset.title.clone());
            if row.newer {
                title_response
                    .on_hover_text("written by a newer app generation — visible, read-only");
                ui.colored_label(ui.visuals().warn_fg_color, "newer format");
            }
            if let Some(version) = &row.foreign {
                ui.colored_label(ui.visuals().warn_fg_color, format!("core {version}"))
                    .on_hover_text(
                        "authored by a different core version; it still applies structurally",
                    );
            }
            if row.read_only {
                ui.weak("(read-only)");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let reference = manage_reference(row);
                if ui.add_enabled(!row.newer, egui::Button::new("Apply")).clicked() {
                    *apply = Some(reference);
                }
                if ui.button("Duplicate").clicked() {
                    *duplicate = Some(row.preset.clone());
                }
                if ui.button("Export…").clicked() {
                    *export = Some(row.preset.clone());
                }
                if !row.read_only {
                    if ui.button("Rename").clicked() {
                        *rename_start = Some(row.preset.title.clone());
                    }
                    if ui.button("Edit description").clicked() {
                        *description_start = Some(row.preset.title.clone());
                    }
                    if ui.button("Delete").clicked() {
                        *delete = Some(row.preset.title.clone());
                    }
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            let description = if row.preset.description.is_empty() {
                "(no description)".to_string()
            } else {
                row.preset.description.clone()
            };
            ui.weak(description);
            ui.separator();
            ui.weak(format!("{} · {}", row.scope, row.modified));
        });
    });
}

/// The [`PresetRef`] of a manage row (built-ins resolve by title in the
/// action pass).
fn manage_reference(row: &ManageRow) -> PresetRef {
    if row.preset.builtin {
        PresetRef::Builtin(
            crate::presets::profiles()
                .iter()
                .position(|profile| profile.preset_title == row.preset.title)
                .unwrap_or_default(),
        )
    } else {
        PresetRef::User(row.preset.title.clone())
    }
}

/// The current description of a user preset (the inline description
/// editor's prefill; empty for unknown titles).
fn app_user_description(rows: &[ManageRow], title: &str) -> String {
    rows.iter()
        .find(|row| row.preset.title == title)
        .map_or_else(String::new, |row| row.preset.description.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_height_target_tracks_content_and_caps_at_the_viewport() {
        // collapsed trees: the panel shrinks to the small content height
        assert_eq!(
            panel_height_target(40.0, 1000.0),
            40.0 + PANEL_VERTICAL_PADDING
        );
        // first-frame seed: 220 pt content fits an average viewport
        assert_eq!(
            panel_height_target(PANEL_SEED_HEIGHT, 2000.0),
            PANEL_SEED_HEIGHT + PANEL_VERTICAL_PADDING
        );
        // expanded beyond the cap: at most 60 % of the viewport
        assert_eq!(panel_height_target(2000.0, 720.0), 720.0 * PANEL_HEIGHT_CAP);
        // at the cap the target stays capped even as content grows further
        assert_eq!(
            panel_height_target(f32::MAX, 720.0),
            720.0 * PANEL_HEIGHT_CAP
        );
    }
}
