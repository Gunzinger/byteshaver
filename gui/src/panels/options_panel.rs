//! Options panel (bottom, auto-sizing per plan 13 §4): target format
//! picker (capability-driven, with disabled entries grayed out + reason
//! tooltip), the quality-ladder chips ([`crate::chips`]), the per-encoder
//! options editor ([`crate::options`]) behind a "Custom…"/"adjust ▾"
//! disclosure, the per-encoder "↺ defaults" button with undo notice and
//! the global policy mirrors (output directory, EXIF, collisions,
//! animation guards — every dropdown maps 1:1 onto a CLI flag).
//!
//! The panel height tracks its content: the measured content height of
//! each frame becomes the next frame's animation target (capped at 60 % of
//! the viewport; beyond the cap the internal `ScrollArea` keeps the rest
//! reachable). The two tree collapse states persist in
//! [`crate::settings::Settings`]; the "Custom…" disclosure and the
//! "Advanced" sub-header ride on egui's own persisted memory.

use std::time::Instant;

use egui::containers::collapsing_header::CollapsingState;

use crate::app::App;
use crate::chips::{self, Chip};
use crate::options;

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
        let config = ladder[index].encoder.clone();
        if app.settings.encoder != config {
            app.settings.encoder = config;
            app.mark_settings_dirty();
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
    let mut same_as_input = app.settings.output_dir.is_none();
    ui.horizontal(|ui| {
        ui.label("Output:");
        if ui
            .radio_value(&mut same_as_input, true, "same as input")
            .changed()
        {
            app.settings.output_dir = None;
            app.mark_settings_dirty();
        }
        if ui
            .radio_value(&mut same_as_input, false, "directory:")
            .changed()
        {
            app.settings.output_dir = Some(String::new());
            app.mark_settings_dirty();
        }
        if !same_as_input {
            if let Some(dir) = app.settings.output_dir.as_mut() {
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
                app.settings.output_dir = Some(path.display().to_string());
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
        let mut choice = app.settings.collision;
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
        if choice != app.settings.collision {
            app.settings.collision = choice;
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
        let mut mode = app.settings.exif.mode;
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
        if mode != app.settings.exif.mode {
            app.settings.exif.mode = mode;
            app.mark_settings_dirty();
        }
        let (enabled, text, field) = match mode {
            ExifMode::FilterExcept => (true, "except tags:", 0),
            ExifMode::KeepOnly => (true, "only tags:", 1),
            _ => (false, "", 0),
        };
        if enabled {
            let field = if field == 0 {
                &mut app.settings.exif.except_tags
            } else {
                &mut app.settings.exif.only_tags
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
        let mut policy = app.settings.animated_input;
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
        if policy != app.settings.animated_input {
            app.settings.animated_input = policy;
            app.mark_settings_dirty();
        }

        ui.label("animation memory cap:")
            .on_hover_text("hard cap for decoded animation memory in MiB (CLI: --max-animation-memory)");
        let mut mib = app.settings.max_animation_memory_mib;
        if ui.add(egui::DragValue::new(&mut mib).range(1..=1_048_576).suffix(" MiB")).changed() {
            app.settings.max_animation_memory_mib = mib;
            app.mark_settings_dirty();
        }

        ui.add_enabled_ui(app.capabilities.heif_input_enabled, |ui| {
            ui.label("HEIF multi-image:")
                .on_hover_text("how HEIC/HEIF files with multiple images are treated (CLI: --heif-image-policy; requires the dec-heif feature)");
            let mut heif = app.settings.heif_image_policy;
            let combo = egui::ComboBox::from_id_salt("heif-policy")
                .selected_text(match heif {
                    HeifImagePolicy::Primary => "primary image",
                    HeifImagePolicy::All => "all images",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut heif, HeifImagePolicy::Primary, "primary image");
                    ui.selectable_value(&mut heif, HeifImagePolicy::All, "all images (stem_1, stem_2, …)");
                });
            if heif != app.settings.heif_image_policy {
                app.settings.heif_image_policy = heif;
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
fn misc_ui(ui: &mut egui::Ui, app: &mut App) {
    let mut discard_larger = app.settings.discard_if_larger_than_input;
    if ui
        .checkbox(&mut discard_larger, "discard if larger than input")
        .on_hover_text("CLI: --discard-if-larger-than-input")
        .changed()
    {
        app.settings.discard_if_larger_than_input = discard_larger;
        app.mark_settings_dirty();
    }
    let mut discard_alpha = app.settings.discard_input_alpha_channel;
    if ui
        .checkbox(&mut discard_alpha, "discard input alpha channel")
        .on_hover_text("CLI: --discard-input-alpha-channel")
        .changed()
    {
        app.settings.discard_input_alpha_channel = discard_alpha;
        app.mark_settings_dirty();
    }
    let mut reverse = app.settings.reverse_processing_order;
    if ui
        .checkbox(&mut reverse, "reverse processing order")
        .on_hover_text("CLI: --reverse-processing-order")
        .changed()
    {
        app.settings.reverse_processing_order = reverse;
        app.mark_settings_dirty();
    }
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
