//! The calendar the date, month and week pickers show, and the month & year screen it
//! drills down into.
//!
//! Ported from `PickerWindow.swift`'s `DatePage` and `MonthYearPage`. The grid is one
//! `DrawingArea` rather than forty-two buttons: the design's cells are numerals with a disc
//! behind the chosen one, which is a handful of cairo calls, where styled buttons would be
//! a fight with the theme over something that is not a button.
//!
//! Coordinates are the design's, multiplied by the shell's scale on the way in.

use chrono::{Datelike, Days, Months, NaiveDate};
use gtk4::cairo::{Context, FontSlant, FontWeight};
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, DrawingArea, Fixed, GestureClick, Label, Orientation};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::datetime::{PickerBounds, PickerValue};
use super::shell::font_size;
use super::stepper::StepperField;

/// Grid geometry, in design units.
const CELL_PITCH: f64 = 78.0;
const ROW_PITCH: f64 = 58.0;
const WEEKDAY_Y: f64 = 158.0;
const FIRST_ROW_CENTER: f64 = 212.0;

type Callback = RefCell<Option<Box<dyn Fn(PickerValue)>>>;
type Action = RefCell<Option<Box<dyn Fn()>>>;

pub struct DatePage {
    pub widget: Fixed,
    grid: DrawingArea,
    title: Label,
    day_field: Rc<StepperField>,
    month_field: Option<Rc<StepperField>>,
    year_field: Rc<StepperField>,
    caption: Label,

    /// A week picker selects rows, not cells, and indexes them with their ISO week number.
    weeks: bool,
    scale: f64,
    value: Cell<PickerValue>,
    /// The month on show, which is not always the value's: the arrows turn the page without
    /// moving the choice.
    shown: Cell<(i32, u32)>,
    today: PickerValue,
    limits: RefCell<PickerBounds>,
    /// " at 10:30 PM", for a datetime-local picker.
    caption_suffix: RefCell<String>,
    on_change: Callback,
    on_open_month_year: Action,
    syncing: Cell<bool>,
}

impl DatePage {
    pub fn new(weeks: bool, scale: f64, dark: bool) -> Rc<Self> {
        let s = |v: f64| v * scale;
        let widget = Fixed::new();
        widget.set_size_request(s(690.0) as i32, s(807.0) as i32);

        let grid = DrawingArea::new();
        grid.set_size_request(s(690.0) as i32, s(540.0) as i32);
        widget.put(&grid, 0.0, 0.0);

        let today_button = Button::with_label("Today");
        today_button.add_css_class("picker-today");
        today_button.set_size_request(s(110.0) as i32, s(37.0) as i32);
        today_button.set_attributes_on_label(s(17.0));
        widget.put(&today_button, s(560.0), s(22.0));

        let prev = chevron("‹", scale);
        let next = chevron("›", scale);
        widget.put(&prev, s(33.0), s(92.0));
        widget.put(&next, s(608.0), s(92.0));

        let title = Label::new(None);
        title.add_css_class("picker-month-title");
        title.set_xalign(0.0);
        title.set_attributes(Some(&font_size(s(22.0))));
        let title_button = Button::builder().child(&title).build();
        title_button.add_css_class("picker-flat");
        title_button.set_size_request(s(300.0) as i32, s(40.0) as i32);
        widget.put(&title_button, s(100.0), s(92.0));

        // A week input steps weeks and years; a date input days, months and years.
        let day_field = StepperField::new(124.0, 56.0, scale);
        let year_field = StepperField::new(179.0, 56.0, scale);
        // 234 in the design, but the design is set in a narrower face than any GTK desktop
        // has: "September" needs the extra to sit inside its box. It still clears the year
        // field, which begins at 435.
        let month_field = (!weeks).then(|| StepperField::new(252.0, 56.0, scale));
        let layout: Vec<(&Rc<StepperField>, &str, f64)> = if weeks {
            vec![(&day_field, "Week", 25.0), (&year_field, "Year", 175.0)]
        } else {
            vec![
                (&day_field, "Day", 25.0),
                (month_field.as_ref().expect("a date page has a month field"), "Month", 175.0),
                (&year_field, "Year", 435.0),
            ]
        };
        for (field, name, x) in layout {
            let label = Label::new(Some(name));
            label.add_css_class("picker-field-label");
            label.set_xalign(0.0);
            label.set_attributes(Some(&font_size(s(16.0))));
            widget.put(&label, s(x), s(552.0));
            widget.put(&field.widget, s(x), s(578.0));
        }

        let rule = GtkBox::new(Orientation::Horizontal, 0);
        rule.add_css_class("picker-divider");
        rule.set_size_request(s(620.0) as i32, 1);
        widget.put(&rule, s(25.0), s(697.0));

        let caption_title = Label::new(Some(if weeks { "Selected week:" } else { "Selected date:" }));
        caption_title.add_css_class("picker-caption-title");
        caption_title.set_xalign(0.0);
        caption_title.set_attributes(Some(&font_size(s(18.0))));
        widget.put(&caption_title, s(75.0), s(722.0));

        let caption = Label::new(None);
        caption.add_css_class("picker-caption-value");
        caption.set_xalign(0.0);
        caption.set_attributes(Some(&font_size(s(20.0))));
        widget.put(&caption, s(75.0), s(749.0));

        let today = PickerValue::now(None);
        let page = Rc::new(Self {
            widget,
            grid,
            title,
            day_field,
            month_field,
            year_field,
            caption,
            weeks,
            scale,
            value: Cell::new(today),
            shown: Cell::new((today.year, today.month)),
            today,
            limits: RefCell::new(PickerBounds::new(beacon_core::event::PickerKind::Date, None, None, None)),
            caption_suffix: RefCell::new(String::new()),
            on_change: RefCell::new(None),
            on_open_month_year: RefCell::new(None),
            syncing: Cell::new(false),
        });

        page.configure_fields();
        page.wire(&today_button, &prev, &next, &title_button, dark);
        page.refresh();
        page
    }

    fn configure_fields(self: &Rc<Self>) {
        let this = Rc::downgrade(self);
        if self.weeks {
            self.day_field.set_range(1, 53);
            // Week 52 up is week 1 of the next year, week 1 down is the last of the one
            // before, so the week field steps days rather than wrapping in its range.
            self.day_field.connect_step({
                let this = this.clone();
                move |direction| {
                    if let Some(page) = this.upgrade() {
                        page.shift_days(7 * i64::from(direction));
                    }
                }
            });
        } else {
            self.day_field.set_range(1, 31);
            self.day_field.set_wraps(true);
        }
        if let Some(month) = &self.month_field {
            month.set_range(1, 12);
            month.set_wraps(true);
            month.set_format(|m| month_name(m as u32).to_string());
            month.set_parse(|text| {
                let text = text.trim().to_ascii_lowercase();
                if text.is_empty() {
                    return None;
                }
                if let Ok(number) = text.parse::<i32>() {
                    return (1..=12).contains(&number).then_some(number);
                }
                (1..=12).find(|m| month_name(*m as u32).to_ascii_lowercase().starts_with(&text))
            });
        }
        self.year_field.set_range(1, 9999);
        self.year_field.set_format(|y| y.to_string());

        for field in [Some(&self.day_field), self.month_field.as_ref(), Some(&self.year_field)]
            .into_iter()
            .flatten()
        {
            let this = this.clone();
            field.connect_change(move |_| {
                if let Some(page) = this.upgrade() {
                    if !page.syncing.get() {
                        page.fields_changed();
                    }
                }
            });
        }
    }

    fn wire(self: &Rc<Self>, today: &Button, prev: &Button, next: &Button, title: &Button, dark: bool) {
        let this = Rc::downgrade(self);
        today.connect_clicked({
            let this = this.clone();
            move |_| {
                if let Some(page) = this.upgrade() {
                    let today = page.today;
                    page.emit(if page.weeks {
                        today.monday().map(|m| today.with_date(m)).unwrap_or(today)
                    } else {
                        today
                    });
                }
            }
        });
        for (button, months) in [(prev, -1), (next, 1)] {
            let this = this.clone();
            button.connect_clicked(move |_| {
                if let Some(page) = this.upgrade() {
                    page.step_month(months);
                }
            });
        }
        title.connect_clicked({
            let this = this.clone();
            move |_| {
                if let Some(page) = this.upgrade() {
                    if let Some(open) = page.on_open_month_year.borrow().as_ref() {
                        open();
                    }
                }
            }
        });

        let click = GestureClick::new();
        click.connect_released({
            let this = this.clone();
            move |_, _, x, y| {
                if let Some(page) = this.upgrade() {
                    page.click(x, y);
                }
            }
        });
        self.grid.add_controller(click);

        let page = Rc::downgrade(self);
        let weeks = self.weeks;
        let scale = self.scale;
        self.grid.set_draw_func(move |_, cr, _, _| {
            let Some(page) = page.upgrade() else { return };
            page.draw(cr, weeks, scale, dark);
        });
    }

    // ── state ─────────────────────────────────────────────────────────────

    pub fn set_limits(&self, limits: PickerBounds) {
        *self.limits.borrow_mut() = limits;
        self.grid.queue_draw();
    }

    /// A datetime-local picker appends its time here; that picker is the next pass.
    #[allow(dead_code)]
    pub fn set_caption_suffix(&self, suffix: String) {
        *self.caption_suffix.borrow_mut() = suffix;
        self.refresh();
    }

    pub fn set_value(&self, value: PickerValue) {
        self.value.set(value);
        // Turn to the value's month, unless it is a week already in view: a week is a row
        // here, and picking a visible row must not move the page under the pointer.
        if !(self.weeks && self.is_shown(value)) {
            self.shown.set((value.year, value.month));
        }
        self.refresh();
    }

    fn emit(&self, value: PickerValue) {
        if let Some(callback) = self.on_change.borrow().as_ref() {
            callback(value);
        }
    }

    pub fn connect_change(&self, callback: impl Fn(PickerValue) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(callback));
    }

    pub fn connect_open_month_year(&self, callback: impl Fn() + 'static) {
        *self.on_open_month_year.borrow_mut() = Some(Box::new(callback));
    }

    fn is_shown(&self, value: PickerValue) -> bool {
        let Some(date) = value.date() else { return false };
        self.cells().iter().any(|cell| cell.date == date)
    }

    fn step_month(&self, months: i32) {
        let (year, month) = self.shown.get();
        let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
            return;
        };
        let moved = if months >= 0 {
            first.checked_add_months(Months::new(months as u32))
        } else {
            first.checked_sub_months(Months::new((-months) as u32))
        };
        if let Some(moved) = moved {
            self.shown.set((moved.year(), moved.month()));
            self.refresh();
        }
    }

    /// Move the chosen date by whole days, across month and year ends.
    fn shift_days(&self, days: i64) {
        let value = self.value.get();
        let Some(current) = value.date() else { return };
        let moved = if days >= 0 {
            current.checked_add_days(Days::new(days as u64))
        } else {
            current.checked_sub_days(Days::new((-days) as u64))
        };
        let Some(moved) = moved else { return };
        if !self.limits.borrow().allows(moved) {
            return;
        }
        self.emit(value.with_date(moved));
    }

    fn fields_changed(&self) {
        let mut value = self.value.get();
        if self.weeks {
            let year = self.year_field.value().max(1);
            let week = self.day_field.value().max(1) as u32;
            let Some(monday) = NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::Mon) else {
                return;
            };
            value = value.with_date(monday);
        } else {
            value.year = self.year_field.value().max(1);
            value.month = self.month_field.as_ref().map(|f| f.value() as u32).unwrap_or(value.month);
            value.day = self.day_field.value() as u32;
            value.clamp_day();
        }
        self.emit(value);
    }

    /// Put the fields, the title and the caption back in step with the value.
    fn refresh(&self) {
        let value = self.value.get();
        let was_syncing = self.syncing.replace(true);
        if self.weeks {
            if let Some(date) = value.date() {
                let week = date.iso_week();
                let monday = value.monday().unwrap_or(date);
                let sunday = monday.checked_add_days(Days::new(6)).unwrap_or(monday);
                self.day_field.set_range(1, weeks_in_year(week.year()));
                self.day_field.set_value(week.week() as i32);
                self.year_field.set_value(week.year());
                self.caption.set_text(&format!(
                    "Week {}, {} · {} – {}",
                    week.week(),
                    week.year(),
                    monday.format("%-d %b"),
                    sunday.format("%-d %b")
                ));
            }
        } else {
            self.day_field.set_range(1, PickerValue::days_in(value.year, value.month) as i32);
            self.day_field.set_value(value.day as i32);
            if let Some(month) = &self.month_field {
                month.set_value(value.month as i32);
            }
            self.year_field.set_value(value.year);
            let spelled = value.date().map(|date| date.format("%A %-d %B %Y").to_string()).unwrap_or_default();
            self.caption.set_text(&format!("{spelled}{}", self.caption_suffix.borrow()));
        }
        let (year, month) = self.shown.get();
        self.title.set_text(&format!("{} {year} ⌄", month_name(month)));
        self.syncing.set(was_syncing);
        self.grid.queue_draw();
    }

    // ── the grid ──────────────────────────────────────────────────────────

    /// Six rows of seven: the shown month's days plus the neighbours' that fill the grid.
    /// Weeks start on Monday, as the design draws them.
    fn cells(&self) -> Vec<Cell42> {
        let (year, month) = self.shown.get();
        let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
            return Vec::new();
        };
        let lead = u64::from(first.weekday().num_days_from_monday());
        let Some(start) = first.checked_sub_days(Days::new(lead)) else {
            return Vec::new();
        };
        (0..42)
            .filter_map(|i| {
                let date = start.checked_add_days(Days::new(i))?;
                Some(Cell42 {
                    date,
                    in_month: date.month() == month && date.year() == year,
                    col: (i % 7) as f64,
                    row: (i / 7) as f64,
                })
            })
            .collect()
    }

    fn grid_left(&self) -> f64 {
        if self.weeks {
            112.0
        } else {
            77.0
        }
    }

    fn draw(&self, cr: &Context, weeks: bool, scale: f64, dark: bool) {
        let s = |v: f64| v * scale;
        let ink = if dark { (0.788, 0.839, 0.941) } else { (0.141, 0.227, 0.408) };
        let muted = if dark { (0.604, 0.647, 0.710) } else { (0.420, 0.478, 0.565) };
        let accent = if dark { (0.298, 0.553, 1.0) } else { (0.106, 0.427, 1.0) };
        let grid_left = self.grid_left();

        for (col, name) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].iter().enumerate() {
            text_centered(
                cr,
                name,
                s(grid_left + col as f64 * CELL_PITCH),
                s(WEEKDAY_Y),
                TextStyle::new(s(16.0), false, muted),
            );
        }

        let cells = self.cells();
        let value = self.value.get();
        let limits = self.limits.borrow();

        if weeks {
            // A gutter of week numbers: its own tinted band and a rule between it and the
            // days, so it reads as an index to the rows rather than an eighth column.
            let chosen = value.date().map(|date| (date.iso_week().year(), date.iso_week().week()));
            let top = s(WEEKDAY_Y - 18.0);
            let bottom = s(FIRST_ROW_CENTER + 5.0 * ROW_PITCH + 26.0);
            if dark {
                cr.set_source_rgb(0.165, 0.184, 0.220);
            } else {
                cr.set_source_rgb(0.933, 0.945, 0.969);
            }
            rounded_rect(cr, s(22.0), top, s(44.0), bottom - top, s(10.0));
            let _ = cr.fill();
            cr.set_source_rgba(muted.0, muted.1, muted.2, 0.35);
            cr.rectangle(s(72.0), top + s(6.0), 1.0, bottom - top - s(12.0));
            let _ = cr.fill();
            text_centered(cr, "WK", s(44.0), s(WEEKDAY_Y), TextStyle::new(s(12.0), true, muted));

            for row in 0..6 {
                let Some(first) = cells.iter().find(|cell| cell.row == f64::from(row)) else {
                    continue;
                };
                let week = first.date.iso_week();
                let y = s(FIRST_ROW_CENTER + f64::from(row) * ROW_PITCH);
                let is_chosen = chosen == Some((week.year(), week.week()));
                if is_chosen {
                    cr.set_source_rgba(accent.0, accent.1, accent.2, 0.15);
                    rounded_rect(cr, s(grid_left - 30.0), y - s(24.0), s(6.0 * CELL_PITCH + 60.0), s(48.0), s(24.0));
                    let _ = cr.fill();
                    cr.set_source_rgb(accent.0, accent.1, accent.2);
                    rounded_rect(cr, s(27.0), y - s(14.0), s(34.0), s(28.0), s(8.0));
                    let _ = cr.fill();
                }
                let colour = if is_chosen { (1.0, 1.0, 1.0) } else { ink };
                text_centered(cr, &week.week().to_string(), s(44.0), y, TextStyle::new(s(15.0), true, colour));
            }
        }

        for cell in &cells {
            let cx = s(grid_left + cell.col * CELL_PITCH);
            let cy = s(FIRST_ROW_CENTER + cell.row * ROW_PITCH);
            let is_chosen = !weeks && Some(cell.date) == value.date();
            let is_today = Some(cell.date) == self.today.date();
            let allowed = limits.allows(cell.date);
            if is_chosen {
                cr.set_source_rgb(accent.0, accent.1, accent.2);
                cr.arc(cx, cy, s(20.0), 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
            } else if is_today {
                cr.set_source_rgba(accent.0, accent.1, accent.2, 0.5);
                cr.set_line_width(1.5);
                cr.arc(cx, cy, s(19.5), 0.0, std::f64::consts::TAU);
                let _ = cr.stroke();
            }
            let (colour, alpha) = if is_chosen {
                ((1.0, 1.0, 1.0), 1.0)
            } else if !cell.in_month || !allowed {
                (muted, 0.55)
            } else {
                (ink, 1.0)
            };
            text_centered(
                cr,
                &cell.date.day().to_string(),
                cx,
                cy,
                TextStyle::new(s(20.0), is_chosen, colour).faded(alpha),
            );
        }
    }

    fn click(&self, x: f64, y: f64) {
        let s = |v: f64| v * self.scale;
        let cells = self.cells();
        let value = self.value.get();
        let limits = self.limits.borrow();
        let grid_left = self.grid_left();

        if self.weeks {
            // Anywhere along a row, week number included, picks that week by its Monday.
            for row in 0..6 {
                let cy = s(FIRST_ROW_CENTER + f64::from(row) * ROW_PITCH);
                if (y - cy).abs() > s(26.0) || x < s(25.0) || x > s(grid_left + 6.0 * CELL_PITCH + 30.0) {
                    continue;
                }
                let Some(monday) = cells.iter().find(|cell| cell.row == f64::from(row)) else {
                    continue;
                };
                if !limits.allows(monday.date) {
                    return;
                }
                drop(limits);
                self.emit(value.with_date(monday.date));
                return;
            }
            return;
        }
        for cell in &cells {
            let cx = s(grid_left + cell.col * CELL_PITCH);
            let cy = s(FIRST_ROW_CENTER + cell.row * ROW_PITCH);
            if (x - cx).abs() <= s(26.0) && (y - cy).abs() <= s(24.0) {
                if !limits.allows(cell.date) {
                    return;
                }
                drop(limits);
                self.emit(value.with_date(cell.date));
                return;
            }
        }
    }
}

struct Cell42 {
    date: NaiveDate,
    in_month: bool,
    col: f64,
    row: f64,
}

/// How many ISO weeks a week-year has: 52, or 53 when the year is long.
fn weeks_in_year(year: i32) -> i32 {
    NaiveDate::from_ymd_opt(year, 12, 28)
        .map(|date| date.iso_week().week() as i32)
        .unwrap_or(52)
}

pub fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES.get((month.max(1) as usize - 1).min(11)).copied().unwrap_or("January")
}

pub fn short_month_name(month: u32) -> &'static str {
    &month_name(month)[..3]
}

/// Centre a numeral on a point. Cairo's own text API is enough here: these are digits and
/// two-letter weekday stubs, not running text that would want Pango's shaping.
/// How a numeral is drawn: the style travels together rather than as four arguments.
#[derive(Clone, Copy)]
pub struct TextStyle {
    pub size: f64,
    pub bold: bool,
    pub colour: (f64, f64, f64),
    pub alpha: f64,
}

impl TextStyle {
    pub fn new(size: f64, bold: bool, colour: (f64, f64, f64)) -> Self {
        Self {
            size,
            bold,
            colour,
            alpha: 1.0,
        }
    }

    pub fn faded(mut self, alpha: f64) -> Self {
        self.alpha = alpha;
        self
    }
}

pub fn text_centered(cr: &Context, text: &str, cx: f64, cy: f64, style: TextStyle) {
    let TextStyle { size, bold, colour, alpha } = style;
    cr.select_font_face(
        "sans-serif",
        FontSlant::Normal,
        if bold { FontWeight::Bold } else { FontWeight::Normal },
    );
    cr.set_font_size(size);
    let Ok(extents) = cr.text_extents(text) else { return };
    cr.set_source_rgba(colour.0, colour.1, colour.2, alpha);
    cr.move_to(
        cx - extents.width() / 2.0 - extents.x_bearing(),
        cy - extents.height() / 2.0 - extents.y_bearing(),
    );
    let _ = cr.show_text(text);
}

pub fn rounded_rect(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
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

/// `Button::set_attributes` does not exist; the label inside one takes them.
trait ButtonLabelExt {
    fn set_attributes_on_label(&self, size: f64);
}

impl ButtonLabelExt for Button {
    fn set_attributes_on_label(&self, size: f64) {
        if let Some(label) = self.child().and_then(|child| child.downcast::<Label>().ok()) {
            label.set_attributes(Some(&font_size(size)));
        }
    }
}
