//! The engine's settings store, over the C ABI.
//!
//! `gosub://config` is a page under GTK because GTK can put widgets in a tab; a Mac shell
//! wants a Preferences window instead, and neither is the browser's business. So the ABI
//! hands over rows — key, description, type, current value, default, constraint — and the
//! shell decides what a boolean or a bounded number looks like on its platform.
//!
//! Snapshot then read by index, like every other list here. The rules about what a write
//! means (typing text into a value of the key's own type; setting a key to its default
//! removing the override rather than storing a copy) live in `beacon_core::settings`,
//! shared with the GTK page.

use std::ffi::c_char;

use beacon_core::settings;
use gosub_engine::{Constraint, Setting};

use crate::{to_c_string, to_str, BeaconBrowser};

/// What kind of value a setting holds, so a shell can pick an editor for it.
#[repr(u32)]
#[derive(Clone, Copy)]
pub enum BeaconSettingType {
    Bool = 0,
    Int = 1,
    Uint = 2,
    Float = 3,
    String = 4,
    /// A comma-separated list, edited as text.
    Map = 5,
}

fn type_of(setting: &Setting) -> BeaconSettingType {
    match setting {
        Setting::Bool(_) => BeaconSettingType::Bool,
        Setting::SInt(_) => BeaconSettingType::Int,
        Setting::UInt(_) => BeaconSettingType::Uint,
        Setting::Float(_) => BeaconSettingType::Float,
        Setting::String(_) => BeaconSettingType::String,
        Setting::Map(_) => BeaconSettingType::Map,
    }
}

/// Snapshot the settings whose key matches `filter`, sorted, and return how many.
///
/// An empty or NULL filter means every setting. A filter containing `*` is a wildcard
/// pattern; anything else is a case-insensitive substring, which is what someone typing
/// into a search field means.
///
/// # Safety
/// `browser` must be a live handle; `filter` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn beacon_settings_snapshot(browser: *mut BeaconBrowser, filter: *const c_char) -> usize {
    let b = browser!(browser, 0);
    let filter = to_str(filter).unwrap_or("");
    b.settings = settings::rows(b.engine.settings(), filter);
    b.settings.len()
}

macro_rules! setting_string {
    ($name:ident, $doc:literal, |$row:ident| $value:expr) => {
        #[doc = $doc]
        ///
        /// Free with [`crate::beacon_string_free`]; NULL when out of range or absent.
        ///
        /// # Safety
        /// `browser` must be a live handle from [`crate::beacon_new`].
        #[no_mangle]
        pub unsafe extern "C" fn $name(browser: *mut BeaconBrowser, index: usize) -> *mut c_char {
            let b = browser!(browser, std::ptr::null_mut());
            let Some($row) = b.settings.get(index) else {
                return std::ptr::null_mut();
            };
            match $value {
                Some(value) => to_c_string(&value),
                None => std::ptr::null_mut(),
            }
        }
    };
}

setting_string!(beacon_setting_key, "The setting's key, in dot notation.", |row| Some(
    row.info.key.clone()
));
setting_string!(
    beacon_setting_description,
    "What the setting does, from the engine's own schema.",
    |row| Some(row.info.description.clone())
);
setting_string!(
    beacon_setting_value,
    "The value in force, as text — the stored override, or the default when there is none.",
    |row| Some(row.current.value_string())
);
setting_string!(beacon_setting_default, "The schema's default, as text.", |row| Some(
    row.info.default.value_string()
));
setting_string!(
    beacon_setting_constraint,
    "The values this setting will accept, in one line (`left | right`, `-1 | 0-9999`). NULL when it takes anything of its type.",
    |row| row.constraint()
);

/// What kind of value the setting holds. `BEACON_SETTING_STRING` for an unknown index,
/// which is the type that accepts anything.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_setting_type(browser: *mut BeaconBrowser, index: usize) -> BeaconSettingType {
    let b = browser!(browser, BeaconSettingType::String);
    match b.settings.get(index) {
        Some(row) => type_of(&row.info.default),
        None => BeaconSettingType::String,
    }
}

/// Whether this setting has been changed from its default — what an editor marks, and what
/// decides whether a reset control does anything.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_setting_is_modified(browser: *mut BeaconBrowser, index: usize) -> bool {
    let b = browser!(browser, false);
    b.settings.get(index).is_some_and(|row| row.is_modified())
}

/// How many literal choices the setting is restricted to, or 0 when it is unconstrained or
/// constrained by range instead. A shell with a non-zero count should offer a popup rather
/// than a text field.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_setting_choice_count(browser: *mut BeaconBrowser, index: usize) -> usize {
    let b = browser!(browser, 0);
    match b.settings.get(index).and_then(|row| row.info.constraint.as_ref()) {
        Some(Constraint::Enum(values)) => values.len(),
        _ => 0,
    }
}

/// Choice `choice` of a constrained setting. Free with [`crate::beacon_string_free`].
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_setting_choice(browser: *mut BeaconBrowser, index: usize, choice: usize) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    match b.settings.get(index).and_then(|row| row.info.constraint.as_ref()) {
        Some(Constraint::Enum(values)) => match values.get(choice) {
            Some(value) => to_c_string(value),
            None => std::ptr::null_mut(),
        },
        _ => std::ptr::null_mut(),
    }
}

/// The bounds of a numeric setting, written into `lo` and `hi`. False when the setting is
/// not range-constrained, leaving both untouched — a stepper should then use whatever its
/// platform's default bounds are.
///
/// The widest range when the schema lists several: a setting allowing `-1` or `0-9999`
/// means one thing to a validator and another to a stepper, and the stepper's job is to
/// reach every allowed value.
///
/// # Safety
/// `browser` must be a live handle; `lo` and `hi` must point at `int64_t`s.
#[no_mangle]
pub unsafe extern "C" fn beacon_setting_range(browser: *mut BeaconBrowser, index: usize, lo: *mut i64, hi: *mut i64) -> bool {
    let b = browser!(browser, false);
    if lo.is_null() || hi.is_null() {
        return false;
    }
    let Some(Constraint::Range(ranges)) = b.settings.get(index).and_then(|row| row.info.constraint.as_ref()) else {
        return false;
    };
    let (Some(low), Some(high)) = (ranges.iter().map(|r| r.0).min(), ranges.iter().map(|r| r.1).max()) else {
        return false;
    };
    unsafe {
        std::ptr::write(lo, low as i64);
        std::ptr::write(hi, high as i64);
    }
    true
}

/// Write `value` to `key`, typed by the key's own schema: `"true"`, `"8080"`, `"left"`.
///
/// False when the key is unknown, the value is outside its constraint, or the store
/// refused the write — a shell should put the editor back rather than assume it landed.
/// Writing the default removes the override, so the stored profile only holds real changes.
///
/// Some settings (`net.*`) are read once when the engine starts, so a write may only take
/// effect on the next launch. That is the engine's behaviour, not this call's.
///
/// # Safety
/// `browser` must be a live handle; `key` and `value` NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn beacon_setting_set(browser: *mut BeaconBrowser, key: *const c_char, value: *const c_char) -> bool {
    let b = browser!(browser, false);
    let (Some(key), Some(value)) = (to_str(key), to_str(value)) else {
        return false;
    };
    settings::set_text(b.engine.settings(), key, value)
}

/// Put `key` back to its default, which means forgetting the override entirely.
///
/// # Safety
/// `browser` must be a live handle; `key` NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn beacon_setting_reset(browser: *mut BeaconBrowser, key: *const c_char) -> bool {
    let b = browser!(browser, false);
    let Some(key) = to_str(key) else { return false };
    settings::reset(b.engine.settings(), key)
}

/// The page a new tab opens on: `useragent.general.homepage`, or `gosub://home` when it is
/// unset. Free with [`crate::beacon_string_free`].
///
/// Asked for rather than hard-coded in the shell, so the setting means the same thing in
/// every frontend.
///
/// # Safety
/// `browser` must be a live handle from [`crate::beacon_new`].
#[no_mangle]
pub unsafe extern "C" fn beacon_homepage(browser: *mut BeaconBrowser) -> *mut c_char {
    let b = browser!(browser, std::ptr::null_mut());
    let homepage = b.engine.settings().get_string("useragent.general.homepage");
    to_c_string(if homepage.is_empty() { "gosub://home" } else { &homepage })
}
