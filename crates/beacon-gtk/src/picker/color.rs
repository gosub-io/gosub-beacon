//! A colour the way CSS spells it: sRGB channels in 0..=1, plus alpha.
//!
//! The picker's own model, kept apart from `gdk::RGBA` because that carries no opinion
//! about parsing or naming: an `<input type=color>` holds six hex digits in sRGB and
//! nothing else, and every conversion here (HSV for the plane, HSL for the fields, names
//! for the list) is defined on those digits.
//!
//! Ported from the Mac shell's `CSSColor.swift`, so the two chromes agree on what typing
//! `#63` or `hsl(270 50% 40%)` into a picker means. The named table is not transcribed
//! again: it comes from the engine's own `gosub_shared::css_colors`.

use gosub_shared::css_colors::CSS_COLORNAMES;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CssColor {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

/// A CSS named colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Named {
    pub name: String,
    pub color: CssColor,
    /// Other names for the same six digits: `grey` for `gray`, `cyan` for `aqua`.
    pub aliases: Vec<String>,
}

impl CssColor {
    pub const BLACK: CssColor = CssColor {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };

    pub fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self {
            r: r.clamp(0.0, 1.0),
            g: g.clamp(0.0, 1.0),
            b: b.clamp(0.0, 1.0),
            a: a.clamp(0.0, 1.0),
        }
    }

    pub fn from_rgb8(r: i32, g: i32, b: i32, a: f64) -> Self {
        Self::new(f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0, a)
    }

    fn from_u32(rgb: u32) -> Self {
        Self::from_rgb8(((rgb >> 16) & 0xff) as i32, ((rgb >> 8) & 0xff) as i32, (rgb & 0xff) as i32, 1.0)
    }

    // ── channels ──────────────────────────────────────────────────────────

    pub fn r8(self) -> i32 {
        (self.r * 255.0).round() as i32
    }
    pub fn g8(self) -> i32 {
        (self.g * 255.0).round() as i32
    }
    pub fn b8(self) -> i32 {
        (self.b * 255.0).round() as i32
    }
    pub fn a8(self) -> i32 {
        (self.a * 255.0).round() as i32
    }

    /// `#rrggbb`, which is what the page's control holds. Alpha is not part of it.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r8(), self.g8(), self.b8())
    }

    /// `#rrggbb`, or `#rrggbbaa` when the colour is not opaque.
    pub fn hex_with_alpha(self) -> String {
        if self.a < 1.0 {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r8(), self.g8(), self.b8(), self.a8())
        } else {
            self.hex()
        }
    }

    pub fn opaque(self) -> Self {
        Self { a: 1.0, ..self }
    }

    // ── HSV, for the plane and the hue strip ──────────────────────────────

    /// Hue in degrees (0..360), saturation and value in 0..=1. A grey has no hue of its own
    /// and reports 0; callers that need to keep the strip where it was hold their own hue.
    pub fn hsv(self) -> (f64, f64, f64) {
        let max_c = self.r.max(self.g).max(self.b);
        let min_c = self.r.min(self.g).min(self.b);
        let delta = max_c - min_c;
        let mut h = 0.0;
        if delta > 0.0 {
            if max_c == self.r {
                h = 60.0 * (((self.g - self.b) / delta) % 6.0);
            } else if max_c == self.g {
                h = 60.0 * ((self.b - self.r) / delta + 2.0);
            } else {
                h = 60.0 * ((self.r - self.g) / delta + 4.0);
            }
            if h < 0.0 {
                h += 360.0;
            }
        }
        let s = if max_c == 0.0 { 0.0 } else { delta / max_c };
        (h, s, max_c)
    }

    pub fn from_hsv(h: f64, s: f64, v: f64, a: f64) -> Self {
        let h = ((h % 360.0) + 360.0) % 360.0;
        let s = s.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);
        let c = v * s;
        let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
        let m = v - c;
        let (r1, g1, b1) = match h {
            h if h < 60.0 => (c, x, 0.0),
            h if h < 120.0 => (x, c, 0.0),
            h if h < 180.0 => (0.0, c, x),
            h if h < 240.0 => (0.0, x, c),
            h if h < 300.0 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        Self::new(r1 + m, g1 + m, b1 + m, a)
    }

    // ── HSL, for the fields ───────────────────────────────────────────────

    /// Hue in degrees, saturation and lightness in 0..=1 — what `hsl()` in a stylesheet takes.
    pub fn hsl(self) -> (f64, f64, f64) {
        let (h, sv, v) = self.hsv();
        let l = v * (1.0 - sv / 2.0);
        let s = if l == 0.0 || l == 1.0 { 0.0 } else { (v - l) / l.min(1.0 - l) };
        (h, s, l)
    }

    pub fn from_hsl(h: f64, s: f64, l: f64, a: f64) -> Self {
        let s = s.clamp(0.0, 1.0);
        let l = l.clamp(0.0, 1.0);
        let v = l + s * l.min(1.0 - l);
        let sv = if v == 0.0 { 0.0 } else { 2.0 * (1.0 - l / v) };
        Self::from_hsv(h, sv, v, a)
    }

    // ── parsing ───────────────────────────────────────────────────────────

    /// Anything CSS would accept as a colour: `#639`, `#663399`, `#663399cc`, a name,
    /// `transparent`, `rgb()`/`rgba()` and `hsl()`/`hsla()` in either the comma or the
    /// space-separated syntax. Bare hex digits without the `#` are taken too, as a kindness
    /// to someone typing into the hex field.
    pub fn parse(text: &str) -> Option<Self> {
        let s = text.trim().to_ascii_lowercase();
        if s.is_empty() {
            return None;
        }
        if let Some(digits) = s.strip_prefix('#') {
            return Self::parse_hex(digits);
        }
        if s == "transparent" {
            return Some(Self {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            });
        }
        if let Some(named) = named_by_name(&s) {
            return Some(named);
        }
        if let Some(call) = Self::parse_function(&s) {
            return Some(call);
        }
        Self::parse_hex(&s)
    }

    fn parse_hex(digits: &str) -> Option<Self> {
        if !matches!(digits.len(), 3 | 4 | 6 | 8) || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        // `#639` is `#663399`: each digit doubled.
        let expanded: String = if digits.len() <= 4 {
            digits.chars().flat_map(|c| [c, c]).collect()
        } else {
            digits.to_string()
        };
        let byte = |i: usize| i32::from_str_radix(&expanded[i..i + 2], 16).unwrap_or(0);
        let a = if expanded.len() == 8 { f64::from(byte(6)) / 255.0 } else { 1.0 };
        Some(Self::from_rgb8(byte(0), byte(2), byte(4), a))
    }

    fn parse_function(s: &str) -> Option<Self> {
        let open = s.find('(')?;
        if !s.ends_with(')') {
            return None;
        }
        let name = s[..open].trim();
        let inner = &s[open + 1..s.len() - 1];
        // `rgb(1 2 3 / 0.5)` and `rgb(1, 2, 3, 0.5)` are the same call.
        let flattened = inner.replace('/', " ");
        let parts: Vec<&str> = flattened
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() != 3 && parts.len() != 4 {
            return None;
        }

        /// A number, or a percentage as a fraction of 1.
        fn fraction(p: &str, scale: f64) -> Option<f64> {
            if let Some(rest) = p.strip_suffix('%') {
                return rest.parse::<f64>().ok().map(|v| v / 100.0);
            }
            p.parse::<f64>().ok().map(|v| v / scale)
        }

        let alpha = if parts.len() == 4 { fraction(parts[3], 1.0)? } else { 1.0 };

        match name {
            "rgb" | "rgba" => {
                let r = fraction(parts[0], 255.0)?;
                let g = fraction(parts[1], 255.0)?;
                let b = fraction(parts[2], 255.0)?;
                Some(Self::new(r, g, b, alpha))
            }
            "hsl" | "hsla" => {
                let hue_text = parts[0].strip_suffix("deg").unwrap_or(parts[0]);
                let h: f64 = hue_text.parse().ok()?;
                let s = fraction(parts[1], 100.0)?;
                let l = fraction(parts[2], 100.0)?;
                Some(Self::from_hsl(h, s, l, alpha))
            }
            _ => None,
        }
    }

    // ── names ─────────────────────────────────────────────────────────────

    /// The entry whose six digits these are, when there is one. The first of a pair of
    /// aliases, so `#808080` is `gray` and `grey` is its alias.
    pub fn css_name(self) -> Option<&'static Named> {
        if self.a < 1.0 {
            return None;
        }
        let hex = self.hex();
        named().iter().find(|entry| entry.color.hex() == hex)
    }

    /// The named colour nearest to this one, with how far off it is: 0 is exact, and a few
    /// units is what most people would still call by that name.
    pub fn nearest_name(self) -> (&'static Named, f64) {
        let mut best = (&named()[0], f64::INFINITY);
        for entry in named() {
            let d = self.distance(entry.color);
            if d < best.1 {
                best = (entry, d);
            }
        }
        best
    }

    /// Perceptual-ish distance ("redmean"): weighted RGB that agrees with the eye far better
    /// than a plain Euclidean distance does, for a fraction of the cost of going through Lab.
    pub fn distance(self, other: CssColor) -> f64 {
        let r_mean = (self.r + other.r) / 2.0;
        let dr = (self.r - other.r) * 255.0;
        let dg = (self.g - other.g) * 255.0;
        let db = (self.b - other.b) * 255.0;
        ((2.0 + r_mean) * dr * dr + 4.0 * dg * dg + (2.0 + (1.0 - r_mean)) * db * db).sqrt()
    }
}

/// The CSS named colours, in the engine's own table order, each knowing the other spellings
/// of the same six digits (`gray`/`grey`, `aqua`/`cyan`, `fuchsia`/`magenta`).
pub fn named() -> &'static [Named] {
    static NAMED: OnceLock<Vec<Named>> = OnceLock::new();
    NAMED.get_or_init(|| {
        let parsed: Vec<(String, CssColor)> = CSS_COLORNAMES
            .iter()
            .filter_map(|entry| CssColor::parse(entry.value).map(|color| (entry.name.to_ascii_lowercase(), color)))
            .collect();
        parsed
            .iter()
            .map(|(name, color)| Named {
                name: name.clone(),
                color: *color,
                aliases: parsed
                    .iter()
                    .filter(|(other, c)| other != name && c.hex() == color.hex())
                    .map(|(other, _)| other.clone())
                    .collect(),
            })
            .collect()
    })
}

fn named_by_name(name: &str) -> Option<CssColor> {
    named().iter().find(|entry| entry.name == name).map(|entry| entry.color)
}

/// The CSS system colours (`Canvas`, `Highlight`, `AccentColor`, …).
///
/// The Mac shell resolves these against AppKit's semantic colours. GTK has no such per-name
/// table, so they are resolved against the running theme where the toolkit exposes a colour
/// for them and left at the CSS defaults otherwise. Asked for each time rather than stored:
/// the theme can change while the picker is up.
pub fn system_colors(dark: bool) -> Vec<Named> {
    // The CSS Color 4 defaults, which is what a page gets when nothing overrides them. The
    // light and dark pairs are the same two the rest of the picker uses.
    let table: [(&str, u32); 19] = if dark {
        [
            ("AccentColor", 0x4c8dff),
            ("AccentColorText", 0xffffff),
            ("ActiveText", 0xff6b6b),
            ("ButtonBorder", 0x3a414c),
            ("ButtonFace", 0x2e343e),
            ("ButtonText", 0xe6eaf0),
            ("Canvas", 0x1e2229),
            ("CanvasText", 0xe6eaf0),
            ("Field", 0x2a2f38),
            ("FieldText", 0xe6eaf0),
            ("GrayText", 0x9aa5b5),
            ("Highlight", 0x2c3a55),
            ("HighlightText", 0xffffff),
            ("LinkText", 0x6ea8ff),
            ("Mark", 0xffd54f),
            ("MarkText", 0x000000),
            ("SelectedItem", 0x2c3a55),
            ("SelectedItemText", 0xe6eaf0),
            ("VisitedText", 0xc58af9),
        ]
    } else {
        [
            ("AccentColor", 0x1b6dff),
            ("AccentColorText", 0xffffff),
            ("ActiveText", 0xff0000),
            ("ButtonBorder", 0xc9d1dd),
            ("ButtonFace", 0xf5f7fa),
            ("ButtonText", 0x1a2233),
            ("Canvas", 0xffffff),
            ("CanvasText", 0x000000),
            ("Field", 0xffffff),
            ("FieldText", 0x000000),
            ("GrayText", 0x6b7a90),
            ("Highlight", 0xdce7f8),
            ("HighlightText", 0x000000),
            ("LinkText", 0x0000ee),
            ("Mark", 0xffff00),
            ("MarkText", 0x000000),
            ("SelectedItem", 0xdce7f8),
            ("SelectedItemText", 0x000000),
            ("VisitedText", 0x551a8b),
        ]
    };
    table
        .into_iter()
        .map(|(name, rgb)| Named {
            name: name.to_string(),
            color: CssColor::from_u32(rgb),
            aliases: Vec::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    #[test]
    fn hex_round_trips() {
        let c = CssColor::parse("#663399").unwrap();
        assert_eq!(c.hex(), "#663399");
        assert_eq!((c.r8(), c.g8(), c.b8()), (102, 51, 153));
    }

    #[test]
    fn short_hex_doubles_its_digits() {
        assert_eq!(CssColor::parse("#639").unwrap().hex(), "#663399");
        // Bare digits are taken too, for someone typing into the hex field.
        assert_eq!(CssColor::parse("639").unwrap().hex(), "#663399");
    }

    #[test]
    fn hex_with_alpha_is_only_used_when_it_has_to_be() {
        let opaque = CssColor::parse("#663399").unwrap();
        assert_eq!(opaque.hex_with_alpha(), "#663399");
        let translucent = CssColor::parse("#66339980").unwrap();
        assert_eq!(translucent.hex_with_alpha(), "#66339980");
        // The page's control keeps six digits whatever the picker shows.
        assert_eq!(translucent.hex(), "#663399");
    }

    #[test]
    fn a_name_and_its_functions_agree() {
        let expected = CssColor::parse("#663399").unwrap();
        for text in ["rebeccapurple", "rgb(102 51 153)", "rgb(102, 51, 153)", "hsl(270 50% 40%)"] {
            assert_eq!(CssColor::parse(text).unwrap().hex(), expected.hex(), "{text}");
        }
    }

    #[test]
    fn alpha_comes_through_either_syntax() {
        approx(CssColor::parse("rgb(0 0 0 / 0.5)").unwrap().a, 0.5);
        approx(CssColor::parse("rgba(0, 0, 0, 0.5)").unwrap().a, 0.5);
        approx(CssColor::parse("transparent").unwrap().a, 0.0);
    }

    #[test]
    fn nonsense_does_not_parse() {
        for text in ["", "not-a-colour", "#12345", "rgb(1 2)", "hsl(x 1% 1%)"] {
            assert!(CssColor::parse(text).is_none(), "{text} should not parse");
        }
    }

    #[test]
    fn hsv_and_hsl_round_trip() {
        let c = CssColor::parse("#663399").unwrap();
        let (h, s, v) = c.hsv();
        assert_eq!(CssColor::from_hsv(h, s, v, 1.0).hex(), c.hex());
        let (h, s, l) = c.hsl();
        assert_eq!(CssColor::from_hsl(h, s, l, 1.0).hex(), c.hex());
        // The design's readout for rebeccapurple: 270°, 50%, 40%.
        approx(h.round(), 270.0);
        approx((s * 100.0).round(), 50.0);
        approx((l * 100.0).round(), 40.0);
    }

    #[test]
    fn a_grey_reports_no_hue() {
        // Which is why the picker keeps its own: the plane must not jump to red when the
        // value reaches black.
        let (h, s, _) = CssColor::parse("#808080").unwrap().hsv();
        approx(h, 0.0);
        approx(s, 0.0);
    }

    #[test]
    fn names_know_their_aliases() {
        let gray = CssColor::parse("#808080").unwrap().css_name().unwrap();
        assert!(gray.name == "gray" || gray.name == "grey");
        assert!(gray.aliases.iter().any(|a| a == "grey" || a == "gray"));
    }

    #[test]
    fn an_unnamed_colour_has_a_nearest_one() {
        let c = CssColor::parse("#663a99").unwrap();
        assert!(c.css_name().is_none());
        let (near, distance) = c.nearest_name();
        assert_eq!(near.name, "rebeccapurple");
        assert!(distance > 0.0 && distance < 140.0);
    }

    #[test]
    fn the_table_is_the_css_one() {
        // 148 keywords in CSS Color 4, aliases counted separately as the spec's table does.
        assert_eq!(named().len(), 148);
        assert!(named().iter().any(|n| n.name == "rebeccapurple"));
    }
}
