//! The month & year screen: a 4 × 3 grid of months, a year stepper, and a 3 × 3 grid of
//! years around the shown one. The arrows step the year.
//!
//! Ported from `PickerWindow.swift`'s `MonthYearPage`. The date picker drills down into it
//! from the calendar's title; a month input opens here and never leaves.

use gtk4::cairo::Context;
use gtk4::prelude::*;
use gtk4::{Button, DrawingArea, Fixed, GestureClick, Label};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::date_page::{rounded_rect, text_centered, TextStyle};
use super::locale::short_month_name;
use super::shell::{font_size, Dark};
use super::stepper::StepperField;

type Callback = RefCell<Option<Box<dyn Fn(i32, u32, bool)>>>;

pub struct MonthYearPage {
    pub widget: Fixed,
    grid: DrawingArea,
    year_field: Rc<StepperField>,
    caption: Label,
    scale: f64,
    year: Cell<i32>,
    month: Cell<u32>,
    /// The first of the nine years on show; the grid scrolls by rows of three.
    window_start: Cell<i32>,
    on_change: Callback,
    syncing: Cell<bool>,
}

impl MonthYearPage {
    pub fn new(scale: f64, dark: Dark) -> Rc<Self> {
        let s = |v: f64| v * scale;
        let widget = Fixed::new();
        widget.set_size_request(s(690.0) as i32, s(807.0) as i32);

        let grid = DrawingArea::new();
        grid.set_size_request(s(690.0) as i32, s(700.0) as i32);
        widget.put(&grid, 0.0, 0.0);

        let prev = chevron("‹", scale);
        let next = chevron("›", scale);
        widget.put(&prev, s(33.0), s(92.0));
        widget.put(&next, s(608.0), s(92.0));

        for (text, y, size) in [("Month", 127.0, 16.0), ("Year", 400.0, 20.0)] {
            let label = Label::new(Some(text));
            label.add_css_class("picker-field-label");
            label.set_xalign(0.0);
            label.set_attributes(Some(&font_size(s(size))));
            widget.put(&label, s(25.0), s(y));
        }

        let year_field = StepperField::new(209.0, 45.0, scale);
        year_field.set_range(1, 9999);
        year_field.set_format(|y| y.to_string());
        widget.put(&year_field.widget, s(200.0), s(399.0));

        let caption = Label::new(None);
        caption.add_css_class("picker-caption-value");
        caption.set_xalign(0.0);
        caption.set_attributes(Some(&font_size(s(18.0))));
        widget.put(&caption, s(25.0), s(752.0));

        let page = Rc::new(Self {
            widget,
            grid,
            year_field,
            caption,
            scale,
            year: Cell::new(2026),
            month: Cell::new(1),
            window_start: Cell::new(2023),
            on_change: RefCell::new(None),
            syncing: Cell::new(false),
        });

        let this = Rc::downgrade(&page);
        page.year_field.connect_change({
            let this = this.clone();
            move |year| {
                let Some(page) = this.upgrade() else { return };
                if page.syncing.get() || year < 1 {
                    return;
                }
                page.year.set(year);
                page.keep_year_in_view();
                page.grid.queue_draw();
                page.emit(year, page.month.get(), false);
            }
        });
        for (button, delta) in [(prev, -1), (next, 1)] {
            let this = this.clone();
            button.connect_clicked(move |_| {
                if let Some(page) = this.upgrade() {
                    let year = (page.year.get() + delta).max(1);
                    page.emit(year, page.month.get(), false);
                }
            });
        }

        let click = GestureClick::new();
        click.connect_released({
            let this = this.clone();
            move |_, _, x, y| {
                if let Some(page) = this.upgrade() {
                    page.click(x, y);
                }
            }
        });
        page.grid.add_controller(click);

        let drawing = Rc::downgrade(&page);
        page.grid.set_draw_func(move |_, cr, _, _| {
            if let Some(page) = drawing.upgrade() {
                page.draw(cr, dark.get());
            }
        });

        page
    }

    pub fn connect_change(&self, callback: impl Fn(i32, u32, bool) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(callback));
    }

    fn emit(&self, year: i32, month: u32, done: bool) {
        if let Some(callback) = self.on_change.borrow().as_ref() {
            callback(year, month, done);
        }
    }

    pub fn set(&self, year: i32, month: u32) {
        self.year.set(year);
        self.month.set(month);
        let was_syncing = self.syncing.replace(true);
        self.year_field.set_value(year);
        self.syncing.set(was_syncing);
        self.keep_year_in_view();
        self.grid.queue_draw();
    }

    pub fn set_caption(&self, caption: &str) {
        self.caption.set_text(caption);
    }

    /// Keep the chosen year on show; recentre only when it has scrolled out of view.
    fn keep_year_in_view(&self) {
        let start = self.window_start.get();
        let year = self.year.get();
        if year < start || year > start + 8 {
            self.window_start.set((year - 3).max(1));
        }
    }

    fn month_rect(&self, month: u32) -> (f64, f64, f64, f64) {
        let s = |v: f64| v * self.scale;
        let index = f64::from(month - 1);
        (
            s(25.0 + (index % 4.0) * 150.0),
            s(158.0 + (index / 4.0).floor() * 70.0),
            s(139.0),
            s(57.0),
        )
    }

    fn year_rect(&self, index: usize) -> (f64, f64, f64, f64) {
        let s = |v: f64| v * self.scale;
        let i = index as f64;
        (s(25.0 + (i % 3.0) * 202.0), s(470.0 + (i / 3.0).floor() * 64.0), s(190.0), s(50.0))
    }

    fn years_shown(&self) -> Vec<i32> {
        let start = self.window_start.get();
        (start..start + 9).collect()
    }

    fn draw(&self, cr: &Context, dark: bool) {
        for month in 1..=12u32 {
            self.box_at(cr, self.month_rect(month), short_month_name(month), month == self.month.get(), dark);
        }
        for (i, year) in self.years_shown().into_iter().enumerate() {
            self.box_at(cr, self.year_rect(i), &year.to_string(), year == self.year.get(), dark);
        }
    }

    fn box_at(&self, cr: &Context, rect: (f64, f64, f64, f64), text: &str, on: bool, dark: bool) {
        let (x, y, w, h) = rect;
        let accent = if dark { (0.298, 0.553, 1.0) } else { (0.106, 0.427, 1.0) };
        let brand = if dark { (0.788, 0.839, 0.941) } else { (0.141, 0.227, 0.408) };
        rounded_rect(cr, x + 0.5, y + 0.5, w - 1.0, h - 1.0, 10.0 * self.scale);
        if on {
            cr.set_source_rgb(accent.0, accent.1, accent.2);
            let _ = cr.fill();
        } else {
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
        }
        let colour = if on { (1.0, 1.0, 1.0) } else { brand };
        text_centered(cr, text, x + w / 2.0, y + h / 2.0, TextStyle::new(20.0 * self.scale, on, colour));
    }

    fn click(&self, x: f64, y: f64) {
        for month in 1..=12u32 {
            let (bx, by, bw, bh) = self.month_rect(month);
            if x >= bx && x <= bx + bw && y >= by && y <= by + bh {
                self.emit(self.year.get(), month, true);
                return;
            }
        }
        for (i, year) in self.years_shown().into_iter().enumerate() {
            let (bx, by, bw, bh) = self.year_rect(i);
            if x >= bx && x <= bx + bw && y >= by && y <= by + bh {
                self.emit(year, self.month.get(), false);
                return;
            }
        }
    }
}

fn chevron(text: &str, scale: f64) -> Button {
    let label = Label::new(Some(text));
    label.set_attributes(Some(&font_size(30.0 * scale)));
    let button = Button::builder().child(&label).build();
    button.add_css_class("picker-flat");
    button.add_css_class("picker-chevron");
    button.set_size_request((40.0 * scale) as i32, (40.0 * scale) as i32);
    button
}
