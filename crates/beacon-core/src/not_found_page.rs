//! The page for a `gosub://` address that no page answers, in the style of Beacon's own
//! home and help pages rather than the engine's plain built-in.
//!
//! The engine hands it every known page name, so pages added later still get listed; the
//! ones below also get a title and a line on what they are for.

use gosub_engine::internal_pages::InternalPages;

/// Beacon's internal pages in the order gosub://help shows them: name, title, purpose.
const PAGES: &[(&str, &str, &str)] = &[
    ("home", "Home", "The page a new tab opens on"),
    ("bookmarks", "Bookmarks", "Your saved bookmarks"),
    ("history", "History", "Where this tab has been"),
    ("config", "Settings", "Engine settings, editable"),
    ("version", "Version", "Build, engine and render backend"),
    ("stats", "Stats", "Rendering diagnostics and engine timings"),
    ("blank", "Blank", "An empty page"),
    ("help", "Help", "Every internal page"),
];

/// Escape text for the template, attributes included. The address is whatever was typed.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn title(name: &str) -> &str {
    PAGES.iter().find(|(n, ..)| *n == name).map_or(name, |(_, title, _)| title)
}

/// The page for `url`, whose page name is `name` (empty for a bare `gosub://`), given the
/// names of every known page.
pub fn build(name: &str, url: &str, known: &[String]) -> String {
    let (heading, sub) = if name.is_empty() {
        (
            "Which page?".to_string(),
            "An internal address needs a page name, as in <span class=\"addr\">gosub://help</span>.".to_string(),
        )
    } else {
        (
            "Page not found".to_string(),
            format!("<span class=\"addr\">{}</span> is not one of Beacon's pages.", escape(url)),
        )
    };

    let suggestion = match InternalPages::suggest(name, known).filter(|_| !name.is_empty()) {
        Some(guess) => format!(
            "<a class=\"guess\" href=\"gosub://{0}\"><div class=\"guess-label\">Did you mean</div>\
             <div class=\"guess-name\">{1}</div><div class=\"addr\">gosub://{0}</div></a>",
            escape(guess),
            escape(title(guess))
        ),
        None => String::new(),
    };

    // Beacon's pages in help's order, then any others the engine knows.
    let mut names: Vec<&str> = PAGES.iter().map(|(n, ..)| *n).filter(|n| known.iter().any(|k| k == n)).collect();
    names.extend(known.iter().map(String::as_str).filter(|k| !PAGES.iter().any(|(n, ..)| n == k)));
    let mut tiles = String::new();
    for name in names {
        let what = PAGES
            .iter()
            .find(|(n, ..)| *n == name)
            .map(|(.., what)| format!("<div class=\"tile-what\">{what}</div>"))
            .unwrap_or_default();
        tiles.push_str(&format!(
            "<a class=\"tile\" href=\"gosub://{0}\"><div class=\"tile-name\">{1}</div>\
             <div class=\"addr\">gosub://{0}</div>{what}</a>",
            escape(name),
            escape(title(name))
        ));
    }

    include_str!("../resources/not_found.html")
        .replace("{{HEADING}}", &heading)
        .replace("{{SUB}}", &sub)
        .replace("{{SUGGESTION}}", &suggestion)
        .replace("{{TILES}}", &tiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        [
            "blank",
            "bookmarks",
            "config",
            "help",
            "history",
            "home",
            "mine",
            "stats",
            "version",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn a_typo_gets_the_page_it_meant() {
        let page = build("hepl", "gosub://hepl", &known());
        assert!(page.contains("Page not found"));
        assert!(page.contains("gosub://hepl</span> is not one of"));
        assert!(page.contains("<a class=\"guess\" href=\"gosub://help\">"));
        assert!(!page.contains("{{"), "every placeholder is filled");
    }

    #[test]
    fn every_known_page_gets_a_tile_in_helps_order() {
        let page = build("nope", "gosub://nope", &known());
        assert!(!page.contains("class=\"guess\""), "nothing is close to nope");
        let home = page.find("href=\"gosub://home\"").unwrap();
        let blank = page.find("href=\"gosub://blank\"").unwrap();
        let mine = page.find("href=\"gosub://mine\"").unwrap();
        assert!(home < blank && blank < mine, "help's order, unknown pages last");
        assert!(page.contains("Engine settings, editable"));
    }

    #[test]
    fn a_bare_address_asks_which_page() {
        let page = build("", "gosub://", &known());
        assert!(page.contains("Which page?"));
        assert!(!page.contains("class=\"guess\""));
    }

    #[test]
    fn the_address_stays_text() {
        let page = build("<x>", "gosub://%3Cx%3E\"<b>", &known());
        assert!(!page.contains("\"<b>"));
        assert!(page.contains("&quot;&lt;b&gt;"));
    }
}
