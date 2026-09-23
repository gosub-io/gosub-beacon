//! The Quick select screen: groups of rows, each a glyph, a title and the value it lands
//! on — "Tomorrow · Wed 23 Sep", "In 15 minutes · 10:45 PM".
//!
//! Ported from `PickerWindow.swift`'s `QuickPick` and `QuickSelectPage`. The picks are
//! rebuilt every time the page is shown, because the relative ones ("Now", "In 1 hour") are
//! only true for as long as it takes to read them.

use beacon_core::event::PickerKind;
use chrono::{Datelike, Days, Local, Months, NaiveDate, NaiveDateTime, Timelike};
use gtk4::cairo::Context;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, DrawingArea, Fixed, Label, Orientation};
use std::cell::RefCell;
use std::rc::Rc;

use super::date_page::month_name;
use super::datetime::{clock_string, PickerValue};
use super::shell::font_size;
use super::time_page::{draw_glyph, Daypart, Glyph, MOON, SUN};

/// What a row does to the value when it is chosen.
#[derive(Clone, Copy)]
pub enum Apply {
    Date(NaiveDate),
    Month(i32, u32),
    Time(u32, u32),
    DateAndTime(NaiveDate, u32, u32),
}

impl Apply {
    pub fn to(self, value: &mut PickerValue) {
        match self {
            Self::Date(date) => {
                (value.year, value.month, value.day) = (date.year(), date.month(), date.day());
            }
            Self::Month(year, month) => {
                (value.year, value.month, value.day) = (year, month, 1);
            }
            Self::Time(hour, minute) => {
                (value.hour, value.minute) = (hour, minute);
                if value.second.is_some() {
                    value.second = Some(0);
                }
            }
            Self::DateAndTime(date, hour, minute) => {
                Self::Date(date).to(value);
                Self::Time(hour, minute).to(value);
            }
        }
    }
}

/// A row: a preset the user can jump to.
pub struct QuickPick {
    /// A clock-like pie filled this far, instead of a glyph — the relative times use it.
    pub pie: Option<f64>,
    pub glyph: Glyph,
    pub tint: (f64, f64, f64),
    pub title: String,
    /// What it resolves to, shown at the right.
    pub value: String,
    pub apply: Apply,
}

pub struct Group {
    pub title: &'static str,
    pub picks: Vec<QuickPick>,
}

fn date_value(date: NaiveDate) -> String {
    date.format("%a %-d %b %Y").to_string()
}

fn day_pick(date: NaiveDate, glyph: Glyph, tint: (f64, f64, f64), title: &str) -> QuickPick {
    QuickPick {
        pie: None,
        glyph,
        tint,
        title: title.to_string(),
        value: date_value(date),
        apply: Apply::Date(date),
    }
}

/// The next date that falls on `weekday`, never today.
fn next_weekday(today: NaiveDate, weekday: chrono::Weekday) -> NaiveDate {
    let current = today.weekday().num_days_from_monday();
    let wanted = weekday.num_days_from_monday();
    let mut ahead = (wanted + 7 - current) % 7;
    if ahead == 0 {
        ahead = 7;
    }
    today.checked_add_days(Days::new(u64::from(ahead))).unwrap_or(today)
}

/// The groups for a kind: relative times and dayparts, or relative days and jumps.
pub fn groups(kind: PickerKind, time_section: bool) -> Vec<Group> {
    let now: NaiveDateTime = Local::now().naive_local();
    let today = now.date();

    if kind == PickerKind::Month {
        let this_month = (today.year(), today.month());
        let plus = |months: u32| {
            let date = NaiveDate::from_ymd_opt(this_month.0, this_month.1, 1)
                .and_then(|d| d.checked_add_months(Months::new(months)))
                .unwrap_or(today);
            (date.year(), date.month())
        };
        let month_pick = |(year, month): (i32, u32), title: &str| QuickPick {
            pie: None,
            glyph: Glyph::Moon,
            tint: MOON,
            title: title.to_string(),
            value: format!("{} {year}", month_name(month)),
            apply: Apply::Month(year, month),
        };
        return vec![
            Group {
                title: "",
                picks: vec![
                    month_pick(this_month, "This month"),
                    month_pick(plus(1), "Next month"),
                    month_pick(plus(3), "In 3 months"),
                    month_pick(plus(6), "In 6 months"),
                ],
            },
            Group {
                title: "",
                picks: vec![
                    month_pick((today.year() + 1, 1), "January next year"),
                    month_pick(plus(12), "Same month next year"),
                ],
            },
        ];
    }

    if kind == PickerKind::DateTimeLocal {
        // A day group and a time group, so both halves can be picked without leaving the
        // page; "Now" sets both.
        let days = groups(PickerKind::Date, false).remove(0).picks;
        let mut times = groups(PickerKind::Time, true);
        let parts = times.remove(1).picks;
        let relative = times.remove(0).picks;
        let now_value = relative[0].value.clone();
        let mut day_picks: Vec<QuickPick> = days.into_iter().collect();
        day_picks.truncate(3);
        return vec![
            Group {
                title: "Day",
                picks: day_picks,
            },
            Group {
                title: "Time",
                picks: std::iter::once(QuickPick {
                    pie: Some(0.0),
                    glyph: Glyph::Moon,
                    tint: MOON,
                    title: "Now".to_string(),
                    value: now_value,
                    apply: Apply::DateAndTime(today, now.hour(), now.minute()),
                })
                .chain(parts)
                .collect(),
            },
        ];
    }

    if time_section {
        let relative = |minutes: i64, pie: f64, title: &str| {
            let at = now + chrono::TimeDelta::minutes(minutes);
            QuickPick {
                pie: Some(pie),
                glyph: Glyph::Moon,
                tint: MOON,
                title: title.to_string(),
                value: clock_string(at.hour(), at.minute()),
                apply: Apply::Time(at.hour(), at.minute()),
            }
        };
        let fixed = |hour: u32, title: &str| {
            let part = Daypart::of(hour);
            QuickPick {
                pie: None,
                glyph: if matches!(part, Daypart::Night | Daypart::Evening) {
                    Glyph::Moon
                } else {
                    Glyph::Sun
                },
                tint: if matches!(part, Daypart::Night | Daypart::Evening) {
                    MOON
                } else {
                    SUN
                },
                title: title.to_string(),
                value: clock_string(hour, 0),
                apply: Apply::Time(hour, 0),
            }
        };
        return vec![
            Group {
                title: "Relative",
                picks: vec![
                    relative(0, 0.0, "Now"),
                    relative(15, 0.25, "In 15 minutes"),
                    relative(30, 0.5, "In 30 minutes"),
                    relative(60, 1.0, "In 1 hour"),
                ],
            },
            Group {
                title: "Dayparts",
                picks: vec![fixed(6, "Morning"), fixed(12, "Noon"), fixed(18, "Evening"), fixed(0, "Night")],
            },
        ];
    }

    let plus_days = |n: u64| today.checked_add_days(Days::new(n)).unwrap_or(today);
    let brand = (0.141, 0.227, 0.408);
    // Headerless groups, separated by rules, as the date screen lists them.
    vec![
        Group {
            title: "",
            picks: vec![
                day_pick(today, Glyph::Moon, MOON, "Today"),
                day_pick(plus_days(1), Glyph::Moon, MOON, "Tomorrow"),
                day_pick(next_weekday(today, chrono::Weekday::Sat), Glyph::Moon, MOON, "This weekend"),
            ],
        },
        Group {
            title: "",
            picks: vec![
                day_pick(next_weekday(today, chrono::Weekday::Mon), Glyph::Sun, brand, "Next Monday"),
                day_pick(next_weekday(today, chrono::Weekday::Fri), Glyph::Sun, brand, "Next Friday"),
            ],
        },
        Group {
            title: "",
            picks: vec![
                day_pick(plus_days(7), Glyph::Moon, MOON, "In 1 week"),
                day_pick(plus_days(14), Glyph::Moon, MOON, "In 2 weeks"),
                day_pick(
                    today.checked_add_months(Months::new(1)).unwrap_or(today),
                    Glyph::Moon,
                    MOON,
                    "In 1 month",
                ),
            ],
        },
    ]
}

type PickCallback = RefCell<Option<Box<dyn Fn(Apply)>>>;

pub struct QuickSelectPage {
    pub widget: Fixed,
    rows: Fixed,
    subtitle: Label,
    caption: Label,
    scale: f64,
    dark: bool,
    on_pick: PickCallback,
}

impl QuickSelectPage {
    pub fn new(scale: f64, dark: bool) -> Rc<Self> {
        let s = |v: f64| v * scale;
        let widget = Fixed::new();
        widget.set_size_request(s(690.0) as i32, s(807.0) as i32);

        let subtitle = Label::new(None);
        subtitle.add_css_class("picker-caption-title");
        subtitle.set_xalign(0.0);
        subtitle.set_attributes(Some(&font_size(s(16.0))));
        widget.put(&subtitle, s(26.0), s(66.0));

        let rows = Fixed::new();
        rows.set_size_request(s(690.0) as i32, s(640.0) as i32);
        widget.put(&rows, 0.0, 0.0);

        let caption = Label::new(None);
        caption.add_css_class("picker-caption-value");
        caption.set_xalign(0.0);
        caption.set_attributes(Some(&font_size(s(18.0))));
        widget.put(&caption, s(27.0), s(752.0));

        Rc::new(Self {
            widget,
            rows,
            subtitle,
            caption,
            scale,
            dark,
            on_pick: RefCell::new(None),
        })
    }

    pub fn connect_pick(&self, callback: impl Fn(Apply) + 'static) {
        *self.on_pick.borrow_mut() = Some(Box::new(callback));
    }

    pub fn set_subtitle(&self, text: &str) {
        self.subtitle.set_text(text);
    }

    pub fn set_caption(&self, text: &str) {
        self.caption.set_text(text);
    }

    /// Rebuild the rows; the relative values are recomputed on the way.
    pub fn reload(self: &Rc<Self>, groups: Vec<Group>) {
        let s = |v: f64| v * self.scale;
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        let headed = groups.iter().any(|group| !group.title.is_empty());
        let mut y = if headed { 114.0 } else { 119.0 };
        for (i, group) in groups.into_iter().enumerate() {
            if headed {
                let header = Label::new(Some(group.title));
                header.add_css_class("picker-caption-title");
                header.set_xalign(0.0);
                header.set_attributes(Some(&font_size(s(15.0))));
                self.rows.put(&header, s(27.0), s(y));
                y += 32.0;
            } else if i > 0 {
                let rule = GtkBox::new(Orientation::Horizontal, 0);
                rule.add_css_class("picker-divider");
                rule.set_size_request(s(600.0) as i32, 1);
                self.rows.put(&rule, s(45.0), s(y + 12.0));
                y += 26.0;
            }
            for pick in group.picks {
                let (x, width, height) = if headed { (27.0, 620.0, 59.0) } else { (45.0, 600.0, 60.0) };
                let row = self.row_button(&pick, width, height);
                self.rows.put(&row, s(x), s(y));
                y += if headed { 70.0 } else { 60.0 };
            }
            y += if headed { 8.0 } else { 0.0 };
        }
    }

    fn row_button(self: &Rc<Self>, pick: &QuickPick, width: f64, height: f64) -> Button {
        let s = |v: f64| v * self.scale;
        let area = DrawingArea::new();
        area.set_size_request(s(width) as i32, s(height) as i32);
        let (pie, glyph, tint) = (pick.pie, pick.glyph, pick.tint);
        let (title, value) = (pick.title.clone(), pick.value.clone());
        let (scale, dark) = (self.scale, self.dark);
        area.set_draw_func(move |_, cr, w, h| {
            draw_row(cr, f64::from(w), f64::from(h), scale, dark, pie, glyph, tint, &title, &value);
        });

        let button = Button::builder().child(&area).build();
        button.add_css_class("picker-quick-row");
        button.set_size_request(s(width) as i32, s(height) as i32);
        let apply = pick.apply;
        let this = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(page) = this.upgrade() {
                if let Some(callback) = page.on_pick.borrow().as_ref() {
                    callback(apply);
                }
            }
        });
        button
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    cr: &Context,
    w: f64,
    h: f64,
    scale: f64,
    dark: bool,
    pie: Option<f64>,
    glyph: Glyph,
    tint: (f64, f64, f64),
    title: &str,
    value: &str,
) {
    use super::date_page::TextStyle;
    let s = |v: f64| v * scale;
    let brand = if dark { (0.788, 0.839, 0.941) } else { (0.141, 0.227, 0.408) };
    let muted = if dark { (0.604, 0.647, 0.710) } else { (0.420, 0.478, 0.565) };
    let (icon_x, icon_y) = (s(38.0), h / 2.0);

    if let Some(pie) = pie {
        // A clock-like pie: an outline, filled from twelve o'clock this far round.
        let r = s(15.0);
        if dark {
            cr.set_source_rgb(0.165, 0.184, 0.220);
        } else {
            cr.set_source_rgb(1.0, 1.0, 1.0);
        }
        cr.arc(icon_x, icon_y, r, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        if pie > 0.0 {
            cr.set_source_rgb(0.106, 0.427, 1.0);
            cr.move_to(icon_x, icon_y);
            cr.arc(
                icon_x,
                icon_y,
                r,
                -std::f64::consts::FRAC_PI_2,
                -std::f64::consts::FRAC_PI_2 + pie * std::f64::consts::TAU,
            );
            cr.close_path();
            let _ = cr.fill();
        }
        cr.set_source_rgb(MOON.0, MOON.1, MOON.2);
        cr.set_line_width(s(2.0));
        cr.arc(icon_x, icon_y, r, 0.0, std::f64::consts::TAU);
        let _ = cr.stroke();
    } else {
        draw_glyph(cr, glyph, icon_x, icon_y, s(26.0), tint);
    }

    cr.select_font_face("sans-serif", gtk4::cairo::FontSlant::Normal, gtk4::cairo::FontWeight::Bold);
    cr.set_font_size(s(17.0));
    cr.set_source_rgb(brand.0, brand.1, brand.2);
    if let Ok(extents) = cr.text_extents(title) {
        cr.move_to(s(75.0), h / 2.0 - extents.height() / 2.0 - extents.y_bearing());
        let _ = cr.show_text(title);
    }
    cr.select_font_face("sans-serif", gtk4::cairo::FontSlant::Normal, gtk4::cairo::FontWeight::Normal);
    if let Ok(extents) = cr.text_extents(value) {
        text_right(
            cr,
            value,
            w - s(24.0) - extents.width(),
            h / 2.0,
            TextStyle::new(s(17.0), false, muted),
        );
    }
}

fn text_right(cr: &Context, text: &str, x: f64, cy: f64, style: super::date_page::TextStyle) {
    cr.select_font_face(
        "sans-serif",
        gtk4::cairo::FontSlant::Normal,
        if style.bold {
            gtk4::cairo::FontWeight::Bold
        } else {
            gtk4::cairo::FontWeight::Normal
        },
    );
    cr.set_font_size(style.size);
    let Ok(extents) = cr.text_extents(text) else { return };
    cr.set_source_rgba(style.colour.0, style.colour.1, style.colour.2, style.alpha);
    cr.move_to(x - extents.x_bearing(), cy - extents.height() / 2.0 - extents.y_bearing());
    let _ = cr.show_text(text);
}
