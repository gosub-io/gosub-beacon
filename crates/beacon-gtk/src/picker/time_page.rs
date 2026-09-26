//! The time screen: the clock face, the half-day toggle beside it, the number fields under
//! it, and the daypart the chosen time falls in.
//!
//! Ported from `PickerWindow.swift`'s `ClockFaceView`, `MeridiemToggle` and the controller's
//! time section. The face is drawn rather than assembled from widgets — it is a dial with
//! sixty ticks, two rings of numerals and two draggable knobs, none of which is a button.

use gtk4::cairo::Context;
use gtk4::prelude::*;
use gtk4::{DrawingArea, Fixed, GestureDrag, Label};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::date_page::{rounded_rect, text_centered, TextStyle};
use super::datetime::{clock_string, uses_12_hour};
use super::shell::{font_size, Dark};
use super::stepper::StepperField;

type TimeCallback = RefCell<Option<Box<dyn Fn(u32, u32)>>>;
type Action = RefCell<Option<Box<dyn Fn()>>>;
type BoolCallback = RefCell<Option<Box<dyn Fn(bool)>>>;

/// The four parts of the day the caption names.
#[derive(Clone, Copy, PartialEq)]
pub enum Daypart {
    Night,
    Morning,
    Afternoon,
    Evening,
}

impl Daypart {
    pub fn of(hour: u32) -> Self {
        match hour {
            0..=5 => Self::Night,
            6..=11 => Self::Morning,
            12..=17 => Self::Afternoon,
            _ => Self::Evening,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Night => "Night",
            Self::Morning => "Morning",
            Self::Afternoon => "Afternoon",
            Self::Evening => "Evening",
        }
    }

    pub fn range(self) -> &'static str {
        match self {
            Self::Night => "00:00 - 06:00",
            Self::Morning => "06:00 - 12:00",
            Self::Afternoon => "12:00 - 18:00",
            Self::Evening => "18:00 - 24:00",
        }
    }

    /// Sun or moon, and what colour to draw it.
    fn glyph(self) -> (Glyph, (f64, f64, f64)) {
        match self {
            Self::Night => (Glyph::Moon, MOON),
            Self::Morning => (Glyph::Sun, SUNSET),
            Self::Afternoon => (Glyph::Sun, SUN),
            Self::Evening => (Glyph::Moon, MOON),
        }
    }
}

pub const SUN: (f64, f64, f64) = (0.965, 0.706, 0.118);
pub const SUNSET: (f64, f64, f64) = (0.941, 0.541, 0.102);
pub const MOON: (f64, f64, f64) = (0.424, 0.498, 0.769);

#[derive(Clone, Copy, PartialEq)]
pub enum Glyph {
    Sun,
    Moon,
}

/// The design's sun and moon, drawn: no icon theme has a pair that matches, and these sit
/// inside controls that are drawn anyway.
pub fn draw_glyph(cr: &Context, glyph: Glyph, cx: f64, cy: f64, size: f64, colour: (f64, f64, f64)) {
    cr.set_source_rgb(colour.0, colour.1, colour.2);
    match glyph {
        Glyph::Sun => {
            let r = size * 0.28;
            cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            cr.set_line_width(size * 0.08);
            cr.set_line_cap(gtk4::cairo::LineCap::Round);
            for i in 0..8 {
                let angle = f64::from(i) / 8.0 * std::f64::consts::TAU;
                cr.move_to(cx + angle.cos() * r * 1.45, cy + angle.sin() * r * 1.45);
                cr.line_to(cx + angle.cos() * r * 1.95, cy + angle.sin() * r * 1.95);
            }
            let _ = cr.stroke();
        }
        Glyph::Moon => {
            // A crescent: a disc with a second disc taken out of it.
            let r = size * 0.42;
            cr.save().ok();
            cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
            cr.new_sub_path();
            cr.arc(cx + r * 0.55, cy - r * 0.35, r * 0.92, 0.0, std::f64::consts::TAU);
            cr.set_fill_rule(gtk4::cairo::FillRule::EvenOdd);
            let _ = cr.fill();
            cr.restore().ok();
        }
    }
}

// ── the clock face ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Hand {
    Hour,
    Minute,
}

pub struct ClockFace {
    pub area: DrawingArea,
    hour: Cell<u32>,
    minute: Cell<u32>,
    minute_step: Cell<u32>,
    twenty_four_hour: Cell<bool>,
    dragging: Cell<Option<Hand>>,
    scale: f64,
    on_change: TimeCallback,
}

impl ClockFace {
    pub fn new(size: f64, scale: f64, dark: Dark) -> Rc<Self> {
        let area = DrawingArea::new();
        area.set_size_request((size * scale) as i32, (size * scale) as i32);

        let face = Rc::new(Self {
            area,
            hour: Cell::new(10),
            minute: Cell::new(30),
            minute_step: Cell::new(1),
            twenty_four_hour: Cell::new(false),
            dragging: Cell::new(None),
            scale,
            on_change: RefCell::new(None),
        });

        let drawing = Rc::downgrade(&face);
        face.area.set_draw_func(move |_, cr, w, h| {
            if let Some(face) = drawing.upgrade() {
                face.draw(cr, f64::from(w), f64::from(h), dark.get());
            }
        });

        let drag = GestureDrag::new();
        let start = Rc::new(Cell::new((0.0, 0.0)));
        drag.connect_drag_begin({
            let this = Rc::downgrade(&face);
            let start = start.clone();
            move |_, x, y| {
                start.set((x, y));
                if let Some(face) = this.upgrade() {
                    face.press(x, y);
                }
            }
        });
        drag.connect_drag_update({
            let this = Rc::downgrade(&face);
            let start = start.clone();
            move |_, dx, dy| {
                if let Some(face) = this.upgrade() {
                    let (x, y) = start.get();
                    face.drag_to(x + dx, y + dy);
                }
            }
        });
        drag.connect_drag_end({
            let this = Rc::downgrade(&face);
            move |_, _, _| {
                if let Some(face) = this.upgrade() {
                    face.dragging.set(None);
                }
            }
        });
        face.area.add_controller(drag);
        face
    }

    pub fn connect_change(&self, callback: impl Fn(u32, u32) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(callback));
    }

    pub fn set(&self, hour: u32, minute: u32) {
        self.hour.set(hour);
        self.minute.set(minute);
        self.area.queue_draw();
    }

    pub fn set_twenty_four_hour(&self, on: bool) {
        self.twenty_four_hour.set(on);
        self.area.queue_draw();
    }

    pub fn set_minute_step(&self, step: u32) {
        self.minute_step.set(step.max(1));
    }

    fn radius(&self) -> f64 {
        f64::from(self.area.width().max(1)) / 2.0
    }

    fn centre(&self) -> (f64, f64) {
        (
            f64::from(self.area.width().max(1)) / 2.0,
            f64::from(self.area.height().max(1)) / 2.0,
        )
    }

    /// The hour hand sits on its hour, not part-way to the next as a real clock's would: it
    /// is a control, and dragging the minutes must not move it.
    fn hour_angle(&self) -> f64 {
        f64::from(self.hour.get() % 12) / 12.0 * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2
    }

    fn minute_angle(&self) -> f64 {
        f64::from(self.minute.get()) / 60.0 * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2
    }

    fn point(&self, angle: f64, radius: f64) -> (f64, f64) {
        let (cx, cy) = self.centre();
        (cx + angle.cos() * radius, cy + angle.sin() * radius)
    }

    fn draw(&self, cr: &Context, w: f64, h: f64, dark: bool) {
        let s = |v: f64| v * self.scale;
        let radius = w.min(h) / 2.0;
        let (cx, cy) = (w / 2.0, h / 2.0);
        let navy = if dark { (0.835, 0.859, 0.898) } else { (0.082, 0.165, 0.329) };
        let muted = if dark { (0.604, 0.647, 0.710) } else { (0.420, 0.478, 0.565) };
        let blue = if dark { (0.298, 0.553, 1.0) } else { (0.039, 0.451, 0.980) };

        // The face: a soft disc with a lighter middle, as the screens draw it.
        let gradient = gtk4::cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, radius);
        if dark {
            gradient.add_color_stop_rgb(0.0, 0.188, 0.212, 0.247);
            gradient.add_color_stop_rgb(1.0, 0.165, 0.184, 0.220);
        } else {
            gradient.add_color_stop_rgb(0.0, 1.0, 1.0, 1.0);
            gradient.add_color_stop_rgb(1.0, 0.933, 0.953, 0.984);
        }
        let _ = cr.set_source(&gradient);
        cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        if dark {
            cr.set_source_rgb(0.227, 0.255, 0.298);
        } else {
            cr.set_source_rgb(0.863, 0.894, 0.941);
        }
        cr.set_line_width(1.0);
        cr.arc(cx, cy, radius - 0.5, 0.0, std::f64::consts::TAU);
        let _ = cr.stroke();

        // Sixty ticks, every fifth one longer.
        cr.set_source_rgb(navy.0, navy.1, navy.2);
        cr.set_line_cap(gtk4::cairo::LineCap::Round);
        for i in 0..60 {
            let angle = f64::from(i) / 60.0 * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
            let major = i % 5 == 0;
            cr.set_line_width(if major { s(2.5) } else { s(1.2) });
            let inner = radius - if major { s(20.0) } else { s(14.0) };
            let (x1, y1) = (cx + angle.cos() * inner, cy + angle.sin() * inner);
            let (x2, y2) = (cx + angle.cos() * (radius - s(6.0)), cy + angle.sin() * (radius - s(6.0)));
            cr.move_to(x1, y1);
            cr.line_to(x2, y2);
            let _ = cr.stroke();
        }

        let twenty_four = self.twenty_four_hour.get();
        let hour = self.hour.get();
        let hour_numerals = radius - s(46.0);
        let minute_numerals = radius - s(95.0);
        for h in 1..=12u32 {
            let angle = f64::from(h) / 12.0 * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
            // 24 h: the chosen half's own hours, 00-11 or 12-23, with the top numeral 00 or 12.
            let label = if twenty_four {
                format!("{:02}", (h % 12) + if hour >= 12 { 12 } else { 0 })
            } else {
                h.to_string()
            };
            let (x, y) = (cx + angle.cos() * hour_numerals, cy + angle.sin() * hour_numerals);
            text_centered(cr, &label, x, y, TextStyle::new(s(22.0), true, navy));
        }
        for m in (0..60).step_by(5) {
            let angle = f64::from(m) / 60.0 * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
            let (x, y) = (cx + angle.cos() * minute_numerals, cy + angle.sin() * minute_numerals);
            text_centered(cr, &format!("{m:02}"), x, y, TextStyle::new(s(14.0), false, muted));
        }

        // The hands: the short one is the hour and reaches the inner ring.
        let hour_knob = self.point(self.hour_angle(), radius - s(95.0));
        let minute_knob = self.point(self.minute_angle(), radius - s(46.0));
        cr.set_source_rgb(blue.0, blue.1, blue.2);
        cr.set_line_width(s(5.0));
        for (x, y) in [hour_knob, minute_knob] {
            cr.move_to(cx, cy);
            cr.line_to(x, y);
            let _ = cr.stroke();
        }
        let hour_text = if twenty_four {
            format!("{hour:02}")
        } else if hour.is_multiple_of(12) {
            "12".to_string()
        } else {
            (hour % 12).to_string()
        };
        self.knob(cr, hour_knob, &hour_text, blue, s(21.0), s(20.0));
        self.knob(cr, minute_knob, &format!("{:02}", self.minute.get()), blue, s(21.0), s(20.0));
        cr.set_source_rgb(navy.0, navy.1, navy.2);
        cr.arc(cx, cy, s(12.0), 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    }

    fn knob(&self, cr: &Context, at: (f64, f64), text: &str, fill: (f64, f64, f64), radius: f64, size: f64) {
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.18);
        cr.arc(at.0, at.1 + 2.0, radius, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        cr.set_source_rgb(fill.0, fill.1, fill.2);
        cr.arc(at.0, at.1, radius, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        text_centered(cr, text, at.0, at.1, TextStyle::new(size, true, (1.0, 1.0, 1.0)));
    }

    fn press(&self, x: f64, y: f64) {
        let (cx, cy) = self.centre();
        let radius = self.radius();
        let distance = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        let from_centre = distance((x, y), (cx, cy));
        if from_centre > radius + 4.0 {
            return;
        }
        let hour_ring = radius - 95.0 * self.scale;
        let minute_ring = radius - 46.0 * self.scale;
        let to_hour = distance((x, y), self.point(self.hour_angle(), hour_ring));
        let to_minute = distance((x, y), self.point(self.minute_angle(), minute_ring));
        // The knob under the pointer takes the drag; a press elsewhere goes to the hand
        // whose reach it landed on -- the long minute hand outside, the short hour hand in.
        self.dragging.set(Some(if to_hour.min(to_minute) < 30.0 * self.scale {
            if to_hour < to_minute {
                Hand::Hour
            } else {
                Hand::Minute
            }
        } else if from_centre > (hour_ring + minute_ring) / 2.0 {
            Hand::Minute
        } else {
            Hand::Hour
        }));
        self.drag_to(x, y);
    }

    fn drag_to(&self, x: f64, y: f64) {
        let Some(hand) = self.dragging.get() else { return };
        let (cx, cy) = self.centre();
        let angle = (y - cy).atan2(x - cx) + std::f64::consts::FRAC_PI_2;
        let turn = if angle < 0.0 { angle + std::f64::consts::TAU } else { angle } / std::f64::consts::TAU;
        match hand {
            Hand::Hour => {
                let half = if self.hour.get() >= 12 { 12 } else { 0 };
                self.hour.set(half + (turn * 12.0).round() as u32 % 12);
            }
            Hand::Minute => {
                let step = self.minute_step.get().max(1);
                let minute = (turn * 60.0).round() as u32 % 60;
                self.minute.set((minute / step) * step);
            }
        }
        self.area.queue_draw();
        if let Some(callback) = self.on_change.borrow().as_ref() {
            callback(self.hour.get(), self.minute.get());
        }
    }
}

// ── the half-day toggle ─────────────────────────────────────────────────────

/// The column beside the face: two halves in one rounded border. In 12 h mode PM (sun) sits
/// above AM (moon); in 24 h mode the halves are 00-11 and 12-23.
pub struct MeridiemToggle {
    pub area: DrawingArea,
    is_top: Cell<bool>,
    twelve_hour: Cell<bool>,
    scale: f64,
    on_change: BoolCallback,
}

impl MeridiemToggle {
    pub fn new(width: f64, height: f64, scale: f64, dark: Dark) -> Rc<Self> {
        let area = DrawingArea::new();
        area.set_size_request((width * scale) as i32, (height * scale) as i32);
        let toggle = Rc::new(Self {
            area,
            is_top: Cell::new(false),
            twelve_hour: Cell::new(true),
            scale,
            on_change: RefCell::new(None),
        });

        let drawing = Rc::downgrade(&toggle);
        toggle.area.set_draw_func(move |_, cr, w, h| {
            if let Some(toggle) = drawing.upgrade() {
                toggle.draw(cr, f64::from(w), f64::from(h), dark.get());
            }
        });

        let click = gtk4::GestureClick::new();
        click.connect_released({
            let this = Rc::downgrade(&toggle);
            move |_, _, _, y| {
                let Some(toggle) = this.upgrade() else { return };
                let top = y < 127.0 * toggle.scale;
                if top == toggle.is_top.get() {
                    return;
                }
                toggle.is_top.set(top);
                toggle.area.queue_draw();
                toggle.emit(top);
            }
        });
        toggle.area.add_controller(click);
        toggle
    }

    pub fn connect_change(&self, callback: impl Fn(bool) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(callback));
    }

    fn emit(&self, top: bool) {
        if let Some(callback) = self.on_change.borrow().as_ref() {
            callback(top);
        }
    }

    pub fn set(&self, is_top: bool, twelve_hour: bool) {
        self.is_top.set(is_top);
        self.twelve_hour.set(twelve_hour);
        self.area.queue_draw();
    }

    fn draw(&self, cr: &Context, w: f64, h: f64, dark: bool) {
        let s = |v: f64| v * self.scale;
        let brand = if dark { (0.788, 0.839, 0.941) } else { (0.141, 0.227, 0.408) };
        rounded_rect(cr, 0.5, 0.5, w - 1.0, h - 1.0, s(14.0));
        if dark {
            cr.set_source_rgb(0.165, 0.184, 0.220);
        } else {
            cr.set_source_rgb(1.0, 1.0, 1.0);
        }
        let _ = cr.fill_preserve();
        if dark {
            cr.set_source_rgb(0.227, 0.255, 0.298);
        } else {
            cr.set_source_rgb(0.780, 0.831, 0.910);
        }
        cr.set_line_width(1.0);
        let _ = cr.stroke();

        let split = s(126.0);
        let twelve = self.twelve_hour.get();
        for (y, height, top) in [(0.0, split, true), (s(128.0), h - s(128.0), false)] {
            let on = top == self.is_top.get();
            let (label, glyph, tint) = if twelve {
                if top {
                    ("PM", Glyph::Sun, SUN)
                } else {
                    ("AM", Glyph::Moon, MOON)
                }
            } else if top {
                ("00-11", Glyph::Moon, MOON)
            } else {
                ("12-23", Glyph::Sun, SUN)
            };
            if on {
                cr.set_source_rgb(0.039, 0.451, 0.980);
                rounded_rect(cr, 0.0, y, w, height, s(14.0));
                let _ = cr.fill();
            }
            let glyph_colour = if on { (1.0, 1.0, 1.0) } else { tint };
            draw_glyph(cr, glyph, w / 2.0, y + s(45.0), s(34.0), glyph_colour);
            let colour = if on { (1.0, 1.0, 1.0) } else { brand };
            text_centered(
                cr,
                label,
                w / 2.0,
                y + height - s(30.0),
                TextStyle::new(s(if twelve { 24.0 } else { 20.0 }), true, colour),
            );
        }
    }
}

// ── the 12 h / 24 h toggle ──────────────────────────────────────────────────

/// The small segmented control in the card's corner. Its choice is remembered for the next
/// picker, as the Mac remembers it.
pub struct ClockFormatToggle {
    pub area: DrawingArea,
    scale: f64,
    on_change: Action,
}

impl ClockFormatToggle {
    pub fn new(width: f64, height: f64, scale: f64, dark: Dark) -> Rc<Self> {
        let area = DrawingArea::new();
        area.set_size_request((width * scale) as i32, (height * scale) as i32);
        let toggle = Rc::new(Self {
            area,
            scale,
            on_change: RefCell::new(None),
        });

        let drawing = Rc::downgrade(&toggle);
        toggle.area.set_draw_func(move |_, cr, w, h| {
            if let Some(toggle) = drawing.upgrade() {
                toggle.draw(cr, f64::from(w), f64::from(h), dark.get());
            }
        });

        let click = gtk4::GestureClick::new();
        click.connect_released({
            let this = Rc::downgrade(&toggle);
            move |_, _, x, _| {
                let Some(toggle) = this.upgrade() else { return };
                let width = f64::from(toggle.area.width().max(1));
                let chosen = if x < width / 2.0 { 12 } else { 24 };
                if (chosen == 12) == uses_12_hour() {
                    return;
                }
                super::datetime::set_clock_format(chosen);
                toggle.area.queue_draw();
                toggle.emit();
            }
        });
        toggle.area.add_controller(click);
        toggle
    }

    pub fn connect_change(&self, callback: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(callback));
    }

    fn emit(&self) {
        if let Some(callback) = self.on_change.borrow().as_ref() {
            callback();
        }
    }

    fn draw(&self, cr: &Context, w: f64, h: f64, dark: bool) {
        let s = |v: f64| v * self.scale;
        rounded_rect(cr, 0.0, 0.0, w, h, s(8.0));
        if dark {
            cr.set_source_rgb(0.180, 0.204, 0.243);
        } else {
            cr.set_source_rgb(0.910, 0.929, 0.961);
        }
        let _ = cr.fill();
        let twelve = uses_12_hour();
        let brand = if dark { (0.788, 0.839, 0.941) } else { (0.141, 0.227, 0.408) };
        let muted = if dark { (0.604, 0.647, 0.710) } else { (0.420, 0.478, 0.565) };
        for (i, label) in ["12 h", "24 h"].iter().enumerate() {
            let half = w / 2.0 - s(3.0);
            let x = s(3.0) + i as f64 * half;
            let on = (i == 0) == twelve;
            if on {
                if dark {
                    cr.set_source_rgb(0.165, 0.184, 0.220);
                } else {
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                }
                rounded_rect(cr, x, s(3.0), half, h - s(6.0), s(6.0));
                let _ = cr.fill();
            }
            text_centered(
                cr,
                label,
                x + half / 2.0,
                h / 2.0,
                TextStyle::new(s(12.0), on, if on { brand } else { muted }),
            );
        }
    }
}

// ── the page ────────────────────────────────────────────────────────────────

pub struct TimePage {
    pub widget: Fixed,
    pub clock: Rc<ClockFace>,
    pub meridiem: Rc<MeridiemToggle>,
    hour_field: Rc<StepperField>,
    minute_field: Rc<StepperField>,
    second_field: Rc<StepperField>,
    caption_glyph: DrawingArea,
    caption_time: Label,
    caption_part: Label,
    shows_seconds: bool,
    daypart: Cell<Daypart>,
    on_fields_changed: Action,
}

impl TimePage {
    pub fn new(shows_seconds: bool, scale: f64, dark: Dark) -> Rc<Self> {
        let s = |v: f64| v * scale;
        let widget = Fixed::new();
        widget.set_size_request(s(690.0) as i32, s(807.0) as i32);

        let clock = ClockFace::new(448.0, scale, dark.clone());
        widget.put(&clock.area, s(71.0), s(127.0));

        let meridiem = MeridiemToggle::new(81.0, 258.0, scale, dark);
        widget.put(&meridiem.area, s(549.0), s(198.0));

        // Three fields share the row when seconds are shown; otherwise hour and minute sit
        // either side of a colon, as the screens draw them.
        let layout: Vec<(&str, f64, f64)> = if shows_seconds {
            vec![("Hour", 109.0, 140.0), ("Minute", 279.0, 140.0), ("Second", 449.0, 140.0)]
        } else {
            vec![("Hour", 109.0, 181.0), ("Minute", 329.0, 204.0)]
        };
        let hour_field = StepperField::new(layout[0].2, 60.0, scale);
        let minute_field = StepperField::new(layout[1].2, 60.0, scale);
        let second_field = StepperField::new(if shows_seconds { 140.0 } else { 1.0 }, 60.0, scale);
        let fields = [&hour_field, &minute_field, &second_field];
        for (i, (name, x, _)) in layout.iter().enumerate() {
            let label = Label::new(Some(name));
            label.add_css_class("picker-field-label");
            label.set_xalign(0.0);
            label.set_attributes(Some(&font_size(s(16.0))));
            widget.put(&label, s(*x), s(615.0));
            widget.put(&fields[i].widget, s(*x), s(647.0));
        }
        if !shows_seconds {
            let colon = Label::new(Some(":"));
            colon.add_css_class("picker-caption-title");
            colon.set_attributes(Some(&font_size(s(26.0))));
            widget.put(&colon, s(299.0), s(660.0));
        }

        let caption_glyph = DrawingArea::new();
        caption_glyph.set_size_request(s(40.0) as i32, s(40.0) as i32);
        widget.put(&caption_glyph, s(42.0), s(728.0));

        let caption_time = Label::new(None);
        caption_time.add_css_class("picker-caption-title");
        caption_time.set_xalign(0.0);
        caption_time.set_attributes(Some(&font_size(s(18.0))));
        widget.put(&caption_time, s(108.0), s(724.0));

        let caption_part = Label::new(None);
        caption_part.add_css_class("picker-caption-title");
        caption_part.set_xalign(0.0);
        caption_part.set_attributes(Some(&font_size(s(18.0))));
        widget.put(&caption_part, s(108.0), s(752.0));

        let page = Rc::new(Self {
            widget,
            clock,
            meridiem,
            hour_field,
            minute_field,
            second_field,
            caption_glyph,
            caption_time,
            caption_part,
            shows_seconds,
            daypart: Cell::new(Daypart::Morning),
            on_fields_changed: RefCell::new(None),
        });

        page.minute_field.set_range(0, 59);
        page.minute_field.set_wraps(true);
        page.second_field.set_range(0, 59);
        page.second_field.set_wraps(true);
        page.hour_field.set_wraps(true);

        for field in [&page.hour_field, &page.minute_field, &page.second_field] {
            let this = Rc::downgrade(&page);
            field.connect_change(move |_| {
                if let Some(page) = this.upgrade() {
                    if let Some(callback) = page.on_fields_changed.borrow().as_ref() {
                        callback();
                    }
                }
            });
        }

        let drawing = Rc::downgrade(&page);
        page.caption_glyph.set_draw_func(move |_, cr, w, h| {
            let Some(page) = drawing.upgrade() else { return };
            let (glyph, colour) = page.daypart.get().glyph();
            draw_glyph(cr, glyph, f64::from(w) / 2.0, f64::from(h) / 2.0, f64::from(w.min(h)), colour);
        });

        page
    }

    pub fn connect_fields_changed(&self, callback: impl Fn() + 'static) {
        *self.on_fields_changed.borrow_mut() = Some(Box::new(callback));
    }

    pub fn set_minute_step(&self, step: u32) {
        self.minute_field.set_step(step as i32);
        self.clock.set_minute_step(step);
    }

    /// What the fields hold, in 24-hour terms.
    pub fn fields(&self, was_pm: bool) -> (u32, u32, Option<u32>) {
        let raw = self.hour_field.value().max(0) as u32;
        let hour = if uses_12_hour() {
            (raw % 12) + if was_pm { 12 } else { 0 }
        } else {
            raw
        };
        let second = self.shows_seconds.then(|| self.second_field.value().max(0) as u32);
        (hour, self.minute_field.value().max(0) as u32, second)
    }

    /// Put every part of the screen back in step with the value.
    pub fn refresh(&self, hour: u32, minute: u32, second: Option<u32>, caption: &str) {
        let twelve = uses_12_hour();
        self.clock.set_twenty_four_hour(!twelve);
        self.clock.set(hour, minute);
        self.meridiem.set(if twelve { hour >= 12 } else { hour < 12 }, twelve);
        self.hour_field.set_range(if twelve { 1 } else { 0 }, if twelve { 12 } else { 23 });
        self.hour_field.set_value(if twelve {
            if hour.is_multiple_of(12) {
                12
            } else {
                (hour % 12) as i32
            }
        } else {
            hour as i32
        });
        self.minute_field.set_value(minute as i32);
        self.second_field.set_value(second.unwrap_or(0) as i32);
        self.caption_time.set_text(caption);
        let part = Daypart::of(hour);
        self.daypart.set(part);
        self.caption_part.set_text(&format!("{} ({})", part.name(), part.range()));
        self.caption_glyph.queue_draw();
    }
}

/// "Selected time: 10:30 PM", the caption when the picker edits a time alone.
pub fn time_caption(hour: u32, minute: u32) -> String {
    format!("Selected time: {}", clock_string(hour, minute))
}
