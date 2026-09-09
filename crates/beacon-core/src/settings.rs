//! Reading and writing the engine's settings store, without a toolkit.
//!
//! The store itself is the engine's (`gosub_engine::Config`): typed values, a schema with
//! a description and a default for each key, and an optional constraint. What lives here
//! is the handful of rules a *settings editor* needs and neither the engine nor a shell
//! should have to reinvent — chiefly that text typed into a box becomes a value of the
//! key's own type, and that setting a key back to its default removes the override rather
//! than storing a copy of it, so the persisted store only ever holds real customizations.
//!
//! `gosub://config` under GTK and a Preferences window under AppKit are two renderings of
//! this; both call these functions rather than each deciding what "reset" means.

use gosub_engine::{Config, Constraint, Setting, SettingInfo};

/// One row of a settings editor: what the schema says about a key, and what it is set to.
#[derive(Clone, Debug)]
pub struct SettingRow {
    pub info: SettingInfo,
    /// The value in force — the stored override, or the schema default when there is none.
    pub current: Setting,
}

impl SettingRow {
    /// Whether this key has been changed from its default. What an editor marks, and what
    /// decides whether "reset" does anything.
    pub fn is_modified(&self) -> bool {
        self.current != self.info.default
    }

    /// The constraint in one line (`left | right`, `-1 | 0-9999`), or `None` when the key
    /// takes any value of its type.
    pub fn constraint(&self) -> Option<String> {
        self.info.constraint.as_ref().map(Constraint::compact)
    }
}

/// Every key matching `filter`, sorted, with its schema and current value.
///
/// An empty filter means every key. A filter containing `*` is handed to the store as a
/// wildcard pattern; anything else is a case-insensitive substring, because that is what
/// someone typing into a search box means by it.
pub fn rows(config: &Config, filter: &str) -> Vec<SettingRow> {
    let filter = filter.trim().to_lowercase();
    let mut keys = if filter.contains('*') {
        config.find(&filter)
    } else {
        config.find("*")
    };
    if !filter.is_empty() && !filter.contains('*') {
        keys.retain(|key| key.to_lowercase().contains(&filter));
    }
    keys.sort();

    keys.into_iter()
        .filter_map(|key| {
            let info = config.get_info(&key)?;
            let current = config.get(&key).ok().flatten().unwrap_or_else(|| info.default.clone());
            Some(SettingRow { info, current })
        })
        .collect()
}

/// Parse `text` into a `Setting` of the same variant as `template`.
///
/// A settings editor hands back text — from an entry, a dropdown or a number field — and
/// the store wants a typed value. The type comes from the schema's default rather than
/// from what the text looks like, so `net.port = "8080"` stays an integer and a string
/// setting that happens to read `true` stays a string.
pub fn parse_like(template: &Setting, text: &str) -> Setting {
    let text = text.trim();
    match template {
        Setting::Bool(_) => Setting::Bool(matches!(text.to_lowercase().as_str(), "true" | "1" | "yes" | "on")),
        Setting::UInt(_) => Setting::UInt(text.parse().unwrap_or(0)),
        Setting::SInt(_) => Setting::SInt(text.parse().unwrap_or(0)),
        Setting::Float(_) => Setting::Float(text.parse().unwrap_or(0.0)),
        Setting::String(_) => Setting::String(text.to_string()),
        Setting::Map(_) => Setting::Map(text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()),
    }
}

/// Store `value` under `key`, or remove the override when it equals the default.
///
/// Returns false when the key is unknown, the value is outside the key's constraint, or
/// the store refused the write — all of which an editor should show rather than pretend
/// the edit landed.
pub fn set(config: &Config, key: &str, value: Setting) -> bool {
    let Some(info) = config.get_info(key) else {
        return false;
    };
    if let Some(constraint) = &info.constraint {
        if !constraint.allows(&value) {
            log::warn!("settings: {key} rejected: {value} is not {constraint}");
            return false;
        }
    }

    let result = if value == info.default {
        config.remove(key)
    } else {
        config.set(key, value)
    };
    if let Err(e) = result {
        log::warn!("settings: writing {key} failed: {e:?}");
        return false;
    }
    true
}

/// Store the text form of a value, typed by the key's schema. What an editor's text box,
/// dropdown or number field commits.
pub fn set_text(config: &Config, key: &str, text: &str) -> bool {
    let Some(info) = config.get_info(key) else {
        return false;
    };
    set(config, key, parse_like(&info.default, text))
}

/// Put a key back to its default, which means forgetting the override entirely.
pub fn reset(config: &Config, key: &str) -> bool {
    let Some(info) = config.get_info(key) else {
        return false;
    };
    set(config, key, info.default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_becomes_the_type_the_schema_asked_for() {
        assert_eq!(parse_like(&Setting::UInt(0), "8080"), Setting::UInt(8080));
        // A string setting whose text happens to read like a boolean stays a string.
        assert_eq!(parse_like(&Setting::String(String::new()), "true"), Setting::String("true".into()));
        assert_eq!(parse_like(&Setting::Bool(false), "yes"), Setting::Bool(true));
        assert_eq!(
            parse_like(&Setting::Map(vec![]), "left, right"),
            Setting::Map(vec!["left".into(), "right".into()])
        );
    }

    #[test]
    fn a_number_that_will_not_parse_does_not_become_a_string() {
        // The alternative -- falling back to Setting::String -- would put a value of the
        // wrong type in the store, and every later read would log "setting is not an
        // integer" rather than fail here where the typing went wrong.
        assert_eq!(parse_like(&Setting::SInt(0), "not a number"), Setting::SInt(0));
    }
}
