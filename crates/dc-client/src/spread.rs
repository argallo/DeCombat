//! Spread, recoil and aim_mark of the local player's weapon, the way the 2009 client computes them.
//!
//! The weapon keeps two spread values in 0..1 (the 0x82 `frac` byte is `trunc(255 x max(motion, firing))`, the 0x64
//! `weapon_f` float the same maximum):
//! - **firing spread**: one rise of `0.0125 x fire_gain x PerturbIncSpeed` per shot, capped at the ammunition row's
//!   `FirePerturb` / `FireDuckPerturb` / `FireMovePerturb` / `FireMoveDuckPerturb` (standing, crouched, moving,
//!   moving crouched); after `DecDelay` ms without a shot it falls by `0.0125 x fire_loss x PerturbDecSpeed` per step;
//! - **motion spread**: follows `speed fraction x WalkPerturb / RunPerturb / JumpPerturb` (by the run bit and the
//!   jump state), rising by at most `0.0125 x move_gain` x `PerturbIncSpeed` and falling by at most
//!   `0.0125 x move_loss` x `PerturbDecSpeed` per step.
//!
//! Both advance one step per tick of the millisecond tick clock, which moves in 15.625 ms ticks (the Windows default
//! timer the 2009 client read with `GetTickCount`): 64 steps a second. A frame runs the steps of the ticks that
//! passed since the last frame that ran one, each with its own tick value for the delay test; a shot frame runs
//! those first, reads the value the 0x82 sends, and then runs one fire step at the shot tick. So the state and the
//! bytes are a function of the session clock and the shot times only, never of the frame rate (project decision: no
//! frame-rate cap, frame-rate independence by design):
//! 30, 60, 144 or 1000 frames per second give the same bytes. The 2009 client counted `trunc(elapsed ms x 0.001 /
//! 0.0125)` steps per frame (the same one per tick for frames shorter than 62 ms), forced one step on a shot frame
//! (which swallowed the fall step of a tick that passed in that frame) and capped a frame at 100 steps; DeCombat
//! does none of these three, so a stall or a shot leaves the same state as the frames it skipped would have.
//! The one frame-sampled input is the view turn: a frame's view change is split evenly over its steps.
//!
//! Recoil: the numbers are read here ([`RecoilRow`]); the pitch kick, the yaw kick and the drain live on the view, in
//! [`crate::recoil`].
//!
//! Aim mark: four 1-pixel arms of length `7 x s` around the screen centre at a distance `(MinPerturb + spread x
//! (MaxPerturb - MinPerturb)) x s` (`s` = target width / 1024), pushed out by `4 x s x (1 - t)` and lengthened by
//! `6 x s x (1 - t)` for `t` = 0..1 over 0.1 s after a shot.
//!
//! All numbers of a weapon come from the user's ROM at run time (`[Weapon<n>]`, its `[Ammo<n>]` row, the `[Perturb]`
//! section of the weapon table); the constants here are code constants of the 2009 client or DeCombat defaults.
// EVIDENCE: cshell+0x237180 (weapon update: the step loop, DISASM cshell+0x237590..0x23760f: elapsed GetTickCount ms
// x 0.001 / 0.0125, ftol, at least 1 on a shot frame, at most 100, last tick := now only when a step ran)
// EVIDENCE: cshell+0x232120 (firing spread, weapon +0xd0), cshell+0x2319b0 (motion spread, weapon +0xcc),
// cshell+0x2f430 (weapon slot 0x44 = max(+0xcc, +0xd0)), cshell+0x23bd15 (the fire routine reads slot 0x44 for 0x82)
// EVIDENCE: cshell+0x17bd20 (weapon row loader: MinPerturb +0x288, MaxPerturb +0x28c, DuckPerturb +0x290,
// MoveDuckPerturb +0x294, PerturbIncSpeed +0x2dc, PerturbDecSpeed +0x2e0), cshell+0x174d30 (ammunition row loader:
// FireMoveDuckPerturb +0xac .. FirePerturb +0xb8, WalkPerturb +0xa4, RunPerturb +0xa8, JumpPerturb +0xa0, DecDelay
// +0xe8 default 150, JitterDecDelay +0xec default 600, JitterWaitRecover +0xf0 default 0.1, JitterRecover +0xf4
// default 0.3)
// EVIDENCE: cshell+0x180590 ([Perturb] section of the weapon table), DISASM cshell+0x231330..0x231456 (the console
// values the two updates read and their defaults from that section)
// EVIDENCE: cshell+0x1f5150 (recoil kick), cshell+0x1f3070 (recoil recovery), cshell+0x82c00 / cshell+0x845f0
// (aim_mark size and arms), DISASM cshell+0x1a96d6..0x1a9744 (aim_mark console values and their defaults)
// OBSERVED: capture sweep/20261025/spread_recoil.csv and the per-shot shots.csv of its captures
// (PROVEN-CAPTURE: 0x82 bytes of the swept weapons; the replay of tests/spread.rs reproduces 1851 of the 1966 volleys
// of the 2026-10-25 sweep with one tick phase and one stance per burst; 1886 when the byte was read before the
// elapsed steps as the 2009 client did, see `shot`), updates.csv of sweep-5 (0x64 `weapon_f` falls 4 or 5 steps per
// 67 ms update = 64 steps per second)

use std::collections::BTreeMap;

use dc_bute::{ButeDoc, SectionMap};

/// Key names of the weapon table read here (names only; the values are the user's ROM).
// OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>], [Ammo<n>], [Perturb] (names listed by the `weapons` example and
// the client's loaders above)
pub mod key {
    pub const MIN: &str = "MinPerturb";
    pub const MAX: &str = "MaxPerturb";
    pub const DUCK: &str = "DuckPerturb";
    pub const MOVE_DUCK: &str = "MoveDuckPerturb";
    pub const INC: &str = "PerturbIncSpeed";
    pub const DEC: &str = "PerturbDecSpeed";
    pub const RECOIL_BASE: &str = "BaseCamRecoilPitch";
    pub const RECOIL_ACCEL: &str = "BaseCamRecoilAccelPitch";
    pub const RECOIL_LIMIT: &str = "MaxCamRecoilPitch";
    pub const WAG_LIMIT: &str = "MaxCamRecoilWag";
    pub const WAG_LEFT: &str = "BaseCamRecoilWagL";
    pub const WAG_RIGHT: &str = "BaseCamRecoilWagR";
    pub const RANDOM_PITCH: &str = "MaxRandomPitch";
    pub const RANDOM_WAG: &str = "MaxRandomWag";
    pub const BACK_JITTER: &str = "BaseCamBackJitter";
    pub const BACK_RECOVER: &str = "BackJitterRecover";
    pub const TRIPLE_RECOIL_LIMIT: &str = "TripleShotMaxCamRecoilPitch";
    pub const TRIPLE_RECOIL_BASE: &str = "TripleShotBaseCamRecoilPitch";
    pub const NO_AIM_MARK: &str = "DisableCrosshair";
    pub const AMMO_NAME0: &str = "AmmoName0";
    pub const NAME: &str = "Name";
    // ammunition row
    pub const FIRE: &str = "FirePerturb";
    pub const FIRE_DUCK: &str = "FireDuckPerturb";
    pub const FIRE_MOVE: &str = "FireMovePerturb";
    pub const FIRE_MOVE_DUCK: &str = "FireMoveDuckPerturb";
    pub const WALK: &str = "WalkPerturb";
    pub const RUN: &str = "RunPerturb";
    pub const JUMP: &str = "JumpPerturb";
    // the zoomed variants (used while zoomed when the value is not 0)
    // DISASM: cshell+0x175418..0x1754df (ammunition row: ZoomedJumpPerturb +0xc0, ZoomedWalkPerturb +0xc4,
    // ZoomedRunPerturb +0xc8, ZoomedFireMoveDuckPerturb +0xcc, ZoomedFireMovePerturb +0xd0, ZoomedFireDuckPerturb +0xd4,
    // ZoomedFirePerturb +0xd8), cshell+0x17dc51 / 0x17dc70 (weapon row: ZoomedPerturbIncSpeed +0x2e8,
    // ZoomedPerturbDecSpeed +0x2ec; binary default 0, cshell+0x17fc3e)
    pub const ZOOM_JUMP: &str = "ZoomedJumpPerturb";
    pub const ZOOM_WALK: &str = "ZoomedWalkPerturb";
    pub const ZOOM_RUN: &str = "ZoomedRunPerturb";
    pub const ZOOM_FIRE: &str = "ZoomedFirePerturb";
    pub const ZOOM_FIRE_DUCK: &str = "ZoomedFireDuckPerturb";
    pub const ZOOM_FIRE_MOVE: &str = "ZoomedFireMovePerturb";
    pub const ZOOM_FIRE_MOVE_DUCK: &str = "ZoomedFireMoveDuckPerturb";
    pub const ZOOM_INC: &str = "ZoomedPerturbIncSpeed";
    pub const ZOOM_DEC: &str = "ZoomedPerturbDecSpeed";
    pub const DEC_DELAY: &str = "DecDelay";
    pub const JITTER_DELAY: &str = "JitterDecDelay";
    pub const JITTER_WAIT: &str = "JitterWaitRecover";
    pub const JITTER: &str = "JitterRecover";
    // [Perturb] section. The client's firing update reads the console value whose default is the section's
    // "Movement..." pair, the motion update the one from the "Firing..." pair (DISASM cshell+0x231385..0x2313c3,
    // cshell+0x231413..0x231456).
    pub const SECTION: &str = "Perturb";
    pub const FIRE_GAIN: &str = "MovementPerturbIncreaseSpeed";
    pub const FIRE_LOSS: &str = "MovementPerturbDecreaseSpeed";
    pub const MOVE_GAIN: &str = "FiringPerturbIncreaseSpeed";
    pub const MOVE_LOSS: &str = "FiringPerturbDecreaseSpeed";
    pub const ROTATION: &str = "PerturbRotationEffect";
}

/// Step clock constants.
pub mod clock {
    /// Seconds per step: the factor of every rise and fall. The 2009 client also divided the elapsed milliseconds
    /// by it for its step count; DeCombat runs one step per tick (see the module doc).
    // EVIDENCE: cshell+0x2375b0 (fdiv by the f32 at VA 0x37556bbc = 0.0125), cshell+0x232120 / +0x2319b0 (x 0.0125)
    pub const STEP_S: f64 = 0.012_500_000_186_264_515;
    /// Period of the millisecond tick clock the steps and delays are counted on.
    // PUBLIC: the Windows default timer interval, 15.625 ms (64 Hz), the resolution of GetTickCount
    // OBSERVED: sweep-5 updates.csv (the spread falls 4 or 5 steps per 67 ms 0x64 interval, not 5 or 6); the sweep
    // replay of tests/spread.rs fits 1851 of 1966 volleys with this period, one phase per burst (report
    // client-spread.md; a 1 ms tick clock fits far fewer)
    // DC-DEFAULT: CLI-61 (proposed) `client.spread.tick_ms`; one step per tick, no step cap, no forced step
    pub const TICK_MS: f64 = 15.625;
}

/// Code defaults of the ammunition row.
// EVIDENCE: cshell+0x174d30 (DecDelay 0x96, JitterDecDelay 600, JitterWaitRecover 0x3dcccccd, JitterRecover
// 0x3e99999a when the row has no such key)
pub mod row_default {
    pub const DEC_DELAY_MS: u32 = 150;
    pub const JITTER_DELAY_MS: u32 = 600;
    pub const JITTER_WAIT: f32 = 0.1;
    pub const JITTER: f32 = 0.3;
}

/// The turn-rate console value of the motion update.
// EVIDENCE: DISASM cshell+0x231343 (console value registered with default 0x40133333 = 2.3)
pub const FAST_TURN_RATE: f32 = 2.3;

/// Gain of the four rise / fall rates when the ROM has no `[Perturb]` section.
// OBSERVED: sweep 0x82 bytes (PROVEN-CAPTURE): the firing spread of every swept weapon rises by 0.0375 x its
// PerturbIncSpeed per shot = 0.0125 x 3.0
// DC-DEFAULT: CLI-62 (proposed) the same 3.0 for the motion rates (no capture measures them)
pub const FALLBACK_GAIN: f32 = 3.0;

/// The aim_mark console values.
pub mod mark {
    // EVIDENCE: DISASM cshell+0x1a96dc (CrossHair_DefaultLength 7.0), +0x1a96f9 (CrossHair_FiringOffset 4.0),
    // +0x1a9715 (CrossHair_FiringAddLength 6.0), +0x1a9732 (CrossHair_FiringDuration 0.1)
    pub const LENGTH: f32 = 7.0;
    pub const FIRING_OFFSET: f32 = 4.0;
    pub const FIRING_ADD_LENGTH: f32 = 6.0;
    pub const FIRING_S: f32 = 0.1;
    /// Width of the virtual screen the aim_mark scale divides by.
    // EVIDENCE: cshell+0x845f0 (the scale is a screen value read as width / 1024; at 16:10 it cannot be told from
    // height / 640)
    // OBSERVED (PLAUSIBLE, not proven by the screenshots): xhair-frozen / xhair2-frozen rest_*.png (1680 x 1050
    // images) show the arms at x 822..832 / 846..856, y 505..517 / 531..543 for the held weapon's MinPerturb, the
    // 14 px inner gap ours draws at 1680 wide. But both driver logs record the 2009 window as `client [1280, 960]`,
    // so the 2009 client did not render at 1680 x 1050 and the images are its 1280 x 960 frame stretched by the
    // wrapper; and the arms are anisotropic (11.7 px long horizontally, 12.9 vertically), which a uniform
    // width / 1024 scale cannot give at either size (a 1280 frame stretched 1.3125 x / 1.094 would be longer
    // horizontally). The scale rule stands on the disassembly; the screenshot match is unexplained.
    pub const VIRTUAL_WIDTH: f32 = 1024.0;
    /// Colour when the weapon row names none.
    // OBSERVED: xhair2-frozen rest_1.png (core pixels (1, 252, 1) over the world)
    // DC-DEFAULT: CLI-65 (proposed) opaque green
    pub const COLOUR: [u8; 4] = [0, 255, 0, 255];
    /// use-items-sweep: the mark's shape bits: the left, right, top and bottom arms, four diagonal arms, the centre
    /// pixel and a ring.
    // EVIDENCE: cshell+0x845f0 (bit 1 left, 2 right, 4 top, 8 bottom arm; 0x10 / 0x40 / 0x80 / 0x20 an arm at a
    // quarter, three quarters, five quarters and seven quarters of the angle constant; 0x100 a 1 x 1 quad at the
    // centre; 0x200 a 32-segment ring turned by a sixteenth of it a step: the constant is pi)
    pub const LEFT: u16 = 1;
    pub const RIGHT: u16 = 2;
    pub const TOP: u16 = 4;
    pub const BOTTOM: u16 = 8;
    pub const DIAGONALS: u16 = 0x10 | 0x20 | 0x40 | 0x80;
    pub const DOT: u16 = 0x100;
    pub const RING: u16 = 0x200;
    /// The plain mark (no reticle item).
    pub const SHAPE_PLAIN: u16 = LEFT | RIGHT | TOP | BOTTOM;
    /// The shapes of a reticle item's `ShapeType` 1..4 (index 0: the plain mark).
    // EVIDENCE: cshell+0x857e0 (the mark's shape list: 0xf, 0x10f, 0x10b, 0x1f0, 0x300), cshell+0x851f0 (shape 1..4 ->
    // its entry, else the first)
    pub const SHAPES: [u16; 5] = [SHAPE_PLAIN, 0x10f, 0x10b, 0x1f0, 0x300];

    /// The shape of `ShapeType` `n`.
    pub fn shape_of(n: i32) -> u16 {
        match n {
            1..=4 => SHAPES[n as usize],
            _ => SHAPE_PLAIN,
        }
    }
}

/// The spread and recoil numbers of one weapon.
#[derive(Debug, Clone, PartialEq)]
pub struct SpreadRow {
    /// Cone bounds (the aim_mark distance and the shot cone), `MinPerturb` / `MaxPerturb` (integers in the row).
    pub min: f32,
    pub max: f32,
    pub duck: f32,
    pub move_duck: f32,
    pub inc: f32,
    pub dec: f32,
    /// Firing spread caps: standing, crouched, moving, moving crouched.
    pub caps: [f32; 4],
    pub walk: f32,
    pub run: f32,
    pub jump: f32,
    /// The zoomed variants: firing caps (same order as `caps`), walk / run / jump limits, rise and fall rates; a 0
    /// keeps the plain value.
    pub zoom_caps: [f32; 4],
    pub zoom_walk: f32,
    pub zoom_run: f32,
    pub zoom_jump: f32,
    pub zoom_inc: f32,
    pub zoom_dec: f32,
    pub dec_delay_ms: u32,
    pub recoil: RecoilRow,
    pub aim_mark: bool,
}

/// Camera recoil numbers of one weapon (angles in degrees as in the row; the two thresholds are raw row values the
/// client compares with radians). [`Default`] is "no row": no recoil at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecoilRow {
    pub base_deg: f32,
    pub accel: f32,
    pub max_deg: f32,
    pub delay_ms: u32,
    pub wait_rate: f32,
    pub rate: f32,
    /// `MaxCamRecoilWag`, `BaseCamRecoilWagL`, `BaseCamRecoilWagR` (degrees).
    pub wag_max_deg: f32,
    pub wag_left_deg: f32,
    pub wag_right_deg: f32,
    /// `MaxRandomPitch`: the yaw kick starts once the pending pitch exceeds it.
    pub pitch_threshold: f32,
    /// `MaxRandomWag`: the yaw kick may change side once the pending yaw exceeds it.
    pub wag_threshold: f32,
    /// `BaseCamBackJitter`, `BackJitterRecover` (the kick-back distance and its fall rate).
    pub back_jitter: f32,
    pub back_recover: f32,
    /// `TripleShotMaxCamRecoilPitch`, `TripleShotBaseCamRecoilPitch` (degrees; used in triple-shot mode when both
    /// are set).
    pub triple_max_deg: f32,
    pub triple_base_deg: f32,
}

impl Default for RecoilRow {
    fn default() -> Self {
        RecoilRow {
            base_deg: 0.0,
            accel: 0.0,
            max_deg: 0.0,
            delay_ms: row_default::JITTER_DELAY_MS,
            wait_rate: row_default::JITTER_WAIT,
            rate: row_default::JITTER,
            wag_max_deg: 0.0,
            wag_left_deg: 0.0,
            wag_right_deg: 0.0,
            pitch_threshold: 0.0,
            wag_threshold: 0.0,
            back_jitter: 0.0,
            back_recover: 0.0,
            triple_max_deg: 0.0,
            triple_base_deg: 0.0,
        }
    }
}

/// Code defaults of the weapon row's recoil keys: what a `[Weapon<n>]` row without the key gets (the row loader reads
/// each key with the field's current value as its default).
// EVIDENCE: cshell+0x17f7b0 (row defaults: +0x2f0 0x41200000, +0x2f4 0x3f99999a, +0x2f8 0, +0x304 0x41f00000, +0x308
// 0x3f800000, +0x30c 0xbf800000, +0x310 0x3d03126f, +0x314 0x3c75c28f, +0x324 0x3f800000, +0x328 0x3f000000, +0x32c
// 0x40400000); DISASM cshell+0x17dd12..0x17def9 (each read loads the field's current value, then pushes the key)
pub mod recoil_default {
    pub const MAX_DEG: f32 = 10.0;
    pub const BASE_DEG: f32 = 1.2;
    pub const ACCEL: f32 = 0.0;
    pub const WAG_MAX_DEG: f32 = 30.0;
    pub const WAG_RIGHT_DEG: f32 = 1.0;
    pub const WAG_LEFT_DEG: f32 = -1.0;
    pub const RANDOM_PITCH: f32 = 0.032;
    pub const RANDOM_WAG: f32 = 0.015;
    pub const TRIPLE_MAX_DEG: f32 = 0.0;
    pub const TRIPLE_BASE_DEG: f32 = 1.0;
    pub const BACK_JITTER: f32 = 0.5;
    pub const BACK_RECOVER: f32 = 3.0;
}

impl Default for SpreadRow {
    /// A weapon without spread (no row): every value 0, the code defaults for the delays.
    fn default() -> Self {
        SpreadRow {
            min: 0.0,
            max: 0.0,
            duck: 0.0,
            move_duck: 0.0,
            inc: 0.0,
            dec: 0.0,
            caps: [0.0; 4],
            walk: 0.0,
            run: 0.0,
            jump: 0.0,
            zoom_caps: [0.0; 4],
            zoom_walk: 0.0,
            zoom_run: 0.0,
            zoom_jump: 0.0,
            zoom_inc: 0.0,
            zoom_dec: 0.0,
            dec_delay_ms: row_default::DEC_DELAY_MS,
            recoil: RecoilRow::default(),
            aim_mark: true,
        }
    }
}

/// The four rate gains (the client's console values; defaults from the `[Perturb]` section).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerturbRates {
    pub fire_gain: f32,
    pub fire_loss: f32,
    pub move_gain: f32,
    pub move_loss: f32,
    pub rotation: f32,
}

impl Default for PerturbRates {
    fn default() -> Self {
        PerturbRates {
            fire_gain: FALLBACK_GAIN,
            fire_loss: FALLBACK_GAIN,
            move_gain: FALLBACK_GAIN,
            move_loss: FALLBACK_GAIN,
            // DC-DEFAULT: CLI-62 (proposed) no view-turn term without the section
            rotation: 0.0,
        }
    }
}

/// Spread rows of every weapon of the ROM and the rate gains.
#[derive(Debug, Clone, Default)]
pub struct SpreadTable {
    pub rows: BTreeMap<u16, SpreadRow>,
    pub rates: PerturbRates,
}

fn get<'a>(sec: &'a SectionMap, k: &str) -> Option<&'a String> {
    sec.iter()
        .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
        .map(|(_, v)| v)
}

fn num(sec: &SectionMap, k: &str) -> Option<f32> {
    let t = get(sec, k)?.trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

fn text(sec: &SectionMap, k: &str) -> Option<String> {
    let t = get(sec, k)?.trim().trim_matches('"').trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn numbered(name: &str, prefix: &str) -> Option<u16> {
    let r = name.strip_prefix(prefix)?;
    (!r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
        .then(|| r.parse().ok())
        .flatten()
}

impl SpreadTable {
    /// The rows of a parsed weapon table. The integer keys are truncated like the client's integer reader.
    // EVIDENCE: cshell+0x17bd20 (MinPerturb / MaxPerturb read as integers; DuckPerturb / MoveDuckPerturb as integers
    // stored as floats)
    pub fn from_doc(doc: &ButeDoc) -> SpreadTable {
        let mut ammo: BTreeMap<String, &SectionMap> = BTreeMap::new();
        for (name, sec) in doc.iter() {
            if numbered(name, "Ammo").is_some() {
                if let Some(n) = text(sec, key::NAME) {
                    ammo.entry(n.to_ascii_lowercase()).or_insert(sec);
                }
            }
        }
        let int = |sec: &SectionMap, k: &str| num(sec, k).map(|v| v.trunc()).unwrap_or(0.0);
        let mut rows = BTreeMap::new();
        for (name, sec) in doc.iter() {
            let Some(n) = numbered(name, "Weapon") else {
                continue;
            };
            let a =
                text(sec, key::AMMO_NAME0).and_then(|a| ammo.get(&a.to_ascii_lowercase()).copied());
            let af = |k: &str| a.and_then(|s| num(s, k)).unwrap_or(0.0);
            let am = |k: &str, d: u32| {
                a.and_then(|s| num(s, k))
                    .map(|v| v.max(0.0) as u32)
                    .unwrap_or(d)
            };
            let ar = |k: &str, d: f32| a.and_then(|s| num(s, k)).unwrap_or(d);
            let wn = |k: &str, d: f32| num(sec, k).unwrap_or(d);
            rows.insert(
                n,
                SpreadRow {
                    min: int(sec, key::MIN),
                    max: int(sec, key::MAX),
                    duck: int(sec, key::DUCK),
                    move_duck: int(sec, key::MOVE_DUCK),
                    inc: num(sec, key::INC).unwrap_or(0.0),
                    dec: num(sec, key::DEC).unwrap_or(0.0),
                    caps: [
                        af(key::FIRE),
                        af(key::FIRE_DUCK),
                        af(key::FIRE_MOVE),
                        af(key::FIRE_MOVE_DUCK),
                    ],
                    walk: af(key::WALK),
                    run: af(key::RUN),
                    jump: af(key::JUMP),
                    zoom_caps: [
                        af(key::ZOOM_FIRE),
                        af(key::ZOOM_FIRE_DUCK),
                        af(key::ZOOM_FIRE_MOVE),
                        af(key::ZOOM_FIRE_MOVE_DUCK),
                    ],
                    zoom_walk: af(key::ZOOM_WALK),
                    zoom_run: af(key::ZOOM_RUN),
                    zoom_jump: af(key::ZOOM_JUMP),
                    zoom_inc: num(sec, key::ZOOM_INC).unwrap_or(0.0),
                    zoom_dec: num(sec, key::ZOOM_DEC).unwrap_or(0.0),
                    dec_delay_ms: am(key::DEC_DELAY, row_default::DEC_DELAY_MS),
                    // DC-DEFAULT: CLI-310 (proposed) a missing recoil key takes the 2009 code default
                    // (`recoil_default`); before this a missing pitch key meant no recoil
                    recoil: RecoilRow {
                        base_deg: wn(key::RECOIL_BASE, recoil_default::BASE_DEG),
                        accel: wn(key::RECOIL_ACCEL, recoil_default::ACCEL),
                        max_deg: wn(key::RECOIL_LIMIT, recoil_default::MAX_DEG),
                        delay_ms: am(key::JITTER_DELAY, row_default::JITTER_DELAY_MS),
                        wait_rate: ar(key::JITTER_WAIT, row_default::JITTER_WAIT),
                        rate: ar(key::JITTER, row_default::JITTER),
                        wag_max_deg: wn(key::WAG_LIMIT, recoil_default::WAG_MAX_DEG),
                        wag_left_deg: wn(key::WAG_LEFT, recoil_default::WAG_LEFT_DEG),
                        wag_right_deg: wn(key::WAG_RIGHT, recoil_default::WAG_RIGHT_DEG),
                        pitch_threshold: wn(key::RANDOM_PITCH, recoil_default::RANDOM_PITCH),
                        wag_threshold: wn(key::RANDOM_WAG, recoil_default::RANDOM_WAG),
                        back_jitter: wn(key::BACK_JITTER, recoil_default::BACK_JITTER),
                        back_recover: wn(key::BACK_RECOVER, recoil_default::BACK_RECOVER),
                        triple_max_deg: wn(
                            key::TRIPLE_RECOIL_LIMIT,
                            recoil_default::TRIPLE_MAX_DEG,
                        ),
                        triple_base_deg: wn(
                            key::TRIPLE_RECOIL_BASE,
                            recoil_default::TRIPLE_BASE_DEG,
                        ),
                    },
                    // DC-DEFAULT: CLI-65 (proposed) a row with DisableCrosshair 1 draws no aim mark (meaning of the key
                    // PLAUSIBLE: it is set on the scoped rifles)
                    aim_mark: num(sec, key::NO_AIM_MARK).is_none_or(|v| v == 0.0),
                },
            );
        }
        let mut rates = PerturbRates::default();
        if let Some(p) = doc.get(key::SECTION) {
            let f = |k: &str, d: f32| num(p, k).unwrap_or(d);
            rates = PerturbRates {
                fire_gain: f(key::FIRE_GAIN, rates.fire_gain),
                fire_loss: f(key::FIRE_LOSS, rates.fire_loss),
                move_gain: f(key::MOVE_GAIN, rates.move_gain),
                move_loss: f(key::MOVE_LOSS, rates.move_loss),
                rotation: f(key::ROTATION, rates.rotation),
            };
        }
        SpreadTable { rows, rates }
    }

    pub fn get(&self, weapon: u16) -> Option<&SpreadRow> {
        self.rows.get(&weapon)
    }
}

/// The player's movement state the spread reads each step.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Motion {
    /// The control word (CROUCH 0x20, RUN 0x200, ANY_MOVE 0x800 are read).
    pub controls: u32,
    /// Airborne after a jump (mover +0x80).
    pub airborne: bool,
    /// Speed over the stance's target speed, 0..1.
    pub speed_frac: f32,
    /// View pitch and yaw (radians) for the turn term.
    pub pitch: f32,
    pub yaw: f32,
    /// The zoom state (level >= 1 or a change running): the zoomed variants of the row apply ([`crate::zoom`]).
    // EVIDENCE: cshell+0x232120 / cshell+0x2319b0 (player manager +0xf0 >= 1 or +0xf4 != 0)
    pub zoomed: bool,
}

impl Motion {
    /// The motion of a simulated player: the speed fraction is `|velocity| / target speed` of the stance.
    // EVIDENCE: cshell+0x1fd610 (|velocity| / cshell+0x1fd390 = the stance's target speed, MOVEMENT_2009.md §7)
    // DC-DEFAULT: CLI-63 (proposed) the target speed is taken from the movement parameters by the control word
    // (crouch, run = the last run speed, else walk); the sprint and ladder cases use the run speed
    pub fn of(
        state: &dc_sim::PlayerState,
        params: &dc_sim::MovementParams,
        yaw: f32,
        pitch: f32,
    ) -> Motion {
        use dc_sim::player::bits;
        let c = state.controls;
        let target = if c & bits::CROUCH != 0 {
            params.crouch_speed
        } else if c & (bits::RUN | bits::SPRINT) != 0 {
            if state.last_run_speed > 0.0 {
                state.last_run_speed
            } else {
                params.run_forward
            }
        } else {
            params.walk_speed
        };
        let v = state.vel;
        let speed = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
        let frac = if target > 0.0 && speed.is_finite() {
            (speed / target).min(1.0)
        } else {
            0.0
        };
        Motion {
            controls: c,
            airborne: state.jumping,
            speed_frac: frac,
            pitch,
            yaw,
            zoomed: false,
        }
    }

    fn crouch(&self) -> bool {
        self.controls & dc_sim::player::bits::CROUCH != 0
    }
    fn run(&self) -> bool {
        self.controls & dc_sim::player::bits::RUN != 0
    }
    fn moving(&self) -> bool {
        self.controls & dc_sim::player::bits::ANY_MOVE != 0
    }
}

/// The number of the tick that holds session time `ms` (tick `j` begins at `j x tick_ms`).
pub fn tick_index(ms: f64, tick_ms: f64) -> u64 {
    if !(ms.is_finite() && ms > 0.0) {
        return 0;
    }
    if tick_ms <= 0.0 {
        return ms as u64;
    }
    (ms / tick_ms).floor() as u64
}

/// The tick clock value (integer milliseconds) of tick number `index`.
pub fn tick_at(index: u64, tick_ms: f64) -> u64 {
    if tick_ms <= 0.0 {
        return index;
    }
    (index as f64 * tick_ms).floor() as u64
}

/// The tick clock value (integer milliseconds) at session time `ms`.
// PUBLIC: GetTickCount advances by the timer interval (15 or 16 ms, 15.625 on average)
pub fn tick_of(ms: f64, tick_ms: f64) -> u64 {
    tick_at(tick_index(ms, tick_ms), tick_ms)
}

/// The spread and recoil state of one weapon.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpreadState {
    /// Firing spread (weapon +0xd0).
    pub firing: f32,
    /// Motion spread (weapon +0xcc).
    pub motion: f32,
    /// Number of the last tick whose step ran.
    last_run: Option<u64>,
    fire_tick: Option<u64>,
    last_view: Option<(f32, f32)>,
    /// When the last shot left (session ms), for the aim mark.
    pub last_shot_ms: Option<f64>,
}

/// What a shot frame gives the 0x82.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotSpread {
    /// The spread before the shot (0..1), `frac = trunc(255 x value)`.
    pub value: f32,
}

/// `frac` of the 0x82.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (`frac` = truncate(a2 x 255.0))
pub fn frac_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0) as u8
}

impl SpreadState {
    /// The spread the weapon reports (slot 0x44): the larger of the two values.
    // EVIDENCE: cshell+0x2f430
    pub fn value(&self) -> f32 {
        self.motion.max(self.firing)
    }

    /// Back to rest (respawn, new weapon row).
    pub fn reset(&mut self) {
        *self = SpreadState::default();
    }

    /// A frame without a shot at session time `now_ms`.
    pub fn frame(
        &mut self,
        row: &SpreadRow,
        rates: &PerturbRates,
        now_ms: f64,
        m: &Motion,
        tick_ms: f64,
    ) {
        self.run(row, rates, now_ms, m, tick_ms);
    }

    /// A shot frame: the steps of the ticks that passed, then the spread before the shot (the 0x82 reads it), then
    /// the fire step (the recoil kick is [`crate::recoil::CameraRecoil::kick`]).
    // DC-DEFAULT: CLI-61 (proposed) the 2009 fire routine read the value before the frame's steps, which made the
    // byte depend on whether a tick boundary fell inside the shot frame; here it is read at the shot tick
    pub fn shot(
        &mut self,
        row: &SpreadRow,
        rates: &PerturbRates,
        now_ms: f64,
        m: &Motion,
        tick_ms: f64,
    ) -> ShotSpread {
        self.run(row, rates, now_ms, m, tick_ms);
        let value = self.value();
        self.fire_step(row, rates, now_ms, m, tick_ms);
        self.last_shot_ms = Some(now_ms);
        ShotSpread { value }
    }

    /// The view change since the last step, split over `steps` steps.
    fn turned(&mut self, m: &Motion, steps: u64) -> f32 {
        match self.last_view.replace((m.pitch, m.yaw)) {
            Some((p, y)) => ((m.pitch - p).abs() + (m.yaw - y).abs()) / steps as f32,
            None => 0.0,
        }
    }

    /// The steps of the ticks that passed since the last step, each at its own tick.
    // The 2009 client ran trunc(elapsed ms x 0.001 / 0.0125) steps per frame (one per tick below 62 ms), at least
    // one on a shot frame and at most 100, all at the frame's tick; DeCombat's count depends on the session clock
    // only (project decision: frame-rate independence, no frame-rate cap).
    // EVIDENCE: DISASM cshell+0x237590..0x23760f (the 2009 count and its cap), cshell+0x237180 (fire state)
    // DC-DEFAULT: CLI-61 (proposed) one step per tick, no cap, no forced step
    fn run(
        &mut self,
        row: &SpreadRow,
        rates: &PerturbRates,
        now_ms: f64,
        m: &Motion,
        tick_ms: f64,
    ) {
        let j = tick_index(now_ms, tick_ms);
        let last = *self.last_run.get_or_insert(j);
        let n = j.saturating_sub(last);
        if n == 0 {
            return;
        }
        let turned = self.turned(m, n);
        for i in 0..n {
            self.motion_step(row, rates, m, turned);
            self.firing_step(row, rates, tick_at(last + i + 1, tick_ms), false, m);
        }
        self.last_run = Some(j);
    }

    /// The fire step of a shot, at the shot tick (after [`SpreadState::run`]).
    fn fire_step(
        &mut self,
        row: &SpreadRow,
        rates: &PerturbRates,
        now_ms: f64,
        m: &Motion,
        tick_ms: f64,
    ) {
        let j = tick_index(now_ms, tick_ms);
        let turned = self.turned(m, 1);
        self.motion_step(row, rates, m, turned);
        self.firing_step(row, rates, tick_at(j, tick_ms), true, m);
        self.last_run = Some(j);
    }

    /// One step of the firing spread.
    // EVIDENCE: cshell+0x232120 (zoomed: the cap of the stance := its Zoomed value and the rise / fall rate := the
    // weapon row's ZoomedPerturbInc / DecSpeed, each when not 0; the three-round variants, the mods' terms and the
    // skill terms are not modelled: skill 0)
    // DC-DEFAULT: CLI-500 the mods' terms of the spread steps are not modelled
    fn firing_step(
        &mut self,
        row: &SpreadRow,
        rates: &PerturbRates,
        tick: u64,
        firing: bool,
        m: &Motion,
    ) {
        if row.max == 0.0 {
            self.firing = 0.0;
            return;
        }
        if !firing {
            if let Some(f) = self.fire_tick {
                if tick.saturating_sub(f) < row.dec_delay_ms as u64 {
                    return;
                }
            }
            self.fire_tick = None;
        }
        let i = match (m.moving(), m.crouch()) {
            (false, false) => 0,
            (false, true) => 1,
            (true, false) => 2,
            (true, true) => 3,
        };
        let cap = zoomed_or(m.zoomed, row.zoom_caps[i], row.caps[i]);
        let (inc, dec) = row.rates(m.zoomed);
        let target = if firing { 1.0 } else { 0.0 };
        let step = clock::STEP_S as f32;
        if target > self.firing {
            self.fire_tick = Some(tick);
            self.firing = (self.firing + step * rates.fire_gain * inc).min(cap);
        } else if target < self.firing {
            self.firing = (self.firing - step * rates.fire_loss * dec).max(0.0);
        }
    }

    /// One step of the motion spread; `turned` is the view change (radians, |pitch| + |yaw|) this step sees.
    // EVIDENCE: cshell+0x2319b0 (zoomed: the walk / run / jump limit := its Zoomed value when not 0, except a
    // running jump, whose zoomed branch reads the plain JumpPerturb; the rates as in the firing step; the always-full
    // case of two input-state tests, the mods' terms and the skill terms are not modelled; the 2009 step read the
    // view change since the last step)
    fn motion_step(&mut self, row: &SpreadRow, rates: &PerturbRates, m: &Motion, turned: f32) {
        if row.max == 0.0 {
            self.motion = 0.0;
            return;
        }
        let limit = match (m.run(), m.airborne) {
            (true, true) => row.jump,
            (false, true) => zoomed_or(m.zoomed, row.zoom_jump, row.jump),
            (false, false) => zoomed_or(m.zoomed, row.zoom_walk, row.walk),
            (true, false) => zoomed_or(m.zoomed, row.zoom_run, row.run),
        };
        let (inc, dec) = row.rates(m.zoomed);
        let speed_target = limit * m.speed_frac.clamp(0.0, 1.0);
        let turn =
            (turned * rates.rotation / (FAST_TURN_RATE * clock::STEP_S as f32 + 2.0)).min(1.0);
        let target = turn.max(speed_target);
        let diff = (target - self.motion).abs();
        let step = clock::STEP_S as f32;
        if self.motion < target {
            let d = diff.min(step * rates.move_gain);
            self.motion = (self.motion + inc * d).min(limit).min(speed_target);
        } else if self.motion > target {
            let d = diff.min(step * rates.move_loss);
            if m.airborne {
                self.motion += d * inc;
            } else {
                self.motion -= d * dec;
            }
            if self.motion > row.jump {
                self.motion = row.jump;
            }
            if self.motion < 0.0 {
                self.motion = 0.0;
            }
        }
    }

    /// The aim_mark the HUD draws from this state at `now_ms`.
    pub fn aim_mark(&self, row: &SpreadRow, m: &Motion, now_ms: f64) -> Option<AimMarkView> {
        if !row.aim_mark {
            return None;
        }
        let since = self
            .last_shot_ms
            .map(|t| ((now_ms - t) / 1000.0) as f32)
            .filter(|s| (0.0..mark::FIRING_S).contains(s));
        Some(AimMarkView {
            shape: mark::SHAPE_PLAIN,
            distance: cone_units(row, m, self.value()),
            since_shot_s: since,
            colour: mark::COLOUR,
            rest: row.min,
        })
    }
}

/// `MinPerturb + spread x (MaxPerturb - MinPerturb)` with the crouched minimum: the aim_mark distance (virtual
/// pixels) and the shot cone (perturb units).
// EVIDENCE: cshell+0x82c00 (MaxPerturb / MinPerturb; while crouched and both duck values are set: DuckPerturb, or
// MoveDuckPerturb with RUN or ANY_MOVE), cshell+0x845f0 (`(max - min) x spread + min`)
pub fn cone_units(row: &SpreadRow, m: &Motion, spread: f32) -> f32 {
    use dc_sim::player::bits;
    let mut min = row.min;
    if row.duck != 0.0 && row.move_duck != 0.0 && m.crouch() {
        min = if m.controls & (bits::RUN | bits::ANY_MOVE) != 0 {
            row.move_duck
        } else {
            row.duck
        };
    }
    min + spread * (row.max - min)
}

/// The zoomed value when zoomed and not 0, else the plain one.
fn zoomed_or(zoomed: bool, z: f32, plain: f32) -> f32 {
    if zoomed && z != 0.0 {
        z
    } else {
        plain
    }
}

impl SpreadRow {
    /// The rise and fall rates (`PerturbIncSpeed` / `PerturbDecSpeed`, their zoomed values while zoomed).
    // EVIDENCE: cshell+0x232120, cshell+0x2319b0 (row +0x2e8 / +0x2ec while zoomed and not 0)
    pub fn rates(&self, zoomed: bool) -> (f32, f32) {
        (
            zoomed_or(zoomed, self.zoom_inc, self.inc),
            zoomed_or(zoomed, self.zoom_dec, self.dec),
        )
    }
}

/// What the HUD needs to draw the aim mark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AimMarkView {
    /// `MinPerturb + spread x (MaxPerturb - MinPerturb)` (virtual pixels of the 1024-wide screen).
    pub distance: f32,
    /// Seconds since the last shot while it is younger than the firing duration.
    pub since_shot_s: Option<f32>,
    pub colour: [u8; 4],
    /// The row's `MinPerturb`: the distance at rest, for a drawn crosshair without recoil (CLI-1633).
    pub rest: f32,
    /// use-items-sweep: the shape bits ([`mark::SHAPES`]; the plain mark without a reticle item, CLI-1878).
    pub shape: u16,
}

/// use-items-sweep: the target pixels `[x0, y0, x1, y1]` (exclusive ends) the mark of `v` covers: the arms of its shape
/// ([`aim_mark_rects`]), the centre pixel, the diagonal arms and the ring.
// EVIDENCE: cshell+0x845f0 (the centre quad at (cx - 1.5, cy - 1.5), 1 x 1; a diagonal arm: the two ends of a
// horizontal arm, from the centre outward, turned by its angle and drawn as a 1-pixel line; the ring: a start point at
// the arms' inner end turned by pi / 16 per segment)
// DC-DEFAULT: CLI-1878 (proposed) a line is walked one pixel per step of its longer axis from the pixel the arms cross;
// the ring's radius is the arms' inner end
pub fn aim_mark_fills(width: u32, height: u32, v: &AimMarkView) -> Vec<[i32; 4]> {
    let mut out = Vec::new();
    let bits = [mark::LEFT, mark::RIGHT, mark::TOP, mark::BOTTOM];
    for (b, r) in bits.iter().zip(aim_mark_rects(width, height, v)) {
        if v.shape & b != 0 {
            let (x0, x1) = covered(r[0], r[2]);
            let (y0, y1) = covered(r[1], r[3]);
            out.push([x0, y0, x1, y1]);
        }
    }
    let (cx, cy) = ((width / 2) as i32 - 1, (height / 2) as i32 - 1);
    let s = width as f32 / mark::VIRTUAL_WIDTH;
    let arms = aim_mark_rects(width, height, v);
    // the right arm's ends relative to the pixel the arms cross
    let inner = arms[1][0] + 0.5 - cx as f32;
    let len = arms[1][2];
    let line = |a: [f32; 2], b: [f32; 2], out: &mut Vec<[i32; 4]>| {
        let n = (b[0] - a[0])
            .abs()
            .max((b[1] - a[1]).abs())
            .round()
            .max(1.0) as i32;
        let mut last = None;
        for k in 0..=n {
            let t = k as f32 / n as f32;
            let x = cx + (a[0] + (b[0] - a[0]) * t).round() as i32;
            let y = cy + (a[1] + (b[1] - a[1]) * t).round() as i32;
            if last != Some((x, y)) {
                out.push([x, y, x + 1, y + 1]);
                last = Some((x, y));
            }
        }
    };
    if v.shape & mark::DIAGONALS != 0 {
        for (bit, q) in [(0x10u16, 0.25f32), (0x40, 0.75), (0x80, 1.25), (0x20, 1.75)] {
            if v.shape & bit != 0 {
                let (sn, cs) = (std::f32::consts::PI * q).sin_cos();
                let at = |r: f32| [r * cs, r * sn];
                line(at(inner), at(inner + len - 1.0), &mut out);
            }
        }
    }
    if v.shape & mark::RING != 0 {
        let r = inner.max(s);
        let step = std::f32::consts::PI / 16.0;
        for k in 0..32 {
            let (a, b) = (k as f32 * step, (k + 1) as f32 * step);
            line(
                [r * a.cos(), r * a.sin()],
                [r * b.cos(), r * b.sin()],
                &mut out,
            );
        }
    }
    if v.shape & mark::DOT != 0 {
        out.push([cx, cy, cx + 1, cy + 1]);
    }
    out
}

/// The four arms in target pixels `(x, y, w, h)` in the 2009 client's 2D coordinates (a pixel `i` is covered when
/// `x <= i < x + w`): left, right, top, bottom.
// EVIDENCE: cshell+0x845f0 (arms: left x = cx + o - e - d - 1 - (l + 1) - 0.5, right x = cx - o + d + 1 - 0.5,
// y = cy - 1 - 0.5, length l + e, thickness 1; top / bottom the same about cy; cx, cy = half the target size)
pub fn aim_mark_rects(width: u32, height: u32, v: &AimMarkView) -> [[f32; 4]; 4] {
    let s = width as f32 / mark::VIRTUAL_WIDTH;
    let cx = (width / 2) as f32;
    let cy = (height / 2) as f32;
    let d = v.distance * s;
    let l = mark::LENGTH * s;
    let (e, o) = match v.since_shot_s {
        Some(t) => {
            let k = 1.0 - (t / mark::FIRING_S).clamp(0.0, 1.0);
            (
                mark::FIRING_ADD_LENGTH * k * s,
                -(mark::FIRING_OFFSET * k * s),
            )
        }
        None => (0.0, 0.0),
    };
    let len = l + e;
    let near = |c: f32| c + o - e - d - 1.0 - (l + 1.0) - 0.5;
    let far = |c: f32| c - o + d + 1.0 - 0.5;
    [
        [near(cx), cy - 1.5, len, 1.0],
        [far(cx), cy - 1.5, len, 1.0],
        [cx - 1.5, near(cy), 1.0, len],
        [cx - 1.5, far(cy), 1.0, len],
    ]
}

/// Pixel span `[first, end)` a 2D rectangle edge pair covers.
pub fn covered(x: f32, w: f32) -> (i32, i32) {
    (x.ceil() as i32, (x + w).ceil() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up row (no ROM value).
    fn row() -> SpreadRow {
        SpreadRow {
            min: 2.0,
            max: 80.0,
            inc: 1.0,
            dec: 0.5,
            caps: [0.2, 0.15, 0.4, 0.3],
            walk: 0.1,
            run: 0.2,
            jump: 0.6,
            dec_delay_ms: 100,
            recoil: RecoilRow {
                base_deg: 2.0,
                accel: 0.0,
                max_deg: 6.0,
                delay_ms: 100,
                wait_rate: 0.1,
                rate: 0.2,
                ..RecoilRow::default()
            },
            ..SpreadRow::default()
        }
    }

    fn rates() -> PerturbRates {
        PerturbRates {
            fire_gain: 2.0,
            fire_loss: 2.0,
            move_gain: 4.0,
            move_loss: 8.0,
            rotation: 0.0,
        }
    }

    #[test]
    fn a_shot_rises_one_step_and_the_byte_is_the_value_before_it() {
        let (r, k) = (row(), rates());
        let mut s = SpreadState::default();
        let m = Motion::default();
        s.frame(&r, &k, 1000.0, &m, clock::TICK_MS);
        let a = s.shot(&r, &k, 1010.0, &m, clock::TICK_MS);
        assert_eq!(a.value, 0.0);
        assert!((s.firing - 0.025).abs() < 1e-6, "{}", s.firing);
        let b = s.shot(&r, &k, 1050.0, &m, clock::TICK_MS);
        assert!((b.value - 0.025).abs() < 1e-6);
        assert_eq!(frac_byte(b.value), 6);
        // capped
        for i in 0..20 {
            s.shot(&r, &k, 1100.0 + 40.0 * i as f64, &m, clock::TICK_MS);
        }
        assert!((s.firing - 0.2).abs() < 1e-6);
        // a shot after a pause with no frame in between reads the fallen value (the elapsed steps run before the
        // byte is read), the same one a state fed a frame every millisecond reads
        let mut twin = s.clone();
        for t in 1861..2200 {
            twin.frame(&r, &k, t as f64, &m, clock::TICK_MS);
        }
        let c = s.shot(&r, &k, 2200.0, &m, clock::TICK_MS);
        let d = twin.shot(&r, &k, 2200.0, &m, clock::TICK_MS);
        assert!(c.value < 0.2 - 1e-6, "{}", c.value);
        assert_eq!(c.value, d.value);
        assert_eq!(s, twin);
    }

    #[test]
    fn zoomed_steps_use_the_zoomed_cap_rates_and_limits_when_set() {
        // made-up zoomed values: the cap 0.05 standing, the rise rate 2 (twice PerturbIncSpeed), a walk limit 0.02
        let r = SpreadRow {
            zoom_caps: [0.05, 0.0, 0.0, 0.0],
            zoom_inc: 2.0,
            zoom_walk: 0.02,
            ..row()
        };
        let k = rates();
        let plain = Motion::default();
        let zoomed = Motion {
            zoomed: true,
            ..Motion::default()
        };
        let mut a = SpreadState::default();
        let mut b = SpreadState::default();
        a.frame(&r, &k, 1000.0, &plain, clock::TICK_MS);
        b.frame(&r, &k, 1000.0, &zoomed, clock::TICK_MS);
        a.shot(&r, &k, 1010.0, &plain, clock::TICK_MS);
        b.shot(&r, &k, 1010.0, &zoomed, clock::TICK_MS);
        assert!((a.firing - 0.025).abs() < 1e-6, "{}", a.firing);
        assert!(
            (b.firing - 0.05).abs() < 1e-6,
            "twice the rise, {}",
            b.firing
        );
        for i in 0..20 {
            a.shot(&r, &k, 1100.0 + 40.0 * i as f64, &plain, clock::TICK_MS);
            b.shot(&r, &k, 1100.0 + 40.0 * i as f64, &zoomed, clock::TICK_MS);
        }
        assert!((a.firing - 0.2).abs() < 1e-6);
        assert!(
            (b.firing - 0.05).abs() < 1e-6,
            "the zoomed cap, {}",
            b.firing
        );
        // a zero zoomed value keeps the plain one (crouched cap, the fall rate)
        assert_eq!(r.rates(true), (2.0, r.dec));
        // walking: the zoomed walk limit
        let walk = |z: bool| Motion {
            controls: dc_sim::player::bits::ANY_MOVE | dc_sim::player::bits::FWD,
            speed_frac: 1.0,
            zoomed: z,
            ..Motion::default()
        };
        let mut c = SpreadState::default();
        let mut d = SpreadState::default();
        for t in 0..2000 {
            c.frame(&r, &k, t as f64, &walk(false), clock::TICK_MS);
            d.frame(&r, &k, t as f64, &walk(true), clock::TICK_MS);
        }
        assert!((c.motion - r.walk).abs() < 1e-5, "{}", c.motion);
        assert!((d.motion - 0.02).abs() < 1e-5, "{}", d.motion);
    }

    /// use-items-sweep (CLI-1878): the reticle shapes' pixels.
    #[test]
    fn reticle_shapes_cover_their_arms_dot_diagonals_and_ring() {
        let v = |shape: u16| AimMarkView {
            shape,
            distance: 10.0,
            since_shot_s: None,
            colour: mark::COLOUR,
            rest: 10.0,
        };
        assert_eq!(
            (mark::shape_of(0), mark::shape_of(2), mark::shape_of(9)),
            (mark::SHAPE_PLAIN, 0x10b, mark::SHAPE_PLAIN)
        );
        // the plain mark: the four arms exactly as before
        let plain = aim_mark_fills(1024, 768, &v(mark::SHAPE_PLAIN));
        let arms: Vec<[i32; 4]> = aim_mark_rects(1024, 768, &v(mark::SHAPE_PLAIN))
            .iter()
            .map(|r| {
                let (x0, x1) = covered(r[0], r[2]);
                let (y0, y1) = covered(r[1], r[3]);
                [x0, y0, x1, y1]
            })
            .collect();
        assert_eq!(plain, arms);
        let (cx, cy) = (511, 383);
        let has = |f: &[[i32; 4]], x: i32, y: i32| {
            f.iter()
                .any(|r| r[0] <= x && x < r[2] && r[1] <= y && y < r[3])
        };
        // shape 2: no top arm, the centre pixel
        let t = aim_mark_fills(1024, 768, &v(mark::shape_of(2)));
        assert_eq!(t.len(), 4);
        assert!(has(&t, cx, cy) && !has(&t, cx, cy - 13) && has(&t, cx, cy + 13));
        // shape 3: the diagonals only, the first pixel at the arms' inner end along each
        let x = aim_mark_fills(1024, 768, &v(mark::shape_of(3)));
        assert!(!has(&x, cx + 12, cy) && has(&x, cx, cy));
        let r = 12.0 / 2f32.sqrt();
        for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
            let (px, py) = (cx + (sx * r).round() as i32, cy + (sy * r).round() as i32);
            assert!(has(&x, px, py), "{sx} {sy}");
        }
        // shape 4: a ring through the arms' inner ends and the centre pixel
        let ring = aim_mark_fills(1024, 768, &v(mark::shape_of(4)));
        assert!(has(&ring, cx + 12, cy) && has(&ring, cx, cy - 12) && has(&ring, cx, cy));
        assert!(!has(&ring, cx + 14, cy) && !has(&ring, cx + 6, cy));
    }

    #[test]
    fn the_fall_waits_for_the_delay_counted_in_ticks_then_one_step_per_tick() {
        let (r, k) = (row(), rates());
        let m = Motion::default();
        let mut s = SpreadState::default();
        s.frame(&r, &k, 0.0, &m, clock::TICK_MS);
        s.shot(&r, &k, 1.0, &m, clock::TICK_MS);
        let after = s.firing;
        // every millisecond a frame: no fall inside 100 ms of ticks
        for t in 2..=93 {
            s.frame(&r, &k, t as f64, &m, clock::TICK_MS);
        }
        assert_eq!(s.firing, after);
        // 7 ticks (109 ms) after the shot tick: one step of 0.0125 x 2 x 0.5
        for t in 94..=110 {
            s.frame(&r, &k, t as f64, &m, clock::TICK_MS);
        }
        assert!((s.firing - (after - 0.0125)).abs() < 1e-6, "{}", s.firing);
        // the frame rate does not matter: one frame per 5 ms gives the same value at 500 ms
        let mut a = SpreadState::default();
        let mut b = SpreadState::default();
        for st in [&mut a, &mut b] {
            st.frame(&r, &k, 0.0, &m, clock::TICK_MS);
            st.shot(&r, &k, 1.0, &m, clock::TICK_MS);
        }
        for t in 2..=500 {
            a.frame(&r, &k, t as f64, &m, clock::TICK_MS);
        }
        for t in (5..=500).step_by(5) {
            b.frame(&r, &k, t as f64, &m, clock::TICK_MS);
        }
        assert_eq!(a.firing, b.firing);
    }

    #[test]
    fn moving_and_crouching_choose_the_cap_and_motion_spread_follows_speed() {
        use dc_sim::player::bits;
        let (r, k) = (row(), rates());
        let mut s = SpreadState::default();
        let run = Motion {
            controls: bits::RUN | bits::ANY_MOVE | bits::FWD,
            speed_frac: 1.0,
            ..Motion::default()
        };
        for t in 0..400 {
            s.frame(&r, &k, t as f64, &run, clock::TICK_MS);
        }
        assert!((s.motion - 0.2).abs() < 1e-6, "{}", s.motion);
        assert!((s.value() - 0.2).abs() < 1e-6);
        // firing while moving: the moving cap 0.4
        for i in 0..40 {
            s.shot(&r, &k, 400.0 + 20.0 * i as f64, &run, clock::TICK_MS);
        }
        assert!((s.firing - 0.4).abs() < 1e-6);
        // stop: the motion value falls to 0
        let stop = Motion::default();
        for t in 1200..2000 {
            s.frame(&r, &k, t as f64, &stop, clock::TICK_MS);
        }
        assert!(s.motion < 1e-6, "{}", s.motion);
        assert!(s.firing < 1e-6, "{}", s.firing);
    }

    #[test]
    fn aim_mark_arms_scale_with_the_width_and_open_with_the_distance() {
        // a made-up distance; the ROM-gated tests of tests/spread.rs hold the screenshot comparison
        let (w, h) = (1680u32, 1050u32);
        let s = w as f32 / mark::VIRTUAL_WIDTH;
        let v = AimMarkView {
            shape: mark::SHAPE_PLAIN,
            distance: 5.0,
            since_shot_s: None,
            colour: mark::COLOUR,
            rest: 5.0,
        };
        let r = aim_mark_rects(w, h, &v);
        // 1-pixel arms of the same length on both axes, left / right on the centre row, top / bottom on the column
        for a in &r[..2] {
            assert_eq!(a[3], 1.0);
            assert_eq!(covered(a[1], a[3]), (524, 525));
        }
        for a in &r[2..] {
            assert_eq!(a[2], 1.0);
            assert_eq!(covered(a[0], a[2]), (839, 840));
        }
        assert_eq!(r[0][2], r[2][3]);
        assert!((r[0][2] - mark::LENGTH * s).abs() < 1e-4);
        // the inner gap (first lit pixel after the centre minus the last lit before it) is the same on both axes
        let gap = |near: [f32; 4], far: [f32; 4], i: usize| {
            covered(far[i], far[i + 2]).0 - (covered(near[i], near[i + 2]).1 - 1)
        };
        assert_eq!(gap(r[0], r[1], 0), gap(r[2], r[3], 1));
        // a larger distance moves both inner edges out by `s` per unit
        let far = aim_mark_rects(
            w,
            h,
            &AimMarkView {
                shape: mark::SHAPE_PLAIN,
                distance: 15.0,
                ..v
            },
        );
        assert!((r[0][0] - far[0][0] - 10.0 * s).abs() < 1e-4);
        assert!((far[1][0] - r[1][0] - 10.0 * s).abs() < 1e-4);
        // a narrower target scales the arms down with its width
        let narrow = aim_mark_rects(w / 2, h / 2, &v);
        assert!((narrow[0][2] - r[0][2] / 2.0).abs() < 1e-4);
        // right after a shot the arms move out by 4 x s and grow by 6 x s, half-way through the duration by half
        let g = aim_mark_rects(
            w,
            h,
            &AimMarkView {
                shape: mark::SHAPE_PLAIN,
                since_shot_s: Some(0.0),
                ..v
            },
        );
        assert!((g[1][0] - r[1][0] - 4.0 * s).abs() < 1e-4);
        assert!((g[1][2] - r[1][2] - 6.0 * s).abs() < 1e-4);
        let half = aim_mark_rects(
            w,
            h,
            &AimMarkView {
                shape: mark::SHAPE_PLAIN,
                since_shot_s: Some(mark::FIRING_S / 2.0),
                ..v
            },
        );
        assert!((half[1][0] - r[1][0] - 2.0 * s).abs() < 1e-4);
    }

    /// Frames at `fps` with shots at `shots` (session ms): the 0x82 bytes and the end state at `end`.
    fn burst_at(fps: f64, shots: &[f64], m: &Motion, end: f64) -> (Vec<u8>, SpreadState) {
        let (r, k) = (row(), rates());
        let mut s = SpreadState::default();
        let period = 1000.0 / fps;
        let mut t = 0.0;
        let mut bytes = Vec::new();
        for &at in shots {
            while t < at {
                s.frame(&r, &k, t, m, clock::TICK_MS);
                t += period;
            }
            bytes.push(frac_byte(s.shot(&r, &k, at, m, clock::TICK_MS).value));
        }
        while t <= end {
            s.frame(&r, &k, t, m, clock::TICK_MS);
            t += period;
        }
        s.frame(&r, &k, end, m, clock::TICK_MS);
        (bytes, s)
    }

    #[test]
    fn the_bytes_do_not_depend_on_the_frame_rate() {
        // project decision: frame-rate independence by design (no frame-rate cap)
        use dc_sim::player::bits;
        // two bursts with falls between and inside them (cadences across the delay), while moving
        let mut shots: Vec<f64> = (0..12).map(|i| 1003.0 + 90.0 * i as f64).collect();
        shots.extend((0..8).map(|i| 2500.0 + 57.0 * i as f64));
        shots.extend((0..6).map(|i| 3100.0 + 160.0 * i as f64));
        let m = Motion {
            controls: bits::RUN | bits::ANY_MOVE | bits::FWD,
            speed_frac: 0.7,
            ..Motion::default()
        };
        let (reference, end) = burst_at(1000.0, &shots, &m, 5000.0);
        assert!(reference.iter().any(|&b| b > 0));
        assert!(reference.windows(2).any(|w| w[1] < w[0]), "{reference:?}");
        for fps in [30.0, 60.0, 144.0, 250.0, 1000.0] {
            let (bytes, s) = burst_at(fps, &shots, &m, 5000.0);
            assert_eq!(bytes, reference, "{fps} fps");
            assert_eq!(s, end, "{fps} fps");
        }
        // standing still the same holds
        let still = Motion::default();
        let (reference, _) = burst_at(1000.0, &shots, &still, 5000.0);
        for fps in [30.0, 60.0, 144.0] {
            assert_eq!(
                burst_at(fps, &shots, &still, 5000.0).0,
                reference,
                "{fps} fps"
            );
        }
    }

    #[test]
    fn a_turning_view_raises_the_motion_spread_no_further_than_the_speed_target() {
        use dc_sim::player::bits;
        let r = row();
        let k = PerturbRates {
            rotation: 1.0,
            ..rates()
        };
        // walking at half speed: the speed target is half of WalkPerturb
        let mut m = Motion {
            controls: bits::ANY_MOVE | bits::FWD,
            speed_frac: 0.5,
            ..Motion::default()
        };
        let speed_target = r.walk * 0.5;
        let mut s = SpreadState::default();
        // the view turns fast (yaw) and the recoil kicks the pitch each frame: the turn target is far above
        for t in 0..64 {
            m.yaw += 0.4;
            m.pitch -= 0.05;
            s.frame(&r, &k, 16.0 * t as f64, &m, clock::TICK_MS);
            assert!(s.motion <= speed_target + 1e-6, "{}", s.motion);
        }
        assert!((s.motion - speed_target).abs() < 1e-6, "{}", s.motion);
        // standing still (speed target 0) the turn alone raises nothing
        let mut still = SpreadState::default();
        let mut mv = Motion::default();
        for t in 0..64 {
            mv.yaw += 0.4;
            still.frame(&r, &k, 16.0 * t as f64, &mv, clock::TICK_MS);
        }
        assert_eq!(still.motion, 0.0);
        // without the turn the same walk rises to the same target (the turn only speeds the rise)
        let mut plain = SpreadState::default();
        let mp = Motion {
            speed_frac: 0.5,
            ..m
        };
        for t in 0..64 {
            plain.frame(&r, &k, 16.0 * t as f64, &mp, clock::TICK_MS);
        }
        assert!(
            (plain.motion - speed_target).abs() < 1e-6,
            "{}",
            plain.motion
        );
    }

    #[test]
    fn ticks_are_counted_from_the_session_clock() {
        assert_eq!(tick_index(0.0, clock::TICK_MS), 0);
        assert_eq!(tick_index(15.624, clock::TICK_MS), 0);
        assert_eq!(tick_index(15.625, clock::TICK_MS), 1);
        assert_eq!(tick_at(1, clock::TICK_MS), 15);
        assert_eq!(tick_at(2, clock::TICK_MS), 31);
        assert_eq!(tick_at(64, clock::TICK_MS), 1000);
        assert_eq!(tick_of(1010.0, clock::TICK_MS), 1000);
        // a 1 ms tick clock: the millisecond itself
        assert_eq!(tick_of(1010.0, 1.0), 1010);
        assert_eq!(tick_of(1010.0, 0.0), 1010);
    }
}
