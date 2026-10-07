//! Drawing the game into a pixel buffer. One painter for every shell: a
//! shell shows the buffer the way it shows a page's tiles, and nothing
//! about how the game looks depends on the toolkit.
//!
//! The buffer is straight-alpha RGBA, eight bits per channel, row-major,
//! sized in device pixels; `scale` says how many device pixels one logical
//! pixel of the game is, so sprites stay crisp on a high-density screen.

use super::font;
use super::game::{Fish, Game, Phase, Species, WalkerKind, Weed, Wreck, JELLY_SIZE, STARFISH_SIZE, SUB_H, SUB_W, WEED_W};
use gosub_render_pipeline::common::media::{DecodedMedia, MediaDecoderRegistry};

/// A decoded sprite: straight RGBA pixels.
pub struct Sprite {
    pub width: u32,
    pub height: u32,
    pixels: Vec<u8>,
}

impl Sprite {
    fn decode(png: &[u8]) -> Sprite {
        let registry = MediaDecoderRegistry::with_defaults();
        match registry.decode(Some("image/png"), png) {
            Ok(DecodedMedia::Raster(image)) => Sprite {
                width: image.width(),
                height: image.height(),
                pixels: image.as_raw().to_vec(),
            },
            // A sprite that fails to decode draws as nothing; the game still plays.
            _ => Sprite {
                width: 0,
                height: 0,
                pixels: Vec::new(),
            },
        }
    }

    /// The sprite at half size, each pixel the alpha-weighted mean of a two
    /// by two block: the sheet's art is drawn at double scale, and this keeps
    /// its outlines where a nearest pick would drop every other line.
    fn halved(self) -> Sprite {
        if self.width < 2 || self.height < 2 {
            return self;
        }
        let (w, h) = (self.width / 2, self.height / 2);
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = self.pixel(x * 2 + dx, y * 2 + dy);
                    let pa = p[3] as u32;
                    r += p[0] as u32 * pa;
                    g += p[1] as u32 * pa;
                    b += p[2] as u32 * pa;
                    a += pa;
                }
                match a {
                    0 => pixels.extend_from_slice(&[0, 0, 0, 0]),
                    a => pixels.extend_from_slice(&[(r / a) as u8, (g / a) as u8, (b / a) as u8, (a / 4) as u8]),
                }
            }
        }
        Sprite {
            width: w,
            height: h,
            pixels,
        }
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]]
    }
}

/// One animal's frames from the sheet, at half size.
macro_rules! animal {
    ($($file:literal),+ $(,)?) => {
        vec![$(Sprite::decode(include_bytes!(concat!("../../resources/dive/animals/", $file))).halved()),+]
    };
}

/// The game's art, decoded once: the submarine and the sea life at half the
/// sheet's size, the original bubbles and axolotl as they were.
pub struct Sprites {
    /// The submarine's four propeller frames.
    pub sub: Vec<Sprite>,
    pub bubbles: [Sprite; 3],
    pub axolotl: Sprite,
    pub crab: Vec<Sprite>,
    pub jellyfish: Vec<Sprite>,
    pub starfish: Sprite,
    /// Indexed as [`Species::ALL`].
    pub species: Vec<Vec<Sprite>>,
}

impl Sprites {
    pub fn load() -> Self {
        Self {
            sub: vec![
                Sprite::decode(include_bytes!("../../resources/dive/sub/sub-0.png"))
                    .halved()
                    .halved(),
                Sprite::decode(include_bytes!("../../resources/dive/sub/sub-1.png"))
                    .halved()
                    .halved(),
                Sprite::decode(include_bytes!("../../resources/dive/sub/sub-2.png"))
                    .halved()
                    .halved(),
                Sprite::decode(include_bytes!("../../resources/dive/sub/sub-3.png"))
                    .halved()
                    .halved(),
            ],
            bubbles: [
                Sprite::decode(include_bytes!("../../resources/dive/bubble-sm.png")),
                Sprite::decode(include_bytes!("../../resources/dive/bubble-md.png")),
                Sprite::decode(include_bytes!("../../resources/dive/bubble-lg.png")),
            ],
            axolotl: Sprite::decode(include_bytes!("../../resources/dive/axolotl.png")),
            crab: animal!["crab-0.png", "crab-1.png", "crab-2.png", "crab-3.png"],
            jellyfish: animal!["jellyfish-0.png", "jellyfish-1.png", "jellyfish-2.png", "jellyfish-3.png"],
            starfish: Sprite::decode(include_bytes!("../../resources/dive/animals/starfish-0.png")).halved(),
            species: vec![
                animal!["minnow-0.png", "minnow-1.png", "minnow-2.png", "minnow-3.png"],
                animal!["damselfish-0.png", "damselfish-1.png", "damselfish-2.png", "damselfish-3.png"],
                animal!["herring-0.png", "herring-1.png", "herring-2.png", "herring-3.png"],
                animal!["angelfish-0.png", "angelfish-1.png", "angelfish-2.png", "angelfish-3.png"],
                animal![
                    "butterflyfish-0.png",
                    "butterflyfish-1.png",
                    "butterflyfish-2.png",
                    "butterflyfish-3.png"
                ],
                animal!["grouper-0.png", "grouper-1.png", "grouper-2.png", "grouper-3.png"],
                animal!["pufferfish-0.png", "pufferfish-1.png", "pufferfish-2.png"],
                animal!["tuna-0.png", "tuna-1.png", "tuna-2.png"],
                animal!["turtle-0.png", "turtle-1.png", "turtle-2.png", "turtle-3.png"],
                animal!["ray-0.png", "ray-1.png", "ray-2.png"],
                animal!["eel-0.png", "eel-1.png", "eel-2.png", "eel-3.png", "eel-4.png", "eel-5.png"],
            ],
        }
    }

    /// The frames of `species`.
    pub fn of(&self, species: Species) -> &[Sprite] {
        let index = Species::ALL.iter().position(|s| *s == species).unwrap_or(0);
        &self.species[index]
    }
}

/// Draw the hit boxes of the hull and the kelp in red over the frame: what
/// the game decides a collision on, for tuning the boxes against the art.
pub const SHOW_HITBOXES: bool = false;
const HITBOX: [u8; 3] = [220, 30, 30];

/// The colours of the water, the sand and the words, and whether the art is
/// shown as drawn or with its light and dark swapped: the sprites are dark
/// marks on light water, which on dark water would vanish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub water: [u8; 3],
    pub sand: [u8; 3],
    pub ink: [u8; 3],
    pub message: [u8; 3],
    pub invert_sprites: bool,
}

impl Palette {
    /// The game as drawn: grey water, darker sand, dark ink.
    pub const LIGHT: Palette = Palette {
        water: [192, 192, 192],
        sand: [164, 164, 164],
        ink: [64, 64, 64],
        message: [110, 110, 110],
        invert_sprites: false,
    };
    /// For a dark shell: deep water, darker sand, light ink, the art inverted.
    pub const DARK: Palette = Palette {
        water: [46, 46, 54],
        sand: [34, 34, 40],
        ink: [210, 210, 218],
        message: [160, 160, 170],
        invert_sprites: true,
    };

    pub fn for_dark(dark: bool) -> Palette {
        if dark {
            Palette::DARK
        } else {
            Palette::LIGHT
        }
    }
}

/// The byte order of a frame's pixels, chosen by the shell for what it
/// shows them with: RGBA for most, BGRA for Cairo and Core Graphics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelOrder {
    Rgba,
    Bgra,
}

impl PixelOrder {
    /// Where red, green and blue sit in a pixel's four bytes.
    fn rgb(self) -> [usize; 3] {
        match self {
            PixelOrder::Rgba => [0, 1, 2],
            PixelOrder::Bgra => [2, 1, 0],
        }
    }
}

/// A pixel buffer the game is painted into.
pub struct Canvas {
    width: u32,
    height: u32,
    scale: f32,
    order: PixelOrder,
    palette: Palette,
    pixels: Vec<u8>,
}

impl Canvas {
    pub fn new(width: u32, height: u32, scale: f32, order: PixelOrder) -> Self {
        Self {
            width,
            height,
            scale: scale.max(0.5),
            order,
            palette: Palette::LIGHT,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }

    pub fn order(&self) -> PixelOrder {
        self.order
    }

    pub fn palette(&self) -> Palette {
        self.palette
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The frame, `width * height * 4` bytes in the canvas's [`PixelOrder`];
    /// every pixel opaque, so straight and premultiplied alpha agree.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// One opaque pixel in the canvas's byte order.
    fn opaque(&self, rgb: [u8; 3]) -> [u8; 4] {
        let [r, g, b] = self.order.rgb();
        let mut px = [255u8; 4];
        px[r] = rgb[0];
        px[g] = rgb[1];
        px[b] = rgb[2];
        px
    }

    /// The water: lighter towards the surface, the water's own colour by the
    /// time it reaches the sand, with a slow haze drifting through it and a
    /// few rays of light coming down from the surface, so the scene has depth
    /// and movement before anything is in it. The haze and the rays are worked
    /// out once per four-by-four cell and folded into the fill through a small
    /// table per band of rows: at a few levels of difference nobody sees the
    /// cells, and the frame stays one pass.
    fn water(&mut self, water: [u8; 3], floor_device_y: i32, distance: f32) {
        const CELL: usize = 4;
        /// The haze and the rays together stay within this many levels.
        const RANGE: i32 = 12;
        let surface = Self::mix(water, [255, 255, 255], 3);
        let height = self.height as usize;
        let width = self.width as usize;
        let floor = (floor_device_y.max(1) as usize).min(height);
        let s = self.scale;
        let drift = distance * 0.25 * s;
        let sway = (distance * 0.01).sin() * 30.0 * s;
        let cells = width.div_ceil(CELL);
        let mut deltas: Vec<i32> = vec![0; cells];
        let mut table: Vec<[u8; 4]> = vec![[0; 4]; (2 * RANGE + 1) as usize];
        for y in 0..height {
            let t = ((y * 16) / floor).min(16) as u32;
            let base = Self::mix(surface, water, t);
            if y >= floor {
                // Below the sand line the sand is painted over this anyway.
                let px = self.opaque(base);
                let row = &mut self.pixels[y * width * 4..(y + 1) * width * 4];
                for out in row.as_chunks_mut::<4>().0 {
                    *out = px;
                }
                continue;
            }
            if y % CELL == 0 {
                // A new band: its cells' deltas, and the colours they map to at
                // this depth (the band is four rows, close enough in tone).
                let depth = y as f32 / floor as f32;
                let fade = (1.0 - depth * 2.0).clamp(0.0, 1.0);
                let fy = y as f32;
                for (c, delta) in deltas.iter_mut().enumerate() {
                    let cx = (c * CELL) as f32;
                    let x = cx + drift;
                    let haze = ((x * 0.011 + fy * 0.007).sin() + (x * 0.004 - fy * 0.013 + 1.7).sin()) * 2.5;
                    let along = (cx + fy * 0.45 + sway) / (260.0 * s);
                    let band = (along - along.floor() - 0.5).abs();
                    let ray = (1.0 - band * 9.0).clamp(0.0, 1.0) * 7.0 * fade;
                    *delta = ((haze + ray).round() as i32).clamp(-RANGE, RANGE);
                }
                for (i, entry) in table.iter_mut().enumerate() {
                    let d = i as i32 - RANGE;
                    let lift = |v: u8| (v as i32 + d).clamp(0, 255) as u8;
                    *entry = self.opaque([lift(base[0]), lift(base[1]), lift(base[2])]);
                }
            }
            if y % CELL == 0 {
                // The band's first row is computed; the rows under it are copies.
                let row = &mut self.pixels[y * width * 4..(y + 1) * width * 4];
                for (c, out) in row.as_chunks_mut::<16>().0.iter_mut().enumerate() {
                    let px = table[(deltas[c] + RANGE) as usize];
                    out[..4].copy_from_slice(&px);
                    out[4..8].copy_from_slice(&px);
                    out[8..12].copy_from_slice(&px);
                    out[12..].copy_from_slice(&px);
                }
                let tail = (width / CELL) * CELL;
                for x in tail..width {
                    let i = x * 4;
                    row[i..i + 4].copy_from_slice(&table[(deltas[x / CELL] + RANGE) as usize]);
                }
            } else {
                let band_start = (y - y % CELL) * width * 4;
                self.pixels.copy_within(band_start..band_start + width * 4, y * width * 4);
            }
        }
    }

    /// An opaque rectangle in device pixels.
    fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, rgb: [u8; 3]) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.width as i32);
        let y1 = (y + h).min(self.height as i32);
        let out = self.opaque(rgb);
        for py in y0..y1 {
            for px in x0..x1 {
                let i = ((py as u32 * self.width + px as u32) * 4) as usize;
                self.pixels[i..i + 4].copy_from_slice(&out);
            }
        }
    }

    /// Blend one straight-alpha pixel over the canvas.
    fn blend(&mut self, x: i32, y: i32, rgba: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 || rgba[3] == 0 {
            return;
        }
        let i = ((y as u32 * self.width + x as u32) * 4) as usize;
        let a = rgba[3] as u32;
        for (c, &over) in self.order.rgb().iter().zip(rgba.iter()) {
            let under = self.pixels[i + c] as u32;
            self.pixels[i + c] = ((over as u32 * a + under * (255 - a)) / 255) as u8;
        }
        self.pixels[i + 3] = 255;
    }

    /// A sprite stretched into a rectangle given in logical pixels; nearest
    /// sampling, optionally mirrored top to bottom. A sprite at its own size
    /// is the common case and lands pixel for pixel at integer scales.
    fn draw_sprite(&mut self, sprite: &Sprite, x: f32, y: f32, w: f32, h: f32, flip_v: bool) {
        self.draw_sprite_flipped(sprite, x, y, w, h, flip_v, false);
    }

    /// [`draw_sprite`](Self::draw_sprite), mirrored left to right as well when
    /// `flip_h`: the sheet's animals all face right.
    #[allow(clippy::too_many_arguments)]
    fn draw_sprite_flipped(&mut self, sprite: &Sprite, x: f32, y: f32, w: f32, h: f32, flip_v: bool, flip_h: bool) {
        if sprite.width == 0 || sprite.height == 0 || w <= 0.0 || h <= 0.0 {
            return;
        }
        let s = self.scale;
        let (dx0, dy0) = ((x * s).round() as i32, (y * s).round() as i32);
        let (dw, dh) = (((w * s).round() as i32).max(1), ((h * s).round() as i32).max(1));
        let invert = self.palette.invert_sprites;
        for row in 0..dh {
            let mut sy = (row as u32 * sprite.height) / dh as u32;
            if flip_v {
                sy = sprite.height - 1 - sy;
            }
            for col in 0..dw {
                let mut sx = (col as u32 * sprite.width) / dw as u32;
                if flip_h {
                    sx = sprite.width - 1 - sx;
                }
                let mut px = sprite.pixel(sx, sy);
                if invert {
                    px[0] = 255 - px[0];
                    px[1] = 255 - px[1];
                    px[2] = 255 - px[2];
                }
                self.blend(dx0 + col, dy0 + row, px);
            }
        }
    }

    /// Text in the pixel font at logical position `(x, y)`, each font pixel
    /// `px` logical pixels square.
    fn text(&mut self, text: &str, x: f32, y: f32, px: f32, rgb: [u8; 3]) {
        let s = self.scale;
        let cell = (px * s).round().max(1.0) as i32;
        let mut cursor = (x * s).round() as i32;
        let top = (y * s).round() as i32;
        for c in text.chars() {
            for (row, line) in font::glyph(c).iter().enumerate() {
                for (col, ink) in line.chars().enumerate() {
                    if ink == '#' {
                        self.fill_rect(cursor + col as i32 * cell, top + row as i32 * cell, cell, cell, rgb);
                    }
                }
            }
            cursor += font::ADVANCE as i32 * cell;
        }
    }

    /// Text centred on `cx`.
    fn text_centered(&mut self, text: &str, cx: f32, y: f32, px: f32, rgb: [u8; 3]) {
        let w = font::width(text) as f32 * px;
        self.text(text, cx - w / 2.0, y, px, rgb);
    }

    /// The sea floor: a rippled top edge, speckles and the odd ripple line, all
    /// drawn against the distance scrolled so the sand moves with the world.
    fn sand(&mut self, game: &Game, sand: [u8; 3], water: [u8; 3], ink: [u8; 3]) {
        let (w, _) = game.size();
        let floor = game.floor();
        let s = self.scale;
        let offset = game.distance();
        // A mix of the sand and the ink, and of the sand and the water, for grains.
        let dark_grain = Self::mix(sand, ink, 3);
        let light_grain = Self::mix(sand, water, 6);
        let top = (floor * s).round() as i32;
        self.fill_rect(0, top, self.width as i32, self.height as i32, sand);
        // The edge: a slow wave, one or two logical pixels of sand above the line.
        for col in 0..(w as i32) {
            let world = col as f32 + offset;
            let lift = ((world / 23.0).sin() * 1.5 + (world / 7.0).sin() * 0.5 + 1.0).round().max(0.0);
            if lift > 0.0 {
                self.fill_rect(
                    (col as f32 * s) as i32,
                    top - (lift * s) as i32,
                    s.ceil() as i32,
                    (lift * s).ceil() as i32,
                    sand,
                );
            }
        }
        // Grains and ripples, placed by a hash of their world position.
        let hash = |x: i32, y: i32| -> u32 {
            let mut v = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
            v ^= v >> 13;
            v = v.wrapping_mul(0xC2B2_AE3D);
            v ^ (v >> 16)
        };
        // Grains two logical pixels square on a two-pixel grid: a quarter of
        // the hashing of a per-pixel pass, and sand is not finer than that.
        let rows = (game.size().1 - floor) as i32;
        let grain_px = (2.0 * s).ceil() as i32;
        let first_col = ((offset / 2.0).floor() as i32) * 2;
        for row in (2..rows).step_by(2) {
            let mut world_x = first_col;
            while (world_x as f32 - offset) < w {
                let col = world_x as f32 - offset;
                let h = hash(world_x, row);
                let grain = match h % 41 {
                    0 | 1 => Some(dark_grain),
                    2 => Some(light_grain),
                    _ => None,
                };
                if let Some(rgb) = grain {
                    self.fill_rect((col * s) as i32, ((floor + row as f32) * s) as i32, grain_px, grain_px, rgb);
                }
                // A short ripple line now and then, three grains long.
                if h % 601 == 7 {
                    self.fill_rect(
                        (col * s) as i32,
                        ((floor + row as f32) * s) as i32,
                        (6.0 * s) as i32,
                        s.ceil() as i32,
                        light_grain,
                    );
                }
                world_x += 2;
            }
        }
    }

    /// A fish from the sheet, centred on its position, facing the way it
    /// swims; a school draws its five minnows around the lead.
    fn fish(&mut self, fish: &Fish, sprites: &Sprites) {
        let frames = sprites.of(fish.species);
        if frames.is_empty() {
            return;
        }
        let (w, h) = fish.species.size();
        let members: &[(f32, f32)] = if fish.school { &Fish::SCHOOL } else { &Fish::SCHOOL[..1] };
        for (i, (dx, dy)) in members.iter().enumerate() {
            let sprite = &frames[fish.frame(i as u32) as usize % frames.len()];
            // The school trails behind its lead: offsets are drawn for a
            // fish heading left, mirrored for one heading right.
            let dx = if fish.faces_right() { -dx } else { *dx };
            let (sw, sh) = (sprite.width as f32, sprite.height as f32);
            // Centred on the species' nominal size, so frames of different
            // extents do not jitter.
            let x = fish.x + dx - sw / 2.0;
            let y = fish.y + dy + (h - sh) / 2.0;
            let _ = w;
            self.draw_sprite_flipped(sprite, x, y, sw, sh, false, !fish.faces_right());
        }
    }

    /// A one-logical-pixel outline of `(left, top, right, bottom)`.
    fn outline(&mut self, (l, t, r, b): (f32, f32, f32, f32), rgb: [u8; 3]) {
        let (w, h) = (r - l, b - t);
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.block(l, t, w, 1.0, rgb);
        self.block(l, b - 1.0, w, 1.0, rgb);
        self.block(l, t, 1.0, h, rgb);
        self.block(r - 1.0, t, 1.0, h, rgb);
    }

    /// `a` towards `b` by `t` sixteenths.
    fn mix(a: [u8; 3], b: [u8; 3], t: u32) -> [u8; 3] {
        let m = |x: u8, y: u8| ((x as u32 * (16 - t) + y as u32 * t) / 16) as u8;
        [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
    }

    /// A logical-pixel block, `w` by `h` of them, at a logical position.
    /// [`block`](Self::block) blended over what is there at `alpha`, or laid
    /// down opaque at 255.
    fn block_alpha(&mut self, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3], alpha: u8) {
        if alpha == 255 {
            self.block(x, y, w, h, rgb);
            return;
        }
        let s = self.scale;
        let (x0, y0) = ((x * s).round() as i32, (y * s).round() as i32);
        let (w, h) = (((w * s).round() as i32).max(1), ((h * s).round() as i32).max(1));
        let px = [rgb[0], rgb[1], rgb[2], alpha];
        for py in y0..y0 + h {
            for px_x in x0..x0 + w {
                self.blend(px_x, py, px);
            }
        }
    }

    fn block(&mut self, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3]) {
        let s = self.scale;
        self.fill_rect(
            (x * s).round() as i32,
            (y * s).round() as i32,
            ((w * s).round() as i32).max(1),
            ((h * s).round() as i32).max(1),
            rgb,
        );
    }

    /// One strand of kelp: a stalk from `from` towards `to` (`dir` is +1 for
    /// one hanging down, -1 for one growing up), swaying with the current,
    /// with fronds along it reaching outward and away from the root. The
    /// stalk and the fronds in their own colours; `fade` is a distant strand,
    /// thinner and with shorter fronds.
    #[allow(clippy::too_many_arguments)]
    fn kelp(
        &mut self,
        centre: f32,
        from: f32,
        to: f32,
        dir: f32,
        wave: f32,
        phase: f32,
        stalk: [u8; 3],
        frond: [u8; 3],
        fade: bool,
        alpha: u8,
        seed: f32,
    ) {
        let length = (to - from).abs();
        if length < 4.0 {
            return;
        }
        let stalk_w = if fade { 3.0 } else { 6.0 };
        let mut d = 0.0;
        while d < length {
            let y = from + d * dir;
            // Sway grows along the strand: the root holds, the tip swings.
            let reach = (d / length).min(1.0);
            let sway = ((d / wave) + phase).sin() * (2.0 + 9.0 * reach);
            let x = centre + sway;
            let width = if reach > 0.9 { stalk_w * 0.5 } else { stalk_w };
            self.block_alpha(x - width / 2.0, y, width.max(1.0), 1.0, stalk, alpha);
            // A frond every few pixels, alternating sides: a tapering stroke
            // that leaves the stalk outward and bends away from the root.
            let step = d as i32;
            let every = if fade { 12 } else { 6 };
            if step % every == 3 && reach < 0.95 && d > 6.0 {
                let side = if (step / every) % 2 == 0 { 1.0 } else { -1.0 };
                // Each frond's length is the strand's own, fixed by its seed:
                // tied to the sway it would change every frame and flicker.
                let swing = ((step * 7) as f32 + seed * 31.0).sin() * 0.5 + 0.5;
                let len = if fade { 8.0 + 6.0 * swing } else { 16.0 + 14.0 * swing } * (1.0 - 0.4 * reach);
                let mut k = 0.0;
                while k < len {
                    let t = k / len;
                    let fx = x + side * (stalk_w / 2.0 + k * 0.8);
                    let fy = y + dir * (k * 0.5 + t * t * 8.0);
                    let thick = if fade {
                        if t < 0.5 {
                            2.0
                        } else {
                            1.0
                        }
                    } else if t < 0.25 {
                        4.0
                    } else if t < 0.6 {
                        3.0
                    } else {
                        2.0
                    };
                    self.block_alpha(fx, fy, thick, thick, frond, alpha);
                    // Step by most of the brush: a frond painted pixel by pixel
                    // writes each of its pixels four times over.
                    k += (thick * 0.7).max(1.0);
                }
            }
            d += 1.0;
        }
    }

    /// One stand of weed as an obstacle: a vine hanging from the surface down
    /// to the gap, a clump of algae rising from the bottom up to it.
    fn weed(&mut self, weed: &Weed, height: f32, distance: f32, stalk: [u8; 3], frond: [u8; 3]) {
        let centre = weed.x + WEED_W / 2.0;
        let phase = weed.seed as f32 * 0.37 + distance * 0.04;
        self.kelp(
            centre,
            0.0,
            weed.gap_top,
            1.0,
            26.0,
            phase,
            stalk,
            frond,
            false,
            255,
            weed.seed as f32,
        );
        self.kelp(
            centre,
            height,
            weed.gap_bottom,
            -1.0,
            21.0,
            phase + 1.0,
            stalk,
            frond,
            false,
            255,
            weed.seed as f32,
        );
        let shoulder = ((height - weed.gap_bottom) * 0.65).max(0.0);
        self.kelp(
            centre - 16.0,
            height,
            height - shoulder,
            -1.0,
            17.0,
            phase + 2.3,
            stalk,
            frond,
            false,
            255,
            weed.seed as f32,
        );
        self.kelp(
            centre + 15.0,
            height,
            height - shoulder * 0.8,
            -1.0,
            19.0,
            phase + 3.9,
            stalk,
            frond,
            false,
            255,
            weed.seed as f32,
        );
    }

    /// A one-logical-pixel column from `top` running `height` down, shaded
    /// from `crest` at the top to `base` at the bottom: a ridge with light on it.
    fn shaded_column(&mut self, x: f32, top: f32, height: f32, crest: [u8; 3], base: [u8; 3]) {
        let rows = height.round().max(1.0) as i32;
        for row in 0..rows {
            let t = ((row * 16) / rows.max(1)).min(16) as u32;
            self.block(x, top + row as f32, 1.0, 1.0, Self::mix(crest, base, t));
        }
    }

    /// What lies behind the game, far to near: faint kelp on a far ridge,
    /// a far line of hills, darker kelp standing between the ridges, the near
    /// hills the sand runs into. Each layer scrolls at its own pace, the far
    /// ones slower, so the water has depth.
    fn backdrop(&mut self, game: &Game, water: [u8; 3], sand: [u8; 3], ink: [u8; 3]) {
        let (w, _) = game.size();
        let floor = game.floor();
        let distance = game.distance();
        let hash = |n: i32| -> f32 {
            let mut v = (n as u32).wrapping_mul(0x9E37_79B1);
            v ^= v >> 15;
            v = v.wrapping_mul(0x85EB_CA77);
            ((v >> 8) & 0xFFFF) as f32 / 65535.0
        };
        // A row of kelp at `offset` scroll, `spacing` apart, standing on `base`.
        let kelp_row = |this: &mut Self,
                        offset: f32,
                        spacing: f32,
                        base: f32,
                        tall: (f32, f32),
                        stalk: [u8; 3],
                        frond: [u8; 3],
                        fade: bool,
                        alpha: u8,
                        salt: i32| {
            let first = ((offset - WEED_W) / spacing).floor() as i32;
            let last = ((offset + w + WEED_W) / spacing).ceil() as i32;
            for n in first..=last {
                let x = n as f32 * spacing + hash(n * 3 + salt) * spacing * 0.6 - offset;
                let height = tall.0 + hash(n * 5 + salt) * (tall.1 - tall.0);
                let wave = 20.0 + hash(n * 7 + salt) * 8.0;
                this.kelp(
                    x,
                    base,
                    base - height,
                    -1.0,
                    wave,
                    hash(n + salt) * 6.3 + distance * 0.02,
                    stalk,
                    frond,
                    fade,
                    alpha,
                    hash(n * 11 + salt) * 100.0,
                );
            }
        };
        // Far ridge and its kelp: barely darker than the water.
        let far_hill = Self::mix(water, sand, 5);
        let far_offset = distance * 0.3;
        let far_crest = Self::mix(far_hill, water, 8);
        for col in 0..(w as i32) {
            let wx = col as f32 + far_offset;
            let rise = 34.0 + 18.0 * (wx / 140.0).sin() + 9.0 * (wx / 53.0).sin() + 3.0 * (wx / 17.0).cos();
            self.shaded_column(col as f32, floor - rise, rise + 1.0, far_crest, far_hill);
        }
        let far_kelp = Self::mix(water, sand, 9);
        kelp_row(
            self,
            distance * 0.3,
            170.0,
            floor - 20.0,
            (50.0, 120.0),
            far_kelp,
            far_kelp,
            true,
            90,
            11,
        );
        // Mid kelp: darker, taller, between the ridges.
        let mid_kelp = Self::mix(water, ink, 4);
        let mid_frond = Self::mix(water, ink, 3);
        kelp_row(
            self,
            distance * 0.55,
            300.0,
            floor + 4.0,
            (110.0, 220.0),
            mid_kelp,
            mid_frond,
            false,
            150,
            23,
        );
        // Near hills: the ridge the sand runs into.
        let hill = Self::mix(water, sand, 10);
        let hill_offset = distance * 0.6;
        let crest = Self::mix(hill, water, 7);
        let base = Self::mix(hill, sand, 8);
        for col in 0..(w as i32) {
            let wx = col as f32 + hill_offset;
            let rise = 12.0 + 10.0 * (wx / 95.0).sin() + 6.0 * (wx / 41.0).sin() + 3.0 * (wx / 13.0).sin();
            self.shaded_column(col as f32, floor - rise, rise + 1.0, crest, base);
        }
    }

    /// What stands in front of the sand: dark kelp silhouettes rising past the
    /// bottom edge, and the rock rim along it, scrolling faster than the game.
    fn foreground(&mut self, game: &Game, sand: [u8; 3], ink: [u8; 3]) {
        let (w, h) = game.size();
        let distance = game.distance();
        let hash = |n: i32| -> f32 {
            let mut v = (n as u32).wrapping_mul(0x9E37_79B1);
            v ^= v >> 15;
            v = v.wrapping_mul(0x85EB_CA77);
            ((v >> 8) & 0xFFFF) as f32 / 65535.0
        };
        let dark = Self::mix(sand, ink, 9);
        let darker = Self::mix(sand, ink, 11);
        let offset = distance * 1.15;
        let spacing = 420.0;
        let first = ((offset - 80.0) / spacing).floor() as i32;
        let last = ((offset + w + 80.0) / spacing).ceil() as i32;
        for n in first..=last {
            let x = n as f32 * spacing + hash(n * 3 + 41) * 200.0 - offset;
            let tall = 70.0 + hash(n * 5 + 41) * 90.0;
            self.kelp(
                x,
                h + 4.0,
                h - tall,
                -1.0,
                24.0,
                hash(n + 41) * 6.3 + distance * 0.03,
                dark,
                darker,
                false,
                230,
                hash(n * 11 + 41) * 100.0,
            );
        }
        // The rock rim along the very bottom, bumpy.
        let rock = Self::mix(sand, ink, 7);
        let rock_offset = distance * 1.25;
        for col in 0..(w as i32) {
            let wx = col as f32 + rock_offset;
            let rise = 8.0 + 6.0 * (wx / 29.0).sin().abs() + 4.0 * (wx / 11.0).cos().abs();
            self.block(col as f32, h - rise, 1.0, rise + 1.0, rock);
        }
    }

    /// Paint one frame of `game`.
    pub fn paint(&mut self, game: &Game, sprites: &Sprites) {
        let (w, h) = game.size();
        let Palette {
            water, sand, ink, message, ..
        } = self.palette;
        self.water(water, (game.floor() * self.scale).round() as i32, game.distance());
        self.backdrop(game, water, sand, ink);
        self.sand(game, sand, water, ink);
        self.foreground(game, sand, ink);

        for star in &game.starfish {
            self.draw_sprite(
                &sprites.starfish,
                star.x,
                game.starfish_y(),
                STARFISH_SIZE.0,
                STARFISH_SIZE.1,
                false,
            );
        }
        for jelly in &game.jellies {
            let frames = &sprites.jellyfish;
            let sprite = &frames[jelly.frame() as usize % frames.len()];
            let (sw, sh) = (sprite.width as f32, sprite.height as f32);
            self.draw_sprite(sprite, jelly.x - sw / 2.0, jelly.y + (JELLY_SIZE.1 - sh), sw, sh, false);
        }
        for fish in &game.fish {
            self.fish(fish, sprites);
        }
        for walker in &game.walkers {
            let (w, h) = walker.kind.size();
            let y = game.walker_y(walker);
            match walker.kind {
                WalkerKind::Axolotl => self.draw_sprite(&sprites.axolotl, walker.x, y, w, h, false),
                WalkerKind::Crab => {
                    let sprite = &sprites.crab[walker.frame() as usize % sprites.crab.len()];
                    let (sw, sh) = (sprite.width as f32, sprite.height as f32);
                    self.draw_sprite(sprite, walker.x, y + (h - sh), sw, sh, false);
                }
            }
        }
        for bubble in &game.bubbles {
            let sprite = &sprites.bubbles[(bubble.size as usize).min(2)];
            self.draw_sprite(sprite, bubble.x, bubble.y, sprite.width as f32, sprite.height as f32, false);
        }
        let leaf = Self::mix(ink, water, 4);
        for weed in &game.weeds {
            self.weed(weed, h, game.distance(), ink, leaf);
        }
        if !sprites.sub.is_empty() {
            let frame = &sprites.sub[game.sub_frame() as usize % sprites.sub.len()];
            let (fw, fh) = (frame.width as f32, frame.height as f32);
            // Centred on the hull's nominal box, so frames of slightly different
            // extents do not jitter.
            let x = game.sub_x + (SUB_W - fw) / 2.0;
            let y = game.sub_drawn_y() + (SUB_H - fh) / 2.0;
            self.draw_sprite(frame, x, y, fw.max(1.0), fh.max(1.0), false);
        }

        if SHOW_HITBOXES {
            for weed in &game.weeds {
                for rect in game.weed_boxes(weed) {
                    self.outline(rect, HITBOX);
                }
            }
            if game.sub_visible() {
                self.outline(game.hull(), HITBOX);
            }
        }

        let score = format!("SCORE {:06}   BEST {:06}", game.score(), game.best());
        self.text(&score, 16.0, 12.0, 2.0, ink);
        match game.phase() {
            Phase::Ready => self.text_centered("PRESS SPACE TO DIVE", w / 2.0, h / 2.0 + 60.0, 3.0, message),
            Phase::Sunk => {
                let what = match game.wreck() {
                    Some(Wreck::Kelp) => "CAUGHT IN THE KELP",
                    Some(Wreck::Surface) => "BROKE THE SURFACE",
                    Some(Wreck::Bottom) | None => "RAN AGROUND",
                };
                self.text_centered(what, w / 2.0, h / 2.0 - 20.0, 4.0, message);
                self.text_centered("PRESS SPACE TO TRY AGAIN", w / 2.0, h / 2.0 + 30.0, 3.0, message);
            }
            Phase::Playing => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sprites_decode() {
        let sprites = Sprites::load();
        assert_eq!(sprites.sub.len(), 4);
        for frame in &sprites.sub {
            assert_eq!((frame.width, frame.height), (95, 74));
        }
        assert_eq!((sprites.axolotl.width, sprites.axolotl.height), (42, 24));
        assert_eq!(sprites.bubbles[2].width, 24);
        for species in Species::ALL {
            let frames = sprites.of(species);
            assert_eq!(frames.len() as u32, species.frames(), "{species:?}");
            let (w, h) = species.size();
            for frame in frames {
                assert!(frame.width > 0 && frame.height > 0, "{species:?}");
                assert!(
                    (frame.width as f32 - w).abs() <= 16.0 && (frame.height as f32 - h).abs() <= 30.0,
                    "{species:?} {}x{}",
                    frame.width,
                    frame.height
                );
            }
        }
        assert_eq!(sprites.crab.len(), 4);
        assert_eq!(sprites.jellyfish.len(), 4);
        assert!(sprites.starfish.width > 0);
    }

    #[test]
    fn a_frame_paints_the_whole_buffer_and_the_message() {
        let sprites = Sprites::load();
        let game = Game::new(400.0, 300.0, 1, 0);
        let mut canvas = Canvas::new(800, 600, 2.0, PixelOrder::Rgba);
        canvas.paint(&game, &sprites);
        let px = canvas.pixels();
        assert_eq!(px.len(), 800 * 600 * 4);
        let pixels = px.as_chunks::<4>().0;
        assert!(pixels.iter().all(|p| p[3] == 255), "every pixel is opaque");
        // The message is drawn in its own grey; some pixel carries it.
        assert!(pixels
            .iter()
            .any(|p| p[0] == Palette::LIGHT.message[0] && p[1] == Palette::LIGHT.message[1]));
        // Sand below, water above.
        let at = |x: u32, y: u32| {
            let i = ((y * 800 + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2]]
        };
        // The floor is sand with grains in it: close to the sand colour, not
        // water. Sampled between the hills above and the rock rim below.
        let floor_px = at(790, 470);
        assert!((floor_px[0] as i32 - Palette::LIGHT.sand[0] as i32).abs() <= 40, "{floor_px:?}");
        // The water lightens towards the surface.
        assert!(at(790, 10)[0] >= Palette::LIGHT.water[0]);
    }

    #[test]
    fn a_bgra_canvas_swaps_the_channels_and_nothing_else() {
        let sprites = Sprites::load();
        let game = Game::new(400.0, 300.0, 1, 0);
        let mut rgba = Canvas::new(400, 300, 1.0, PixelOrder::Rgba);
        let mut bgra = Canvas::new(400, 300, 1.0, PixelOrder::Bgra);
        rgba.paint(&game, &sprites);
        bgra.paint(&game, &sprites);
        for (a, b) in rgba.pixels().as_chunks::<4>().0.iter().zip(bgra.pixels().as_chunks::<4>().0) {
            assert_eq!([a[0], a[1], a[2], a[3]], [b[2], b[1], b[0], b[3]]);
        }
    }

    #[test]
    fn the_dark_palette_paints_dark_water_and_light_words() {
        let sprites = Sprites::load();
        let game = Game::new(400.0, 300.0, 1, 0);
        let mut canvas = Canvas::new(400, 300, 1.0, PixelOrder::Rgba);
        canvas.set_palette(Palette::DARK);
        canvas.paint(&game, &sprites);
        let px = canvas.pixels();
        let at = |x: u32, y: u32| {
            let i = ((y * 400 + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2]]
        };
        assert!(at(390, 10)[0] >= Palette::DARK.water[0] && at(390, 10)[0] < Palette::DARK.ink[0]);
        let floor_px = at(390, 240);
        assert!((floor_px[0] as i32 - Palette::DARK.sand[0] as i32).abs() <= 40, "{floor_px:?}");
        assert!(px.as_chunks::<4>().0.iter().any(|p| [p[0], p[1], p[2]] == Palette::DARK.message));
    }

    #[test]
    fn a_stand_of_weed_leaves_its_gap_open() {
        let sprites = Sprites::load();
        let mut game = Game::new(400.0, 300.0, 1, 0);
        game.weeds.push(Weed {
            x: 200.0,
            gap_top: 100.0,
            gap_bottom: 200.0,
            seed: 3,
        });
        let mut canvas = Canvas::new(400, 300, 1.0, PixelOrder::Rgba);
        canvas.paint(&game, &sprites);
        let px = canvas.pixels();
        let at = |x: u32, y: u32| {
            let i = ((y * 400 + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2]]
        };
        // Something is drawn above and below the gap in the weed's column...
        let column = |y: u32| (200..264).any(|x| at(x, y) != Palette::LIGHT.water);
        assert!(column(30), "no vine above the gap");
        assert!(
            column(260) || (200..264).any(|x| at(x, 260) != at(0, 260)),
            "no algae below the gap"
        );
        // ...and the middle of the gap is open water.
        assert!(!(200..264).any(|x| at(x, 150) == Palette::LIGHT.ink), "the gap is blocked");
    }

    /// How long a big frame takes: a 2560 by 1500 view at scale 2, with weed
    /// in it, painted on the shell's main thread sixty times a second.
    #[test]
    #[ignore = "a timing, run by hand with --release and --nocapture"]
    fn paints_a_big_frame_fast_enough() {
        let sprites = Sprites::load();
        let mut game = Game::new(1280.0, 750.0, 1, 0);
        game.flap();
        for _ in 0..600 {
            game.tick();
            if game.phase() == Phase::Sunk {
                game.flap();
                game.flap();
            }
        }
        for i in 0..4 {
            game.weeds.push(Weed {
                x: 300.0 + i as f32 * 250.0,
                gap_top: 250.0,
                gap_bottom: 470.0,
                seed: i,
            });
        }
        let mut canvas = Canvas::new(2560, 1500, 2.0, PixelOrder::Bgra);
        let started = std::time::Instant::now();
        for _ in 0..30 {
            canvas.paint(&game, &sprites);
        }
        let per_frame = started.elapsed() / 30;
        println!("one 2560x1500 frame: {per_frame:?}");
        // Where it goes, stage by stage.
        let Palette { water, sand, ink, .. } = canvas.palette();
        let mut time = |name: &str, f: &mut dyn FnMut(&mut Canvas)| {
            let t = std::time::Instant::now();
            for _ in 0..30 {
                f(&mut canvas);
            }
            println!("  {name}: {:?}", t.elapsed() / 30);
        };
        time("water", &mut |c| c.water(water, (game.floor() * 2.0) as i32, game.distance()));
        time("backdrop", &mut |c| c.backdrop(&game, water, sand, ink));
        time("sand", &mut |c| c.sand(&game, sand, water, ink));
        time("foreground", &mut |c| c.foreground(&game, sand, ink));
        let leaf = Canvas::mix(ink, water, 4);
        time("weeds", &mut |c| {
            for weed in &game.weeds {
                c.weed(weed, game.size().1, game.distance(), ink, leaf);
            }
        });
        assert!(per_frame < std::time::Duration::from_millis(50), "{per_frame:?}");
    }

    #[test]
    fn drawing_off_the_edge_is_clipped_not_a_panic() {
        let sprites = Sprites::load();
        let mut game = Game::new(200.0, 150.0, 3, 0);
        game.sub_x = -30.0;
        game.sub_y = 140.0;
        game.weeds.push(Weed {
            x: 190.0,
            gap_top: 10.0,
            gap_bottom: 140.0,
            seed: 5,
        });
        game.weeds.push(Weed {
            x: -40.0,
            gap_top: 60.0,
            gap_bottom: 100.0,
            seed: 9,
        });
        let mut canvas = Canvas::new(200, 150, 1.0, PixelOrder::Rgba);
        canvas.paint(&game, &sprites);
    }
}
