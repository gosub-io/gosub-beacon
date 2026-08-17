//! `gosub://config` — an `about:config`-style editor for the engine's settings store.
//!
//! Shell-rendered (GTK widgets) because the engine cannot do forms yet. Each row shows
//! a setting's key, type, current value and description; the value cell is an editor
//! matched to the setting's type/constraint (switch, dropdown, spin button or entry).
//! Edits go straight into the store; the row's key turns bold once its value differs
//! from the schema default, and a reset button restores the default. Note that some
//! settings are read once at engine start (e.g. `net.*` into the fetcher), so a change
//! may only take effect after a restart.

use gosub_engine::{Config, Constraint, Setting, SettingInfo};
use gtk4::prelude::*;
use gtk4::{glib, Widget};
use std::rc::Rc;

/// Build the config page for `config`. Rows are (re)built on every filter change so the
/// list always reflects the store.
pub fn build(config: Config) -> Widget {
    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    page.set_hexpand(true);
    page.set_vexpand(true);
    page.add_css_class("config-page");

    // Header: title + search.
    let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    header.add_css_class("config-header");
    let title = gtk4::Label::new(Some("Engine settings"));
    title.add_css_class("config-title");
    title.set_halign(gtk4::Align::Start);
    header.append(&title);
    let search = gtk4::SearchEntry::new();
    search.set_placeholder_text(Some("Filter settings"));
    search.set_hexpand(true);
    header.append(&search);
    page.append(&header);

    let notice = gtk4::Label::new(Some(
        "Changes apply to the running engine; settings read once at startup (net.*) take effect after a restart.",
    ));
    notice.add_css_class("config-notice");
    notice.set_halign(gtk4::Align::Start);
    notice.set_wrap(true);
    page.append(&notice);

    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    list.add_css_class("config-list");
    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .child(&list)
        .build();
    page.append(&scroller);

    populate(&list, &config, "");
    search.connect_search_changed({
        let list = list.clone();
        let config = config.clone();
        move |entry| populate(&list, &config, entry.text().as_str())
    });

    page.upcast::<Widget>()
}

/// Rebuild the rows: every key when `filter` is empty, otherwise keys containing
/// `filter` (case-insensitive substring; a `*` makes it a wildcard pattern instead).
fn populate(list: &gtk4::ListBox, config: &Config, filter: &str) {
    while let Some(row) = list.first_child() {
        list.remove(&row);
    }

    let filter = filter.trim().to_lowercase();
    let mut keys = if filter.contains('*') {
        config.find(&filter)
    } else {
        config.find("*")
    };
    if !filter.is_empty() && !filter.contains('*') {
        keys.retain(|k| k.to_lowercase().contains(&filter));
    }
    keys.sort();

    for key in keys {
        let Some(info) = config.get_info(&key) else { continue };
        let current = config.get(&key).ok().flatten().unwrap_or_else(|| info.default.clone());
        list.append(&build_row(config.clone(), info, current));
    }
}

fn build_row(config: Config, info: SettingInfo, current: Setting) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.set_activatable(false);
    let grid = gtk4::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(2);
    grid.add_css_class("config-row");

    let key_label = gtk4::Label::new(Some(&info.key));
    key_label.set_halign(gtk4::Align::Start);
    key_label.set_hexpand(true);
    key_label.set_selectable(true);
    key_label.add_css_class("config-key");
    if current != info.default {
        key_label.add_css_class("modified");
    }
    grid.attach(&key_label, 0, 0, 1, 1);

    let type_label = gtk4::Label::new(Some(info.default.type_name()));
    type_label.add_css_class("config-type");
    type_label.set_halign(gtk4::Align::End);
    grid.attach(&type_label, 1, 0, 1, 1);

    let reset = gtk4::Button::from_icon_name("edit-undo-symbolic");
    reset.set_has_frame(false);
    reset.set_tooltip_text(Some("Reset to default"));
    reset.set_sensitive(current != info.default);
    reset.add_css_class("config-reset");
    grid.attach(&reset, 3, 0, 1, 1);

    let desc = gtk4::Label::new(Some(&info.description));
    desc.set_halign(gtk4::Align::Start);
    desc.set_wrap(true);
    desc.set_xalign(0.0);
    desc.add_css_class("config-desc");
    grid.attach(&desc, 0, 1, 2, 1);

    // The value editor + the closure that pushes an edit into the store and updates the
    // row's modified/reset state. Setting a value equal to the default removes the
    // override instead, so the persisted store only ever holds real customizations.
    let key = info.key.clone();
    let default = info.default.clone();
    let on_change: Rc<dyn Fn(Setting)> = Rc::new({
        let config = config.clone();
        let key_label = key_label.clone();
        let reset = reset.clone();
        let default = default.clone();
        let key = key.clone();
        move |value: Setting| {
            let modified = value != default;
            let result = if modified { config.set(&key, value) } else { config.remove(&key) };
            match result {
                Ok(()) => {
                    if modified {
                        key_label.add_css_class("modified");
                    } else {
                        key_label.remove_css_class("modified");
                    }
                    reset.set_sensitive(modified);
                }
                Err(e) => log::warn!("gosub://config: update {key} failed: {e:?}"),
            }
        }
    });

    let (editor, set_editor_value) = build_editor(&info, &current, on_change.clone());
    editor.set_valign(gtk4::Align::Center);
    grid.attach(&editor, 2, 0, 1, 2);

    reset.connect_clicked({
        let default = default.clone();
        move |_| {
            set_editor_value(&default);
            on_change(default.clone());
        }
    });

    row.set_child(Some(&grid));
    row
}

/// Pushes a value into an editor widget without firing its change handler (used by reset).
type EditorSetter = Box<dyn Fn(&Setting)>;

/// An editor widget for `info`'s type/constraint, plus a setter that updates the widget
/// without triggering `on_change` (used by reset).
fn build_editor(info: &SettingInfo, current: &Setting, on_change: Rc<dyn Fn(Setting)>) -> (Widget, EditorSetter) {
    // Guard against the setter re-entering on_change through the widget's own signal.
    let suppress = Rc::new(std::cell::Cell::new(false));

    match (&info.default, &info.constraint) {
        (Setting::Bool(_), _) => {
            let sw = gtk4::Switch::new();
            sw.set_active(current.to_bool());
            sw.connect_state_set({
                let suppress = suppress.clone();
                move |_, on| {
                    if !suppress.get() {
                        on_change(Setting::Bool(on));
                    }
                    glib::Propagation::Proceed
                }
            });
            let setter = {
                let sw = sw.clone();
                Box::new(move |v: &Setting| {
                    suppress.set(true);
                    sw.set_active(v.to_bool());
                    suppress.set(false);
                })
            };
            (sw.upcast(), setter)
        }
        (_, Some(Constraint::Enum(options))) => {
            let opts: Vec<&str> = options.iter().map(String::as_str).collect();
            let dropdown = gtk4::DropDown::from_strings(&opts);
            let select = |dd: &gtk4::DropDown, v: &Setting| {
                let cur = v.value_string();
                if let Some(i) = options.iter().position(|o| *o == cur) {
                    dd.set_selected(i as u32);
                }
            };
            select(&dropdown, current);
            dropdown.connect_selected_notify({
                let suppress = suppress.clone();
                let options = options.clone();
                let template = info.default.clone();
                move |dd| {
                    if suppress.get() {
                        return;
                    }
                    if let Some(chosen) = options.get(dd.selected() as usize) {
                        on_change(setting_from_str(&template, chosen));
                    }
                }
            });
            let setter = {
                let dropdown = dropdown.clone();
                let options = options.clone();
                Box::new(move |v: &Setting| {
                    suppress.set(true);
                    let cur = v.value_string();
                    if let Some(i) = options.iter().position(|o| *o == cur) {
                        dropdown.set_selected(i as u32);
                    }
                    suppress.set(false);
                })
            };
            (dropdown.upcast(), setter)
        }
        (Setting::UInt(_) | Setting::SInt(_), constraint) => {
            let (lo, hi) = match constraint {
                Some(Constraint::Range(ranges)) => (
                    ranges.iter().map(|r| r.0).min().unwrap_or(0) as f64,
                    ranges.iter().map(|r| r.1).max().unwrap_or(isize::MAX) as f64,
                ),
                _ if matches!(info.default, Setting::UInt(_)) => (0.0, u32::MAX as f64),
                _ => (i32::MIN as f64, i32::MAX as f64),
            };
            let spin = gtk4::SpinButton::with_range(lo, hi, 1.0);
            spin.set_digits(0);
            spin.set_width_chars(12);
            spin.set_value(current.to_sint() as f64);
            spin.connect_value_changed({
                let suppress = suppress.clone();
                let unsigned = matches!(info.default, Setting::UInt(_));
                move |s| {
                    if suppress.get() {
                        return;
                    }
                    let v = s.value_as_int() as isize;
                    on_change(if unsigned {
                        Setting::UInt(v.max(0) as usize)
                    } else {
                        Setting::SInt(v)
                    });
                }
            });
            let setter = {
                let spin = spin.clone();
                Box::new(move |v: &Setting| {
                    suppress.set(true);
                    spin.set_value(v.to_sint() as f64);
                    suppress.set(false);
                })
            };
            (spin.upcast(), setter)
        }
        (Setting::Float(_), _) => {
            let spin = gtk4::SpinButton::with_range(f64::MIN, f64::MAX, 0.1);
            spin.set_digits(2);
            spin.set_width_chars(12);
            spin.set_value(current.to_float());
            spin.connect_value_changed({
                let suppress = suppress.clone();
                move |s| {
                    if !suppress.get() {
                        on_change(Setting::Float(s.value()));
                    }
                }
            });
            let setter = {
                let spin = spin.clone();
                Box::new(move |v: &Setting| {
                    suppress.set(true);
                    spin.set_value(v.to_float());
                    suppress.set(false);
                })
            };
            (spin.upcast(), setter)
        }
        // Strings and maps (comma-separated) edit as free text, committed on Enter/focus-out.
        (template, _) => {
            let entry = gtk4::Entry::new();
            entry.set_text(&current.value_string());
            entry.set_width_chars(28);
            let template = template.clone();
            let commit = Rc::new({
                let suppress = suppress.clone();
                move |e: &gtk4::Entry| {
                    if !suppress.get() {
                        on_change(setting_from_str(&template, e.text().as_str()));
                    }
                }
            });
            entry.connect_activate({
                let commit = commit.clone();
                move |e| commit(e)
            });
            let focus = gtk4::EventControllerFocus::new();
            focus.connect_leave({
                let entry = entry.clone();
                move |_| commit(&entry)
            });
            entry.add_controller(focus);
            let setter = {
                let entry = entry.clone();
                Box::new(move |v: &Setting| {
                    suppress.set(true);
                    entry.set_text(&v.value_string());
                    suppress.set(false);
                })
            };
            (entry.upcast(), setter)
        }
    }
}

/// Parse `text` into a `Setting` of the same variant as `template`.
fn setting_from_str(template: &Setting, text: &str) -> Setting {
    let text = text.trim();
    match template {
        Setting::Bool(_) => Setting::Bool(matches!(text.to_lowercase().as_str(), "true" | "1" | "yes" | "on")),
        Setting::UInt(_) => Setting::UInt(text.parse().unwrap_or(0)),
        Setting::SInt(_) => Setting::SInt(text.parse().unwrap_or(0)),
        Setting::Float(_) => Setting::Float(text.parse().unwrap_or(0.0)),
        Setting::String(_) => Setting::String(text.to_string()),
        Setting::Map(_) => Setting::Map(text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()),
    }
}
