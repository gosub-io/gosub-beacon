//! The user's languages, as the desktop reports them.
//!
//! Only the `Accept-Language` header for now. Both shells go through [`crate::engine`], so
//! this is where the header comes from on Linux and the Mac alike.

/// What is sent when the desktop names no language at all.
const FALLBACK: &str = "en-US,en;q=0.9";

/// At most this many languages go on the wire; past that they only add to the fingerprint.
const MAX_LANGUAGES: usize = 6;

/// The `Accept-Language` header for the desktop's preferred languages.
pub fn accept_language() -> String {
    accept_language_from(sys_locale::get_locales())
}

/// `locales` in order of preference, as BCP 47 tags ("nl-NL") or POSIX names ("nl_NL.UTF-8").
///
/// Each language is followed by its bare form ("nl-NL" then "nl"), which is how browsers
/// let a site that only has "nl" still match. Weights fall by 0.1 per entry after the first.
fn accept_language_from(locales: impl IntoIterator<Item = String>) -> String {
    let mut tags: Vec<String> = Vec::new();
    for locale in locales {
        let Some(tag) = normalize(&locale) else { continue };
        let bare = tag.split('-').next().unwrap_or(&tag).to_string();
        for t in [tag, bare] {
            if !tags.contains(&t) {
                tags.push(t);
            }
        }
    }
    tags.truncate(MAX_LANGUAGES);
    if tags.is_empty() {
        return FALLBACK.to_string();
    }

    tags.iter()
        .enumerate()
        .map(|(i, tag)| match i {
            0 => tag.clone(),
            _ => format!("{tag};q=0.{}", 10 - i),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// "nl_NL.UTF-8@euro" -> "nl-NL". `None` for the C/POSIX locale, which names no language.
fn normalize(locale: &str) -> Option<String> {
    let tag = locale.split(['.', '@']).next().unwrap_or("").replace('_', "-");
    let mut parts = tag.split('-').filter(|p| !p.is_empty());
    let language = parts.next()?.to_ascii_lowercase();
    if language == "c" || language == "posix" || !language.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut out = language;
    for part in parts {
        out.push('-');
        // Regions are upper case ("NL"), scripts title case ("Hant"), as BCP 47 writes them.
        match part.len() {
            2 => out.push_str(&part.to_ascii_uppercase()),
            4 => {
                let mut chars = part.chars();
                out.extend(chars.next().map(|c| c.to_ascii_uppercase()));
                out.push_str(&chars.as_str().to_ascii_lowercase());
            }
            _ => out.push_str(part),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(locales: &[&str]) -> String {
        accept_language_from(locales.iter().map(|s| s.to_string()))
    }

    #[test]
    fn one_language_is_followed_by_its_bare_form() {
        assert_eq!(header(&["nl-NL"]), "nl-NL,nl;q=0.9");
    }

    #[test]
    fn posix_names_are_read_as_tags() {
        assert_eq!(header(&["de_DE.UTF-8@euro"]), "de-DE,de;q=0.9");
        assert_eq!(header(&["zh_hant_tw"]), "zh-Hant-TW,zh;q=0.9");
    }

    #[test]
    fn several_languages_keep_their_order_without_repeats() {
        assert_eq!(
            header(&["nl-NL", "en-GB", "en-US", "nl"]),
            "nl-NL,nl;q=0.9,en-GB;q=0.8,en;q=0.7,en-US;q=0.6"
        );
    }

    #[test]
    fn a_bare_language_has_no_duplicate() {
        assert_eq!(header(&["fr"]), "fr");
    }

    #[test]
    fn the_list_is_capped() {
        let header = header(&["a-AA", "b-BB", "c-CC", "d-DD"]);
        assert_eq!(header.split(',').count(), MAX_LANGUAGES);
    }

    #[test]
    fn no_language_falls_back_to_english() {
        assert_eq!(header(&[]), FALLBACK);
        assert_eq!(header(&["C", "POSIX", "C.UTF-8"]), FALLBACK);
    }
}
