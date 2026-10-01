//! Persistent viewport host (plan 15 F12): every popup window of the GUI
//! (run report, About, visual-difference inspector, preset save modal and
//! preset manage window) renders in its **own OS viewport**, so it can be
//! dragged outside the main window, resized and maximized.
//!
//! # Creation flash (F12)
//!
//! Creating a winit window on demand paints 1–2 default (background)
//! frames before egui's first paint — the flash the report showed on
//! Windows. The host therefore passes every popup to
//! [`egui::Context::show_viewport_immediate`] **every frame, regardless
//! of its open state**: the OS windows are created once at startup,
//! before the main window is visible to the user, and afterwards only
//! toggled via [`egui::ViewportBuilder::with_visible`] (egui emits
//! `ViewportCommand::Visible` when the flag flips between frames, so the
//! windows are shown/hidden in place — never destroyed). Hidden viewports
//! skip their contents: the render callback only runs a popup's body
//! while it is open.
//!
//! # Fallbacks
//!
//! - When the backend cannot spawn multiple viewports, egui invokes the
//!   callback with [`egui::ViewportClass::Embedded`]; the host then
//!   renders that popup's bounded plain `egui::Window` inside the main
//!   viewport instead (plan 09 doctrine) — and only while it is open.
//! - `with_visible` is a no-op on Wayland (winit cannot hide or remap
//!   windows there), so on Wayland the host degrades to **on-demand**
//!   creation (see [`supports_persistent_viewports`]): a popup's viewport
//!   is only spawned while it is open, exactly like the pre-F12 report.
//!
//! The pure parts (id mapping, titles, default sizes, visibility mapping
//! and geometry application) are unit-tested below; the render callbacks
//! are thin closures over the panels' existing body functions. Title-bar
//! close (`close_requested`) flips each popup's open flag on `App`, which
//! hides the window on the next frame — the same pattern the report
//! established in plan 09.

use crate::app::App;

/// Kind of a hosted popup window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKind {
    /// Per-file results of the last run (plan 09).
    Report,
    /// Core version + build capabilities.
    About,
    /// Visual difference inspector (plan 10 §phase 3).
    Inspector,
    /// "Save current as preset" modal (plan 14 §4).
    PresetSave,
    /// Manage-presets window (plan 14 §4).
    PresetManage,
}

impl PopupKind {
    /// Stable viewport id: re-opening re-renders the same viewport, so
    /// egui reuses the OS window instead of churning a new one per open.
    #[must_use]
    pub fn viewport_id(self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(match self {
            PopupKind::Report => "run-report",
            PopupKind::About => "byteshaver-about",
            PopupKind::Inspector => "byteshaver-inspector",
            PopupKind::PresetSave => "byteshaver-preset-save",
            PopupKind::PresetManage => "byteshaver-preset-manage",
        })
    }

    /// OS window title (the inspector overrides it per compared pair).
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            PopupKind::Report => "byteshaver — run report",
            PopupKind::About => "About byteshaver",
            PopupKind::Inspector => "visual difference",
            PopupKind::PresetSave => "Save current as preset",
            PopupKind::PresetManage => "Manage presets",
        }
    }

    /// Default inner size, used before any persisted geometry applies.
    /// The preset-save modal is sized to its short form (plan 16 F20) —
    /// a taller default would open with a large empty area below it.
    #[must_use]
    pub fn default_size(self) -> [f32; 2] {
        match self {
            PopupKind::Report => [760.0, 480.0],
            PopupKind::About => [560.0, 440.0],
            PopupKind::Inspector => [880.0, 620.0],
            PopupKind::PresetSave => [420.0, 260.0],
            PopupKind::PresetManage => [600.0, 520.0],
        }
    }
}

/// What the host needs to know about one popup this frame: its kind
/// (id/title/default size), whether it is currently open and optional
/// persisted geometry (`[x, y, width, height]`, outer position + inner
/// size — currently only the report persists it).
#[derive(Clone, Debug, PartialEq)]
pub struct PopupSpec {
    /// Which popup this is (stable viewport id + title + default size).
    pub kind: PopupKind,
    /// OS window title override (the inspector names its compared pair).
    pub title: Option<String>,
    /// Whether the popup is open this frame (drives the visibility).
    pub open: bool,
    /// Persisted `[x, y, width, height]` from a previous session, if any.
    pub geometry: Option<[f32; 4]>,
}

impl PopupSpec {
    /// A popup of `kind` with the kind's default title.
    #[must_use]
    pub fn new(kind: PopupKind, open: bool) -> Self {
        PopupSpec {
            kind,
            title: None,
            open,
            geometry: None,
        }
    }

    /// Overrides the OS window title (e.g. the inspector's pair label).
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Restores persisted geometry (`[x, y, width, height]`).
    #[must_use]
    pub fn with_geometry(mut self, geometry: Option<[f32; 4]>) -> Self {
        self.geometry = geometry;
        self
    }

    /// The effective OS window title.
    #[must_use]
    pub fn effective_title(&self) -> &str {
        self.title.as_deref().unwrap_or(self.kind.title())
    }
}

/// Whether this session can hide and re-show OS windows in place.
///
/// Persistent viewports rely on `ViewportCommand::Visible`, which winit
/// implements everywhere **except Wayland** (`set_visible` is a no-op
/// there — a "hidden" window would stay on screen forever). On Wayland
/// the host falls back to creating popups on demand instead.
#[must_use]
pub fn supports_persistent_viewports() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("WAYLAND_SOCKET").is_none()
}

/// Builds the viewport builder of one popup: title, default size,
/// visibility and the geometry persisted from a previous session, if any.
///
/// `persistent` is [`supports_persistent_viewports`]; in the on-demand
/// fallback the window is only created while open, so it is always
/// visible.
#[must_use]
pub fn popup_builder(spec: &PopupSpec, persistent: bool) -> egui::ViewportBuilder {
    let mut builder = egui::ViewportBuilder::default()
        .with_title(spec.effective_title())
        .with_inner_size(spec.kind.default_size())
        .with_visible(spec.open || !persistent);
    if let Some([x, y, width, height]) = spec.geometry {
        builder = builder
            .with_position(egui::Pos2::new(x, y))
            .with_inner_size([width, height]);
    }
    builder
}

/// Hosts one popup for this frame (called every frame, open or not — that
/// is the whole point: the OS window exists from the first frame and is
/// only toggled visible/hidden, which removes the creation flash).
///
/// - `render_embedded`: the popup's bounded `egui::Window` body for the
///   [`egui::ViewportClass::Embedded`] fallback (skipped while closed).
/// - `render_contents`: the popup's own-viewport body (`CentralPanel`,
///   close handling, repaint requests; skipped while closed).
pub fn show_popup(
    app: &mut App,
    ctx: &egui::Context,
    spec: PopupSpec,
    render_embedded: impl FnMut(&mut App, &egui::Context),
    mut render_contents: impl FnMut(&mut App, &egui::Context),
) {
    let persistent = supports_persistent_viewports();
    if !persistent && !spec.open {
        // Wayland fallback: create viewports on demand (no hiding there).
        return;
    }
    let open = spec.open;
    let id = spec.kind.viewport_id();
    let builder = popup_builder(&spec, persistent);
    let mut render_embedded = render_embedded;
    ctx.show_viewport_immediate(id, builder, move |vctx, class| {
        if class == egui::ViewportClass::Embedded {
            if open {
                render_embedded(app, vctx);
            }
        } else if open {
            render_contents(app, vctx);
        }
    });
}

/// Hosts **all** popups for this frame (called once per frame from
/// `panels::show`, after the main-window panels). Each popup keeps its
/// own open flag on `App`; the embedded fallbacks are the panels' plain
/// `egui::Window` renderers.
pub fn show_all(app: &mut App, ctx: &egui::Context) {
    use crate::panels::{about, inspector, options_panel, report};

    // the run report: hidden while a job re-runs (`report` is cleared),
    // re-shown with fresh contents when the run finishes (plan 09)
    let report_open = app.show_report && app.report.is_some();
    let report_geometry = app.settings.report_window_geometry;
    show_popup(
        app,
        ctx,
        PopupSpec::new(PopupKind::Report, report_open).with_geometry(report_geometry),
        report::show_embedded_window,
        report::show_viewport_contents,
    );

    // the visual difference inspector: its state (`Option`) is dropped
    // when closed, freeing buffers and textures
    let inspector_open = app.inspector.is_some();
    let inspector_title = app
        .inspector
        .as_ref()
        .map_or(PopupKind::Inspector.title(), |state| state.title.as_str());
    show_popup(
        app,
        ctx,
        PopupSpec::new(PopupKind::Inspector, inspector_open).with_title(inspector_title),
        inspector::InspectorState::show_embedded_window,
        inspector::InspectorState::show_viewport_contents,
    );

    // About
    let about_open = app.show_about;
    show_popup(
        app,
        ctx,
        PopupSpec::new(PopupKind::About, about_open),
        about::show_embedded_window,
        about::show_viewport_contents,
    );

    // the preset save modal + manage window (plan 14 §4)
    let save_open = app.preset_save.is_some();
    show_popup(
        app,
        ctx,
        PopupSpec::new(PopupKind::PresetSave, save_open),
        options_panel::show_save_window,
        options_panel::show_save_viewport_contents,
    );
    let manage_open = app.show_preset_manager;
    show_popup(
        app,
        ctx,
        PopupSpec::new(PopupKind::PresetManage, manage_open),
        options_panel::show_manage_window,
        options_panel::show_manage_viewport_contents,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_KINDS: [PopupKind; 5] = [
        PopupKind::Report,
        PopupKind::About,
        PopupKind::Inspector,
        PopupKind::PresetSave,
        PopupKind::PresetManage,
    ];

    #[test]
    fn viewport_ids_are_stable_and_unique() {
        for kind in ALL_KINDS {
            assert_eq!(kind.viewport_id(), kind.viewport_id(), "{kind:?} stable");
        }
        for (index, kind) in ALL_KINDS.iter().enumerate() {
            for other in &ALL_KINDS[index + 1..] {
                assert_ne!(
                    kind.viewport_id(),
                    other.viewport_id(),
                    "{kind:?} vs {other:?}"
                );
            }
        }
    }

    #[test]
    fn kinds_carry_distinct_titles_and_sizes() {
        for kind in ALL_KINDS {
            assert!(!kind.title().is_empty());
            assert!(kind.default_size()[0] > 0.0 && kind.default_size()[1] > 0.0);
        }
        assert_eq!(PopupKind::Report.title(), "byteshaver — run report");
        assert_ne!(PopupKind::About.title(), PopupKind::PresetManage.title());
    }

    #[test]
    fn preset_save_default_size_fits_its_short_form() {
        // plan 16 F20: the modal must open close to its form height —
        // tall enough for both fields, the two toggles and a validation
        // line, but without a large empty area below the content.
        let [width, height] = PopupKind::PresetSave.default_size();
        assert_eq!(width, 420.0, "width matches the embedded save window");
        assert!(
            (200.0..=280.0).contains(&height),
            "save modal default height {height} must fit the short form"
        );
    }

    #[test]
    fn spec_title_overrides_the_kind_default() {
        let spec = PopupSpec::new(PopupKind::Inspector, true).with_title("visual difference — pair");
        assert_eq!(spec.effective_title(), "visual difference — pair");
        let plain = PopupSpec::new(PopupKind::About, false);
        assert_eq!(plain.effective_title(), PopupKind::About.title());
    }

    #[test]
    fn builder_maps_visibility_per_mode() {
        let open = PopupSpec::new(PopupKind::About, true);
        let closed = PopupSpec::new(PopupKind::About, false);
        // persistent mode: the window exists from startup and is toggled
        assert!(popup_builder(&open, true).visible.unwrap_or(true));
        assert!(!popup_builder(&closed, true).visible.unwrap_or(true));
        // on-demand fallback (Wayland): only created while open → visible
        assert!(popup_builder(&open, false).visible.unwrap_or(true));
    }

    #[test]
    fn builder_applies_defaults_and_persisted_geometry() {
        let plain = popup_builder(&PopupSpec::new(PopupKind::Report, false), true);
        assert_eq!(
            plain.inner_size,
            Some(egui::vec2(760.0, 480.0)),
            "kind default size without geometry"
        );
        assert_eq!(plain.position, None);

        let spec = PopupSpec::new(PopupKind::Report, true)
            .with_geometry(Some([12.0, 34.0, 800.0, 600.0]));
        let restored = popup_builder(&spec, true);
        assert_eq!(restored.position, Some(egui::Pos2::new(12.0, 34.0)));
        assert_eq!(restored.inner_size, Some(egui::vec2(800.0, 600.0)));
        assert!(restored.visible.unwrap_or(true));
    }

    #[test]
    fn wayland_fallback_keeps_wayland_free_of_persistent_windows() {
        // the probe must react to both Wayland env vars (the pure mapping
        // itself is covered by `builder_maps_visibility_per_mode`)
        let probe = supports_persistent_viewports();
        let on_wayland =
            std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("WAYLAND_SOCKET").is_some();
        assert_eq!(probe, !on_wayland);
    }
}
