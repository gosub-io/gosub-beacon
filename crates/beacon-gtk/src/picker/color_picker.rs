//! The colour picker, drawn to the Figma design: the plane and its strips, the CSS name and
//! hex fields, the R/G/B/H/S/L cells, the quick swatches, and the list of CSS colours.
//!
//! A port of the Mac shell's `ColorPickerWindow.swift`, layout and behaviour both. The
//! design is a fixed frame, so the content is placed at its own coordinates inside the
//! cards the shell provides; everything that is a colour or a font lives in `picker.css`.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, Entry, EventControllerFocus, GestureClick, Label, ListBox, ListBoxRow, Orientation, PolicyType,
    ScrolledWindow, Window,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::color::{named, system_colors, CssColor, Named};
use super::shell::{Metrics, NavIcon, NavItem, PickerShell, Rect, ShellConfig};
use super::widgets::{row_swatch, swatch, ColorStrip, StripKind, SvPlane};

/// A picker reports every intermediate colour, so the control on the page previews live.
type ChangeCallback = RefCell<Option<Box<dyn Fn(CssColor)>>>;

/// How many quick swatches are kept. The row shows them all; the "+" goes away when it is
/// full, and a colour added past the cap replaces the oldest.
const SWATCH_LIMIT: usize = 10;

/// The eight the design ships with.
const DEFAULT_SWATCHES: [&str; 8] = [
    "#663399", "#5a5fea", "#d63b9d", "#f06661", "#ff8a1a", "#ffbe2e", "#37ad72", "#18a8b6",
];

/// Where a change came from, so the control that caused it is not written back to while it
/// is being used: typing in the hex field must not re-render the hex field under the caret,
/// and dragging the plane must not move the plane's own knob.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    External,
    Plane,
    Hue,
    Alpha,
    Name,
    Hex,
    Channels,
    List,
    Swatch,
}

struct Row {
    color: CssColor,
    name: String,
}

pub struct ColorPicker {
    shell: Rc<PickerShell>,
    plane: SvPlane,
    hue_strip: ColorStrip,
    alpha_strip: ColorStrip,
    name_field: Entry,
    name_caption: Label,
    hex_field: Entry,
    channel_fields: Vec<Entry>,
    swatch_row: GtkBox,
    /// One per drawn swatch, in row order: whether it is the colour showing. The areas draw
    /// from these, so the ring follows the colour without rebuilding the row.
    swatch_flags: RefCell<Vec<Rc<Cell<bool>>>>,
    search_field: Entry,
    clear_search: Button,
    chips: Vec<Button>,
    list: ListBox,
    scroller: ScrolledWindow,
    rows: RefCell<Vec<Row>>,

    color: Cell<CssColor>,
    /// The hue the strip is on. Separate from the colour because a grey has none: dragging
    /// the plane into the black corner and back out must return to the same hue.
    hue: Cell<f64>,
    allows_alpha: bool,
    quick_swatches: RefCell<Vec<CssColor>>,
    /// Set while the picker writes to its own widgets, so their change signals are ignored.
    syncing: Cell<bool>,
    on_change: ChangeCallback,
}

impl ColorPicker {
    /// Build the picker over `parent` for a control currently holding `initial`.
    pub fn new(parent: Option<&impl IsA<Window>>, initial: CssColor, allows_alpha: bool) -> Rc<Self> {
        let shell = PickerShell::new(
            parent,
            ShellConfig {
                title: "Select a color",
                size: (960.0, 600.0),
                main_card: Rect::new(154.0, 42.0, 520.0, 500.0),
                side_card: Some(Rect::new(686.0, 42.0, 260.0, 500.0)),
                nav: vec![NavItem {
                    icon: NavIcon::HueDisc,
                    title: "Color",
                }],
                metrics: &Metrics::SMALL,
                ok_label: "Select",
                help_url: None,
            },
        );

        let picker = Rc::new(Self {
            plane: SvPlane::new(380, 260, 11.0),
            hue_strip: ColorStrip::new(StripKind::Hue, 22, 260, 12.0),
            alpha_strip: ColorStrip::new(StripKind::Alpha, 22, 260, 12.0),
            name_field: field("e.g. rebeccapurple, rgb(…)", 200, &[]),
            name_caption: caption(""),
            hex_field: field("#rrggbb", 130, &["hex"]),
            channel_fields: (0..6).map(|_| field("", 48, &["numeric"])).collect(),
            swatch_row: GtkBox::new(Orientation::Horizontal, 8),
            swatch_flags: RefCell::new(Vec::new()),
            search_field: field("Search", 178, &[]),
            clear_search: Button::from_icon_name("edit-clear-symbolic"),
            chips: ["All", "Named", "System"].iter().map(|title| chip(title)).collect(),
            list: ListBox::new(),
            scroller: ScrolledWindow::builder()
                .hscrollbar_policy(PolicyType::Never)
                .vscrollbar_policy(PolicyType::Automatic)
                .build(),
            rows: RefCell::new(Vec::new()),
            color: Cell::new(initial),
            hue: Cell::new(initial.hsv().0),
            allows_alpha,
            quick_swatches: RefCell::new(load_swatches()),
            syncing: Cell::new(false),
            on_change: RefCell::new(None),
            shell,
        });

        picker.build_main();
        picker.build_side();
        picker.wire();
        picker.apply(initial, Source::External);
        picker.rebuild_rows();
        picker
    }

    pub fn connect_change(&self, callback: impl Fn(CssColor) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(callback));
    }

    /// `ok` carries the chosen colour; a cancel carries nothing and the caller puts the
    /// original back.
    pub fn connect_finish(self: &Rc<Self>, callback: impl Fn(Option<CssColor>) + 'static) {
        let picker = Rc::downgrade(self);
        self.shell.connect_finish(move |ok| {
            let color = picker.upgrade().map(|p| p.color.get());
            callback(if ok { color } else { None });
        });
    }

    pub fn present(&self) {
        self.shell.present();
    }

    pub fn detach(&self) {
        self.shell.detach();
    }

    // ── the main card ─────────────────────────────────────────────────────

    fn build_main(self: &Rc<Self>) {
        let card = &self.shell.main_card;
        // The design draws an alpha strip beside the hue one, but a control that cannot hold
        // an opacity should not be offered a slider for it: a disabled strip still says the
        // picker has a setting it does not. So it appears only when alpha is wanted, and the
        // plane takes back the width it would have used -- the hue strip stays on the right
        // edge either way, so the composition is the design's in both shapes.
        if self.allows_alpha {
            card.put(&self.plane.area, 16.0, 16.0);
            card.put(&self.hue_strip.area, 408.0, 16.0);
            card.put(&self.alpha_strip.area, 442.0, 16.0);
            self.alpha_strip.area.set_tooltip_text(Some(
                "Opacity. A form's colour input keeps only the opaque colour; the hex here shows both.",
            ));
        } else {
            self.plane.area.set_size_request(414, 260);
            card.put(&self.plane.area, 16.0, 16.0);
            card.put(&self.hue_strip.area, 442.0, 16.0);
        }

        card.put(&label("CSS name"), 16.0, 292.0);
        card.put(&self.name_field, 16.0, 310.0);
        card.put(&self.name_caption, 16.0, 344.0);

        card.put(&label("Hex"), 232.0, 292.0);
        card.put(&self.hex_field, 232.0, 310.0);

        let copy = icon_button("edit-copy-symbolic", "Copy hex");
        card.put(&copy, 370.0, 310.0);
        copy.connect_clicked({
            let hex_field = self.hex_field.clone();
            move |button| {
                if let Some(display) = gtk4::gdk::Display::default() {
                    display.clipboard().set_text(&hex_field.text());
                }
                let _ = button;
            }
        });

        // The design's eyedropper. A browser cannot read the screen itself, so this is the
        // desktop portal's own picker; a desktop that does not offer one leaves the button
        // there but insensitive, rather than dropping it from a design people have seen.
        let available = super::eyedropper::is_available();
        let eyedropper = icon_button(
            "color-select-symbolic",
            if available {
                "Pick a colour from the screen"
            } else {
                "Pick a colour from the screen (this desktop offers no colour picker)"
            },
        );
        eyedropper.set_sensitive(available);
        card.put(&eyedropper, 410.0, 310.0);
        self.wire_eyedropper(&eyedropper);

        for (i, name) in ["R", "G", "B", "H", "S", "L"].iter().enumerate() {
            let x = [16.0, 68.0, 120.0, 190.0, 242.0, 294.0][i];
            let heading = Label::new(Some(name));
            heading.add_css_class("picker-caption");
            heading.set_size_request(48, 14);
            card.put(&heading, x, 364.0);
            card.put(&self.channel_fields[i], x, 380.0);
        }
        let percent = Label::new(Some("%"));
        percent.add_css_class("picker-unit");
        card.put(&percent, 346.0, 387.0);

        let divider = GtkBox::new(Orientation::Horizontal, 0);
        divider.add_css_class("picker-divider");
        divider.set_size_request(488, 1);
        card.put(&divider, 16.0, 424.0);

        let heading = Label::new(Some("Quick swatches"));
        heading.add_css_class("picker-section");
        card.put(&heading, 16.0, 432.0);
        card.put(&self.swatch_row, 16.0, 451.0);
    }

    /// The portal hands back an opaque sRGB colour whenever the user clicks a pixel. It is
    /// applied like any other outside change -- `External` re-renders every control,
    /// including the plane's knob -- and it keeps whatever opacity the picker already had.
    fn wire_eyedropper(self: &Rc<Self>, button: &Button) {
        let this = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            let this = this.clone();
            super::eyedropper::pick(move |picked| {
                if let Some(p) = this.upgrade() {
                    p.apply(
                        CssColor {
                            a: p.color.get().a,
                            ..picked
                        },
                        Source::External,
                    );
                }
            });
        });
    }

    // ── the side card ─────────────────────────────────────────────────────

    fn build_side(&self) {
        let Some(card) = &self.shell.side_card else { return };

        let heading = Label::new(Some("CSS colors"));
        heading.add_css_class("picker-side-heading");
        heading.set_xalign(0.0);
        card.put(&heading, 12.0, 12.0);

        // The glass and the field share one rounded box, as the design draws them.
        let search = GtkBox::new(Orientation::Horizontal, 6);
        search.add_css_class("picker-field");
        search.add_css_class("picker-search");
        search.set_size_request(236, 30);
        let glass = gtk4::Image::from_icon_name("system-search-symbolic");
        glass.set_pixel_size(16);
        search.append(&glass);
        self.search_field.remove_css_class("picker-field");
        self.search_field.set_hexpand(true);
        self.search_field.set_has_frame(false);
        search.append(&self.search_field);
        // The design's clear button: shown only once something is typed.
        self.clear_search.add_css_class("picker-clear");
        self.clear_search.set_visible(false);
        self.clear_search.set_valign(Align::Center);
        self.clear_search.set_tooltip_text(Some("Clear"));
        search.append(&self.clear_search);
        card.put(&search, 12.0, 44.0);

        // The design also has a "Page" chip — the colours the page uses — which needs an
        // engine API that does not exist yet, so it is not offered. The Mac shell leaves it
        // out for the same reason.
        for (i, (x, w)) in [(12.0, 44.0), (62.0, 70.0), (138.0, 74.0)].iter().enumerate() {
            let chip = &self.chips[i];
            chip.set_size_request(*w as i32, 26);
            if i != 0 {
                chip.remove_css_class("selected");
            }
            card.put(chip, *x, 84.0);
        }

        self.list.add_css_class("picker-list");
        self.list.set_selection_mode(gtk4::SelectionMode::Single);
        self.scroller.set_child(Some(&self.list));
        self.scroller.add_css_class("picker-list");
        self.scroller.set_size_request(244, 368);
        card.put(&self.scroller, 8.0, 120.0);
    }

    // ── wiring ────────────────────────────────────────────────────────────

    fn wire(self: &Rc<Self>) {
        let this = Rc::downgrade(self);
        self.plane.connect_change({
            let this = this.clone();
            move |s, v| {
                if let Some(p) = this.upgrade() {
                    let next = CssColor::from_hsv(p.hue.get(), s, v, p.color.get().a);
                    p.apply(next, Source::Plane);
                }
            }
        });
        self.hue_strip.connect_change({
            let this = this.clone();
            move |position| {
                if let Some(p) = this.upgrade() {
                    p.hue.set(position * 360.0);
                    let (_, s, v) = p.color.get().hsv();
                    p.apply(CssColor::from_hsv(p.hue.get(), s, v, p.color.get().a), Source::Hue);
                }
            }
        });
        self.alpha_strip.connect_change({
            let this = this.clone();
            move |position| {
                if let Some(p) = this.upgrade() {
                    let mut next = p.color.get();
                    next.a = 1.0 - position;
                    p.apply(next, Source::Alpha);
                }
            }
        });

        for (field, source) in [(&self.name_field, Source::Name), (&self.hex_field, Source::Hex)] {
            field.connect_changed({
                let this = this.clone();
                move |entry| {
                    let Some(p) = this.upgrade() else { return };
                    if p.syncing.get() {
                        return;
                    }
                    // Only a complete colour applies: typing "#66" should not paint black.
                    if let Some(parsed) = CssColor::parse(&entry.text()) {
                        p.apply(parsed, source);
                    }
                }
            });
            // Leaving a field that never became a colour puts the current one back, so the
            // box never keeps half a value.
            let focus = EventControllerFocus::new();
            focus.connect_leave({
                let this = this.clone();
                move |_| {
                    if let Some(p) = this.upgrade() {
                        p.apply(p.color.get(), Source::External);
                    }
                }
            });
            field.add_controller(focus);
        }

        for (i, field) in self.channel_fields.iter().enumerate() {
            field.connect_changed({
                let this = this.clone();
                move |_| {
                    let Some(p) = this.upgrade() else { return };
                    if p.syncing.get() {
                        return;
                    }
                    let numbers: Vec<Option<f64>> = p.channel_fields.iter().map(|f| f.text().trim().parse::<f64>().ok()).collect();
                    let next = if i < 3 {
                        match (numbers[0], numbers[1], numbers[2]) {
                            (Some(r), Some(g), Some(b)) => CssColor::new(r / 255.0, g / 255.0, b / 255.0, p.color.get().a),
                            _ => return,
                        }
                    } else {
                        match (numbers[3], numbers[4], numbers[5]) {
                            (Some(h), Some(s), Some(l)) => CssColor::from_hsl(h, s / 100.0, l / 100.0, p.color.get().a),
                            _ => return,
                        }
                    };
                    p.apply(next, Source::Channels);
                }
            });
        }

        // The system colours are the theme's own (Canvas is white by day, near black at
        // night), so a theme flip changes the list, not only how it is drawn.
        self.shell.connect_theme_changed({
            let this = this.clone();
            move || {
                if let Some(p) = this.upgrade() {
                    p.rebuild_rows();
                }
            }
        });
        self.search_field.connect_changed({
            let this = this.clone();
            move |entry| {
                if let Some(p) = this.upgrade() {
                    p.clear_search.set_visible(!entry.text().is_empty());
                    p.rebuild_rows();
                }
            }
        });
        self.clear_search.connect_clicked({
            let this = this.clone();
            move |_| {
                if let Some(p) = this.upgrade() {
                    p.search_field.set_text("");
                }
            }
        });

        for (i, chip) in self.chips.iter().enumerate() {
            chip.connect_clicked({
                let this = this.clone();
                move |_| {
                    let Some(p) = this.upgrade() else { return };
                    for (j, other) in p.chips.iter().enumerate() {
                        if j == i {
                            other.add_css_class("selected");
                        } else {
                            other.remove_css_class("selected");
                        }
                    }
                    p.rebuild_rows();
                }
            });
        }

        self.list.connect_row_selected({
            let this = this.clone();
            move |_, row| {
                let Some(p) = this.upgrade() else { return };
                if p.syncing.get() {
                    return;
                }
                let Some(row) = row else { return };
                let index = row.index() as usize;
                let color = p.rows.borrow().get(index).map(|r| r.color);
                if let Some(color) = color {
                    p.apply(
                        CssColor {
                            a: p.color.get().a,
                            ..color
                        },
                        Source::List,
                    );
                }
            }
        });

        self.rebuild_swatch_row();
    }

    // ── the colour, and everything that shows it ──────────────────────────

    /// The one place a change lands, whatever caused it.
    fn apply(&self, proposed: CssColor, source: Source) {
        let next = if self.allows_alpha { proposed } else { proposed.opaque() };
        self.color.set(next);
        let (h, s, v) = next.hsv();
        // A grey has no hue to read, so the strip keeps the one it had.
        if !matches!(source, Source::Plane | Source::Alpha | Source::Hue) && s > 0.0 && v > 0.0 {
            self.hue.set(h);
        }

        let was_syncing = self.syncing.replace(true);
        self.plane.set_hue(self.hue.get());
        if source != Source::Plane {
            self.plane.set_position(s, v);
        }
        if source != Source::Hue {
            self.hue_strip.set_position(self.hue.get() / 360.0);
        }
        self.alpha_strip.set_tint(next);
        if source != Source::Alpha {
            self.alpha_strip.set_position(1.0 - next.a);
        }

        let exact = next.css_name();
        if source != Source::Name {
            self.name_field.set_text(exact.map(|n| n.name.as_str()).unwrap_or(""));
        }
        // Under the name field: whether what is in it is a CSS keyword a stylesheet can use
        // as-is, and if not, which keyword comes closest.
        self.name_caption.set_text(&match exact {
            Some(named) if named.aliases.is_empty() => "CSS keyword".to_string(),
            Some(named) => format!("CSS keyword · also spelled {}", named.aliases.join(", ")),
            None => format!("Nearest keyword: {}", next.nearest_name().0.name),
        });

        if source != Source::Hex {
            self.hex_field.set_text(&next.hex_with_alpha());
        }
        if source != Source::Channels {
            let (h, s, l) = next.hsl();
            let values = [
                next.r8(),
                next.g8(),
                next.b8(),
                h.round() as i32,
                (s * 100.0).round() as i32,
                (l * 100.0).round() as i32,
            ];
            for (field, value) in self.channel_fields.iter().zip(values) {
                field.set_text(&value.to_string());
            }
        }

        self.refresh_swatch_rings();
        if source != Source::List {
            self.sync_list_selection();
        }
        self.syncing.set(was_syncing);

        if let Some(callback) = self.on_change.borrow().as_ref() {
            callback(next);
        }
    }

    // ── the list of CSS colours ───────────────────────────────────────────

    /// The rows for the current chip and search: names that start with the query, then names
    /// that contain it, then — when what was typed is itself a colour ("purple", "#639") —
    /// the names nearest to that colour, so a search for purple also turns up indigo and
    /// blueviolet.
    fn rebuild_rows(&self) {
        let selected = self.chips.iter().position(|c| c.has_css_class("selected")).unwrap_or(0);
        let system = system_colors(self.shell.dark.get());
        let pool: Vec<&Named> = match selected {
            1 => named().iter().collect(),
            2 => system.iter().collect(),
            _ => named().iter().chain(system.iter()).collect(),
        };

        let query = self.search_field.text().trim().to_ascii_lowercase();
        let picked: Vec<&Named> = if query.is_empty() {
            pool
        } else {
            let starts: Vec<&Named> = pool
                .iter()
                .copied()
                .filter(|n| n.name.to_ascii_lowercase().starts_with(&query))
                .collect();
            let contains: Vec<&Named> = pool
                .iter()
                .copied()
                .filter(|n| {
                    let name = n.name.to_ascii_lowercase();
                    !name.starts_with(&query) && name.contains(&query)
                })
                .collect();
            let mut picked: Vec<&Named> = starts.into_iter().chain(contains).collect();
            if let Some(probe) = CssColor::parse(&query) {
                let listed: Vec<&str> = picked.iter().map(|n| n.name.as_str()).collect();
                let mut near: Vec<(&Named, f64)> = pool
                    .iter()
                    .copied()
                    .filter(|n| !listed.contains(&n.name.as_str()))
                    .map(|n| (n, probe.distance(n.color)))
                    .filter(|(_, d)| *d < 140.0)
                    .collect();
                near.sort_by(|a, b| a.1.total_cmp(&b.1));
                picked.extend(near.into_iter().take(12).map(|(n, _)| n));
            }
            picked
        };

        let was_syncing = self.syncing.replace(true);
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let mut rows = Vec::with_capacity(picked.len());
        for entry in picked {
            self.list.append(&color_row(entry));
            rows.push(Row {
                color: entry.color,
                name: entry.name.clone(),
            });
        }
        *self.rows.borrow_mut() = rows;
        self.syncing.set(was_syncing);
        self.sync_list_selection();
    }

    fn sync_list_selection(&self) {
        let was_syncing = self.syncing.replace(true);
        let current = self.color.get().opaque();
        let index = self.rows.borrow().iter().position(|row| row.color.hex() == current.hex());
        match index {
            Some(index) => {
                if let Some(row) = self.list.row_at_index(index as i32) {
                    self.list.select_row(Some(&row));
                    // A freshly filled list has no allocations yet, so the scroll waits for
                    // the layout that follows this turn of the loop.
                    let scroller = self.scroller.clone();
                    let list = self.list.clone();
                    glib::idle_add_local_once(move || {
                        let Some(bounds) = row.compute_bounds(&list) else {
                            return;
                        };
                        let adjustment = scroller.vadjustment();
                        let (top, height) = (f64::from(bounds.y()), f64::from(bounds.height()));
                        let page = adjustment.page_size();
                        if top < adjustment.value() || top + height > adjustment.value() + page {
                            adjustment.set_value((top - (page - height) / 2.0).max(0.0));
                        }
                    });
                }
            }
            None => self.list.unselect_all(),
        }
        for (i, row) in self.rows.borrow().iter().enumerate() {
            if let Some(widget) = self.list.row_at_index(i as i32) {
                let selected = Some(i) == index;
                if selected {
                    widget.add_css_class("selected");
                } else {
                    widget.remove_css_class("selected");
                }
                let _ = &row.name;
            }
        }
        self.syncing.set(was_syncing);
    }

    // ── quick swatches ────────────────────────────────────────────────────

    fn rebuild_swatch_row(self: &Rc<Self>) {
        while let Some(child) = self.swatch_row.first_child() {
            self.swatch_row.remove(&child);
        }
        let this = Rc::downgrade(self);
        let current = self.color.get();
        let swatches = self.quick_swatches.borrow().clone();
        self.swatch_flags.borrow_mut().clear();
        for (i, color) in swatches.iter().take(SWATCH_LIMIT).enumerate() {
            let is_current = Rc::new(Cell::new(color.hex_with_alpha() == current.hex_with_alpha()));
            self.swatch_flags.borrow_mut().push(is_current.clone());
            let area = swatch(34, Some(*color), is_current, self.shell.dark.clone());
            area.set_tooltip_text(Some(&format!(
                "{} · right-click to remove",
                color.css_name().map(|n| n.name.clone()).unwrap_or_else(|| color.hex())
            )));
            let click = GestureClick::new();
            click.connect_released({
                let this = this.clone();
                let color = *color;
                move |_, _, _, _| {
                    if let Some(p) = this.upgrade() {
                        p.apply(color, Source::Swatch);
                    }
                }
            });
            area.add_controller(click);

            let remove = GestureClick::new();
            remove.set_button(3);
            remove.connect_released({
                let this = this.clone();
                move |_, _, _, _| {
                    if let Some(p) = this.upgrade() {
                        p.quick_swatches.borrow_mut().remove(i);
                        save_swatches(&p.quick_swatches.borrow());
                        p.rebuild_swatch_row();
                    }
                }
            });
            area.add_controller(remove);
            self.swatch_row.append(&area);
        }

        if swatches.len() < SWATCH_LIMIT {
            let adder = swatch(34, None, Rc::new(Cell::new(false)), self.shell.dark.clone());
            adder.set_tooltip_text(Some(&format!(
                "Keep this colour as a quick swatch ({} of {SWATCH_LIMIT}); right-click one to remove it",
                swatches.len()
            )));
            let click = GestureClick::new();
            click.connect_released({
                let this = this.clone();
                move |_, _, _, _| {
                    let Some(p) = this.upgrade() else { return };
                    let color = p.color.get();
                    {
                        let mut swatches = p.quick_swatches.borrow_mut();
                        if swatches.iter().any(|c| c.hex_with_alpha() == color.hex_with_alpha()) {
                            return;
                        }
                        swatches.push(color);
                        // Past the cap the oldest goes, as on the Mac.
                        while swatches.len() > SWATCH_LIMIT {
                            swatches.remove(0);
                        }
                        save_swatches(&swatches);
                    }
                    p.rebuild_swatch_row();
                }
            });
            adder.add_controller(click);
            self.swatch_row.append(&adder);
        }
    }

    fn refresh_swatch_rings(&self) {
        // Each area draws its ring from the flag it was built with, so the flag is what has
        // to change -- a redraw on its own would only repaint the old answer.
        let swatches = self.quick_swatches.borrow();
        let flags = self.swatch_flags.borrow();
        let current = self.color.get().hex_with_alpha();
        for (flag, color) in flags.iter().zip(swatches.iter()) {
            flag.set(color.hex_with_alpha() == current);
        }
        let mut child = self.swatch_row.first_child();
        while let Some(area) = child {
            area.queue_draw();
            child = area.next_sibling();
        }
    }
}

// ── small builders ──────────────────────────────────────────────────────────

fn label(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.add_css_class("picker-label");
    label.set_xalign(0.0);
    label
}

fn caption(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.add_css_class("picker-caption");
    label.set_xalign(0.0);
    label.set_size_request(488, 16);
    label
}

fn field(placeholder: &str, width: i32, classes: &[&str]) -> Entry {
    let entry = Entry::new();
    entry.add_css_class("picker-field");
    for class in classes {
        entry.add_css_class(class);
    }
    if !placeholder.is_empty() {
        entry.set_placeholder_text(Some(placeholder));
    }
    entry.set_size_request(width, 30);
    gtk4::prelude::EditableExt::set_alignment(&entry, if classes.contains(&"numeric") { 0.5 } else { 0.0 });
    entry
}

fn chip(title: &str) -> Button {
    let button = Button::with_label(title);
    button.add_css_class("picker-chip");
    button.add_css_class("selected");
    button
}

fn icon_button(icon: &str, tooltip: &str) -> Button {
    let button = Button::from_icon_name(icon);
    button.add_css_class("picker-icon-button");
    button.set_tooltip_text(Some(tooltip));
    button.set_size_request(32, 30);
    button
}

/// One row of the CSS-colour list: the chip, the keyword, its six digits.
fn color_row(entry: &Named) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.add_css_class("picker-row");
    row.set_size_request(236, 44);
    let content = GtkBox::new(Orientation::Horizontal, 10);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_margin_top(4);
    content.set_margin_bottom(4);

    let chip = row_swatch(44, 28, entry.color);
    chip.set_valign(Align::Center);
    content.append(&chip);

    let text = GtkBox::new(Orientation::Vertical, 0);
    text.set_valign(Align::Center);
    let name = Label::new(Some(&entry.name));
    name.add_css_class("picker-row-name");
    name.set_xalign(0.0);
    text.append(&name);
    let hex = Label::new(Some(&entry.color.hex()));
    hex.add_css_class("picker-row-hex");
    hex.set_xalign(0.0);
    text.append(&hex);
    content.append(&text);

    row.set_child(Some(&content));
    row
}

// ── quick-swatch storage ────────────────────────────────────────────────────

fn swatches_path() -> std::path::PathBuf {
    beacon_core::paths::data_dir().join("picker-swatches.txt")
}

/// The quick swatches, one hex per line beside the rest of the profile. Small enough that a
/// file is kinder than a settings table, and it survives a profile copy like everything else
/// there does.
fn load_swatches() -> Vec<CssColor> {
    let text = std::fs::read_to_string(swatches_path()).unwrap_or_default();
    let stored: Vec<CssColor> = text.lines().filter_map(CssColor::parse).collect();
    if stored.is_empty() {
        DEFAULT_SWATCHES.iter().filter_map(|hex| CssColor::parse(hex)).collect()
    } else {
        stored.into_iter().take(SWATCH_LIMIT).collect()
    }
}

fn save_swatches(swatches: &[CssColor]) {
    let text: String = swatches.iter().map(|c| format!("{}\n", c.hex_with_alpha())).collect();
    if let Err(error) = std::fs::write(swatches_path(), text) {
        log::warn!(target: "gtk", "could not save the picker's quick swatches: {error}");
    }
}
