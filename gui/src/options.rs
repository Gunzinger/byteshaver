//! Per-encoder options editors (plan WS8 §5.4, task §3).
//!
//! One hand-written editor function per [`EncoderConfig`] variant; the
//! module also holds the **pure**, egui-free plumbing the rest of the GUI
//! needs: capability-name ↔ encoder-kind mapping, defaults per encoder
//! (identical to the CLI subcommand defaults) and capability-driven
//! availability checks.
//!
//! All state mutation happens directly on the `EncoderConfig` stored in the
//! app settings (immediate mode); no rendering state is kept here, so the
//! pure helpers stay unit-testable without a display server. The EXIF
//! policy fields of the oxipng/jxl options are **not** edited here — they
//! are injected from the global policy when building the job spec
//! (mirroring the CLI, see `app::App::build_job_spec`).

use byteshaver::config::{
    AlphaColorMode, ApngOptions, AvifOptions, BitDepth, ColorModel, CompressionType, EncoderConfig,
    FilterType, GifOptions, JxlBitDepthChoice, JxlColorEncodingChoice, JxlOptions, OxipngFilter,
    OxipngInterlace, OxipngLevel, OxipngOptions, OxipngReduction, OxipngStrip, PngOptions,
    WebpAnimOptions, WebpOptions,
};
use byteshaver::job::Capabilities;

use crate::app::JxlAdvancedDraft;

/// Registry-order (capability) names of all encoders, in the same order as
/// `capabilities().encoders` (test fixture for the availability checks and
/// default comparisons).
#[cfg(test)]
pub const ENCODER_NAMES: [&str; 10] = [
    "webp",
    "webp-image",
    "avif",
    "png",
    "jpeg",
    "jxl",
    "oxipng",
    "webp-anim",
    "apng",
    "gif",
];

/// The capability name (`CLI subcommand`) of an encoder config.
#[must_use]
pub fn encoder_kind_name(encoder: &EncoderConfig) -> &'static str {
    match encoder {
        EncoderConfig::Webp(_) => "webp",
        EncoderConfig::WebpImage => "webp-image",
        EncoderConfig::Avif(_) => "avif",
        EncoderConfig::Png(_) => "png",
        EncoderConfig::Jpeg => "jpeg",
        EncoderConfig::Jxl(_) => "jxl",
        EncoderConfig::Oxipng(_) => "oxipng",
        EncoderConfig::WebpAnim(_) => "webp-anim",
        EncoderConfig::Apng(_) => "apng",
        EncoderConfig::Gif(_) => "gif",
    }
}

/// Default encoder configuration for a capability name, matching the CLI
/// subcommand defaults (`EncoderConfig::from_args` with all flags unset;
/// for `gif` that means the crate-default palette speed, like the CLI).
#[must_use]
pub fn default_encoder_config(name: &str) -> Option<EncoderConfig> {
    match name {
        "webp" => Some(EncoderConfig::Webp(WebpOptions::default())),
        "webp-image" => Some(EncoderConfig::WebpImage),
        "avif" => Some(EncoderConfig::Avif(AvifOptions::default())),
        "png" => Some(EncoderConfig::Png(PngOptions::default())),
        "jpeg" => Some(EncoderConfig::Jpeg),
        "jxl" => Some(EncoderConfig::Jxl(JxlOptions::default())),
        "oxipng" => Some(EncoderConfig::Oxipng(OxipngOptions::default())),
        "webp-anim" => Some(EncoderConfig::WebpAnim(WebpAnimOptions::default())),
        "apng" => Some(EncoderConfig::Apng(ApngOptions::default())),
        "gif" => Some(EncoderConfig::Gif(GifOptions::default())),
        _ => None,
    }
}

/// Whether the named encoder is compiled into this build (from
/// [`capabilities()`][byteshaver::job::capabilities]).
#[must_use]
pub fn encoder_enabled(caps: &Capabilities, name: &str) -> bool {
    caps.encoders
        .iter()
        .find(|info| info.name == name)
        .is_some_and(|info| info.enabled)
}

/// Why the named encoder is unavailable (`None` when available or unknown).
#[must_use]
pub fn encoder_disabled_reason(caps: &Capabilities, name: &str) -> Option<&'static str> {
    caps.encoders
        .iter()
        .find(|info| info.name == name)
        .and_then(|info| info.disabled_reason)
}

/// Renders the options editor for the selected encoder (dispatch by
/// variant; disabled encoders are never selected, so no editor is shown).
pub fn show_encoder_options(
    ui: &mut egui::Ui,
    encoder: &mut EncoderConfig,
    draft: &mut JxlAdvancedDraft,
) {
    match encoder {
        EncoderConfig::Webp(options) => webp_ui(ui, options),
        EncoderConfig::WebpImage => {
            ui.label("The image-crate lossless webp encoder has no options.");
        }
        EncoderConfig::Avif(options) => avif_ui(ui, options),
        EncoderConfig::Png(options) => png_ui(ui, options),
        EncoderConfig::Jpeg => {
            ui.label("The mozjpeg-based jpeg encoder has no options.");
        }
        EncoderConfig::Jxl(options) => jxl_ui(ui, options, draft),
        EncoderConfig::Oxipng(options) => oxipng_ui(ui, options),
        EncoderConfig::WebpAnim(options) => webp_anim_ui(ui, options),
        EncoderConfig::Apng(options) => png_ui(
            ui,
            &mut PngOptions {
                compression_type: options.compression_type,
                filter_type: options.filter_type,
            },
        ),
        EncoderConfig::Gif(options) => gif_ui(ui, options),
    }
}

// ---- shared editor helpers ----------------------------------------------

/// Dropdown over a fixed variant table.
fn combo<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut T,
    variants: &[(T, &str)],
) -> egui::InnerResponse<Option<()>> {
    let selected = variants
        .iter()
        .find(|(candidate, _)| candidate == value)
        .map_or("?", |(_, name)| *name);
    egui::ComboBox::from_id_salt(label)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for (variant, name) in variants {
                ui.selectable_value(value, *variant, *name);
            }
        })
}

/// Dropdown over `Option<T>` with an "auto/default" entry mapped to `None`.
fn optional_combo<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<T>,
    variants: &[(T, &str)],
) {
    let selected = match value {
        None => "auto".to_string(),
        Some(current) => variants
            .iter()
            .find(|(candidate, _)| candidate == current)
            .map_or("?".to_string(), |(_, name)| (*name).to_string()),
    };
    egui::ComboBox::from_id_salt(label)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, "auto");
            for (variant, name) in variants {
                ui.selectable_value(value, Some(*variant), *name);
            }
        });
}

/// Checkbox row over a `Vec<T>` membership list.
fn multi_select<T: PartialEq + Copy>(ui: &mut egui::Ui, list: &mut Vec<T>, variants: &[(T, &str)]) {
    ui.horizontal_wrapped(|ui| {
        for (variant, name) in variants {
            let mut checked = list.contains(variant);
            if ui.checkbox(&mut checked, *name).changed() {
                if checked {
                    if !list.contains(variant) {
                        list.push(*variant);
                    }
                } else {
                    list.retain(|member| member != variant);
                }
            }
        }
    });
}

/// "0 = auto" integer editor for an `Option` number field.
fn optional_number<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<N>,
    range: std::ops::RangeInclusive<N>,
) {
    let mut current = value.unwrap_or_else(|| *range.start());
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(&mut current).range(range));
    });
    *value = Some(current);
}

// ---- per-encoder editors -------------------------------------------------

fn webp_ui(ui: &mut egui::Ui, options: &mut WebpOptions) {
    ui.checkbox(&mut options.lossless, "lossless")
        .on_hover_text("lossless encoding mode");
    ui.horizontal(|ui| {
        ui.label("quality");
        ui.add(egui::Slider::new(&mut options.quality, 0.0..=100.0));
    });
}

fn avif_ui(ui: &mut egui::Ui, options: &mut AvifOptions) {
    ui.horizontal(|ui| {
        ui.label("quality");
        ui.add(egui::Slider::new(&mut options.quality, 0.0..=100.0));
    });
    ui.horizontal(|ui| {
        ui.label("speed");
        ui.add(egui::DragValue::new(&mut options.speed).range(1..=10))
            .on_hover_text("1 = slowest/best, 10 = fastest");
    });
    ui.horizontal(|ui| {
        ui.label("alpha quality");
        ui.add(egui::Slider::new(&mut options.alpha_quality, 0.0..=100.0));
    });
    ui.horizontal(|ui| {
        ui.label("bit depth");
        optional_combo(
            ui,
            "avif-bit-depth",
            &mut options.bit_depth,
            &[(BitDepth::Eight, "8"), (BitDepth::Ten, "10")],
        );
    });
    ui.horizontal(|ui| {
        ui.label("color model");
        optional_combo(
            ui,
            "avif-color-model",
            &mut options.color_model,
            &[(ColorModel::YCbCr, "YCbCr"), (ColorModel::RGB, "RGB")],
        );
    });
    ui.horizontal(|ui| {
        ui.label("alpha color mode");
        optional_combo(
            ui,
            "avif-alpha-color-mode",
            &mut options.alpha_color_mode,
            &[
                (AlphaColorMode::UnassociatedDirty, "unassociated dirty"),
                (AlphaColorMode::UnassociatedClean, "unassociated clean"),
                (AlphaColorMode::Premultiplied, "premultiplied"),
            ],
        );
    });
}

fn png_ui(ui: &mut egui::Ui, options: &mut PngOptions) {
    ui.horizontal(|ui| {
        ui.label("compression");
        optional_combo(
            ui,
            "png-compression",
            &mut options.compression_type,
            &[
                (CompressionType::Default, "default"),
                (CompressionType::Best, "best"),
                (CompressionType::Fast, "fast"),
            ],
        );
    });
    ui.horizontal(|ui| {
        ui.label("filter");
        optional_combo(
            ui,
            "png-filter",
            &mut options.filter_type,
            &[
                (FilterType::NoFilter, "none"),
                (FilterType::Sub, "sub"),
                (FilterType::Up, "up"),
                (FilterType::Avg, "avg"),
                (FilterType::Paeth, "paeth"),
                (FilterType::Adaptive, "adaptive"),
            ],
        );
    });
}

fn jxl_ui(ui: &mut egui::Ui, options: &mut JxlOptions, draft: &mut JxlAdvancedDraft) {
    ui.checkbox(&mut options.lossless, "lossless")
        .on_hover_text("true lossless mode; overrides quality/distance, implies original profile");
    ui.horizontal(|ui| {
        ui.label("quality");
        optional_number(ui, "", &mut options.quality, 0.0..=100.0);
    });
    ui.horizontal(|ui| {
        ui.label("distance");
        optional_number(ui, "", &mut options.distance, 0.0..=25.0);
    });
    ui.horizontal(|ui| {
        ui.label("effort");
        ui.add(egui::DragValue::new(&mut options.effort).range(1..=10));
    });
    ui.horizontal(|ui| {
        ui.label("decoding speed");
        ui.add(egui::DragValue::new(&mut options.decoding_speed).range(0..=4));
    });
    ui.horizontal(|ui| {
        ui.label("intensity target (nits)");
        optional_number(ui, "", &mut options.intensity_target, 0.0..=10_000.0);
    });
    ui.checkbox(&mut options.container, "force container")
        .on_hover_text("force the ISOBMFF container (auto-enabled for EXIF boxes)");
    ui.checkbox(&mut options.original_profile, "original profile")
        .on_hover_text("keep the original color profile (skip the XYB transform)");
    ui.horizontal(|ui| {
        ui.label("bit depth");
        optional_combo(
            ui,
            "jxl-bit-depth",
            &mut options.bit_depth,
            &[
                (JxlBitDepthChoice::Eight, "8"),
                (JxlBitDepthChoice::Sixteen, "16"),
            ],
        );
    });
    ui.horizontal(|ui| {
        ui.label("color encoding");
        optional_combo(
            ui,
            "jxl-color-encoding",
            &mut options.color_encoding,
            &[
                (JxlColorEncodingChoice::Srgb, "sRGB"),
                (JxlColorEncodingChoice::LinearSrgb, "linear sRGB"),
                (JxlColorEncodingChoice::SrgbLuma, "sRGB (luma)"),
                (JxlColorEncodingChoice::LinearSrgbLuma, "linear sRGB (luma)"),
                (JxlColorEncodingChoice::IccPassthrough, "ICC passthrough"),
            ],
        );
    });

    // advanced `--setting ID=VALUE` passthrough (WS2)
    ui.separator();
    ui.label(egui::RichText::new("Advanced settings (libjxl frame-setting passthrough)").weak());
    let mut remove_at: Option<usize> = None;
    for (index, (id, value)) in options.advanced.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let id_response = ui.add(egui::TextEdit::singleline(id).desired_width(220.0));
            id_response.on_hover_text("setting id (resolved case-insensitively at run time)");
            ui.add(egui::DragValue::new(value));
            if ui.button("✕").clicked() {
                remove_at = Some(index);
            }
        });
    }
    if let Some(index) = remove_at {
        options.advanced.remove(index);
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut draft.new_id)
                .hint_text("new setting id")
                .desired_width(220.0),
        );
        ui.add(egui::DragValue::new(&mut draft.new_value).range(i64::MIN..=i64::MAX));
        if ui.button("add").clicked() && !draft.new_id.trim().is_empty() {
            options
                .advanced
                .push((draft.new_id.trim().to_string(), draft.new_value));
            draft.new_id.clear();
            draft.new_value = 0;
        }
    });
}

fn oxipng_ui(ui: &mut egui::Ui, options: &mut OxipngOptions) {
    ui.horizontal(|ui| {
        ui.label("level");
        combo(
            ui,
            "oxipng-level",
            &mut options.level,
            &[
                (OxipngLevel::Zero, "0 (fastest)"),
                (OxipngLevel::One, "1"),
                (OxipngLevel::Two, "2 (default)"),
                (OxipngLevel::Three, "3"),
                (OxipngLevel::Four, "4"),
                (OxipngLevel::Five, "5"),
                (OxipngLevel::Six, "6"),
                (OxipngLevel::Max, "max"),
            ],
        );
    });
    ui.checkbox(&mut options.zopfli, "zopfli")
        .on_hover_text("zopfli DEFLATE instead of libdeflate (much slower, slightly smaller)");
    ui.add_enabled_ui(options.zopfli, |ui| {
        ui.horizontal(|ui| {
            ui.label("zopfli iterations");
            ui.add(egui::DragValue::new(&mut options.zopfli_iterations).range(1..=1000));
        });
    });
    ui.horizontal(|ui| {
        ui.label("interlace");
        combo(
            ui,
            "oxipng-interlace",
            &mut options.interlace,
            &[
                (OxipngInterlace::Keep, "keep"),
                (OxipngInterlace::None, "none"),
                (OxipngInterlace::Adam7, "adam7"),
            ],
        );
    });
    ui.horizontal(|ui| {
        ui.label("strip");
        combo(
            ui,
            "oxipng-strip",
            &mut options.strip,
            &[
                (OxipngStrip::None, "none"),
                (OxipngStrip::Safe, "safe"),
                (OxipngStrip::All, "all"),
            ],
        )
        .response
        .on_hover_text(
            "the EXIF policy may adjust stripping at run time (keep policy forces stripping off)",
        );
    });
    ui.checkbox(&mut options.strip_explicit, "strip explicitly set")
        .on_hover_text("mirrors passing --strip on the CLI (drives the EXIF interplay)");
    ui.label("filters (empty = level preset)");
    multi_select(
        ui,
        &mut options.filters,
        &[
            (OxipngFilter::None, "none"),
            (OxipngFilter::Sub, "sub"),
            (OxipngFilter::Up, "up"),
            (OxipngFilter::Avg, "avg"),
            (OxipngFilter::Paeth, "paeth"),
            (OxipngFilter::MinSum, "min-sum"),
            (OxipngFilter::Entropy, "entropy"),
            (OxipngFilter::Bigrams, "bigrams"),
            (OxipngFilter::BigEnt, "big-entropy"),
            (OxipngFilter::Brute, "brute"),
        ],
    );
    ui.label("disable reductions");
    multi_select(
        ui,
        &mut options.no_reduction,
        &[
            (OxipngReduction::BitDepth, "bit depth"),
            (OxipngReduction::ColorType, "color type"),
            (OxipngReduction::Palette, "palette"),
            (OxipngReduction::Grayscale, "grayscale"),
        ],
    );
    ui.checkbox(&mut options.optimize_alpha, "optimize alpha");
    ui.checkbox(&mut options.scale_16, "scale 16-bit → 8-bit")
        .on_hover_text("potentially pixel-altering");
    ui.checkbox(&mut options.fix_errors, "fix errors")
        .on_hover_text("attempt fixing broken input PNGs");
    ui.horizontal(|ui| {
        ui.label("timeout");
        let mut secs = options.timeout.map_or(0, |timeout| timeout.as_secs());
        ui.add(
            egui::DragValue::new(&mut secs)
                .range(0..=86_400)
                .suffix("s"),
        )
        .on_hover_text("per-file optimization time limit; 0 = unlimited");
        options.timeout = (secs > 0).then(|| std::time::Duration::from_secs(secs));
    });
}

fn webp_anim_ui(ui: &mut egui::Ui, options: &mut WebpAnimOptions) {
    ui.checkbox(&mut options.lossless, "lossless");
    ui.horizontal(|ui| {
        ui.label("quality");
        ui.add(egui::Slider::new(&mut options.quality, 0.0..=100.0))
            .on_hover_text("in lossless mode this is the compression effort");
    });
    ui.horizontal(|ui| {
        ui.label("kmin");
        optional_number(ui, "", &mut options.kmin, 0..=10_000);
    });
    ui.horizontal(|ui| {
        ui.label("kmax");
        optional_number(ui, "", &mut options.kmax, 0..=10_000);
    });
    ui.checkbox(&mut options.minimize_size, "minimize size")
        .on_hover_text("slow; disables keyframe insertion");
    ui.checkbox(
        &mut options.allow_mixed,
        "allow mixed lossy/lossless frames",
    );
    ui.horizontal(|ui| {
        ui.label("method");
        optional_number(ui, "", &mut options.method, 0..=6);
    });
}

fn gif_ui(ui: &mut egui::Ui, options: &mut GifOptions) {
    ui.horizontal(|ui| {
        ui.label("palette speed");
        optional_number(ui, "", &mut options.speed, 1..=30);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_maps_to_a_config_and_kind_name_round_trips() {
        let caps = byteshaver::job::capabilities();
        for info in &caps.encoders {
            let config = default_encoder_config(info.name)
                .unwrap_or_else(|| panic!("no default config for {}", info.name));
            assert_eq!(encoder_kind_name(&config), info.name);
        }
        assert!(default_encoder_config("nope").is_none());
        assert_eq!(
            encoder_kind_name(&default_encoder_config("avif").expect("avif")),
            "avif"
        );
    }

    #[test]
    fn capability_enablement_matches_availability_helpers() {
        let caps = byteshaver::job::capabilities();
        // the default-feature gui build compiles in every optional encoder
        assert!(encoder_enabled(&caps, "webp"));
        assert!(encoder_enabled(&caps, "webp-image"));
        assert!(encoder_enabled(&caps, "avif"));
        assert!(encoder_enabled(&caps, "png"));
        assert!(encoder_enabled(&caps, "jpeg"));
        assert!(encoder_enabled(&caps, "jxl"));
        assert!(encoder_enabled(&caps, "oxipng"));
        assert!(encoder_enabled(&caps, "webp-anim"));
        assert!(encoder_enabled(&caps, "apng"));
        assert!(encoder_enabled(&caps, "gif"));
        assert!(encoder_disabled_reason(&caps, "webp").is_none());
        assert!(encoder_disabled_reason(&caps, "unknown-encoder").is_none());
        for name in ENCODER_NAMES {
            assert!(
                encoder_enabled(&caps, name),
                "encoder {name} should be listed as enabled in a default build"
            );
        }
        assert_eq!(ENCODER_NAMES.len(), caps.encoders.len());
    }

    #[test]
    fn defaults_match_the_cli_subcommand_defaults() {
        use byteshaver::cli::CliArgs;
        use clap::Parser;

        for name in ENCODER_NAMES {
            let expected = default_encoder_config(name).expect("variant exists");
            // build the CLI args for the same subcommand (no options set);
            // the CLI shape is `byteshaver <PATTERN> <SUBCOMMAND>`
            let args = CliArgs::parse_from(["byteshaver", "pattern", name]);
            let from_cli = EncoderConfig::from_args(&args.command)
                .expect("every gui-listed encoder is a conversion subcommand");
            assert_eq!(encoder_kind_name(&from_cli), name);
            assert_eq!(
                from_cli, expected,
                "gui defaults of {name} must match the CLI defaults"
            );
        }
    }
}
