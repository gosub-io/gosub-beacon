//! The window every picker shares: a sidebar with the gosub mark, the sections and the
//! lighthouse; a title; a main card; an optional side card; Cancel and the commit button.
//!
//! Laid out on a `GtkFixed` at the design's own coordinates rather than in boxes. The
//! design is a fixed-size Figma frame where every measurement is known, and the Mac shell
//! it is ported from places its views the same way — matching those numbers is what keeps
//! the two chromes looking like one product. Anything that can be a style instead of a
//! measurement lives in `picker.css`.
//!
//! Two sizes exist, both from the designs: the **small** one the colour picker uses at its
//! own scale, and the **large** one from the date and time screens, drawn at 0.7 because
//! 983 × 910 is most of a laptop screen. The Mac scales the large frame by remapping the
//! root view's bounds; GTK has no equivalent for an arbitrary subtree, so every coordinate
//! is multiplied on the way into `put()` instead — `Metrics::scale`, and `PickerShell::s`
//! for the pages.

use gtk4::gdk_pixbuf::Pixbuf;
use gtk4::glib;
use gtk4::prelude::GdkCairoContextExt;
use gtk4::prelude::*;
use gtk4::{
    gdk, Align, Box as GtkBox, Button, DrawingArea, EventControllerKey, Fixed, Image, Label, Orientation, Settings, Widget, Window,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::color::CssColor;

/// Called once, with whether the picker was accepted.
type FinishCallback = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;
type NavCallback = Rc<RefCell<Option<Box<dyn Fn(usize)>>>>;
/// Answers whether the picker dealt with the button itself, as the month & year drill-down
/// does: there, OK and Cancel belong to that screen, not to the picker's own answer.
type InterceptCallback = Rc<RefCell<Option<Box<dyn Fn(bool) -> bool>>>>;
/// Everything to rerun when the theme flips; see `connect_theme_changed`.
type ThemeCallbacks = RefCell<Vec<Box<dyn Fn()>>>;

/// A rectangle in design coordinates, top-left origin.
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
}

/// What a sidebar section wears. The colour picker's rainbow disc and the date pickers'
/// bolt are drawn, because neither has an icon-theme equivalent worth relying on.
#[derive(Clone, Copy, PartialEq)]
pub enum NavIcon {
    HueDisc,
    Calendar,
    /// Quick select and the clock face arrive with the time pickers.
    #[allow(dead_code)]
    Bolt,
    #[allow(dead_code)]
    Clock,
}

pub struct NavItem {
    pub icon: NavIcon,
    pub title: &'static str,
}

/// The shell's fixed measurements, for each of its two sizes.
pub struct Metrics {
    pub sidebar_width: f64,
    pub logo_origin: (f64, f64),
    pub logo_scale: f64,
    pub wordmark_origin: (f64, f64),
    pub wordmark_size: f64,
    /// Where "For a more open web" goes, in the shell that has room for it.
    pub tagline: Option<(f64, f64)>,
    pub nav_origin: (f64, f64),
    pub nav_size: (f64, f64),
    pub nav_pitch: f64,
    pub nav_font_size: f64,
    pub nav_symbol_size: f64,
    pub title_origin: (f64, f64),
    pub title_size: f64,
    pub button_height: f64,
    #[allow(dead_code)] // the time pickers size their own foot buttons from this
    pub button_font_size: f64,
    pub ok_width: f64,
    pub cancel_width: f64,
    pub button_bottom_inset: f64,
    pub help_button: bool,
    /// How much of the design to actually draw; see the module comment.
    pub scale: f64,
}

impl Metrics {
    pub const SMALL: Metrics = Metrics {
        sidebar_width: 143.0,
        logo_origin: (17.0, 41.0),
        logo_scale: 0.6,
        wordmark_origin: (51.0, 50.0),
        wordmark_size: 20.0,
        tagline: None,
        nav_origin: (11.0, 78.0),
        nav_size: (124.0, 35.0),
        nav_pitch: 40.0,
        nav_font_size: 15.0,
        nav_symbol_size: 15.0,
        title_origin: (163.0, 14.0),
        title_size: 20.0,
        button_height: 35.0,
        button_font_size: 15.0,
        ok_width: 102.0,
        cancel_width: 100.0,
        button_bottom_inset: 46.0,
        help_button: false,
        scale: 1.0,
    };

    pub const LARGE: Metrics = Metrics {
        sidebar_width: 275.0,
        logo_origin: (34.0, 70.0),
        logo_scale: 0.95,
        wordmark_origin: (92.0, 85.0),
        wordmark_size: 30.0,
        tagline: Some((93.0, 113.0)),
        nav_origin: (17.0, 167.0),
        nav_size: (244.0, 58.0),
        nav_pitch: 68.0,
        nav_font_size: 20.0,
        nav_symbol_size: 24.0,
        title_origin: (301.0, 42.0),
        title_size: 30.0,
        button_height: 50.0,
        button_font_size: 18.0,
        ok_width: 150.0,
        cancel_width: 141.0,
        button_bottom_inset: 69.0,
        help_button: true,
        scale: 0.7,
    };
}

/// Everything a picker hands the shell when it is built.
pub struct ShellConfig<'a> {
    pub title: &'a str,
    /// The design's frame, before scaling.
    pub size: (f64, f64),
    pub main_card: Rect,
    pub side_card: Option<Rect>,
    pub nav: Vec<NavItem>,
    pub metrics: &'static Metrics,
    /// "Select" on the colour picker, "OK" on the date and time ones, as the screens have it.
    pub ok_label: &'a str,
    /// Where the `?` button leads, in the shell that has one.
    pub help_url: Option<&'a str>,
}

/// Whether a picker is drawn dark. One per picker, shared by everything in it and flipped
/// when the desktop theme changes, so a draw function reads it as it draws rather than
/// keeping the value it was built with.
pub type Dark = Rc<Cell<bool>>;

pub struct PickerShell {
    pub window: Window,
    /// Where a picker puts its content: the design's coordinates inside the card, scaled.
    pub main_card: Fixed,
    pub side_card: Option<Fixed>,
    pub dark: Dark,
    pub scale: f64,
    title_label: Label,
    nav_buttons: Vec<Button>,
    selected_nav: Cell<usize>,
    finished: Rc<Cell<bool>>,
    on_finish: FinishCallback,
    on_nav: NavCallback,
    intercept: InterceptCallback,
    on_theme: ThemeCallbacks,
}

impl PickerShell {
    pub fn new(parent: Option<&impl IsA<Window>>, config: ShellConfig<'_>) -> Rc<Self> {
        let metrics = config.metrics;
        let scale = metrics.scale;
        let s = |v: f64| v * scale;
        let size = config.size;
        let dark: Dark = Rc::new(Cell::new(prefers_dark()));

        let window = Window::builder()
            .title(config.title)
            .resizable(false)
            .default_width(s(size.0) as i32)
            .default_height(s(size.1) as i32)
            .modal(true)
            .build();
        // The design has no titlebar: the window is the card. An empty header keeps the
        // client-side shadow and rounded corners that an undecorated window loses.
        let header = gtk4::HeaderBar::builder().show_title_buttons(false).build();
        header.set_visible(false);
        window.set_titlebar(Some(&header));
        window.set_decorated(false);
        window.add_css_class("picker");
        if dark.get() {
            window.add_css_class("dark");
        }
        // Rounded corners need a compositor to put transparency outside them; without one
        // (a bare X11 session, or the Xvfb the tests run on) the corners come out as white
        // notches, so the window is square there instead.
        let composited = gdk::Display::default().map(|display| display.is_composited()).unwrap_or(false);
        if !composited {
            window.add_css_class("square");
        }
        if let Some(parent) = parent {
            window.set_transient_for(Some(parent.as_ref()));
        }

        let root = Fixed::new();
        root.add_css_class("picker-root");
        root.set_size_request(s(size.0) as i32, s(size.1) as i32);
        window.set_child(Some(&root));

        // ── sidebar ──────────────────────────────────────────────────────
        let sidebar = Fixed::new();
        sidebar.set_size_request(s(metrics.sidebar_width) as i32, s(size.1) as i32);
        let art = sidebar_art(metrics, size, dark.clone(), composited);
        sidebar.put(&art, 0.0, 0.0);

        let wordmark = Label::new(Some("gosub"));
        wordmark.add_css_class("picker-wordmark");
        wordmark.set_xalign(0.0);
        wordmark.set_attributes(Some(&font_size(s(metrics.wordmark_size))));
        sidebar.put(
            &wordmark,
            s(metrics.wordmark_origin.0),
            s(metrics.wordmark_origin.1 - metrics.wordmark_size * 0.75),
        );
        if let Some((x, y)) = metrics.tagline {
            let tagline = Label::new(Some("For a more open web"));
            tagline.add_css_class("picker-tagline");
            tagline.set_xalign(0.0);
            tagline.set_attributes(Some(&font_size(s(13.0))));
            sidebar.put(&tagline, s(x), s(y));
        }

        let mut nav_buttons = Vec::new();
        for (i, item) in config.nav.iter().enumerate() {
            let button = nav_button(item, metrics, scale);
            if i == 0 {
                button.add_css_class("selected");
            }
            sidebar.put(
                &button,
                s(metrics.nav_origin.0),
                s(metrics.nav_origin.1 + i as f64 * metrics.nav_pitch),
            );
            nav_buttons.push(button);
        }
        root.put(&sidebar, 0.0, 0.0);

        // ── title ────────────────────────────────────────────────────────
        let title_label = Label::new(Some(config.title));
        title_label.add_css_class("picker-title");
        title_label.set_xalign(0.0);
        title_label.set_attributes(Some(&font_size(s(metrics.title_size))));
        root.put(&title_label, s(metrics.title_origin.0), s(metrics.title_origin.1));

        // ── cards ────────────────────────────────────────────────────────
        let main = card(config.main_card, scale);
        root.put(&main.0, s(config.main_card.x), s(config.main_card.y));
        let side = config.side_card.map(|rect| {
            let side = card(rect, scale);
            root.put(&side.0, s(rect.x), s(rect.y));
            side.1
        });

        // ── buttons ──────────────────────────────────────────────────────
        let button_y = s(size.1 - metrics.button_bottom_inset);
        let ok = Button::with_label(config.ok_label);
        ok.add_css_class("picker-button");
        ok.add_css_class("select");
        ok.set_size_request(s(metrics.ok_width) as i32, s(metrics.button_height) as i32);
        root.put(&ok, s(size.0 - 18.0 - metrics.ok_width), button_y);

        let cancel = Button::with_label("Cancel");
        cancel.add_css_class("picker-button");
        cancel.add_css_class("cancel");
        cancel.set_size_request(s(metrics.cancel_width) as i32, s(metrics.button_height) as i32);
        root.put(&cancel, s(size.0 - 18.0 - metrics.ok_width - 15.0 - metrics.cancel_width), button_y);

        if metrics.help_button {
            if let Some(url) = config.help_url.map(str::to_string) {
                let help = Button::with_label("?");
                help.add_css_class("picker-help");
                help.set_size_request(s(34.0) as i32, s(34.0) as i32);
                help.set_tooltip_text(Some("What this control accepts"));
                root.put(&help, s(metrics.sidebar_width + 5.0), s(size.1 - 61.0));
                let window = window.clone();
                help.connect_clicked(move |_| {
                    gtk4::UriLauncher::new(&url).launch(Some(&window), gtk4::gio::Cancellable::NONE, |_| {});
                });
            }
        }

        let shell = Rc::new(Self {
            window,
            main_card: main.1,
            side_card: side,
            dark,
            scale,
            title_label,
            nav_buttons,
            selected_nav: Cell::new(0),
            finished: Rc::new(Cell::new(false)),
            on_finish: Rc::new(RefCell::new(None)),
            on_nav: Rc::new(RefCell::new(None)),
            intercept: Rc::new(RefCell::new(None)),
            on_theme: RefCell::new(Vec::new()),
        });
        shell.follow_theme();

        for (i, button) in shell.nav_buttons.iter().enumerate() {
            let shell_ref = Rc::downgrade(&shell);
            button.connect_clicked(move |_| {
                if let Some(shell) = shell_ref.upgrade() {
                    shell.select_nav(i);
                    if let Some(callback) = shell.on_nav.borrow().as_ref() {
                        callback(i);
                    }
                }
            });
        }

        let finish = {
            let finished = shell.finished.clone();
            let on_finish = shell.on_finish.clone();
            let intercept = shell.intercept.clone();
            let window = shell.window.clone();
            move |ok: bool| {
                // A picker showing a screen of its own answers for these first.
                if let Some(handled) = intercept.borrow().as_ref() {
                    if handled(ok) {
                        return;
                    }
                }
                // The commit button, Cancel, Escape and the window manager's close all land
                // here, and only the first of them answers: the control that asked gets one
                // reply. A picker that intercepts them (the month & year drill-down) does so
                // before they reach this.
                if finished.replace(true) {
                    return;
                }
                if let Some(callback) = on_finish.borrow().as_ref() {
                    callback(ok);
                }
                window.close();
            }
        };
        ok.connect_clicked({
            let finish = finish.clone();
            move |_| finish(true)
        });
        cancel.connect_clicked({
            let finish = finish.clone();
            move |_| finish(false)
        });
        shell.window.connect_close_request({
            let finish = finish.clone();
            move |_| {
                finish(false);
                glib::Propagation::Proceed
            }
        });
        let keys = EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Escape => {
                finish(false);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        shell.window.add_controller(keys);

        shell
    }

    /// A design coordinate in the pixels this shell actually draws.
    #[allow(dead_code)] // pages take the scale directly today; the time pages use this
    pub fn s(&self, value: f64) -> f64 {
        value * self.scale
    }

    pub fn connect_finish(&self, callback: impl Fn(bool) + 'static) {
        *self.on_finish.borrow_mut() = Some(Box::new(callback));
    }

    /// Take first refusal on OK and Cancel; answer `true` to keep the picker open.
    pub fn connect_intercept(&self, callback: impl Fn(bool) -> bool + 'static) {
        *self.intercept.borrow_mut() = Some(Box::new(callback));
    }

    /// A sidebar section was chosen. Every picker has sections once Quick select lands.
    #[allow(dead_code)]
    pub fn connect_nav(&self, callback: impl Fn(usize) + 'static) {
        *self.on_nav.borrow_mut() = Some(Box::new(callback));
    }

    /// The theme flipped while the picker was open. Drawing areas are redrawn by the shell;
    /// this is for what a picker built from the theme, such as its list of system colours.
    pub fn connect_theme_changed(&self, callback: impl Fn() + 'static) {
        self.on_theme.borrow_mut().push(Box::new(callback));
    }

    /// Follow the chrome's theme while the picker is open, as the Mac's pickers do.
    fn follow_theme(self: &Rc<Self>) {
        let Some(settings) = Settings::default() else { return };
        let shell = Rc::downgrade(self);
        let handler = settings.connect_gtk_application_prefer_dark_theme_notify(move |settings| {
            let Some(shell) = shell.upgrade() else { return };
            let dark = settings.is_gtk_application_prefer_dark_theme();
            if shell.dark.replace(dark) == dark {
                return;
            }
            if dark {
                shell.window.add_css_class("dark");
            } else {
                shell.window.remove_css_class("dark");
            }
            redraw_drawings(shell.window.upcast_ref());
            for callback in shell.on_theme.borrow().iter() {
                callback();
            }
        });
        // Settings outlives every picker, so the handler has to go with the window.
        let handler = Cell::new(Some(handler));
        self.window.connect_destroy(move |_| {
            if let Some(handler) = handler.take() {
                settings.disconnect(handler);
            }
        });
    }

    #[allow(dead_code)] // as above
    pub fn selected_nav(&self) -> usize {
        self.selected_nav.get()
    }

    pub fn select_nav(&self, index: usize) {
        self.selected_nav.set(index);
        for (i, button) in self.nav_buttons.iter().enumerate() {
            if i == index {
                button.add_css_class("selected");
            } else {
                button.remove_css_class("selected");
            }
        }
    }

    /// The title changes with the section, and with the drill-down.
    pub fn set_title(&self, title: &str) {
        self.title_label.set_text(title);
    }

    /// Swap what the main card holds. The pickers that have sections build each page once
    /// and move it in and out, as the Mac shell does.
    pub fn set_page(&self, page: &impl IsA<Widget>) {
        while let Some(child) = self.main_card.first_child() {
            self.main_card.remove(&child);
        }
        self.main_card.put(page, 0.0, 0.0);
    }

    /// Answer for the control without the user: used when a second request arrives and this
    /// picker is replaced, so the one being closed does not report for a control that has
    /// moved on.
    pub fn detach(&self) {
        self.finished.set(true);
        self.window.close();
    }

    pub fn present(&self) {
        self.window.present();
    }
}

/// A card: a styled box with a `Fixed` inside, so content is placed in card coordinates.
fn card(rect: Rect, scale: f64) -> (GtkBox, Fixed) {
    let outer = GtkBox::new(Orientation::Vertical, 0);
    outer.add_css_class("picker-card");
    outer.set_size_request((rect.w * scale) as i32, (rect.h * scale) as i32);
    let inner = Fixed::new();
    inner.set_hexpand(true);
    inner.set_vexpand(true);
    outer.append(&inner);
    (outer, inner)
}

/// Pango attributes for a point size, since CSS cannot size a label the design sizes — and
/// the large shell's text scales with everything else.
pub fn font_size(size: f64) -> gtk4::pango::AttrList {
    let attrs = gtk4::pango::AttrList::new();
    attrs.insert(gtk4::pango::AttrSize::new((size * f64::from(gtk4::pango::SCALE)) as i32));
    attrs
}

/// A sidebar section: its mark and a title, in the accent tint when it is the one showing.
fn nav_button(item: &NavItem, metrics: &Metrics, scale: f64) -> Button {
    let row = GtkBox::new(Orientation::Horizontal, (8.0 * scale) as i32);
    let symbol = metrics.nav_symbol_size * scale;
    match item.icon {
        NavIcon::Calendar | NavIcon::Clock => {
            let name = if item.icon == NavIcon::Calendar {
                "x-office-calendar-symbolic"
            } else {
                "alarm-symbolic"
            };
            let image = Image::from_icon_name(name);
            image.set_pixel_size(symbol as i32);
            image.set_valign(Align::Center);
            image.add_css_class("picker-nav-icon");
            row.append(&image);
        }
        NavIcon::HueDisc | NavIcon::Bolt => {
            let drawn = DrawingArea::new();
            let icon = item.icon;
            drawn.set_size_request(symbol as i32, symbol as i32);
            drawn.set_valign(Align::Center);
            drawn.set_draw_func(move |_, cr, w, h| match icon {
                NavIcon::Bolt => draw_bolt(cr, f64::from(w), f64::from(h)),
                _ => draw_hue_disc(cr, f64::from(w), f64::from(h)),
            });
            row.append(&drawn);
        }
    }
    let label = Label::new(Some(item.title));
    label.set_xalign(0.0);
    label.set_attributes(Some(&font_size(metrics.nav_font_size * scale)));
    row.append(&label);

    let button = Button::builder().child(&row).build();
    button.add_css_class("picker-nav");
    button.set_size_request((metrics.nav_size.0 * scale) as i32, (metrics.nav_size.1 * scale) as i32);
    button
}

/// The rainbow disc the colour picker's section wears: a conic sweep with a white core.
fn draw_hue_disc(cr: &gtk4::cairo::Context, w: f64, h: f64) {
    let radius = w.min(h) / 2.0 - 1.0;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let steps = 36;
    for i in 0..steps {
        let start = f64::from(i) / f64::from(steps) * std::f64::consts::TAU;
        let end = f64::from(i + 1) / f64::from(steps) * std::f64::consts::TAU + 0.02;
        let c = CssColor::from_hsv(f64::from(i) / f64::from(steps) * 360.0, 0.85, 1.0, 1.0);
        cr.set_source_rgb(c.r, c.g, c.b);
        cr.move_to(cx, cy);
        cr.arc(cx, cy, radius, start, end);
        cr.close_path();
        let _ = cr.fill();
    }
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.arc(cx, cy, radius * 0.39, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
}

/// The quick-select bolt, drawn rather than borrowed from an icon theme: no theme has one
/// that matches, and the sections should not look like a grab bag.
fn draw_bolt(cr: &gtk4::cairo::Context, w: f64, h: f64) {
    let (x, y, k) = (w / 2.0, h / 2.0, w.min(h) / 24.0);
    cr.set_source_rgb(0.05, 0.09, 0.16);
    cr.move_to(x + 2.0 * k, y - 12.0 * k);
    cr.line_to(x - 9.0 * k, y + 2.0 * k);
    cr.line_to(x - 1.0 * k, y + 2.0 * k);
    cr.line_to(x - 3.0 * k, y + 12.0 * k);
    cr.line_to(x + 9.0 * k, y - 3.0 * k);
    cr.line_to(x + 1.0 * k, y - 3.0 * k);
    cr.close_path();
    let _ = cr.fill();
}

/// The sidebar's own drawing: its tint with the window's left corners rounded, the
/// submarine, and the lighthouse fading in at the foot.
/// The theme the chrome is in: `theme.rs` mirrors the desktop's colour scheme into this
/// property, and the manual toggle writes it too.
fn prefers_dark() -> bool {
    Settings::default()
        .map(|settings| settings.is_gtk_application_prefer_dark_theme())
        .unwrap_or(false)
}

/// Queue a redraw of every drawing area under `widget`: they paint from [`Dark`] and
/// nothing else tells them it changed.
fn redraw_drawings(widget: &gtk4::Widget) {
    if let Some(area) = widget.downcast_ref::<DrawingArea>() {
        area.queue_draw();
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        redraw_drawings(&c);
        child = c.next_sibling();
    }
}

fn sidebar_art(metrics: &Metrics, size: (f64, f64), dark: Dark, composited: bool) -> DrawingArea {
    let area = DrawingArea::new();
    let scale = metrics.scale;
    area.set_size_request((metrics.sidebar_width * scale) as i32, (size.1 * scale) as i32);
    let logo = metrics.logo_origin;
    let logo_scale = metrics.logo_scale * scale;
    let lighthouse = Pixbuf::from_resource("/io/gosub/beacon/assets/picker-sidebar.png").ok();

    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        let radius = if composited { 12.0 } else { 0.0 };
        let dark = dark.get();
        let (fill, brand) = if dark {
            ((0x23, 0x28, 0x30), (0xC9, 0xD6, 0xF0))
        } else {
            ((0xE4, 0xE9, 0xF3), (0x24, 0x3A, 0x68))
        };
        let rgb = |(r, g, b): (u8, u8, u8)| (f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0);
        let (sr, sg, sb) = rgb(fill);

        cr.save().ok();
        // Cairo's `arc` always sweeps towards increasing angle, and y grows downwards, so a
        // corner is a quarter turn between the two edges it joins -- 90 to 180 degrees at the
        // bottom left, 180 to 270 at the top left. Giving them in the other order sweeps three
        // quarters of a circle instead, which clips the corners away and lets the page behind
        // show through.
        cr.new_sub_path();
        cr.move_to(w, 0.0);
        cr.line_to(w, h);
        cr.arc(radius, h - radius, radius, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
        cr.arc(radius, radius, radius, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
        cr.close_path();
        cr.clip_preserve();
        cr.set_source_rgb(sr, sg, sb);
        let _ = cr.fill();

        if let Some(art) = &lighthouse {
            let art_h = w * f64::from(art.height()) / f64::from(art.width());
            let scaled = art.scale_simple(w as i32, art_h as i32, gtk4::gdk_pixbuf::InterpType::Bilinear);
            if let Some(scaled) = scaled {
                let top = h - art_h;
                cr.set_source_pixbuf(&scaled, 0.0, top);
                let _ = cr.paint();
                // The design tints the artwork back towards the sidebar and fades its top
                // edge, so it reads as a watermark rather than a photograph. The artwork is
                // a daylight photograph, so at night it has to be pushed much further than
                // the Mac's 25% black: tinting towards the sidebar itself, rather than
                // simply darkening, is what keeps it a watermark instead of a bright panel.
                if dark {
                    cr.set_source_rgba(sr, sg, sb, 0.62);
                } else {
                    cr.set_source_rgba(0.784, 0.855, 0.933, 0.12);
                }
                cr.rectangle(0.0, top, w, art_h);
                let _ = cr.fill();
                let fade = gtk4::cairo::LinearGradient::new(0.0, top - 10.0, 0.0, top - 10.0 + art_h * 0.4);
                fade.add_color_stop_rgba(0.0, sr, sg, sb, 1.0);
                fade.add_color_stop_rgba(1.0, sr, sg, sb, 0.0);
                let _ = cr.set_source(&fade);
                cr.rectangle(0.0, top - 10.0, w, art_h * 0.4);
                let _ = cr.fill();
            }
        }
        cr.restore().ok();

        // The submarine, as the design draws it: hull, tower, periscope, three portholes.
        let (br, bg, bb) = rgb(brand);
        cr.set_source_rgb(br, bg, bb);
        let at = |x: f64, y: f64, rw: f64, rh: f64, r: f64| {
            let (x, y, rw, rh, r) = (
                logo.0 * scale + x * logo_scale,
                logo.1 * scale + y * logo_scale,
                rw * logo_scale,
                rh * logo_scale,
                r * logo_scale,
            );
            cr.new_sub_path();
            cr.arc(x + rw - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
            cr.arc(x + rw - r, y + rh - r, r, 0.0, std::f64::consts::FRAC_PI_2);
            cr.arc(x + r, y + rh - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
            cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
            cr.close_path();
        };
        at(0.0, 12.0, 48.0, 18.0, 9.0);
        at(19.0, 6.0, 14.0, 8.0, 4.0);
        at(25.0, 0.0, 4.0, 8.0, 2.0);
        let _ = cr.fill();
        cr.set_source_rgb(sr, sg, sb);
        for x in [14.5_f64, 23.5, 32.5] {
            cr.arc(
                logo.0 * scale + x * logo_scale,
                logo.1 * scale + 19.5 * logo_scale,
                2.5 * logo_scale,
                0.0,
                std::f64::consts::TAU,
            );
            let _ = cr.fill();
        }
    });
    area
}
