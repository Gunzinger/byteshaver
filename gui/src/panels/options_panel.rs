//! Options panel (bottom): target format picker (capability-driven, with
//! disabled entries grayed out + reason tooltip), the per-encoder options
//! editor ([`crate::options`]) and the global policy mirrors (output
//! directory, EXIF, collisions, animation guards — every dropdown maps 1:1
//! onto a CLI flag).

use crate::app::App;
use crate::options;

/// Renders the bottom options & policies panel.
pub fn show(app: &mut App, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("byteshaver-options")
        .resizable(true)
        .default_height(220.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // encoder picker + options ------------------------------
                    egui::CollapsingHeader::new("Target format & options")
                        .default_open(true)
                        .show(ui, |ui| {
                            encoder_picker(ui, app);
                            ui.add_space(4.0);
                            let before = app.settings.encoder.clone();
                            let (encoder, draft) = (&mut app.settings.encoder, &mut app.jxl_draft);
                            options::show_encoder_options(ui, encoder, draft);
                            if app.settings.encoder != before {
                                app.mark_settings_dirty();
                            }
                        });

                    ui.add_space(4.0);

                    // output + global policies -------------------------------
                    egui::CollapsingHeader::new("Output & global policies")
                        .default_open(true)
                        .show(ui, |ui| {
                            output_dir_ui(ui, app);
                            ui.separator();
                            collision_ui(ui, app);
                            exif_ui(ui, app);
                            ui.separator();
                            animation_ui(ui, app);
                            misc_ui(ui, app);
                        });
                });
        });
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
