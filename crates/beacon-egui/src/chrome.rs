//! The browser chrome, in egui's own idiom.
//!
//! Deliberately not a GTK impersonation. egui has its own visual language — flat panels,
//! its own widget styling, its own spacing scale — and this leans on that rather than
//! hand-painting an imitation of Adwaita. What it borrows from other browsers is
//! *behaviour*: where the tabs are, what the toolbar does, that hovering a link tells you
//! where it goes.
//!
//! The one place we paint by hand is the tab itself, because egui has no tab widget and a
//! `selectable_label` does not read as a tab.

use std::collections::HashMap;

use beacon_core::tab::TabId;
use egui::{Color32, CornerRadius, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};

/// Decoded favicons, keyed by tab. Kept here rather than in core: a texture belongs to a
/// renderer, and core only carries the encoded bytes.
#[derive(Default)]
pub struct Favicons {
    textures: HashMap<TabId, Option<egui::TextureHandle>>,
}

impl Favicons {
    /// The texture for `tab_id`, decoding `bytes` the first time it is seen. A tab whose
    /// icon fails to decode is remembered as `None` so it is not retried every frame.
    pub fn get(&mut self, ctx: &egui::Context, tab_id: TabId, bytes: Option<&[u8]>) -> Option<egui::TextureHandle> {
        let bytes = bytes?;
        if let Some(cached) = self.textures.get(&tab_id) {
            return cached.clone();
        }
        let decoded = decode(bytes).map(|image| ctx.load_texture(format!("favicon-{tab_id}"), image, egui::TextureOptions::LINEAR));
        self.textures.insert(tab_id, decoded.clone());
        decoded
    }

    /// Forget a tab's icon — on close, or when fresh bytes arrive.
    pub fn forget(&mut self, tab_id: TabId) {
        self.textures.remove(&tab_id);
    }
}

/// Favicons are PNG or ICO in practice; `image` guesses from the content.
fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let decoded = image::load_from_memory(bytes).ok()?;
    // 16px is the size a tab shows; scaling here keeps the texture small.
    let decoded = decoded.resize_exact(16, 16, image::imageops::FilterType::Lanczos3).to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([16, 16], decoded.as_raw()))
}

/// What a click on a tab meant.
pub enum TabAction {
    Activate(TabId),
    Close(TabId),
}

/// One tab. Painted rather than composed from widgets: egui has no tab, and this needs an
/// active state that reads as "this panel belongs to me" plus a close button that only
/// appears when it is useful.
#[allow(clippy::too_many_arguments)]
pub fn tab(ui: &mut Ui, label: &str, icon: Option<&egui::TextureHandle>, loading: bool, active: bool, width: f32) -> (Response, bool) {
    let height = ui.spacing().interact_size.y + 8.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let visuals = ui.style().interact_selectable(&response, active);

    // The active tab shares the fill of the panel below it, so the two read as one surface;
    // inactive tabs sit back and only lift on hover.
    let fill = if active {
        ui.visuals().panel_fill
    } else if response.hovered() {
        visuals.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let radius = CornerRadius {
        nw: 6,
        ne: 6,
        sw: 0,
        se: 0,
    };
    ui.painter().rect_filled(rect, radius, fill);
    if active {
        ui.painter()
            .rect_stroke(rect, radius, ui.visuals().widgets.noninteractive.bg_stroke, StrokeKind::Inside);
    }

    let mut cursor = rect.min.x + 8.0;
    let middle = rect.center().y;

    // Icon slot: spinner while loading, favicon once there is one, and nothing otherwise —
    // an empty slot rather than a placeholder glyph, so titles do not shift when it arrives.
    let icon_box = Rect::from_center_size(egui::pos2(cursor + 8.0, middle), Vec2::splat(16.0));
    if loading {
        ui.put(icon_box, egui::Spinner::new().size(12.0));
    } else if let Some(icon) = icon {
        ui.painter().image(
            icon.id(),
            icon_box,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    cursor += 22.0;

    // Close button, but only on the active tab or under the pointer: a row of permanent
    // ✕ marks is noise, and clicking one by accident is worse than an extra hover.
    let show_close = active || response.hovered();
    let close_box = Rect::from_center_size(egui::pos2(rect.max.x - 14.0, middle), Vec2::splat(16.0));
    let mut closed = false;
    if show_close {
        let close = ui.interact(close_box, response.id.with("close"), Sense::click());
        if close.hovered() {
            ui.painter()
                .rect_filled(close_box, CornerRadius::same(4), ui.visuals().widgets.hovered.bg_fill);
        }
        let tint = if close.hovered() {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        let d = 3.5;
        let c = close_box.center();
        let stroke = Stroke::new(1.2, tint);
        ui.painter().line_segment([c + Vec2::new(-d, -d), c + Vec2::new(d, d)], stroke);
        ui.painter().line_segment([c + Vec2::new(d, -d), c + Vec2::new(-d, d)], stroke);
        closed = close.clicked();
    }

    let text_end = if show_close { close_box.min.x - 4.0 } else { rect.max.x - 8.0 };
    let available = (text_end - cursor).max(0.0);
    let color = if active {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    let galley = ui
        .painter()
        .layout(label.to_owned(), egui::TextStyle::Body.resolve(ui.style()), color, available);
    // One line only: a wrapped tab title would grow the strip.
    ui.painter()
        .with_clip_rect(Rect::from_min_max(egui::pos2(cursor, rect.min.y), egui::pos2(text_end, rect.max.y)))
        .galley(egui::pos2(cursor, middle - galley.size().y / 2.0), galley, color);

    (response, closed)
}

/// A toolbar icon button, sized so the row reads as one control strip.
pub fn tool_button(ui: &mut Ui, glyph: &str, tooltip: &str, enabled: bool) -> Response {
    let size = Vec2::splat(ui.spacing().interact_size.y);
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(glyph).size(15.0)).min_size(size).frame(false),
    )
    .on_hover_text(tooltip)
}
