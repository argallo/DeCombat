//! Destructible world objects (DeCombat rules on the 2009 wire forms): the world file's breakable models (crates,
//! barrels) with their hit points and death commands, and the explosion objects a death command switches on.
//!
//! What the world file says (OBSERVED over the 21 ROM worlds, read at run time, never copied): every object of the
//! breakable class carries a hit-point property, a can-damage switch, a never-destroy switch and a death command; the
//! death commands are `msg <name> on` statements naming a placed client effect (the breaking crate or barrel effect,
//! one per breakable, switched off at the start) and, for explosive barrels, an explosion object (radius, damage at
//! the centre and the name of an impact effect). Plain world models carry hit points as well, but no
//! death command and no effect: they stay (DC-DEFAULT, below). No 2009 capture shows a breakable dying, so what the
//! server sends is ours: the dead model's flag words without visible / solid / ray-hit (a change record of the
//! named brush), the switched-on effects as placed-effect objects (the form the reference stack used for the world's effects,
//! REFERENCE, accepted by the real client) removed after a while, and the explosion as the type 0x06 blast record with
//! the ammunition whose impact effect it names plus area damage through the damage path of DSV-30.
// SPEC: FMT_DAT85.md §6 (object list, properties by name), §6.2 (breakable brush role), §6.4 (model link by name);
// UDP_SESSION_2009_B.md §3.3 (kind 2), §4.2 item 3 (flag words); GAME_MESSAGES_2009_S2C.md §8.2 (type 0x06 blast row),
// §8.4 (type 0x64 placed effect); GAME_MESSAGES_2009_C2S.md §4.1 (`hit_obj`: the object the client's trace hit)
// DC-DEFAULT: DSV-115..DSV-120 (proposed, staged world-objects)

use crate::objects::{write_record_head, CF_FLAGS};
use crate::world::{BrushClass, BrushInfo, PropHash};
use dc_wire::udp::BitWriter;
use dc_world::World;

/// Class of the explosion objects and the properties this module reads (hash and length of the name; no ROM text).
// OBSERVED: class and property names of the ROM world files (hashes only; 2026-10-25 scratch survey, world-objects:
// 243 breakables on 21 worlds, all with the can-damage switch on and the never-destroy switch off; 85 explosion
// objects on 12 worlds)
pub const CLASS_EXPLOSION: PropHash = ([0xca, 0x64, 0x82, 0xa4, 0x2b, 0xab, 0xcb, 0x32], 9);
/// The breakable's hit points: the `dc_world::names` key (ROM hash or `hit_points` on a community map, MAP-20).
pub const PROP_HIT_POINTS: &dc_world::Key = &dc_world::names::PROP_HIT_POINTS;
pub const PROP_DEATH_COMMAND: PropHash = ([0x05, 0x6b, 0x54, 0x7b, 0xd8, 0x4a, 0xf7, 0x66], 12);
pub const PROP_CAN_DAMAGE: PropHash = ([0x75, 0xdb, 0x1c, 0x6c, 0x74, 0xef, 0x3e, 0x30], 9);
pub const PROP_NEVER_DESTROY: PropHash = ([0x72, 0x97, 0x34, 0xad, 0x0c, 0x1d, 0x60, 0x67], 12);
pub const PROP_DAMAGE_RADIUS: PropHash = ([0x93, 0xc1, 0x8b, 0x71, 0x4b, 0x65, 0x93, 0x8d], 12);
pub const PROP_MAX_DAMAGE: PropHash = ([0x83, 0x8d, 0x9a, 0x98, 0xe6, 0xd3, 0x73, 0x59], 9);
pub const PROP_IMPACT_FX_NAME: PropHash = ([0xe0, 0xc2, 0x75, 0xf6, 0x43, 0x1c, 0x54, 0x92], 12);
/// Command words of a death command: the message statement and its `on` argument.
// OBSERVED: command texts of the ROM world files (hashes only)
const WORD_MSG: PropHash = ([0xe4, 0x6b, 0x32, 0x01, 0x65, 0xee, 0xc9, 0x1e], 3);
const WORD_ON: PropHash = ([0xb8, 0xd3, 0x1e, 0x85, 0x27, 0x25, 0xaf, 0xb1], 2);

/// What a breakable's death switches on.
#[derive(Debug, Clone, PartialEq)]
pub enum DeathAction {
    /// A placed client effect of this name (the world's effect objects, matched without case).
    Effect(Vec<u8>),
    /// The explosion object of this index in [`DestructibleWorld::explosions`].
    Explode(usize),
}

/// One breakable model of the world file.
#[derive(Debug, Clone, PartialEq)]
pub struct BreakableInfo {
    /// The object name (= the BSP model name, FMT_DAT85 §6.4).
    pub name: Vec<u8>,
    pub hit_points: f32,
    /// The actions of its death command, in command order (statements other than `msg <name> on` are not run).
    pub death: Vec<DeathAction>,
    /// Statements of the death command this server does not run (logged).
    pub skipped: u32,
}

/// One explosion object of the world file.
#[derive(Debug, Clone, PartialEq)]
pub struct ExplosionInfo {
    pub name: Vec<u8>,
    pub pos: [f32; 3],
    /// Damage at the centre and the radius.
    pub damage: f32,
    pub radius: f32,
    /// The impact effect name (an ammunition row's `ImpactFXName`, which picks the blast record's ids).
    pub impact_fx: Option<Vec<u8>>,
}

/// The destructible objects of a world.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DestructibleWorld {
    pub breakables: Vec<BreakableInfo>,
    pub explosions: Vec<ExplosionInfo>,
    /// The model geometry of each breakable, for the server's own pellet trace (the 2009 client names no object
    /// for a world-model hit, `crate::game` destruct): lower-case model name -> its polygons.
    // DC-DEFAULT: DSV-122 (proposed, client-brushhits)
    pub shapes: Vec<(Vec<u8>, std::sync::Arc<dc_sim::CollisionWorld>)>,
}

impl DestructibleWorld {
    /// Adds the model geometry of every breakable (by its model name in `w`) that has none yet.
    // SPEC: FMT_DAT85.md §6.4 (object name = model name; model geometry stored in world space)
    pub fn add_shapes(&mut self, w: &World<'_>) {
        for b in &self.breakables {
            let name = b.name.to_ascii_lowercase();
            if self.shapes.iter().any(|(n, _)| *n == name) {
                continue;
            }
            let Some(m) = w.models.iter().find(|m| m.name.eq_ignore_ascii_case(&name)) else {
                continue;
            };
            let model = dc_sim::from_world::coll_model(m);
            self.shapes.push((
                name,
                std::sync::Arc::new(dc_sim::CollisionWorld::from_parts(vec![model], &[])),
            ));
        }
    }

    /// The geometry of the breakable model of this name.
    pub fn shape(&self, name: &[u8]) -> Option<&dc_sim::CollisionWorld> {
        self.shapes
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, c)| c.as_ref())
    }

    /// The breakable of this name (ASCII case-insensitive).
    pub fn breakable(&self, name: &[u8]) -> Option<&BreakableInfo> {
        self.breakables
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case(name))
    }
}

fn key_of(name: &[u8]) -> PropHash {
    (dc_world::hash16(name), name.len())
}

fn prop<'o, 'a>(o: &'o dc_world::Object<'a>, k: PropHash) -> Option<&'o dc_world::FieldValue<'a>> {
    o.props
        .iter()
        .find(|p| p.name.len() == k.1 && dc_world::hash16(p.name) == k.0)
        .map(|p| &p.value)
}

fn prop_real(o: &dc_world::Object<'_>, k: PropHash) -> Option<f32> {
    match prop(o, k)? {
        dc_world::FieldValue::Real(r) if r.is_finite() => Some(*r),
        dc_world::FieldValue::Int(i) | dc_world::FieldValue::Bits(i) => Some(*i as f32),
        _ => None,
    }
}

fn prop_bool(o: &dc_world::Object<'_>, k: PropHash) -> Option<bool> {
    match prop(o, k)? {
        dc_world::FieldValue::Bool(b) => Some(*b),
        dc_world::FieldValue::Int(i) | dc_world::FieldValue::Bits(i) => Some(*i != 0),
        _ => None,
    }
}

fn prop_text<'a>(o: &dc_world::Object<'a>, k: PropHash) -> Option<&'a [u8]> {
    match prop(o, k)? {
        dc_world::FieldValue::Text(t) => Some(*t),
        _ => None,
    }
}

/// The `msg <target> on` targets of a command text, in order, and the number of other statements.
// OBSERVED: the ROM worlds' death commands (`msg <effect> on; msg <explosion> on; `, statements separated by `;` or
// line breaks; a few name `off`, `remove` or `lock` arguments, which this server does not run)
// DC-DEFAULT: DSV-116 (proposed) only the switch-on statements run
pub fn switched_on(text: &[u8]) -> (Vec<Vec<u8>>, u32) {
    let mut on = Vec::new();
    let mut skipped = 0;
    for st in text.split(|&b| b == b';' || b == b'\n' || b == b'\r') {
        let words: Vec<&[u8]> = st
            .split(|b| b.is_ascii_whitespace() || *b == b'(' || *b == b')')
            .filter(|w| !w.is_empty())
            .collect();
        if words.is_empty() {
            continue;
        }
        let lower = |w: &[u8]| w.to_ascii_lowercase();
        if words.len() == 3
            && key_of(&lower(words[0])) == WORD_MSG
            && key_of(&lower(words[2])) == WORD_ON
        {
            on.push(words[1].to_vec());
        } else {
            skipped += 1;
        }
    }
    (on, skipped)
}

/// The destructible objects of a parsed world: every breakable-class object linked to a model with positive hit
/// points, the can-damage switch on and the never-destroy switch off, and every explosion object.
// SPEC: FMT_DAT85.md §6.2 (breakable role), §6.4
// DC-DEFAULT: DSV-115 (proposed) only the breakable class breaks (plain world models carry hit points too, but
// breaking them would open the level's walls)
pub fn destructible_world(w: &World<'_>) -> DestructibleWorld {
    let mut d = DestructibleWorld::default();
    for o in &w.objects {
        if key_of(o.class_name) != CLASS_EXPLOSION {
            continue;
        }
        let damage = prop_real(o, PROP_MAX_DAMAGE).unwrap_or(0.0).max(0.0);
        let radius = prop_real(o, PROP_DAMAGE_RADIUS).unwrap_or(0.0).max(0.0);
        d.explosions.push(ExplosionInfo {
            name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
            pos: o.position().unwrap_or([0.0; 3]),
            damage,
            radius,
            impact_fx: prop_text(o, PROP_IMPACT_FX_NAME)
                .filter(|t| !t.is_empty())
                .map(<[u8]>::to_vec),
        });
    }
    for o in &w.objects {
        if o.role != Some(dc_world::ObjectRole::Breakable) || w.brush_of(o).is_none() {
            continue;
        }
        // a `dc.breakable` needs only `hit_points`: the can-damage and never-destroy switches default to pass
        // DC-DEFAULT: MAP-20
        let hit_points = crate::world::named_real(o, PROP_HIT_POINTS).unwrap_or(0.0);
        if hit_points.is_nan()
            || hit_points <= 0.0
            || prop_bool(o, PROP_CAN_DAMAGE) == Some(false)
            || prop_bool(o, PROP_NEVER_DESTROY) == Some(true)
        {
            continue;
        }
        let (on, skipped) = prop_text(o, PROP_DEATH_COMMAND)
            .map(switched_on)
            .unwrap_or_default();
        let death = on
            .into_iter()
            .map(|t| {
                match d
                    .explosions
                    .iter()
                    .position(|e| e.name.eq_ignore_ascii_case(&t))
                {
                    Some(i) => DeathAction::Explode(i),
                    None => DeathAction::Effect(t),
                }
            })
            .collect();
        d.breakables.push(BreakableInfo {
            name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
            hit_points,
            death,
            skipped,
        });
    }
    d.add_shapes(w);
    d
}

/// A breakable brush is bound when the server created it in a state a player can hit (visible, solid or ray-hit).
pub fn bindable(b: &BrushInfo) -> bool {
    b.class == BrushClass::Breakable && (b.visible || b.solid || b.rayhit)
}

/// The brush of a dead breakable: not visible, not solid, not hit by rays (the flag words of DSV-37 without bits
/// 0x0001, 0x2000, 0x1000 and with user word bit 0 cleared).
// DC-DEFAULT: DSV-117 (proposed) a dead breakable stays as an object with its model switched off
pub fn broken(b: &BrushInfo) -> BrushInfo {
    BrushInfo {
        visible: false,
        solid: false,
        rayhit: false,
        ..b.clone()
    }
}

/// The brush of a restored breakable: the visible / solid / ray-hit switches of the brush as the world created it
/// (`original`), everything else as it is now (`now`).
// SPEC: FMT_DAT85.md §6.2 (breakable brush role); OBSERVED: the ROM worlds' state save / state restore trigger
// statements run by the mode controller and the start trigger (hashes only; world-objects.md §1.2)
// DC-DEFAULT: GMD-24 (staged) a new round of a round mode switches the dead breakables back on
pub fn restored(now: &BrushInfo, original: &BrushInfo) -> BrushInfo {
    BrushInfo {
        visible: original.visible,
        solid: original.solid,
        rayhit: original.rayhit,
        ..now.clone()
    }
}

/// The change record of a named brush's flag words (`cf` = `CF_FLAGS` only), the four words in the create's form.
// SPEC: UDP_SESSION_2009_B.md §4.2 item 3 (`[flags:u16][flags2:u16][uflags:u32][flags3:u32]`), §3.2 (record head)
// DC-DEFAULT: DSV-117 (proposed)
pub fn brush_flags_record(obj: u16, b: &BrushInfo) -> BitWriter {
    let (f, f2, uf, f3) = crate::worldobj::brush_flags(b);
    let mut w = BitWriter::new();
    write_record_head(&mut w, CF_FLAGS, obj);
    w.write_u16(f & !0x0024)
        .write_u16(f2)
        .write_u32(uf)
        .write_u32(f3);
    w
}

/// The `destruct.*` rule values (`-destruct key=value[,key=value...]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DestructTuning {
    /// Breakables take damage and die (`destruct.enabled`).
    // DC-DEFAULT: DSV-115 (proposed) on
    pub enabled: bool,
    /// How long a switched-on death effect object stays on the clients (`destruct.effect_ms`).
    // DC-DEFAULT: DSV-118 (proposed) 10 s (our value; the ROM effects of the breakables are one-shots)
    pub effect_ms: u32,
    /// Fraction of a grenade blast's damage a breakable takes (`destruct.blast_factor`).
    // DC-DEFAULT: DSV-119 (proposed) 1.0
    pub blast_factor: f32,
    /// A new round of a round mode restores every breakable (`destruct.restore`): full hit points, the model switched
    /// back on, the death effects removed.
    // DC-DEFAULT: GMD-24 (staged) on
    pub restore: bool,
}

impl Default for DestructTuning {
    fn default() -> Self {
        DestructTuning {
            enabled: true,
            effect_ms: 10_000,
            blast_factor: 1.0,
            restore: true,
        }
    }
}

/// The `destruct.*` keys.
pub const DESTRUCT_KEYS: [&str; 4] = ["enabled", "effect_ms", "blast_factor", "restore"];

impl DestructTuning {
    /// Sets one `destruct.*` key (the prefix is optional).
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let k = key.trim();
        let k = k.strip_prefix("destruct.").unwrap_or(k);
        let v = value.trim();
        let bad = || format!("destruct.{k}: bad value {v:?}");
        let switch = || match v.to_ascii_lowercase().as_str() {
            "on" | "1" | "true" => Ok(true),
            "off" | "0" | "false" => Ok(false),
            _ => Err(bad()),
        };
        match k {
            "enabled" => self.enabled = switch()?,
            "restore" => self.restore = switch()?,
            "effect_ms" => self.effect_ms = v.parse::<u32>().map_err(|_| bad())?.min(600_000),
            "blast_factor" => {
                self.blast_factor = v
                    .parse::<f32>()
                    .ok()
                    .filter(|x| x.is_finite() && *x >= 0.0)
                    .ok_or_else(bad)?
            }
            _ => {
                return Err(format!(
                    "unknown key destruct.{k} (known: {})",
                    DESTRUCT_KEYS.join(", ")
                ))
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn death_commands_keep_the_switch_on_statements() {
        let (on, skipped) =
            switched_on(b"msg FxA on; msg ExpA ON;\n msg Other off; delay 2 (msg X on);");
        assert_eq!(on, vec![b"FxA".to_vec(), b"ExpA".to_vec()]);
        assert_eq!(skipped, 2);
        assert_eq!(switched_on(b"").0.len(), 0);
    }

    #[test]
    fn broken_brush_flags_drop_visible_solid_and_rayhit() {
        let b = BrushInfo {
            name: b"Crate1".to_vec(),
            role: Some(dc_world::ObjectRole::Breakable),
            pos: [1.0, 2.0, 3.0],
            rot: [0.0; 3],
            visible: true,
            solid: true,
            rayhit: true,
            blend: 0,
            fog_disable: false,
            class: BrushClass::Breakable,
            sky: None,
        };
        assert!(bindable(&b));
        let (f, _, uf, _) = crate::worldobj::brush_flags(&b);
        assert_eq!((f, uf), (0x3101, 0x0121));
        let d = broken(&b);
        assert!(!bindable(&d));
        let (f, _, uf, _) = crate::worldobj::brush_flags(&d);
        assert_eq!((f, uf), (0x0100, 0x0120));
        let rec = brush_flags_record(77, &d);
        let mut r = rec.reader();
        assert_eq!(r.read_u8(), CF_FLAGS as u8);
        assert_eq!(r.read_u16(), 77);
        assert_eq!(r.read_u16(), 0x0100);
        assert_eq!(r.read_u16(), 0);
        assert_eq!(r.read_u32(), 0x0120);
        assert_eq!(r.read_u32(), 0);
        assert_eq!(r.remaining_bits(), 0);
        // GMD-24: the restored brush has the created switches again, the rest as it is now
        let moved = BrushInfo {
            pos: [9.0, 9.0, 9.0],
            ..d.clone()
        };
        let back = restored(&moved, &b);
        assert!(bindable(&back));
        assert_eq!(back.pos, [9.0, 9.0, 9.0]);
        assert_eq!(
            crate::worldobj::brush_flags(&back),
            crate::worldobj::brush_flags(&b)
        );
    }

    #[test]
    fn tuning_keys() {
        let mut t = DestructTuning::default();
        t.set("destruct.effect_ms", "2500").unwrap();
        t.set("enabled", "off").unwrap();
        t.set("blast_factor", "0.5").unwrap();
        assert_eq!((t.enabled, t.effect_ms, t.blast_factor), (false, 2500, 0.5));
        assert!(t.restore);
        t.set("destruct.restore", "off").unwrap();
        assert!(!t.restore);
        assert!(t.set("restore", "maybe").is_err());
        assert!(t.set("bogus", "1").is_err());
        assert!(t.set("blast_factor", "-1").is_err());
    }
}
