//! The picker a page's date, time, `datetime-local`, month or week input opens: the large
//! shell, with the sidebar offering the input's own section and Quick select, a 12 h / 24 h
//! toggle for the time kinds, and one main card.
//!
//! Ported from `PickerWindow.swift`'s controller. Values cross the seam in the HTML forms'
//! ISO shapes; every change is reported live, and the finish fires once with the final
//! value, or nothing on cancel.

use beacon_core::event::PickerKind;
use gtk4::prelude::*;
use gtk4::Window;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::date_page::{month_name, DatePage};
use super::datetime::{clock_string, PickerBounds, PickerValue};
use super::month_year_page::MonthYearPage;
use super::quick_select::{self, QuickSelectPage};
use super::shell::{Metrics, NavIcon, NavItem, PickerShell, Rect, ShellConfig};
use super::time_page::{time_caption, ClockFormatToggle, TimePage};

type Callback = RefCell<Option<Box<dyn Fn(String)>>>;

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Date,
    Time,
    Quick,
}

pub struct DateTimePicker {
    shell: Rc<PickerShell>,
    date_page: Option<Rc<DatePage>>,
    month_year_page: Rc<MonthYearPage>,
    time_page: Option<Rc<TimePage>>,
    quick_page: Rc<QuickSelectPage>,
    format_toggle: Option<Rc<ClockFormatToggle>>,

    kind: PickerKind,
    base_title: &'static str,
    sections: Vec<Section>,
    value: Cell<PickerValue>,
    /// The Date section drilled down into the month & year view, and the value it had when
    /// it did, for Cancel to put back.
    showing_month_year: Cell<bool>,
    before_drill_down: Cell<Option<PickerValue>>,
    on_change: Callback,
}

impl DateTimePicker {
    pub fn new(
        parent: Option<&impl IsA<Window>>,
        kind: PickerKind,
        value: &str,
        min: Option<&str>,
        max: Option<&str>,
        step: Option<&str>,
    ) -> Rc<Self> {
        let limits = PickerBounds::new(kind, min, max, step);
        let mut initial = PickerValue::parse(value, kind).unwrap_or_else(|| PickerValue::now(limits.step));
        // A control can open on a day it would not accept -- an empty input on a page whose
        // min is next month. Landing on the nearest day it does accept means the commit
        // button is always a valid answer.
        if matches!(kind, PickerKind::Date | PickerKind::Month | PickerKind::Week) {
            if let Some(date) = initial.date() {
                initial = initial.with_date(limits.nearest_allowed(date));
            }
        }
        if kind == PickerKind::Month {
            initial.day = 1;
        }
        if kind == PickerKind::Week {
            if let Some(monday) = initial.monday() {
                initial = initial.with_date(monday);
            }
        }
        // Seconds are edited only when the control's step asks for them, or its value
        // already carried them.
        let shows_seconds = limits.step.map(|step| step < 60).unwrap_or(initial.second.is_some());
        if shows_seconds && initial.second.is_none() {
            initial.second = Some(0);
        }

        let (sections, nav, base_title) = match kind {
            PickerKind::Time => (
                vec![Section::Time, Section::Quick],
                vec![
                    NavItem {
                        icon: NavIcon::Clock,
                        title: "Time",
                    },
                    NavItem {
                        icon: NavIcon::Bolt,
                        title: "Quick select",
                    },
                ],
                "Select a time",
            ),
            PickerKind::DateTimeLocal => (
                vec![Section::Date, Section::Time, Section::Quick],
                vec![
                    NavItem {
                        icon: NavIcon::Calendar,
                        title: "Date",
                    },
                    NavItem {
                        icon: NavIcon::Clock,
                        title: "Time",
                    },
                    NavItem {
                        icon: NavIcon::Bolt,
                        title: "Quick select",
                    },
                ],
                "Select a date and time",
            ),
            PickerKind::Month => (
                vec![Section::Date, Section::Quick],
                vec![
                    NavItem {
                        icon: NavIcon::Calendar,
                        title: "Month",
                    },
                    NavItem {
                        icon: NavIcon::Bolt,
                        title: "Quick select",
                    },
                ],
                "Select a month",
            ),
            PickerKind::Week => (
                vec![Section::Date, Section::Quick],
                vec![
                    NavItem {
                        icon: NavIcon::Calendar,
                        title: "Week",
                    },
                    NavItem {
                        icon: NavIcon::Bolt,
                        title: "Quick select",
                    },
                ],
                "Select a week",
            ),
            _ => (
                vec![Section::Date, Section::Quick],
                vec![
                    NavItem {
                        icon: NavIcon::Calendar,
                        title: "Date",
                    },
                    NavItem {
                        icon: NavIcon::Bolt,
                        title: "Quick select",
                    },
                ],
                "Select a date",
            ),
        };
        let help_url = match kind {
            PickerKind::Time => "https://developer.mozilla.org/en-US/docs/Web/HTML/Element/input/time",
            PickerKind::DateTimeLocal => "https://developer.mozilla.org/en-US/docs/Web/HTML/Element/input/datetime-local",
            PickerKind::Month => "https://developer.mozilla.org/en-US/docs/Web/HTML/Element/input/month",
            PickerKind::Week => "https://developer.mozilla.org/en-US/docs/Web/HTML/Element/input/week",
            _ => "https://developer.mozilla.org/en-US/docs/Web/HTML/Element/input/date",
        };

        let shell = PickerShell::new(
            parent,
            ShellConfig {
                title: base_title,
                size: (983.0, 910.0),
                main_card: Rect::new(275.0, 18.0, 690.0, 807.0),
                side_card: None,
                nav,
                metrics: &Metrics::LARGE,
                ok_label: "OK",
                help_url: Some(help_url),
            },
        );

        let has_date = sections.contains(&Section::Date);
        let has_time = sections.contains(&Section::Time);
        let date_page = has_date.then(|| DatePage::new(kind == PickerKind::Week, shell.scale, shell.dark));
        if let Some(page) = &date_page {
            page.set_limits(PickerBounds::new(kind, min, max, step));
        }
        let time_page = has_time.then(|| TimePage::new(shows_seconds, shell.scale, shell.dark));
        if let Some(page) = &time_page {
            // A step of a minute or more moves the minute field and the clock onto its grid.
            let minutes = limits.step.map(|step| if step >= 60 { (step / 60) as u32 } else { 1 }).unwrap_or(1);
            page.set_minute_step(minutes);
        }
        let month_year_page = MonthYearPage::new(shell.scale, shell.dark);
        let quick_page = QuickSelectPage::new(shell.scale, shell.dark);
        let format_toggle = has_time.then(|| ClockFormatToggle::new(106.0, 27.0, shell.scale, shell.dark));

        let picker = Rc::new(Self {
            shell,
            date_page,
            month_year_page,
            time_page,
            quick_page,
            format_toggle,
            kind,
            base_title,
            sections,
            value: Cell::new(initial),
            // A month input opens on the month & year screen and never leaves it.
            showing_month_year: Cell::new(kind == PickerKind::Month),
            before_drill_down: Cell::new(None),
            on_change: RefCell::new(None),
        });

        picker.wire();
        picker.show();
        picker
    }

    fn section(&self) -> Section {
        self.sections[self.shell.selected_nav().min(self.sections.len() - 1)]
    }

    fn has_time(&self) -> bool {
        self.time_page.is_some()
    }

    fn wire(self: &Rc<Self>) {
        let this = Rc::downgrade(self);

        self.shell.connect_nav({
            let this = this.clone();
            move |_| {
                if let Some(picker) = this.upgrade() {
                    if picker.kind != PickerKind::Month {
                        picker.showing_month_year.set(false);
                    }
                    picker.show();
                }
            }
        });

        if let Some(page) = &self.date_page {
            page.connect_change({
                let this = this.clone();
                move |picked| {
                    if let Some(picker) = this.upgrade() {
                        let mut value = picker.value.get();
                        (value.year, value.month, value.day) = (picked.year, picked.month, picked.day);
                        picker.value.set(value);
                        picker.changed();
                        if let Some(page) = &picker.date_page {
                            page.set_value(value);
                        }
                    }
                }
            });
            page.connect_open_month_year({
                let this = this.clone();
                move || {
                    if let Some(picker) = this.upgrade() {
                        picker.before_drill_down.set(Some(picker.value.get()));
                        picker.showing_month_year.set(true);
                        picker.show();
                    }
                }
            });
        }

        self.month_year_page.connect_change({
            let this = this.clone();
            move |year, month, _done| {
                let Some(picker) = this.upgrade() else { return };
                let mut value = picker.value.get();
                (value.year, value.month) = (year, month);
                value.clamp_day();
                if picker.kind == PickerKind::Month {
                    value.day = 1;
                }
                picker.value.set(value);
                picker.changed();
                // The screen stays up so month and year can both be chosen; OK and Cancel
                // at the foot return to the calendar, applying or discarding them.
                picker.show();
            }
        });

        if let Some(page) = &self.time_page {
            page.clock.connect_change({
                let this = this.clone();
                move |hour, minute| {
                    if let Some(picker) = this.upgrade() {
                        let mut value = picker.value.get();
                        (value.hour, value.minute) = (hour, minute);
                        picker.value.set(value);
                        picker.time_changed();
                    }
                }
            });
            page.meridiem.connect_change({
                let this = this.clone();
                move |top| {
                    let Some(picker) = this.upgrade() else { return };
                    let mut value = picker.value.get();
                    // 12 h: the top half is PM; 24 h: the top half is 00-11.
                    let pm = if super::datetime::uses_12_hour() { top } else { !top };
                    value.hour = (value.hour % 12) + if pm { 12 } else { 0 };
                    picker.value.set(value);
                    picker.time_changed();
                }
            });
            page.connect_fields_changed({
                let this = this.clone();
                move || {
                    let Some(picker) = this.upgrade() else { return };
                    let Some(page) = &picker.time_page else { return };
                    let mut value = picker.value.get();
                    let (hour, minute, second) = page.fields(value.hour >= 12);
                    (value.hour, value.minute) = (hour, minute);
                    if second.is_some() {
                        value.second = second;
                    }
                    picker.value.set(value);
                    picker.time_changed();
                }
            });
        }

        if let Some(toggle) = &self.format_toggle {
            toggle.connect_change({
                let this = this.clone();
                move || {
                    if let Some(picker) = this.upgrade() {
                        picker.refresh_time();
                        picker.reload_quick();
                    }
                }
            });
        }

        self.quick_page.connect_pick({
            let this = this.clone();
            move |apply| {
                let Some(picker) = this.upgrade() else { return };
                let mut value = picker.value.get();
                apply.to(&mut value);
                picker.value.set(value);
                picker.changed();
                if picker.kind == PickerKind::DateTimeLocal {
                    // Both a day and a time are picked here, so the page stays; its caption
                    // shows what they add up to.
                    picker.quick_page.set_caption(&picker.combined_caption());
                    return;
                }
                // Back to the section the pick belongs to, with the choice showing.
                picker.shell.select_nav(0);
                picker.show();
            }
        });

        self.shell.connect_intercept({
            let this = this.clone();
            move |ok| {
                let Some(picker) = this.upgrade() else { return false };
                if !picker.in_drill_down() {
                    return false;
                }
                if !ok {
                    if let Some(before) = picker.before_drill_down.get() {
                        picker.value.set(before);
                        picker.changed();
                    }
                }
                picker.showing_month_year.set(false);
                picker.show();
                true
            }
        });
    }

    /// In the month & year drill-down the foot buttons belong to that screen: OK keeps the
    /// month and year and returns to the calendar, Cancel returns with them as they were.
    /// The picker itself is committed from the calendar only -- except for a month input,
    /// where that screen *is* the picker.
    fn in_drill_down(&self) -> bool {
        self.section() == Section::Date && self.showing_month_year.get() && self.kind != PickerKind::Month
    }

    fn show(self: &Rc<Self>) {
        let value = self.value.get();
        match self.section() {
            Section::Date => {
                if self.showing_month_year.get() {
                    self.shell.set_title(if self.kind == PickerKind::Month {
                        self.base_title
                    } else {
                        "Select month and year"
                    });
                    self.month_year_page.set(value.year, value.month);
                    self.month_year_page.set_caption(&if self.kind == PickerKind::Month {
                        format!("Selected month: {} {}", month_name(value.month), value.year)
                    } else {
                        String::new()
                    });
                    self.shell.set_page(&self.month_year_page.widget);
                } else if let Some(page) = &self.date_page {
                    self.shell.set_title(if self.kind == PickerKind::DateTimeLocal {
                        "Select a date"
                    } else {
                        self.base_title
                    });
                    if self.kind == PickerKind::DateTimeLocal {
                        page.set_caption_suffix(format!(" at {}", clock_string(value.hour, value.minute)));
                    }
                    page.set_value(value);
                    self.shell.set_page(&page.widget);
                }
            }
            Section::Time => {
                if let Some(page) = &self.time_page {
                    self.shell.set_title(if self.kind == PickerKind::DateTimeLocal {
                        "Select a time"
                    } else {
                        self.base_title
                    });
                    self.shell.set_page(&page.widget);
                    self.refresh_time();
                }
            }
            Section::Quick => {
                self.shell.set_title("Quick select");
                self.quick_page.set_subtitle(if self.kind == PickerKind::Time {
                    "Choose a useful time without setting the clock manually."
                } else {
                    ""
                });
                self.quick_page.set_caption(&if self.kind == PickerKind::DateTimeLocal {
                    self.combined_caption()
                } else {
                    String::new()
                });
                self.reload_quick();
                self.shell.set_page(&self.quick_page.widget);
            }
        }
        // The 12 h / 24 h toggle rides in the card's corner on every section of a time kind
        // except the calendar, which has its own furniture there.
        if let Some(toggle) = &self.format_toggle {
            if self.has_time() && self.section() != Section::Date {
                self.shell.main_card.put(&toggle.area, self.shell.s(568.0), self.shell.s(30.0));
            }
        }
    }

    fn reload_quick(self: &Rc<Self>) {
        self.quick_page
            .reload(quick_select::groups(self.kind, self.kind == PickerKind::Time));
    }

    /// "Tuesday 15 September 2026 at 10:30 PM", for the pages of a datetime-local picker.
    fn combined_caption(&self) -> String {
        let value = self.value.get();
        let day = value.date().map(|date| date.format("%A %-d %B %Y").to_string()).unwrap_or_default();
        format!("Selected: {day} at {}", clock_string(value.hour, value.minute))
    }

    fn refresh_time(&self) {
        let Some(page) = &self.time_page else { return };
        let value = self.value.get();
        let caption = if self.kind == PickerKind::DateTimeLocal {
            self.combined_caption()
        } else {
            time_caption(value.hour, value.minute)
        };
        page.refresh(value.hour, value.minute, value.second, &caption);
    }

    fn time_changed(&self) {
        self.refresh_time();
        self.changed();
    }

    fn changed(&self) {
        if let Some(callback) = self.on_change.borrow().as_ref() {
            callback(self.value.get().iso(self.kind));
        }
    }

    pub fn connect_change(&self, callback: impl Fn(String) + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(callback));
    }

    /// `ok` carries the chosen value; a cancel carries nothing and the caller puts the
    /// original back.
    pub fn connect_finish(self: &Rc<Self>, callback: impl Fn(Option<String>) + 'static) {
        let picker = Rc::downgrade(self);
        let kind = self.kind;
        self.shell.connect_finish(move |ok| {
            let value = picker.upgrade().map(|p| p.value.get().iso(kind));
            callback(if ok { value } else { None });
        });
    }

    pub fn present(&self) {
        self.shell.present();
    }

    pub fn detach(&self) {
        self.shell.detach();
    }
}
