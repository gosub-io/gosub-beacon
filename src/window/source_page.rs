//! `view-source:` page generation: escaped, line-numbered, lightly highlighted HTML
//! source, rendered in the tab itself via `LoadHtml`.
//!
//! Lines are emitted as `<div>` rows (not `<pre>`: the engine's `white-space`
//! handling collapses pre text for now) with leading spaces as `&nbsp;` so
//! indentation survives.

/// Token classes the highlighter distinguishes. Deliberately coarse: enough to make
/// markup scannable, not a full grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Text content between tags.
    Plain,
    /// A whole tag except its quoted values: `<a href=` ... `>`.
    Tag,
    /// A quoted attribute value inside a tag.
    Str,
    /// `<!-- ... -->`, and `<!doctype ...>`.
    Comment,
}

/// Split `source` into classified spans (spans may contain newlines).
fn tokenize(source: &str) -> Vec<(Class, &str)> {
    fn push<'a>(spans: &mut Vec<(Class, &'a str)>, source: &'a str, class: Class, from: usize, to: usize) {
        if to > from {
            spans.push((class, &source[from..to]));
        }
    }

    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut start = 0;
    let mut pos = 0;

    while pos < bytes.len() {
        if bytes[pos] == b'<' {
            push(&mut spans, source, Class::Plain, start, pos);
            let rest = &source[pos..];
            if rest.starts_with("<!--") {
                // Comment: up to and including "-->".
                let end = rest.find("-->").map(|i| pos + i + 3).unwrap_or(bytes.len());
                push(&mut spans, source, Class::Comment, pos, end);
                pos = end;
            } else if rest.len() >= 2 && rest.as_bytes()[1] == b'!' {
                // Doctype and friends: up to '>'.
                let end = rest.find('>').map(|i| pos + i + 1).unwrap_or(bytes.len());
                push(&mut spans, source, Class::Comment, pos, end);
                pos = end;
            } else {
                // A tag; quoted values become their own spans.
                let mut tag_start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos] != b'>' {
                    let quote = bytes[pos];
                    if quote == b'"' || quote == b'\'' {
                        push(&mut spans, source, Class::Tag, tag_start, pos);
                        let value_start = pos;
                        pos += 1;
                        while pos < bytes.len() && bytes[pos] != quote {
                            pos += 1;
                        }
                        pos = (pos + 1).min(bytes.len());
                        push(&mut spans, source, Class::Str, value_start, pos);
                        tag_start = pos;
                    } else {
                        pos += 1;
                    }
                }
                pos = (pos + 1).min(bytes.len());
                push(&mut spans, source, Class::Tag, tag_start, pos);
            }
            start = pos;
        } else {
            pos += 1;
        }
    }
    push(&mut spans, source, Class::Plain, start, bytes.len());
    spans
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Emit one source line's spans, converting the line's leading spaces to `&nbsp;`.
fn emit_line(out: &mut String, spans: &[(Class, &str)]) {
    let mut at_line_start = true;
    for (class, text) in spans {
        let mut body = String::with_capacity(text.len());
        for c in text.chars() {
            if at_line_start && c == ' ' {
                body.push_str("&nbsp;");
                continue;
            }
            at_line_start = false;
            match c {
                '&' => body.push_str("&amp;"),
                '<' => body.push_str("&lt;"),
                '>' => body.push_str("&gt;"),
                '"' => body.push_str("&quot;"),
                _ => body.push(c),
            }
        }
        match class {
            Class::Plain => out.push_str(&body),
            Class::Tag => {
                out.push_str("<span class=\"t\">");
                out.push_str(&body);
                out.push_str("</span>");
            }
            Class::Str => {
                out.push_str("<span class=\"s\">");
                out.push_str(&body);
                out.push_str("</span>");
            }
            Class::Comment => {
                out.push_str("<span class=\"c\">");
                out.push_str(&body);
                out.push_str("</span>");
            }
        }
    }
}

/// The full `view-source:` page for `source`, fetched from `url`.
/// `highlighted` off gives plain escaped text (the `raw:` address prefix).
pub fn build(url: &str, source: &str, highlighted: bool) -> String {
    let source = source.replace('\t', "    ");
    let spans = if highlighted {
        tokenize(&source)
    } else {
        vec![(Class::Plain, source.as_str())]
    };

    // Split the classified spans on newlines into per-line span lists.
    let mut lines: Vec<Vec<(Class, &str)>> = vec![Vec::new()];
    for (class, text) in spans {
        let mut first = true;
        for part in text.split('\n') {
            if !first {
                lines.push(Vec::new());
            }
            first = false;
            if !part.is_empty() {
                lines.last_mut().unwrap().push((class, part));
            }
        }
    }

    let mut rows = String::new();
    for (number, line) in lines.iter().enumerate() {
        rows.push_str(&format!(
            "<div class=\"ln\"><span class=\"n\">{}</span><span class=\"l\">",
            number + 1
        ));
        emit_line(&mut rows, line);
        rows.push_str("</span></div>");
    }

    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Source of {title}</title><style>\
         body{{margin:0;padding:16px 20px;font-family:monospace;font-size:13px;color:#1c2333;background:#ffffff}}\
         .ln{{display:flex;line-height:1.45}}\
         .n{{flex:none;width:46px;padding-right:14px;text-align:right;color:#9aa3b2}}\
         .l{{flex:1;min-width:0;overflow-wrap:anywhere}}\
         .t{{color:#1d5fd1}} .s{{color:#116329}} .c{{color:#6b7280;font-style:italic}}\
         </style></head><body>{rows}</body></html>",
        title = escape(url),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_strings_and_comments_are_classified() {
        let spans = tokenize("<a href=\"x\">hi</a><!-- note -->");
        assert_eq!(
            spans,
            vec![
                (Class::Tag, "<a href="),
                (Class::Str, "\"x\""),
                (Class::Tag, ">"),
                (Class::Plain, "hi"),
                (Class::Tag, "</a>"),
                (Class::Comment, "<!-- note -->"),
            ]
        );
    }

    #[test]
    fn doctype_is_comment_colored_and_unterminated_input_survives() {
        assert_eq!(tokenize("<!doctype html>"), vec![(Class::Comment, "<!doctype html>")]);
        // Unterminated constructs must not panic or lose text.
        assert_eq!(tokenize("<div class=\"x"), vec![(Class::Tag, "<div class="), (Class::Str, "\"x")]);
        assert_eq!(tokenize("<!-- open"), vec![(Class::Comment, "<!-- open")]);
    }

    #[test]
    fn page_escapes_markup_and_numbers_lines() {
        let html = build("https://example.com/", "<b>&amp;</b>\n  indented", true);
        assert!(html.contains("&lt;b&gt;"), "source markup must be escaped");
        assert!(html.contains("&amp;amp;"));
        assert!(html.contains(">1<"));
        assert!(html.contains(">2<"));
        assert!(html.contains("&nbsp;&nbsp;indented"), "indentation survives");
        assert!(!html.contains("<b>&amp;</b>"), "raw source must not leak through");
    }

    #[test]
    fn raw_mode_emits_no_highlight_spans() {
        let html = build("https://example.com/", "<a href=\"x\">hi</a>", false);
        assert!(!html.contains("class=\"t\""));
        assert!(html.contains("&lt;a href="));
    }
}
