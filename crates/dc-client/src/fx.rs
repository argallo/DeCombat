//! Weapon effects of the 2009 client: the muzzle flash of a shot, tracers, impact effects by surface (sparks, dust,
//! blood on a character), bullet-hole marks, the thrown object of a grenade and its explosion, with the impact and
//! explosion sounds handed to the sound director as one-shot cues.
//!
//! Three ROM sources are read at run time (nothing of them is stored here):
//! - the weapon table: per weapon the first-person and third-person muzzle effect names, per ammunition row the
//!   impact, fire, tracer and projectile rows it names;
//! - the effect table: tracer rows (texture, frequency, speed, width, length, colour, alpha), impact rows (sound,
//!   surface effects, mark), projectile rows (the thrown model);
//! - the surface table: per surface id the impact and grenade effect groups, the bullet-hole sprite, the impact
//!   sounds;
//! - the effect file of the effect archive (FMT_SPR_LTC_FX §5): named effect groups made of timed keys; this module
//!   plays the particle, sprite and spawn-group keys (the behaviour read from the clean decompiler output of the effect
//!   module), other key types are skipped.
//!
//! Effect property names are ROM data of the effect module: they are matched by name key (SHA-256 of the lower-case
//! name, the form `arms` uses for clip names), never quoted.
// SPEC: FMT_SPR_LTC_FX.md §2 (sprites), §5 (the effect file: groups, keys, properties), §6 (the effect types);
// RENDER_2009.md §4.2 / §4.3 (player-view camera and depth range), §8.1 / §8.2 (sprites and particles are camera-facing
// quads, blended, depth-tested unless asked not to); WEAPON_FIRE_2009.md §5.3 (a shot's trace hands its end to the
// impact routine); GAME_MESSAGES_2009_S2C.md §8.2 (the shot effect record and the blast record)

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dc_bute::{ButeDoc, SectionMap};
use dc_fx_data::fxf::{FieldValue, FxKey, Property};
use dc_fx_data::{FxType, FxfFile};
use dc_render::{name_key, NameKey};
use dc_rez::RezVfs;
use glam::{Mat4, Quat, Vec3};

// --------------------------------------------------------------------------------------------- ROM table keys

/// Entries of the attribute archive read here.
// OBSERVED: ROM table entries FX.TXT, SURFACE.TXT, WEAPONS.TXT of ATTRIBUTES.rez
pub const FX_ENTRY: &str = "ATTRIBUTES\\FX.TXT";

/// Keys of the rows this module reads.
// OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] PVMuzzleFXName, HHMuzzleFXName, MuzzlePos; [Ammo<n>] Name,
// ImpactFXName, FireFXName, TracerFXName, ProjectileFXName; FX.TXT [TracerFX<n>] Name, Texture, Frequency, Velocity,
// Width, InitialAlpha, FinalAlpha, Color, Addtive, Length; [ImpactFX<n>] Name, Sound, SoundRadius, DoSurfaceFX,
// CreateMark, MarkScale, IgnoreFlesh; [FireFX<n>] Name, EjectShells, ShellModel, ShellSkin; [ProjectileFX<n>] Name,
// Model, Skin, ModelScale; SURFACE.TXT [Surface<n>] Id, ImpactFXName, GrenadeFXName, ShowsMark, BulletHoleSpr,
// BulletHoleMinScale, BulletHoleMaxScale, BulletImpactSnd<k>, MeleeImpactSnd<k>, HandsImpactSnd<k>,
// ProjectileImpactSnd<k>; [Ammo<n>] Type, InstDamageType; [ImpactFX<n>] FXName, Mark
pub mod key {
    pub const NAME: &str = "Name";
    pub const PV_MUZZLE: &str = "PVMuzzleFXName";
    pub const HH_MUZZLE: &str = "HHMuzzleFXName";
    // OBSERVED: ROM table keys WEAPONS.TXT [Mod<n>] PVMuzzlefxname, HHMuzzlefxname (DISASM cshell+0x177db2 -> mod
    // +0x294, cshell+0x177e09 -> mod +0x298)
    pub const MOD_PV_MUZZLE: &str = "PVMuzzlefxname";
    pub const MOD_HH_MUZZLE: &str = "HHMuzzlefxname";
    pub const MUZZLE_POS: &str = "MuzzlePos";
    pub const IMPACT_FX: &str = "ImpactFXName";
    pub const FIRE_FX: &str = "FireFXName";
    pub const TRACER_FX: &str = "TracerFXName";
    pub const PROJECTILE_FX: &str = "ProjectileFXName";
    pub const TEXTURE: &str = "Texture";
    pub const TRACER_STRIDE: &str = "Frequency";
    pub const VELOCITY: &str = "Velocity";
    pub const WIDTH: &str = "Width";
    pub const ALPHA0: &str = "InitialAlpha";
    pub const ALPHA1: &str = "FinalAlpha";
    pub const COLOUR: &str = "Color";
    pub const ADDITIVE: &str = "Addtive";
    pub const LENGTH: &str = "Length";
    pub const SOUND: &str = "Sound";
    pub const SOUND_RADIUS: &str = "SoundRadius";
    pub const SURFACE_FX: &str = "DoSurfaceFX";
    pub const CREATE_MARK: &str = "CreateMark";
    pub const MARK_SCALE: &str = "MarkScale";
    pub const IGNORE_FLESH: &str = "IgnoreFlesh";
    pub const EJECT_SHELLS: &str = "EjectShells";
    pub const SHELL_MODEL: &str = "ShellModel";
    pub const SHELL_SKIN: &str = "ShellSkin";
    pub const MODEL: &str = "Model";
    pub const SKIN: &str = "Skin";
    pub const MODEL_SCALE: &str = "ModelScale";
    // EVIDENCE: cshell+0x292f90 (projectile row: `FXName` at +0x164, `FXLoop` -> flag bit 1, `FXSmoothShutdown` -> bit 2)
    pub const PROJECTILE_FX_NAME: &str = "FXName";
    pub const FX_LOOP: &str = "FXLoop";
    pub const FX_SMOOTH_SHUTDOWN: &str = "FXSmoothShutdown";
    pub const ID: &str = "Id";
    pub const GRENADE_FX: &str = "GrenadeFXName";
    pub const SHOWS_MARK: &str = "ShowsMark";
    pub const HOLE: &str = "BulletHoleSpr";
    pub const HOLE_MIN: &str = "BulletHoleMinScale";
    pub const HOLE_MAX: &str = "BulletHoleMaxScale";
    pub const BULLET_SOUND: &str = "BulletImpactSnd";
    pub const MELEE_SOUND: &str = "MeleeImpactSnd";
    pub const HANDS_SOUND: &str = "HandsImpactSnd";
    pub const PROJECTILE_SOUND: &str = "ProjectileImpactSnd";
    pub const AMMO_TYPE: &str = "Type";
    pub const DAMAGE_TYPE: &str = "InstDamageType";
    pub const FX_NAME: &str = "FXName";
    pub const MARK: &str = "Mark";
}

/// The impact row's sound value that means "the surface's impact sound" (name key of the lower-case word; the word is
/// ROM data).
// OBSERVED: ROM table key FX.TXT [ImpactFX<n>] Sound (a wave path, or this word in the bullet and melee rows)
pub const SOUND_WORD_SURFACE: NameKey = ([0x76, 0x3c, 0xdc, 0x62, 0xa8, 0x69, 0x26, 0x2b], 7);

fn text(v: Option<&String>) -> Option<String> {
    let t = v?.trim().trim_matches('"').trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn num(v: Option<&String>) -> Option<f32> {
    let t = v?.trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

fn flag(v: Option<&String>) -> bool {
    num(v).is_some_and(|x| x != 0.0)
}

/// A `<x, y, z>` vector value of a table row.
fn vec3(v: Option<&String>) -> Option<[f32; 3]> {
    let t = v?.trim().trim_start_matches('<').trim_end_matches('>');
    let p: Vec<f32> = t
        .split(',')
        .map(|s| s.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .ok()?;
    (p.len() == 3 && p.iter().all(|x| x.is_finite())).then(|| [p[0], p[1], p[2]])
}

fn index_of(section: &str, prefix: &str) -> Option<u16> {
    let rest = section.strip_prefix(prefix)?;
    (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| rest.parse().ok())
        .flatten()
}

fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

// --------------------------------------------------------------------------------------------- the table rows

/// The effect rows an ammunition row names (lower-case names).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AmmoFx {
    pub impact: Option<String>,
    pub fire: Option<String>,
    pub tracer: Option<String>,
    pub projectile: Option<String>,
    /// The ammunition row's `Type` (0 bullets, 1 / 2 projectiles; 2 = thrown) and `InstDamageType`.
    pub ammo_type: u32,
    pub damage_type: u32,
}

/// A tracer row.
#[derive(Debug, Clone, PartialEq)]
pub struct TracerRow {
    pub texture: Option<String>,
    /// One shot in `frequency` draws a tracer.
    pub frequency: u32,
    pub velocity: f32,
    pub width: f32,
    pub alpha: (f32, f32),
    /// 0..1.
    pub colour: [f32; 3],
    pub additive: bool,
    pub length: f32,
}

/// An impact row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImpactRow {
    /// A wave path, or `None` with `surface_sound` for the surface's own impact sound.
    pub sound: Option<String>,
    pub surface_sound: bool,
    pub sound_radius: f32,
    pub surface_fx: bool,
    pub create_mark: bool,
    pub mark_scale: f32,
    pub ignore_flesh: bool,
    /// The effect group the row plays at the impact (lower-case).
    pub fx: Option<String>,
    /// The mark sprite, or `None` with `mark_surface` for the surface's bullet-hole sprite.
    pub mark: Option<String>,
    pub mark_surface: bool,
}

/// A fire row (the shell ejection part).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FireRow {
    pub eject_shells: bool,
    pub shell_model: Option<String>,
    pub shell_skin: Option<String>,
}

/// A projectile row (the look of the thrown object).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectileRow {
    pub model: Option<String>,
    pub skin: Option<String>,
    pub scale: Option<[f32; 3]>,
    /// The effect group attached to the thrown object in flight (lower-case), its loop and smooth-shutdown flags.
    pub fx: Option<String>,
    pub fx_loop: bool,
    pub fx_smooth: bool,
}

/// A surface row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SurfaceRow {
    pub impact_fx: Option<String>,
    pub grenade_fx: Option<String>,
    pub shows_mark: bool,
    pub hole: Option<String>,
    pub hole_scale: (f32, f32),
    pub bullet_sounds: Vec<String>,
    pub melee_sounds: Vec<String>,
    pub hands_sounds: Vec<String>,
    pub projectile_sounds: Vec<String>,
}

/// The weapon effect rows of the ROM.
#[derive(Debug, Clone, Default)]
pub struct FxTables {
    pub ammo: BTreeMap<u16, AmmoFx>,
    /// First-person and third-person muzzle effect group names (lower-case).
    pub muzzle: BTreeMap<u16, (Option<String>, Option<String>)>,
    /// The weapon row's muzzle position (`None` or all zero for a melee weapon).
    pub muzzle_pos: BTreeMap<u16, [f32; 3]>,
    /// Every `[Mod<n>]` row's first-person and third-person muzzle effect names (lower-case; empty: none).
    pub mod_muzzle: BTreeMap<u16, (String, String)>,
    pub tracers: HashMap<String, TracerRow>,
    pub impacts: HashMap<String, ImpactRow>,
    pub fire: HashMap<String, FireRow>,
    pub projectiles: HashMap<String, ProjectileRow>,
    /// By the surface row's `Id`.
    pub surfaces: BTreeMap<u16, SurfaceRow>,
}

fn numbered(sec: &SectionMap, k: &str, from: u32, max: u32) -> Vec<String> {
    (from..from + max)
        .filter_map(|i| text(sec.get(&format!("{k}{i}"))))
        .collect()
}

/// The muzzle effect group of a shot (`third`: another player's, else the own first-person one): with a mod in the
/// weapon's muzzle socket its row's `PVMuzzlefxname` / `HHMuzzlefxname` (empty: no muzzle effect at all), without one
/// the weapon row's `PVMuzzleFXName` / `HHMuzzleFXName`.
// EVIDENCE: cshell+0x234770 (own first-person muzzle: the mod of socket 0 (slot +0x11c(0)), `FlashHiderLevel` 1..9
// picks the row's flash-hider list, else the mod's +0x294 when the pointer is set (the row loader stores a copy of
// "" for a missing key, so any muzzle mod replaces the row's name), else the row's +0xcc; an empty name starts
// nothing); DISASM cshell+0x2c728e..0x2c7337 (another player's shot object: the record's first mod's +0x298, else
// the flash-hider list or the row's +0xd0)
// DC-DEFAULT: CLI-619 the `FlashHiderLevel` branch is not ported (no mod row of the user's ROM has the key)
impl FxTables {
    pub fn muzzle_group(&self, weapon: u16, flash_mod: Option<u16>, third: bool) -> Option<String> {
        if let Some(m) = flash_mod.and_then(|m| self.mod_muzzle.get(&m)) {
            let n = if third { &m.1 } else { &m.0 };
            return (!n.is_empty()).then(|| n.clone());
        }
        let w = self.muzzle.get(&crate::camo::base_weapon(weapon))?;
        if third {
            w.1.clone()
        } else {
            w.0.clone()
        }
    }
}

impl FxTables {
    /// The rows of the three parsed tables.
    pub fn from_docs(
        weapons: Option<&ButeDoc>,
        fx: Option<&ButeDoc>,
        surface: Option<&ButeDoc>,
    ) -> FxTables {
        let mut t = FxTables::default();
        if let Some(w) = weapons {
            for (name, sec) in w.iter() {
                if let Some(n) = index_of(name, "Ammo") {
                    let f = |k: &str| text(sec.get(k)).map(|s| lower(&s));
                    t.ammo.insert(
                        n,
                        AmmoFx {
                            impact: f(key::IMPACT_FX),
                            fire: f(key::FIRE_FX),
                            tracer: f(key::TRACER_FX),
                            projectile: f(key::PROJECTILE_FX),
                            ammo_type: num(sec.get(key::AMMO_TYPE))
                                .map_or(0, |x| x.max(0.0) as u32),
                            damage_type: num(sec.get(key::DAMAGE_TYPE))
                                .map_or(0, |x| x.max(0.0) as u32),
                        },
                    );
                } else if let Some(n) = index_of(name, "Mod") {
                    let f = |k: &str| {
                        let v = sec
                            .iter()
                            .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
                            .map(|(_, v)| v);
                        text(v).map(|s| lower(&s)).unwrap_or_default()
                    };
                    t.mod_muzzle
                        .insert(n, (f(key::MOD_PV_MUZZLE), f(key::MOD_HH_MUZZLE)));
                } else if let Some(n) = index_of(name, "Weapon") {
                    let f = |k: &str| text(sec.get(k)).map(|s| lower(&s));
                    t.muzzle.insert(n, (f(key::PV_MUZZLE), f(key::HH_MUZZLE)));
                    if let Some(p) = vec3(sec.get(key::MUZZLE_POS)) {
                        t.muzzle_pos.insert(n, p);
                    }
                }
            }
        }
        if let Some(fx) = fx {
            for (name, sec) in fx.iter() {
                let Some(nm) = text(sec.get(key::NAME)).map(|s| lower(&s)) else {
                    continue;
                };
                if index_of(name, "TracerFX").is_some() {
                    let c = vec3(sec.get(key::COLOUR)).unwrap_or([255.0; 3]);
                    t.tracers.entry(nm).or_insert(TracerRow {
                        texture: text(sec.get(key::TEXTURE)),
                        frequency: num(sec.get(key::TRACER_STRIDE))
                            .map_or(1, |f| f.max(1.0) as u32),
                        velocity: num(sec.get(key::VELOCITY)).unwrap_or(0.0),
                        width: num(sec.get(key::WIDTH)).unwrap_or(0.0),
                        alpha: (
                            num(sec.get(key::ALPHA0)).unwrap_or(1.0),
                            num(sec.get(key::ALPHA1)).unwrap_or(1.0),
                        ),
                        colour: c.map(|x| (x / 255.0).clamp(0.0, 1.0)),
                        additive: flag(sec.get(key::ADDITIVE)),
                        length: num(sec.get(key::LENGTH)).unwrap_or(0.0),
                    });
                } else if index_of(name, "ImpactFX").is_some() {
                    let snd = text(sec.get(key::SOUND));
                    let surface_sound = snd
                        .as_deref()
                        .is_some_and(|s| name_key(lower(s).as_bytes()) == SOUND_WORD_SURFACE);
                    let mark = text(sec.get(key::MARK));
                    let mark_surface = mark
                        .as_deref()
                        .is_some_and(|s| name_key(lower(s).as_bytes()) == SOUND_WORD_SURFACE);
                    t.impacts.entry(nm).or_insert(ImpactRow {
                        fx: text(sec.get(key::FX_NAME)).map(|s| lower(&s)),
                        mark: mark.filter(|_| !mark_surface),
                        mark_surface,
                        sound: snd.filter(|_| !surface_sound),
                        surface_sound,
                        sound_radius: num(sec.get(key::SOUND_RADIUS)).unwrap_or(0.0),
                        surface_fx: flag(sec.get(key::SURFACE_FX)),
                        create_mark: flag(sec.get(key::CREATE_MARK)),
                        mark_scale: num(sec.get(key::MARK_SCALE)).unwrap_or(0.0),
                        ignore_flesh: flag(sec.get(key::IGNORE_FLESH)),
                    });
                } else if index_of(name, "FireFX").is_some() {
                    t.fire.entry(nm).or_insert(FireRow {
                        eject_shells: flag(sec.get(key::EJECT_SHELLS)),
                        shell_model: text(sec.get(key::SHELL_MODEL)),
                        shell_skin: text(sec.get(key::SHELL_SKIN)),
                    });
                } else if index_of(name, "ProjectileFX").is_some() {
                    t.projectiles.entry(nm).or_insert(ProjectileRow {
                        model: text(sec.get(key::MODEL)),
                        skin: text(sec.get(key::SKIN)),
                        scale: vec3(sec.get(key::MODEL_SCALE)),
                        fx: text(sec.get(key::PROJECTILE_FX_NAME)).map(|s| lower(&s)),
                        fx_loop: flag(sec.get(key::FX_LOOP)),
                        fx_smooth: flag(sec.get(key::FX_SMOOTH_SHUTDOWN)),
                    });
                }
            }
        }
        if let Some(s) = surface {
            for (name, sec) in s.iter() {
                if index_of(name, "Surface").is_none() {
                    continue;
                }
                let Some(id) = num(sec.get(key::ID)).map(|x| x as u16) else {
                    continue;
                };
                let lo = num(sec.get(key::HOLE_MIN)).unwrap_or(1.0);
                let hi = num(sec.get(key::HOLE_MAX)).unwrap_or(lo);
                t.surfaces.entry(id).or_insert(SurfaceRow {
                    impact_fx: text(sec.get(key::IMPACT_FX)).map(|s| lower(&s)),
                    grenade_fx: text(sec.get(key::GRENADE_FX)).map(|s| lower(&s)),
                    shows_mark: flag(sec.get(key::SHOWS_MARK)),
                    hole: text(sec.get(key::HOLE)),
                    hole_scale: (lo, hi),
                    bullet_sounds: numbered(sec, key::BULLET_SOUND, 1, 4),
                    melee_sounds: numbered(sec, key::MELEE_SOUND, 1, 4),
                    hands_sounds: numbered(sec, key::HANDS_SOUND, 1, 4),
                    projectile_sounds: numbered(sec, key::PROJECTILE_SOUND, 1, 4),
                });
            }
        }
        t
    }

    /// Reads the three tables from the user's ROM folder (a missing table leaves its rows empty).
    pub fn open(rom: &Path) -> FxTables {
        let vfs = RezVfs::new(rom);
        let doc = |e: &str| ButeDoc::from_vfs(&vfs, crate::arms::ATTRIBUTES_REZ, e);
        FxTables::from_docs(
            doc(crate::arms::WEAPONS_ENTRY).as_ref(),
            doc(FX_ENTRY).as_ref(),
            doc(crate::sound::SURFACE_ENTRY).as_ref(),
        )
    }

    pub fn ammo_fx(&self, ammo: u16) -> Option<&AmmoFx> {
        self.ammo.get(&ammo)
    }

    pub fn tracer(&self, ammo: u16) -> Option<&TracerRow> {
        self.ammo_fx(ammo)?
            .tracer
            .as_ref()
            .and_then(|n| self.tracers.get(n))
    }

    pub fn impact(&self, ammo: u16) -> Option<&ImpactRow> {
        self.ammo_fx(ammo)?
            .impact
            .as_ref()
            .and_then(|n| self.impacts.get(n))
    }

    /// The thrown objects of this frame's events whose projectile row names an effect and no model (the
    /// flamethrower's flame): the server's create record carries the weapon's own third-person model as a stand-in
    /// (a record needs one model), which is not to be drawn.
    // DC-DEFAULT: CLI-1136 (amended 2026-10-31; playtest report "the flamethrower is shooting flamethrower models") a
    // flown object whose projectile row has no `Model` shows its effect only, no body
    pub fn bodiless_thrown(&self, events: &[FxEvent]) -> BTreeSet<u16> {
        events
            .iter()
            .filter_map(|e| match e {
                FxEvent::Thrown { obj, ammo, .. } => {
                    let row = self.projectile(*ammo)?;
                    (row.model.is_none() && row.fx.is_some()).then_some(*obj)
                }
                _ => None,
            })
            .collect()
    }

    pub fn projectile(&self, ammo: u16) -> Option<&ProjectileRow> {
        self.ammo_fx(ammo)?
            .projectile
            .as_ref()
            .and_then(|n| self.projectiles.get(n))
    }

    /// A melee weapon: its row places the muzzle at the eye (all-zero `MuzzlePos`).
    pub fn melee(&self, weapon: u16) -> bool {
        self.muzzle_pos
            .get(&crate::camo::base_weapon(weapon))
            .is_some_and(|p| p.iter().all(|c| *c == 0.0))
    }
}

// --------------------------------------------------------------------------------------------- the effect file

/// Name keys of the effect properties this module reads (SHA-256 of the lower-case property name).
// SPEC: FMT_SPR_LTC_FX.md §5.6 (property names matched case-insensitively by each type's parser); EVIDENCE:
// clientfx+0x16f0 (the shared parser: placement choice, attach name, offset, colour keys, scale keys), clientfx+0x1d640
// (the particle parser), clientfx+0x23740 (the sprite declarations)
pub mod prop {
    use dc_render::NameKey;
    pub const UPDATE_POS: NameKey = ([0xf0, 0x82, 0xdb, 0x12, 0xe9, 0xad, 0x60, 0xc5], 9);
    pub const ATTACH_NAME: NameKey = ([0x42, 0x9b, 0x8e, 0x2c, 0x3d, 0xde, 0x9d, 0x3d], 10);
    pub const OFFSET: NameKey = ([0xc1, 0xd6, 0x0f, 0xe5, 0x81, 0x5f, 0xb5, 0xb2], 6);
    pub const COLOUR_KEY: NameKey = ([0xd9, 0x3b, 0xec, 0xa6, 0xef, 0xd0, 0x42, 0x1b], 2);
    pub const SCALE_KEY: NameKey = ([0x32, 0xb5, 0x6e, 0xd5, 0x33, 0x48, 0xa8, 0x58], 2);
    pub const SPRITE: NameKey = ([0x4a, 0x04, 0x6e, 0x33, 0xec, 0xf7, 0xac, 0xed], 6);
    pub const TEXTURE: NameKey = ([0x38, 0xa3, 0xc5, 0x0b, 0x7c, 0x1f, 0x96, 0x35], 7);
    pub const EMISSION_INTERVAL: NameKey = ([0xb5, 0x5a, 0xfc, 0x64, 0x2d, 0x77, 0x8d, 0x98], 16);
    pub const PER_EMISSION: NameKey = ([0x59, 0x78, 0x9d, 0x80, 0x8f, 0x49, 0xc8, 0x40], 20);
    pub const GRAVITY: NameKey = ([0xef, 0x2f, 0xed, 0x7e, 0xa6, 0x45, 0x9f, 0xea], 19);
    pub const ACCELERATION: NameKey = ([0x00, 0x1b, 0x59, 0x19, 0x02, 0xee, 0xd3, 0xde], 22);
    pub const MIN_LIFE: NameKey = ([0x1a, 0xb0, 0x3a, 0x0b, 0xd2, 0x61, 0xf9, 0x2f], 19);
    pub const MAX_LIFE: NameKey = ([0xdb, 0x07, 0xed, 0x8a, 0x4c, 0xa7, 0xd5, 0x93], 19);
    pub const MIN_RADIUS: NameKey = ([0xd8, 0x37, 0x7d, 0x53, 0x5f, 0x28, 0x98, 0x17], 9);
    pub const MAX_RADIUS: NameKey = ([0x90, 0xf3, 0x02, 0x8a, 0xc6, 0x4a, 0xf1, 0x1b], 9);
    pub const EMISSION_PLANE: NameKey = ([0x43, 0x85, 0x42, 0xd4, 0x2a, 0x19, 0x2f, 0xfd], 13);
    pub const MIN_VELOCITY: NameKey = ([0x5d, 0x59, 0xd6, 0xfd, 0x80, 0x85, 0x3a, 0xa6], 19);
    pub const MAX_VELOCITY: NameKey = ([0x0c, 0xb7, 0x52, 0x2c, 0x51, 0xc3, 0xa9, 0x2d], 19);
    pub const SHAPE: NameKey = ([0x13, 0x03, 0xc0, 0x6b, 0x0b, 0x01, 0x4d, 0x0c], 4);
    pub const ROTATE: NameKey = ([0xa8, 0x4d, 0xca, 0xa6, 0x65, 0xe5, 0x41, 0x4c], 15);
    pub const MIN_SPIN: NameKey = ([0x25, 0x00, 0xd0, 0xa2, 0xf9, 0xac, 0x33, 0x63], 18);
    pub const MAX_SPIN: NameKey = ([0x94, 0x71, 0x81, 0x49, 0x79, 0xb2, 0x0d, 0x13], 18);
    pub const BLEND: NameKey = ([0x64, 0x67, 0x3b, 0x24, 0xc2, 0xe4, 0x94, 0x07], 9);
    pub const WITH_SYSTEM: NameKey = ([0x4f, 0x29, 0xca, 0x0f, 0x64, 0xaf, 0x9d, 0x3a], 23);
    pub const FRICTION: NameKey = ([0xb0, 0xde, 0x59, 0x8f, 0x2e, 0x8e, 0x47, 0x53], 8);
    pub const VELOCITY_KIND: NameKey = ([0xe7, 0x43, 0x85, 0xd9, 0x79, 0x4c, 0x0e, 0x05], 12);
    pub const NO_DEPTH: NameKey = ([0x30, 0x87, 0x87, 0xf7, 0x84, 0x73, 0x9e, 0x0e], 8);
    pub const FX_NAME: NameKey = ([0xfa, 0x45, 0x3e, 0xb8, 0x01, 0x48, 0x45, 0xbf], 6);
    pub const SHADER: NameKey = ([0xd6, 0x1c, 0xde, 0x4f, 0xfb, 0x5c, 0x26, 0xa0], 14);
    // OBSERVED: ROM effect file keys Facing (choice CameraFacing, AlongNormal, ParentAlign) and Normal of the sprite
    // keys (the indicator groups of the bomb sites: AlongNormal, normal 0 0 1)
    pub const FACING: NameKey = ([0xb3, 0x19, 0xc6, 0x17, 0x74, 0xb0, 0xb7, 0x43], 6);
    pub const NORMAL: NameKey = ([0x31, 0x7b, 0x32, 0xc1, 0x43, 0x69, 0x2b, 0x99], 6);
}

/// Name key of the first-person model's muzzle socket (the rifle model's barrel-tip socket the muzzle group attaches
/// to; the name is ROM data).
// OBSERVED: the first-person muzzle group of the ROM's rifle rows attaches its particles to this socket name, and the
// rifle's first-person model has a socket of that name on its barrel node (2026-10-25)
pub const SOCKET_MUZZLE: NameKey = ([0x85, 0x1e, 0x43, 0xbd, 0x44, 0xa2, 0xc3, 0xd3], 5);

/// Where a key is placed (the shared placement choice).
// EVIDENCE: clientfx+0x16f0 (the choice index is stored at object +0x20); the meanings follow the choice's option list
// in the ROM effect file (fixed, follow, player view, node, socket, player-view node, player-view socket)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Fixed,
    Follow,
    PlayerView,
    Node,
    Socket,
    ViewNode,
    ViewSocket,
}

impl Place {
    fn of(i: i32) -> Place {
        match i {
            1 => Place::Follow,
            2 => Place::PlayerView,
            3 => Place::Node,
            4 => Place::Socket,
            5 => Place::ViewNode,
            6 => Place::ViewSocket,
            _ => Place::Fixed,
        }
    }

    /// Drawn in the player-view (eye) space.
    pub fn in_view(self) -> bool {
        matches!(
            self,
            Place::PlayerView | Place::ViewNode | Place::ViewSocket
        )
    }
}

/// How a quad is blended over the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Blend {
    /// Source alpha over the frame.
    Alpha,
    /// Source colour added (alpha scales it).
    Additive,
    /// Frame colour times source colour.
    Multiply,
}

impl Blend {
    /// The blend-mode choice of a particle or sprite key: index 1 sets the engine's flag 2, index 2 the flag 4, any
    /// other index none.
    // EVIDENCE: clientfx+0x1d640 (BlendMode 1 -> +0x13c = 2, 2 -> +0x13c = 4), clientfx+0x1fdc0 (+0x13c joins the
    // engine object's flags at creation)
    // DC-DEFAULT: CLI-190 (proposed) flag 2 = additive, flag 4 = multiply, none = alpha blend (PLAUSIBLE: the additive
    // reading fits the ROM groups' colour keys that fade to black while the alpha stays opaque)
    pub fn of_choice(i: i32) -> Blend {
        match i {
            1 => Blend::Additive,
            2 => Blend::Multiply,
            _ => Blend::Alpha,
        }
    }
}

/// One colour key: the key time (fraction of the life) and red, green, blue (0..255) and opacity (0..1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColourKey {
    pub t: f32,
    pub rgb: [f32; 3],
    pub alpha: f32,
}

/// The colour at life fraction `t`: linear between the neighbouring keys; the first key before it, the last after.
/// The packed word's low byte is red, then green, blue, and the high byte is the transparency (opacity = 1 - it/255).
// EVIDENCE: clientfx+0x16f0 (a key = t and the four bytes of the packed word, low byte first, as floats),
// clientfx+0x1110 (linear interpolation between the neighbouring keys by t; colour x 1/255; alpha = 1 - byte 4 / 255),
// clientfx+0x1d1c0 (a particle's colour from the keys at its life fraction 1 - remaining / life)
pub fn colour_at(keys: &[ColourKey], t: f32) -> ([f32; 3], f32) {
    let Some(first) = keys.first() else {
        return ([255.0; 3], 1.0);
    };
    if t <= first.t {
        return (first.rgb, first.alpha);
    }
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t < b.t {
            let span = b.t - a.t;
            if span <= 0.0 {
                return (b.rgb, b.alpha);
            }
            let f = (t - a.t) / span;
            let mix = |x: f32, y: f32| x + (y - x) * f;
            return (
                [
                    mix(a.rgb[0], b.rgb[0]),
                    mix(a.rgb[1], b.rgb[1]),
                    mix(a.rgb[2], b.rgb[2]),
                ],
                mix(a.alpha, b.alpha),
            );
        }
    }
    let l = keys[keys.len() - 1];
    (l.rgb, l.alpha)
}

/// The scale at life fraction `t` (linear between neighbouring keys).
// EVIDENCE: clientfx+0x1260 (scale keys: t and value, linear interpolation), clientfx+0x1d290 (a particle's size from
// the keys at its life fraction)
pub fn scale_at(keys: &[(f32, f32)], t: f32) -> f32 {
    let Some(first) = keys.first() else {
        return 1.0;
    };
    if t <= first.0 {
        return first.1;
    }
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t < b.0 {
            let span = b.0 - a.0;
            if span <= 0.0 {
                return b.1;
            }
            return a.1 + (b.1 - a.1) * (t - a.0) / span;
        }
    }
    keys[keys.len() - 1].1
}

/// A particle-system key.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleKey {
    /// The particle texture: the sprite path when set, else the texture path.
    pub texture: Option<String>,
    /// Seconds between emissions (zero or less: one emission at the start).
    pub interval: f32,
    pub per_emission: u32,
    /// Constant acceleration; the gravity value is added to its vertical part.
    pub accel: [f32; 3],
    pub life: (f32, f32),
    pub radius: (f32, f32),
    pub plane: [f32; 3],
    pub vel_min: [f32; 3],
    pub vel_max: [f32; 3],
    /// Emission shape (0 sphere, 1 one point of the plane, 2 plane ring, 3 / 4 cone forms).
    pub shape: i32,
    pub rotate: bool,
    /// Radians per second.
    pub spin: (f32, f32),
    pub blend: Blend,
    pub with_system: bool,
    pub friction: f32,
    /// 0: per-axis velocity between the two bounds; 1: along the emission offset with the speed of the bounds' x.
    pub velocity_kind: i32,
}

/// A sprite key.
#[derive(Debug, Clone, PartialEq)]
pub struct SpriteKey {
    pub sprite: Option<String>,
    pub blend: Blend,
    pub no_depth: bool,
    /// How the quad is turned (CLI-1101): 0 to the camera, 1 across the key's normal turned by the effect's
    /// rotation, 2 in the effect object's own plane.
    pub facing: i32,
    /// The normal of facing 1, in the effect's frame.
    pub normal: [f32; 3],
}

/// The sprite quad's two half-axis directions: camera facing (`right`, `up`), along the key's normal turned by the
/// effect's rotation (the quad lies across it), or the effect object's own x / y axes.
// DC-DEFAULT: CLI-1101 (proposed) the three facing choices of the ROM sprite keys (0 camera, 1 along the normal, 2
// the parent's plane); any other value faces the camera
pub fn sprite_axes(
    facing: i32,
    normal: [f32; 3],
    rot: Quat,
    right: Vec3,
    up: Vec3,
) -> (Vec3, Vec3) {
    match facing {
        1 => {
            let n = Vec3::from(normal);
            if n.length_squared() < 1e-12 || !n.is_finite() {
                return (right, up);
            }
            let (a, b) = plane_axes(n);
            (rot * a, rot * b)
        }
        2 => (rot * Vec3::X, rot * Vec3::Y),
        _ => (right, up),
    }
}

/// What a key does.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyKind {
    Particles(ParticleKey),
    Sprite(SpriteKey),
    /// Starts another group at the same place.
    Spawn(String),
    /// A dynamic light: lights the models near it while the key runs ([`FxRunner::lights_at`], w2-fx).
    Light(crate::fxkeys::LightKey),
    /// A camera shake: moves the camera near it while the key runs ([`FxRunner::shake_at`], w2-fx).
    Shake(crate::fxkeys::ShakeKey),
    /// A moving point other keys link to (client-destructfx, [`crate::fxkeys`]).
    Anchor(crate::fxkeys::AnchorKey),
    /// A model drawn at the key's place ([`crate::fxkeys`]).
    Model(crate::fxkeys::ModelKey),
    /// A wave played once at the key's start ([`crate::fxkeys`]).
    Sound(crate::fxkeys::SoundKey),
    /// Any other type (trail, lightning, ...): skipped.
    Other,
}

/// One timed key of a group.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectKey {
    /// Seconds from the group start.
    pub start: f32,
    pub end: f32,
    pub place: Place,
    /// Name key of the node or socket a key attaches to.
    pub attach: Option<NameKey>,
    pub offset: [f32; 3],
    pub colours: Vec<ColourKey>,
    pub scales: Vec<(f32, f32)>,
    pub kind: KeyKind,
    /// The key's id and, for a linked key, the id of the key it attaches to (FMT_SPR_LTC_FX §5.7).
    pub id: u32,
    pub link: Option<u32>,
}

/// A named effect group.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectGroup {
    pub length_s: f32,
    pub keys: Vec<EffectKey>,
}

/// The properties of one key with their name keys (computed once per key).
struct Props<'a>(Vec<(NameKey, &'a FieldValue)>);

impl<'a> Props<'a> {
    fn of(list: &'a [Property]) -> Props<'a> {
        Props(
            list.iter()
                .map(|p| (name_key(p.name.to_ascii_lowercase().as_bytes()), &p.value))
                .collect(),
        )
    }
    fn find(&self, k: NameKey) -> Option<&'a FieldValue> {
        self.0.iter().find(|p| p.0 == k).map(|p| p.1)
    }
    fn all(&self, k: NameKey) -> impl Iterator<Item = &'a FieldValue> + '_ {
        self.0.iter().filter(move |p| p.0 == k).map(|p| p.1)
    }
    fn f(&self, k: NameKey, d: f32) -> f32 {
        match self.find(k) {
            Some(FieldValue::Float(x)) if x.is_finite() => *x,
            Some(FieldValue::Int(i)) => *i as f32,
            _ => d,
        }
    }
    fn i(&self, k: NameKey, d: i32) -> i32 {
        match self.find(k) {
            Some(FieldValue::Int(i)) => *i,
            Some(FieldValue::Choice { index, .. }) => *index,
            Some(FieldValue::Float(x)) if x.is_finite() => *x as i32,
            _ => d,
        }
    }
    fn v3(&self, k: NameKey, d: [f32; 3]) -> [f32; 3] {
        match self.find(k) {
            Some(FieldValue::Vec3(v)) if v.iter().all(|x| x.is_finite()) => *v,
            _ => d,
        }
    }
    fn path(&self, k: NameKey) -> Option<String> {
        self.find(k)
            .and_then(|v| v.path_parts())
            .and_then(|(_, p)| p)
            .filter(|p| !p.trim().is_empty())
            .map(|p| p.to_string())
    }
    fn string(&self, k: NameKey) -> Option<String> {
        match self.find(k) {
            Some(FieldValue::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => None,
        }
    }
}

impl EffectKey {
    /// The key of an effect file key (`None` for a key whose type is not known).
    pub fn of(k: &FxKey) -> Option<EffectKey> {
        let t = k.effect_type?;
        let p = Props::of(&k.properties);
        // EVIDENCE: clientfx+0x16f0 (the colour and scale key lists of every type)
        let colours = p
            .all(prop::COLOUR_KEY)
            .filter_map(|v| match v {
                FieldValue::ColourKey { t, packed } => Some(ColourKey {
                    t: *t,
                    rgb: [
                        (packed & 0xff) as f32,
                        ((packed >> 8) & 0xff) as f32,
                        ((packed >> 16) & 0xff) as f32,
                    ],
                    alpha: 1.0 - ((packed >> 24) & 0xff) as f32 / 255.0,
                }),
                _ => None,
            })
            .collect();
        let scales = p
            .all(prop::SCALE_KEY)
            .filter_map(|v| match v {
                FieldValue::Vec4(v) if v[0].is_finite() && v[1].is_finite() => Some((v[0], v[1])),
                _ => None,
            })
            .collect();
        // DC-DEFAULT: CLI-198 (proposed) a particle or sprite key with an effect shader (the refraction keys of the
        // explosion groups) is not drawn: their texture is a refraction map for the shader, not a colour image
        // EVIDENCE: clientfx+0x1d640 (the particle parser keeps the shader id at +0x1fc), clientfx+0x1fdc0 (and hands
        // it to the engine object); SPEC: RENDER_2009.md §11.3 (effect shaders are the game module's)
        let shaded = p.i(prop::SHADER, -1) >= 0;
        let kind =
            match t {
                _ if shaded && matches!(t, FxType::Particles | FxType::Sprite) => KeyKind::Other,
                FxType::Particles => {
                    // EVIDENCE: clientfx+0x1d640 (the particle parser: the sprite path wins over the texture path; the
                    // gravity value is added to the vertical acceleration; angular speeds in degrees x pi / 180;
                    // friction clamped to 0..1)
                    let mut accel = p.v3(prop::ACCELERATION, [0.0; 3]);
                    accel[1] += p.f(prop::GRAVITY, 0.0);
                    let deg = std::f32::consts::PI / 180.0;
                    KeyKind::Particles(ParticleKey {
                        texture: p.path(prop::SPRITE).or_else(|| p.path(prop::TEXTURE)),
                        interval: p.f(prop::EMISSION_INTERVAL, 0.0),
                        per_emission: p.i(prop::PER_EMISSION, 1).clamp(0, 256) as u32,
                        accel,
                        life: (p.f(prop::MIN_LIFE, 1.0), p.f(prop::MAX_LIFE, 1.0)),
                        radius: (p.f(prop::MIN_RADIUS, 0.0), p.f(prop::MAX_RADIUS, 0.0)),
                        plane: p.v3(prop::EMISSION_PLANE, [0.0, 0.0, 1.0]),
                        vel_min: p.v3(prop::MIN_VELOCITY, [0.0; 3]),
                        vel_max: p.v3(prop::MAX_VELOCITY, [0.0; 3]),
                        shape: p.i(prop::SHAPE, 0),
                        rotate: p.i(prop::ROTATE, 0) != 0,
                        spin: (
                            p.f(prop::MIN_SPIN, 0.0) * deg,
                            p.f(prop::MAX_SPIN, 0.0) * deg,
                        ),
                        blend: Blend::of_choice(p.i(prop::BLEND, 0)),
                        with_system: p.i(prop::WITH_SYSTEM, 0) != 0,
                        friction: p.f(prop::FRICTION, 1.0).clamp(0.0, 1.0),
                        velocity_kind: p.i(prop::VELOCITY_KIND, 0),
                    })
                }
                FxType::Sprite => KeyKind::Sprite(SpriteKey {
                    sprite: p.path(prop::SPRITE),
                    blend: Blend::of_choice(p.i(prop::BLEND, 0)),
                    no_depth: p.i(prop::NO_DEPTH, 0) != 0,
                    facing: p.i(prop::FACING, 0),
                    normal: p.v3(prop::NORMAL, [0.0, 0.0, 1.0]),
                }),
                FxType::SpawnGroup => match p.string(prop::FX_NAME) {
                    Some(n) => KeyKind::Spawn(lower(&n)),
                    None => KeyKind::Other,
                },
                // w2-fx: the light and shake keys (crate::fxkeys)
                FxType::DynamicLight => KeyKind::Light(crate::fxkeys::LightKey::of(&k.properties)),
                FxType::CameraShake => KeyKind::Shake(crate::fxkeys::ShakeKey::of(&k.properties)),
                // client-destructfx: the breaking animation's keys
                FxType::Empty => KeyKind::Anchor(crate::fxkeys::AnchorKey::of(&k.properties)),
                FxType::Model => crate::fxkeys::ModelKey::of(&k.properties)
                    .map_or(KeyKind::Other, KeyKind::Model),
                FxType::Sound => crate::fxkeys::SoundKey::of(&k.properties)
                    .map_or(KeyKind::Other, KeyKind::Sound),
                _ => KeyKind::Other,
            };
        Some(EffectKey {
            start: k.start,
            end: k.end.max(k.start),
            place: Place::of(p.i(prop::UPDATE_POS, 0)),
            attach: p
                .string(prop::ATTACH_NAME)
                .map(|s| name_key(lower(&s).as_bytes())),
            offset: p.v3(prop::OFFSET, [0.0; 3]),
            colours,
            scales,
            kind,
            id: k.id,
            link: k.linked.then_some(k.link_id),
        })
    }
}

/// The named groups of the ROM effect file(s).
#[derive(Debug, Clone, Default)]
pub struct EffectLibrary {
    groups: HashMap<String, Arc<EffectGroup>>,
}

/// Name key of the effect directory the shell loads `.fxf` files from, upper-cased (archive names compare without
/// case; the literal is ROM data).
// SPEC: FMT_SPR_LTC_FX.md §5.1 (cshell+0x29c7c0 enumerates the directory named by the 8-byte literal
// 85736ee4731ef67b/8 and loads every entry ending in `.fxf`; the same literal upper-cased has the key below)
pub const EFFECT_DIR: NameKey = ([0xae, 0xfd, 0x38, 0xaa, 0xe9, 0x40, 0xb6, 0x3b], 8);

impl EffectLibrary {
    pub fn from_files<'a>(files: impl IntoIterator<Item = &'a FxfFile>) -> EffectLibrary {
        let mut groups = HashMap::new();
        for f in files {
            for g in &f.groups {
                let keys: Vec<EffectKey> = g.keys.iter().filter_map(EffectKey::of).collect();
                groups.entry(lower(&g.name)).or_insert_with(|| {
                    Arc::new(EffectGroup {
                        length_s: g.length_s,
                        keys,
                    })
                });
            }
        }
        EffectLibrary { groups }
    }

    /// The `.fxf` entries of the effect directory in the user's archives.
    pub fn open(rom: &Path) -> EffectLibrary {
        let vfs = RezVfs::new(rom);
        let mut files = Vec::new();
        let mut rez: Vec<PathBuf> = std::fs::read_dir(rom.join("Game"))
            .map(|d| {
                d.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rez")))
                    .collect()
            })
            .unwrap_or_default();
        rez.sort();
        for p in rez {
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Ok(list) = vfs.list_entries(name) else {
                continue;
            };
            for (entry, _, _) in list {
                let up = entry.to_ascii_uppercase();
                let in_dir = up
                    .split(['\\', '/'])
                    .next()
                    .is_some_and(|d| name_key(d.as_bytes()) == EFFECT_DIR);
                if !(in_dir && up.ends_with(".FXF")) {
                    continue;
                }
                let bytes = match vfs.entry_raw(name, &entry) {
                    Ok(Some(b))
                        if dc_fx_data::fxf::detect_encoding(&b) == dc_fx_data::Encoding::Text =>
                    {
                        Some(b)
                    }
                    _ => vfs.entry(name, &entry).ok().flatten(),
                };
                if let Some(f) = bytes.and_then(|b| FxfFile::parse(&b).ok()) {
                    files.push(f);
                }
            }
        }
        EffectLibrary::from_files(files.iter())
    }

    pub fn get(&self, name: &str) -> Option<Arc<EffectGroup>> {
        self.groups.get(&lower(name)).cloned()
    }

    pub fn len(&self) -> usize {
        self.groups.len()
    }

    /// Every group name (lower case), sorted (perf-g1's stress scenarios pick groups by index).
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.groups.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

// --------------------------------------------------------------------------------------------- first-person rig

/// The animation's shift of the first-person rig: the posed translation of the rig root (the child of the model root
/// with the most descendants) minus its bind translation (for reports and tests). The first-person placement no
/// longer takes it out (arms-fixes: CLI-191 retired by CLI-575; the clip offsets of the model files place the rigs).
// DC-DEFAULT: CLI-191 (RETIRED by CLI-575) the rig root's animated translation was not applied to the first-person model.
// OBSERVED: the ROM first-person clip sets of weapons 0 and 7 hold the rig root 4.2 units below / 5.0 units above its
// bind position in every frame, the rifle's (145) at its bind position; with the shift taken out all three idle poses
// span the same eye-space band (2026-10-25, tests/fx_view.rs). The 2009 engine's handling of a root's animated
// translation is not read (EVIDENCE cshell+0x232c10: the game sets the model position to the row's Pos plus the bob
// and recoil terms only, so the engine side must keep the rig in place; PLAUSIBLE)
pub fn view_rig_shift(inst: &dc_render::ModelInstance) -> Vec3 {
    let sk = &inst.anim.skeleton;
    let n = sk.parents.len();
    let mut size = vec![1usize; n];
    for i in (0..n).rev() {
        if let Some(p) = sk.parents[i].filter(|p| *p < i) {
            size[p] += size[i];
        }
    }
    let Some(root) = (0..n).find(|i| sk.parents[*i].is_none()) else {
        return Vec3::ZERO;
    };
    let Some(rig) = (0..n)
        .filter(|i| sk.parents[*i] == Some(root))
        .max_by_key(|i| (size[*i], std::cmp::Reverse(*i)))
    else {
        return Vec3::ZERO;
    };
    match (inst.globals.get(rig), sk.bind_global.get(rig)) {
        (Some(g), Some(b)) => {
            let d = g.w_axis.truncate() - b.w_axis.truncate();
            if d.is_finite() {
                d
            } else {
                Vec3::ZERO
            }
        }
        _ => Vec3::ZERO,
    }
}

/// A `<x, y, z>` vector value of a ROM table row (`None` when absent or malformed).
pub fn row_vec3(v: Option<&String>) -> Option<[f32; 3]> {
    vec3(v)
}

// --------------------------------------------------------------------------------------------- frame inputs

/// One effect-related thing the frame source saw since the last frame.
#[derive(Debug, Clone, PartialEq)]
pub enum FxEvent {
    /// One pellet of the own shot (the local trace).
    Shot(crate::arms::ShotFx),
    /// A server shot-effect record (another player's shot, the echo of an own shot, or the blast of a thrown object).
    Record(ShotRecord),
    /// A placed-effect object the server created (a breakable's death effect, a world effect): the named effect group
    /// plays at the object's position, once or looping by the object's value (w2-fx).
    World(WorldFx),
    /// The placed-effect object with this id left the server's table (w2-fx): its groups stop looping, and with the
    /// value's cut bit they stop at once.
    WorldGone(u16),
    /// A thrown object (a rocket, a grenade, a flame) of the ammunition `ammo` is drawn at `pos` with the object's
    /// rotation `rot` (x, y, z, w) this frame: the first one starts its projectile row's effect group on it in the
    /// object's frame, later ones move the group with it; its removal is a [`FxEvent::WorldGone`].
    Thrown {
        obj: u16,
        ammo: u16,
        pos: [f32; 3],
        rot: [f32; 4],
    },
}

/// Bits of a placed effect's value (the type-0x64 object's u32, the group's create flags).
// EVIDENCE: cshell+0x26de40 (create: bit 0 -> instance +0x4c, bit 1 -> +0x4d = NOT bit 1, bit 2 -> +0x74, the group is
// placed relative to its object), cshell+0x26e4a0 (each frame the group clock advances and wraps at the group length,
// the keys stepped over [previous, length) and then from 0), cshell+0x26d550 (a key that ended is restarted when the
// wrapped clock passes its start again while +0x4c is set and the instance is not shutting down, +0x4e; without +0x4c
// it is done, and the instance is deleted once no key is left), cshell+0x26e650 (the object's removal: +0x4e := 1;
// with +0x4d each running key whose key flag +0x45 is set runs on to its end, the others are removed; without +0x4d
// all are removed at once), cshell+0x26c950; OBSERVED: dc-dserver worldobj.rs effect_blob (the reference stack sent value 1 on
// every placed effect of tdm4 / tdm6, all of them looping)
pub const WORLD_FX_LOOP: u32 = 0x1;
pub const WORLD_FX_CUT: u32 = 0x2;
pub const WORLD_FX_RELATIVE: u32 = 0x4;

/// A placed-effect object handed to the effect player (CLI-234).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.4 (type 0x64 `[name:zstr8][value:u32][has_target:u8]`: the effect name the client
// looks up in its effect groups)
#[derive(Debug, Clone, PartialEq)]
pub struct WorldFx {
    /// The effect group name the object carries (matched without case).
    pub name: String,
    pub pos: [f32; 3],
    /// The object's rotation (x, y, z, w; identity when the create carried none).
    pub rot: [f32; 4],
    /// The server object that carries it (`None`: not tied to an object, plays once).
    pub obj: Option<u16>,
    /// The object's value ([`WORLD_FX_LOOP`], [`WORLD_FX_CUT`]).
    pub value: u32,
}

/// The fields of a server shot-effect record the effects use.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (type 0x06: hit object, shooter, weapon, ammunition, surface `a`, flags `b`,
// client `c`, from, to, normal)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotRecord {
    pub shooter: u16,
    /// The hit object (0xFFFF: none).
    pub first: u16,
    pub weapon: u16,
    pub ammo: u16,
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub normal: [f32; 3],
    pub surface: u8,
    /// `b`: the record's suppression mask (bit 0x10 suppresses the tracer).
    pub clear: u16,
    /// The record names the own client (or the own character).
    pub own: bool,
    /// `e`, `f`, `g`: the shooter weapon's three mod numbers (0xff = none); the first picks the muzzle effect.
    pub mods: [u8; 3],
}

/// The record's hit object value for "none".
pub const NO_OBJECT: u16 = 0xFFFF;

/// The record of an engine effect message (`None` for anything but a shot effect).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2
pub fn record_of(e: &dc_botclient::fx::Effect, own: bool) -> Option<ShotRecord> {
    match e {
        dc_botclient::fx::Effect::Shot(x) => Some(ShotRecord {
            shooter: x.shooter,
            first: x.first,
            weapon: x.weapon,
            ammo: x.ammo,
            from: x.from,
            to: x.to,
            normal: x.normal,
            surface: x.surface,
            clear: x.clear,
            own,
            mods: x.mods,
        }),
        _ => None,
    }
}

/// The surface id a shot reports for the sky (and for a miss's end point): no impact work at all.
// EVIDENCE: cshell+0x23a910 (a miss produces the impact record with surface 0x6e); DISASM cshell+0x2c8dbb (surface
// 0x6e skips the impact effect, mark, surface effect and sound unless the impact row asks for sky impacts)
pub const SURFACE_SKY: u16 = 0x6e;

/// CLI-1603: the ROM surface id of a character hit drawn at the shot (the body row: the server's record names the
/// head row 4 for a head-zone hit, which the client does not know at fire time; both are the gore rows).
// EVIDENCE: DSV-29 (amendment): DISASM cshell+0x2c6808 (`a` 4 / 6 are the gore surfaces), ROM SURFACE.TXT row 6
pub const SURFACE_BODY: u16 = 6;

/// Tracers are drawn only for paths longer than this.
// EVIDENCE: cshell+0x2c6160 (the tracer is made when the ammunition has one and the path length is above 100)
pub const TRACER_MIN_PATH: f32 = 100.0;

/// The record flag that suppresses the tracer.
// DISASM: cshell+0x2c8e81 (tracer gate: bit 0x10 of the shot's flags not suppressed), cshell+0x235a90 (the own
// tracer counter sets it when the count is not zero)
pub const RECORD_NO_TRACER: u16 = 0x0010;

/// The record flag of the impact-only record of a hitting shot (no fire part: no muzzle effect for it).
// DC-DEFAULT: CLI-192 (proposed) another player's muzzle effect is started for records without 0x1000 (the fire-sound
// rule's bit, DISASM cshell+0x2c8b25..0x2c8b46; which record starts the 2009 muzzle effect is not traced)
pub const RECORD_NO_FIRE: u16 = 0x1000;

/// Marks kept at most (the oldest goes first).
// DC-DEFAULT: CLI-193 (amended by CLI-633) 64 marks
pub const MARKS_MAX: usize = 64;
/// A mark stays solid this long, then fades out over [`MARK_FADE_S`] and goes: the console values `MarkSolidTime`
/// and `MarkFadeTime` (both registered with 3.0 s; no ROM config sets them).
// DISASM: cshell+0x2bf210..0x2bf2c9 (the mark object's console values: MarksClip 0, MarkLight 1.0, MarkShow 1.0,
// MarkFadeTime 3.0, MarkSolidTime 3.0), cshell+0x2bf310 (update: age += frame time; past solid + fade the object
// goes; past solid its alpha becomes min(alpha, (solid + fade - age) / solid))
pub const MARK_SOLID_S: f64 = 3.0;
pub const MARK_FADE_S: f64 = 3.0;
/// The colour a mark is drawn with: the sprite's texels times 0.5 grey (alpha 1), alpha blended.
// DISASM: cshell+0x2bf460 (the mark is a sprite object, type 3, flags 9 (lit unless MarkLight is 0), colour set to
// (0.5, 0.5, 0.5, 1.0) after creation); SPEC: RENDER_2009.md 8.1 (sprites: TEXTURE x DIFFUSE, SRCALPHA / INVSRCALPHA)
pub const MARK_TINT: f32 = 0.5;

/// A mark's alpha `age_s` seconds after it was made: 1 while solid, then the 2009 fade, 0 at the end.
// DISASM: cshell+0x2bf3d5..0x2bf44d (the fade divides by the solid time, as the 2009 code does)
pub fn mark_alpha(age_s: f64) -> f32 {
    if age_s <= MARK_SOLID_S {
        1.0
    } else {
        ((MARK_SOLID_S + MARK_FADE_S - age_s) / MARK_SOLID_S).clamp(0.0, 1.0) as f32
    }
}
/// A new mark among more than this many marks within [`MARK_CROWD_RADIUS`] replaces the nearest of them.
// DISASM: cshell+0x2c8dda (more than 10 marks within 10 units: the nearest one is handed back instead of a new one)
pub const MARK_CROWD: usize = 10;
pub const MARK_CROWD_RADIUS: f32 = 10.0;

/// The inner radius of an impact sound as a fraction of its outer radius (x 255).
// DC-DEFAULT: CLI-194 (proposed) inner = 10 % of the impact row's SoundRadius, one-shot positional voice
pub const IMPACT_SOUND_INNER: u8 = 26;

/// The particle quad's half size per unit of the particle's size (its scale-key value): the engine builds each
/// particle's corners as `centre +- size x 2 right +- size x 2 up` (world units, or eye units for a player-view key).
// CORRECTION 2026-10-27 (arms-fixes2, CLI-591; was CLI-195 "half size = scale value", PLAUSIBLE): the engine side is
// now read. DISASM engine+0x1bb310 (the particle batch: the camera's two in-plane axes, or the unit eye axes for a
// player-view object (+0x84 bit 0x40), are each doubled before the four corner vectors +-2 right +-2 up are built),
// engine+0x1babb0 (per particle: corner = position + corner vector x the f32 at particle +0x18; the rotating form turns
// the doubled axes by the particle's angle at +0x2c), clientfx+0x1d290 (ClientFx writes the scale key value by life
// fraction into that same +0x18). Sprites use the same camera axes undoubled (engine+0x1af730: half = texture size x
// scale), so the doubling is the particle's own.
// DC-DEFAULT: CLI-195 (friction part unchanged) friction = the velocity kept per second (the power's exponent is not
// read; EVIDENCE clientfx+0x1f3a0 for the per-frame step: position += velocity dt, velocity = friction^dt' x velocity +
// acceleration dt)
pub const PARTICLE_HALF_PER_SIZE: f32 = 2.0;

/// The ammunition `Type` of thrown weapons (their surface effect is the surface's grenade group).
// EVIDENCE: cshell+0x2c7820 (the surface row's grenade group when the ammunition type is 2)
pub const AMMO_THROWN: u32 = 2;
/// The ammunition `Type` of the launchers' rounds (rockets, flames): fired without a trace, flown by the server.
// OBSERVED: ROM table key WEAPONS.TXT [Ammo<n>] Type (the rocket and flamethrower rows 1, with a ProjectileFXName)
pub const AMMO_LAUNCHED: u32 = 1;

/// Whether the 2009 fire routine traces a pellet of an ammunition `Type`: 0 (the instant trace) and 3 (the trace
/// after the zoom note); 1, 2 and 5 send the shot without a trace, any other value only logs an error.
// EVIDENCE: cshell+0x23c041..0x23c064 (`Type` at ammunition row +0x0c: 3 -> cshell+0x23ab90, which ends in the trace
// cshell+0x23a910; 0..5 through the table at cshell+0x23c298: 0 -> the trace, 1 / 2 / 5 -> none, 3 / 4 and above 5 ->
// the error line); cshell+0x174d30 (row +0x0c is the `Type` key)
pub fn traced_ammo(ammo_type: u32) -> bool {
    matches!(ammo_type, 0 | 3)
}

/// Whether rounds of an ammunition `Type` are flown by the server (launched or thrown), so their record is a blast.
pub fn flown_ammo(ammo_type: u32) -> bool {
    matches!(ammo_type, AMMO_LAUNCHED | AMMO_THROWN)
}
/// `InstDamageType` values whose "surface" sound is the surface's melee sound (and the hands sound).
// EVIDENCE: cshell+0x27f320 (0xd / 0x20 -> MeleeImpactSnd, 0x24 -> HandsImpactSnd, else BulletImpactSnd; type 1 / 2
// ammunition -> ProjectileImpactSnd)
pub const DAMAGE_MELEE: [u32; 2] = [0x0d, 0x20];
pub const DAMAGE_HANDS: u32 = 0x24;

// --------------------------------------------------------------------------------------------- textures

/// The textures of the effects, read from the user's ROM on first use (sprites play their frames, FMT_SPR §2.4).
#[derive(Default)]
pub struct TextureStore {
    index: Option<dc_render::TextureIndex>,
    sprites: HashMap<String, Option<dc_fx_data::Sprite>>,
    images: HashMap<String, Option<Arc<dc_render::TextureImage>>>,
    pub missing: u64,
}

impl std::fmt::Debug for TextureStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextureStore")
            .field("images", &self.images.len())
            .field("missing", &self.missing)
            .finish()
    }
}

impl TextureStore {
    pub fn open(rom: Option<&Path>) -> TextureStore {
        TextureStore {
            index: rom.and_then(|r| dc_render::TextureIndex::open(r).ok()),
            ..TextureStore::default()
        }
    }

    /// The texture path to draw for `path` `ms` after its start: a sprite's frame (looping), else the path.
    // SPEC: FMT_SPR_LTC_FX.md §2.4 (frame = (time / (1000 / fps)) % frame count; flags 3 = playing and looping)
    pub fn frame(&mut self, path: &str, ms: u32) -> Option<String> {
        if !dc_render::is_sprite_name(path.as_bytes()) {
            return Some(path.to_string());
        }
        if !self.sprites.contains_key(path) {
            let s = self
                .index
                .as_ref()
                .and_then(|i| i.read(path.as_bytes()).ok().flatten())
                .and_then(|b| dc_fx_data::Sprite::parse(&b).ok());
            self.sprites.insert(path.to_string(), s);
        }
        let s = self.sprites.get(path)?.as_ref()?;
        let n = s.frames.len();
        if n == 0 {
            return None;
        }
        let per = s.frame_period_ms().max(1);
        Some(s.frames[((ms / per) as usize) % n].clone())
    }

    /// The decoded image of a texture path (DTX).
    pub fn image(&mut self, path: &str) -> Option<Arc<dc_render::TextureImage>> {
        if !self.images.contains_key(path) {
            let img = self
                .index
                .as_ref()
                .and_then(|i| i.read(path.as_bytes()).ok().flatten())
                .and_then(|b| dc_tex::DtxFile::parse(&b).ok())
                .and_then(|d| dc_render::decode_dtx(&d))
                .map(Arc::new);
            if img.is_none() {
                self.missing += 1;
            }
            // perf-g1: the effect sprite cache keeps every decoded image (no eviction): its size is a pool gauge
            if let Some(i) = &img {
                let bytes: usize = i.levels.iter().map(Vec::len).sum();
                dc_render::perf::gauge(dc_render::perf::Gauge::DecodedImageBytes, bytes as i64);
            }
            self.images.insert(path.to_string(), img);
        }
        self.images.get(path).cloned().flatten()
    }

    /// Width and height of a texture path (1 x 1 when it does not load).
    pub fn dims(&mut self, path: &str) -> (f32, f32) {
        self.image(path)
            .map_or((1.0, 1.0), |i| (i.width as f32, i.height as f32))
    }
}

// --------------------------------------------------------------------------------------------- draw lists

/// One textured quad: centre and two half axes, colour (0..1, straight alpha), texture path (`None`: white).
#[derive(Debug, Clone, PartialEq)]
pub struct Quad {
    pub centre: Vec3,
    pub u: Vec3,
    pub v: Vec3,
    pub colour: [f32; 4],
    pub texture: Option<String>,
    pub blend: Blend,
    pub depth_test: bool,
}

/// The quads of one frame: world space (main camera) and eye space (player-view camera).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FxDraw {
    pub world: Vec<Quad>,
    pub view: Vec<Quad>,
}

/// Counters for reports and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FxStats {
    pub own_shots: u64,
    pub records: u64,
    pub muzzle: u64,
    pub remote_muzzle: u64,
    /// Remote records whose muzzle group / tracer started at the drawn weapon's socket (CLI-1591).
    pub remote_muzzle_socket: u64,
    pub tracers: u64,
    pub impacts: u64,
    pub surface_fx: u64,
    pub marks: u64,
    /// Marks dropped because what they lay on left ([`FxRunner::support_marks`]).
    pub marks_dropped: u64,
    pub sounds: u64,
    pub blasts: u64,
    pub groups_started: u64,
    pub groups_missing: u64,
    /// Placed-effect objects handed over by [`FxEvent::World`] (played or not found).
    pub world: u64,
    /// Quads of the last [`FxRunner::draw`].
    pub world_quads: usize,
    pub view_quads: usize,
    /// Own shots whose impact lay on a linked world model (a crate, a drum: CLI-250).
    pub model_impacts: u64,
    /// client-destructfx: anchor keys started, sound keys played, the most effect models placed in one frame and the
    /// highest rise of an anchor above its start (units, rounded).
    pub anchors: u64,
    pub sound_keys: u64,
    pub fx_models_max: usize,
    pub anchor_rise: i32,
    /// w2-fx: cycles started by looping placed effects.
    pub world_cycles: u64,
}

struct LiveFx {
    group: Arc<EffectGroup>,
    t0: f64,
    pos: Vec3,
    rot: Quat,
    /// Placed on the first-person model (eye space).
    view: bool,
    seed: u64,
    spawned: Vec<bool>,
    life: f64,
    /// Anchor paths, link parents and played sounds ([`crate::fxkeys::GroupMotion`]).
    motion: crate::fxkeys::GroupMotion,
    /// w2-fx: the placed-effect object that owns the group (its value and lower-case group name).
    owner: Option<Owner>,
}

/// The placed-effect object of a live group (w2-fx).
#[derive(Debug, Clone)]
struct Owner {
    obj: u16,
    name: String,
    value: u32,
    /// The next cycle is still to start (the loop bit, the object present, the group has a length).
    looping: bool,
}

#[derive(Debug, Clone)]
struct Tracer {
    from: Vec3,
    to: Vec3,
    t0: f64,
    row: TracerRow,
}

#[derive(Debug, Clone)]
struct MarkFx {
    pos: Vec3,
    normal: Vec3,
    roll: f32,
    half: f32,
    texture: String,
    t0: f64,
    /// A mark of a world hit (not on a character): it needs a polygon under it ([`FxRunner::support_marks`]).
    world_hit: bool,
    /// The support was looked up (the first frame with a world after the mark was made).
    probed: bool,
    /// The linked world model the mark lies on (lower-case name), `None` for the level.
    on: Option<Vec<u8>>,
}

/// How far in front of and behind its point a world mark looks for the polygon it lies on.
// DC-DEFAULT: CLI-603 (proposed) 8 units
pub const MARK_PROBE: f32 = 8.0;

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A small deterministic random source per particle (the 2009 module uses the C runtime `rand`; the values are
/// not reproduced, only their ranges).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(splitmix(seed))
    }
    /// 0..1.
    fn unit(&mut self) -> f32 {
        self.0 = splitmix(self.0);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    /// -1..1.
    fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }
}

/// A rotation whose +Z is `normal` (any roll).
pub fn frame_of(normal: Vec3) -> Quat {
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return Quat::IDENTITY;
    }
    Quat::from_rotation_arc(Vec3::Z, n)
}

/// The two unit axes of a plane with normal `n`.
fn plane_axes(n: Vec3) -> (Vec3, Vec3) {
    let n = n.normalize_or_zero();
    let up = if n.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
    let a = up.cross(n).normalize_or_zero();
    (a, n.cross(a))
}

/// Position of a particle `age` seconds after its emission with constant acceleration `a` and the velocity kept per
/// second `friction` (closed form of the per-frame step).
fn particle_at(p0: Vec3, v0: Vec3, a: Vec3, friction: f32, age: f32) -> Vec3 {
    let k = -friction.clamp(1e-6, 1.0).ln();
    if k < 1e-5 {
        return p0 + v0 * age + a * (0.5 * age * age);
    }
    let e = (-k * age).exp();
    let vt = a / k;
    p0 + vt * age + (v0 - vt) * ((1.0 - e) / k)
}

/// The effects of the frame: live groups, tracers and marks, driven by the frame source's events.
pub struct FxRunner {
    pub tables: FxTables,
    pub library: EffectLibrary,
    pub textures: TextureStore,
    /// CLI-1591: the drawn third-person weapon's muzzle socket (world transform) of each remote shooter this frame,
    /// by object id; a remote shot's muzzle group and tracer start there instead of at the record position.
    pub remote_muzzle: BTreeMap<u16, Mat4>,
    live: Vec<LiveFx>,
    tracers: Vec<Tracer>,
    marks: Vec<MarkFx>,
    /// Shot counters per weapon for the tracer frequency.
    tracer_count: BTreeMap<u16, u32>,
    /// The scene seed every random stream of the runner derives from ([`FxRunner::stream_seed`]).
    seed: u64,
    /// The start time (bits) of `seed_counts`.
    seed_instant: u64,
    /// Streams with the same content key started at `seed_instant` (the n-th identical effect of one instant).
    seed_counts: BTreeMap<u64, u64>,
    /// The thrown objects whose projectile effect was started (CLI-1136).
    thrown_started: std::collections::BTreeSet<u16>,
    pub stats: FxStats,
}

impl std::fmt::Debug for FxRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FxRunner")
            .field("groups", &self.library.len())
            .field("live", &self.live.len())
            .field("stats", &self.stats)
            .finish()
    }
}

impl FxRunner {
    pub fn new(tables: FxTables, library: EffectLibrary, textures: TextureStore) -> FxRunner {
        FxRunner {
            tables,
            library,
            textures,
            remote_muzzle: BTreeMap::new(),
            live: Vec::new(),
            tracers: Vec::new(),
            marks: Vec::new(),
            tracer_count: BTreeMap::new(),
            seed: 0x0dc0_ffee,
            seed_instant: u64::MAX,
            seed_counts: BTreeMap::new(),
            thrown_started: Default::default(),
            stats: FxStats::default(),
        }
    }

    /// The runner of the user's ROM (`None`: empty tables, nothing is drawn).
    pub fn open(rom: Option<&Path>) -> FxRunner {
        match rom {
            Some(r) => FxRunner::new(
                FxTables::open(r),
                EffectLibrary::open(r),
                TextureStore::open(Some(r)),
            ),
            None => FxRunner::new(
                FxTables::default(),
                EffectLibrary::default(),
                TextureStore::default(),
            ),
        }
    }

    /// The seed of one random stream, from what the effect IS: the scene seed, the stream's kind, the group name
    /// (case-folded), the place and rotation bits and the start time, plus the ordinal among identical effects of the
    /// same instant. Never from how many effects started before it, so an effect's particles do not depend on the
    /// order or the frame in which unrelated events arrived (G0: scheduling-independent random streams,
    /// docs/RENDER_DETERMINISM_CONTRACT.md).
    // DC-DEFAULT: CLI-430 (proposed), see the staged rows (ORIGINAL: the 2009 module's C runtime `rand` values are not
    // reproduced, only their ranges)
    fn stream_seed(&mut self, kind: u64, name: &str, pos: Vec3, rot: Quat, now: f64) -> u64 {
        let mut h = splitmix(self.seed ^ kind.rotate_left(56));
        for b in name.bytes() {
            h = splitmix(h ^ u64::from(b.to_ascii_lowercase()));
        }
        for v in pos.to_array().into_iter().chain(rot.to_array()) {
            h = splitmix(h ^ u64::from(v.to_bits()));
        }
        h = splitmix(h ^ now.to_bits());
        if self.seed_instant != now.to_bits() {
            self.seed_instant = now.to_bits();
            self.seed_counts.clear();
        }
        let n = self.seed_counts.entry(h).or_insert(0);
        let ordinal = *n;
        *n += 1;
        splitmix(h ^ ordinal)
    }

    /// Live groups, tracers and marks now.
    pub fn live_counts(&self) -> (usize, usize, usize) {
        (self.live.len(), self.tracers.len(), self.marks.len())
    }

    /// The place and rotation (x, y, z, w) of every running group (tests and tools).
    pub fn live_frames(&self) -> Vec<([f32; 3], [f32; 4])> {
        self.live
            .iter()
            .map(|l| (l.pos.to_array(), l.rot.to_array()))
            .collect()
    }

    /// The dynamic lights of the light keys running at `now_ms` (w2-fx): one point light per key at the key's place
    /// (a group on the first-person model: at `eye`), colour from the key's colour keys x opacity (x the random
    /// dimming when the key asks for it), radius from its scale keys, both at the key's life fraction. A key with no
    /// radius or no colour gives none. The model light sets take them with the dynamic-light law
    /// ([`dc_render::ModelLight::dynamic`]).
    // EVIDENCE: clientfx+0xd970 (update), clientfx+0xdc00 (create), clientfx+0x1f70 (the colour and scale keys at the
    // key's time); SPEC: RENDER_2009.md §6.3 (dynamic lights in the model light set: attenuation (1, 0, 19 / r^2))
    // DC-DEFAULT: CLI-514 (proposed) a light key of a group on the first-person model stands at the eye (the player-view
    // object's world place is not traced); effect lights light the models only, not the world (RENDER_2009.md §5.4: the
    // world part's attenuation map is not decoded)
    pub fn lights_at(&self, now_ms: u32, eye: Vec3) -> Vec<dc_render::ModelLight> {
        use crate::fxkeys::{light_colour, random_step, step_unit, LIGHT_DIM_BASE, LIGHT_DIM_SPAN};
        let now = now_ms as f64 / 1000.0;
        let mut out = Vec::new();
        for l in &self.live {
            let t = (now - l.t0) as f32;
            let mut views: Option<Vec<crate::fxkeys::KeyView<'_>>> = None;
            for (ki, k) in l.group.keys.iter().enumerate() {
                let KeyKind::Light(lk) = &k.kind else {
                    continue;
                };
                if t < k.start || t > k.end {
                    continue;
                }
                let span = (k.end - k.start).max(1e-4);
                let f = ((t - k.start) / span).clamp(0.0, 1.0);
                let (rgb, opacity) = colour_at(&k.colours, f);
                let dim = if lk.dim {
                    LIGHT_DIM_BASE + LIGHT_DIM_SPAN * step_unit(l.seed, ki, random_step(t), 0)
                } else {
                    1.0
                };
                let colour = Vec3::from(light_colour(rgb, opacity, dim));
                let radius = scale_at(&k.scales, f);
                if radius.is_nan()
                    || radius <= 0.0
                    || !radius.is_finite()
                    || colour.max_element() <= 0.0
                {
                    continue;
                }
                let pos = if l.view {
                    eye
                } else {
                    let v = views.get_or_insert_with(|| key_views(&l.group));
                    l.motion.key_pos(v, ki, t, l.pos, l.rot)
                };
                if pos.is_finite() {
                    out.push(dc_render::ModelLight::dynamic(pos, radius, colour));
                }
            }
        }
        out
    }

    /// The camera offset of the shake keys running at `now_ms` for a camera at `cam` (w2-fx): per key, when the camera
    /// is within the key's outer radius, a random offset per axis of up to the key's scale value times the fade
    /// ([`crate::fxkeys::ShakeKey::fade`]); the offsets of several keys add up. A key of a group on the first-person
    /// model stands at the camera.
    // EVIDENCE: clientfx+0x3f40 (update), clientfx+0x1f70 (the scale value +0x34 from the key's scale keys)
    // DC-DEFAULT: CLI-513 (proposed) the random offset is drawn once per 1 / 60 s of the group's clock; CLI-515
    // (proposed) the shake runs whether or not the window has the focus (the module skips it without the focus flag
    // its export 7 stores) and on whichever camera the frame draws (own, death or world camera)
    pub fn shake_at(&self, now_ms: u32, cam: Vec3) -> Vec3 {
        use crate::fxkeys::{random_step, shake_offset, step_unit};
        let now = now_ms as f64 / 1000.0;
        let mut sum = Vec3::ZERO;
        for l in &self.live {
            let t = (now - l.t0) as f32;
            let mut views: Option<Vec<crate::fxkeys::KeyView<'_>>> = None;
            for (ki, k) in l.group.keys.iter().enumerate() {
                let KeyKind::Shake(sk) = &k.kind else {
                    continue;
                };
                if t < k.start || t > k.end {
                    continue;
                }
                let pos = if l.view {
                    cam
                } else {
                    let v = views.get_or_insert_with(|| key_views(&l.group));
                    l.motion.key_pos(v, ki, t, l.pos, l.rot)
                };
                let Some(fade) = sk.fade(pos.distance_squared(cam)) else {
                    continue;
                };
                let span = (k.end - k.start).max(1e-4);
                let f = ((t - k.start) / span).clamp(0.0, 1.0);
                let step = random_step(t);
                let u = [0, 1, 2].map(|lane| step_unit(l.seed, ki, step, 0x5a + lane));
                let off = shake_offset(u, fade, scale_at(&k.scales, f));
                if off.is_finite() {
                    sum += off;
                }
            }
        }
        sum
    }

    /// Starts the effect group `name` at `pos` with +Z along `rot` (`view`: on the first-person model, eye space).
    pub fn start_group(&mut self, name: &str, pos: Vec3, rot: Quat, view: bool, now: f64) -> bool {
        let Some(group) = self.library.get(name) else {
            self.stats.groups_missing += 1;
            return false;
        };
        self.start_live(group, name, pos, rot, view, now);
        true
    }

    /// Starts one instance of `group` (found under `name`) at `now`.
    fn start_live(
        &mut self,
        group: Arc<EffectGroup>,
        name: &str,
        pos: Vec3,
        rot: Quat,
        view: bool,
        now: f64,
    ) {
        dc_render::perf::add(dc_render::perf::Counter::FxSpawns, 1);
        let mut life = group.length_s as f64;
        for k in &group.keys {
            let tail = match &k.kind {
                KeyKind::Particles(p) => p.life.0.max(p.life.1),
                _ => 0.0,
            };
            life = life.max((k.end + tail) as f64);
        }
        let n = group.keys.len();
        let seed = self.stream_seed(1, name, pos, rot, now);
        let motion = crate::fxkeys::GroupMotion::new(&key_views(&group));
        self.live.push(LiveFx {
            group,
            t0: now,
            pos,
            rot,
            view,
            seed,
            spawned: vec![false; n],
            life,
            motion,
            owner: None,
        });
        self.stats.groups_started += 1;
    }

    /// Takes the frame's events at `now_ms` (the frame source's clock); `world` resolves the own shots' impacts
    /// (polygon, normal, surface). Returns the impact and explosion sounds to play.
    pub fn feed(
        &mut self,
        now_ms: u32,
        events: &[FxEvent],
        world: Option<&crate::world::LoadedWorld>,
    ) -> Vec<crate::sound::SoundCue> {
        let now = now_ms as f64 / 1000.0;
        let mut cues = Vec::new();
        for e in events {
            match e {
                FxEvent::Shot(s) => self.own_shot(s, world, now, &mut cues),
                FxEvent::Record(r) => self.record(r, now, &mut cues),
                FxEvent::World(w) => self.world_effect(w, now),
                FxEvent::WorldGone(obj) => self.world_gone(*obj),
                FxEvent::Thrown {
                    obj,
                    ammo,
                    pos,
                    rot,
                } => self.thrown(*obj, *ammo, *pos, *rot, now),
            }
        }
        self.step(now);
        self.step_motion(now, world, &mut cues);
        if let Some(w) = world {
            self.support_marks(w);
        }
        cues
    }

    /// The marks of world hits keep to what they lie on: a new mark looks up the polygon under it (a short ray through
    /// its point along its normal, the shot trace's world); one with nothing under it is dropped, and one on a linked
    /// world model goes when the server switches that model off (a broken crate or drum takes its bullet holes with
    /// it instead of leaving them in the air).
    // EVIDENCE: cshell+0x2c5ef0 (the mark effect is created from the point, rotation, sprite, ammunition and surface
    // only: no object of the hit is handed over); UDP_SESSION_2009_B.md §4.2 item 3 (a dead breakable is its brush
    // with the visible / solid / ray-hit flags cleared, objects.rs)
    // DC-DEFAULT: CLI-603 (proposed) the 2009 removal of a mark on a world model that leaves is not traced; ours
    // follows the model
    fn support_marks(&mut self, w: &crate::world::LoadedWorld) {
        for m in self.marks.iter_mut().filter(|m| m.world_hit && !m.probed) {
            m.probed = true;
            let n = m.normal.normalize_or_zero();
            let from = m.pos + n * MARK_PROBE;
            match w.shot_hit(from.to_array(), (-n).to_array(), 2.0 * MARK_PROBE) {
                Some(h) => m.on = h.model,
                // nothing to lie on: the model under it already left
                None => m.half = 0.0,
            }
        }
        let before = self.marks.len();
        self.marks.retain(|m| {
            m.half > 0.0
                && m.on
                    .as_deref()
                    .is_none_or(|name| w.world_models.state(name).is_none_or(|(shown, _)| shown))
        });
        self.stats.marks_dropped += (before - self.marks.len()) as u64;
    }

    /// One pellet of the own shot: the first-person muzzle group on the shot's first pellet, the tracer every
    /// `Frequency`-th pellet (first pellets only), the impact of a world hit.
    // EVIDENCE: cshell+0x2348d0 (the first-person muzzle group shows for one group length after each shot);
    // cshell+0x235a90 (the tracer counter per weapon, every pellet; the later pellets of a shot have the tracer bit
    // suppressed); cshell+0x23a910 / +0x238eb0 (the trace hands the hit point, polygon normal and surface on)
    // DC-DEFAULT: CLI-196 (proposed; amended by CLI-1603, USER REQUEST 2026-10-31 "the view for the player should be
    // instant") the own shot's character hit draws its impact (the body gore surface) at the shot, like its world hit;
    // the server's record of the hit draws nothing more. Before: the character hit waited for the server's record
    // (surface by hit zone) draws the blood; own world hits are drawn from the local trace and the server's world
    // records of own shots are skipped (the 2009 client's handling of its own echoed records is not traced)
    fn own_shot(
        &mut self,
        s: &crate::arms::ShotFx,
        world: Option<&crate::world::LoadedWorld>,
        now: f64,
        cues: &mut Vec<crate::sound::SoundCue>,
    ) {
        self.stats.own_shots += 1;
        let dir = Vec3::from(s.dir).normalize_or_zero();
        let origin = Vec3::from(s.origin);
        if s.first {
            if let Some(name) = self.tables.muzzle_group(s.weapon, s.flash_mod, false) {
                if self.start_group(&name, Vec3::ZERO, Quat::IDENTITY, true, now) {
                    self.stats.muzzle += 1;
                }
            }
        }
        // an untraced round (a launcher's, a thrown one) has no tracer and no impact of its own: the server's object
        // and blast show it
        // EVIDENCE: cshell+0x23c041..0x23c064, table cshell+0x23c298 (no trace for `Type` 1, 2, 5)
        // DC-DEFAULT: CLI-1590 (proposed)
        if self
            .tables
            .ammo_fx(s.ammo)
            .is_some_and(|a| !traced_ammo(a.ammo_type))
        {
            return;
        }
        // the end point: the character's box, the world polygon, or the range end
        let mut end = origin + dir * s.range;
        let mut impact: Option<(Vec3, Vec3, u16, bool)> = None;
        match s.hit {
            crate::arms::RayHit::Character { dist, normal, .. } => {
                end = origin + dir * dist;
                // CLI-1603: the hit's gore effect at the shot (no mark on a character), on the box face entered
                let n = Vec3::from(normal);
                let n = if n.is_finite() && n.length_squared() > 1e-6 {
                    n.normalize()
                } else {
                    -dir
                };
                impact = Some((end, n, SURFACE_BODY, false));
            }
            crate::arms::RayHit::Miss => {
                // the level or a world model the server created (a crate, a drum): the polygon's surface
                // DC-DEFAULT: CLI-250 (proposed) the same shot set and blocker rule as the pellet trace
                let at = s.impact.or_else(|| {
                    world.and_then(|w| {
                        w.shot_hit(s.origin, dir.to_array(), s.range)
                            .map(|h| h.at())
                    })
                });
                if let Some(h) = at {
                    end = Vec3::from(h.point);
                    impact = Some((end, Vec3::from(h.normal), h.surface, true));
                    if h.on_model {
                        self.stats.model_impacts += 1;
                    }
                }
            }
        }
        let freq = self.tables.tracer(s.ammo).map(|t| t.frequency);
        let c = self.tracer_count.entry(s.weapon).or_insert(0);
        *c = match freq {
            Some(f) if f > 0 => (*c + 1) % f,
            _ => 0,
        };
        let tracer_now = *c == 0;
        if s.first && tracer_now {
            self.tracer(s.ammo, Vec3::from(s.muzzle), end, now);
        }
        if let Some((p, n, surface, world_hit)) = impact {
            self.impact(s.ammo, surface, p, n, world_hit, now, cues);
        }
    }

    /// A server record: another player's muzzle group and tracer, and the impact (the blast of a thrown object takes
    /// the same impact path with its ammunition's impact row).
    // DISASM: cshell+0x2c6770 / +0x2c6940 (the record fills the same per-shot object the local shot builds),
    // cshell+0x2c8540 (spawn: tracer, impact, mark, surface effect, sound); EVIDENCE: cshell+0x2c71d0 (another
    // player's muzzle group: the weapon row's third-person name, at the record position, per shot)
    fn record(&mut self, r: &ShotRecord, now: f64, cues: &mut Vec<crate::sound::SoundCue>) {
        self.stats.records += 1;
        let fx = self.tables.ammo_fx(r.ammo).cloned().unwrap_or_default();
        // a round the server flies (a launched or thrown one): its record is the blast where it burst, drawn for the
        // shooter too, with no muzzle effect or tracer (the flight was the object)
        // DC-DEFAULT: CLI-1590 (proposed) launched rounds as thrown ones (before: only `Type` 2, so the shooter never
        // saw his own rocket's explosion and the others saw a tracer along its flight)
        let thrown = flown_ammo(fx.ammo_type);
        // CLI-1603: every own record but a blast is skipped: the local shot drew its world or character impact
        if r.own && !thrown {
            return;
        }
        let from = Vec3::from(r.from);
        let to = Vec3::from(r.to);
        if !(from.is_finite() && to.is_finite()) {
            return;
        }
        if thrown && u16::from(r.surface) != SURFACE_SKY {
            self.stats.blasts += 1;
        }
        if !r.own && !thrown {
            // DC-DEFAULT: CLI-1591 (proposed; amends CLI-192) the muzzle group and the tracer of another player's
            // shot start at his drawn weapon's muzzle socket when the window placed one (the record position is the
            // shooter's first-person muzzle point, about 9 units before his eye: inside the third-person gun)
            let start = match self.remote_muzzle.get(&r.shooter) {
                Some(m) => {
                    let p = m.w_axis.truncate();
                    if p.is_finite() {
                        self.stats.remote_muzzle_socket += 1;
                        p
                    } else {
                        from
                    }
                }
                None => from,
            };
            if r.clear & RECORD_NO_FIRE == 0 {
                let first = (r.mods[0] != 0xff).then_some(u16::from(r.mods[0]));
                if let Some(name) = self.tables.muzzle_group(r.weapon, first, true) {
                    let rot = frame_of(to - from);
                    if self.start_group(&name, start, rot, false, now) {
                        self.stats.remote_muzzle += 1;
                    }
                }
            }
            if r.clear & RECORD_NO_TRACER == 0 {
                self.tracer(r.ammo, start, to, now);
            }
        }
        let n = Vec3::from(r.normal);
        let n = if n.is_finite() && n.length_squared() > 1e-6 {
            n.normalize()
        } else {
            Vec3::Y
        };
        // a world hit (first 0) or a blast (no object) lies on the world; a character's record does not
        let world_hit = r.first == 0 || r.first == NO_OBJECT;
        self.impact(r.ammo, u16::from(r.surface), to, n, world_hit, now, cues);
    }

    /// A placed-effect object of the server: its effect group once, at the object's position and rotation. A breakable's
    /// explosion arrives as the server's blast record ([`FxEvent::Record`]), which takes the grenade path (impact
    /// group, surface group, explosion sound), so this plays the object's own group only; the object's removal later
    /// does nothing more (the group runs out by its own length).
    // SPEC: GAME_MESSAGES_2009_S2C.md §8.4 (type 0x64 effect name); EVIDENCE: clientfx+0x16f0 (the placement and
    // lifetime of a group come from its own keys)
    // DC-DEFAULT: CLI-234 (proposed; amended by w2-fx) the placed effect starts when its object appears; with the loop
    // bit of its value it plays again every group length while its object exists (CLI-516), and its object's removal
    // stops it (CLI-517)
    // DISASM: cshell+0x26e070 (type 0x64 with an object: the group is created at the object's position AND rotation,
    // read through the engine's object position / rotation calls), cshell+0x26de40 (the group instance takes both from
    // its object). CORRECTION 2026-10-26 (client-destructfx): the group was turned +Z up, which laid the world-axis
    // designs of the breakables' groups (a drum's upward fire column and its launch) on their side
    fn world_effect(&mut self, w: &WorldFx, now: f64) {
        self.stats.world += 1;
        let pos = Vec3::from(w.pos);
        let [x, y, z, q] = w.rot;
        let rot = Quat::from_xyzw(x, y, z, q);
        let rot = if rot.is_finite() && rot.length_squared() > 1e-6 {
            rot.normalize()
        } else {
            Quat::IDENTITY
        };
        if pos.is_finite() && self.start_group(&w.name, pos, rot, false, now) {
            let Some(obj) = w.obj else {
                return;
            };
            if let Some(l) = self.live.last_mut() {
                l.owner = Some(Owner {
                    obj,
                    name: lower(&w.name),
                    value: w.value,
                    looping: w.value & WORLD_FX_LOOP != 0 && l.group.length_s > 0.0,
                });
            }
        }
    }

    /// A thrown object drawn at `pos`: its projectile row's effect group starts on the first sight and follows it
    /// (the trail's particles stay where they were emitted).
    // EVIDENCE: cshell+0x2cab60 (the thrown-object record starts the projectile row's `FXName` group on the server's
    // object with the row's flags), cshell+0x292f90 (`FXLoop`, `FXSmoothShutdown`)
    // DC-DEFAULT: CLI-1136 (proposed, playtest report "rocket launcher missile disappears half way if you shoot it far")
    // the group is placed at the object's drawn position every frame; `FXLoop` loops it for the whole flight, without
    // `FXSmoothShutdown` the removal cuts it
    // EVIDENCE: cshell+0x2cab60 (the row's group is created on the object, so it takes the object's place and
    // rotation: the flame row's jet keys fire along the group's +Z, the way the object faces)
    // DC-DEFAULT: CLI-1136 (amended, proposed) the group takes the object's rotation (before: none, so a flame jet
    // went along the world's +Z whatever the aim)
    fn thrown(&mut self, obj: u16, ammo: u16, pos: [f32; 3], rot: [f32; 4], now: f64) {
        let at = Vec3::from(pos);
        if !at.is_finite() {
            return;
        }
        let mut seen = false;
        for l in &mut self.live {
            if l.owner.as_ref().is_some_and(|o| o.obj == obj) {
                l.pos = at;
                seen = true;
            }
        }
        if seen || self.thrown_started.contains(&obj) {
            return;
        }
        self.thrown_started.insert(obj);
        let Some(row) = self.tables.projectile(ammo).cloned() else {
            return;
        };
        let Some(name) = row.fx else {
            return;
        };
        let value = if row.fx_loop { WORLD_FX_LOOP } else { 0 }
            | if row.fx_smooth { 0 } else { WORLD_FX_CUT };
        let rot = if rot.iter().all(|c| c.is_finite()) {
            rot
        } else {
            [0.0, 0.0, 0.0, 1.0]
        };
        self.world_effect(
            &WorldFx {
                name,
                pos,
                rot,
                obj: Some(obj),
                value,
            },
            now,
        );
    }

    /// The placed-effect object `obj` left: its groups start no further cycle; with the cut bit they end now.
    // EVIDENCE: cshell+0x26e650 (shutdown on the object's removal), cshell+0x26c950
    // DC-DEFAULT: CLI-517 (proposed) without the cut bit every running key of the group runs on to its end (the key
    // flag +0x45 that picks which keys may do so is not read); with it the group is removed at once
    fn world_gone(&mut self, obj: u16) {
        self.thrown_started.remove(&obj);
        for l in &mut self.live {
            if let Some(o) = l.owner.as_mut().filter(|o| o.obj == obj) {
                o.looping = false;
            }
        }
        self.live.retain(|l| {
            !l.owner
                .as_ref()
                .is_some_and(|o| o.obj == obj && o.value & WORLD_FX_CUT != 0)
        });
    }

    /// The next cycles of the looping placed effects that came due by `now`: a cycle starts every group length after
    /// the previous one (on the session clock, whatever the frame rate) and the new instance carries the loop on, while
    /// the old one runs out its particles.
    // EVIDENCE: cshell+0x26e4a0, cshell+0x26d550 (see WORLD_FX_LOOP)
    // DC-DEFAULT: CLI-516 (proposed) each cycle is a new instance of the group (its keys restart at their start times;
    // particles of the previous cycle live on); after a stall of more than one group length only the latest due cycle
    // starts
    fn loop_cycles(&mut self, now: f64) {
        let mut due: Vec<(Arc<EffectGroup>, Owner, Vec3, Quat, f64)> = Vec::new();
        for l in &mut self.live {
            let Some(o) = l.owner.as_mut().filter(|o| o.looping) else {
                continue;
            };
            let len = f64::from(l.group.length_s);
            if len <= 0.0 || !len.is_finite() || now < l.t0 + len {
                continue;
            }
            let k = ((now - l.t0) / len).floor().max(1.0);
            o.looping = false;
            due.push((l.group.clone(), o.clone(), l.pos, l.rot, l.t0 + k * len));
        }
        for (group, mut o, pos, rot, t0) in due {
            let name = o.name.clone();
            self.start_live(group, &name, pos, rot, false, t0);
            o.looping = true;
            if let Some(l) = self.live.last_mut() {
                l.owner = Some(o);
            }
            self.stats.world_cycles += 1;
        }
    }

    fn tracer(&mut self, ammo: u16, from: Vec3, to: Vec3, now: f64) {
        let Some(row) = self.tables.tracer(ammo).cloned() else {
            return;
        };
        if row.velocity <= 0.0 || from.distance(to) <= TRACER_MIN_PATH {
            return;
        }
        self.tracers.push(Tracer {
            from,
            to,
            t0: now,
            row,
        });
        self.stats.tracers += 1;
    }

    /// The impact of the ammunition `ammo` on `surface` at `pos` facing `normal`: the impact row's group, the mark, the
    /// surface's group and the sound.
    // EVIDENCE: cshell+0x293b00 (the impact row's effect group at the point along the normal), cshell+0x2c7820 (with
    // DoSurfaceFX the surface row's impact group, its grenade group for thrown ammunition), cshell+0x2c6440 /
    // +0x27f320 (the sound: the row's wave, or the surface's by ammunition and damage type, one of four at random,
    // radius = SoundRadius); DISASM cshell+0x2c8dda (the mark: CreateMark, the surface's ShowsMark; the row's Mark
    // sprite or the surface's bullet hole with a scale between its min and max, times MarkScale; a random roll)
    #[allow(clippy::too_many_arguments)]
    fn impact(
        &mut self,
        ammo: u16,
        surface: u16,
        pos: Vec3,
        normal: Vec3,
        world_hit: bool,
        now: f64,
        cues: &mut Vec<crate::sound::SoundCue>,
    ) {
        if surface == SURFACE_SKY {
            return;
        }
        let Some(row) = self.tables.impact(ammo).cloned() else {
            return;
        };
        let fx = self.tables.ammo_fx(ammo).cloned().unwrap_or_default();
        let srow = self.tables.surfaces.get(&surface).cloned();
        self.stats.impacts += 1;
        let rot = frame_of(normal);
        if let Some(g) = row.fx.as_deref() {
            self.start_group(g, pos, rot, false, now);
        }
        let mut rng = Rng::new(self.stream_seed(2, "", pos, frame_of(normal), now));
        if row.create_mark && srow.as_ref().is_some_and(|s| s.shows_mark) {
            let tex = if row.mark_surface {
                srow.as_ref().and_then(|s| {
                    let sc = rng.range(s.hole_scale.0, s.hole_scale.1);
                    s.hole.clone().map(|h| (h, sc))
                })
            } else {
                row.mark.clone().map(|m| (m, 1.0))
            };
            // a mark whose sprite does not load is not made (no white square)
            let frame = tex
                .as_ref()
                .and_then(|(t, _)| self.textures.frame(t, 0))
                .filter(|f| self.textures.image(f).is_some());
            if let (Some((t, sc)), Some(frame)) = (tex, frame) {
                let (w, h) = self.textures.dims(&frame);
                let half = 0.5 * w.max(h) * sc * row.mark_scale;
                if half > 0.0 && half.is_finite() {
                    self.add_mark(MarkFx {
                        pos,
                        normal,
                        roll: rng.unit() * std::f32::consts::TAU,
                        half,
                        texture: t,
                        t0: now,
                        world_hit,
                        probed: false,
                        on: None,
                    });
                }
            }
        }
        if row.surface_fx {
            let g = srow.as_ref().and_then(|s| {
                if fx.ammo_type == AMMO_THROWN {
                    s.grenade_fx.clone()
                } else {
                    s.impact_fx.clone()
                }
            });
            if let Some(g) = g {
                if self.start_group(&g, pos, rot, false, now) {
                    self.stats.surface_fx += 1;
                }
            }
        }
        let file = if row.surface_sound {
            srow.as_ref().and_then(|s| {
                let list = if fx.ammo_type != 0 {
                    &s.projectile_sounds
                } else if DAMAGE_MELEE.contains(&fx.damage_type) {
                    &s.melee_sounds
                } else if fx.damage_type == DAMAGE_HANDS {
                    &s.hands_sounds
                } else {
                    &s.bullet_sounds
                };
                (!list.is_empty())
                    .then(|| list[(rng.unit() * list.len() as f32) as usize % list.len()].clone())
            })
        } else {
            row.sound.clone()
        };
        if let Some(f) = file {
            cues.push(impact_cue(f, pos, row.sound_radius));
            self.stats.sounds += 1;
        }
    }

    fn add_mark(&mut self, m: MarkFx) {
        let near: Vec<usize> = (0..self.marks.len())
            .filter(|i| self.marks[*i].pos.distance(m.pos) < MARK_CROWD_RADIUS)
            .collect();
        if near.len() > MARK_CROWD {
            if let Some(i) = near.into_iter().min_by(|a, b| {
                self.marks[*a]
                    .pos
                    .distance(m.pos)
                    .total_cmp(&self.marks[*b].pos.distance(m.pos))
            }) {
                self.marks[i] = m;
            }
            return;
        }
        if self.marks.len() >= MARKS_MAX {
            self.marks.remove(0);
        }
        self.marks.push(m);
        self.stats.marks += 1;
    }

    /// Spawn keys that came due, the next cycles of looping placed effects, and the end of finished groups, tracers and
    /// marks.
    fn step(&mut self, now: f64) {
        self.loop_cycles(now);
        let mut spawn: Vec<(String, Vec3, Quat, bool)> = Vec::new();
        for l in &mut self.live {
            let t = (now - l.t0) as f32;
            for (i, k) in l.group.keys.iter().enumerate() {
                if let KeyKind::Spawn(name) = &k.kind {
                    if !l.spawned[i] && t >= k.start {
                        l.spawned[i] = true;
                        let pos = l.pos + l.rot * Vec3::from(k.offset);
                        spawn.push((name.clone(), pos, l.rot, l.view));
                    }
                }
            }
        }
        for (n, p, r, v) in spawn {
            self.start_group(&n, p, r, v, now);
        }
        self.live.retain(|l| now - l.t0 <= l.life + 0.05);
        self.tracers.retain(|t| {
            let d = t.from.distance(t.to);
            now - t.t0 <= (d / t.row.velocity.max(1e-3)) as f64
        });
        self.marks
            .retain(|m| now - m.t0 <= MARK_SOLID_S + MARK_FADE_S);
    }

    /// The quads at `now_ms` for `camera`; `view_socket` = the eye-space transform of the first-person model's muzzle
    /// socket (the attach point of the player-view keys; `None`: the first-person keys are not drawn).
    pub fn draw(
        &mut self,
        now_ms: u32,
        camera: &dc_render::Camera,
        view_socket: Option<Mat4>,
    ) -> FxDraw {
        let now = now_ms as f64 / 1000.0;
        let right = camera.rot * Vec3::X;
        let up = camera.rot * Vec3::Y;
        let mut out = FxDraw::default();
        let live = std::mem::take(&mut self.live);
        for l in &live {
            if l.view {
                let Some(sock) = view_socket else {
                    continue;
                };
                let (_, rot, pos) = sock.to_scale_rotation_translation();
                // the keys' offsets turn with the parent object (the first-person model, unrotated in eye space:
                // `ModelScene::place_view_weapon`), not with the socket; the socket gives the place and the
                // emission frame
                // DISASM: clientfx+0x1fe7..0x2003 (the offset frame: the effect object's rotation, replaced by the
                // parent object's when there is one), clientfx+0x2025..0x20a0 (offset = right x ox + up x oy +
                // forward x oz of that rotation), clientfx+0x1f70 case 6 (player-view socket: socket position + offset)
                self.draw_group(
                    l,
                    pos,
                    rot,
                    VIEW_PARENT_ROT,
                    now,
                    Vec3::X,
                    Vec3::Y,
                    &mut out.view,
                );
            } else {
                self.draw_group(l, l.pos, l.rot, l.rot, now, right, up, &mut out.world);
            }
        }
        self.live = live;
        for t in &self.tracers {
            // DISASM: cshell+0x2c0880 / +0x2c0520 (life = distance / velocity; tail = start + dir v (now - t0), head =
            // tail + dir length clamped to the end); alpha from the initial to the final value over the life
            // (PLAUSIBLE, cshell+0x2c2210 hands both to the strip)
            let d = t.from.distance(t.to);
            let dir = (t.to - t.from).normalize_or_zero();
            let age = (now - t.t0).max(0.0) as f32;
            let tail = (t.row.velocity * age).min(d);
            let head = (tail + t.row.length).min(d);
            if head - tail <= 0.0 {
                continue;
            }
            let a = t.from + dir * tail;
            let b = t.from + dir * head;
            let mid = (a + b) * 0.5;
            let to_eye = (camera.pos - mid).normalize_or_zero();
            let side = dir.cross(to_eye).normalize_or_zero() * (t.row.width * 0.5);
            let f = (age * t.row.velocity / d.max(1e-3)).clamp(0.0, 1.0);
            let alpha = t.row.alpha.0 + (t.row.alpha.1 - t.row.alpha.0) * f;
            out.world.push(Quad {
                centre: mid,
                u: (b - a) * 0.5,
                v: side,
                colour: [t.row.colour[0], t.row.colour[1], t.row.colour[2], alpha],
                texture: t.row.texture.clone(),
                // DC-DEFAULT: CLI-197 (proposed) tracers are drawn additively (the effect table's additive key is not
                // read by the 2009 tracer reader, cshell+0x293540; the strip's blend constants are not decoded)
                blend: Blend::Additive,
                depth_test: true,
            });
        }
        let marks = std::mem::take(&mut self.marks);
        for m in &marks {
            let (a, b) = plane_axes(m.normal);
            let (s, c) = m.roll.sin_cos();
            let u = (a * c + b * s) * m.half;
            let v = (b * c - a * s) * m.half;
            let tex = self.textures.frame(&m.texture, 0);
            out.world.push(Quad {
                centre: m.pos + m.normal * 0.25,
                u,
                v,
                // DC-DEFAULT: CLI-633 (proposed) the mark is not lit by the world's lights (2009 MarkLight 1 lets
                // dynamic lights tint it; not ported)
                colour: [MARK_TINT, MARK_TINT, MARK_TINT, mark_alpha(now - m.t0)],
                texture: tex,
                blend: Blend::Alpha,
                depth_test: true,
            });
        }
        self.marks = marks;
        self.stats.world_quads = out.world.len();
        self.stats.view_quads = out.view.len();
        out
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn draw_group(
        &mut self,
        l: &LiveFx,
        pos: Vec3,
        rot: Quat,
        off_rot: Quat,
        now: f64,
        right: Vec3,
        up: Vec3,
        out: &mut Vec<Quad>,
    ) {
        let t = (now - l.t0) as f32;
        let views = key_views(&l.group);
        for (ki, k) in l.group.keys.iter().enumerate() {
            // client-destructfx: a linked key sits on its parent (an anchor moves), see crate::fxkeys
            let kpos = l.motion.key_pos(&views, ki, t, pos, off_rot);
            match &k.kind {
                KeyKind::Particles(p) => {
                    let emit = |te: f32| l.motion.key_pos(&views, ki, te, pos, off_rot);
                    self.draw_particles(l, ki, k, p, &emit, rot, t, (right, up), out)
                }
                KeyKind::Sprite(s) => {
                    if t < k.start || t > k.end {
                        continue;
                    }
                    let span = (k.end - k.start).max(1e-4);
                    let f = ((t - k.start) / span).clamp(0.0, 1.0);
                    let (rgb, alpha) = colour_at(&k.colours, f);
                    let scale = scale_at(&k.scales, f);
                    let Some(path) = s.sprite.as_deref() else {
                        continue;
                    };
                    let ms = ((t - k.start) * 1000.0).max(0.0) as u32;
                    let Some(frame) = self.textures.frame(path, ms) else {
                        continue;
                    };
                    let (w, h) = self.textures.dims(&frame);
                    // DISASM: engine+0x1af8ed..0x1af917 (the sprite quad's half extents = the bound texture's top-mip width
                    // and height, u16 at +4 / +6 of its render record, times the object scale x / y); clientfx+0x2532..0x2585
                    // (the scale key's value is set as the object scale of a sprite or model, all three axes).
                    // CORRECTION 2026-10-26 (client-destructfx, CLI-195 amendment): half size = scale x the texture size
                    // (it was half that)
                    let (hw, hh) = (w * scale, h * scale);
                    if !(hw > 0.0 && hh > 0.0) {
                        continue;
                    }
                    // CLI-1101: a sprite along its normal or in the parent's plane turns with the effect (the bomb
                    // site indicators lie flat on the ground)
                    let (ax, ay) = sprite_axes(s.facing, s.normal, rot, right, up);
                    out.push(Quad {
                        centre: kpos,
                        u: ax * hw,
                        v: ay * hh,
                        colour: [rgb[0] / 255.0, rgb[1] / 255.0, rgb[2] / 255.0, alpha],
                        texture: Some(frame),
                        blend: s.blend,
                        depth_test: !s.no_depth,
                    });
                }
                _ => {}
            }
        }
    }

    /// The particles of one key: emissions at the key start (an interval of zero or less) or every interval while the
    /// key lives; per particle the emission point by the shape, the velocity by the velocity kind, the life between
    /// the two bounds, the colour and size from the keys at its life fraction.
    // EVIDENCE: clientfx+0x1ea00 (emission: per particle a point by the shape type (0 sphere x radius, 1 the system's
    // one point, 2 the emission plane's ring, 3 / 4 cone and disc forms), a velocity between the two bounds per axis or
    // along the point, rotated by the system unless the particles move with it, life between the bounds, a random
    // angle and spin when rotating), clientfx+0x1f3a0 (the per-frame step), clientfx+0x1d1c0 / +0x1d290 (colour and
    // size by the life fraction 1 - remaining / life)
    #[allow(clippy::too_many_arguments)]
    fn draw_particles(
        &mut self,
        l: &LiveFx,
        ki: usize,
        k: &EffectKey,
        p: &ParticleKey,
        emit: &dyn Fn(f32) -> Vec3,
        rot: Quat,
        t: f32,
        (right, up): (Vec3, Vec3),
        out: &mut Vec<Quad>,
    ) {
        if t < k.start || p.per_emission == 0 {
            return;
        }
        let emissions: u32 = if p.interval <= 0.0 {
            1
        } else {
            (((k.end - k.start) / p.interval).ceil() as u32).clamp(1, 64)
        };
        let (pa, pb) = plane_axes(Vec3::from(p.plane));
        for e in 0..emissions {
            let te = k.start + e as f32 * p.interval.max(0.0);
            if te > t || (e > 0 && te >= k.end) {
                break;
            }
            for j in 0..p.per_emission {
                let mut r = Rng::new(l.seed ^ ((ki as u64) << 40) ^ ((e as u64) << 20) ^ j as u64);
                let life = r.range(p.life.0, p.life.1).max(1e-3);
                let age = t - te;
                if age > life {
                    continue;
                }
                let radius = r.range(p.radius.0, p.radius.1);
                let off = match p.shape {
                    1 => Vec3::ZERO,
                    2..=4 => (pa * r.signed() + pb * r.signed()).normalize_or_zero() * radius,
                    _ => Vec3::new(r.signed(), r.signed(), r.signed()).normalize_or_zero() * radius,
                };
                let vmin = Vec3::from(p.vel_min);
                let vmax = Vec3::from(p.vel_max);
                let vel = if p.velocity_kind == 1 {
                    off.normalize_or_zero() * r.range(vmin.x, vmax.x)
                } else {
                    Vec3::new(
                        r.range(vmin.x, vmax.x),
                        r.range(vmin.y, vmax.y),
                        r.range(vmin.z, vmax.z),
                    )
                };
                let (angle0, spin) = if p.rotate {
                    (
                        r.unit() * std::f32::consts::TAU,
                        r.range(p.spin.0, p.spin.1),
                    )
                } else {
                    (0.0, 0.0)
                };
                let pos = if p.with_system {
                    // local frame: positions and velocities turn with the system
                    emit(t) + rot * particle_at(off, vel, Vec3::from(p.accel), p.friction, age)
                } else {
                    // left behind where the emitter was at the emission (a trail of a moving anchor)
                    particle_at(
                        emit(te) + off,
                        rot * vel,
                        Vec3::from(p.accel),
                        p.friction,
                        age,
                    )
                };
                let f = (age / life).clamp(0.0, 1.0);
                let (rgb, alpha) = colour_at(&k.colours, f);
                let size = scale_at(&k.scales, f);
                let half = size * PARTICLE_HALF_PER_SIZE;
                if !(half > 0.0 && half.is_finite() && pos.is_finite()) {
                    continue;
                }
                let ang = angle0 + spin * age;
                let (s, c) = ang.sin_cos();
                let u = (right * c + up * s) * half;
                let v = (up * c - right * s) * half;
                let texture = p
                    .texture
                    .as_deref()
                    .and_then(|tp| self.textures.frame(tp, (age * 1000.0) as u32));
                out.push(Quad {
                    centre: pos,
                    u,
                    v,
                    colour: [rgb[0] / 255.0, rgb[1] / 255.0, rgb[2] / 255.0, alpha],
                    texture,
                    blend: p.blend,
                    depth_test: true,
                });
            }
        }
    }
}

/// A one-shot positional sound at `pos` with outer radius `radius`, in the sound director's server-sound form.
pub fn impact_cue(file: String, pos: Vec3, radius: f32) -> crate::sound::SoundCue {
    crate::sound::SoundCue::Server {
        start: dc_botclient::objects::SoundStart {
            track: None,
            file: 0xFFFF,
            flags: crate::sound::flag::POSITIONAL,
            prio: 0,
            outer: radius.clamp(1.0, u16::MAX as f32) as u16,
            inner_ratio: IMPACT_SOUND_INNER,
            volume: 100,
            pitch: 1.0,
            start_ms: 0,
            tag: 0,
            pos: Some([pos.x, pos.y, pos.z]),
        },
        file: Some(file),
        at_load: false,
    }
}

/// The rotation of the first-person model object in eye space (the parent of the player-view effect keys): none, the
/// model is placed by position only.
// EVIDENCE: cshell+0x232c10 (the row `Pos` places the first-person model in eye space; DISASM)
pub const VIEW_PARENT_ROT: Quat = Quat::IDENTITY;

/// The muzzle socket's transform: the socket whose name key is [`SOCKET_MUZZLE`] (`Flash`) on the model file `model`,
/// posed and placed as the instance is (eye space for the first-person model, world space for a remote player's
/// held weapon, CLI-1591).
pub fn view_muzzle_socket(
    inst: &dc_render::ModelInstance,
    model: &dc_model::Model,
) -> Option<Mat4> {
    let i = model.sockets.iter().position(|s| {
        name_key(s.name.to_text().to_ascii_lowercase().as_bytes()) == SOCKET_MUZZLE
    })?;
    inst.socket_world(model, i)
}

/// The shell type of a thrown object's create blob.
// EVIDENCE: cshell+0x2ca820 / +0x2cab60 (the client's thrown-object record only attaches the projectile row's trail
// group to the server's engine object; the model is the server object's own model block)
// SPEC: UDP_SESSION_2009_B.md §3.5 (a kind-1 object's model block; the thrown object's create, type 0x13 blob)
pub const THROWN_BLOB: u8 = 0x13;

// --------------------------------------------------------------------------------------------- the GPU pass

const FX_WGSL: &str = r#"
struct Frame {
    clip_from_space: mat4x4<f32>,
    // w2-fx: view depth = dot(depth_row.xyz, pos) + depth_row.w; the world pass's linear fog (x far, y 1 / (far -
    // near), z 1 when on) and its colour
    depth_row: vec4<f32>,
    fog: vec4<f32>,
    fog_colour: vec4<f32>,
};
@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) colour: vec4<f32>,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) colour: vec4<f32>,
    @location(2) depth: f32,
};

@vertex
fn vs_fx(v: VIn) -> VOut {
    var o: VOut;
    o.pos = frame.clip_from_space * vec4<f32>(v.pos, 1.0);
    o.uv = v.uv;
    o.colour = v.colour;
    o.depth = dot(frame.depth_row.xyz, v.pos) + frame.depth_row.w;
    return o;
}

// the share of the quad's colour that stays (public Direct3D 9 linear fog); 1 with the fog off
fn fog_keep(depth: f32) -> f32 {
    if (frame.fog.z > 0.5) {
        return clamp((frame.fog.x - depth) * frame.fog.y, 0.0, 1.0);
    }
    return 1.0;
}

// alpha-blended quads fog toward the scene's fog colour
@fragment
fn fs_fx(v: VOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, v.uv) * v.colour;
    return vec4<f32>(mix(frame.fog_colour.rgb, c.rgb, fog_keep(v.depth)), c.a);
}

// additive quads fog toward black
@fragment
fn fs_fx_add(v: VOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, v.uv) * v.colour;
    return vec4<f32>(c.rgb * fog_keep(v.depth), c.a);
}

// multiplying quads fog toward white
@fragment
fn fs_fx_mul(v: VOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, v.uv) * v.colour;
    return vec4<f32>(mix(vec3<f32>(1.0), c.rgb, fog_keep(v.depth)), c.a);
}
"#;

/// The frame uniform of the effect pass (w2-fx: the depth row and the fog after the matrix).
// SPEC: RENDER_2009.md §7.1 (the object blend helper engine+0x1abc80 used by sprites and particles: additive objects
// fog toward black, multiplying ones toward white, the others toward the scene's fog colour; object flag 0x80 turns
// the fog off for the object), §3.3 (linear fog in world units on the view depth)
// DC-DEFAULT: CLI-518 (proposed) every effect quad takes the world pass's fog by its blend (the per-object fog-off flag
// of the effect keys is not traced, so no key turns it off); quads on the first-person model fog by their eye-space
// depth
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct FxFrame {
    clip_from_space: [f32; 16],
    depth_row: [f32; 4],
    fog: [f32; 4],
    fog_colour: [f32; 4],
}

impl FxFrame {
    fn new(clip: Mat4, depth_row: [f32; 4], fog: Option<&dc_render::Fog>) -> FxFrame {
        let (fog, fog_colour) = fx_fog_uniform(fog);
        FxFrame {
            clip_from_space: clip.to_cols_array(),
            depth_row,
            fog,
            fog_colour,
        }
    }
}

/// The fog uniform `[far, 1 / (far - near), 1, 0]` and the colour in 0..1 (all zero with the fog off or near = far).
// SPEC: RENDER_2009.md §3.3 (fog off when near == far; FOGSTART / FOGEND in world units)
pub fn fx_fog_uniform(fog: Option<&dc_render::Fog>) -> ([f32; 4], [f32; 4]) {
    match fog {
        Some(f) if (f.far - f.near).abs() > 1e-6 && f.far.is_finite() && f.near.is_finite() => {
            let c = f.colour.map(|v| (v / 255.0).clamp(0.0, 1.0));
            (
                [f.far, 1.0 / (f.far - f.near), 1.0, 0.0],
                [c[0], c[1], c[2], 1.0],
            )
        }
        _ => ([0.0; 4], [0.0, 0.0, 0.0, 1.0]),
    }
}

/// The CPU form of the shader's fog for one quad colour (tests and tools): `keep` from the fog at `depth`, the colour
/// moved toward the blend's fog colour.
pub fn fx_fogged(
    colour: [f32; 3],
    blend: Blend,
    fog: Option<&dc_render::Fog>,
    depth: f32,
) -> [f32; 3] {
    let keep = fog.map_or(1.0, |f| f.factor(depth));
    let toward = match (blend, fog) {
        (Blend::Additive, _) | (_, None) => [0.0; 3],
        (Blend::Multiply, _) => [1.0; 3],
        (Blend::Alpha, Some(f)) => f.colour.map(|v| (v / 255.0).clamp(0.0, 1.0)),
    };
    [0, 1, 2].map(|i| toward[i] * (1.0 - keep) + colour[i] * keep)
}

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct FxVertex {
    pos: [f32; 3],
    uv: [f32; 2],
    colour: [f32; 4],
}

const FX_DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The effect quads drawn into the finished frame: world quads with the main camera in the world depth range (tested
/// against the world, not written), eye-space quads with the player-view camera in its depth range, after the
/// first-person model.
// SPEC: RENDER_2009.md §8.1 / §8.2 (camera-facing textured quads, blended, depth tested unless asked not to), §4.2 /
// §4.3 (the player-view camera and the depth ranges [0.1, 1] and [0, 0.1])
pub struct FxPass {
    pipelines: HashMap<(Blend, bool), wgpu::RenderPipeline>,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    world_uniform: wgpu::Buffer,
    view_uniform: wgpu::Buffer,
    world_group: wgpu::BindGroup,
    view_group: wgpu::BindGroup,
    white: wgpu::BindGroup,
    textures: HashMap<String, Option<wgpu::BindGroup>>,
    vertices: wgpu::Buffer,
    capacity: usize,
    /// CLI-1593: samples per pixel of the frame target the pipelines draw into (the world renderer's count; 1 off).
    msaa: u32,
    /// Quads drawn by the last [`FxPass::encode`].
    pub last_drawn: usize,
}

impl std::fmt::Debug for FxPass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FxPass")
            .field("textures", &self.textures.len())
            .field("last_drawn", &self.last_drawn)
            .finish()
    }
}

fn blend_state(b: Blend) -> wgpu::BlendState {
    use wgpu::{BlendComponent, BlendFactor as F, BlendOperation as O};
    match b {
        Blend::Alpha => wgpu::BlendState::ALPHA_BLENDING,
        Blend::Additive => wgpu::BlendState {
            color: BlendComponent {
                src_factor: F::SrcAlpha,
                dst_factor: F::One,
                operation: O::Add,
            },
            alpha: BlendComponent {
                src_factor: F::Zero,
                dst_factor: F::One,
                operation: O::Add,
            },
        },
        Blend::Multiply => wgpu::BlendState {
            color: BlendComponent {
                src_factor: F::Dst,
                dst_factor: F::Zero,
                operation: O::Add,
            },
            alpha: BlendComponent {
                src_factor: F::Zero,
                dst_factor: F::One,
                operation: O::Add,
            },
        },
    }
}

impl FxPass {
    /// The pass for a single-sample target (MSAA off).
    pub fn new(ctx: &dc_render::GpuContext, format: wgpu::TextureFormat) -> FxPass {
        Self::with_msaa(ctx, format, 1)
    }

    /// CLI-1593: the pass for the frame's 3D target of `format` with `msaa` samples per pixel (the world renderer's
    /// [`dc_render::WorldRenderer::msaa`]; the pipelines must match the attachments they draw into).
    pub fn with_msaa(ctx: &dc_render::GpuContext, format: wgpu::TextureFormat, msaa: u32) -> FxPass {
        let msaa = msaa.max(1);
        let device = &ctx.device;
        let module = dc_render::shader_module(
            device,
            wgpu::ShaderModuleDescriptor {
                label: Some("fx.wgsl"),
                source: wgpu::ShaderSource::Wgsl(FX_WGSL.into()),
            },
        );
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let uniform = |label| {
            dc_render::perf::buffer(
                device,
                &wgpu::BufferDescriptor {
                    label: Some(label),
                    size: std::mem::size_of::<FxFrame>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
            )
        };
        let world_uniform = uniform("fx world frame");
        let view_uniform = uniform("fx view frame");
        let group = |buf: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fx frame"),
                layout: &frame_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            })
        };
        let world_group = group(&world_uniform);
        let view_group = group(&view_uniform);
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx"),
            bind_group_layouts: &[&frame_layout, &tex_layout],
            push_constant_ranges: &[],
        });
        let vlayout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<FxVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4],
        };
        let mut pipelines = HashMap::new();
        for b in [Blend::Alpha, Blend::Additive, Blend::Multiply] {
            for test in [true, false] {
                let p = dc_render::render_pipeline(
                    device,
                    &wgpu::RenderPipelineDescriptor {
                        label: Some("fx quads"),
                        layout: Some(&pl),
                        vertex: wgpu::VertexState {
                            module: &module,
                            entry_point: Some("vs_fx"),
                            compilation_options: Default::default(),
                            buffers: std::slice::from_ref(&vlayout),
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &module,
                            entry_point: Some(match b {
                                Blend::Alpha => "fs_fx",
                                Blend::Additive => "fs_fx_add",
                                Blend::Multiply => "fs_fx_mul",
                            }),
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState {
                                format,
                                blend: Some(blend_state(b)),
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                        }),
                        primitive: wgpu::PrimitiveState::default(),
                        depth_stencil: Some(wgpu::DepthStencilState {
                            format: FX_DEPTH,
                            depth_write_enabled: false,
                            depth_compare: if test {
                                wgpu::CompareFunction::LessEqual
                            } else {
                                wgpu::CompareFunction::Always
                            },
                            stencil: wgpu::StencilState::default(),
                            bias: wgpu::DepthBiasState::default(),
                        }),
                        // CLI-1593: the sample count of the frame's 3D target (1: the default state, MSAA off)
                        multisample: dc_render::multisample_state(msaa),
                        multiview: None,
                        cache: None,
                    },
                );
                pipelines.insert((b, test), p);
            }
        }
        let sampler = dc_render::sampler(
            device,
            &wgpu::SamplerDescriptor {
                label: Some("fx sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            },
        );
        let white_tex = dc_render::perf::texture_with_data(
            device,
            &ctx.queue,
            &wgpu::TextureDescriptor {
                label: Some("fx white"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255, 255, 255, 255],
        );
        let white = Self::tex_group(device, &tex_layout, &sampler, &white_tex);
        let capacity = 1024;
        FxPass {
            pipelines,
            vertices: Self::vertex_buffer(device, capacity),
            capacity,
            tex_layout,
            sampler,
            world_uniform,
            view_uniform,
            world_group,
            view_group,
            white,
            textures: HashMap::new(),
            msaa,
            last_drawn: 0,
        }
    }

    /// Samples per pixel the pipelines were built for (1: MSAA off).
    pub fn msaa(&self) -> u32 {
        self.msaa
    }

    fn vertex_buffer(device: &wgpu::Device, quads: usize) -> wgpu::Buffer {
        dc_render::perf::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("fx vertices"),
                size: (quads.max(1) * 6 * std::mem::size_of::<FxVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        )
    }

    fn tex_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        tex: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        let view = tex.create_view(&Default::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fx texture"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    fn texture(
        &mut self,
        ctx: &dc_render::GpuContext,
        store: &mut TextureStore,
        path: &str,
    ) -> bool {
        if !self.textures.contains_key(path) {
            dc_render::perf::add(dc_render::perf::Counter::TextureLoads, 1);
            let g = store.image(path).and_then(|img| {
                let (w, h) = (img.width.max(1), img.height.max(1));
                let px = img.levels.first()?.get(..(w * h * 4) as usize)?;
                let tex = dc_render::perf::texture_with_data(
                    &ctx.device,
                    &ctx.queue,
                    &wgpu::TextureDescriptor {
                        label: Some("fx texture"),
                        size: wgpu::Extent3d {
                            width: w,
                            height: h,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    px,
                );
                Some(Self::tex_group(
                    &ctx.device,
                    &self.tex_layout,
                    &self.sampler,
                    &tex,
                ))
            });
            self.textures.insert(path.to_string(), g);
        }
        self.textures.get(path).is_some_and(|g| g.is_some())
    }

    fn quad_vertices(q: &Quad, out: &mut Vec<FxVertex>) {
        let c = |du: f32, dv: f32| (q.centre + q.u * du + q.v * dv).to_array();
        let col = q.colour;
        let v = |p: [f32; 3], uv: [f32; 2]| FxVertex {
            pos: p,
            uv,
            colour: col,
        };
        let (a, b, cc, d) = (
            v(c(-1.0, 1.0), [0.0, 0.0]),
            v(c(1.0, 1.0), [1.0, 0.0]),
            v(c(1.0, -1.0), [1.0, 1.0]),
            v(c(-1.0, -1.0), [0.0, 1.0]),
        );
        out.extend_from_slice(&[a, b, cc, a, cc, d]);
    }

    /// Records the effect quads over `target` (colour and depth loaded; the world and first-person passes ran first).
    /// CLI-1593: with `target.resolve` set this is the frame's last 3D pass and resolves the multisampled frame;
    /// with nothing to draw it then records the resolve alone, so the frame resolves exactly once either way.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        ctx: &dc_render::GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: dc_render::FrameTarget<'_>,
        camera: &dc_render::Camera,
        draw: &FxDraw,
        store: &mut TextureStore,
        fog: Option<&dc_render::Fog>,
    ) {
        self.last_drawn = 0;
        let (w, h) = target.size;
        if (draw.world.is_empty() && draw.view.is_empty()) || w == 0 || h == 0 {
            if w != 0 && h != 0 {
                target.encode_resolve_only(encoder);
            }
            return;
        }
        // world quads back to front, eye quads after them
        let mut world: Vec<&Quad> = draw
            .world
            .iter()
            .filter(|q| q.centre.is_finite() && q.u.is_finite() && q.v.is_finite())
            .collect();
        // SPEC: RENDER_2009.md §8.1 (objects that ignore depth are drawn last, with Z off)
        world.sort_by(|a, b| {
            a.depth_test.cmp(&b.depth_test).reverse().then(
                b.centre
                    .distance_squared(camera.pos)
                    .total_cmp(&a.centre.distance_squared(camera.pos)),
            )
        });
        let view: Vec<&Quad> = draw
            .view
            .iter()
            .filter(|q| q.centre.is_finite() && q.u.is_finite() && q.v.is_finite())
            .collect();
        let mut verts = Vec::with_capacity((world.len() + view.len()) * 6);
        let mut batches: Vec<(bool, Blend, bool, Option<String>, u32, u32)> = Vec::new();
        for (space, list) in [(false, &world), (true, &view)] {
            for q in list.iter() {
                let tex = match q.texture.as_deref() {
                    Some(p) if self.texture(ctx, store, p) => Some(p.to_string()),
                    Some(_) => continue,
                    None => None,
                };
                let first = verts.len() as u32;
                Self::quad_vertices(q, &mut verts);
                match batches.last_mut() {
                    Some(b)
                        if b.0 == space && b.1 == q.blend && b.2 == q.depth_test && b.3 == tex =>
                    {
                        b.5 += 6
                    }
                    _ => batches.push((space, q.blend, q.depth_test, tex, first, 6)),
                }
            }
        }
        if verts.is_empty() {
            target.encode_resolve_only(encoder);
            return;
        }
        let quads = verts.len() / 6;
        crate::prof::FX_QUADS.fetch_add(quads as u64, std::sync::atomic::Ordering::Relaxed);
        if quads > self.capacity {
            self.capacity = quads.next_power_of_two();
            self.vertices = Self::vertex_buffer(&ctx.device, self.capacity);
            dc_render::perf::regrow();
        }
        dc_render::perf::write_buffer(&ctx.queue, &self.vertices, 0, bytemuck::cast_slice(&verts));
        // the view depth of a world point: its eye-space z (the camera's forward axis, left-handed)
        let fwd = camera.rot * Vec3::Z;
        let world_frame = FxFrame::new(
            camera.view_projection(),
            [fwd.x, fwd.y, fwd.z, -fwd.dot(camera.pos)],
            fog,
        );
        dc_render::perf::write_buffer(
            &ctx.queue,
            &self.world_uniform,
            0,
            bytemuck::bytes_of(&world_frame),
        );
        let pv = dc_render::PLAYER_VIEW_DEFAULT.projection(w, h);
        let view_frame = FxFrame::new(pv, [0.0, 0.0, 1.0, 0.0], fog);
        dc_render::perf::write_buffer(
            &ctx.queue,
            &self.view_uniform,
            0,
            bytemuck::bytes_of(&view_frame),
        );
        let stamp = dc_render::perf::pass_stamp("fx");
        // CLI-1593: colour and depth loaded; stored, or resolved and discarded when this pass ends the 3D frame
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fx quads"),
            color_attachments: &[target.colour_attachment(wgpu::LoadOp::Load)],
            depth_stencil_attachment: target.depth_attachment(wgpu::LoadOp::Load),
            timestamp_writes: stamp.as_ref().map(dc_render::perf::PassStamp::writes),
            occlusion_query_set: None,
        });
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        for (space, blend, test, tex, first, count) in &batches {
            let range = if *space {
                dc_render::PLAYER_VIEW_DEPTH_RANGE
            } else {
                dc_render::WORLD_DEPTH_RANGE
            };
            pass.set_viewport(0.0, 0.0, w as f32, h as f32, range.0, range.1);
            let Some(p) = self.pipelines.get(&(*blend, *test)) else {
                continue;
            };
            pass.set_pipeline(p);
            pass.set_bind_group(
                0,
                if *space {
                    &self.view_group
                } else {
                    &self.world_group
                },
                &[],
            );
            let g = tex
                .as_deref()
                .and_then(|t| self.textures.get(t).and_then(|g| g.as_ref()))
                .unwrap_or(&self.white);
            pass.set_bind_group(1, g, &[]);
            pass.draw(*first..*first + *count, 0..1);
            dc_render::perf::add(dc_render::perf::Counter::ClientDraws, 1);
        }
        self.last_drawn = quads;
    }
}

// --------------------------------------------------------------------------------------------- the frame stage

/// What the frame loops hold: the runner, its GPU pass (created with the first frame drawn) and the first-person
/// models' socket files.
pub struct FxStage {
    pub runner: FxRunner,
    pass: Option<FxPass>,
    vfs: Option<RezVfs>,
    view_models: HashMap<String, Option<dc_model::Model>>,
    /// The quads of the last [`FxStage::encode`].
    pub last: FxDraw,
    /// w2-fx: the world pass's linear fog, taken by [`FxStage::frame`] from the loaded world's scene.
    fog: Option<dc_render::Fog>,
    /// w2-sprites: the billboards of the last frame ([`crate::sprites`]) and the quads they drew in the last encode.
    billboards: Vec<crate::sprites::Billboard>,
    pub billboard_quads: usize,
    /// CLI-1593: samples per pixel of the frame target ([`FxStage::set_msaa`]); the pass is built with it.
    msaa: u32,
    /// `last` holds this frame's quads already ([`FxStage::prepare`] ran; the next encode records them).
    prepared: bool,
}

impl std::fmt::Debug for FxStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FxStage")
            .field("runner", &self.runner)
            .finish()
    }
}

impl FxStage {
    pub fn open(rom: Option<&Path>) -> FxStage {
        FxStage {
            runner: FxRunner::open(rom),
            pass: None,
            vfs: rom.map(RezVfs::new),
            view_models: HashMap::new(),
            last: FxDraw::default(),
            fog: None,
            billboards: Vec::new(),
            billboard_quads: 0,
            msaa: 1,
            prepared: false,
        }
    }

    /// CLI-1593: the sample count of the frame target the effect pass draws into (the world renderer's
    /// [`dc_render::WorldRenderer::msaa`]). A change puts the pass away; the next encode builds it with the new count.
    pub fn set_msaa(&mut self, msaa: u32) {
        let msaa = msaa.max(1);
        if msaa != self.msaa {
            self.msaa = msaa;
            self.pass = None;
        }
    }

    pub fn msaa(&self) -> u32 {
        self.msaa
    }

    /// Builds this frame's quads (the effects at `now_ms`, the billboards) ahead of [`FxStage::encode`] and says
    /// whether there is anything to draw. CLI-1593: the frame loop asks before it records the world pass, so it
    /// knows which pass ends the 3D frame and resolves the multisampled target. The encode after it records these
    /// quads without building them again (the runner's draw consumes its per-frame state).
    pub fn prepare(
        &mut self,
        camera: &dc_render::Camera,
        now_ms: u32,
        view_socket: Option<Mat4>,
    ) -> bool {
        self.last = self.runner.draw(now_ms, camera, view_socket);
        // w2-sprites: the billboards are sprite quads of the same pass (translucent, sorted with the effects)
        let (world_b, view_b) = crate::sprites::quads_with_store(
            &self.billboards,
            camera,
            now_ms,
            &mut self.runner.textures,
        );
        self.billboard_quads = world_b.len() + view_b.len();
        self.last.world.extend(world_b);
        self.last.view.extend(view_b);
        self.prepared = true;
        !self.last.world.is_empty() || !self.last.view.is_empty()
    }

    /// gpu-recover: the effect pass of a lost GPU device is put away; the next encode builds it on the new device. The
    /// running effects stay.
    pub fn gpu_reset(&mut self) {
        if let Some(p) = self.pass.take() {
            crate::gpulost::retire(p);
        }
    }

    /// The frame's events (see [`FxRunner::feed`]); the returned cues go to the sound director with the frame's.
    pub fn frame(
        &mut self,
        now_ms: u32,
        fs: &crate::world::FrameState,
        world: &crate::world::LoadedWorld,
    ) -> Vec<crate::sound::SoundCue> {
        self.fog = world.scene.as_ref().and_then(|s| s.fog);
        self.billboards.clone_from(&fs.billboards);
        self.runner.feed(now_ms, &fs.fx_events, Some(world))
    }

    /// The effects' part of the frame's camera (w2-fx), called once the frame's eye is final: returns the eye moved by
    /// the running shake keys ([`FxRunner::shake_at`]) and hands the running light keys
    /// ([`FxRunner::lights_at`], at the moved eye) to the world's model lighting for this frame's light sets.
    pub fn camera_frame(
        &self,
        now_ms: u32,
        eye: dc_sim::V3,
        world: &crate::world::LoadedWorld,
    ) -> dc_sim::V3 {
        let cam = Vec3::new(eye.x, eye.y, eye.z);
        let cam = crate::camfx::shaken_eye(cam, self.runner.shake_at(now_ms, cam));
        if let Some(l) = world.model_lights.as_ref() {
            l.set_fx_lights(self.runner.lights_at(now_ms, cam));
        }
        dc_sim::V3::new(cam.x, cam.y, cam.z)
    }

    /// The eye-space muzzle socket of the first-person model `models` drew this frame.
    pub fn view_socket(&mut self, models: &crate::characters::ModelScene) -> Option<Mat4> {
        let path = models.view_model_path()?.to_string();
        let inst = models.view_placed()?;
        if !self.view_models.contains_key(&path) {
            let m = self.vfs.as_ref().and_then(|v| {
                let b = dc_render::read_rom_entry(v, &path).ok()?;
                dc_model::Model::parse(&b).ok()
            });
            self.view_models.insert(path.clone(), m);
        }
        let m = self.view_models.get(&path)?.as_ref()?;
        view_muzzle_socket(inst, m)
    }

    /// Records the effect quads of `now_ms` after the world, first-person and box passes (the quads of
    /// [`FxStage::prepare`] when it ran this frame, else built here). CLI-1593: the pass is built for the target's
    /// sample count ([`FxStage::set_msaa`]); with `target.resolve` set it resolves the frame (see [`FxPass::encode`]).
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        ctx: &dc_render::GpuContext,
        format: wgpu::TextureFormat,
        encoder: &mut wgpu::CommandEncoder,
        target: dc_render::FrameTarget<'_>,
        camera: &dc_render::Camera,
        now_ms: u32,
        view_socket: Option<Mat4>,
    ) {
        if !self.prepared {
            self.prepare(camera, now_ms, view_socket);
        }
        self.prepared = false;
        let msaa = self.msaa;
        let pass = self
            .pass
            .get_or_insert_with(|| FxPass::with_msaa(ctx, format, msaa));
        pass.encode(
            ctx,
            encoder,
            target,
            camera,
            &self.last,
            &mut self.runner.textures,
            self.fog.as_ref(),
        );
    }

    /// The fog the effect quads take (w2-fx: the world pass's, from the last [`FxStage::frame`]).
    pub fn fog(&self) -> Option<dc_render::Fog> {
        self.fog
    }

    /// Quads drawn by the last frame.
    pub fn drawn(&self) -> usize {
        self.pass.as_ref().map_or(0, |p| p.last_drawn)
    }
}

// --------------------------------------------------------------------------------------------- client-destructfx

/// The motion view of a group's keys ([`crate::fxkeys::GroupMotion`]).
pub fn key_views(g: &EffectGroup) -> Vec<crate::fxkeys::KeyView<'_>> {
    g.keys
        .iter()
        .map(|k| crate::fxkeys::KeyView {
            start: k.start,
            end: k.end,
            offset: k.offset,
            id: k.id,
            link: k.link,
            anchor: match &k.kind {
                KeyKind::Anchor(a) => Some(a),
                _ => None,
            },
        })
        .collect()
}

impl FxRunner {
    /// Advances the anchors of the world groups to `now` (fixed steps, bounces against `world`'s collision set) and
    /// plays the sound keys that came due (one positional cue each, at the key's place when it starts).
    // EVIDENCE: clientfx+0x1cae0 / +0x1cd50 (anchors), clientfx+0x20ec0 (sound key: file, inner and outer radius, volume)
    // DC-DEFAULT: CLI-361 (proposed) effect groups on the first-person model move no anchors and play no sound keys
    fn step_motion(
        &mut self,
        now: f64,
        world: Option<&crate::world::LoadedWorld>,
        cues: &mut Vec<crate::sound::SoundCue>,
    ) {
        let ray = |a: Vec3, b: Vec3| -> Option<Vec3> {
            let w = world?;
            let h = w.collision.ray(
                dc_sim::V3::new(a.x, a.y, a.z),
                dc_sim::V3::new(b.x, b.y, b.z),
            )?;
            Some(Vec3::new(h.normal.x, h.normal.y, h.normal.z))
        };
        for l in &mut self.live {
            if l.view {
                continue;
            }
            let t = (now - l.t0) as f32;
            let group = l.group.clone();
            let views = key_views(&group);
            let before = l.motion.anchors.iter().flatten().count();
            l.motion.advance(&views, t, l.pos, l.rot, l.seed, &ray);
            self.stats.anchors += (l.motion.anchors.iter().flatten().count() - before) as u64;
            for a in l.motion.anchors.iter().flatten() {
                let y0 = a.samples.first().map_or(0.0, |p| p.y);
                let top = a.samples.iter().map(|p| p.y).fold(y0, f32::max);
                self.stats.anchor_rise = self.stats.anchor_rise.max((top - y0).round() as i32);
            }
            for (ki, k) in group.keys.iter().enumerate() {
                let KeyKind::Sound(sk) = &k.kind else {
                    continue;
                };
                if l.motion.sounded[ki] || t < k.start {
                    continue;
                }
                l.motion.sounded[ki] = true;
                let at = l.motion.key_pos(&views, ki, k.start, l.pos, l.rot);
                cues.push(key_sound_cue(sk, at));
                self.stats.sound_keys += 1;
            }
        }
    }

    /// The models the world groups draw at `now_ms` (the model keys that live then, each at its key's place with the
    /// group's rotation).
    // EVIDENCE: clientfx+0x17290 (model key: file and skins); clientfx+0x1f70 (a key placed by following its parent
    // takes the parent object's position each update); DISASM cshell+0x26d970 (a linked key's parent is the object of
    // the key it links to)
    // DC-DEFAULT: CLI-362 (proposed) a model key keeps the group's rotation and its scale keys are not applied (the
    // ROM's breaking groups hold every scale key at 1)
    pub fn models_at(&mut self, now_ms: u32) -> Vec<crate::fxkeys::FxModel> {
        let now = now_ms as f64 / 1000.0;
        let mut out = Vec::new();
        for l in &self.live {
            if l.view {
                continue;
            }
            let t = (now - l.t0) as f32;
            let views = key_views(&l.group);
            for (ki, k) in l.group.keys.iter().enumerate() {
                let KeyKind::Model(m) = &k.kind else {
                    continue;
                };
                if t < k.start || t > k.end {
                    continue;
                }
                let pos = l.motion.key_pos(&views, ki, t, l.pos, l.rot);
                if !pos.is_finite() {
                    continue;
                }
                out.push(crate::fxkeys::FxModel {
                    id: l.seed.rotate_left(12) ^ ki as u64,
                    model: crate::world::ModelRef {
                        model: m.model.clone(),
                        textures: m.skins.clone(),
                    },
                    styles: m.styles.clone(),
                    pos,
                    rot: l.rot,
                    anim: m.anim,
                    elapsed: t - k.start,
                    life: k.end - k.start,
                });
            }
        }
        self.stats.fx_models_max = self.stats.fx_models_max.max(out.len());
        out
    }
}

/// The one-shot positional cue of a sound key.
// EVIDENCE: clientfx+0x20ec0 (inner and outer radius, volume)
pub fn key_sound_cue(k: &crate::fxkeys::SoundKey, pos: Vec3) -> crate::sound::SoundCue {
    crate::sound::SoundCue::Server {
        start: dc_botclient::objects::SoundStart {
            track: None,
            file: 0xFFFF,
            flags: crate::sound::flag::POSITIONAL,
            prio: 0,
            outer: k.outer.clamp(1.0, u16::MAX as f32) as u16,
            inner_ratio: k.inner_ratio(),
            volume: k.volume,
            pitch: 1.0,
            start_ms: 0,
            tag: 0,
            pos: Some([pos.x, pos.y, pos.z]),
        },
        file: Some(k.file.clone()),
        at_load: false,
    }
}

/// The bodies the model pass draws: the remote players followed by the thrown objects.
pub fn with_thrown<'a>(
    remotes: &'a [crate::world::RemoteBody],
    thrown: &[crate::world::RemoteBody],
) -> std::borrow::Cow<'a, [crate::world::RemoteBody]> {
    if thrown.is_empty() {
        std::borrow::Cow::Borrowed(remotes)
    } else {
        let mut v = remotes.to_vec();
        v.extend(thrown.iter().cloned());
        std::borrow::Cow::Owned(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Made-up tables (names and numbers are ours, not the ROM's).
    fn tables() -> FxTables {
        let w = ButeDoc::parse(
            "[Weapon1]\nPVMuzzleFXName = \"flashA\"\nHHMuzzleFXName = \"flashB\"\nMuzzlePos = <1.0, -2.0, 3.0>\n\
             [Weapon2]\nMuzzlePos = <0.000000,0.000000,0.000000>\n\
             [Ammo1]\nImpactFXName = \"hitA\"\nTracerFXName = \"trA\"\nType = 0\nInstDamageType = 2\n\
             [Ammo2]\nImpactFXName = \"boomA\"\nType = 2\n",
        );
        let fx = ButeDoc::parse(
            "[TracerFX0]\nName = \"trA\"\nTexture = \"t.dtx\"\nFrequency = 2\nVelocity = 1000.0\nWidth = 4\n\
             InitialAlpha = 1.0\nFinalAlpha = 0.5\nColor = <255.0, 128.0, 0.0>\nLength = 100.0\n\
             [ImpactFX0]\nName = \"hitA\"\nSound = \"surface\"\nSoundRadius = 500\nDoSurfaceFX = 1\nCreateMark = 1\n\
             MarkScale = 0.5\nMark = \"surface\"\n\
             [ImpactFX1]\nName = \"boomA\"\nSound = \"boom.wav\"\nSoundRadius = 1500\nDoSurfaceFX = 1\n",
        );
        let s = ButeDoc::parse(
            "[Surface0]\nId = 10\nImpactFXName = \"dust\"\nGrenadeFXName = \"bigdust\"\nShowsMark = 1\n\
             BulletHoleSpr = \"hole.spr\"\nBulletHoleMinScale = 1.0\nBulletHoleMaxScale = 1.0\n\
             BulletImpactSnd1 = \"a.wav\"\nBulletImpactSnd2 = \"b.wav\"\n\
             [Surface1]\nId = 110\n",
        );
        FxTables::from_docs(Some(&w), Some(&fx), Some(&s))
    }

    /// The user's round 4: a suppressor in the muzzle socket swaps the weapon row's muzzle flash for the mod row's
    /// silenced group, first person and third person; the user's ROM has no effect group of that name, so the 2009
    /// lookup finds nothing and a suppressed shot has no flash; without a suppressor the row's flash plays (CLI-619).
    #[test]
    fn rom_a_suppressor_picks_the_silenced_muzzle_group() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let mut r = FxRunner::open(Some(&rom));
        for (weapon, silencer) in [(27u16, 33u16), (164, 34), (25, 31)] {
            let pv = r.tables.muzzle_group(weapon, Some(silencer), false);
            let hh = r.tables.muzzle_group(weapon, Some(silencer), true);
            let pv0 = r.tables.muzzle_group(weapon, None, false);
            let hh0 = r.tables.muzzle_group(weapon, None, true);
            println!("{weapon}: with {silencer} {pv:?} / {hh:?}, without {pv0:?} / {hh0:?}");
            assert_eq!(pv.as_deref(), Some("silenced01_pv_muzz"));
            assert_eq!(hh.as_deref(), Some("silenced01_hh_muzz"));
            assert_ne!(pv0, pv, "{weapon}: the plain flash differs");
            assert_ne!(hh0, hh);
            assert!(r.library.get("silenced01_pv_muzz").is_none());
            // an own first shot: the plain flash starts, the suppressed one does not
            for (m, want) in [(None, 1u64), (Some(silencer), 0)] {
                let before = r.stats.muzzle;
                let shot = crate::arms::ShotFx {
                    weapon,
                    ammo: weapon,
                    origin: [0.0; 3],
                    muzzle: [0.0; 3],
                    dir: [0.0, 0.0, 1.0],
                    range: 100.0,
                    hit: crate::arms::RayHit::Miss,
                    first: true,
                    time_ms: 1000,
                    impact: None,
                    flash_mod: m,
                };
                r.feed(1000, &[FxEvent::Shot(shot)], None);
                assert_eq!(r.stats.muzzle - before, want, "{weapon} with {m:?}");
            }
        }
        // a mod row without a muzzle name: no flash at all (the 2009 loader keeps "" for the missing key)
        let mut t = FxTables::default();
        t.muzzle.insert(1, (Some("a".into()), Some("b".into())));
        t.mod_muzzle.insert(9, (String::new(), String::new()));
        assert_eq!(t.muzzle_group(1, Some(9), false), None);
        assert_eq!(t.muzzle_group(1, None, false).as_deref(), Some("a"));
    }

    #[test]
    fn tables_link_ammo_tracer_impact_and_surface_rows() {
        let t = tables();
        let tr = t.tracer(1).expect("tracer");
        assert_eq!(tr.frequency, 2);
        assert_eq!(tr.colour, [1.0, 128.0 / 255.0, 0.0]);
        let i = t.impact(1).expect("impact");
        assert!(i.surface_sound && i.mark_surface && i.surface_fx && i.create_mark);
        assert_eq!(t.impact(2).unwrap().sound.as_deref(), Some("boom.wav"));
        assert_eq!(t.ammo_fx(2).unwrap().ammo_type, AMMO_THROWN);
        assert_eq!(t.muzzle[&1].0.as_deref(), Some("flasha"));
        assert!(t.melee(2) && !t.melee(1));
        assert_eq!(t.surfaces[&10].bullet_sounds.len(), 2);
    }

    #[test]
    fn colour_and_scale_keys_interpolate_linearly() {
        let k = [
            ColourKey {
                t: 0.0,
                rgb: [255.0, 0.0, 0.0],
                alpha: 1.0,
            },
            ColourKey {
                t: 0.5,
                rgb: [0.0, 255.0, 0.0],
                alpha: 0.0,
            },
        ];
        let (c, a) = colour_at(&k, 0.25);
        assert!((c[0] - 127.5).abs() < 1e-3 && (c[1] - 127.5).abs() < 1e-3);
        assert!((a - 0.5).abs() < 1e-6);
        assert_eq!(colour_at(&k, 0.9), ([0.0, 255.0, 0.0], 0.0));
        assert_eq!(scale_at(&[(0.0, 2.0), (1.0, 4.0)], 0.5), 3.0);
        assert_eq!(scale_at(&[], 0.5), 1.0);
    }

    #[test]
    fn particles_follow_the_closed_form_step() {
        // no friction loss: ballistic
        let p = particle_at(
            Vec3::ZERO,
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, -2.0, 0.0),
            1.0,
            2.0,
        );
        assert!((p - Vec3::new(20.0, -4.0, 0.0)).length() < 1e-4);
        // with friction the particle travels less far
        let q = particle_at(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::ZERO, 0.5, 2.0);
        assert!(q.x > 0.0 && q.x < 20.0);
    }

    fn runner() -> FxRunner {
        FxRunner::new(tables(), EffectLibrary::default(), TextureStore::default())
    }

    fn record(own: bool, first: u16, ammo: u16, surface: u8, clear: u16) -> ShotRecord {
        ShotRecord {
            shooter: 9,
            first,
            weapon: 1,
            ammo,
            from: [0.0, 0.0, 0.0],
            to: [0.0, 0.0, 500.0],
            normal: [0.0, 0.0, -1.0],
            surface,
            clear,
            own,
            mods: [0xff; 3],
        }
    }

    /// CLI-1591: with the shooter's drawn weapon socket known, his muzzle group and tracer start there, not at the
    /// record's first-person muzzle point.
    #[test]
    fn a_remote_shot_starts_at_the_drawn_weapons_muzzle_socket() {
        let mut r = runner_with("hh_flash", 1.0, vec![key(KeyKind::Other, 0.0, 0.5)]);
        r.tables.muzzle.insert(1, (None, Some("hh_flash".into())));
        let ev = |x: ShotRecord| vec![FxEvent::Record(x)];
        // without a socket: at the record position
        r.feed(1000, &ev(record(false, NO_OBJECT, 1, 10, RECORD_NO_TRACER)), None);
        assert_eq!((r.stats.remote_muzzle, r.stats.remote_muzzle_socket), (1, 0));
        assert_eq!(r.live.last().unwrap().pos, Vec3::ZERO);
        // the window placed the shooter's (object 9) weapon socket 40 units out
        r.remote_muzzle
            .insert(9, Mat4::from_translation(Vec3::new(3.0, 50.0, 40.0)));
        r.feed(1100, &ev(record(false, NO_OBJECT, 1, 10, RECORD_NO_TRACER)), None);
        assert_eq!((r.stats.remote_muzzle, r.stats.remote_muzzle_socket), (2, 1));
        assert_eq!(r.live.last().unwrap().pos, Vec3::new(3.0, 50.0, 40.0));
        // another shooter's record keeps the record position
        let mut other = record(false, NO_OBJECT, 1, 10, RECORD_NO_TRACER);
        other.shooter = 4;
        r.feed(1200, &ev(other), None);
        assert_eq!(r.live.last().unwrap().pos, Vec3::ZERO);
    }

    /// CLI-1136 (amended): a flown object whose projectile row names an effect and no model has no body to draw.
    #[test]
    fn a_model_less_flame_round_has_no_body() {
        let mut t = tables();
        t.projectiles.insert(
            "flame".into(),
            ProjectileRow {
                model: None,
                skin: None,
                scale: None,
                fx: Some("flame_trail".into()),
                fx_loop: true,
                fx_smooth: false,
            },
        );
        t.projectiles.insert(
            "rocket".into(),
            ProjectileRow {
                model: Some("missile.ltb".into()),
                skin: None,
                scale: None,
                fx: Some("rocket_trail".into()),
                fx_loop: true,
                fx_smooth: false,
            },
        );
        t.ammo.entry(70).or_default().projectile = Some("flame".into());
        t.ammo.entry(71).or_default().projectile = Some("rocket".into());
        let thrown = |obj: u16, ammo: u16| FxEvent::Thrown {
            obj,
            ammo,
            pos: [0.0; 3],
            rot: [0.0, 0.0, 0.0, 1.0],
        };
        let hidden = t.bodiless_thrown(&[thrown(300, 70), thrown(301, 71), thrown(302, 9)]);
        assert_eq!(hidden.into_iter().collect::<Vec<_>>(), vec![300]);
    }

    /// CLI-1603: an own shot that met a character draws the gore impact at the shot (surface 6, no mark), on the box
    /// face it entered; the server's record of that hit adds nothing.
    #[test]
    fn an_own_character_hit_draws_its_impact_at_the_shot() {
        let mut r = runner();
        let shot = crate::arms::ShotFx {
            weapon: 1,
            ammo: 1,
            origin: [0.0; 3],
            muzzle: [0.0; 3],
            dir: [0.0, 0.0, 1.0],
            range: 1000.0,
            hit: crate::arms::RayHit::Character {
                obj: 7,
                dist: 200.0,
                node: 6,
                normal: [0.0, 0.0, -1.0],
            },
            first: true,
            time_ms: 1000,
            impact: None,
            flash_mod: None,
        };
        r.feed(1000, &[FxEvent::Shot(shot)], None);
        assert_eq!(r.stats.impacts, 1, "the gore impact at the shot");
        assert_eq!(r.stats.marks, 0, "no hole on a character");
        let ev = |x: ShotRecord| vec![FxEvent::Record(x)];
        r.feed(1100, &ev(record(true, 7, 1, 6, 0x0f11)), None);
        assert_eq!(r.stats.impacts, 1, "the server's record draws nothing more");
    }

    #[test]
    fn records_draw_tracers_impacts_and_blasts() {
        let mut r = runner();
        let ev = |x: ShotRecord| vec![FxEvent::Record(x)];
        let cues = r.feed(1000, &ev(record(false, NO_OBJECT, 1, 10, 0)), None);
        assert_eq!(
            r.stats.tracers, 1,
            "a record of another player with the tracer bit clear"
        );
        assert_eq!(r.stats.impacts, 1);
        assert_eq!(cues.len(), 1, "the surface bullet sound");
        assert_eq!(r.stats.marks, 0, "no hole texture without a ROM: no mark");
        // the suppression bit, the sky surface
        r.feed(
            1100,
            &ev(record(false, NO_OBJECT, 1, 110, RECORD_NO_TRACER)),
            None,
        );
        assert_eq!((r.stats.tracers, r.stats.impacts), (1, 1));
        // an own world record is skipped (the local trace drew it); so is an own character hit (CLI-1603: the local
        // shot drew it)
        r.feed(1200, &ev(record(true, NO_OBJECT, 1, 10, 0)), None);
        assert_eq!(r.stats.impacts, 1);
        r.feed(1300, &ev(record(true, 7, 1, 10, 0)), None);
        assert_eq!(r.stats.impacts, 1);
        // a blast (thrown ammunition) is drawn even when the thrower is the own player: the row's wave
        let cues = r.feed(1400, &ev(record(true, NO_OBJECT, 2, 10, 0x59f0)), None);
        assert_eq!(r.stats.blasts, 1);
        match &cues[..] {
            [crate::sound::SoundCue::Server { file, start, .. }] => {
                assert_eq!(file.as_deref(), Some("boom.wav"));
                assert_eq!(start.outer, 1500);
                assert_eq!(start.pos, Some([0.0, 0.0, 500.0]));
            }
            other => panic!("{other:?}"),
        }
    }

    /// Playtest report: "bullet holes don't blend with the object they're on, and don't disappear as quickly as in C++".
    /// A mark is drawn alpha blended at 0.5 grey, solid for 3 s, fading to alpha 0 at 6 s, and gone after (CLI-633).
    #[test]
    fn a_mark_is_half_grey_solid_for_three_seconds_and_gone_at_six() {
        assert_eq!(mark_alpha(0.0), 1.0);
        assert_eq!(mark_alpha(3.0), 1.0);
        assert!((mark_alpha(4.5) - 0.5).abs() < 1e-6);
        assert_eq!(mark_alpha(6.0), 0.0);
        let mut r = runner();
        r.add_mark(MarkFx {
            pos: Vec3::new(0.0, 0.0, 100.0),
            normal: Vec3::new(0.0, 0.0, -1.0),
            roll: 0.0,
            half: 4.0,
            texture: "hole.spr".into(),
            t0: 1.0,
            world_hit: false,
            probed: true,
            on: None,
        });
        let cam = dc_render::Camera::looking(Vec3::ZERO, 0.0, 0.0, 1.5, 1.2);
        let at = |r: &mut FxRunner, ms: u32| {
            r.feed(ms, &[], None);
            let d = r.draw(ms, &cam, None);
            d.world.first().map(|q| (q.colour, q.blend))
        };
        assert_eq!(at(&mut r, 2000), Some(([0.5, 0.5, 0.5, 1.0], Blend::Alpha)));
        let (c, _) = at(&mut r, 5500).expect("fading");
        assert!((c[3] - 0.5).abs() < 1e-3, "{c:?}");
        assert_eq!(at(&mut r, 7100), None, "gone after 6 s");
    }

    #[test]
    fn own_tracer_every_frequency_th_first_pellet_and_ends_with_its_flight() {
        let mut r = runner();
        let shot = |t: u32| crate::arms::ShotFx {
            weapon: 1,
            ammo: 1,
            origin: [0.0; 3],
            muzzle: [0.0; 3],
            dir: [0.0, 0.0, 1.0],
            range: 800.0,
            hit: crate::arms::RayHit::Miss,
            first: true,
            time_ms: t,
            impact: None,
            flash_mod: None,
        };
        for i in 0..4 {
            r.feed(1000 + i * 10, &[FxEvent::Shot(shot(1000 + i * 10))], None);
        }
        assert_eq!(r.stats.tracers, 2, "frequency 2: every second shot");
        let cam = dc_render::Camera::looking(Vec3::new(50.0, 0.0, 0.0), 0.0, 0.0, 1.5, 1.2);
        let d = r.draw(1040, &cam, None);
        assert!(!d.world.is_empty());
        assert!(
            (d.world[0].v.length() - 2.0).abs() < 1e-3,
            "half the tracer width"
        );
        // 800 units at 1000 u/s: gone after 0.8 s
        r.feed(2000, &[], None);
        assert_eq!(r.live_counts().1, 0);
    }

    /// CLI-1101: a sprite key with facing "along the normal" reads its normal; in an effect pitched by -90 degrees
    /// (the bomb site indicators of the ROM worlds) its quad lies flat (both axes horizontal), a camera-facing one
    /// keeps the camera axes.
    #[test]
    fn a_sprite_along_its_normal_lies_flat_in_a_pitched_effect() {
        use dc_fx_data::fxf::{FieldValue, FxGroup, FxKey, Property};
        let prop = |name: &str, value: FieldValue| Property {
            name: name.into(),
            type_code: 0,
            value,
        };
        let key = FxKey {
            effect_type: Some(FxType::Sprite),
            type_id: String::new(),
            id: 1,
            linked: false,
            link_id: 0,
            attach_node: String::new(),
            start: 0.0,
            end: 2.0,
            repeat: 1,
            whole_group: false,
            properties: vec![
                prop("Normal", FieldValue::Vec3([0.0, 0.0, 1.0])),
                prop(
                    "Facing",
                    FieldValue::Choice {
                        index: 1,
                        raw: String::new(),
                    },
                ),
            ],
        };
        let f = FxfFile {
            encoding: dc_fx_data::Encoding::Text,
            groups: vec![FxGroup {
                name: "G1".into(),
                length_ms: 2000,
                length_s: 2.0,
                keys: vec![key],
            }],
            trailing: 0,
        };
        let lib = EffectLibrary::from_files([&f]);
        let g = lib.get("g1").expect("group");
        let KeyKind::Sprite(sk) = &g.keys[0].kind else {
            panic!("{:?}", g.keys[0].kind);
        };
        assert_eq!((sk.facing, sk.normal), (1, [0.0, 0.0, 1.0]));
        let rot = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let (right, up) = (Vec3::X, Vec3::Y);
        let (u, v) = sprite_axes(sk.facing, sk.normal, rot, right, up);
        assert!(u.y.abs() < 1e-5 && v.y.abs() < 1e-5, "flat: {u} {v}");
        assert!((u.length() - 1.0).abs() < 1e-5 && (v.length() - 1.0).abs() < 1e-5);
        assert_eq!(sprite_axes(0, sk.normal, rot, right, up), (right, up));
        let (u2, v2) = sprite_axes(2, sk.normal, rot, right, up);
        assert!((u2 - rot * Vec3::X).length() < 1e-6 && (v2 - rot * Vec3::Y).length() < 1e-6);
    }

    #[test]
    fn effect_keys_without_properties_take_the_defaults() {
        use dc_fx_data::fxf::{FxGroup, FxKey};
        let key = FxKey {
            effect_type: Some(FxType::Particles),
            type_id: String::new(),
            id: 1,
            linked: false,
            link_id: 0,
            attach_node: String::new(),
            start: 0.0,
            end: 0.5,
            repeat: 1,
            whole_group: false,
            properties: Vec::new(),
        };
        let f = FxfFile {
            encoding: dc_fx_data::Encoding::Text,
            groups: vec![FxGroup {
                name: "G1".into(),
                length_ms: 500,
                length_s: 0.5,
                keys: vec![key],
            }],
            trailing: 0,
        };
        let lib = EffectLibrary::from_files([&f]);
        let g = lib.get("g1").expect("lookup without case");
        match &g.keys[0].kind {
            KeyKind::Particles(p) => {
                assert_eq!(
                    (p.per_emission, p.blend, p.friction),
                    (1, Blend::Alpha, 1.0)
                );
            }
            k => panic!("{k:?}"),
        }
        assert_eq!(g.keys[0].place, Place::Fixed);
        assert_eq!(Blend::of_choice(1), Blend::Additive);
        assert_eq!(Blend::of_choice(2), Blend::Multiply);
    }

    #[test]
    fn frame_helpers() {
        let q = frame_of(Vec3::new(0.0, 1.0, 0.0));
        assert!((q * Vec3::Z - Vec3::Y).length() < 1e-5);
        let (a, b) = plane_axes(Vec3::Y);
        assert!(a.dot(Vec3::Y).abs() < 1e-5 && b.dot(Vec3::Y).abs() < 1e-5);
        assert!(with_thrown(&[], &[]).is_empty());
    }

    // ------------------------------------------------------------------ w2-fx: lights, shake, looping placed effects

    /// A runner whose library holds one made-up group `name` of `length_s` seconds with the given keys.
    fn runner_with(name: &str, length_s: f32, keys: Vec<EffectKey>) -> FxRunner {
        let mut lib = EffectLibrary::default();
        lib.groups
            .insert(name.to_string(), Arc::new(EffectGroup { length_s, keys }));
        FxRunner::new(tables(), lib, TextureStore::default())
    }

    fn key(kind: KeyKind, start: f32, end: f32) -> EffectKey {
        EffectKey {
            start,
            end,
            place: Place::Fixed,
            attach: None,
            offset: [0.0, 10.0, 0.0],
            colours: Vec::new(),
            scales: Vec::new(),
            kind,
            id: 1,
            link: None,
        }
    }

    fn light_key(dim: bool) -> EffectKey {
        EffectKey {
            colours: vec![
                ColourKey {
                    t: 0.0,
                    rgb: [255.0, 128.0, 0.0],
                    alpha: 1.0,
                },
                ColourKey {
                    t: 1.0,
                    rgb: [255.0, 128.0, 0.0],
                    alpha: 0.5,
                },
            ],
            scales: vec![(0.0, 100.0), (1.0, 300.0)],
            ..key(
                KeyKind::Light(crate::fxkeys::LightKey { dim, world: false }),
                0.1,
                0.5,
            )
        }
    }

    fn shake_key(scale: f32) -> EffectKey {
        EffectKey {
            scales: vec![(0.0, scale), (1.0, scale)],
            ..key(
                KeyKind::Shake(crate::fxkeys::ShakeKey::with_radii(100.0, 200.0)),
                0.0,
                1.0,
            )
        }
    }

    #[test]
    fn light_keys_light_the_models_while_they_run() {
        let mut r = runner_with("lit", 1.0, vec![light_key(false)]);
        assert!(r.start_group("lit", Vec3::new(5.0, 0.0, 0.0), Quat::IDENTITY, false, 0.0));
        let eye = Vec3::new(0.0, 50.0, 0.0);
        // before the key's start and after its end: no light
        assert!(r.lights_at(50, eye).is_empty());
        assert!(r.lights_at(600, eye).is_empty());
        // mid-key (fraction 0.5): opacity 0.75, radius 200, at the key's place (group place + offset)
        let l = r.lights_at(300, eye);
        assert_eq!(l.len(), 1);
        let l = l[0];
        assert!((l.colour - Vec3::new(255.0 * 0.75, 128.0 * 0.75, 0.0)).length() < 1e-3);
        assert!((l.radius - 200.0).abs() < 1e-3);
        assert_eq!(l.pos, Vec3::new(5.0, 10.0, 0.0));
        // the dynamic-light law of the model light set (RENDER_2009.md §6.3)
        assert_eq!(l, dc_render::ModelLight::dynamic(l.pos, l.radius, l.colour));
        // a model next to it takes it, one beyond its radius does not
        let near = dc_render::LightSet::build(
            Vec3::new(5.0, 10.0, 50.0),
            20.0,
            None,
            &[],
            &[l],
            &Default::default(),
        );
        assert_eq!(near.lights.len(), 1);
        let far = dc_render::LightSet::build(
            Vec3::new(5.0, 10.0, 1000.0),
            20.0,
            None,
            &[],
            &[l],
            &Default::default(),
        );
        assert!(far.lights.is_empty());
    }

    #[test]
    fn dimmed_light_keys_take_a_clock_bound_factor() {
        let mut r = runner_with("dim", 1.0, vec![light_key(true)]);
        r.start_group("dim", Vec3::ZERO, Quat::IDENTITY, false, 0.0);
        let full = 255.0 * 0.75;
        let a = r.lights_at(300, Vec3::ZERO)[0].colour.x;
        // 0.3 .. 0.49 of the undimmed colour (clientfx+0xd970)
        assert!(a >= full * 0.3 - 1e-3 && a <= full * 0.49 + 1e-3, "{a}");
        // the same session time gives the same value however often it is asked (frame-rate independence, CLI-513)
        assert_eq!(a, r.lights_at(300, Vec3::ZERO)[0].colour.x);
        // another 1/60 s step draws again
        let steps: Vec<f32> = (0..6)
            .map(|i| r.lights_at(200 + i * 17, Vec3::ZERO)[0].colour.x / (255.0 * 0.75))
            .collect();
        assert!(steps.windows(2).any(|w| w[0] != w[1]), "{steps:?}");
    }

    #[test]
    fn first_person_light_keys_stand_at_the_eye() {
        let mut r = runner_with("pv", 1.0, vec![light_key(false)]);
        r.start_group("pv", Vec3::ZERO, Quat::IDENTITY, true, 0.0);
        let eye = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(r.lights_at(300, eye)[0].pos, eye);
    }

    #[test]
    fn shake_keys_move_the_camera_near_them() {
        let mut r = runner_with("boom", 1.0, vec![shake_key(10.0)]);
        r.start_group("boom", Vec3::ZERO, Quat::IDENTITY, false, 0.0);
        // the key stands at the offset (0, 10, 0); inside the inner radius: up to 10 per axis
        let cam = Vec3::new(0.0, 10.0, 50.0);
        let o = r.shake_at(200, cam);
        assert!(o != Vec3::ZERO && o.abs().max_element() <= 10.0, "{o}");
        // the same session time: the same offset (CLI-513)
        assert_eq!(o, r.shake_at(200, cam));
        // at 150 units: fade 1 - 12500 / 30000
        let f = 1.0 - 12_500.0 / 30_000.0;
        let o = r.shake_at(200, Vec3::new(0.0, 10.0, 150.0));
        assert!(o.abs().max_element() <= 10.0 * f + 1e-4, "{o}");
        // beyond the outer radius, or after the key: none
        assert_eq!(r.shake_at(200, Vec3::new(0.0, 10.0, 250.0)), Vec3::ZERO);
        assert_eq!(r.shake_at(1500, cam), Vec3::ZERO);
        // two shaking groups add up
        r.start_group("boom", Vec3::ZERO, Quat::IDENTITY, false, 0.0);
        assert!(r.shake_at(200, cam).abs().max_element() <= 20.0);
    }

    fn placed(obj: u16, value: u32) -> FxEvent {
        FxEvent::World(WorldFx {
            name: "Fire".into(),
            pos: [0.0; 3],
            rot: [0.0, 0.0, 0.0, 1.0],
            obj: Some(obj),
            value,
        })
    }

    #[test]
    fn looping_placed_effects_play_again_every_group_length_until_their_object_goes() {
        let lit = |r: &FxRunner, ms: u32| !r.lights_at(ms, Vec3::ZERO).is_empty();
        // once (value 0): the key at 0.1 .. 0.5 s of the first second only
        let mut once = runner_with("fire", 1.0, vec![light_key(false)]);
        once.feed(0, &[placed(7, 0)], None);
        assert!(lit(&once, 300));
        for ms in (100..=2400).step_by(100) {
            once.feed(ms, &[], None);
        }
        assert!(!lit(&once, 2300));
        // looping (value bit 0): again in the third second, on the session clock
        let mut l = runner_with("fire", 1.0, vec![light_key(false)]);
        l.feed(0, &[placed(7, WORLD_FX_LOOP)], None);
        for ms in (100..=2200).step_by(100) {
            l.feed(ms, &[], None);
        }
        assert!(lit(&l, 2300));
        assert!(!lit(&l, 2700));
        assert_eq!(l.stats.world_cycles, 2);
        // the same at a tenth of the frame rate (cycles start on the group clock, not at a frame)
        let mut slow = runner_with("fire", 1.0, vec![light_key(false)]);
        slow.feed(0, &[placed(7, WORLD_FX_LOOP)], None);
        slow.feed(1050, &[], None);
        slow.feed(2050, &[], None);
        assert_eq!(
            slow.lights_at(2300, Vec3::ZERO),
            l.lights_at(2300, Vec3::ZERO)
        );
        // the object leaves: no further cycle; the running one runs out (no cut bit)
        l.feed(2300, &[FxEvent::WorldGone(7)], None);
        assert!(lit(&l, 2400));
        for ms in (2400..=4000).step_by(100) {
            l.feed(ms, &[], None);
        }
        assert!(!lit(&l, 3300));
        assert_eq!(l.stats.world_cycles, 2);
        assert_eq!(l.live_counts().0, 0);
        // another object's removal does not stop it
        let mut k = runner_with("fire", 1.0, vec![light_key(false)]);
        k.feed(0, &[placed(7, WORLD_FX_LOOP)], None);
        k.feed(500, &[FxEvent::WorldGone(8)], None);
        k.feed(1200, &[], None);
        assert!(lit(&k, 1300));
    }

    #[test]
    fn the_cut_bit_stops_a_placed_effect_at_its_objects_removal() {
        let mut r = runner_with("fire", 1.0, vec![light_key(false)]);
        r.feed(0, &[placed(9, WORLD_FX_LOOP | WORLD_FX_CUT)], None);
        r.feed(200, &[], None);
        assert_eq!(r.live_counts().0, 1);
        r.feed(300, &[FxEvent::WorldGone(9)], None);
        assert_eq!(r.live_counts().0, 0);
        assert!(r.lights_at(300, Vec3::ZERO).is_empty());
    }

    #[test]
    fn a_placed_effect_without_an_object_or_length_plays_once() {
        let mut r = runner_with("fire", 0.0, vec![light_key(false)]);
        r.feed(0, &[placed(7, WORLD_FX_LOOP)], None);
        for ms in (100..=3000).step_by(100) {
            r.feed(ms, &[], None);
        }
        assert_eq!(r.stats.world_cycles, 0);
        let mut r = runner_with("fire", 1.0, vec![light_key(false)]);
        r.feed(
            0,
            &[FxEvent::World(WorldFx {
                name: "fire".into(),
                pos: [0.0; 3],
                rot: [0.0, 0.0, 0.0, 1.0],
                obj: None,
                value: WORLD_FX_LOOP,
            })],
            None,
        );
        for ms in (100..=3000).step_by(100) {
            r.feed(ms, &[], None);
        }
        assert_eq!(r.stats.world_cycles, 0);
    }

    #[test]
    fn effect_quads_fog_by_their_blend() {
        // RENDER_2009.md §7.1: additive toward black, multiplying toward white, the others toward the fog colour
        let fog = dc_render::Fog {
            near: 100.0,
            far: 300.0,
            colour: [255.0, 0.0, 0.0],
        };
        let c = [0.2, 0.4, 0.6];
        assert_eq!(fx_fogged(c, Blend::Alpha, Some(&fog), 50.0), c);
        assert_eq!(
            fx_fogged(c, Blend::Alpha, Some(&fog), 400.0),
            [1.0, 0.0, 0.0]
        );
        assert_eq!(fx_fogged(c, Blend::Additive, Some(&fog), 400.0), [0.0; 3]);
        assert_eq!(fx_fogged(c, Blend::Multiply, Some(&fog), 400.0), [1.0; 3]);
        let half = fx_fogged(c, Blend::Additive, Some(&fog), 200.0);
        assert!((half[2] - 0.3).abs() < 1e-6);
        assert_eq!(fx_fogged(c, Blend::Alpha, None, 400.0), c);
        // the uniform: [far, 1 / (far - near), on]; off when near equals far (RENDER_2009.md §3.3)
        let (u, col) = fx_fog_uniform(Some(&fog));
        assert_eq!(u, [300.0, 1.0 / 200.0, 1.0, 0.0]);
        assert_eq!(col, [1.0, 0.0, 0.0, 1.0]);
        let flat = dc_render::Fog {
            near: 5.0,
            far: 5.0,
            colour: [0.0; 3],
        };
        assert_eq!(fx_fog_uniform(Some(&flat)).0[2], 0.0);
        assert_eq!(fx_fog_uniform(None).0[2], 0.0);
    }

    /// CLI-1590: a launched round (`Type` 1) is drawn as the thrown ones are: the shooter's own shot has its muzzle
    /// effect but no tracer and no impact (the server's object and blast show it), the server's blast record is drawn
    /// for the shooter too, and another player's blast record has no tracer along the flight; the launch record (sky
    /// surface) draws nothing.
    #[test]
    fn a_launched_round_has_no_own_tracer_or_impact_and_its_blast_is_drawn_for_the_shooter() {
        let w = ButeDoc::parse(
            "[Weapon3]\nPVMuzzleFXName = \"flashA\"\nMuzzlePos = <1.0, -2.0, 3.0>\n\
             [Ammo3]\nImpactFXName = \"boomA\"\nTracerFXName = \"trA\"\nType = 1\n\
             [Ammo1]\nImpactFXName = \"boomA\"\nTracerFXName = \"trA\"\nType = 0\n",
        );
        let fx = ButeDoc::parse(
            "[TracerFX0]\nName = \"trA\"\nTexture = \"t.dtx\"\nFrequency = 1\nVelocity = 1000.0\nWidth = 4\n\
             InitialAlpha = 1.0\nFinalAlpha = 0.5\nColor = <255.0, 128.0, 0.0>\nLength = 100.0\n\
             [ImpactFX0]\nName = \"boomA\"\nSound = \"boom.wav\"\nSoundRadius = 1500\n",
        );
        let s = ButeDoc::parse("[Surface0]\nId = 10\n[Surface1]\nId = 110\n");
        let t = FxTables::from_docs(Some(&w), Some(&fx), Some(&s));
        assert!(!traced_ammo(t.ammo_fx(3).unwrap().ammo_type));
        let mut r = FxRunner::new(t, EffectLibrary::default(), TextureStore::default());
        let shot = |ammo: u16| crate::arms::ShotFx {
            weapon: 3,
            ammo,
            origin: [0.0; 3],
            muzzle: [0.0; 3],
            dir: [0.0, 0.0, 1.0],
            range: 1000.0,
            hit: crate::arms::RayHit::Miss,
            first: true,
            time_ms: 1000,
            impact: Some(crate::world::ShotImpactAt {
                point: [0.0, 0.0, 500.0],
                normal: [0.0, 0.0, -1.0],
                surface: 10,
                on_model: false,
            }),
            flash_mod: None,
        };
        r.feed(1000, &[FxEvent::Shot(shot(3))], None);
        assert_eq!(
            (r.stats.tracers, r.stats.impacts),
            (0, 0),
            "the own launch: no tracer, no impact"
        );
        r.feed(1050, &[FxEvent::Shot(shot(1))], None);
        assert_eq!(
            (r.stats.tracers, r.stats.impacts),
            (1, 1),
            "a traced round draws both"
        );
        let ev = |x: ShotRecord| vec![FxEvent::Record(x)];
        // the launch record (the sky surface): nothing
        r.feed(1100, &ev(record(false, NO_OBJECT, 3, 110, 0)), None);
        assert_eq!((r.stats.tracers, r.stats.impacts, r.stats.blasts), (1, 1, 0));
        // the shooter's own blast: drawn, with its wave
        let cues = r.feed(1200, &ev(record(true, NO_OBJECT, 3, 10, 0)), None);
        assert_eq!((r.stats.impacts, r.stats.blasts), (2, 1));
        assert!(cues.iter().any(|c| matches!(
            c,
            crate::sound::SoundCue::Server { file, .. } if file.as_deref() == Some("boom.wav")
        )));
        // another player's blast: the impact, no tracer along the flight
        r.feed(1300, &ev(record(false, NO_OBJECT, 3, 10, 0)), None);
        assert_eq!((r.stats.tracers, r.stats.impacts, r.stats.blasts), (1, 3, 2));
    }
}
