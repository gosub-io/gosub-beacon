//! Dive: the submarine game behind `gosub://dive`, and behind a space press on
//! the page a failed navigation shows - the browser's take on the dinosaur.
//!
//! Everything but the view is here, shared by every shell: the rules
//! ([`game`]), the art and the painter ([`paint`]), and the words ([`font`]).
//! A shell owns a [`Dive`], ticks it sixty times a second, hands it the one
//! input it has, and shows the pixels it paints the way it shows a page.

pub mod font;
pub mod game;
pub mod paint;

use std::sync::atomic::{AtomicU64, Ordering};

/// The address the game answers to.
pub const URL: &str = "gosub://dive";
/// The page name under the `gosub://` scheme.
pub const PAGE: &str = "dive";
/// The title a tab showing the game carries.
pub const TITLE: &str = "Dive";

/// The best score of this run of the browser, across tabs and games.
static BEST: AtomicU64 = AtomicU64::new(0);

/// The seconds a frame lasts: the game is tuned for sixty a second.
pub const FRAME: std::time::Duration = std::time::Duration::from_micros(16_667);

/// One game and its frame, as a shell drives it.
pub struct Dive {
    game: game::Game,
    sprites: paint::Sprites,
    canvas: paint::Canvas,
    /// The game moved since the frame was last painted.
    dirty: bool,
}

impl Dive {
    /// A game for a view of `width` by `height` device pixels at `scale`
    /// device pixels per logical pixel, painting frames in `order`.
    pub fn new(width: u32, height: u32, scale: f32, order: paint::PixelOrder) -> Self {
        let scale = scale.max(0.5);
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        let game = game::Game::new(width as f32 / scale, height as f32 / scale, seed, BEST.load(Ordering::Relaxed));
        let mut dive = Self {
            game,
            sprites: paint::Sprites::load(),
            canvas: paint::Canvas::new(width.max(1), height.max(1), scale, order),
            dirty: true,
        };
        dive.paint();
        dive
    }

    /// Paint for a dark or a light shell from now on.
    pub fn set_dark(&mut self, dark: bool) {
        let palette = paint::Palette::for_dark(dark);
        if self.canvas.palette() != palette {
            self.canvas.set_palette(palette);
            self.dirty = true;
        }
    }

    /// The view changed size or density. Cheap when nothing changed.
    pub fn resize(&mut self, width: u32, height: u32, scale: f32) {
        let scale = scale.max(0.5);
        if self.canvas.size() == (width, height) && (self.canvas.scale() - scale).abs() < 1e-3 {
            return;
        }
        let palette = self.canvas.palette();
        self.canvas = paint::Canvas::new(width.max(1), height.max(1), scale, self.canvas.order());
        self.canvas.set_palette(palette);
        self.game.resize(width as f32 / scale, height as f32 / scale);
        self.dirty = true;
    }

    /// One frame of play, painted.
    pub fn tick(&mut self) {
        self.game.tick();
        BEST.fetch_max(self.game.best(), Ordering::Relaxed);
        self.dirty = true;
    }

    /// The one input: space, a tap, a click.
    pub fn flap(&mut self) {
        self.game.flap();
        self.dirty = true;
    }

    /// The current frame: `width * height * 4` bytes in the order chosen at
    /// construction, every pixel opaque.
    pub fn frame(&mut self) -> &[u8] {
        self.paint();
        self.canvas.pixels()
    }

    /// Paint the frame if the game moved since: ticks only move the game, so
    /// several ticks caught up in one go cost one paint, not several.
    fn paint(&mut self) {
        if self.dirty {
            self.canvas.paint(&self.game, &self.sprites);
            self.dirty = false;
        }
    }

    /// The frame's size in device pixels.
    pub fn size(&self) -> (u32, u32) {
        self.canvas.size()
    }

    pub fn game(&self) -> &game::Game {
        &self.game
    }
}

/// Whether `url` is the game's address.
pub fn is_dive(url: &url::Url) -> bool {
    matches!(url.scheme(), "gosub" | "about") && url.host_str().unwrap_or_else(|| url.path()).trim_matches('/') == PAGE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dive_paints_ticks_and_flaps() {
        let mut dive = Dive::new(640, 480, 1.0, paint::PixelOrder::Rgba);
        assert_eq!(dive.frame().len(), 640 * 480 * 4);
        dive.flap();
        assert_eq!(dive.game().phase(), game::Phase::Playing);
        dive.tick();
        dive.resize(1280, 960, 2.0);
        assert_eq!(dive.size(), (1280, 960));
        assert_eq!(dive.frame().len(), 1280 * 960 * 4);
        assert_eq!(dive.game().size(), (640.0, 480.0));
    }

    #[test]
    fn the_address_is_recognised() {
        assert!(is_dive(&url::Url::parse("gosub://dive").unwrap()));
        assert!(is_dive(&url::Url::parse("gosub://dive/").unwrap()));
        assert!(!is_dive(&url::Url::parse("gosub://config").unwrap()));
        assert!(!is_dive(&url::Url::parse("https://dive").unwrap()));
    }
}
