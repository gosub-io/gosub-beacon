//! Dates the way the desktop writes them: month and day names, the first day of the week and
//! whether clocks carry AM/PM.
//!
//! The Mac asks Foundation (`DateFormatter`, `Calendar.firstWeekday`, a "j" skeleton). There
//! is no such thing in Rust without ICU, so this reads the glibc locale tables that
//! `pure-rust-locales` carries, which is also where chrono's localised formatting comes from.
//! The locale is the one for times: `LC_ALL`, then `LC_TIME`, then `LANG`, as POSIX orders them.

use chrono::{Datelike, NaiveDate, Weekday};
use pure_rust_locales::{locale_match, Locale};
use std::sync::OnceLock;

/// The desktop's time locale, read once. `POSIX` when none is set or it is one glibc lacks.
pub fn locale() -> Locale {
    static LOCALE: OnceLock<Locale> = OnceLock::new();
    *LOCALE.get_or_init(|| {
        ["LC_ALL", "LC_TIME", "LANG"]
            .iter()
            .filter_map(|var| std::env::var(var).ok())
            .find(|value| !value.is_empty())
            .and_then(|value| parse(&value))
            .unwrap_or(Locale::POSIX)
    })
}

/// "nl_NL.UTF-8" -> `nl_NL`, keeping a "@modifier" when glibc has that variant.
fn parse(value: &str) -> Option<Locale> {
    let (name, modifier) = match value.split_once('@') {
        Some((name, modifier)) => (name, Some(modifier)),
        None => (value, None),
    };
    let name = name.split('.').next().unwrap_or(name);
    if name == "C" {
        return Some(Locale::POSIX);
    }
    modifier
        .and_then(|m| Locale::try_from(format!("{name}@{m}").as_str()).ok())
        .or_else(|| Locale::try_from(name).ok())
}

/// "September", or the locale's word for it, as a heading uses it: the standalone form where
/// the language has one ("сентябрь", not the "сентября" of "26 сентября").
pub fn month_name(month: u32) -> &'static str {
    let locale = locale();
    let names = locale_match!(locale => LC_TIME::ALT_MON).unwrap_or(locale_match!(locale => LC_TIME::MON));
    pick(names, month)
}

/// "Sep", or the locale's abbreviation.
pub fn short_month_name(month: u32) -> &'static str {
    let locale = locale();
    let names = locale_match!(locale => LC_TIME::AB_ALT_MON).unwrap_or(locale_match!(locale => LC_TIME::ABMON));
    pick(names, month)
}

fn pick(names: &[&'static str], month: u32) -> &'static str {
    names.get((month.max(1) as usize - 1).min(11)).copied().unwrap_or("")
}

/// A column heading for the day: two letters, as the design has room for ("Mo", "ma", "月").
pub fn weekday_initials(day: Weekday) -> String {
    let locale = locale();
    let names = locale_match!(locale => LC_TIME::ABDAY);
    let name = names.get(day.num_days_from_sunday() as usize).copied().unwrap_or("");
    name.chars().take(2).collect()
}

/// The day a week starts on here: Sunday in the US, Monday in most of Europe.
pub fn first_weekday() -> Weekday {
    first_weekday_of(locale())
}

fn first_weekday_of(locale: Locale) -> Weekday {
    // A locale with no data (POSIX) keeps ISO's Monday, which is what the pickers drew
    // before they read the locale at all.
    let Some(week) = locale_match!(locale => LC_TIME::WEEK) else {
        return Weekday::Mon;
    };
    // glibc: `week` names a date that is day 1 of the week, and `first_weekday` counts from
    // there, 1 being that day itself. Unset means 1.
    let day_one = week
        .get(1)
        .and_then(|&ymd| NaiveDate::from_ymd_opt((ymd / 10000) as i32, (ymd / 100 % 100) as u32, (ymd % 100) as u32))
        .map(|date| date.weekday())
        .unwrap_or(Weekday::Sun);
    let offset = locale_match!(locale => LC_TIME::FIRST_WEEKDAY).unwrap_or(1) - 1;
    (0..offset.rem_euclid(7)).fold(day_one, |day, _| day.succ())
}

/// Whether this locale writes times with AM/PM.
pub fn uses_12_hour() -> bool {
    uses_12_hour_of(locale())
}

fn uses_12_hour_of(locale: Locale) -> bool {
    let format = locale_match!(locale => LC_TIME::T_FMT);
    ["%p", "%P", "%r", "%I", "%l"].iter().any(|spec| format.contains(spec))
}

/// `date` formatted with chrono's specifiers, in the locale's words.
pub fn format(date: NaiveDate, pattern: &str) -> String {
    date.format_localized(pattern, locale()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_names_are_read_like_the_environment_writes_them() {
        assert_eq!(parse("nl_NL.UTF-8"), Some(Locale::nl_NL));
        assert_eq!(parse("en_US"), Some(Locale::en_US));
        assert_eq!(parse("C.UTF-8"), Some(Locale::POSIX));
        assert_eq!(parse("aa_ER.UTF-8@saaho"), Some(Locale::aa_ER_saaho));
        assert_eq!(parse("de_DE.UTF-8@euro"), Some(Locale::de_DE_euro));
        // A modifier glibc has no variant for falls back to the plain locale.
        assert_eq!(parse("de_DE.UTF-8@nonsense"), Some(Locale::de_DE));
        assert_eq!(parse("xx_YY"), None);
    }

    #[test]
    fn weeks_start_where_the_locale_starts_them() {
        assert_eq!(first_weekday_of(Locale::en_US), Weekday::Sun);
        assert_eq!(first_weekday_of(Locale::nl_NL), Weekday::Mon);
        assert_eq!(first_weekday_of(Locale::en_GB), Weekday::Mon);
        assert_eq!(first_weekday_of(Locale::POSIX), Weekday::Mon);
    }

    #[test]
    fn the_clock_follows_the_locale() {
        assert!(uses_12_hour_of(Locale::en_US));
        assert!(!uses_12_hour_of(Locale::en_GB));
        assert!(!uses_12_hour_of(Locale::nl_NL));
        assert!(!uses_12_hour_of(Locale::ja_JP));
    }
}
