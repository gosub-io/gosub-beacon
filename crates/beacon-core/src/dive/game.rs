//! The rules of Dive: a submarine that sinks unless you flap it upward, weed
//! scrolling in from the right with a gap to thread, bubbles, fish and the
//! odd axolotl for scenery. Nothing here knows how it is drawn or what pressed
//! the key: a shell ticks it sixty times a second and tells it when to flap.
//!
//! Coordinates are logical pixels of the view, origin top-left, y downward.
//! Sizes the rules depend on (the submarine's hull, a stand of weed's width)
//! are constants here so a collision is decided by the same numbers the
//! painter draws with.

/// The submarine's hull, in logical pixels: the sheet's frames (382 by 298)
/// drawn at a quarter, which keeps its pixels whole.
pub const SUB_W: f32 = 95.0;
pub const SUB_H: f32 = 74.0;
/// How wide a stand of weed is, leaves included; the stalk the hull must
/// clear is narrower (see [`WEED_INSET`]).
pub const WEED_W: f32 = 64.0;
/// How far in from a weed's edges the fronds give way: brushing a frond's
/// tip is not a hit, the stalk and the thick of the fronds are.
pub const WEED_INSET: f32 = 4.0;

/// Downward pull per frame, in px per frame squared.
const GRAVITY: f32 = 0.2;
/// What a flap sets the vertical speed to (upward is negative).
const FLAP: f32 = -5.0;
/// How fast the world scrolls past, in px per frame.
const SCROLL: f32 = 3.0;
/// How fast a bubble rises.
const BUBBLE_RISE: f32 = 3.0;
/// How many bubbles are about at any time.
const BUBBLES: usize = 10;
/// How far inside the hull's box a hit is counted: the periscope and the
/// propeller stick out of the sprite's box, the hull sits well inside it.
const HULL_INSET: f32 = 10.0;
/// Frames between stands of weed, as a range.
const WEED_EVERY: (u32, u32) = (75, 200);
/// The open water between a vine from above and the algae below, as a range
/// of heights.
const GAP: (u32, u32) = (210, 290);
/// Frames between walkers on the sand (axolotls, crabs), as a range.
const WALKER_EVERY: (u32, u32) = (50, 300);
/// Frames between fish, as a range, and how many swim at most.
const FISH_EVERY: (u32, u32) = (300, 700);
const FISH_MAX: usize = 2;
/// Frames between jellyfish, and how many drift at most.
const JELLY_EVERY: (u32, u32) = (300, 900);
const JELLY_MAX: usize = 2;
/// Frames between starfish on the sand.
const STARFISH_EVERY: (u32, u32) = (200, 700);
/// A jellyfish's drawn size and a starfish's, in logical pixels.
pub const JELLY_SIZE: (f32, f32) = (36.0, 52.0);
pub const STARFISH_SIZE: (f32, f32) = (44.0, 42.0);

/// Where the game is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Bobbing at the start; a flap begins the game.
    Ready,
    Playing,
    /// Hit something; a flap starts over.
    Sunk,
}

/// What ended a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wreck {
    /// Caught in a stand of weed.
    Kelp,
    /// Hit the sea floor.
    Bottom,
    /// Broke the surface.
    Surface,
}

/// One obstacle: a vine hanging from the surface down to `gap_top` and
/// algae growing from the bottom up to `gap_bottom`, with open water between.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weed {
    /// Left edge.
    pub x: f32,
    /// The gap: open water between `gap_top` and `gap_bottom`.
    pub gap_top: f32,
    pub gap_bottom: f32,
    /// Which of the stand's looks, so no two neighbours sway alike.
    pub seed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bubble {
    pub x: f32,
    pub y: f32,
    /// Which of the three sizes, 0 small to 2 large.
    pub size: u8,
}

/// What walks along the sand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkerKind {
    Axolotl,
    Crab,
}

impl WalkerKind {
    /// Drawn size in logical pixels.
    pub fn size(self) -> (f32, f32) {
        match self {
            WalkerKind::Axolotl => (42.0, 24.0),
            WalkerKind::Crab => (50.0, 28.0),
        }
    }

    /// How many frames its walk has (one means a still sprite that bobs).
    pub fn frames(self) -> u32 {
        match self {
            WalkerKind::Axolotl => 1,
            WalkerKind::Crab => 4,
        }
    }
}

/// Something walking along the sand, from right to left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walker {
    pub kind: WalkerKind,
    pub x: f32,
    /// Its walk, in radians: the body bobs a pixel or two with each step.
    pub step: f32,
    pub age: u32,
}

impl Walker {
    /// Which frame of its walk to draw.
    pub fn frame(&self) -> u32 {
        (self.age / 8) % self.kind.frames()
    }
}

/// The fish of the sea, as the sprite sheet has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Species {
    Minnow,
    Damselfish,
    Herring,
    Angelfish,
    Butterflyfish,
    Grouper,
    Pufferfish,
    Tuna,
    Turtle,
    Ray,
    Eel,
}

impl Species {
    pub const ALL: [Species; 11] = [
        Species::Minnow,
        Species::Damselfish,
        Species::Herring,
        Species::Angelfish,
        Species::Butterflyfish,
        Species::Grouper,
        Species::Pufferfish,
        Species::Tuna,
        Species::Turtle,
        Species::Ray,
        Species::Eel,
    ];

    /// Drawn size in logical pixels: half the sheet's.
    pub fn size(self) -> (f32, f32) {
        match self {
            Species::Minnow => (23.0, 15.0),
            Species::Damselfish => (36.0, 31.0),
            Species::Herring => (58.0, 26.0),
            Species::Angelfish => (33.0, 40.0),
            Species::Butterflyfish => (40.0, 33.0),
            Species::Grouper => (53.0, 37.0),
            Species::Pufferfish => (36.0, 29.0),
            Species::Tuna => (58.0, 36.0),
            Species::Turtle => (53.0, 38.0),
            Species::Ray => (60.0, 39.0),
            Species::Eel => (51.0, 30.0),
        }
    }

    /// How many frames its swim has.
    pub fn frames(self) -> u32 {
        match self {
            Species::Pufferfish | Species::Tuna | Species::Ray => 3,
            Species::Eel => 6,
            _ => 4,
        }
    }

    /// How fast it swims, in tenths of a px per frame, as a range.
    fn pace(self) -> (u32, u32) {
        match self {
            Species::Minnow => (10, 18),
            Species::Damselfish => (6, 12),
            Species::Herring => (12, 20),
            Species::Angelfish => (5, 9),
            Species::Butterflyfish => (6, 10),
            Species::Grouper => (4, 8),
            Species::Pufferfish => (3, 6),
            Species::Tuna => (16, 26),
            Species::Turtle => (3, 5),
            Species::Ray => (5, 9),
            Species::Eel => (4, 8),
        }
    }

    /// Whether it keeps to the water just above the sand.
    fn bottom_dweller(self) -> bool {
        matches!(self, Species::Ray | Species::Eel | Species::Grouper)
    }

    /// How often it turns up, relative to the others.
    fn weight(self) -> u32 {
        match self {
            Species::Minnow => 5,
            Species::Damselfish | Species::Herring => 4,
            Species::Butterflyfish => 3,
            Species::Angelfish | Species::Grouper | Species::Pufferfish => 2,
            Species::Tuna | Species::Turtle | Species::Ray | Species::Eel => 1,
        }
    }

    fn pick(rng: &mut Rng) -> Species {
        let total: u32 = Species::ALL.iter().map(|s| s.weight()).sum();
        let mut roll = rng.range(0, total);
        for species in Species::ALL {
            if roll < species.weight() {
                return species;
            }
            roll -= species.weight();
        }
        Species::Minnow
    }
}

/// A fish swimming through the water on its own, at its own pace, or a
/// school of minnows keeping together.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fish {
    pub species: Species,
    /// The centre of the fish (of the school's lead fish).
    pub x: f32,
    pub y: f32,
    /// Px per frame; negative swims left.
    pub speed: f32,
    /// A school of five minnows rather than one fish.
    pub school: bool,
    /// Frames lived: the swim's frames follow it.
    pub age: u32,
}

impl Fish {
    /// Whether the fish faces right (swims right).
    pub fn faces_right(&self) -> bool {
        self.speed > 0.0
    }

    /// Which frame of its swim to draw; a school's members are out of step.
    pub fn frame(&self, member: u32) -> u32 {
        ((self.age / 8) + member) % self.species.frames()
    }

    /// Where the school's members swim relative to its lead, for a school
    /// heading left: the others trail to the right of it.
    pub const SCHOOL: [(f32, f32); 5] = [(0.0, 0.0), (18.0, -12.0), (20.0, 14.0), (38.0, 2.0), (40.0, -20.0)];
}

/// A jellyfish drifting up through the water, slowly, swaying.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jelly {
    pub x: f32,
    pub y: f32,
    pub age: u32,
}

impl Jelly {
    pub fn frame(&self) -> u32 {
        (self.age / 10) % 4
    }
}

/// A starfish lying on the sand, going by with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Starfish {
    pub x: f32,
}

/// A small deterministic generator (xorshift), so a game can be replayed in
/// a test from its seed and the crate needs no random-number dependency.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A number in `lo..hi`.
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % u64::from(hi - lo)) as u32
    }
}

#[derive(Debug, Clone)]
pub struct Game {
    width: f32,
    height: f32,
    phase: Phase,
    /// Frames played this round; the score is derived from it.
    frames: u64,
    /// Frames since the game began, whatever the phase: what animates.
    ticks: u64,
    /// What ended the last round, while `Sunk`.
    wreck: Option<Wreck>,
    best: u64,
    pub sub_x: f32,
    pub sub_y: f32,
    sub_vy: f32,
    /// Bobbing phase while ready, in radians.
    bob: f32,
    pub weeds: Vec<Weed>,
    next_weed_in: u32,
    pub bubbles: Vec<Bubble>,
    pub walkers: Vec<Walker>,
    next_walker_in: u32,
    pub fish: Vec<Fish>,
    next_fish_in: u32,
    pub jellies: Vec<Jelly>,
    next_jelly_in: u32,
    pub starfish: Vec<Starfish>,
    next_starfish_in: u32,
    /// How far the world has scrolled, in px: what the sand is drawn against.
    distance: f32,
    rng: Rng,
}

impl Game {
    /// A game in a view of `width` by `height` logical pixels, with `best`
    /// as the high score to beat.
    pub fn new(width: f32, height: f32, seed: u64, best: u64) -> Self {
        let mut game = Self {
            width,
            height,
            phase: Phase::Ready,
            frames: 0,
            ticks: 0,
            wreck: None,
            best,
            sub_x: 100.0,
            sub_y: 0.0,
            sub_vy: 0.0,
            bob: 0.0,
            weeds: Vec::new(),
            next_weed_in: 0,
            bubbles: Vec::new(),
            walkers: Vec::new(),
            next_walker_in: 0,
            fish: Vec::new(),
            next_fish_in: 30,
            jellies: Vec::new(),
            next_jelly_in: 200,
            starfish: Vec::new(),
            next_starfish_in: 60,
            distance: 0.0,
            rng: Rng::new(seed),
        };
        game.reset_round();
        for _ in 0..BUBBLES {
            let bubble = game.new_bubble(true);
            game.bubbles.push(bubble);
        }
        game
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The score of the round in play (or the one just lost): a tenth of the
    /// frames survived, so roughly six a second.
    pub fn score(&self) -> u64 {
        self.frames / 10
    }

    pub fn best(&self) -> u64 {
        self.best
    }

    /// What ended the round, while sunk.
    pub fn wreck(&self) -> Option<Wreck> {
        self.wreck
    }

    pub fn size(&self) -> (f32, f32) {
        (self.width, self.height)
    }

    /// Where the sand begins: the lower third is sea floor.
    pub fn floor(&self) -> f32 {
        self.height - self.height / 3.0
    }

    /// How far the world has scrolled past, in px.
    pub fn distance(&self) -> f32 {
        self.distance
    }

    /// The view changed size. The submarine keeps its place in the water;
    /// the weed keeps its place. Nothing is rescaled: it is a game, not a layout.
    pub fn resize(&mut self, width: f32, height: f32) {
        self.width = width;
        self.height = height;
        self.sub_y = self.sub_y.min(height - SUB_H).max(0.0);
    }

    fn reset_round(&mut self) {
        self.frames = 0;
        self.wreck = None;
        self.sub_y = (self.height / 2.0 - SUB_H / 2.0).max(0.0);
        self.sub_vy = 0.0;
        self.bob = 0.0;
        self.weeds.clear();
        self.walkers.clear();
        self.jellies.clear();
        self.starfish.clear();
        self.next_weed_in = 60;
        self.next_walker_in = self.rng.range(WALKER_EVERY.0, WALKER_EVERY.1);
    }

    /// The one input: the submarine flaps upward. Starts a round from
    /// `Ready`, starts over from `Sunk`.
    pub fn flap(&mut self) {
        match self.phase {
            Phase::Ready => {
                self.phase = Phase::Playing;
                self.sub_vy = FLAP;
            }
            Phase::Playing => self.sub_vy = FLAP,
            Phase::Sunk => {
                self.reset_round();
                self.phase = Phase::Ready;
            }
        }
    }

    /// One frame, one sixtieth of a second.
    pub fn tick(&mut self) {
        self.ticks += 1;
        self.tick_scenery();
        match self.phase {
            Phase::Ready => {
                self.bob = (self.bob + 0.04) % std::f32::consts::TAU;
            }
            Phase::Playing => {
                self.frames += 1;
                self.sub_vy += GRAVITY;
                self.sub_y += self.sub_vy;
                self.tick_weeds();
                if let Some(wreck) = self.collided() {
                    self.phase = Phase::Sunk;
                    self.wreck = Some(wreck);
                    self.best = self.best.max(self.score());
                }
            }
            Phase::Sunk => {
                // The wreck leaves the screen: one that broke the surface
                // bobs up and away, any other sinks out through the bottom.
                // Once out of sight it stays there until the next round.
                match self.wreck {
                    Some(Wreck::Surface) => {
                        if self.sub_y > -SUB_H * 2.0 {
                            self.sub_vy = (self.sub_vy - GRAVITY).max(-4.0);
                            self.sub_y += self.sub_vy;
                        }
                    }
                    _ => {
                        if self.sub_y < self.height + SUB_H {
                            self.sub_vy = (self.sub_vy + GRAVITY).min(6.0);
                            self.sub_y += self.sub_vy;
                        }
                    }
                }
            }
        }
    }

    /// Whether the submarine is anywhere on screen.
    pub fn sub_visible(&self) -> bool {
        self.sub_y < self.height && self.sub_y + SUB_H > 0.0
    }

    /// Which of the submarine's four propeller frames to draw: the screw
    /// turns while the sub is going, and stops once it has sunk.
    pub fn sub_frame(&self) -> u32 {
        match self.phase {
            Phase::Sunk => 0,
            _ => ((self.ticks / 5) % 4) as u32,
        }
    }

    /// Where the submarine is drawn: its position plus the bob while ready.
    pub fn sub_drawn_y(&self) -> f32 {
        match self.phase {
            Phase::Ready => self.sub_y + self.bob.sin() * 10.0,
            _ => self.sub_y,
        }
    }

    fn tick_scenery(&mut self) {
        let scroll = if self.phase == Phase::Playing { SCROLL } else { SCROLL / 3.0 };
        self.distance += scroll;
        for bubble in &mut self.bubbles {
            bubble.y -= BUBBLE_RISE;
            bubble.x -= scroll / 3.0;
        }
        let (w, h) = (self.width, self.height);
        for i in 0..self.bubbles.len() {
            if self.bubbles[i].y < -24.0 || self.bubbles[i].x < -24.0 {
                self.bubbles[i] = self.new_bubble(false);
            }
        }
        let _ = (w, h);
        // Walkers cross the sand, a little slower than the world scrolls.
        for walker in &mut self.walkers {
            walker.x -= scroll * 0.8;
            walker.step = (walker.step + 0.15) % std::f32::consts::TAU;
            walker.age += 1;
        }
        self.walkers.retain(|w| w.x > -60.0);
        if self.next_walker_in == 0 {
            let kind = if self.rng.range(0, 2) == 0 {
                WalkerKind::Axolotl
            } else {
                WalkerKind::Crab
            };
            self.walkers.push(Walker {
                kind,
                x: self.width,
                step: 0.0,
                age: 0,
            });
            self.next_walker_in = self.rng.range(WALKER_EVERY.0, WALKER_EVERY.1);
        } else {
            self.next_walker_in -= 1;
        }
        // Fish swim either way through the open water, each at its own pace.
        for fish in &mut self.fish {
            fish.x += fish.speed - scroll * 0.5;
            fish.age += 1;
        }
        let width = self.width;
        self.fish.retain(|f| {
            let reach = f.species.size().0 + if f.school { 50.0 } else { 0.0 };
            f.x > -reach - 10.0 && f.x < width + reach + 10.0
        });
        if self.next_fish_in == 0 {
            if self.fish.len() < FISH_MAX {
                let species = Species::pick(&mut self.rng);
                let school = species == Species::Minnow && self.rng.range(0, 2) == 0;
                let leftward = self.rng.range(0, 3) != 0;
                let (lo, hi) = species.pace();
                // Its own pace; a fish heading right must also beat the
                // current it is drawn against, or it would slide backwards.
                let speed = self.rng.range(lo, hi) as f32 / 10.0 + if leftward { 0.0 } else { SCROLL * 0.5 + 0.4 };
                let (w, h) = species.size();
                let top = if species.bottom_dweller() {
                    (self.floor() * 0.6).max(20.0)
                } else {
                    20.0
                };
                let water = (self.floor() - h - top - 10.0).max(1.0);
                let y = top + self.rng.range(0, water as u32) as f32;
                let reach = w + if school { 50.0 } else { 0.0 };
                self.fish.push(Fish {
                    species,
                    x: if leftward { self.width + reach } else { -reach },
                    y,
                    speed: if leftward { -speed } else { speed },
                    school,
                    age: self.rng.range(0, 64),
                });
            }
            self.next_fish_in = self.rng.range(FISH_EVERY.0, FISH_EVERY.1);
        } else {
            self.next_fish_in -= 1;
        }
        // Jellyfish drift up, swaying, and leave at the surface.
        for jelly in &mut self.jellies {
            jelly.y -= 0.35;
            jelly.x -= scroll * 0.4;
            jelly.x += (jelly.age as f32 / 40.0).sin() * 0.3;
            jelly.age += 1;
        }
        self.jellies.retain(|j| j.y > -JELLY_SIZE.1 && j.x > -JELLY_SIZE.0);
        if self.next_jelly_in == 0 {
            if self.jellies.len() < JELLY_MAX {
                let x = self.rng.range(40, (self.width - 40.0).max(41.0) as u32) as f32;
                self.jellies.push(Jelly {
                    x,
                    y: self.floor() + 10.0,
                    age: 0,
                });
            }
            self.next_jelly_in = self.rng.range(JELLY_EVERY.0, JELLY_EVERY.1);
        } else {
            self.next_jelly_in -= 1;
        }
        // Starfish lie on the sand and go by with it.
        for star in &mut self.starfish {
            star.x -= scroll;
        }
        self.starfish.retain(|s| s.x > -STARFISH_SIZE.0);
        if self.next_starfish_in == 0 {
            self.starfish.push(Starfish { x: self.width });
            self.next_starfish_in = self.rng.range(STARFISH_EVERY.0, STARFISH_EVERY.1);
        } else {
            self.next_starfish_in -= 1;
        }
    }

    /// Where a walker is drawn: on the sand, bobbing with its steps.
    pub fn walker_y(&self, walker: &Walker) -> f32 {
        self.floor() - walker.kind.size().1 + 2.0 + walker.step.sin().abs() * -2.0
    }

    /// Where a starfish lies: half sunk into the sand.
    pub fn starfish_y(&self) -> f32 {
        self.floor() - STARFISH_SIZE.1 * 0.55
    }

    fn new_bubble(&mut self, anywhere: bool) -> Bubble {
        let x = self.rng.range(0, (self.width + 300.0) as u32) as f32;
        let y = if anywhere {
            self.rng.range(0, self.height.max(1.0) as u32) as f32
        } else {
            self.height + self.rng.range(10, 100) as f32
        };
        let size = self.rng.range(0, 3) as u8;
        Bubble { x, y, size }
    }

    fn tick_weeds(&mut self) {
        for weed in &mut self.weeds {
            weed.x -= SCROLL;
        }
        self.weeds.retain(|w| w.x + WEED_W > 0.0);
        if self.next_weed_in == 0 {
            let gap = self.rng.range(GAP.0, GAP.1) as f32;
            let room = (self.height - gap - 100.0).max(1.0) as u32;
            let gap_top = 50.0 + self.rng.range(0, room) as f32;
            let seed = self.rng.range(0, 1 << 16);
            self.weeds.push(Weed {
                x: self.width,
                gap_top,
                gap_bottom: gap_top + gap,
                seed,
            });
            self.next_weed_in = self.rng.range(WEED_EVERY.0, WEED_EVERY.1);
        } else {
            self.next_weed_in -= 1;
        }
    }

    /// The hull's hit box: `(left, top, right, bottom)`.
    pub fn hull(&self) -> (f32, f32, f32, f32) {
        (
            self.sub_x + HULL_INSET,
            self.sub_y + HULL_INSET,
            self.sub_x + SUB_W - HULL_INSET,
            self.sub_y + SUB_H - HULL_INSET,
        )
    }

    /// A stand of weed's two hit boxes, the vine above the gap and the algae
    /// below it, each `(left, top, right, bottom)`.
    pub fn weed_boxes(&self, weed: &Weed) -> [(f32, f32, f32, f32); 2] {
        let (left, right) = (weed.x + WEED_INSET, weed.x + WEED_W - WEED_INSET);
        [(left, 0.0, right, weed.gap_top), (left, weed.gap_bottom, right, self.height)]
    }

    /// What the hull hit this frame, if anything: the weed, the surface or
    /// the bottom. Decided on the same boxes [`Self::hull`] and
    /// [`Self::weed_boxes`] give, which is what the debug overlay draws.
    fn collided(&self) -> Option<Wreck> {
        let (left, top, right, bottom) = self.hull();
        if top < 0.0 {
            return Some(Wreck::Surface);
        }
        if bottom > self.height {
            return Some(Wreck::Bottom);
        }
        let tangled = self.weeds.iter().any(|weed| {
            self.weed_boxes(weed)
                .iter()
                .any(|&(l, t, r, b)| right > l && left < r && bottom > t && top < b)
        });
        tangled.then_some(Wreck::Kelp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        Game::new(800.0, 600.0, 7, 0)
    }

    #[test]
    fn a_flap_starts_the_game_and_lifts_the_sub() {
        let mut g = game();
        assert_eq!(g.phase(), Phase::Ready);
        let start = g.sub_y;
        g.flap();
        assert_eq!(g.phase(), Phase::Playing);
        g.tick();
        assert!(g.sub_y < start, "the sub should rise after a flap");
    }

    #[test]
    fn gravity_sinks_the_sub_until_it_hits_the_bottom() {
        let mut g = game();
        g.flap();
        for _ in 0..600 {
            g.tick();
            if g.phase() == Phase::Sunk {
                break;
            }
        }
        assert_eq!(g.phase(), Phase::Sunk, "left alone, the sub reaches the bottom");
        assert_eq!(g.wreck(), Some(Wreck::Bottom));
        assert!(g.score() > 0);
    }

    #[test]
    fn weeds_arrive_scroll_left_and_leave() {
        let mut g = game();
        g.flap();
        // Keep the sub afloat so the weed is what ends things, not the floor.
        let mut seen = false;
        for frame in 0..400 {
            if frame % 25 == 0 {
                g.flap();
            }
            g.tick();
            if let Some(w) = g.weeds.first() {
                seen = true;
                assert!(w.gap_bottom - w.gap_top >= GAP.0 as f32);
                assert!(w.gap_top >= 50.0);
            }
            if g.phase() == Phase::Sunk {
                break;
            }
        }
        assert!(seen, "a stand of weed should have come along within 400 frames");
    }

    #[test]
    fn hitting_the_weed_sinks_the_sub_and_records_the_best() {
        let mut g = game();
        g.flap();
        g.frames = 200;
        g.sub_y = 300.0;
        g.sub_vy = 0.0;
        // Weed right on the sub with its gap elsewhere.
        g.weeds.push(Weed {
            x: g.sub_x,
            gap_top: 0.0,
            gap_bottom: 100.0,
            seed: 1,
        });
        g.next_weed_in = 1000;
        g.tick();
        assert_eq!(g.phase(), Phase::Sunk);
        assert_eq!(g.wreck(), Some(Wreck::Kelp));
        assert_eq!(g.best(), 20);
    }

    #[test]
    fn threading_the_gap_is_not_a_hit() {
        let mut g = game();
        g.flap();
        g.sub_y = 300.0;
        g.sub_vy = 0.0;
        g.weeds.push(Weed {
            x: g.sub_x,
            gap_top: 250.0,
            gap_bottom: 450.0,
            seed: 1,
        });
        g.next_weed_in = 1000;
        g.tick();
        assert_eq!(g.phase(), Phase::Playing);
    }

    #[test]
    fn a_flap_after_sinking_starts_over() {
        let mut g = game();
        g.flap();
        for _ in 0..600 {
            g.tick();
        }
        assert_eq!(g.phase(), Phase::Sunk);
        assert!(!g.sub_visible(), "the wreck should have left the screen");
        g.flap();
        assert_eq!(g.phase(), Phase::Ready);
        assert!(g.sub_visible(), "a new round brings the sub back");
        assert_eq!(g.score(), 0);
        assert!(g.weeds.is_empty());
    }

    #[test]
    fn the_same_seed_replays_the_same_game() {
        let mut a = game();
        let mut b = game();
        for frame in 0..300 {
            if frame % 20 == 0 {
                a.flap();
                b.flap();
            }
            a.tick();
            b.tick();
        }
        assert_eq!(a.weeds, b.weeds);
        assert_eq!(a.sub_y, b.sub_y);
    }

    #[test]
    fn the_scenery_keeps_its_bubbles() {
        let mut g = game();
        for _ in 0..2000 {
            g.tick();
        }
        assert_eq!(g.bubbles.len(), BUBBLES);
        assert!(g.bubbles.iter().all(|b| b.y >= -24.0 && b.x >= -24.0));
    }

    #[test]
    fn walkers_cross_the_sand_and_fish_swim_the_water() {
        let mut g = game();
        let mut saw_walker = false;
        let mut saw_fish = false;
        let mut saw_jelly = false;
        for _ in 0..4000 {
            g.tick();
            for w in &g.walkers {
                saw_walker = true;
                let bottom = g.walker_y(w) + w.kind.size().1;
                assert!((bottom - g.floor()).abs() <= 2.5, "walker off the sand: {bottom}");
            }
            for f in &g.fish {
                saw_fish = true;
                assert!(f.y >= 20.0 && f.y + f.species.size().1 <= g.floor(), "fish in the sand: {f:?}");
            }
            saw_jelly |= !g.jellies.is_empty();
            assert!(g.fish.len() <= FISH_MAX);
            assert!(g.jellies.len() <= JELLY_MAX);
        }
        assert!(saw_walker && saw_fish && saw_jelly);
        assert!(g.distance() > 0.0);
    }

    #[test]
    fn every_species_is_picked_eventually() {
        let mut rng = Rng::new(42);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..2000 {
            seen.insert(Species::pick(&mut rng) as u8);
        }
        assert_eq!(seen.len(), Species::ALL.len());
    }
}
