//! The page a tab shows when its navigation failed.
//!
//! Rendered by the engine like any other page rather than drawn by the shell: a failed
//! navigation leaves a tab that still has history, a reload button and an address bar, and
//! replacing its *chrome* would make an error look like a different application. So the
//! shell hands the engine some HTML and everything else keeps working.
//!
//! It lives here, with its template, because every frontend needs the same page and the
//! only toolkit-specific part -- how the HTML reaches the tab -- is one command.

/// Escape the two strings that go into the template. Neither is trusted: the URL is
/// whatever was typed, and the error is whatever the network stack said.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The error page for a failed navigation to `url`.
pub fn build(url: &str, error: &str) -> String {
    page(url, error, "&#8226; Or press space and dive while you wait")
}

/// The error page for a shell on a touch screen, which has no space bar: a button starts the
/// game instead. The shell catches the navigation to it.
pub fn build_for_touch(url: &str, error: &str) -> String {
    page(url, error, r#"<a class="dive" href="gosub://dive">Dive while you wait</a>"#)
}

fn page(url: &str, error: &str, dive: &str) -> String {
    include_str!("../resources/error.html")
        .replace("{{URL}}", &escape(url))
        .replace("{{ERROR}}", &escape(error))
        .replace("{{DIVE}}", dive)
}

/// Whether a failure is worth showing a page for.
///
/// Pressing Stop reaches the shell as a failed navigation, and replacing the page someone
/// just stopped loading with an error is the opposite of what they asked for.
pub fn is_cancellation(error: &str) -> bool {
    error.to_lowercase().contains("cancel")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_and_the_error_both_reach_the_page() {
        let page = build("https://example.test/x", "no route to host");
        assert!(page.contains("https://example.test/x"));
        assert!(page.contains("no route to host"));
        assert!(!page.contains("{{URL}}"));
        assert!(!page.contains("{{ERROR}}"));
    }

    #[test]
    fn markup_in_either_string_stays_text() {
        // A page that puts an unescaped error into the DOM would let a crafted hostname
        // write the error page.
        let page = build("https://example.test/<script>", "<img onerror=x>");
        assert!(!page.contains("<script>"));
        assert!(!page.contains("<img onerror=x>"));
        assert!(page.contains("&lt;script&gt;"));
    }

    #[test]
    fn the_game_hint_names_the_input_there_is() {
        assert!(build("https://example.test/", "x").contains("Or press space and dive"));
        let touch = build_for_touch("https://example.test/", "x");
        assert!(touch.contains(r#"href="gosub://dive""#));
        assert!(!touch.contains("press space"));
    }

    #[test]
    fn stopping_a_load_is_not_an_error() {
        assert!(is_cancellation("Request cancelled"));
        assert!(!is_cancellation("connection refused"));
    }
}
