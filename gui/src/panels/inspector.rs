//! Visual difference inspector (plan 10 §phase 3): comparing a converted
//! pair in three modes — side-by-side, swipe (clip-rect split with a
//! draggable handle) and an amplified abs-difference heatmap.
//!
//! Hosted by [`crate::viewports`] (plan 15 F12): an independent OS
//! viewport with an `Embedded` fallback (`egui::Window`, plan 09
//! doctrine). Both shapes render the same body below: the pan/zoom
//! canvas fills the remaining window height (plan 16 F20), so it stays
//! the interaction surface at every window size.
//!
//! Memory guardrails: both files are decoded **once per open** on the
//! metric worker at [`crate::metrics::DISPLAY_MAX_EDGE`] (2048 px longest
//! edge ≈ 16 MiB per RGBA buffer) and handed over as plain RGBA — the UI
//! thread only uploads textures. The amplification slider recomputes the
//! heatmap **on release** (a 2048² pixel loop is a few ms; per-drag
//! recompute would be wasteful), and closing the window drops buffers and
//! textures.
//!
//! Zoom/pan are shared across modes (one `f32` + one offset pair, plan
//! §UI sketch); the swipe split geometry lives in the pure
//! [`crate::metrics::diff`] helpers.

use std::path::{Path, PathBuf};

use egui::TextureHandle;
use image::RgbaImage;

/// Comparison mode of the inspector (not persisted — an inspector is a
/// transient, per-pair surface).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InspectorMode {
    /// Two panes, synchronized zoom/pan (default).
    #[default]
    SideBySide,
    /// One surface, left of the draggable handle = original, right =
    /// converted.
    Swipe,
    /// Amplified abs-difference heatmap.
    Difference,
}

impl InspectorMode {
    /// Tab label of the mode.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            InspectorMode::SideBySide => "side-by-side",
            InspectorMode::Swipe => "swipe",
            InspectorMode::Difference => "difference",
        }
    }

    /// Every mode in tab order.
    pub const ALL: [InspectorMode; 3] = [
        InspectorMode::SideBySide,
        InspectorMode::Swipe,
        InspectorMode::Difference,
    ];
}

/// Zoom bounds (1.0 = zoom-to-fit; the wheel multiplies in these limits).
const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 12.0;

/// Fit scale of an image inside an area (aspect preserved; zero-sized
/// inputs yield 1.0 — no division by zero, textures upload later anyway).
#[must_use]
pub fn fit_scale(area: egui::Vec2, image: [f32; 2]) -> f32 {
    if image[0] <= 0.0 || image[1] <= 0.0 || area.x <= 0.0 || area.y <= 0.0 {
        return 1.0;
    }
    (area.x / image[0]).min(area.y / image[1])
}

/// Displayed rect of an image inside `area` for the shared zoom/pan:
/// zoom-to-fit scaled by `zoom`, centered, offset by `pan` (pure,
/// unit-tested).
#[must_use]
pub fn display_rect(
    area: egui::Rect,
    image_size: [f32; 2],
    zoom: f32,
    pan: egui::Vec2,
) -> egui::Rect {
    let scale = fit_scale(area.size(), image_size) * zoom.max(MIN_ZOOM);
    let size = egui::vec2(image_size[0] * scale, image_size[1] * scale);
    egui::Rect::from_center_size(area.center() + pan, size)
}

/// State of the open inspector (plan 10 §phase 3: `Option` on [`crate::app::App`];
/// `None` = closed, buffers/textures dropped with it).
pub struct InspectorState {
    /// Input path of the pair.
    pub input: PathBuf,
    /// Converted output path.
    pub output: PathBuf,
    /// Window title (`visual difference — a.png → a.webp`).
    pub title: String,
    /// Whether the window is open (close ✕ flips it, then the state is
    /// dropped by the app).
    pub open: bool,
    /// Comparison mode.
    pub mode: InspectorMode,
    /// Shared zoom factor (1.0 = zoom-to-fit).
    pub zoom: f32,
    /// Shared pan offset in points.
    pub pan: egui::Vec2,
    /// Swipe-split fraction `0.0..=1.0` (handle position).
    pub swipe: f32,
    /// Whether the swipe handle is currently dragged.
    dragging_handle: bool,
    /// Slider value (committed to `amp_active` on release).
    pub amp_pending: f32,
    /// Amplification the displayed heatmap was computed with.
    pub amp_active: f32,
    /// Longest edge of the decoded buffers (`None` while decoding, or
    /// after a failed decode).
    pub decoded_edge: Option<u32>,
    /// Whether the decode failed (explanatory caption instead of images).
    pub decode_failed: bool,
    /// CPU buffers (kept for amp-release heatmap recomputes; ≤ 2048 px
    /// each per the feasibility table).
    buffers: Option<(RgbaImage, RgbaImage)>,
    /// Uploaded textures (created once per open, diff updated on amp
    /// release).
    texture_a: Option<TextureHandle>,
    texture_b: Option<TextureHandle>,
    texture_diff: Option<TextureHandle>,
}

impl InspectorState {
    /// Starts an inspector for a converted pair: the decode request is
    /// the caller's job ([`crate::metrics::MetricState::request_inspect`]).
    #[must_use]
    pub fn new(input: PathBuf, output: PathBuf) -> Self {
        let name = |path: &Path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string())
        };
        InspectorState {
            title: format!("visual difference — {} → {}", name(&input), name(&output)),
            input,
            output,
            open: true,
            mode: InspectorMode::SideBySide,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            swipe: 0.5,
            dragging_handle: false,
            amp_pending: 8.0,
            amp_active: 8.0,
            decoded_edge: None,
            decode_failed: false,
            buffers: None,
            texture_a: None,
            texture_b: None,
            texture_diff: None,
        }
    }

    /// Whether the pair's decoded buffers have arrived.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.texture_a.is_some() && self.texture_b.is_some()
    }

    /// Receipt of the worker's bounded buffers: stores them, uploads the
    /// a/b textures and the initial heatmap (called from the app's poll).
    pub fn receive(&mut self, ctx: &egui::Context, a: RgbaImage, b: RgbaImage) {
        self.decoded_edge = Some(a.width().max(a.height()));
        let texture_a = upload(ctx, "inspector-original", &a);
        let texture_b = upload(ctx, "inspector-converted", &b);
        let heat = crate::metrics::diff::diff_heatmap(&a, &b, self.amp_active);
        self.texture_diff = Some(upload(ctx, "inspector-diff", &heat));
        self.texture_a = Some(texture_a);
        self.texture_b = Some(texture_b);
        self.buffers = Some((a, b));
    }

    /// Receipt of a failed decode (missing/unsupported/corrupt file).
    pub fn receive_failed(&mut self) {
        self.decode_failed = true;
    }

    /// Whether this state matches the given pair (route poll results).
    #[must_use]
    pub fn matches(&self, input: &Path, output: &Path) -> bool {
        self.input == input && self.output == output
    }

    /// Recomputes the heatmap from the kept buffers at the active
    /// amplification (called on slider release; no-op without buffers).
    pub fn recompute_diff(&mut self, ctx: &egui::Context) {
        let Some((a, b)) = &self.buffers else {
            return;
        };
        let heat = crate::metrics::diff::diff_heatmap(a, b, self.amp_active);
        self.texture_diff = Some(upload(ctx, "inspector-diff", &heat));
    }

    /// Renders the embedded fallback window (`ViewportClass::Embedded`
    /// backends, plan 09 doctrine). Dropping the state (window closed)
    /// frees buffers and textures.
    pub fn show_embedded_window(app: &mut crate::app::App, ctx: &egui::Context) {
        let Some(state) = &mut app.inspector else {
            return;
        };
        // cloned out before the mutable state borrow (metric caption of
        // this pair, if already measured)
        let metric_caption = app.metrics.cached(&state.input).map(|entry| {
            format!("{} — {}", entry.result.pretty, entry.result.interpretation())
        });
        let mut open = state.open;
        let title = state.title.clone();
        egui::Window::new(egui::RichText::new(title).small())
            .open(&mut open)
            .resizable(true)
            .default_width(crate::viewports::PopupKind::Inspector.default_size()[0])
            .default_height(crate::viewports::PopupKind::Inspector.default_size()[1])
            .show(ctx, |ui| {
                let state = app.inspector.as_mut().expect("checked above");
                body(ui, state, metric_caption.as_deref());
            });
        if !open {
            app.inspector = None; // textures + buffers dropped here
        }
    }

    /// Renders the contents of the standalone viewport (called by
    /// [`crate::viewports`] while open): the inspector body in a
    /// `CentralPanel` of the viewport's own context, plus the OS-title-bar
    /// close handling (closing drops buffers and textures).
    pub fn show_viewport_contents(app: &mut crate::app::App, ctx: &egui::Context) {
        let Some(state) = &mut app.inspector else {
            return;
        };
        if ctx.input(|input| input.viewport().close_requested()) {
            app.inspector = None; // textures + buffers dropped here
            return;
        }
        // cloned out before the mutable state borrow (metric caption of
        // this pair, if already measured)
        let metric_caption = app.metrics.cached(&state.input).map(|entry| {
            format!("{} — {}", entry.result.pretty, entry.result.interpretation())
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            let state = app.inspector.as_mut().expect("checked above");
            body(ui, state, metric_caption.as_deref());
        });
    }
}

/// Uploads an RGBA buffer as a linear-filtered texture.
fn upload(ctx: &egui::Context, name: &str, image: &RgbaImage) -> TextureHandle {
    let size = [image.width() as usize, image.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    ctx.load_texture(name, color, egui::TextureOptions::LINEAR)
}

/// The window body: controls row, caption, canvas filling the remaining
/// height.
fn body(ui: &mut egui::Ui, state: &mut InspectorState, metric_caption: Option<&str>) {
    // controls ------------------------------------------------------------
    ui.horizontal(|ui| {
        for mode in InspectorMode::ALL {
            if ui
                .selectable_label(state.mode == mode, mode.label())
                .clicked()
            {
                state.mode = mode;
            }
        }
        ui.separator();
        if ui.button("fit").on_hover_text("zoom to fit").clicked() {
            state.zoom = 1.0;
            state.pan = egui::Vec2::ZERO;
        }
        if ui
            .button("zoom −")
            .on_hover_text("zoom out")
            .clicked()
        {
            state.zoom = (state.zoom / 1.25).max(MIN_ZOOM);
        }
        if ui
            .button("zoom +")
            .on_hover_text("zoom in")
            .clicked()
        {
            state.zoom = (state.zoom * 1.25).min(MAX_ZOOM);
        }
        // amplification (difference mode only; recompute on release)
        if state.mode == InspectorMode::Difference {
            ui.separator();
            let slider = ui.add_sized(
                [180.0, 18.0],
                egui::Slider::new(&mut state.amp_pending, 1.0..=32.0)
                    .logarithmic(true)
                    .text("amplification"),
            );
            if slider.drag_stopped() {
                state.amp_active = state.amp_pending;
                state.recompute_diff(ui.ctx());
            }
            ui.weak(format!("×{}", state.amp_active as u32));
        }
    });
    ui.separator();

    // caption row: decode bound + measured quality (if any)
    ui.horizontal(|ui| {
        match state.decoded_edge {
            Some(edge) => ui
                .weak(format!("decoded at {edge} px"))
                .on_hover_text(
                    "both files are decoded bounded (2048 px longest edge) to keep \
                     memory sane; fine-texture differences may be understated",
                ),
            None if state.decode_failed => ui.colored_label(
                ui.visuals().error_fg_color,
                "could not decode this pair (unsupported or unreadable file)",
            ),
            None => ui.weak("decoding…"),
        };
        if let Some(metric) = metric_caption {
            ui.separator();
            ui.weak(metric);
        }
    });
    ui.add_space(4.0);

    // canvas (plan 16 F20: no fixed cap — the controls/caption rows are
    // laid out first, so `available_height` *is* the remaining space; the
    // canvas fills it exactly and is the pan/zoom surface everywhere it
    // is drawn)
    let canvas_height = ui.available_height();
    let (canvas, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), canvas_height), egui::Sense::drag());
    let painter = ui.painter_at(canvas);
    if state.decode_failed {
        painter.rect_filled(canvas, 4.0, ui.visuals().extreme_bg_color);
        return;
    }
    painter.rect_filled(canvas, 4.0, ui.visuals().extreme_bg_color);
    if !state.is_ready() {
        return; // decoding… (caption already says so)
    }
    let texture_a = state.texture_a.as_ref().expect("ready").clone();
    let texture_b = state.texture_b.as_ref().expect("ready").clone();

    // interactions: wheel/trackpad zoom + drag pan (swipe handle drag
    // takes precedence when it grabbed the handle)
    if response.hovered() {
        let zoom_delta = ui.input(|input| input.zoom_delta());
        if zoom_delta != 1.0 {
            state.zoom = (state.zoom * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
        }
    }
    let drag_delta = response.drag_delta();
    let handle_grabbed = state.mode == InspectorMode::Swipe && {
        let split = crate::metrics::diff::swipe_split(canvas.width(), state.swipe);
        ui.input(|input| {
            input
                .pointer
                .latest_pos()
                .is_some_and(|pos| crate::metrics::diff::handle_grab(pos.x - canvas.left(), split))
        })
    };
    if state.mode == InspectorMode::Swipe {
        if response.drag_started() && handle_grabbed {
            state.dragging_handle = true;
        }
        if state.dragging_handle && response.dragged()
            && let Some(pos) = ui.input(|input| input.pointer.latest_pos())
        {
            state.swipe =
                crate::metrics::diff::swipe_fraction(canvas.width(), pos.x - canvas.left());
        }
        if response.drag_stopped() {
            state.dragging_handle = false;
        }
    }
    if !state.dragging_handle && response.dragged_by(egui::PointerButton::Primary) {
        state.pan += drag_delta;
    }

    match state.mode {
        InspectorMode::SideBySide => {
            let gap = 6.0;
            let pane_width = ((canvas.width() - gap) * 0.5).max(40.0);
            let pane_a = egui::Rect::from_min_size(
                canvas.min,
                egui::vec2(pane_width, canvas.height()),
            );
            let pane_b = egui::Rect::from_min_max(
                egui::pos2(canvas.left() + pane_width + gap, canvas.top()),
                canvas.max,
            );
            let (a_size, b_size) = (
                [texture_a.size()[0] as f32, texture_a.size()[1] as f32],
                [texture_b.size()[0] as f32, texture_b.size()[1] as f32],
            );
            let clipped_a = painter.with_clip_rect(pane_a);
            clipped_a.image(
                texture_a.id(),
                display_rect(pane_a, a_size, state.zoom, state.pan),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            let clipped_b = painter.with_clip_rect(pane_b);
            clipped_b.image(
                texture_b.id(),
                display_rect(pane_b, b_size, state.zoom, state.pan),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            pane_captions(&painter, &[("original", pane_a), ("converted", pane_b)]);
        }
        InspectorMode::Swipe => {
            let split = crate::metrics::diff::swipe_split(canvas.width(), state.swipe);
            let (a_size, b_size) = (
                [texture_a.size()[0] as f32, texture_a.size()[1] as f32],
                [texture_b.size()[0] as f32, texture_b.size()[1] as f32],
            );
            let rect_a = display_rect(canvas, a_size, state.zoom, state.pan);
            let rect_b = display_rect(canvas, b_size, state.zoom, state.pan);
            // left of the split = original, right = converted
            let left_clip = egui::Rect::from_min_max(canvas.min, egui::pos2(canvas.left() + split, canvas.bottom()));
            let right_clip = egui::Rect::from_min_max(egui::pos2(canvas.left() + split, canvas.top()), canvas.max);
            let clipped_left = painter.with_clip_rect(left_clip);
            clipped_left.image(
                texture_a.id(),
                rect_a,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            let clipped_right = painter.with_clip_rect(right_clip);
            clipped_right.image(
                texture_b.id(),
                rect_b,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            // the handle: a vertical line plus a generous grabber circle
            let x = canvas.left() + split;
            painter.line_segment(
                [egui::pos2(x, canvas.top()), egui::pos2(x, canvas.bottom())],
                egui::Stroke::new(2.0_f32, ui.visuals().strong_text_color()),
            );
            painter.circle_filled(
                egui::pos2(x, canvas.center().y),
                crate::metrics::diff::HANDLE_RADIUS.min(9.0),
                ui.visuals().strong_text_color(),
            );
            painter.circle_filled(
                egui::pos2(x, canvas.center().y),
                3.0,
                ui.visuals().extreme_bg_color,
            );
        }
        InspectorMode::Difference => {
            let Some(texture_diff) = state.texture_diff.clone() else {
                return;
            };
            let size = [
                texture_diff.size()[0] as f32,
                texture_diff.size()[1] as f32,
            ];
            painter.image(
                texture_diff.id(),
                display_rect(canvas, size, state.zoom, state.pan),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }
}

/// Small weak captions in the top-left corner of each pane (side-by-side).
fn pane_captions(painter: &egui::Painter, panes: &[(&str, egui::Rect)]) {
    for (label, rect) in panes {
        painter.text(
            rect.left_top() + egui::vec2(6.0, 6.0),
            egui::Align2::LEFT_TOP,
            *label,
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_scale_preserves_aspect_and_never_divides_by_zero() {
        assert!((fit_scale(egui::vec2(400.0, 200.0), [800.0, 400.0]) - 0.5).abs() < 1e-6);
        assert!((fit_scale(egui::vec2(200.0, 400.0), [800.0, 400.0]) - 0.25).abs() < 1e-6);
        // small images scale *up* to fill the pane (fit, not 1:1)
        assert!((fit_scale(egui::vec2(400.0, 400.0), [100.0, 50.0]) - 4.0).abs() < 1e-6);
        // degenerate inputs are safe
        assert_eq!(fit_scale(egui::vec2(0.0, 100.0), [10.0, 10.0]), 1.0);
        assert_eq!(fit_scale(egui::vec2(100.0, 100.0), [0.0, 0.0]), 1.0);
    }

    #[test]
    fn display_rect_centers_and_pans() {
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 200.0));
        // 800×400 image in a 400×200 area, zoom 1 → exactly filling
        let rect = display_rect(area, [800.0, 400.0], 1.0, egui::Vec2::ZERO);
        assert_eq!(rect, area);
        // pan shifts, zoom scales around the center
        let panned = display_rect(area, [800.0, 400.0], 1.0, egui::vec2(10.0, -5.0));
        assert_eq!(panned.min, egui::pos2(10.0, -5.0));
        let zoomed = display_rect(area, [800.0, 400.0], 2.0, egui::Vec2::ZERO);
        assert_eq!(zoomed.size(), egui::vec2(800.0, 400.0));
        assert_eq!(zoomed.center(), area.center(), "zoom is center-anchored");
    }

    #[test]
    fn inspector_state_defaults() {
        let state = InspectorState::new(
            PathBuf::from("/x/IMG.png"),
            PathBuf::from("/x/out/IMG.webp"),
        );
        assert_eq!(state.mode, InspectorMode::SideBySide);
        assert_eq!(state.zoom, 1.0);
        assert_eq!(state.swipe, 0.5);
        assert_eq!(state.amp_active, 8.0);
        assert!(!state.is_ready());
        assert_eq!(state.title, "visual difference — IMG.png → IMG.webp");
        assert!(state.matches(Path::new("/x/IMG.png"), Path::new("/x/out/IMG.webp")));
        assert!(!state.matches(Path::new("/x/other.png"), Path::new("/x/out/IMG.webp")));
        // a failed decode flips the caption, never panics
        let mut failed = InspectorState::new(PathBuf::from("/x/a.png"), PathBuf::from("/x/b.webp"));
        failed.receive_failed();
        assert!(failed.decode_failed);
        assert!(!failed.is_ready());
    }

    #[test]
    fn mode_labels_cover_all_modes() {
        assert_eq!(InspectorMode::ALL.len(), 3);
        for mode in InspectorMode::ALL {
            assert!(!mode.label().is_empty());
        }
    }
}
