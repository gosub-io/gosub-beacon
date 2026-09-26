//! The design's number field: a value in a rounded box with a pair of small arrows.
//!
//! Ported from `PickerWindow.swift`'s `StepperField`. It is not a `GtkSpinButton` because
//! the design's box has its own shape and its own arrows, and because some of these fields
//! do not hold a plain number: a month field reads "September" and accepts "sep", and a week
//! field must roll into the next year rather than wrap within its range.

use gtk4::prelude::*;
use gtk4::{gdk, glib, Align, Box as GtkBox, Button, Entry, EventControllerKey, Orientation};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

type Formatter = RefCell<Option<Box<dyn Fn(i32) -> String>>>;
type Parser = RefCell<Option<Box<dyn Fn(&str) -> Option<i32>>>>;
type Callback = RefCell<Option<Box<dyn Fn(i32)>>>;

pub struct StepperField {
    pub widget: GtkBox,
    entry: Entry,
    range: Cell<(i32, i32)>,
    step: Cell<i32>,
    wraps: Cell<bool>,
    value: Cell<i32>,
    format: Formatter,
    parse: Parser,
    /// When set, the arrows call this with +1/-1 instead of stepping within `range` — for a
    /// week stepper that must roll into the next year.
    on_step: Callback,
    on_change: Callback,
    /// Set while the field writes to its own entry, so the change signal is ignored.
    syncing: Cell<bool>,
}

impl StepperField {
    /// `width` and `height` are design units; `scale` is the shell's.
    pub fn new(width: f64, height: f64, scale: f64) -> Rc<Self> {
        let widget = GtkBox::new(Orientation::Horizontal, 0);
        widget.add_css_class("picker-stepper");
        widget.set_size_request((width * scale) as i32, (height * scale) as i32);

        let entry = Entry::new();
        entry.add_css_class("picker-stepper-entry");
        entry.set_hexpand(true);
        entry.set_width_chars(2);
        entry.set_max_width_chars(2);
        entry.set_has_frame(false);
        entry.set_valign(Align::Center);
        entry.set_attributes(&super::shell::font_size(30.0 * scale));
        widget.append(&entry);

        let arrows = GtkBox::new(Orientation::Vertical, 0);
        arrows.set_valign(Align::Center);
        let up = arrow_button("pan-up-symbolic", scale);
        let down = arrow_button("pan-down-symbolic", scale);
        arrows.append(&up);
        arrows.append(&down);
        widget.append(&arrows);

        let field = Rc::new(Self {
            widget,
            entry,
            range: Cell::new((0, 59)),
            step: Cell::new(1),
            wraps: Cell::new(false),
            value: Cell::new(0),
            format: RefCell::new(None),
            parse: RefCell::new(None),
            on_step: RefCell::new(None),
            on_change: RefCell::new(None),
            syncing: Cell::new(false),
        });

        for (button, direction) in [(up, 1), (down, -1)] {
            let this = Rc::downgrade(&field);
            button.connect_clicked(move |_| {
                if let Some(field) = this.upgrade() {
                    field.move_by(direction);
                }
            });
        }

        field.entry.connect_changed({
            let this = Rc::downgrade(&field);
            move |entry| {
                let Some(field) = this.upgrade() else { return };
                if field.syncing.get() {
                    return;
                }
                let text = entry.text();
                let parsed = match field.parse.borrow().as_ref() {
                    Some(parse) => parse(&text),
                    None => text.trim().parse().ok(),
                };
                // Half a number is not a number: the field waits rather than snapping.
                let Some(number) = parsed else { return };
                let (low, high) = field.range.get();
                if number < low || number > high {
                    return;
                }
                field.value.set(number);
                field.emit(number);
            }
        });

        // Up and down in the field step it, as the design's keyboard note asks.
        let keys = EventControllerKey::new();
        keys.connect_key_pressed({
            let this = Rc::downgrade(&field);
            move |_, key, _, _| {
                let Some(field) = this.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                match key {
                    gdk::Key::Up => field.move_by(1),
                    gdk::Key::Down => field.move_by(-1),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        field.entry.add_controller(keys);

        field
    }

    fn emit(&self, value: i32) {
        if let Some(callback) = self.on_change.borrow().as_ref() {
            callback(value);
        }
    }

    fn move_by(&self, direction: i32) {
        if let Some(on_step) = self.on_step.borrow().as_ref() {
            on_step(direction);
            return;
        }
        let (low, high) = self.range.get();
        let mut next = self.value.get() + direction * self.step.get();
        if next > high {
            next = if self.wraps.get() { low } else { high };
        }
        if next < low {
            next = if self.wraps.get() { high } else { low };
        }
        self.set_value(next);
        self.emit(next);
    }

    pub fn set_range(&self, low: i32, high: i32) {
        self.range.set((low, high));
    }

    /// A minute field steps by the control's step; that is the time pickers' business.
    #[allow(dead_code)]
    pub fn set_step(&self, step: i32) {
        self.step.set(step.max(1));
    }

    pub fn set_wraps(&self, wraps: bool) {
        self.wraps.set(wraps);
    }

    pub fn value(&self) -> i32 {
        self.value.get()
    }

    pub fn set_value(&self, value: i32) {
        self.value.set(value);
        let text = match self.format.borrow().as_ref() {
            Some(format) => format(value),
            // A field whose range starts at zero is a clock field, and pads.
            None if self.range.get().0 == 0 => format!("{value:02}"),
            None => value.to_string(),
        };
        let was_syncing = self.syncing.replace(true);
        if self.entry.text() != text {
            self.entry.set_text(&text);
        }
        self.syncing.set(was_syncing);
    }

    pub fn set_format(&self, format: impl Fn(i32) -> String + 'static) {
        *self.format.borrow_mut() = Some(Box::new(format));
    }

    pub fn set_parse(&self, parse: impl Fn(&str) -> Option<i32> + 'static) {
        *self.parse.borrow_mut() = Some(Box::new(parse));
    }

    pub fn connect_step(&self, callback: impl Fn(i32) + 'static) {
        *self.on_step.borrow_mut() = Some(Box::new(callback));
    }

    pub fn connect_change(&self, callback: impl Fn(i32) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(callback));
    }
}

fn arrow_button(icon: &str, scale: f64) -> Button {
    let button = Button::from_icon_name(icon);
    button.add_css_class("picker-stepper-arrow");
    button.set_size_request((18.0 * scale) as i32, (17.0 * scale) as i32);
    button
}
