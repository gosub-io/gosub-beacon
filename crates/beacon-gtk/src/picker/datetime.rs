//! What the date and time pickers edit, and what the control will accept.
//!
//! Values cross the seam to the engine in the HTML forms' ISO shapes — `2026-09-15`,
//! `10:35`, `2026-09-15T10:35`, `2026-09`, `2026-W38` — so this is the one place that knows
//! how to read and write each of them. Ported from `PickerWindow.swift`'s `PickerValue` and
//! `PickerBounds`, with `chrono` doing the calendar arithmetic: ISO week numbering and leap
//! years are exactly the sort of thing that should not be hand-rolled twice.

use beacon_core::event::PickerKind;
use chrono::{Datelike, Days, Local, NaiveDate, NaiveTime, Timelike, Weekday};

/// Enough calendar fields for every kind, formatted per kind. A picker holds one of these
/// whatever it is editing, so a `datetime-local` can move between its Date and Time sections
/// without losing the half the section does not show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickerValue {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    /// `None` unless the control's step asks for seconds, or its value carried them.
    pub second: Option<u32>,
}

impl PickerValue {
    /// Today, now — the fallback when a control has no value, or an unreadable one.
    pub fn now(step: Option<i64>) -> Self {
        let now = Local::now().naive_local();
        let mut value = Self {
            year: now.year(),
            month: now.month(),
            day: now.day(),
            hour: now.hour(),
            minute: now.minute(),
            second: None,
        };
        // A step of a minute or more means the control only accepts times on that grid, so
        // "now" is rounded down onto it rather than offered as something it would reject.
        if let Some(step) = step {
            if step >= 60 {
                let minutes = (step / 60) as u32;
                value.minute = (value.minute / minutes) * minutes;
            }
        }
        value
    }

    /// From the control's sanitised ISO value. Parts the kind does not carry (a time has no
    /// date) come from today, so a `datetime-local` switched to its Date section still shows
    /// a month rather than the year zero.
    pub fn parse(text: &str, kind: PickerKind) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut value = Self::now(None);
        match kind {
            PickerKind::Date => {
                let (y, m, d) = split_date(text)?;
                NaiveDate::from_ymd_opt(y, m, d)?;
                (value.year, value.month, value.day) = (y, m, d);
            }
            PickerKind::Time => {
                let (h, min, s) = split_time(text)?;
                (value.hour, value.minute, value.second) = (h, min, s);
            }
            PickerKind::DateTimeLocal => {
                let (date, time) = text.split_once('T')?;
                let (y, m, d) = split_date(date)?;
                NaiveDate::from_ymd_opt(y, m, d)?;
                let (h, min, s) = split_time(time)?;
                (value.year, value.month, value.day) = (y, m, d);
                (value.hour, value.minute, value.second) = (h, min, s);
            }
            PickerKind::Month => {
                let (y, m) = text.split_once('-')?;
                let (y, m) = (y.parse().ok()?, m.parse().ok()?);
                NaiveDate::from_ymd_opt(y, m, 1)?;
                (value.year, value.month, value.day) = (y, m, 1);
            }
            PickerKind::Week => {
                // `2026-W38` names a week, and the picker holds the Monday of it.
                let (y, w) = text.split_once("-W")?;
                let monday = NaiveDate::from_isoywd_opt(y.parse().ok()?, w.parse().ok()?, Weekday::Mon)?;
                (value.year, value.month, value.day) = (monday.year(), monday.month(), monday.day());
            }
            PickerKind::Color => return None,
        }
        Some(value)
    }

    /// The shape the control wants back.
    pub fn iso(self, kind: PickerKind) -> String {
        let date = format!("{:04}-{:02}-{:02}", self.year, self.month, self.day);
        let time = match self.second {
            Some(second) => format!("{:02}:{:02}:{:02}", self.hour, self.minute, second),
            None => format!("{:02}:{:02}", self.hour, self.minute),
        };
        match kind {
            PickerKind::Date => date,
            PickerKind::Time => time,
            PickerKind::DateTimeLocal => format!("{date}T{time}"),
            PickerKind::Month => format!("{:04}-{:02}", self.year, self.month),
            PickerKind::Week => match self.date() {
                // The ISO week year is not always the calendar year: 1 January can belong to
                // the last week of the year before.
                Some(date) => {
                    let week = date.iso_week();
                    format!("{:04}-W{:02}", week.year(), week.week())
                }
                None => String::new(),
            },
            PickerKind::Color => String::new(),
        }
    }

    pub fn date(self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(self.year, self.month, self.day)
    }

    // Used by the time pickers, which land in the next pass; the value model and the
    // shell's section API are kept whole rather than split across two commits.
    #[allow(dead_code)]
    pub fn time(self) -> Option<NaiveTime> {
        NaiveTime::from_hms_opt(self.hour, self.minute, self.second.unwrap_or(0))
    }

    /// How many days the month has, for clamping a day that survives a month change.
    pub fn days_in(year: i32, month: u32) -> u32 {
        let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
        NaiveDate::from_ymd_opt(next_year, next_month, 1)
            .and_then(|first| first.pred_opt())
            .map(|last| last.day())
            .unwrap_or(31)
    }

    /// Move the day onto a month that may be shorter than the one it came from.
    pub fn clamp_day(&mut self) {
        let days = Self::days_in(self.year, self.month);
        if self.day > days {
            self.day = days;
        }
    }

    /// The Monday of the week this value falls in, which is what a week picker holds.
    pub fn monday(self) -> Option<NaiveDate> {
        let date = self.date()?;
        date.checked_sub_days(Days::new(u64::from(date.weekday().num_days_from_monday())))
    }

    pub fn with_date(mut self, date: NaiveDate) -> Self {
        (self.year, self.month, self.day) = (date.year(), date.month(), date.day());
        self
    }
}

/// "10:30 PM" or "22:30", depending on how this desktop writes times.
#[allow(dead_code)] // the time pickers, next pass
pub fn clock_string(hour: u32, minute: u32) -> String {
    if uses_12_hour() {
        let h = if hour.is_multiple_of(12) { 12 } else { hour % 12 };
        return format!("{h}:{minute:02} {}", if hour < 12 { "AM" } else { "PM" });
    }
    format!("{hour:02}:{minute:02}")
}

/// Whether times are written with AM/PM: the picker's own toggle once the user has touched
/// it, otherwise what the time locale's clock format says (see `locale.rs`).
#[allow(dead_code)] // the time pickers, next pass
pub fn uses_12_hour() -> bool {
    if let Some(stored) = stored_clock_format() {
        return stored == 12;
    }
    super::locale::uses_12_hour()
}

#[allow(dead_code)] // the time pickers, next pass
fn clock_format_path() -> std::path::PathBuf {
    beacon_core::paths::data_dir().join("picker-clock.txt")
}

#[allow(dead_code)] // the time pickers, next pass
fn stored_clock_format() -> Option<u32> {
    let text = std::fs::read_to_string(clock_format_path()).ok()?;
    match text.trim() {
        "12" => Some(12),
        "24" => Some(24),
        _ => None,
    }
}

/// Remember the toggle's choice for the next picker, as the Mac does.
#[allow(dead_code)] // the time pickers, next pass
pub fn set_clock_format(hours: u32) {
    if let Err(error) = std::fs::write(clock_format_path(), format!("{hours}\n")) {
        log::warn!(target: "gtk", "could not save the picker's clock format: {error}");
    }
}

fn split_date(text: &str) -> Option<(i32, u32, u32)> {
    let mut parts = text.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((year, month, day))
}

fn split_time(text: &str) -> Option<(u32, u32, Option<u32>)> {
    let mut parts = text.split(':');
    let hour: u32 = parts.next()?.parse().ok()?;
    let minute: u32 = parts.next()?.parse().ok()?;
    // Seconds may carry a fraction the picker does not edit; the whole part is enough.
    let second = match parts.next() {
        Some(text) => Some(text.split('.').next()?.parse().ok()?),
        None => None,
    };
    if hour > 23 || minute > 59 || second.is_some_and(|s| s > 59) {
        return None;
    }
    Some((hour, minute, second))
}

/// The control's `min`, `max` and `step`, parsed for its kind. `step` is in seconds for the
/// time kinds and days for the date kinds, the way the HTML spec scales them.
pub struct PickerBounds {
    pub min: Option<PickerValue>,
    pub max: Option<PickerValue>,
    pub step: Option<i64>,
    kind: PickerKind,
}

impl PickerBounds {
    pub fn new(kind: PickerKind, min: Option<&str>, max: Option<&str>, step: Option<&str>) -> Self {
        Self {
            min: min.and_then(|text| PickerValue::parse(text, kind)),
            max: max.and_then(|text| PickerValue::parse(text, kind)),
            step: step.and_then(|text| text.trim().parse::<i64>().ok()).filter(|step| *step > 0),
            kind,
        }
    }

    /// Whether a day is one the control would accept: inside `min..=max`, and on the step
    /// grid counted from `min` — or from 1970-01-01, the spec's default base.
    pub fn allows(&self, date: NaiveDate) -> bool {
        if let Some(min) = self.min.and_then(PickerValue::date) {
            if date < min {
                return false;
            }
        }
        if let Some(max) = self.max.and_then(PickerValue::date) {
            if date > max {
                return false;
            }
        }
        if self.kind == PickerKind::Date {
            if let Some(step) = self.step.filter(|step| *step > 1) {
                let base = self
                    .min
                    .and_then(PickerValue::date)
                    .unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).expect("1970-01-01 is a date"));
                if (date - base).num_days().rem_euclid(step) != 0 {
                    return false;
                }
            }
        }
        true
    }

    /// The nearest day the control accepts, searching outwards, so a picker opening on a
    /// disallowed value still lands somewhere it can commit.
    pub fn nearest_allowed(&self, from: NaiveDate) -> NaiveDate {
        if self.allows(from) {
            return from;
        }
        for offset in 1..=366 {
            for candidate in [from.checked_add_days(Days::new(offset)), from.checked_sub_days(Days::new(offset))]
                .into_iter()
                .flatten()
            {
                if self.allows(candidate) {
                    return candidate;
                }
            }
        }
        from
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(y: i32, m: u32, d: u32) -> PickerValue {
        PickerValue {
            year: y,
            month: m,
            day: d,
            hour: 0,
            minute: 0,
            second: None,
        }
    }

    #[test]
    fn a_date_round_trips() {
        let parsed = PickerValue::parse("2026-09-15", PickerKind::Date).unwrap();
        assert_eq!((parsed.year, parsed.month, parsed.day), (2026, 9, 15));
        assert_eq!(parsed.iso(PickerKind::Date), "2026-09-15");
    }

    #[test]
    fn a_time_keeps_its_seconds_only_when_it_had_them() {
        let without = PickerValue::parse("10:35", PickerKind::Time).unwrap();
        assert_eq!(without.second, None);
        assert_eq!(without.iso(PickerKind::Time), "10:35");
        let with = PickerValue::parse("10:35:09", PickerKind::Time).unwrap();
        assert_eq!(with.second, Some(9));
        assert_eq!(with.iso(PickerKind::Time), "10:35:09");
    }

    #[test]
    fn datetime_local_carries_both_halves() {
        let parsed = PickerValue::parse("2026-09-15T22:30", PickerKind::DateTimeLocal).unwrap();
        assert_eq!((parsed.year, parsed.month, parsed.day), (2026, 9, 15));
        assert_eq!((parsed.hour, parsed.minute), (22, 30));
        assert_eq!(parsed.iso(PickerKind::DateTimeLocal), "2026-09-15T22:30");
    }

    #[test]
    fn a_month_pins_the_day_to_the_first() {
        let parsed = PickerValue::parse("2026-09", PickerKind::Month).unwrap();
        assert_eq!(parsed.day, 1);
        assert_eq!(parsed.iso(PickerKind::Month), "2026-09");
    }

    #[test]
    fn a_week_is_held_as_its_monday() {
        // ISO week 38 of 2026 begins on Monday 14 September.
        let parsed = PickerValue::parse("2026-W38", PickerKind::Week).unwrap();
        assert_eq!((parsed.year, parsed.month, parsed.day), (2026, 9, 14));
        assert_eq!(parsed.iso(PickerKind::Week), "2026-W38");
    }

    #[test]
    fn the_iso_week_year_is_not_always_the_calendar_year() {
        // 1 January 2027 is a Friday, so it belongs to week 53 of 2026.
        let new_year = value(2027, 1, 1);
        assert_eq!(new_year.iso(PickerKind::Week), "2026-W53");
    }

    #[test]
    fn nonsense_does_not_parse() {
        for (text, kind) in [
            ("", PickerKind::Date),
            ("2026-13-01", PickerKind::Date),
            ("2026-02-30", PickerKind::Date),
            ("2026-09-15-01", PickerKind::Date),
            ("25:00", PickerKind::Time),
            ("10", PickerKind::Time),
            ("2026-09-15 10:35", PickerKind::DateTimeLocal),
        ] {
            assert!(PickerValue::parse(text, kind).is_none(), "{text} should not parse");
        }
    }

    #[test]
    fn february_knows_about_leap_years() {
        assert_eq!(PickerValue::days_in(2024, 2), 29);
        assert_eq!(PickerValue::days_in(2026, 2), 28);
        assert_eq!(PickerValue::days_in(2026, 12), 31);
        assert_eq!(PickerValue::days_in(2026, 4), 30);
    }

    #[test]
    fn a_day_clamps_onto_a_shorter_month() {
        let mut v = value(2026, 1, 31);
        v.month = 2;
        v.clamp_day();
        assert_eq!(v.day, 28);
    }

    #[test]
    fn bounds_refuse_what_the_control_would() {
        let bounds = PickerBounds::new(PickerKind::Date, Some("2026-09-10"), Some("2026-09-20"), None);
        assert!(!bounds.allows(NaiveDate::from_ymd_opt(2026, 9, 9).unwrap()));
        assert!(bounds.allows(NaiveDate::from_ymd_opt(2026, 9, 10).unwrap()));
        assert!(bounds.allows(NaiveDate::from_ymd_opt(2026, 9, 20).unwrap()));
        assert!(!bounds.allows(NaiveDate::from_ymd_opt(2026, 9, 21).unwrap()));
    }

    #[test]
    fn a_step_makes_a_grid_counted_from_min() {
        let bounds = PickerBounds::new(PickerKind::Date, Some("2026-09-10"), None, Some("7"));
        assert!(bounds.allows(NaiveDate::from_ymd_opt(2026, 9, 10).unwrap()));
        assert!(!bounds.allows(NaiveDate::from_ymd_opt(2026, 9, 11).unwrap()));
        assert!(bounds.allows(NaiveDate::from_ymd_opt(2026, 9, 17).unwrap()));
    }

    #[test]
    fn an_unreachable_day_finds_the_nearest_one_that_is_not() {
        let bounds = PickerBounds::new(PickerKind::Date, Some("2026-09-10"), Some("2026-09-20"), None);
        let landed = bounds.nearest_allowed(NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
        assert_eq!(landed, NaiveDate::from_ymd_opt(2026, 9, 10).unwrap());
    }

    #[test]
    fn monday_is_found_from_any_day_of_the_week() {
        // 15 September 2026 is a Tuesday.
        let monday = value(2026, 9, 15).monday().unwrap();
        assert_eq!(monday, NaiveDate::from_ymd_opt(2026, 9, 14).unwrap());
        // ...and a Monday is its own.
        assert_eq!(value(2026, 9, 14).monday().unwrap(), monday);
    }
}
