//! The window every picker shares: a sidebar with the gosub mark, the sections and the
//! lighthouse; a title; a main card; an optional side card; Cancel and Select.
//!
//! Laid out on a `GtkFixed` at the design's own coordinates rather than in boxes. The
//! design is a fixed-size Figma frame where every measurement is known, and the Mac shell
//! it is ported from places its views the same way — matching those numbers is what keeps
//! the two chromes looking like one product. Anything that can be a style instead of a
//! measurement lives in `picker.css`.

use gtk4::gdk_pixbuf::Pixbuf;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    gdk, prelude::GdkCairoContextExt, Align, Box as GtkBox, Button, DrawingArea, EventControllerKey, Fixed, Label, Orientation, Settings,
    Window,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::color::CssColor;

/// Called once, with whether the picker was accepted.
type FinishCallback = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;

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

/// The shell's fixed measurements. Only the small size is here: it is the one the colour
/// picker uses. The large one (from the time-picker screens) follows with those pickers.
pub struct Metrics {
    pub sidebar_width: f64,
    pub logo_origin: (f64, f64),
    pub logo_scale: f64,
    pub wordmark_origin: (f64, f64),
    pub wordmark_size: f64,
    pub nav_origin: (f64, f64),
    pub nav_size: (f64, f64),
    pub title_origin: (f64, f64),
    pub button_height: f64,
    pub ok_width: f64,
    pub cancel_width: f64,
    pub button_bottom_inset: f64,
}

impl Metrics {
    pub const SMALL: Metrics = Metrics {
        sidebar_width: 143.0,
        logo_origin: (17.0, 41.0),
        logo_scale: 0.6,
        wordmark_origin: (51.0, 50.0),
        wordmark_size: 20.0,
        nav_origin: (11.0, 78.0),
        nav_size: (124.0, 35.0),
        title_origin: (163.0, 14.0),
        button_height: 35.0,
        ok_width: 102.0,
        cancel_width: 100.0,
        button_bottom_inset: 46.0,
    };
}

pub struct PickerShell {
    pub window: Window,
    /// Where a subclass puts its content: the design's coordinates inside the card.
    pub main_card: Fixed,
    pub side_card: Option<Fixed>,
    pub dark: bool,
    finished: Rc<Cell<bool>>,
    on_finish: FinishCallback,
}

impl PickerShell {
    pub fn new(
        parent: Option<&impl IsA<Window>>,
        title: &str,
        size: (f64, f64),
        main_card: Rect,
        side_card: Option<Rect>,
        nav_title: &str,
    ) -> Self {
        let metrics = &Metrics::SMALL;
        let dark = Settings::default()
            .map(|settings| settings.is_gtk_application_prefer_dark_theme())
            .unwrap_or(false);

        let window = Window::builder()
            .title(title)
            .resizable(false)
            .default_width(size.0 as i32)
            .default_height(size.1 as i32)
            .modal(true)
            .build();
        // The design has no titlebar: the window is the card. An empty header keeps the
        // client-side shadow and rounded corners that an undecorated window loses.
        let header = gtk4::HeaderBar::builder().show_title_buttons(false).build();
        header.set_visible(false);
        window.set_titlebar(Some(&header));
        window.set_decorated(false);
        window.add_css_class("picker");
        if dark {
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
        root.set_size_request(size.0 as i32, size.1 as i32);
        window.set_child(Some(&root));

        // ── sidebar ──────────────────────────────────────────────────────
        let sidebar = Fixed::new();
        sidebar.set_size_request(metrics.sidebar_width as i32, size.1 as i32);
        let art = sidebar_art(metrics, size, dark, composited);
        sidebar.put(&art, 0.0, 0.0);

        let wordmark = Label::new(Some("gosub"));
        wordmark.add_css_class("picker-wordmark");
        wordmark.set_xalign(0.0);
        wordmark.set_attributes(Some(&font_size(metrics.wordmark_size)));
        sidebar.put(
            &wordmark,
            metrics.wordmark_origin.0,
            metrics.wordmark_origin.1 - metrics.wordmark_size * 0.75,
        );

        let nav = nav_button(nav_title, metrics.nav_size);
        sidebar.put(&nav, metrics.nav_origin.0, metrics.nav_origin.1);
        root.put(&sidebar, 0.0, 0.0);

        // ── title ────────────────────────────────────────────────────────
        let title_label = Label::new(Some(title));
        title_label.add_css_class("picker-title");
        title_label.set_xalign(0.0);
        root.put(&title_label, metrics.title_origin.0, metrics.title_origin.1);

        // ── cards ────────────────────────────────────────────────────────
        let main = card(main_card);
        root.put(&main.0, main_card.x, main_card.y);
        let side = side_card.map(|rect| {
            let side = card(rect);
            root.put(&side.0, rect.x, rect.y);
            side.1
        });

        // ── buttons ──────────────────────────────────────────────────────
        let button_y = size.1 - metrics.button_bottom_inset;
        let ok = Button::with_label("Select");
        ok.add_css_class("picker-button");
        ok.add_css_class("select");
        ok.set_size_request(metrics.ok_width as i32, metrics.button_height as i32);
        root.put(&ok, size.0 - 18.0 - metrics.ok_width, button_y);

        let cancel = Button::with_label("Cancel");
        cancel.add_css_class("picker-button");
        cancel.add_css_class("cancel");
        cancel.set_size_request(metrics.cancel_width as i32, metrics.button_height as i32);
        root.put(&cancel, size.0 - 18.0 - metrics.ok_width - 15.0 - metrics.cancel_width, button_y);

        let shell = Self {
            window,
            main_card: main.1,
            side_card: side,
            dark,
            finished: Rc::new(Cell::new(false)),
            on_finish: Rc::new(RefCell::new(None)),
        };

        let finish = {
            let finished = shell.finished.clone();
            let on_finish = shell.on_finish.clone();
            let window = shell.window.clone();
            move |ok: bool| {
                // Select, Cancel, Escape and the window manager's close all land here, and
                // only the first of them answers: the control that asked gets one reply.
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

    pub fn connect_finish(&self, callback: impl Fn(bool) + 'static) {
        *self.on_finish.borrow_mut() = Some(Box::new(callback));
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
fn card(rect: Rect) -> (GtkBox, Fixed) {
    let outer = GtkBox::new(Orientation::Vertical, 0);
    outer.add_css_class("picker-card");
    outer.set_size_request(rect.w as i32, rect.h as i32);
    let inner = Fixed::new();
    inner.set_hexpand(true);
    inner.set_vexpand(true);
    outer.append(&inner);
    (outer, inner)
}

/// Pango attributes for a point size, since CSS cannot size a label the design sizes.
fn font_size(size: f64) -> gtk4::pango::AttrList {
    let attrs = gtk4::pango::AttrList::new();
    attrs.insert(gtk4::pango::AttrSize::new((size * f64::from(gtk4::pango::SCALE)) as i32));
    attrs
}

/// A sidebar section: the design's rainbow disc and a title, in the accent tint when it is
/// the one showing. Only one picker has more than a single section today, but the shell
/// keeps the shape.
fn nav_button(title: &str, size: (f64, f64)) -> Button {
    let row = GtkBox::new(Orientation::Horizontal, 8);
    let disc = DrawingArea::new();
    disc.set_size_request(20, 20);
    disc.set_valign(Align::Center);
    disc.set_draw_func(|_, cr, w, h| {
        // A conic sweep with a white core: `PickerStyle.drawHueDisc`.
        let radius = f64::from(w.min(h)) / 2.0 - 1.0;
        let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
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
    });
    row.append(&disc);
    let label = Label::new(Some(title));
    label.set_xalign(0.0);
    row.append(&label);

    let button = Button::builder().child(&row).build();
    button.add_css_class("picker-nav");
    button.add_css_class("selected");
    button.set_size_request(size.0 as i32, size.1 as i32);
    button
}

/// The sidebar's own drawing: its tint with the window's left corners rounded, the
/// submarine, and the lighthouse fading in at the foot.
fn sidebar_art(metrics: &Metrics, size: (f64, f64), dark: bool, composited: bool) -> DrawingArea {
    let area = DrawingArea::new();
    area.set_size_request(metrics.sidebar_width as i32, size.1 as i32);
    let logo = metrics.logo_origin;
    let scale = metrics.logo_scale;
    let lighthouse = Pixbuf::from_resource("/io/gosub/beacon/assets/picker-sidebar.png").ok();

    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        let radius = if composited { 12.0 } else { 0.0 };
        let (fill, brand) = if dark {
            ((0x23, 0x28, 0x30), (0xC9, 0xD6, 0xF0))
        } else {
            ((0xE4, 0xE9, 0xF3), (0x24, 0x3A, 0x68))
        };
        let rgb = |(r, g, b): (u8, u8, u8)| (f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0);
        let (sr, sg, sb) = rgb(fill);

        cr.save().ok();
        // Left corners rounded to the window's radius, right edge square: the sidebar butts
        // against the content.
        //
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
            let (x, y, rw, rh, r) = (logo.0 + x * scale, logo.1 + y * scale, rw * scale, rh * scale, r * scale);
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
            cr.arc(logo.0 + x * scale, logo.1 + 19.5 * scale, 2.5 * scale, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
    });
    area
}
