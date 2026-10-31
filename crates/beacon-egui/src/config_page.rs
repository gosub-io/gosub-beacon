//! `gosub://config` under egui: the engine's settings, editable.
//!
//! The engine's own page at that address is a read-only dump (it cannot do forms), so the
//! frontend draws this in its place, as GTK does with widgets. What an edit means -- text
//! typed by the key's schema, a default dropping the override, a value outside the key's
//! constraint refused -- is `beacon_core::settings`', not this file's.
//!
//! A switch for a boolean, a list for a key with fixed choices, and a text field for the
//! rest. Numbers are typed rather than dragged: dragging a value is no way to set a port on
//! a phone. Some settings are read once at engine start (`net.*`), so a change may only take
//! effect after a restart, which the page says.

use std::collections::HashMap;

use beacon_core::settings::{self, SettingRow};
use gosub_engine::{Config, Constraint, Setting};

/// Text sizes, in points. A phone needs them big enough to read and hit with a finger.
const TEXT: f32 = if cfg!(target_os = "android") { 16.0 } else { 13.0 };
const SMALL: f32 = if cfg!(target_os = "android") { 13.0 } else { 11.0 };
/// The height of a control, in points: a finger's width on a phone.
const CONTROL: f32 = if cfg!(target_os = "android") { 40.0 } else { 22.0 };

/// What a row asked for this frame.
enum Edit {
    Value(Setting),
    Text(String),
    Reset,
}

/// The editor in one tab, for as long as the tab shows `gosub://config`.
pub struct ConfigView {
    config: Config,
    filter: String,
    /// The rows `filter` picks, read again only when the filter or a value changes or the
    /// page comes back into view: reading
    /// them walks the whole store, which is no thing to do every frame.
    rows: Vec<SettingRow>,
    /// What is being typed into a field, by key. It reaches the store when the field is left
    /// (Enter, a tap elsewhere), not on every key.
    drafts: HashMap<String, String>,
    /// The last edit the store would not take, shown rather than pretending it landed.
    refused: Option<String>,
    /// The frame this was last shown in. Shown again after a gap (another tab, another
    /// page), it reads the rows afresh: another tab's editor may have changed them.
    shown: u64,
    /// How tall the list was last frame. On a phone it shrinks as the keyboard slides in,
    /// and a field being typed into is scrolled back into view each time it does.
    visible: f32,
}

impl ConfigView {
    pub fn new(config: Config) -> Self {
        let rows = settings::rows(&config, "");
        Self {
            config,
            filter: String::new(),
            rows,
            drafts: HashMap::new(),
            refused: None,
            shown: 0,
            visible: 0.0,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        let frame = ui.ctx().cumulative_frame_nr();
        if frame > self.shown + 1 {
            self.rows = settings::rows(&self.config, &self.filter);
        }
        self.shown = frame;
        ui.spacing_mut().interact_size.y = CONTROL;
        if cfg!(target_os = "android") {
            ui.spacing_mut().icon_width = 24.0;
            ui.spacing_mut().icon_width_inner = 14.0;
        }
        let mut edit = None;
        egui::Frame::default().inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Advanced settings").size(TEXT * 1.4).strong());
                ui.add_space(12.0);
                let filter = text_field(ui, &mut self.filter, "Filter settings", ui.available_width());
                if filter.changed() {
                    self.rows = settings::rows(&self.config, &self.filter);
                }
            });
            ui.label(
                egui::RichText::new(
                    "Changes apply to the running engine; settings read once at startup (net.*) take effect after a restart.",
                )
                .size(SMALL)
                .weak(),
            );
            if let Some(refused) = &self.refused {
                ui.label(egui::RichText::new(refused).size(SMALL).color(ui.visuals().error_fg_color));
            }
            ui.add_space(8.0);

            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let visible = ui.clip_rect().height();
                let resized = (visible - self.visible).abs() > 0.5;
                self.visible = visible;
                if self.rows.is_empty() {
                    ui.label(egui::RichText::new("No setting matches the filter.").size(TEXT).weak());
                }
                for row in &self.rows {
                    ui.separator();
                    if let Some(asked) = show_row(ui, row, &mut self.drafts, resized) {
                        edit = Some((row.info.key.clone(), asked));
                    }
                }
            });
        });

        if let Some((key, edit)) = edit {
            let stored = match edit {
                Edit::Value(value) => settings::set(&self.config, &key, value),
                Edit::Text(text) => settings::set_text(&self.config, &key, &text),
                Edit::Reset => settings::reset(&self.config, &key),
            };
            self.refused = (!stored).then(|| format!("{key} was not changed: the value is not one it takes."));
            self.rows = settings::rows(&self.config, &self.filter);
        }
    }
}

/// One setting: its key on a line of its own (bold once it differs from the default), its
/// type, the values it takes and its description under that, then the editor and reset.
///
/// The key gets the whole width because it is the one thing that tells two rows apart:
/// `dns.cache.ttl.override.enabled` and `.seconds` cut short to fit beside a type and a
/// button read the same on a phone. `resized` says the list changed height since the last
/// frame, as it does while the phone's keyboard slides in over it.
fn show_row(ui: &mut egui::Ui, row: &SettingRow, drafts: &mut HashMap<String, String>, resized: bool) -> Option<Edit> {
    let key = &row.info.key;
    let modified = row.is_modified();
    let mut edit = None;

    let text = egui::RichText::new(key).size(TEXT).monospace();
    let text = if modified { text.strong() } else { text };
    ui.add(egui::Label::new(text).wrap());

    let mut about = row.info.default.type_name().to_string();
    if let Some(constraint) = row
        .constraint()
        .filter(|_| !matches!(row.info.constraint, Some(Constraint::Enum(_))))
    {
        about = format!("{about}, {constraint}");
    }
    // One label, so the description wraps on from the type like a line of text.
    let mut line = egui::text::LayoutJob::default();
    let (style, font, align) = (ui.style(), egui::FontSelection::Default, egui::Align::Center);
    egui::RichText::new(format!("{about}  "))
        .size(SMALL)
        .monospace()
        .weak()
        .append_to(&mut line, style, font.clone(), align);
    egui::RichText::new(&row.info.description)
        .size(SMALL)
        .weak()
        .append_to(&mut line, style, font, align);
    ui.add(egui::Label::new(line).wrap());

    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let reset = egui::Button::new(egui::RichText::new("Reset").size(SMALL));
            if ui.add_enabled(modified, reset).on_hover_text("Back to the default").clicked() {
                edit = Some(Edit::Reset);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                // A tap on Reset also takes the focus from the field, which then hands in
                // what was typed into it; the Reset is what was asked for.
                let asked = show_editor(ui, row, drafts, resized);
                edit = edit.take().or(asked);
            });
        });
    });
    edit
}

/// The control that changes `row`'s value: a check box, a list of its choices, or a field.
fn show_editor(ui: &mut egui::Ui, row: &SettingRow, drafts: &mut HashMap<String, String>, resized: bool) -> Option<Edit> {
    let key = &row.info.key;
    let mut edit = None;
    match (&row.info.default, &row.info.constraint) {
        (Setting::Bool(_), _) => {
            let mut on = row.current.to_bool();
            let label = egui::RichText::new(if on { "on" } else { "off" }).size(TEXT);
            if ui.checkbox(&mut on, label).changed() {
                edit = Some(Edit::Value(Setting::Bool(on)));
            }
        }
        (_, Some(Constraint::Enum(options))) => {
            let current = row.current.value_string();
            egui::ComboBox::from_id_salt(key)
                .selected_text(egui::RichText::new(&current).size(TEXT))
                .show_ui(ui, |ui| {
                    for option in options {
                        let label = egui::RichText::new(option).size(TEXT);
                        if ui.selectable_label(*option == current, label).clicked() && *option != current {
                            edit = Some(Edit::Text(option.clone()));
                        }
                    }
                });
        }
        _ => {
            let mut text = drafts.get(key).cloned().unwrap_or_else(|| row.current.value_string());
            let width = ui.available_width().min(360.0);
            let field = text_field(ui, &mut text, "", width);
            if field.lost_focus() {
                drafts.remove(key);
                if text != row.current.value_string() {
                    edit = Some(Edit::Text(text));
                }
            } else if field.has_focus() {
                drafts.insert(key.clone(), text);
                // Keep it above the keyboard, which covers the bottom half of a phone.
                if field.gained_focus() || resized {
                    field.scroll_to_me(Some(egui::Align::Center));
                }
            }
        }
    }
    edit
}

/// A one-line text field `width` wide, which raises the phone's keyboard while it has focus.
fn text_field(ui: &mut egui::Ui, text: &mut String, hint: &str, width: f32) -> egui::Response {
    let field = ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .font(egui::FontId::proportional(TEXT))
            .desired_width(width)
            .min_size(egui::vec2(0.0, CONTROL))
            .vertical_align(egui::Align::Center),
    );
    #[cfg(target_os = "android")]
    crate::android::keyboard_for(&field);
    field
}
