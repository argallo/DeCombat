//! The case reel (CRATE-5): with a pack's `case_reel` HUD key on, opening a supply case first runs a strip of the case's
//! possible items past a centre mark, slowing down until it stops on the award, as a case opening in today's shooters
//! does; then the award window opens as before. A click, Enter or Escape skips to the stop.
//!
//! Our own design: the 2009 client shows the award window at once (crate-result-window). The pack only switches the reel on
//! and sets its length; the client draws it, so a pack runs no code here.
//!
//! CRATE-7 dresses it by the award's rarity tier with the game's own sounds and a look of the game: the mark's colour
//! climbs the tiers as the strip slows (never past the award's); from early in the spin the match countdown beep runs as
//! a bomb's timer does, slowly quickening and then ever faster until the strip stops (a Legendary then goes dark and
//! silent); then the won gun fires and is racked, with the rank-up sound (Epic) or the match-win sound (Legendary).
//! CRATE-8: on the stop the gun leaps out of its cell as its own turning 3D model ([`Reel::showcase`]), with a muzzle
//! flash, a shake, ejected brass and, from Rare, searchlights. The ROM's sounds are played by number (the interface
//! sounds, the gun's own sounds); the effect textures are made by [`super::reelfx`].
//!
//! CRATE-9: the reel looks like a window of the game: the award window's own art, read from the user's ROM at run time,
//! widened across the screen (its title bar, its perforated band tiled, its footer), with each item in the shop's own
//! slot frame (name and class tag at the top left, as the shop and the award window place them), the frame tinted by
//! the item's tier.
//!
//! Provenance: ORIGINAL.
// DC-DEFAULT: CRATE-5 (proposed; USER REQUEST 2026-11-05 "a mod for crate opening that animates through all the various
// options, kind of like how CS2 has case openings")

use std::sync::RwLock;
use std::time::{Duration, Instant};

use dc_bute::modpatch::Rarity;
use dc_ui_data::pages::Align;

use super::catalog::Catalog;
use super::frontfx;
use super::reelfx;
use super::sheet::{place_text, ALIGN_CENTRE, ALIGN_LEFT};
use crate::hud::items::DrawItem;

/// The reel as the loaded set asks for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReelSettings {
    /// The spin's length.
    pub ms: u32,
}

/// The spin's length without a `case_reel_ms` key.
// DC-DEFAULT: CRATE-7 (proposed; USER REQUEST 2026-11-06 "make the spin go a little longer to help time it all") 7 s,
// 5.5 s before
pub const DEFAULT_MS: u32 = 7_000;
/// How long the won gun is shown after the hit before the award window opens, plus [`HOLD_PER_TIER_MS`] per step of the
/// tier's strength.
pub const HOLD_MS: u64 = 2_600;
pub const HOLD_PER_TIER_MS: u64 = 400;
/// Lights out before a Legendary's hit.
pub const HUSH_MS: u64 = 900;
/// The bomb run: from [`BEEP_FROM`] of the spin on (every tier alike, so it tells nothing), each countdown beep is
/// followed by the next after the smaller of [`BEEP_GAP_SHARE`] of the time left to the stop and the slow ramp
/// [`BEEP_SLOW_MS`] + [`BEEP_SLOW_PER_MS`] of the time left, between [`BEEP_GAP_MIN_MS`] and [`BEEP_GAP_MAX_MS`],
/// until less than [`BEEP_LAST_MS`] is left: about 550 ms apart at first in a 7 s spin, closing steadily to about
/// 190 ms, then the last half second ever faster, as a bomb timer's last seconds, the last as the strip comes to rest.
// DC-DEFAULT: CRATE-7 (proposed; USER REQUEST 2026-11-06 "the beeps slower and then ramp up faster", "the end of the
// beeps should be also when the spinning reaches its stopping speed") 150 ms + 6.4 % up to 600 ms, to the stop
// (225 ms + 1 % up to 340 ms, to the hit, before)
pub const BEEP_FROM: f32 = 0.1;
pub const BEEP_SLOW_MS: f32 = 150.0;
pub const BEEP_SLOW_PER_MS: f32 = 0.064;
pub const BEEP_GAP_MAX_MS: f32 = 600.0;
pub const BEEP_GAP_MIN_MS: f32 = 55.0;
pub const BEEP_GAP_SHARE: f32 = 0.3;
pub const BEEP_LAST_MS: f32 = 40.0;
/// The progress of the spin at which the mark's colour climbs to Rare, Epic and Legendary (only as far as the award).
pub const TELL_AT: [f32; 4] = [0.0, 0.5, 0.7, 0.8];
/// The weapon sounds the reveal plays: the shot and the draw (the rack).
pub const WEAPON_FIRE: u8 = crate::sound::SOUND_FIRE;
pub const WEAPON_SELECT: u8 = 4;
/// Where the won gun is shown (virtual pixels, centred on the screen: the interface camera's axis), and how much of its
/// width the gun's length takes.
pub const SHOWCASE_RECT: [i32; 4] = [162, 214, 700, 340];
pub const SHOWCASE_FILL: f32 = 0.82;
/// The leap out of the cell, the gun's turn (radians a second) and its yaw at the leap (side on).
pub const LEAP_S: f32 = 0.45;
pub const TURN_PER_S: f32 = 1.2;
pub const SIDE_YAW: f32 = std::f32::consts::FRAC_PI_2;

static SETTINGS: RwLock<Option<ReelSettings>> = RwLock::new(None);

/// The loaded set's reel (`None`: no pack asks for one, or no set).
pub fn set(settings: Option<ReelSettings>) {
    *SETTINGS.write().unwrap_or_else(|e| e.into_inner()) = settings;
}

/// The reel the loaded set asks for.
pub fn settings() -> Option<ReelSettings> {
    *SETTINGS.read().unwrap_or_else(|e| e.into_inner())
}

/// One cell of the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub item: i32,
    pub picture: Option<String>,
    pub name: String,
    /// A weapon variant's (a camo's) rarity tier; `None` for an item that is no camo.
    pub rarity: Option<Rarity>,
    /// The shop's class tag of the item (texture, size), as the award window shows it.
    pub tag: Option<(&'static str, [i32; 2])>,
}

/// A sound the reel asks for: one of the ROM's interface sounds, or sound `k` of a weapon row (its own files).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sound {
    Rom(i32),
    Weapon { k: u8, weapon: u16 },
}

/// What the menu draws of the won gun at a moment ([`Reel::showcase`]): the model, its turn, where its render goes
/// (virtual pixels) and how opaque it is; `shown` false while it is only being made ready (the spin).
#[derive(Debug, Clone, PartialEq)]
pub struct ShowcaseFrame {
    pub model: crate::world::ModelRef,
    pub yaw: f32,
    pub pitch: f32,
    pub dst: [f32; 4],
    pub alpha: f32,
    pub rim: [f32; 3],
    pub shown: bool,
}

/// Cells in the strip and the one the spin stops on (the others are drawn from the pool).
// DC-DEFAULT: CRATE-7 (proposed; USER REQUEST 2026-11-06 "it sits on the gun you're going to win for way too long, it
// should be down to the last second") 76 cells with the award in cell 70, for the gentler ease's same top speed (46 and
// 40 before)
pub const CELLS: usize = 76;
pub const WINNER: usize = 70;
/// Where the strip starts: this many cells already left of the mark, so the screen is full from the first frame.
const LEAD: f32 = 3.0;

/// CRATE-9: the reel window is the award window's art drawn this many times its size, across the screen.
// DC-DEFAULT: CRATE-9 (proposed; USER REQUEST 2026-11-06 "give the slider a look and feel that matches the games UI")
pub const SCALE: f32 = 1.6;
/// The texel rows of the award window's opened art the reel window is built of, top to bottom: the title bar with the
/// strip under it, the perforated band the cells run over, and the footer.
// OBSERVED: the user's ROM art drawn by crate-result-window (0x1db x 0x100 at 1:1): title bar rows 0..46, the
// perforated band 46..124 (the slot frame's at row 49), the message area 124..200, the footer 200..256 (the reel takes
// its lower part, without the OK button's room)
pub const ART_ROWS: [[u32; 2]; 3] = [[0, 46], [46, 124], [222, 256]];
/// The art's left and right edges (kept as they are) and, for the band, the span of columns tiled between them: seven
/// of the perforation's 14.4-texel repeats, so the holes run on across the seams (the other rows stretch their middle).
// OBSERVED: the same art (border columns 0..4 and 471..475; the perforation repeats every 14.4 texels)
pub const ART_LEFT: u32 = 12;
pub const ART_RIGHT: [u32; 2] = [463, 475];
pub const ART_TILE: [u32; 2] = [12, 113];
/// The shop's slot frame art (texels) and how wide a cell takes it (texels; the frame is drawn in nine parts).
// EVIDENCE: cshell+0x286500 (the slot frame 0xfe x 0x47; the name at +6, +6, the class tag at +6, +0x13)
pub const SLOT_ART: (u32, u32) = (0xfe, 0x47);
pub const SLOT_WIDE: f32 = 150.0;
pub const NAME_AT: [f32; 2] = [6.0, 6.0];
pub const TAG_AT: [f32; 2] = [6.0, 19.0];
/// The award window's title place and look, and its item names' look (font, size at 1:1, ARGB).
// EVIDENCE: cshell+0x286500 (title font 0xe size 0xe 0xfff3ab01 at (0xe, 9); the name font 6 size 0xe 0xffa0c3c2)
pub const TITLE_AT: [f32; 2] = [14.0, 9.0];
pub const TITLE_FONT: (i32, f32, u32) = (0xe, 14.0, 0xfff3_ab01);
pub const NAME_FONT: (i32, f32, u32) = (6, 14.0, 0xffa0_c3c2);
/// The award window's "<name> / <period>" line font, for the won item's name under the showcase.
// EVIDENCE: cshell+0x286500 (the period line font 2 size 0x12 0xffa0c3c2)
pub const REVEAL_NAME_FONT: i32 = 2;

/// The strip on the virtual 1024 x 768 screen: the window (the band is its middle row), a cell (the slot frame) and
/// the step between cells.
pub const BAND: [i32; 4] = [0, 240, 1024, 253];
pub const CELL: [i32; 2] = [240, 114];
pub const PITCH: i32 = 252;
pub const CELL_TOP: i32 = 318;
/// The box the item's own pixels are fitted into, and its place in the cell (under the name line).
pub const PICTURE: [i32; 2] = [221, 80];
pub const PICTURE_AT: [i32; 2] = [10, 28];
/// The largest a slot picture is drawn, as a factor of its size.
pub const PICTURE_MAX_SCALE: f32 = 2.5;

/// The box of the pixels of `tex` that are not (nearly) transparent: (x0, y0, x1, y1), ends excluded.
pub fn opaque_box(tex: &crate::hud::compose::Tex) -> Option<[u32; 4]> {
    let (w, h) = (tex.width, tex.height);
    let mut b = [w, h, 0, 0];
    for y in 0..h {
        for x in 0..w {
            let a = tex
                .rgba
                .get(((y * w + x) * 4 + 3) as usize)
                .copied()
                .unwrap_or(0);
            if a > 24 {
                b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
            }
        }
    }
    (b[2] > b[0] && b[3] > b[1]).then_some(b)
}

/// Where a picture of `size` whose own pixels are `bx` is drawn so those pixels fill `area` (x, y, w, h) as far as
/// they can, centred: the destination (x, y, w, h) of the whole picture.
pub fn fit(size: (u32, u32), bx: [u32; 4], area: [f32; 4]) -> [f32; 4] {
    let (bw, bh) = ((bx[2] - bx[0]) as f32, (bx[3] - bx[1]) as f32);
    let s = (area[2] / bw).min(area[3] / bh).min(PICTURE_MAX_SCALE);
    let left = area[0] + (area[2] - bw * s) / 2.0 - bx[0] as f32 * s;
    let top = area[1] + (area[3] - bh * s) / 2.0 - bx[1] as f32 * s;
    [left, top, size.0 as f32 * s, size.1 as f32 * s]
}
pub const MARK_X: i32 = 512;

/// A spin: the strip, its start, its length and where in the award's cell it stops.
#[derive(Debug, Clone, PartialEq)]
pub struct Reel {
    pub cells: Vec<Cell>,
    pub title: String,
    start: Instant,
    spin: Duration,
    /// The stop inside the award's cell, -0.4..0.4 of a cell from its middle.
    land: f32,
    /// The cell under the mark at the last tick (a new one plays the tick sound).
    under: i64,
    /// Sounds asked for since the last [`Reel::take_sounds`].
    sounds: Vec<Sound>,
    landed: bool,
    /// The award's tier and the seed of the strip (the brass uses it too).
    tier: Rarity,
    seed: u64,
    /// The tell's tier now (0 Common .. the award's) and when it last climbed.
    tell: usize,
    tell_at: Option<Instant>,
    /// The bomb run's next beep (and whether it began), and the reveal's sounds still to play (ms after the hit).
    beep_at: Option<Instant>,
    beeping: bool,
    revealed: bool,
    due: Vec<(u64, Sound)>,
    /// A click after the stop: the award window may open now.
    cut: bool,
    /// CRATE-8: the won gun's third-person model (`None`: no showcase, e.g. an item that is no gun).
    model: Option<crate::world::ModelRef>,
}

/// A small public-domain mixer (splitmix64) for the strip's order.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A number from 0 to 1 from `seed` and `k`.
fn unit(seed: u64, k: u64) -> f32 {
    (mix(seed ^ k.wrapping_mul(0x2545_f491_4f6c_dd1d)) % 10_000) as f32 / 10_000.0
}

/// The share of the distance covered at `t` (0..1) of the spin: fast at first, creeping to a stop.
// DC-DEFAULT: CRATE-7 (proposed; USER REQUEST 2026-11-06 "slowdown could be a little slower at the end", then "it sits
// on the gun you're going to win for way too long") the power 2.5 over 67.5 cells: the mark reaches the award's cell
// about 0.5..1.25 s before the stop of a 7 s spin (the fifth power over 37.5 cells took it there 2.2..3.6 s before)
pub const EASE_POWER: f32 = 2.5;
pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powf(EASE_POWER)
}

/// The `[r, g, b, a]` of a tier colour at `alpha` (0..1).
fn rgba(c: [u8; 3], alpha: f32) -> [u8; 4] {
    [c[0], c[1], c[2], (alpha.clamp(0.0, 1.0) * 255.0) as u8]
}

/// `c` mixed toward white by `w` (0..1).
fn whiten(c: [u8; 3], w: f32) -> [u8; 3] {
    c.map(|v| (f32::from(v) + (255.0 - f32::from(v)) * w) as u8)
}

impl Reel {
    /// A spin over `pool` (one cell per pool entry, any order) that stops on `award`, started at `now`; `seed` picks the
    /// order and the stop. `None` for an empty pool.
    pub fn new(
        pool: &[Cell],
        award: Cell,
        title: String,
        ms: u32,
        seed: u64,
        now: Instant,
    ) -> Option<Reel> {
        if pool.is_empty() {
            return None;
        }
        let mut cells: Vec<Cell> = (0..CELLS as u64)
            .map(|i| {
                pool[(mix(seed ^ i.wrapping_mul(0x1000_0001)) % pool.len() as u64) as usize].clone()
            })
            .collect();
        let tier = award.rarity.unwrap_or_default();
        cells[WINNER] = award;
        let land = ((mix(seed ^ 0xa5a5) % 1000) as f32 / 1000.0 - 0.5) * 0.8;
        Some(Reel {
            cells,
            title,
            start: now,
            spin: Duration::from_millis(u64::from(ms.max(1))),
            land,
            under: LEAD as i64,
            sounds: Vec::new(),
            landed: false,
            tier,
            seed,
            tell: 0,
            tell_at: None,
            beep_at: None,
            beeping: false,
            revealed: false,
            due: Vec::new(),
            cut: false,
            model: None,
        })
    }

    /// The same spin, ending on the won gun's own model (CRATE-8).
    pub fn with_model(mut self, model: Option<crate::world::ModelRef>) -> Reel {
        self.model = model;
        self
    }

    /// The award's tier.
    pub fn tier(&self) -> Rarity {
        self.tier
    }

    fn strength(&self) -> u64 {
        u64::from(reelfx::strength(self.tier))
    }

    fn progress(&self, now: Instant) -> f32 {
        now.saturating_duration_since(self.start).as_secs_f32() / self.spin.as_secs_f32()
    }

    /// How far the strip has moved at `now`, in cells: the cell under the mark is its whole part.
    pub fn travel(&self, now: Instant) -> f32 {
        let end = WINNER as f32 + 0.5 + self.land;
        LEAD + (end - LEAD) * ease(self.progress(now))
    }

    /// The strip has stopped.
    pub fn stopped(&self, now: Instant) -> bool {
        self.progress(now) >= 1.0
    }

    /// The lights-out before the hit (Legendary only).
    fn hush(&self) -> Duration {
        Duration::from_millis(if self.tier == Rarity::Legendary {
            HUSH_MS
        } else {
            0
        })
    }

    /// From the stop to the reveal's hit: the hush, then from Epic a short freeze.
    pub fn hit_after_stop(&self) -> Duration {
        let s = self.strength();
        self.hush() + Duration::from_millis(if s >= 3 { 70 + 30 * s } else { 0 })
    }

    /// How long after the stop the award window opens.
    pub fn hold_after_stop(&self) -> Duration {
        self.hit_after_stop() + Duration::from_millis(HOLD_MS + HOLD_PER_TIER_MS * self.strength())
    }

    fn since_stop(&self, now: Instant) -> Option<Duration> {
        let stop = self.start + self.spin;
        (now >= stop).then(|| now - stop)
    }

    /// Seconds since the reveal's hit (`None` before it).
    pub fn reveal_age(&self, now: Instant) -> Option<f32> {
        let s = self.since_stop(now)?;
        s.checked_sub(self.hit_after_stop())
            .map(|d| d.as_secs_f32())
    }

    /// The award window may open: the reveal ran its hold, or a click after the stop.
    pub fn done(&self, now: Instant) -> bool {
        self.cut
            || self
                .since_stop(now)
                .is_some_and(|s| s >= self.hold_after_stop())
    }

    /// A click: during the spin it jumps to the stop (the reveal still plays), after the stop it ends the reel.
    pub fn skip(&mut self, now: Instant) {
        if !self.stopped(now) {
            self.start = now.checked_sub(self.spin).unwrap_or(self.start);
            self.tell = self.tier.index();
            // the click ends the bomb run too (a beep due before it is not played after it)
            self.beeping = true;
            self.beep_at = None;
        } else {
            self.cut = true;
        }
    }

    /// The won item as a weapon row, when it is one (its own sounds play at the reveal).
    fn weapon(&self) -> Option<u16> {
        let n = self.cells[WINNER].item;
        (0..2000).contains(&n).then_some(n as u16)
    }

    /// The reveal's sounds by time after the hit (ms): the gun's shot (from Rare) and its rack, then the rank-up
    /// (Epic) or match-win (Legendary) sound. An item that is no gun gets the equip sound.
    fn reveal_sounds(&self) -> Vec<(u64, Sound)> {
        let s = self.strength();
        let mut v = match self.weapon() {
            Some(weapon) if s >= 2 => vec![
                (
                    0,
                    Sound::Weapon {
                        k: WEAPON_FIRE,
                        weapon,
                    },
                ),
                (
                    380,
                    Sound::Weapon {
                        k: WEAPON_SELECT,
                        weapon,
                    },
                ),
            ],
            Some(weapon) => vec![(
                0,
                Sound::Weapon {
                    k: WEAPON_SELECT,
                    weapon,
                },
            )],
            None => vec![(0, Sound::Rom(frontfx::SOUND_SHOP_EQUIP))],
        };
        match self.tier {
            Rarity::Epic => v.push((650, Sound::Rom(frontfx::SOUND_LEVEL_UP))),
            Rarity::Legendary => v.push((650, Sound::Rom(frontfx::SOUND_RESULT_WIN))),
            _ => {}
        }
        v
    }

    /// One frame: the tell's climb, the bomb run (at most one beep a frame), a tick when a new cell passed the
    /// mark (at most one a frame), the stop and the reveal's sounds as they fall due.
    pub fn tick(&mut self, now: Instant) {
        let p = self.progress(now);
        while self.tell < self.tier.index() && p >= TELL_AT[self.tell + 1] {
            self.tell += 1;
            self.tell_at = Some(now);
        }
        if !self.beeping && p >= BEEP_FROM {
            self.beeping = true;
            self.beep_at = Some(now);
        }
        if let Some(at) = self.beep_at.filter(|&at| at <= now) {
            let stop = self.start + self.spin;
            let left = stop.saturating_duration_since(at).as_secs_f32() * 1000.0;
            if left < BEEP_LAST_MS {
                self.beep_at = None;
            } else {
                self.sounds.push(Sound::Rom(frontfx::SOUND_START_COUNT));
                let gap = (left * BEEP_GAP_SHARE)
                    .min(BEEP_SLOW_MS + BEEP_SLOW_PER_MS * left)
                    .clamp(BEEP_GAP_MIN_MS, BEEP_GAP_MAX_MS);
                self.beep_at = Some(at + Duration::from_secs_f32(gap / 1000.0));
            }
        }
        let under = self.travel(now).floor() as i64;
        if under != self.under && !self.landed {
            self.under = under;
            self.sounds.push(Sound::Rom(frontfx::SOUND_SELECT_CHANGE));
        }
        if !self.landed && self.stopped(now) {
            self.landed = true;
            self.sounds.push(Sound::Rom(frontfx::SOUND_SELECT_ITEM));
        }
        let Some(since) = self.since_stop(now) else {
            return;
        };
        if !self.revealed && since >= self.hit_after_stop() {
            self.revealed = true;
            self.due = self.reveal_sounds();
        }
        if let Some(age) = self.reveal_age(now) {
            let ms = (age * 1000.0) as u64;
            let (now_due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.due)
                .into_iter()
                .partition(|d| d.0 <= ms);
            self.sounds.extend(now_due.into_iter().map(|d| d.1));
            self.due = later;
        }
    }

    pub fn take_sounds(&mut self) -> Vec<Sound> {
        std::mem::take(&mut self.sounds)
    }

    /// The left edge of cell `i` on the screen at `now`.
    pub fn cell_x(&self, i: usize, now: Instant) -> f32 {
        MARK_X as f32 + (i as f32 - self.travel(now)) * PITCH as f32
    }

    /// The reveal's shake at `now`, in virtual pixels: strong at the hit, dying away within a second.
    pub fn shake(&self, now: Instant) -> [f32; 2] {
        let Some(a) = self.reveal_age(now) else {
            return [0.0; 2];
        };
        let s = self.strength() as f32;
        let trauma = (0.18 + 0.17 * s - 1.32 * a).max(0.0);
        let d = trauma * trauma;
        [
            d * 26.0 * ((a * 38.0).sin() * 0.6 + (a * 87.0 + 1.3).sin() * 0.4),
            d * 20.0 * ((a * 33.0 + 0.7).sin() * 0.6 + (a * 76.0 + 2.1).sin() * 0.4),
        ]
    }

    /// CRATE-8: the won gun's model as the menu draws it at `now`: made ready (not shown) from the start of the spin, so
    /// the render is there at the hit; from the hit it leaps out of its cell to [`SHOWCASE_RECT`] and turns.
    pub fn showcase(&self, now: Instant) -> Option<ShowcaseFrame> {
        let model = self.model.clone()?;
        let rim = reelfx::colour(self.tier).map(|c| f32::from(c) / 255.0);
        let [rx, ry, rw, rh] = SHOWCASE_RECT.map(|v| v as f32);
        let Some(a) = self.reveal_age(now) else {
            return Some(ShowcaseFrame {
                model,
                yaw: SIDE_YAW,
                pitch: 0.0,
                dst: [rx, ry, rw, rh],
                alpha: 0.0,
                rim,
                shown: false,
            });
        };
        // the leap: from the cell's size and place to the showcase, overshooting a little
        let q = (a / LEAP_S).min(1.0);
        let c1 = 1.70158;
        let grow = 1.0 + (c1 + 1.0) * (q - 1.0).powi(3) + c1 * (q - 1.0).powi(2);
        let off = self.shake(now);
        let from_w = CELL[0] as f32 / SHOWCASE_FILL;
        let from = [
            self.cell_x(WINNER, now) + CELL[0] as f32 / 2.0,
            (CELL_TOP + CELL[1] / 2) as f32,
            from_w,
            from_w * rh / rw,
        ];
        let to = [rx + rw / 2.0, ry + rh / 2.0, rw, rh];
        let lerp = |i: usize| from[i] + (to[i] - from[i]) * grow;
        let (cx, cy, w, h) = (lerp(0) + off[0], lerp(1) + off[1], lerp(2), lerp(3));
        Some(ShowcaseFrame {
            model,
            yaw: SIDE_YAW + a * TURN_PER_S,
            pitch: 0.18 + 0.06 * (a * 1.7).sin(),
            dst: [cx - w / 2.0, cy - h / 2.0, w, h],
            alpha: (a / 0.08).min(1.0),
            rim,
            shown: true,
        })
    }

    /// The strip's draw items: the window (`look`: the award window's art and the shop's slot frame), every cell on the
    /// screen, the mark and the title, then the reveal's backdrop and effects (the menu draws the gun's model over them,
    /// [`Reel::showcase`]).
    pub fn draw(&self, cat: &mut Catalog, look: &ReelLook, now: Instant, out: &mut Vec<DrawItem>) {
        let fill = |r: [f32; 4], colour: [u8; 4]| DrawItem::Fill { rect: r, colour };
        let [bx, by, bw, bh] = BAND.map(|v| v as f32);
        out.push(fill([bx, by, bw, bh], [29, 26, 22, 245]));
        let mut y = by;
        for (k, rows) in ART_ROWS.into_iter().enumerate() {
            let fill = if k == 1 {
                Middle::Tile
            } else {
                Middle::Stretch
            };
            y += window_rows(&look.window, rows, y, fill, out);
        }
        let band_y = by + (ART_ROWS[0][1] - ART_ROWS[0][0]) as f32 * SCALE;
        let centred = Align {
            first: ALIGN_CENTRE,
            second: 0,
        };
        let (font, size, colour) = TITLE_FONT;
        out.push(place_text(
            &self.title,
            [
                (bx + TITLE_AT[0] * SCALE) as i32,
                (by + TITLE_AT[1] * SCALE) as i32,
            ],
            [0, 0],
            Align {
                first: ALIGN_LEFT,
                second: 0,
            },
            font,
            (size * SCALE).round() as i32,
            colour,
        ));
        let off = self.shake(now);
        let age = self.reveal_age(now);
        let s = self.strength() as f32;
        let tier_c = reelfx::colour(self.tier);
        // the names go on top of the mark, so the stop never hides the award's name
        let mut names = Vec::new();
        for i in 0..self.cells.len() {
            self.draw_cell(cat, look, i, now, off, None, out, &mut names);
        }
        // the cells run under the window's edges
        window_rows(&look.window, ART_ROWS[1], band_y, Middle::None, out);
        // the mark: a line over the strip with a notch above and below, in the tell's colour
        let tell_c = reelfx::colour(Rarity::ALL[self.tell.min(3)]);
        let (mx, top, bottom) = (
            MARK_X as f32 + off[0],
            CELL_TOP as f32 - 10.0 + off[1],
            (CELL_TOP + CELL[1]) as f32 + 10.0 + off[1],
        );
        if let Some(t) = self.tell_at {
            let a = now.saturating_duration_since(t).as_secs_f32();
            let pulse = (1.0 - a / 0.45).max(0.0);
            if pulse > 0.0 {
                out.push(DrawItem::Image {
                    tex: reelfx::GLOW.into(),
                    src: [128.0, 128.0],
                    dst: [mx - 60.0, top - 20.0, 120.0, bottom - top + 40.0],
                    tint: rgba(tell_c, 0.8 * pulse),
                    turn: None,
                });
            }
        }
        out.push(fill([mx - 1.5, top, 3.0, bottom - top], rgba(tell_c, 1.0)));
        out.push(fill([mx - 7.0, top - 4.0, 14.0, 4.0], rgba(tell_c, 1.0)));
        out.push(fill([mx - 7.0, bottom, 14.0, 4.0], rgba(tell_c, 1.0)));
        out.append(&mut names);
        // lights out before a Legendary: the screen goes dark around the award, which stays lit
        let since = self.since_stop(now).map(|d| d.as_secs_f32());
        let hush = self.hush().as_secs_f32();
        if let Some(t) = since.filter(|_| hush > 0.0 && age.is_none()) {
            let dark = (t / 0.35).min(1.0) * 0.9;
            out.push(fill(
                [0.0, 0.0, 1024.0, 768.0],
                [0, 0, 0, (dark * 255.0) as u8],
            ));
            let mut n = Vec::new();
            self.draw_cell(cat, look, WINNER, now, off, Some(0.55), out, &mut n);
            out.append(&mut n);
        }
        let Some(a) = age else {
            return;
        };
        // the stage: the screen dims behind the gun (from the hush's dark for a Legendary)
        let from = if hush > 0.0 { 0.9 } else { 0.0 };
        let dim = from + (0.62 - from) * (a / 0.3).min(1.0);
        out.push(fill(
            [0.0, 0.0, 1024.0, 768.0],
            [0, 0, 0, (dim * 255.0) as u8],
        ));
        let [rx, ry, rw, rh] = SHOWCASE_RECT.map(|v| v as f32);
        let (cx, cy) = (rx + rw / 2.0 + off[0], ry + rh / 2.0 + off[1]);
        // searchlights from above the screen onto the gun, sweeping slowly (Rare and up: 2, 3, 4)
        if s >= 2.0 {
            let beams = s as usize;
            let alpha = [0.0, 0.0, 0.22, 0.3, 0.42][s as usize] * (a / 0.4).min(1.0);
            for k in 0..beams {
                let side = k as f32 / (beams - 1).max(1) as f32 - 0.5;
                let src = [cx + side * 900.0, -40.0];
                let sweep = 0.1 * (a * 0.8 + k as f32 * 1.9).sin();
                let angle = (cx - src[0]).atan2(-(cy - src[1])) + sweep;
                let len = ((cx - src[0]).powi(2) + (cy - src[1]).powi(2)).sqrt() * 1.25;
                out.push(DrawItem::Image {
                    tex: reelfx::RAY.into(),
                    src: [64.0, 512.0],
                    dst: [src[0] - 110.0, src[1] - len, 220.0, len],
                    tint: rgba(whiten(tier_c, 0.55), alpha),
                    turn: Some((src, angle)),
                });
            }
        }
        // the light behind the gun, breathing
        let pulse = 0.4 + 0.12 * (a * std::f32::consts::TAU / 2.4).sin();
        let gr = 210.0 + 30.0 * s;
        out.push(DrawItem::Image {
            tex: reelfx::GLOW.into(),
            src: [128.0, 128.0],
            dst: [cx - gr * 1.6, cy - gr, gr * 3.2, gr * 2.0],
            tint: rgba(tier_c, pulse * (a / 0.25).min(1.0)),
            turn: None,
        });
        // the muzzle flash: a short white burst at the hit
        let flash = (1.0 - a / (0.16 + 0.03 * s)).max(0.0);
        if flash > 0.0 {
            let r = 260.0 + 70.0 * s;
            out.push(DrawItem::Image {
                tex: reelfx::GLOW.into(),
                src: [128.0, 128.0],
                dst: [cx - r, cy - r * 0.7, r * 2.0, r * 1.4],
                tint: rgba(whiten(tier_c, 0.85), flash * (0.55 + 0.1 * s)),
                turn: None,
            });
        }
        // ejected brass: casings thrown up and right of the gun, tumbling and falling
        let count = 3 + 3 * self.strength();
        for i in 0..count {
            let delay = 0.05 * i as f32 + 0.38 * (i % 2) as f32;
            let t = a - delay;
            let life = 1.3;
            if !(0.0..life).contains(&t) {
                continue;
            }
            let vx = 160.0 + 220.0 * unit(self.seed, 3 * i);
            let vy = -(300.0 + 200.0 * unit(self.seed, 3 * i + 1));
            let spin = 9.0 + 10.0 * unit(self.seed, 3 * i + 2);
            let x = cx + 40.0 + vx * t;
            let y = cy - 30.0 + vy * t + 0.5 * 1_300.0 * t * t;
            let fade = (1.0 - t / life).min(1.0).powf(0.6);
            out.push(DrawItem::Image {
                tex: reelfx::CASING.into(),
                src: [24.0, 10.0],
                dst: [x - 9.0, y - 3.5, 18.0, 7.0],
                tint: [255, 255, 255, (fade * 255.0) as u8],
                turn: Some(([x, y], t * spin)),
            });
        }
        // the gun's name and its tier under the showcase
        let fade = (a / 0.25).min(1.0);
        let argb = |c: [u8; 3]| {
            ((fade * 255.0) as u32) << 24
                | u32::from(c[0]) << 16
                | u32::from(c[1]) << 8
                | u32::from(c[2])
        };
        let below = SHOWCASE_RECT[1] + SHOWCASE_RECT[3];
        // in the award window's name line look (CRATE-9), the tier under it in the title's font
        out.push(place_text(
            &self.cells[WINNER].name,
            [MARK_X + off[0] as i32, below + 6],
            [0, 0],
            centred,
            REVEAL_NAME_FONT,
            26,
            argb([0xa0, 0xc3, 0xc2]),
        ));
        out.push(place_text(
            &self.tier.name().to_uppercase(),
            [MARK_X + off[0] as i32, below + 40],
            [0, 0],
            centred,
            TITLE_FONT.0,
            20,
            argb(tier_c),
        ));
    }

    /// Cell `i` at `now`, moved by `off`; `dim` draws it darker (the hush). Its name goes to `names`.
    #[allow(clippy::too_many_arguments)]
    fn draw_cell(
        &self,
        cat: &mut Catalog,
        look: &ReelLook,
        i: usize,
        now: Instant,
        off: [f32; 2],
        dim: Option<f32>,
        out: &mut Vec<DrawItem>,
        names: &mut Vec<DrawItem>,
    ) {
        let c = &self.cells[i];
        let x = self.cell_x(i, now) + off[0];
        if x + CELL[0] as f32 <= 0.0 || x >= 1024.0 {
            return;
        }
        let won = i == WINNER && self.stopped(now);
        let y = CELL_TOP as f32 + off[1];
        let [w, h] = CELL.map(|v| v as f32);
        let shade = dim.unwrap_or(1.0);
        let shaded = |c: [u8; 3]| c.map(|v| (f32::from(v) * shade) as u8);
        // the slot frame, tinted by the tier (an item without one keeps the frame's own grey)
        let tint = match (c.rarity, won) {
            (Some(r), true) => whiten(reelfx::colour(r), 0.35),
            (Some(r), false) => whiten(reelfx::colour(r), 0.15),
            (None, _) => [255, 255, 255],
        };
        let [r, g, b] = shaded(tint);
        nine_scaled(
            &look.slot,
            SLOT_ART,
            [x, y, w, h],
            SCALE,
            [r, g, b, 255],
            out,
        );
        if won {
            let [r, g, b] = shaded(reelfx::deep(self.tier));
            out.push(DrawItem::Fill {
                rect: [x + 3.0, y + 3.0, w - 6.0, h - 6.0],
                colour: [r, g, b, 150],
            });
        }
        let area = [
            x + PICTURE_AT[0] as f32,
            y + PICTURE_AT[1] as f32,
            PICTURE[0] as f32,
            PICTURE[1] as f32,
        ];
        if let Some(t) = c.picture.as_deref().and_then(|p| cat.assets.texture(p)) {
            let size = (t.width, t.height);
            let bx = opaque_box(&t).unwrap_or([0, 0, t.width, t.height]);
            let v = (255.0 * shade) as u8;
            out.push(DrawItem::Image {
                tex: c.picture.clone().unwrap_or_default(),
                src: [size.0 as f32, size.1 as f32],
                dst: fit(size, bx, area),
                tint: [v, v, v, 255],
                turn: None,
            });
        }
        if let Some((tex, size)) = c.tag {
            let v = (255.0 * shade) as u8;
            out.push(DrawItem::Image {
                tex: tex.into(),
                src: size.map(|v| v as f32),
                dst: [
                    x + TAG_AT[0] * SCALE,
                    y + TAG_AT[1] * SCALE,
                    size[0] as f32 * SCALE,
                    size[1] as f32 * SCALE,
                ],
                tint: [v, v, v, 255],
                turn: None,
            });
        }
        let (font, size, colour) = NAME_FONT;
        let name_c = if won {
            let [r, g, b] = whiten(reelfx::colour(self.tier), 0.6);
            0xff00_0000 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
        } else {
            colour
        };
        names.push(place_text(
            &c.name,
            [
                (x + NAME_AT[0] * SCALE) as i32,
                (y + NAME_AT[1] * SCALE) as i32,
            ],
            [0, 0],
            Align {
                first: ALIGN_LEFT,
                second: 0,
            },
            font,
            (size * SCALE).round() as i32,
            name_c,
        ));
    }
}

/// CRATE-9: what the reel window is drawn of: the award window's opened art and the shop's slot frame (both read from
/// the user's ROM at run time).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReelLook {
    pub window: String,
    pub slot: String,
}

/// How [`window_rows`] fills between the art's edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Middle {
    /// [`ART_TILE`] repeated (the perforated band).
    Tile,
    /// The art's middle stretched (the title bar and the footer).
    Stretch,
    /// Nothing: the two edges only, drawn over the cells.
    None,
}

/// The art's rows `rows` across the screen at `y` ([`SCALE`] times their height): the left edge, the middle as `fill`
/// asks, the right edge. Returns the height drawn.
fn window_rows(art: &str, rows: [u32; 2], y: f32, fill: Middle, out: &mut Vec<DrawItem>) -> f32 {
    let th = rows[1] - rows[0];
    let h = th as f32 * SCALE;
    let mut piece = |x0: u32, tw: u32, dx: f32, dw: f32| {
        out.push(DrawItem::Image {
            tex: crate::hud::compose::part_of(art, [x0, rows[0], tw, th]),
            src: [tw as f32, th as f32],
            dst: [dx, y, dw, h],
            tint: [255; 4],
            turn: None,
        });
    };
    let edge = ART_RIGHT[1] - ART_RIGHT[0];
    let right = 1024.0 - edge as f32 * SCALE;
    let left = ART_LEFT as f32 * SCALE;
    match fill {
        Middle::Tile => {
            let span = ART_TILE[1] - ART_TILE[0];
            let mut x = left;
            while x < right {
                let tw = (((right - x) / SCALE).ceil() as u32).min(span);
                piece(ART_TILE[0], tw, x, tw as f32 * SCALE);
                x += tw as f32 * SCALE;
            }
        }
        Middle::Stretch => piece(ART_LEFT, ART_RIGHT[0] - ART_LEFT, left, right - left),
        Middle::None => {}
    }
    piece(0, ART_LEFT, 0.0, left);
    piece(ART_RIGHT[0], edge, right, edge as f32 * SCALE);
    h
}

/// The 2009 frame control's nine parts ([`super::nine`]) at `scale` times their size: the corners keep a quarter of
/// the art each, the edges stretch along their side, the middle over the rest.
fn nine_scaled(
    tex: &str,
    art: (u32, u32),
    dst: [f32; 4],
    scale: f32,
    tint: [u8; 4],
    out: &mut Vec<DrawItem>,
) {
    let (tw, th) = art;
    let (cw, ch) = (tw / 4, th / 4);
    let tx = [(0, cw), (cw, tw - 2 * cw), (tw - cw, cw)];
    let ty = [(0, ch), (ch, th - 2 * ch), (th - ch, ch)];
    let [x, y, w, h] = dst;
    let (sw, sh) = (cw as f32 * scale, ch as f32 * scale);
    let sx = [(x, sw), (x + sw, w - 2.0 * sw), (x + w - sw, sw)];
    let sy = [(y, sh), (y + sh, h - 2.0 * sh), (y + h - sh, sh)];
    for r in 0..3 {
        for c in 0..3 {
            let ((u, tu), (v, tv)) = (tx[c], ty[r]);
            let ((px, pw), (py, ph)) = (sx[c], sy[r]);
            if tu == 0 || tv == 0 || pw <= 0.0 || ph <= 0.0 {
                continue;
            }
            out.push(DrawItem::Image {
                tex: crate::hud::compose::part_of(tex, [u, v, tu, tv]),
                src: [tu as f32, tv as f32],
                dst: [px, py, pw, ph],
                tint,
                turn: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(n: i32) -> Cell {
        Cell {
            item: n,
            picture: None,
            name: format!("#{n}"),
            rarity: None,
            tag: None,
        }
    }

    #[test]
    fn the_strip_slows_down_and_stops_with_the_award_under_the_mark() {
        let now = Instant::now();
        let pool: Vec<Cell> = (1..=5).map(cell).collect();
        for seed in 0..20 {
            let r = Reel::new(&pool, cell(99), "Case".into(), 5_000, seed, now).unwrap();
            assert_eq!(r.cells.len(), CELLS);
            assert_eq!(r.cells[WINNER].item, 99);
            assert!(r
                .cells
                .iter()
                .enumerate()
                .all(|(i, c)| i == WINNER || (1..=5).contains(&c.item)));
            let end = now + Duration::from_millis(5_000);
            assert!(r.stopped(end) && !r.stopped(end - Duration::from_millis(1)));
            assert_eq!(r.travel(end).floor() as usize, WINNER, "seed {seed}");
            // the mark is inside the award's cell, not on its edge
            let x = r.cell_x(WINNER, end);
            assert!(
                x < MARK_X as f32 - 0.1 * PITCH as f32
                    && x + CELL[0] as f32 > MARK_X as f32 + 0.05 * PITCH as f32
            );
            // it moves fastest at the start and never backwards
            let at = |ms| r.travel(now + Duration::from_millis(ms));
            assert!(at(500) - at(0) > at(4_500) - at(4_000));
            assert!((0..50).all(|k| at(k * 100) <= at(k * 100 + 100)));
            assert!(!r.done(end) && r.done(end + r.hold_after_stop()));
        }
        assert!(Reel::new(&[], cell(1), String::new(), 5_000, 1, now).is_none());
    }

    #[test]
    fn a_picture_is_drawn_so_its_own_pixels_fill_the_box() {
        // a 254 x 71 slot picture whose gun sits at x 90..234, y 10..60
        let d = fit((254, 71), [90, 10, 234, 60], [100.0, 50.0, 212.0, 104.0]);
        let s = d[2] / 254.0;
        assert!((s - 212.0 / 144.0).abs() < 1e-4, "width-bound: {s}");
        let gun_left = d[0] + 90.0 * s;
        let gun_right = d[0] + 234.0 * s;
        assert!((gun_left - 100.0).abs() < 1e-3 && (gun_right - 312.0).abs() < 1e-3);
        let gun_top = d[1] + 10.0 * s;
        let gun_bottom = d[1] + 60.0 * s;
        assert!(gun_top >= 50.0 - 1e-3 && gun_bottom <= 154.0 + 1e-3);
        assert!(
            ((gun_top + gun_bottom) / 2.0 - 102.0).abs() < 1e-3,
            "centred"
        );
        // a tiny picture is not blown up past the cap
        let t = fit((20, 10), [0, 0, 20, 10], [0.0, 0.0, 212.0, 104.0]);
        assert!((t[2] - 20.0 * PICTURE_MAX_SCALE).abs() < 1e-4);
        let tex = crate::hud::compose::Tex {
            width: 4,
            height: 3,
            rgba: (0..12)
                .flat_map(|i| [0, 0, 0, if i == 5 || i == 6 { 255 } else { 0 }])
                .collect(),
        };
        assert_eq!(opaque_box(&tex), Some([1, 1, 3, 2]));
    }

    #[test]
    fn a_new_cell_under_the_mark_ticks_and_the_stop_sounds_once() {
        let now = Instant::now();
        let pool: Vec<Cell> = (1..=3).map(cell).collect();
        let mut r = Reel::new(&pool, cell(7), String::new(), 4_000, 3, now).unwrap();
        r.tick(now);
        assert!(r.take_sounds().is_empty());
        let mut ticks = 0;
        // up to just before the stop
        for k in 1..400 {
            r.tick(now + Duration::from_millis(k * 10));
            ticks += r
                .take_sounds()
                .iter()
                .filter(|s| **s == Sound::Rom(frontfx::SOUND_SELECT_CHANGE))
                .count();
        }
        assert!(ticks > 20 && ticks <= WINNER, "{ticks}");
        // a plain gun: the stop, and at once the Common reveal: the gun is drawn (racked)
        r.tick(now + Duration::from_millis(4_100));
        assert_eq!(
            r.take_sounds(),
            vec![
                Sound::Rom(frontfx::SOUND_SELECT_ITEM),
                Sound::Weapon {
                    k: WEAPON_SELECT,
                    weapon: 7
                },
            ]
        );
        r.tick(now + Duration::from_millis(4_200));
        assert!(r.take_sounds().is_empty());
        // a skip stops it at once; a second one ends the reel
        let mut s = Reel::new(&pool, cell(7), String::new(), 4_000, 3, now).unwrap();
        let t = now + Duration::from_millis(100);
        s.skip(t);
        assert!(s.stopped(t));
        assert_eq!(s.travel(t).floor() as usize, WINNER);
        assert!(!s.done(t));
        s.skip(t);
        assert!(s.done(t));
    }

    /// CRATE-7: the tell climbs one tier at a time up to the award's under the bomb run's beeps; a Legendary goes dark
    /// and silent after the run, then the gun fires, is racked, and the match-win sound plays; a rarer award holds longer and
    /// shakes harder. An item that is no gun gets the equip sound.
    #[test]
    fn the_tell_climbs_to_the_awards_tier_and_a_legendary_hushes_before_its_hit() {
        let now = Instant::now();
        let pool: Vec<Cell> = (1..=3).map(cell).collect();
        let award = |r, item| Cell {
            rarity: Some(r),
            ..cell(item)
        };
        let run = |r: Rarity, item: i32| {
            let mut reel = Reel::new(&pool, award(r, item), String::new(), 5_000, 7, now).unwrap();
            let mut got = Vec::new();
            for k in 0..=1_000 {
                reel.tick(now + Duration::from_millis(k * 10));
                got.extend(reel.take_sounds().into_iter().filter(|s| {
                    !matches!(
                        s,
                        Sound::Rom(frontfx::SOUND_SELECT_CHANGE | frontfx::SOUND_SELECT_ITEM)
                    )
                }));
            }
            (reel, got)
        };
        let beep = Sound::Rom(frontfx::SOUND_START_COUNT);
        let gun = |k| Sound::Weapon { k, weapon: 502 };
        // the run's beeps, then the reveal's sounds
        let split = |sounds: Vec<Sound>| {
            let n = sounds.iter().take_while(|s| **s == beep).count();
            assert!(n >= 10, "{sounds:?}");
            sounds[n..].to_vec()
        };
        let (rare, sounds) = run(Rarity::Rare, 502);
        assert_eq!(split(sounds), vec![gun(WEAPON_FIRE), gun(WEAPON_SELECT)]);
        assert_eq!(rare.hit_after_stop(), Duration::ZERO);
        let (epic, sounds) = run(Rarity::Epic, 502);
        assert_eq!(
            split(sounds),
            vec![
                gun(WEAPON_FIRE),
                gun(WEAPON_SELECT),
                Sound::Rom(frontfx::SOUND_LEVEL_UP)
            ]
        );
        assert_eq!(epic.hit_after_stop(), Duration::from_millis(160));
        let (legend, sounds) = run(Rarity::Legendary, 502);
        assert_eq!(
            split(sounds),
            vec![
                gun(WEAPON_FIRE),
                gun(WEAPON_SELECT),
                Sound::Rom(frontfx::SOUND_RESULT_WIN)
            ]
        );
        assert_eq!(
            legend.hit_after_stop(),
            Duration::from_millis(HUSH_MS + 190)
        );
        assert!(legend.hold_after_stop() > rare.hold_after_stop());
        let stop = now + Duration::from_millis(5_000);
        assert!(
            legend
                .reveal_age(stop + Duration::from_millis(500))
                .is_none(),
            "still dark"
        );
        let shake = |r: &Reel| {
            let hit = stop + r.hit_after_stop() + Duration::from_millis(20);
            let [x, y] = r.shake(hit);
            x.abs() + y.abs()
        };
        assert!(shake(&legend) > shake(&rare));
        assert_eq!(rare.shake(stop + Duration::from_secs(3)), [0.0, 0.0]);
        // a character item (not a gun): the equip sound
        let (_, sounds) = run(Rarity::Common, 2_001);
        assert_eq!(split(sounds), vec![Sound::Rom(frontfx::SOUND_SHOP_EQUIP)]);
    }

    /// CRATE-7: the bomb run starts early in the spin for every tier, quickens slowly, then ever faster, and ends as the
    /// strip comes to rest (a Legendary's dark is silent); a click in the spin ends it with the jump to the stop.
    #[test]
    fn the_bomb_run_starts_early_in_the_spin_and_ends_as_the_strip_stops() {
        let now = Instant::now();
        let pool: Vec<Cell> = (1..=3).map(cell).collect();
        let beeps = |r: Rarity, skip_at: Option<u64>| {
            let award = Cell {
                rarity: Some(r),
                ..cell(502)
            };
            let mut reel = Reel::new(&pool, award, String::new(), 7_000, 7, now).unwrap();
            let mut at = Vec::new();
            for ms in 0..=10_000u64 {
                let t = now + Duration::from_millis(ms);
                if skip_at == Some(ms) {
                    reel.skip(t);
                }
                reel.tick(t);
                if reel
                    .take_sounds()
                    .contains(&Sound::Rom(frontfx::SOUND_START_COUNT))
                {
                    at.push(ms);
                }
            }
            at
        };
        for r in Rarity::ALL {
            let at = beeps(r, None);
            assert_eq!(at[0], 700, "{r:?}");
            let gaps: Vec<u64> = at.windows(2).map(|w| w[1] - w[0]).collect();
            assert!(gaps.windows(2).all(|g| g[1] <= g[0] + 1), "{r:?} {gaps:?}");
            assert!((520..=580).contains(&gaps[0]), "{r:?} {gaps:?}");
            assert!(*gaps.last().unwrap() <= 60, "{r:?} {gaps:?}");
            let last = *at.last().unwrap();
            assert!(last < 7_000 && 7_000 - last <= 100, "{r:?} {last}");
        }
        // a click in the spin jumps to the stop: no beep after it
        let at = beeps(Rarity::Legendary, Some(2_000));
        assert!(at.iter().all(|&b| b < 2_000), "{at:?}");
        // not even one that fell due between the last frame and the click
        let mut reel = Reel::new(&pool, cell(502), String::new(), 7_000, 7, now).unwrap();
        reel.tick(now + Duration::from_millis(1_000));
        reel.take_sounds();
        reel.skip(now + Duration::from_millis(1_900));
        reel.tick(now + Duration::from_millis(1_950));
        assert!(!reel
            .take_sounds()
            .contains(&Sound::Rom(frontfx::SOUND_START_COUNT)));
    }

    /// CRATE-8: the gun's model is made ready during the spin and shown from the hit, leaping from its cell to the
    /// showcase and turning; no model, no showcase.
    #[test]
    fn the_won_gun_leaps_out_of_its_cell_and_turns() {
        let now = Instant::now();
        let pool: Vec<Cell> = (1..=3).map(cell).collect();
        let model = crate::world::ModelRef {
            model: "gun.ltb".into(),
            textures: vec!["gun.dtx".into()],
        };
        let r = Reel::new(&pool, cell(9), String::new(), 4_000, 5, now)
            .unwrap()
            .with_model(Some(model.clone()));
        let early = r.showcase(now).unwrap();
        assert!(!early.shown && early.model == model);
        let stop = now + Duration::from_millis(4_000);
        let at = |ms| r.showcase(stop + Duration::from_millis(ms)).unwrap();
        let (leap, landed, later) = (at(20), at(600), at(1_600));
        assert!(
            leap.shown && leap.dst[2] < landed.dst[2],
            "it grows out of the cell"
        );
        let [x, y, w, h] = SHOWCASE_RECT.map(|v| v as f32);
        assert!((landed.dst[2] - w).abs() < 1.0 && (landed.dst[3] - h).abs() < 1.0);
        assert!((landed.dst[0] - x).abs() < 30.0 && (landed.dst[1] - y).abs() < 30.0);
        assert!(later.yaw > landed.yaw, "it turns");
        assert!(Reel::new(&pool, cell(9), String::new(), 4_000, 5, now)
            .unwrap()
            .showcase(stop)
            .is_none());
    }

    /// CRATE-7: the mark gets into the award's cell only about the last second of a 7 s spin (it may stop near the far
    /// edge, as if it could still tip over), while the strip still creeps through its last cells.
    #[test]
    fn the_mark_reaches_the_awards_cell_only_in_the_last_second_or_so() {
        let now = Instant::now();
        let pool: Vec<Cell> = (1..=3).map(cell).collect();
        let mut entries = Vec::new();
        for seed in 0..40u64 {
            let reel = Reel::new(&pool, cell(502), String::new(), 7_000, seed, now).unwrap();
            let entry = (0..=7_000u64)
                .find(|&ms| reel.travel(now + Duration::from_millis(ms)) >= WINNER as f32)
                .unwrap();
            entries.push(7_000 - entry);
        }
        assert!(
            entries.iter().all(|&left| (400..=1_300).contains(&left)),
            "{entries:?}"
        );
        let mean = entries.iter().sum::<u64>() / entries.len() as u64;
        assert!((800..=1_150).contains(&mean), "{mean} {entries:?}");
        // and it still creeps: the last three cells take more than a quarter of the spin
        let t = 1.0 - (3.0f32 / 67.5).powf(1.0 / EASE_POWER);
        assert!(1.0 - t > 0.25);
        assert!((ease(t) - (1.0 - 3.0 / 67.5)).abs() < 1e-4);
    }
}
