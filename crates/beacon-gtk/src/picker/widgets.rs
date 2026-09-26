//! The drawn parts of the pickers: the saturation/value plane, the hue and alpha strips,
//! and the round swatches. Everything else in the design is a styled GTK widget.
//!
//! These are `DrawingArea`s rather than subclassed widgets: each is a handful of cairo
//! fills plus a drag gesture, and a plain area keeps the state in one `Rc` the controller
//! can read. Ported from `ColorControls.swift`.

use gtk4::cairo::{Context, LinearGradient};
use gtk4::prelude::*;
use gtk4::{gdk, glib, DrawingArea, EventControllerKey, GestureDrag};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::color::CssColor;

/// The plane reports a saturation and a value; a strip reports one position.
type PlaneCallback = Rc<RefCell<Option<Box<dyn Fn(f64, f64)>>>>;
type StripCallback = Rc<RefCell<Option<Box<dyn Fn(f64)>>>>;

/// The corner radius the design gives the plane and the strips.
const RADIUS: f64 = 10.0;

/// Add a rounded rectangle to the current path.
fn rounded_rect(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
}

/// The checkerboard translucency is shown against, as every picker draws it.
pub fn checkerboard(cr: &Context, x: f64, y: f64, w: f64, h: f64, cell: f64) {
    cr.set_source_rgb(0xF0 as f64 / 255.0, 0xF0 as f64 / 255.0, 0xF2 as f64 / 255.0);
    let _ = cr.paint();
    cr.set_source_rgb(0xB7 as f64 / 255.0, 0xB9 as f64 / 255.0, 0xC8 as f64 / 255.0);
    let mut row = 0;
    let mut cy = y;
    while cy < y + h {
        let mut cx = x + if row % 2 == 0 { 0.0 } else { cell };
        while cx < x + w {
            cr.rectangle(cx, cy, cell.min(x + w - cx), cell.min(y + h - cy));
            let _ = cr.fill();
            cx += cell * 2.0;
        }
        cy += cell;
        row += 1;
    }
}

/// The ring every knob in the design wears: white, 4 wide, over a soft shadow. Cairo has no
/// blur, so the shadow is two translucent rings just outside the white one — close enough at
/// this size, and it keeps the colour under the knob visible, which is the point of a ring.
fn knob(cr: &Context, x: f64, y: f64, radius: f64) {
    for (offset, alpha) in [(2.5, 0.10), (1.5, 0.18)] {
        cr.set_source_rgba(0.0, 0.0, 0.0, alpha);
        cr.set_line_width(4.0 + offset);
        cr.arc(x, y + 1.0, radius, 0.0, std::f64::consts::TAU);
        let _ = cr.stroke();
    }
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(4.0);
    cr.arc(x, y, radius, 0.0, std::f64::consts::TAU);
    let _ = cr.stroke();
}

// ── the saturation/value plane ──────────────────────────────────────────────

/// Hue across the top, white at the left, black at the bottom.
///
/// Drawn as two gradients over a flat hue fill rather than a bitmap of every pixel, so a
/// drag redraws at the display's rate for the cost of three fills.
pub struct SvPlane {
    pub area: DrawingArea,
    state: Rc<Cell<(f64, f64, f64)>>,
    on_change: PlaneCallback,
}

impl SvPlane {
    pub fn new(width: i32, height: i32, knob_radius: f64) -> Self {
        let area = DrawingArea::new();
        area.set_size_request(width, height);
        area.set_can_focus(true);
        area.set_focusable(true);
        // Hue, saturation, value. The hue is held here rather than derived from the colour:
        // a grey has none, and the plane must not jump to red when the value hits black.
        let state = Rc::new(Cell::new((0.0_f64, 0.0_f64, 1.0_f64)));
        let on_change: PlaneCallback = Rc::new(RefCell::new(None));

        area.set_draw_func({
            let state = state.clone();
            move |_, cr, w, h| {
                let (hue, saturation, value) = state.get();
                let (w, h) = (f64::from(w), f64::from(h));
                cr.save().ok();
                rounded_rect(cr, 0.0, 0.0, w, h, RADIUS);
                cr.clip();

                let full = CssColor::from_hsv(hue, 1.0, 1.0, 1.0);
                cr.set_source_rgb(full.r, full.g, full.b);
                let _ = cr.paint();

                let to_white = LinearGradient::new(0.0, 0.0, w, 0.0);
                to_white.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 1.0);
                to_white.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
                let _ = cr.set_source(&to_white);
                let _ = cr.paint();

                let to_black = LinearGradient::new(0.0, 0.0, 0.0, h);
                to_black.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 0.0);
                to_black.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 1.0);
                let _ = cr.set_source(&to_black);
                let _ = cr.paint();

                cr.restore().ok();
                knob(cr, saturation * w, (1.0 - value) * h, knob_radius);
            }
        });

        let plane = Self { area, state, on_change };
        plane.wire_drag();
        plane.wire_keys();
        plane
    }

    fn wire_drag(&self) {
        let drag = GestureDrag::new();
        let state = self.state.clone();
        let on_change = self.on_change.clone();
        let area = self.area.clone();
        // A press picks, and the drag carries on from there; both go through the same path
        // so a click and a drag cannot disagree.
        let pick = move |x: f64, y: f64| {
            let w = f64::from(area.width()).max(1.0);
            let h = f64::from(area.height()).max(1.0);
            let (hue, _, _) = state.get();
            let s = (x / w).clamp(0.0, 1.0);
            let v = (1.0 - y / h).clamp(0.0, 1.0);
            state.set((hue, s, v));
            area.queue_draw();
            if let Some(callback) = on_change.borrow().as_ref() {
                callback(s, v);
            }
        };
        let start = Rc::new(Cell::new((0.0, 0.0)));
        drag.connect_drag_begin({
            let pick = pick.clone();
            let start = start.clone();
            move |_, x, y| {
                start.set((x, y));
                pick(x, y);
            }
        });
        drag.connect_drag_update({
            let start = start.clone();
            move |_, dx, dy| {
                let (x, y) = start.get();
                pick(x + dx, y + dy);
            }
        });
        self.area.add_controller(drag);
    }

    /// Arrow keys nudge by 1%, Shift by 10%: a mouse can land near a colour, a keyboard can
    /// land on it.
    fn wire_keys(&self) {
        let keys = EventControllerKey::new();
        let state = self.state.clone();
        let on_change = self.on_change.clone();
        let area = self.area.clone();
        keys.connect_key_pressed(move |_, key, _, modifier| {
            let step = if modifier.contains(gdk::ModifierType::SHIFT_MASK) {
                0.1
            } else {
                0.01
            };
            let (hue, mut s, mut v) = state.get();
            match key {
                gdk::Key::Left => s = (s - step).max(0.0),
                gdk::Key::Right => s = (s + step).min(1.0),
                gdk::Key::Up => v = (v + step).min(1.0),
                gdk::Key::Down => v = (v - step).max(0.0),
                _ => return glib::Propagation::Proceed,
            }
            state.set((hue, s, v));
            area.queue_draw();
            if let Some(callback) = on_change.borrow().as_ref() {
                callback(s, v);
            }
            glib::Propagation::Stop
        });
        self.area.add_controller(keys);
    }

    pub fn connect_change(&self, callback: impl Fn(f64, f64) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(callback));
    }

    pub fn set_hue(&self, hue: f64) {
        let (_, s, v) = self.state.get();
        self.state.set((hue, s, v));
        self.area.queue_draw();
    }

    pub fn set_position(&self, saturation: f64, value: f64) {
        let (hue, _, _) = self.state.get();
        self.state.set((hue, saturation, value));
        self.area.queue_draw();
    }
}

// ── the hue and alpha strips ────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum StripKind {
    Hue,
    Alpha,
}

/// A vertical strip with a draggable knob. One type, because the only difference between
/// the hue spectrum and the alpha ramp is what the gradient is made of.
pub struct ColorStrip {
    pub area: DrawingArea,
    /// 0..=1 from the top: hue/360, or 1 - alpha (opaque at the top, as in every picker).
    position: Rc<Cell<f64>>,
    tint: Rc<Cell<CssColor>>,
    on_change: StripCallback,
}

impl ColorStrip {
    pub fn new(kind: StripKind, width: i32, height: i32, knob_radius: f64) -> Self {
        let area = DrawingArea::new();
        area.set_size_request(width, height);
        area.set_can_focus(true);
        area.set_focusable(true);
        let position = Rc::new(Cell::new(0.0));
        let tint = Rc::new(Cell::new(CssColor::BLACK));
        let on_change: StripCallback = Rc::new(RefCell::new(None));

        area.set_draw_func({
            let position = position.clone();
            let tint = tint.clone();
            move |_, cr, w, h| {
                let (w, h) = (f64::from(w), f64::from(h));
                cr.save().ok();
                rounded_rect(cr, 0.0, 0.0, w, h, RADIUS);
                cr.clip();

                let gradient = LinearGradient::new(0.0, 0.0, 0.0, h);
                match kind {
                    StripKind::Hue => {
                        for step in 0..=6 {
                            let c = CssColor::from_hsv(f64::from(step) * 60.0, 1.0, 1.0, 1.0);
                            gradient.add_color_stop_rgb(f64::from(step) / 6.0, c.r, c.g, c.b);
                        }
                    }
                    StripKind::Alpha => {
                        checkerboard(cr, 0.0, 0.0, w, h, 20.0);
                        let c = tint.get().opaque();
                        gradient.add_color_stop_rgba(0.0, c.r, c.g, c.b, 1.0);
                        gradient.add_color_stop_rgba(1.0, c.r, c.g, c.b, 0.0);
                    }
                }
                let _ = cr.set_source(&gradient);
                let _ = cr.paint();

                cr.restore().ok();
                knob(cr, w / 2.0, position.get() * h, knob_radius);
            }
        });

        let strip = Self {
            area,
            position,
            tint,
            on_change,
        };
        strip.wire_drag();
        strip.wire_keys();
        strip
    }

    fn wire_drag(&self) {
        let drag = GestureDrag::new();
        let position = self.position.clone();
        let on_change = self.on_change.clone();
        let area = self.area.clone();
        let pick = move |y: f64| {
            let h = f64::from(area.height()).max(1.0);
            let p = (y / h).clamp(0.0, 1.0);
            position.set(p);
            area.queue_draw();
            if let Some(callback) = on_change.borrow().as_ref() {
                callback(p);
            }
        };
        let start = Rc::new(Cell::new(0.0));
        drag.connect_drag_begin({
            let pick = pick.clone();
            let start = start.clone();
            move |_, _, y| {
                start.set(y);
                pick(y);
            }
        });
        drag.connect_drag_update({
            let start = start.clone();
            move |_, _, dy| pick(start.get() + dy)
        });
        self.area.add_controller(drag);
    }

    fn wire_keys(&self) {
        let keys = EventControllerKey::new();
        let position = self.position.clone();
        let on_change = self.on_change.clone();
        let area = self.area.clone();
        keys.connect_key_pressed(move |_, key, _, modifier| {
            let step = if modifier.contains(gdk::ModifierType::SHIFT_MASK) {
                0.1
            } else {
                0.01
            };
            let p = match key {
                gdk::Key::Up => (position.get() - step).max(0.0),
                gdk::Key::Down => (position.get() + step).min(1.0),
                _ => return glib::Propagation::Proceed,
            };
            position.set(p);
            area.queue_draw();
            if let Some(callback) = on_change.borrow().as_ref() {
                callback(p);
            }
            glib::Propagation::Stop
        });
        self.area.add_controller(keys);
    }

    pub fn connect_change(&self, callback: impl Fn(f64) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(callback));
    }

    pub fn set_position(&self, position: f64) {
        self.position.set(position.clamp(0.0, 1.0));
        self.area.queue_draw();
    }

    pub fn set_tint(&self, tint: CssColor) {
        self.tint.set(tint);
        self.area.queue_draw();
    }
}

// ── the round swatches ──────────────────────────────────────────────────────

/// One quick swatch: filled with its colour, ringed when it is the current one, or drawn as
/// a "+" when it is the button that adds one.
pub fn swatch(size: i32, color: Option<CssColor>, current: Rc<Cell<bool>>, dark: bool) -> DrawingArea {
    let area = DrawingArea::new();
    area.set_size_request(size, size);
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        let radius = (w.min(h) - 5.0) / 2.0;
        let (cx, cy) = (w / 2.0, h / 2.0);
        let Some(color) = color else {
            // The adder: a soft disc with a plus through it.
            let (fill, border) = if dark { (0x2E343E, 0x3A414C) } else { (0xEEF1F5, 0xD1D7E0) };
            let rgb = |v: u32| {
                (
                    f64::from((v >> 16) & 0xff) / 255.0,
                    f64::from((v >> 8) & 0xff) / 255.0,
                    f64::from(v & 0xff) / 255.0,
                )
            };
            let (r, g, b) = rgb(fill);
            cr.set_source_rgb(r, g, b);
            cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            let (r, g, b) = rgb(border);
            cr.set_source_rgb(r, g, b);
            cr.set_line_width(1.0);
            cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
            let _ = cr.stroke();
            let (r, g, b) = rgb(if dark { 0x9AA5B5 } else { 0x6B7A90 });
            cr.set_source_rgb(r, g, b);
            cr.set_line_width(1.5);
            cr.move_to(cx - 6.0, cy);
            cr.line_to(cx + 6.0, cy);
            cr.move_to(cx, cy - 6.0);
            cr.line_to(cx, cy + 6.0);
            let _ = cr.stroke();
            return;
        };

        if color.a < 1.0 {
            cr.save().ok();
            cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
            cr.clip();
            checkerboard(cr, 0.0, 0.0, w, h, 6.0);
            cr.restore().ok();
        }
        cr.set_source_rgba(color.r, color.g, color.b, color.a);
        cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.12);
        cr.set_line_width(1.0);
        cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
        let _ = cr.stroke();
        if current.get() {
            let (r, g, b) = if dark { (0x4C, 0x8D, 0xFF) } else { (0x23, 0x6C, 0xFF) };
            cr.set_source_rgb(f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0);
            cr.set_line_width(3.0);
            cr.arc(cx, cy, radius + 1.0, 0.0, std::f64::consts::TAU);
            let _ = cr.stroke();
        }
    });
    area
}

/// The small colour chip each row of the CSS-colour list carries.
pub fn row_swatch(width: i32, height: i32, color: CssColor) -> DrawingArea {
    let area = DrawingArea::new();
    area.set_size_request(width, height);
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        rounded_rect(cr, 0.5, 0.5, w - 1.0, h - 1.0, 7.0);
        cr.set_source_rgba(color.r, color.g, color.b, color.a);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.10);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    area
}
