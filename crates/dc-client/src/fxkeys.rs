//! The effect keys that turn a breakable's death effect into its breaking animation: the anchor key (a moving point
//! with a start velocity, a gravity step and an optional bounce, which other keys can be linked to), the model key (a
//! model file with its skins, drawn while the key lives at the place of the key it is linked to) and the sound key (a
//! wave played once at the key's start). A crate's death group, for example, is five anchors that each carry one
//! linked piece of the crate, a dust puff and the breaking sound; a drum's group launches the drum model upward on an
//! anchor with fire and smoke particles linked to it.
//!
//! Positions of anchors are integrated on a fixed step from the key's start (the session clock), never per drawn
//! frame: the same session time gives the same place at any frame rate.
//!
//! Effect property names are ROM data of the effect module: they are matched by name key, never quoted.
// SPEC: FMT_SPR_LTC_FX.md §5.5 (keys, linked keys), §5.7 (a linked key attaches to the key whose id is its link id),
// §6.3 (types 2 anchor, 3 model, 5 sound)
// EVIDENCE: clientfx+0x1c910 (anchor declarations: gravity default -500, two velocity bounds, the bounce choice),
// clientfx+0x1c770 (anchor parser: gravity +0x5c, minimum velocity +0x60, maximum +0x6c, bounce +0x78)

use dc_fx_data::fxf::{FieldValue, Property};
use dc_render::{name_key, NameKey};
use glam::{Quat, Vec3};

/// Name keys of the anchor, model and sound properties (lower-case names; the names are ROM data).
// EVIDENCE: clientfx+0x1c770 (anchor), clientfx+0x17290 (model: the file and up to fifteen skins), clientfx+0x20ec0
// (sound: file, inner and outer radius, pitch, volume, priority, loop, local)
pub mod prop {
    use dc_render::NameKey;
    pub const GRAVITY: NameKey = ([0xef, 0x2f, 0xed, 0x7e, 0xa6, 0x45, 0x9f, 0xea], 19);
    pub const MIN_VELOCITY: NameKey = ([0x2d, 0x40, 0xc4, 0xc7, 0x38, 0xee, 0x0e, 0x22], 11);
    pub const MAX_VELOCITY: NameKey = ([0x31, 0x31, 0x54, 0xb9, 0x14, 0xee, 0x8a, 0x82], 11);
    pub const BOUNCE: NameKey = ([0x4d, 0x67, 0x23, 0xe5, 0xe4, 0x54, 0x77, 0x9e], 6);
    pub const MODEL: NameKey = ([0x93, 0x72, 0xc4, 0x70, 0xee, 0xad, 0xd5, 0xec], 5);
    /// The fifteen skin slots, in slot order (texture slot `i` of the model).
    // EVIDENCE: clientfx+0x17290 (skin slots 0..14 stored at +0xdc + 0x80 x i)
    pub const SKINS: [NameKey; 15] = [
        ([0x30, 0x54, 0xa7, 0x7f, 0x5b, 0x81, 0xfa, 0xb9], 5),
        ([0xe5, 0xa0, 0xe3, 0xbb, 0x51, 0xda, 0x15, 0xc3], 5),
        ([0x6a, 0x3a, 0x2e, 0xae, 0x35, 0x65, 0xa1, 0xab], 5),
        ([0x70, 0x2c, 0x05, 0x48, 0x57, 0xc7, 0xf3, 0x13], 5),
        ([0xf9, 0xa3, 0x4c, 0xf9, 0xc7, 0xa1, 0x66, 0x26], 5),
        ([0x28, 0xd6, 0x3d, 0xc7, 0xf4, 0x42, 0x68, 0xb9], 5),
        ([0xa0, 0x0f, 0x39, 0x10, 0x9a, 0xbb, 0x79, 0xeb], 5),
        ([0xb2, 0xb3, 0xdc, 0xac, 0x2e, 0xae, 0x2c, 0xd5], 5),
        ([0x38, 0xa0, 0x9e, 0x32, 0x28, 0xf8, 0x6a, 0x16], 5),
        ([0x47, 0xf0, 0x13, 0x93, 0x70, 0x15, 0xdf, 0xc5], 5),
        ([0xe0, 0x52, 0x21, 0x7c, 0xf3, 0x3f, 0x4d, 0x03], 6),
        ([0x3d, 0x4b, 0xca, 0x87, 0x9c, 0x69, 0x50, 0x91], 6),
        ([0xaa, 0x22, 0x71, 0x86, 0x43, 0x3c, 0x86, 0xc3], 6),
        ([0xe6, 0x98, 0x62, 0xe4, 0x01, 0x9c, 0x90, 0xeb], 6),
        ([0x0a, 0xe6, 0x82, 0xac, 0xd6, 0x54, 0x51, 0x02], 6),
    ];
    /// The four render-style slots (style slot `i` of the model's pieces).
    // EVIDENCE: clientfx+0x17290 (render-style slots 0..3 stored at +0x85c + 0x80 x i)
    pub const STYLES: [NameKey; 4] = [
        ([0xd8, 0xd0, 0xd7, 0xeb, 0xf2, 0xdf, 0x25, 0x60], 12),
        ([0x4d, 0xa0, 0x8a, 0xfc, 0xf5, 0xca, 0x5e, 0x79], 12),
        ([0x27, 0xfc, 0xc3, 0x4f, 0xef, 0x97, 0x41, 0x04], 12),
        ([0x43, 0x28, 0x5d, 0x82, 0xf9, 0x4e, 0x64, 0xae], 12),
    ];
    pub const SOUND: NameKey = ([0xdd, 0x29, 0x44, 0x2d, 0xec, 0xa6, 0x9f, 0x52], 5);
    pub const INNER_RADIUS: NameKey = ([0xbd, 0x20, 0x34, 0x8f, 0x17, 0xea, 0xb8, 0x86], 11);
    pub const OUTER_RADIUS: NameKey = ([0x65, 0x72, 0x33, 0x73, 0x6f, 0x1a, 0xf6, 0xc8], 11);
    pub const VOLUME: NameKey = ([0x62, 0xd7, 0xa6, 0xb1, 0x21, 0x1d, 0x62, 0x76], 6);
    pub const LOOP: NameKey = ([0x25, 0x46, 0x37, 0xf7, 0x2e, 0xfc, 0xdd, 0xb6], 4);
    /// The dynamic-light key's two own properties (w2-fx): the random dimming switch and the world-light switch.
    // EVIDENCE: clientfx+0xdae0 (the light type declares exactly these two after the shared twelve), clientfx+0xd7d0
    // (its parser: the first is an int stored as a bool at +0x5c, the second a choice whose index is stored at +0x5d)
    pub const LIGHT_DIM: NameKey = ([0xe1, 0x8d, 0xff, 0x93, 0xda, 0x44, 0x4b, 0x37], 7);
    pub const LIGHT_WORLD: NameKey = ([0x15, 0xce, 0xe1, 0x3c, 0xe0, 0x86, 0xbd, 0x59], 15);
    /// The model key's animation properties (world-fixes2): the length override switch, the length, and the two
    /// synchronisation switches.
    // EVIDENCE: clientfx+0x17290 (model parser: override +0xaf0, length +0xafc, sync-to-model-animation +0xaf4 and
    // sync-to-key +0xaf8, each of the last two clearing the other)
    pub const ANI_OVERRIDE: NameKey = ([0x13, 0xb4, 0x24, 0x36, 0xdb, 0xd7, 0x20, 0x43], 17);
    pub const ANI_LENGTH: NameKey = ([0xeb, 0x35, 0x5f, 0x7d, 0x75, 0x78, 0x63, 0xcf], 9);
    pub const SYNC_TO_KEY: NameKey = ([0xa2, 0x40, 0x31, 0x7d, 0xb1, 0xd6, 0xd0, 0xc4], 9);
    pub const SYNC_TO_MODEL: NameKey = ([0xb3, 0xe4, 0xbc, 0x62, 0xe4, 0xed, 0x90, 0x8f], 15);
}

/// The anchor's gravity default when the key does not set it.
// EVIDENCE: clientfx+0x1c910 (the declared default is the float bits 0xc3fa0000 = -500)
pub const ANCHOR_GRAVITY_DEFAULT: f32 = -500.0;

/// The fraction of its speed an anchor keeps when it bounces.
// EVIDENCE: clientfx+0x1cae0 (the reflected direction is scaled to 0.75 x the old speed)
pub const BOUNCE_KEEP: f32 = 0.75;

/// Anchor updates per second of session time. The 2009 module adds the gravity value to the vertical velocity once
/// per update (per drawn frame) and moves by velocity x the frame time, so its fall depended on the frame rate; DeCombat
/// integrates on this fixed step instead, from the key's start on the session clock, at the 2009 client's measured
/// frame rate (the effects were authored against a client that runs uncapped: at 60 updates per second a shot drum
/// (640..768 units/s up, gravity -2 per update) rose about 1,100 units and never came down within its 2 s key; at this
/// rate it rises v^2 / (2 x 2 x 845) = 121..175 units, lands after about 0.8 s and bounces, as the bounce switch
/// of its key expects).
// EVIDENCE: clientfx+0x1cae0 (per update: position += velocity x dt, then vertical velocity += gravity, with no dt);
// engine+0x17460 (whole-ms frame time), RENDER_2009.md §2 (no working frame cap, present IMMEDIATE); PROVEN-CAPTURE
// 20261025_161846_walkcap0 (the uncapped 2009 client at about 845 frames/s; tdm1/4/6 reference captures 820..920)
// DC-DEFAULT: CLI-600 (proposed; amends CLI-360) 845 updates per second
pub const ANCHOR_HZ: f32 = 845.0;

/// Most anchor steps one key keeps (a key longer than 30 s stops moving at its end).
pub const ANCHOR_MAX_STEPS: usize = 845 * 30;

fn props(list: &[Property]) -> Vec<(NameKey, &FieldValue)> {
    list.iter()
        .map(|p| (name_key(p.name.to_ascii_lowercase().as_bytes()), &p.value))
        .collect()
}

fn find<'a>(p: &[(NameKey, &'a FieldValue)], k: NameKey) -> Option<&'a FieldValue> {
    p.iter().find(|x| x.0 == k).map(|x| x.1)
}

fn f32_of(p: &[(NameKey, &FieldValue)], k: NameKey, d: f32) -> f32 {
    match find(p, k) {
        Some(FieldValue::Float(x)) if x.is_finite() => *x,
        Some(FieldValue::Int(i)) => *i as f32,
        _ => d,
    }
}

fn i32_of(p: &[(NameKey, &FieldValue)], k: NameKey, d: i32) -> i32 {
    match find(p, k) {
        Some(FieldValue::Int(i)) => *i,
        Some(FieldValue::Choice { index, .. }) => *index,
        Some(FieldValue::Float(x)) if x.is_finite() => *x as i32,
        _ => d,
    }
}

fn v3_of(p: &[(NameKey, &FieldValue)], k: NameKey) -> [f32; 3] {
    match find(p, k) {
        Some(FieldValue::Vec3(v)) if v.iter().all(|x| x.is_finite()) => *v,
        _ => [0.0; 3],
    }
}

fn path_of(p: &[(NameKey, &FieldValue)], k: NameKey) -> Option<String> {
    find(p, k)
        .and_then(|v| v.path_parts())
        .and_then(|(_, p)| p)
        .filter(|p| !p.trim().is_empty())
        .map(|p| p.trim().to_string())
}

/// An anchor key: a point that starts at the key's place with a velocity between the two bounds (per axis) and gains
/// `gravity` on its vertical velocity every update; with `bounce` it reflects off the world.
#[derive(Debug, Clone, PartialEq)]
pub struct AnchorKey {
    /// Added to the vertical velocity once per update (units per second, per update).
    pub gravity: f32,
    pub vel_min: [f32; 3],
    pub vel_max: [f32; 3],
    pub bounce: bool,
}

impl AnchorKey {
    // EVIDENCE: clientfx+0x1c770
    pub fn of(list: &[Property]) -> AnchorKey {
        let p = props(list);
        AnchorKey {
            gravity: f32_of(&p, prop::GRAVITY, ANCHOR_GRAVITY_DEFAULT),
            vel_min: v3_of(&p, prop::MIN_VELOCITY),
            vel_max: v3_of(&p, prop::MAX_VELOCITY),
            bounce: i32_of(&p, prop::BOUNCE, 0) != 0,
        }
    }
}

/// A model key: the model file and its skins, and how its animation runs.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelKey {
    pub model: String,
    pub skins: Vec<String>,
    /// The key's render-style files by style slot (empty: none named).
    pub styles: Vec<String>,
    pub anim: ModelAnim,
}

/// How a model key plays its model's animation (world-fixes2).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ModelAnim {
    /// The clip is stretched to `length` seconds (or to the key's life when `length` is 0).
    pub override_len: bool,
    pub length: f32,
    /// The clip time follows the key's time (wrapping at the clip length).
    pub sync_key: bool,
}

impl ModelAnim {
    /// The clip time in ms `elapsed` seconds after the key started, for a key living `life` seconds and a clip of
    /// `clip_ms`. The clip starts with the key and plays once at its own rate, or stretched to the override length;
    /// a key synchronised to its time wraps at the clip's end, any other holds the last frame.
    // EVIDENCE: clientfx+0x17be0 (the key's model object: the clip named by the key or the model's current one, its
    // looping switched off, with the override the rate = clip length / (length, else the key's life)), clientfx+0x177a0
    // (each update with sync-to-key: the clip time set from the key's time, wrapped by fmod)
    // DC-DEFAULT: CLI-599 (proposed) the model's current clip is clip 0; the key's time drives the clip (session clock,
    // frame-rate free); a sync-to-model-animation key plays like an unsynchronised one
    pub fn clip_ms(&self, elapsed: f32, life: f32, clip_ms: u32) -> f32 {
        let len = clip_ms as f32;
        if len <= 0.0 || !elapsed.is_finite() {
            return 0.0;
        }
        let mut rate = 1.0;
        if self.override_len {
            let l = if self.length >= 1e-11 {
                self.length
            } else {
                life
            };
            if l >= 1e-11 {
                rate = len * 0.001 / l;
            }
        }
        let t = elapsed.max(0.0) * 1000.0 * rate;
        if self.sync_key {
            t.rem_euclid(len)
        } else {
            // the pose wraps a time at the clip length: hold just short of it
            t.min(len - 0.01)
        }
    }
}

impl ModelKey {
    // EVIDENCE: clientfx+0x17290 (the model path, the skin paths; a path of three full stops is none, FMT_SPR_LTC_FX
    // §5.6)
    pub fn of(list: &[Property]) -> Option<ModelKey> {
        let p = props(list);
        let model = path_of(&p, prop::MODEL)?;
        let slots = |keys: &[NameKey]| {
            let mut v: Vec<String> = keys
                .iter()
                .map(|k| path_of(&p, *k).unwrap_or_default())
                .collect();
            while v.last().is_some_and(|s| s.is_empty()) {
                v.pop();
            }
            v
        };
        let skins = slots(&prop::SKINS);
        let styles = slots(&prop::STYLES);
        // the two sync switches are read in file order, each clearing the other (clientfx+0x17290)
        let mut sync_key = false;
        for (k, v) in &p {
            let on =
                || matches!(v, FieldValue::Int(i) | FieldValue::Choice { index: i, .. } if *i != 0);
            if *k == prop::SYNC_TO_KEY {
                sync_key = on();
            } else if *k == prop::SYNC_TO_MODEL {
                sync_key = false;
            }
        }
        let anim = ModelAnim {
            override_len: i32_of(&p, prop::ANI_OVERRIDE, 0) != 0,
            length: f32_of(&p, prop::ANI_LENGTH, 0.0),
            sync_key,
        };
        Some(ModelKey {
            model,
            skins,
            styles,
            anim,
        })
    }
}

/// A sound key: the wave, its radii and volume.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundKey {
    pub file: String,
    pub inner: f32,
    pub outer: f32,
    /// 0..100.
    pub volume: u8,
    pub looped: bool,
}

impl SoundKey {
    // EVIDENCE: clientfx+0x20ec0
    pub fn of(list: &[Property]) -> Option<SoundKey> {
        let p = props(list);
        let file = path_of(&p, prop::SOUND)?;
        Some(SoundKey {
            file,
            inner: f32_of(&p, prop::INNER_RADIUS, 0.0).max(0.0),
            outer: f32_of(&p, prop::OUTER_RADIUS, 0.0).max(0.0),
            volume: i32_of(&p, prop::VOLUME, 100).clamp(0, 100) as u8,
            looped: i32_of(&p, prop::LOOP, 0) != 0,
        })
    }

    /// The inner radius as the sound director's fraction of the outer one (x / 255).
    pub fn inner_ratio(&self) -> u8 {
        if self.outer <= 0.0 {
            return 0;
        }
        ((self.inner / self.outer).clamp(0.0, 1.0) * 255.0).round() as u8
    }
}

// --------------------------------------------------------------------------------------------- w2-fx: lights, shake

/// A dynamic-light key (effect type 4): a point light at the key's place while the key runs, its colour from the key's
/// colour keys times their opacity, its radius from the key's scale keys.
// EVIDENCE: clientfx+0xdc00 (create: an engine object of type 4 with the key's place, a colour of (100, 100, 100) until
// the first update, flag bit 1 when the world switch is set), clientfx+0xd970 (update: the shared update fills the
// colour +0x24..+0x2c and opacity +0x30 from the colour keys and the radius +0x34 from the scale keys, then the random
// dimming, then colour x opacity clamped to 0..1 per channel to engine method +0x2f0 and the radius to +0x2f8; a key
// that has ended sets radius 0), clientfx+0x1f70 (the shared update), clientfx+0x1110 / +0x1260 (the key evaluators)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LightKey {
    /// Random dimming each update.
    pub dim: bool,
    /// The world-light switch (lights the world even where the engine would not; not drawn by DeCombat, which does
    /// not light the world with effect lights yet).
    pub world: bool,
}

impl LightKey {
    // EVIDENCE: clientfx+0xd7d0
    pub fn of(list: &[Property]) -> LightKey {
        let p = props(list);
        LightKey {
            dim: i32_of(&p, prop::LIGHT_DIM, 0) != 0,
            world: i32_of(&p, prop::LIGHT_WORLD, 0) != 0,
        }
    }
}

/// The random dimming factor of a light key: `0.3 + 0.19 x u` for a random fraction `u` in 0..1.
// EVIDENCE: clientfx+0xd970 (rand() x 3.051851e-05 x 0.19 + 0.3, multiplied into the three colour channels before the
// opacity)
pub const LIGHT_DIM_BASE: f32 = 0.3;
pub const LIGHT_DIM_SPAN: f32 = 0.19;

/// The light colour (0..255 per channel, the unit of [`dc_render::ModelLight`]) of a light key whose colour keys give
/// `rgb` (0..255) and `opacity` (0..1) now, after the dimming factor `dim` (1 without dimming).
// EVIDENCE: clientfx+0xd970 (per channel: clamp(opacity x colour / 255 x dim, 0, 1)); RENDER_2009.md §6.3 (the engine's
// dynamic light record keeps colour bytes 0..255 and the model lights divide by 255)
pub fn light_colour(rgb: [f32; 3], opacity: f32, dim: f32) -> [f32; 3] {
    rgb.map(|c| ((c / 255.0) * dim * opacity).clamp(0.0, 1.0) * 255.0)
}

/// Updates per second of session time at which DeCombat draws a new random value for an effect's per-update
/// randomness (the light dimming and the camera shake). The 2009 module drew one per drawn frame, so its rate followed
/// the frame rate; DeCombat draws one per step of this clock from the group's seed, the key and the step number, so
/// the same session time gives the same value at any frame rate.
// EVIDENCE: clientfx+0xd970, clientfx+0x3f40 (one rand() per update and value)
// DC-DEFAULT: CLI-513 (proposed) 60 steps per second, as CLI-360's anchor steps
pub const FX_RANDOM_HZ: f32 = 60.0;

/// A random fraction 0..1 of an effect key at session step `step` (`lane` picks one of several values of one step).
pub fn step_unit(seed: u64, key: usize, step: u64, lane: u64) -> f32 {
    let mut r =
        Rng(seed ^ ((key as u64) << 40) ^ step.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ (lane << 8));
    r.unit()
}

/// The step of [`FX_RANDOM_HZ`] that holds `t` seconds after the group start.
pub fn random_step(t: f32) -> u64 {
    if t.is_finite() && t > 0.0 {
        (t * FX_RANDOM_HZ).floor() as u64
    } else {
        0
    }
}

/// A camera-shake key (effect type 6): while the key runs, the camera within the outer radius of the key's place is
/// moved by a random offset of up to the key's scale value per axis, full inside the inner radius and fading linearly
/// in the SQUARED distance to zero at the outer radius.
// EVIDENCE: clientfx+0x3dd0 (declarations: inner and outer radius, defaults 500 and 600), clientfx+0x3bf0 (the
// property object's start values 250000 and 360000, the squares), clientfx+0x3c10 (the parser keeps the squares; an
// outer square not above the inner one becomes inner square + 1), clientfx+0x3f40 (update)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShakeKey {
    pub inner2: f32,
    pub outer2: f32,
}

/// The declared defaults of the shake radii.
// EVIDENCE: clientfx+0x3dd0 (float bits 0x43fa0000 = 500, 0x44160000 = 600)
pub const SHAKE_INNER_DEFAULT: f32 = 500.0;
pub const SHAKE_OUTER_DEFAULT: f32 = 600.0;

impl ShakeKey {
    // EVIDENCE: clientfx+0x3c10
    pub fn of(list: &[Property]) -> ShakeKey {
        let p = props(list);
        let inner = f32_of(&p, prop::INNER_RADIUS, SHAKE_INNER_DEFAULT);
        let outer = f32_of(&p, prop::OUTER_RADIUS, SHAKE_OUTER_DEFAULT);
        ShakeKey::with_radii(inner, outer)
    }

    pub fn with_radii(inner: f32, outer: f32) -> ShakeKey {
        let inner2 = inner * inner;
        let mut outer2 = outer * outer;
        if inner2.partial_cmp(&outer2) != Some(std::cmp::Ordering::Less) {
            outer2 = inner2 + 1.0;
        }
        ShakeKey { inner2, outer2 }
    }

    /// The share of the shake at squared distance `d2` from the key's place: `None` beyond the outer radius, 1 up to
    /// the inner radius, else `1 - (d2 - inner2) / (outer2 - inner2)`.
    // DISASM: clientfx+0x3fc9..0x4040 (squared distance from the camera object's position to the key's place; no shake
    // when it is above the outer square; 1.0 when it is not above the inner square; else the linear form)
    pub fn fade(&self, d2: f32) -> Option<f32> {
        if d2.is_nan() || d2 > self.outer2 {
            return None;
        }
        if d2 <= self.inner2 {
            return Some(1.0);
        }
        Some(1.0 - (d2 - self.inner2) / (self.outer2 - self.inner2))
    }
}

/// The camera offset of one shake key: per axis a random value in -1..1 (`u` = three fractions 0..1) times the fade
/// times the key's scale value now.
// DISASM: clientfx+0x4044..0x40c5 (three rand() values in -1..1 (clientfx+0x3bb0), each x fade x +0x34, added to the
// position of the object +0x10 and written back through engine method +0xc4, the position setter); +0x10 is the
// create record's +0x50 (clientfx+0x1d00), which the shell fills from its effect manager's +0x3c (cshell+0x26d970), set
// by cshell+0x26bfc0 from the interface root's +0x52ec (cshell+0x1fb930), the object whose position and rotation
// cshell+0x1d0f0 writes every frame and cshell+0x28d2c0 reads as the view frame: PLAUSIBLE the camera. Whether the
// shell rewrites the camera before or after the effect update (which decides whether the offset reaches the drawn
// frame) is not traced; DeCombat draws it.
pub fn shake_offset(u: [f32; 3], fade: f32, scale: f32) -> Vec3 {
    Vec3::from(u.map(|x| (x * 2.0 - 1.0) * fade * scale))
}

/// One model an effect draws this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FxModel {
    /// Stable for the life of the key (the group instance and the key index).
    pub id: u64,
    pub model: crate::world::ModelRef,
    /// The key's render-style files by style slot (empty: the styles the tables give the model file).
    pub styles: Vec<String>,
    pub pos: Vec3,
    pub rot: Quat,
    /// How the key plays the model's clip, the seconds since the key started and the key's life (world-fixes2).
    pub anim: ModelAnim,
    pub elapsed: f32,
    pub life: f32,
}

/// A small deterministic random source (the 2009 module uses the C runtime's; only the ranges are kept).
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }
}

/// The fixed-step path of one anchor: its place at each step from the key's start.
#[derive(Debug, Clone, PartialEq)]
pub struct AnchorPath {
    /// Seconds from the group start.
    pub start: f32,
    pub end: f32,
    pub vel: Vec3,
    pub samples: Vec<Vec3>,
}

impl AnchorPath {
    /// The place `t` seconds after the group start (linear between steps; the start place before, the last after).
    pub fn at(&self, t: f32) -> Vec3 {
        let Some(first) = self.samples.first() else {
            return Vec3::ZERO;
        };
        let f = (t - self.start) * ANCHOR_HZ;
        if f <= 0.0 {
            return *first;
        }
        let i = f.floor() as usize;
        if i + 1 >= self.samples.len() {
            return self.samples[self.samples.len() - 1];
        }
        let w = f - i as f32;
        self.samples[i].lerp(self.samples[i + 1], w)
    }
}

/// One update of an anchor: the bounce test on the segment it moves along, the move, the gravity step.
// EVIDENCE: clientfx+0x1cae0 (with bounce: the world segment from the place to place + velocity x dt; on a hit the
// velocity is mirrored at the hit plane, normalised and scaled to 0.75 x its old length; then place += velocity x dt;
// then vertical velocity += gravity)
pub fn anchor_step(
    pos: Vec3,
    vel: Vec3,
    key: &AnchorKey,
    dt: f32,
    ray: &dyn Fn(Vec3, Vec3) -> Option<Vec3>,
) -> (Vec3, Vec3) {
    let mut v = vel;
    if key.bounce {
        if let Some(n) = ray(pos, pos + v * dt) {
            let n = n.normalize_or_zero();
            if n != Vec3::ZERO {
                let speed = v.length();
                let r = v - n * (2.0 * v.dot(n));
                v = r.normalize_or_zero() * (speed * BOUNCE_KEEP);
            }
        }
    }
    let p = pos + v * dt;
    v.y += key.gravity;
    (p, v)
}

/// The motion state of one live group: each key's parent (by link id), the anchor paths, and which sound keys played.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupMotion {
    pub parents: Vec<Option<usize>>,
    pub anchors: Vec<Option<AnchorPath>>,
    pub sounded: Vec<bool>,
}

/// The parts of a key the motion reads.
#[derive(Debug, Clone, Copy)]
pub struct KeyView<'a> {
    pub start: f32,
    pub end: f32,
    pub offset: [f32; 3],
    pub id: u32,
    pub link: Option<u32>,
    pub anchor: Option<&'a AnchorKey>,
}

impl GroupMotion {
    /// The parents of `keys` (a linked key attaches to the key whose id is its link id; itself and unknown ids give
    /// none) and an empty path for every anchor.
    // SPEC: FMT_SPR_LTC_FX.md §5.7; DISASM: cshell+0x26d970 (a linked key's instance takes the object of the key whose id
    // matches as its parent, the group's object otherwise)
    pub fn new(keys: &[KeyView<'_>]) -> GroupMotion {
        let parents = keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let l = k.link?;
                keys.iter().position(|o| o.id == l).filter(|&p| p != i)
            })
            .collect();
        GroupMotion {
            parents,
            anchors: vec![None; keys.len()],
            sounded: vec![false; keys.len()],
        }
    }

    /// The place of key `ki` at `t` seconds after the group start: an anchor's path, else the parent's place (or the
    /// group's) plus the key's offset turned by the group rotation.
    pub fn key_pos(&self, keys: &[KeyView<'_>], ki: usize, t: f32, base: Vec3, rot: Quat) -> Vec3 {
        self.place(keys, ki, t, base, rot, 0)
    }

    fn place(
        &self,
        keys: &[KeyView<'_>],
        ki: usize,
        t: f32,
        base: Vec3,
        rot: Quat,
        depth: u32,
    ) -> Vec3 {
        if let Some(Some(a)) = self.anchors.get(ki) {
            if !a.samples.is_empty() {
                return a.at(t);
            }
        }
        let off = rot * Vec3::from(keys[ki].offset);
        match self.parents.get(ki).copied().flatten() {
            Some(p) if depth < 8 => self.place(keys, p, t, base, rot, depth + 1) + off,
            _ => base + off,
        }
    }

    /// The place an anchor starts from: its parent's place (or the group's) without its own offset.
    // EVIDENCE: clientfx+0x1cd50 (the anchor's own place starts at the key's base place +0x38, the offset is not
    // added), clientfx+0x1d00 (the base place: the parent object's position, else the group's)
    fn anchor_origin(
        &self,
        keys: &[KeyView<'_>],
        ki: usize,
        t: f32,
        base: Vec3,
        rot: Quat,
    ) -> Vec3 {
        match self.parents.get(ki).copied().flatten() {
            Some(p) => self.place(keys, p, t, base, rot, 1),
            None => base,
        }
    }

    /// Advances every anchor that has started to `t` seconds after the group start (fixed steps of 1 / ANCHOR_HZ,
    /// never past the key's end). `seed` picks each anchor's start velocity; `ray` is the world segment test (the hit
    /// plane's normal).
    // EVIDENCE: clientfx+0x1cd50 (the start velocity per axis = min + (max - min) x a random fraction; with a zero
    // direction in the create record the velocity is not turned, so it is in world axes)
    pub fn advance(
        &mut self,
        keys: &[KeyView<'_>],
        t: f32,
        base: Vec3,
        rot: Quat,
        seed: u64,
        ray: &dyn Fn(Vec3, Vec3) -> Option<Vec3>,
    ) {
        let dt = 1.0 / ANCHOR_HZ;
        for ki in 0..keys.len() {
            let Some(a) = keys[ki].anchor else {
                continue;
            };
            let k = keys[ki];
            if t < k.start {
                continue;
            }
            if self.anchors[ki].is_none() {
                let origin = self.anchor_origin(keys, ki, k.start, base, rot);
                let mut r = Rng(seed ^ ((ki as u64) << 32) ^ 0xa11c_0de5);
                let lo = Vec3::from(a.vel_min);
                let hi = Vec3::from(a.vel_max);
                let vel = Vec3::new(
                    r.range(lo.x, hi.x),
                    r.range(lo.y, hi.y),
                    r.range(lo.z, hi.z),
                );
                self.anchors[ki] = Some(AnchorPath {
                    start: k.start,
                    end: k.end,
                    vel,
                    samples: vec![origin],
                });
            }
            let Some(path) = self.anchors[ki].as_mut() else {
                continue;
            };
            let until = t.min(path.end);
            let want = (((until - path.start) * ANCHOR_HZ).floor().max(0.0) as usize + 1)
                .min(ANCHOR_MAX_STEPS);
            while path.samples.len() < want {
                let p = path.samples[path.samples.len() - 1];
                let (np, nv) = anchor_step(p, path.vel, a, dt, ray);
                if !np.is_finite() || !nv.is_finite() {
                    break;
                }
                path.samples.push(np);
                path.vel = nv;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(gravity: f32, bounce: bool, v: [f32; 3]) -> AnchorKey {
        AnchorKey {
            gravity,
            vel_min: v,
            vel_max: v,
            bounce,
        }
    }

    fn view(a: Option<&AnchorKey>, id: u32, link: Option<u32>, offset: [f32; 3]) -> KeyView<'_> {
        KeyView {
            start: 0.0,
            end: 2.0,
            offset,
            id,
            link,
            anchor: a,
        }
    }

    #[test]
    fn gravity_is_added_per_update_and_the_move_uses_the_old_velocity() {
        let k = anchor(-2.0, false, [0.0; 3]);
        let none = |_: Vec3, _: Vec3| None;
        let (p, v) = anchor_step(Vec3::ZERO, Vec3::new(0.0, 60.0, 0.0), &k, 0.5, &none);
        assert_eq!(p, Vec3::new(0.0, 30.0, 0.0));
        assert_eq!(v, Vec3::new(0.0, 58.0, 0.0));
    }

    #[test]
    fn a_bouncing_anchor_mirrors_at_the_floor_and_keeps_three_quarters_of_its_speed() {
        let k = anchor(0.0, true, [0.0; 3]);
        // a floor at y = 0 below the point
        let floor = |a: Vec3, b: Vec3| (a.y >= 0.0 && b.y < 0.0).then_some(Vec3::Y);
        let (p, v) = anchor_step(
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(3.0, -4.0, 0.0),
            &k,
            1.0,
            &floor,
        );
        assert!((v - Vec3::new(2.25, 3.0, 0.0)).length() < 1e-5, "{v:?}");
        assert!((p - Vec3::new(2.25, 4.0, 0.0)).length() < 1e-5, "{p:?}");
    }

    #[test]
    fn the_path_is_the_same_at_any_frame_rate() {
        let a = anchor(-2.0, true, [10.0, 700.0, -5.0]);
        let keys = [view(Some(&a), 1, None, [0.0, 0.0, 64.0])];
        let floor = |a: Vec3, b: Vec3| (a.y >= 0.0 && b.y < 0.0).then_some(Vec3::Y);
        let base = Vec3::new(5.0, 0.5, 5.0);
        let mut slow = GroupMotion::new(&keys);
        let mut fast = GroupMotion::new(&keys);
        let mut t = 0.0f32;
        while t < 1.9 {
            slow.advance(&keys, t, base, Quat::IDENTITY, 7, &floor);
            t += 1.0 / 30.0;
        }
        let mut t = 0.0f32;
        while t < 1.9 {
            fast.advance(&keys, t, base, Quat::IDENTITY, 7, &floor);
            t += 1.0 / 144.0;
        }
        slow.advance(&keys, 1.9, base, Quat::IDENTITY, 7, &floor);
        fast.advance(&keys, 1.9, base, Quat::IDENTITY, 7, &floor);
        let (ps, pf) = (
            slow.key_pos(&keys, 0, 1.9, base, Quat::IDENTITY),
            fast.key_pos(&keys, 0, 1.9, base, Quat::IDENTITY),
        );
        assert_eq!(ps, pf);
        // it started at the group place (no offset), rose and came down
        assert_eq!(slow.key_pos(&keys, 0, 0.0, base, Quat::IDENTITY), base);
        let top = slow.anchors[0]
            .as_ref()
            .unwrap()
            .samples
            .iter()
            .map(|p| p.y)
            .fold(f32::MIN, f32::max);
        let want = 700.0f32 * 700.0 / (2.0 * 2.0 * ANCHOR_HZ) + 0.5;
        assert!(
            (top - want).abs() < 0.05 * want,
            "rose to {top}, want {want}"
        );
    }

    /// world-fixes2 bug 2 ("the barrel goes way higher on explosion than in C++"): the drum's anchor (vertical start
    /// speed 640..768 units/s, gravity -2 per update) peaks at the 2009 rise `v^2 / (2 |g| R)` with R the 2009
    /// client's update rate, within 5 %, and is back on the floor well inside its 2 s key.
    #[test]
    fn the_drum_anchor_peaks_at_the_2009_rise() {
        for vy in [640.0f32, 704.0, 768.0] {
            let a = AnchorKey {
                gravity: -2.0,
                vel_min: [0.0, vy, 0.0],
                vel_max: [0.0, vy, 0.0],
                bounce: true,
            };
            let keys = [KeyView {
                start: 0.0,
                end: 2.0,
                offset: [0.0; 3],
                id: 1,
                link: None,
                anchor: Some(&a),
            }];
            let floor = |a: Vec3, b: Vec3| (a.y >= 0.0 && b.y < 0.0).then_some(Vec3::Y);
            let mut m = GroupMotion::new(&keys);
            m.advance(&keys, 2.0, Vec3::ZERO, Quat::IDENTITY, 1, &floor);
            let s = &m.anchors[0].as_ref().unwrap().samples;
            let top = s.iter().map(|p| p.y).fold(f32::MIN, f32::max);
            // 2009 rate measured on the uncapped client (CLI-600): 845 updates per second
            let want = vy * vy / (2.0 * 2.0 * 845.0);
            assert!(
                (top - want).abs() <= 0.05 * want,
                "v {vy}: peak {top}, 2009 {want}"
            );
            let landed = s.iter().position(|p| p.y < 1.0 && p != &s[0]);
            assert!(
                landed.is_some_and(|i| (i as f32) / ANCHOR_HZ < 1.0),
                "v {vy}: lands within 1 s"
            );
        }
    }

    #[test]
    fn a_linked_key_follows_its_anchor_and_an_unlinked_key_stays() {
        let a = anchor(0.0, false, [60.0, 0.0, 0.0]);
        let keys = [
            view(Some(&a), 10, None, [0.0; 3]),
            view(None, 11, Some(10), [0.0, 1.0, 0.0]),
            view(None, 12, None, [0.0, 2.0, 0.0]),
            view(None, 13, Some(13), [0.0; 3]),
        ];
        let mut m = GroupMotion::new(&keys);
        assert_eq!(m.parents, vec![None, Some(0), None, None]);
        let none = |_: Vec3, _: Vec3| None;
        m.advance(&keys, 1.0, Vec3::ZERO, Quat::IDENTITY, 1, &none);
        let p = m.key_pos(&keys, 1, 1.0, Vec3::ZERO, Quat::IDENTITY);
        assert!((p - Vec3::new(60.0, 1.0, 0.0)).length() < 1e-3, "{p:?}");
        assert_eq!(
            m.key_pos(&keys, 2, 1.0, Vec3::ZERO, Quat::IDENTITY),
            Vec3::new(0.0, 2.0, 0.0)
        );
    }

    #[test]
    fn sound_inner_ratio() {
        let s = SoundKey {
            file: "a.wav".into(),
            inner: 256.0,
            outer: 768.0,
            volume: 100,
            looped: false,
        };
        assert_eq!(s.inner_ratio(), 85);
    }

    // w2-fx: the light and shake keys (constants from clientfx+0xd970, +0x3dd0, +0x3bf0, +0x3c10, +0x3f40)

    #[test]
    fn light_colour_is_opacity_times_colour_clamped_per_channel() {
        // clientfx+0xd970: clamp(opacity x colour / 255 x dim, 0, 1), handed on as 0..255
        assert_eq!(
            light_colour([255.0, 128.0, 0.0], 1.0, 1.0),
            [255.0, 128.0, 0.0]
        );
        let c = light_colour([255.0, 102.0, 51.0], 0.5, 1.0);
        assert!(
            (c[0] - 127.5).abs() < 1e-3 && (c[1] - 51.0).abs() < 1e-3 && (c[2] - 25.5).abs() < 1e-3
        );
        // over-bright keys clamp at 1 (255) per channel, negative opacity at 0
        assert_eq!(light_colour([255.0; 3], 2.0, 1.0), [255.0; 3]);
        assert_eq!(light_colour([255.0; 3], -1.0, 1.0), [0.0; 3]);
        // the dimming factor 0.3 + 0.19 u: between 0.3 and 0.49 of the colour
        assert_eq!((LIGHT_DIM_BASE, LIGHT_DIM_SPAN), (0.3, 0.19));
        let lo = light_colour([200.0; 3], 1.0, LIGHT_DIM_BASE);
        let hi = light_colour([200.0; 3], 1.0, LIGHT_DIM_BASE + LIGHT_DIM_SPAN);
        assert!((lo[0] - 60.0).abs() < 1e-3 && (hi[0] - 98.0).abs() < 1e-3);
    }

    #[test]
    fn light_key_without_properties_has_both_switches_off() {
        assert_eq!(LightKey::of(&[]), LightKey::default());
    }

    #[test]
    fn shake_radii_are_kept_squared_with_the_declared_defaults() {
        // clientfx+0x3dd0: 500 / 600; clientfx+0x3bf0: 250000 / 360000
        let k = ShakeKey::of(&[]);
        assert_eq!((k.inner2, k.outer2), (250_000.0, 360_000.0));
        assert_eq!((SHAKE_INNER_DEFAULT, SHAKE_OUTER_DEFAULT), (500.0, 600.0));
        // clientfx+0x3c10: an outer square not above the inner one becomes inner square + 1
        let k = ShakeKey::with_radii(300.0, 200.0);
        assert_eq!((k.inner2, k.outer2), (90_000.0, 90_001.0));
        let k = ShakeKey::with_radii(100.0, 100.0);
        assert_eq!(k.outer2, 10_001.0);
    }

    #[test]
    fn shake_fades_linearly_in_the_squared_distance() {
        // clientfx+0x3f40: 1 up to the inner square, 1 - (d2 - i2) / (o2 - i2) up to the outer, none beyond
        let k = ShakeKey::with_radii(100.0, 200.0);
        assert_eq!(k.fade(0.0), Some(1.0));
        assert_eq!(k.fade(10_000.0), Some(1.0));
        let f = k.fade(150.0 * 150.0).unwrap();
        assert!((f - (1.0 - 12_500.0 / 30_000.0)).abs() < 1e-6);
        assert_eq!(k.fade(40_000.0), Some(0.0));
        assert_eq!(k.fade(40_000.5), None);
        assert_eq!(k.fade(f32::NAN), None);
    }

    #[test]
    fn shake_offset_spans_minus_to_plus_scale_times_fade() {
        // clientfx+0x3bb0: (b - a) x rand / 32767 + a with a = -1, b = 1; times fade times the scale value
        assert_eq!(
            shake_offset([0.0, 0.5, 1.0], 1.0, 8.0),
            Vec3::new(-8.0, 0.0, 8.0)
        );
        assert_eq!(shake_offset([1.0, 1.0, 1.0], 0.25, 8.0), Vec3::splat(2.0));
        assert_eq!(shake_offset([1.0; 3], 0.0, 8.0), Vec3::ZERO);
    }

    #[test]
    fn effect_randomness_follows_the_session_clock_not_the_frame() {
        // CLI-513: one value per 1/60 s step of the group clock, from the seed, the key and the step
        assert_eq!(FX_RANDOM_HZ, 60.0);
        assert_eq!(random_step(0.0), 0);
        assert_eq!(random_step(-1.0), 0);
        assert_eq!(random_step(f32::NAN), 0);
        assert_eq!(random_step(0.016), 0);
        assert_eq!(random_step(0.017), 1);
        assert_eq!(random_step(1.0), 60);
        let a = step_unit(7, 3, 10, 0);
        assert_eq!(a, step_unit(7, 3, 10, 0));
        assert!((0.0..1.0).contains(&a));
        // another step, key, lane or seed draws another value
        let others = [
            step_unit(7, 3, 11, 0),
            step_unit(7, 4, 10, 0),
            step_unit(7, 3, 10, 1),
            step_unit(8, 3, 10, 0),
        ];
        assert!(others.iter().all(|o| *o != a));
    }

    #[test]
    fn a_model_key_clip_runs_on_the_key_time() {
        // stretched to the key's life: a 10 s clip over a 15 s key reaches its end with the key
        let a = ModelAnim {
            override_len: true,
            length: 0.0,
            sync_key: true,
        };
        assert_eq!(a.clip_ms(0.0, 15.0, 10_000), 0.0);
        assert!((a.clip_ms(7.5, 15.0, 10_000) - 5_000.0).abs() < 0.01);
        // an explicit length wins over the key's life
        let b = ModelAnim { length: 5.0, ..a };
        assert!((b.clip_ms(2.5, 15.0, 10_000) - 5_000.0).abs() < 0.01);
        // own rate, played once: holds its last frame
        let c = ModelAnim::default();
        assert!((c.clip_ms(4.0, 15.0, 10_000) - 4_000.0).abs() < 0.01);
        assert!(c.clip_ms(12.0, 15.0, 10_000) < 10_000.0);
        assert!(c.clip_ms(12.0, 15.0, 10_000) > 9_999.0);
        // the same key time gives the same clip time however it was reached (no per-frame state)
        assert_eq!(a.clip_ms(3.3, 15.0, 10_000), a.clip_ms(3.3, 15.0, 10_000));
        assert_eq!(c.clip_ms(1.0, 15.0, 0), 0.0);
    }
}
