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
    // One line only, cut short with an ellipsis: a wrapped tab title would grow the strip.
    let mut job = egui::text::LayoutJob::simple_singleline(label.to_owned(), egui::TextStyle::Body.resolve(ui.style()), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(available);
    let galley = ui.painter().layout_job(job);
    ui.painter()
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

/// The tab switcher's button, as phone browsers draw it: the number of open tabs in a rounded
/// square. Stands in for the tab strip where there is no room for one.
pub fn tab_count_button(ui: &mut Ui, count: usize) -> Response {
    let size = Vec2::splat(ui.spacing().interact_size.y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let color = if response.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    let square = Rect::from_center_size(rect.center(), Vec2::splat(size.y * 0.6));
    ui.painter()
        .rect_stroke(square, CornerRadius::same(4), Stroke::new(1.5, color), StrokeKind::Inside);
    // Past 99 the number no longer fits the square.
    let label = if count > 99 { "99+".to_owned() } else { count.to_string() };
    ui.painter().text(
        square.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(size.y * 0.32),
        color,
    );
    response.on_hover_text("Tabs")
}

/// What a tab card shows of its page: the page's texture and the page's aspect (width over
/// height), so the card can show the top of the page without squashing it.
pub struct Thumbnail {
    pub texture: egui::TextureId,
    pub aspect: f32,
}

/// One tab in the tab grid: the top of its page, its favicon and title below, a close button
/// in the corner. Painted for the same reason as [`tab`]: the whole card is the click target.
pub fn tab_card(
    ui: &mut Ui,
    title: &str,
    icon: Option<&egui::TextureHandle>,
    thumbnail: Option<Thumbnail>,
    active: bool,
    width: f32,
) -> (Response, bool) {
    let caption = ui.spacing().interact_size.y + 8.0;
    let picture_height = (width * 1.25).round();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, picture_height + caption), Sense::click());
    let visuals = ui.visuals();
    let radius = CornerRadius::same(10);
    ui.painter().rect_filled(rect, radius, visuals.faint_bg_color);

    // The top of the page, at the page's own proportions: as much of it as fits the card.
    let picture = Rect::from_min_size(rect.min, Vec2::new(width, picture_height));
    let picture_radius = CornerRadius {
        nw: 10,
        ne: 10,
        sw: 0,
        se: 0,
    };
    match thumbnail {
        Some(thumbnail) => {
            let visible = ((picture_height / width) * thumbnail.aspect).min(1.0);
            egui::Image::from_texture((thumbnail.texture, picture.size()))
                .uv(Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, visible)))
                .corner_radius(picture_radius)
                .paint_at(ui, picture);
        }
        // Never drawn yet (opened in the background): the page's favicon, or an empty card.
        None => {
            ui.painter().rect_filled(picture, picture_radius, visuals.extreme_bg_color);
            if let Some(icon) = icon {
                let big = Rect::from_center_size(picture.center(), Vec2::splat(48.0));
                ui.painter().image(
                    icon.id(),
                    big,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
    }

    // Caption: favicon and title, one line cut short with an ellipsis.
    let middle = picture.max.y + caption / 2.0;
    let mut cursor = rect.min.x + 10.0;
    if let Some(icon) = icon {
        let icon_box = Rect::from_center_size(egui::pos2(cursor + 8.0, middle), Vec2::splat(16.0));
        ui.painter().image(
            icon.id(),
            icon_box,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        cursor += 24.0;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(
        title.to_owned(),
        egui::TextStyle::Small.resolve(ui.style()),
        visuals.strong_text_color(),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((rect.max.x - 10.0 - cursor).max(0.0));
    let galley = ui.painter().layout_job(job);
    ui.painter()
        .galley(egui::pos2(cursor, middle - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);

    // Close: a round button over the picture's corner, readable on any page.
    let close_box = Rect::from_center_size(picture.right_top() + Vec2::new(-20.0, 20.0), Vec2::splat(28.0));
    let close = ui.interact(close_box, response.id.with("close"), Sense::click());
    ui.painter().circle_filled(
        close_box.center(),
        14.0,
        Color32::from_black_alpha(if close.hovered() { 200 } else { 140 }),
    );
    let (d, c) = (5.0, close_box.center());
    let stroke = Stroke::new(1.5, Color32::WHITE);
    ui.painter().line_segment([c + Vec2::new(-d, -d), c + Vec2::new(d, d)], stroke);
    ui.painter().line_segment([c + Vec2::new(d, -d), c + Vec2::new(-d, d)], stroke);

    if active {
        ui.painter()
            .rect_stroke(rect, radius, Stroke::new(2.5, visuals.selection.stroke.color), StrokeKind::Outside);
    }
    (response, close.clicked())
}

