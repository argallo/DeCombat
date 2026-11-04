//! The JSON operations every editor of a project goes through (MAPKIT_PLAN.md 5.2 `apply`, D8): the GUI, the
//! CLI, the MCP tools and Lua generators all produce [`Op`] batches and [`apply`] them.
//!
//! Semantics (DC-DEFAULT: MAP-11):
//! - a batch is all-or-nothing: the first invalid op refuses the whole batch and nothing changes;
//! - new things get ids `<prefix>_<n>` from the project's `next_id`; `applied[i].ids` echoes them so an agent can
//!   move what it just added;
//! - `ground: true` snaps `pos.y` to the floor under (x, z) (the terrain height or the top of a box / cylinder /
//!   prism solid, whichever is higher) plus the standing offset: `STAND_OFFSET` for a player spawn, the half
//!   height of an object with an `extent`, half the size of a solid, 0 otherwise; with no floor the y is kept;
//! - terrain ops work in world units on the heightmap; `terrain.gen` creates a terrain when the project has none;
//! - the result lists the files a save would change (`map.json`, `terrain.r16`).
//!
//! Terrain noise ([`noise`]) is seeded value noise / fBm whose permutation is generated at run time from the seed
//! (no literal table), so the same seed gives the same heights on every platform.

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::geom::{self, unit_circle, V3};
use crate::project::{
    BrushRole, Heights, Import, MapObject, Material, MaterialFlags, PrefabInstance, Project,
    PropValue, Solid, SolidKind, Terrain, TextureRef, Vec3f, STAND_OFFSET,
};
use crate::vocab_names as v;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The shape of a `solid.add`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolidSpec {
    Box,
    Wedge,
    Cylinder { sides: u32 },
    Prism { points: Vec<[f32; 2]> },
}

impl SolidSpec {
    /// The stored kind (a cylinder gets its unit circle here, once).
    pub fn to_kind(&self) -> SolidKind {
        match self {
            SolidSpec::Box => SolidKind::Box,
            SolidSpec::Wedge => SolidKind::Wedge,
            SolidSpec::Cylinder { sides } => SolidKind::Cylinder {
                sides: (*sides).max(3),
                circle: unit_circle(*sides),
            },
            SolidSpec::Prism { points } => SolidKind::Prism {
                points: points.clone(),
            },
        }
    }
}

/// Which way a wedge climbs in the world, the safe way to set its direction (DC-DEFAULT: MAP-92).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WedgeRise {
    /// Climbs toward +z.
    North,
    /// Climbs toward +x.
    East,
    /// Climbs toward -z.
    South,
    /// Climbs toward -x.
    West,
}

impl WedgeRise {
    /// The yaw that points a wedge's high side this way: the wedge rises toward its local +z, and a yaw of 90
    /// turns local +z to world +x.
    pub fn rot_y_deg(self) -> f32 {
        match self {
            WedgeRise::North => 0.0,
            WedgeRise::East => 90.0,
            WedgeRise::South => 180.0,
            WedgeRise::West => 270.0,
        }
    }
}

/// `terrain.gen` presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// fBm hills.
    Hills,
    /// Hills with a lowered band along z through the centre.
    Valley,
    /// Hills with a raised band along z through the centre.
    Ridge,
    /// One level floor at `base`: amplitude, scale, octaves and seed do nothing (MAP-93).
    Flat,
}

/// Brush falloff from the centre (1) to the radius (0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Falloff {
    #[default]
    Smooth,
    Linear,
    Flat,
}

impl Falloff {
    /// Weight at `t = distance / radius` (0 at the centre, 1 at the edge).
    pub fn weight(self, t: f32) -> f32 {
        if t >= 1.0 {
            return 0.0;
        }
        let t = t.max(0.0);
        match self {
            Falloff::Smooth => 1.0 - (3.0 * t * t - 2.0 * t * t * t),
            Falloff::Linear => 1.0 - t,
            Falloff::Flat => 1.0,
        }
    }
}

fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}
fn default_amplitude() -> f32 {
    256.0
}
fn default_octaves() -> u32 {
    5
}
fn default_scale() -> f32 {
    1024.0
}
fn default_passes() -> u32 {
    1
}

/// One operation. JSON: `{"op": "<name>", ...fields}`; `dc-mapkit schema --ops` prints the field docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Op {
    #[serde(rename = "add")]
    Add {
        class: String,
        #[serde(default)]
        name: Option<String>,
        pos: Vec3f,
        #[serde(default)]
        rot: Option<Vec3f>,
        #[serde(default)]
        props: BTreeMap<String, PropValue>,
        #[serde(default)]
        ground: bool,
    },
    #[serde(rename = "remove")]
    Remove { id: String },
    /// Empties the project, or only one group of solids / one class of objects, so a generator can run again.
    #[serde(rename = "clear")]
    Clear {
        #[serde(default)]
        group: Option<String>,
        #[serde(default)]
        class: Option<String>,
    },
    #[serde(rename = "move")]
    Move {
        id: String,
        #[serde(default)]
        pos: Option<Vec3f>,
        #[serde(default)]
        delta: Option<Vec3f>,
        #[serde(default)]
        ground: bool,
    },
    #[serde(rename = "set")]
    Set {
        id: String,
        prop: String,
        value: PropValue,
    },
    #[serde(rename = "rename")]
    Rename { id: String, name: String },
    #[serde(rename = "solid.add")]
    SolidAdd {
        kind: SolidSpec,
        pos: Vec3f,
        #[serde(default)]
        rot_y_deg: f32,
        size: Vec3f,
        material: String,
        #[serde(default)]
        faces: BTreeMap<String, String>,
        #[serde(default)]
        group: Option<String>,
        #[serde(default)]
        brush: Option<BrushRole>,
        #[serde(default)]
        ground: bool,
        /// A wedge only: which way it climbs, instead of a `rot_y_deg`.
        #[serde(default)]
        rises: Option<WedgeRise>,
    },
    #[serde(rename = "solid.set")]
    SolidSet {
        id: String,
        #[serde(default)]
        pos: Option<Vec3f>,
        #[serde(default)]
        rot_y_deg: Option<f32>,
        #[serde(default)]
        size: Option<Vec3f>,
        #[serde(default)]
        material: Option<String>,
        #[serde(default)]
        faces: Option<BTreeMap<String, String>>,
        #[serde(default)]
        group: Option<String>,
        #[serde(default)]
        brush: Option<BrushRole>,
        /// A wedge only: which way it climbs, instead of a `rot_y_deg`.
        #[serde(default)]
        rises: Option<WedgeRise>,
    },
    #[serde(rename = "terrain.gen")]
    TerrainGen {
        preset: Preset,
        #[serde(default)]
        seed: Option<u64>,
        #[serde(default = "default_amplitude")]
        amplitude: f32,
        #[serde(default)]
        base: f32,
        #[serde(default = "default_scale")]
        scale: f32,
        #[serde(default = "default_octaves")]
        octaves: u32,
        #[serde(default)]
        cells_x: Option<u32>,
        #[serde(default)]
        cells_z: Option<u32>,
        #[serde(default)]
        cell_size: Option<f32>,
    },
    #[serde(rename = "terrain.raise")]
    TerrainRaise {
        x: f32,
        z: f32,
        radius: f32,
        amount: f32,
        #[serde(default)]
        falloff: Falloff,
    },
    #[serde(rename = "terrain.lower")]
    TerrainLower {
        x: f32,
        z: f32,
        radius: f32,
        amount: f32,
        #[serde(default)]
        falloff: Falloff,
    },
    #[serde(rename = "terrain.smooth")]
    TerrainSmooth {
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        z: Option<f32>,
        #[serde(default)]
        radius: Option<f32>,
        #[serde(default = "default_passes")]
        passes: u32,
    },
    #[serde(rename = "terrain.flatten")]
    TerrainFlatten {
        x: f32,
        z: f32,
        radius: f32,
        height: f32,
        #[serde(default)]
        falloff: Falloff,
    },
    #[serde(rename = "material.set")]
    MaterialSet {
        id: String,
        #[serde(default)]
        texture: Option<TextureRef>,
        #[serde(default)]
        tile: Option<f32>,
        #[serde(default)]
        surface_id: Option<u8>,
        #[serde(default)]
        flags: Option<MaterialFlags>,
        /// Fit this part of the texture `[u0, v0, u1, v1]` once across each face of a solid.
        #[serde(default)]
        fit: Option<[f32; 4]>,
    },
    #[serde(rename = "import.add")]
    ImportAdd {
        file: String,
        pos: Vec3f,
        #[serde(default)]
        rot_y_deg: f32,
        #[serde(default = "one")]
        scale: f32,
        material: String,
        #[serde(default = "yes")]
        collision: bool,
        #[serde(default)]
        ground: bool,
    },
    #[serde(rename = "prefab.add")]
    PrefabAdd {
        prefab: String,
        pos: Vec3f,
        #[serde(default)]
        rot_y_deg: f32,
        #[serde(default = "one")]
        scale: f32,
        #[serde(default)]
        ground: bool,
    },
    /// The world settings the build turns into `dc.world_settings`, `dc.global_light` and the sky model (every
    /// field optional: only the given ones change). The GUI's world panel is one of these per edit (MAP-61).
    #[serde(rename = "settings.set")]
    SettingsSet {
        #[serde(default)]
        fog_enable: Option<bool>,
        #[serde(default)]
        fog_near: Option<f32>,
        #[serde(default)]
        fog_far: Option<f32>,
        #[serde(default)]
        fog_color: Option<[u8; 3]>,
        #[serde(default)]
        far_z: Option<f32>,
        #[serde(default)]
        sun_dir: Option<Vec3f>,
        #[serde(default)]
        sun_color: Option<[u8; 3]>,
        #[serde(default)]
        sun_bright: Option<f32>,
        #[serde(default)]
        ambient: Option<[u8; 3]>,
        #[serde(default)]
        sky_size: Option<Vec3f>,
        #[serde(default)]
        sky_material: Option<String>,
    },
    /// The map's label and the mod metadata `export` writes (every field optional). `name` is the project's
    /// label; the mod id stays what `new` derived.
    #[serde(rename = "meta.set")]
    MetaSet {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        author: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        tags: Option<Vec<String>>,
        /// `""` clears it.
        #[serde(default)]
        homepage: Option<String>,
        #[serde(default)]
        modes: Option<Vec<String>>,
        #[serde(default)]
        goals: Option<BTreeMap<String, u32>>,
        /// 0 clears it.
        #[serde(default)]
        round_seconds: Option<u32>,
        #[serde(default)]
        players: Option<crate::project::Players>,
    },
}

impl Op {
    /// The `op` tag.
    pub fn name(&self) -> &'static str {
        match self {
            Op::Add { .. } => "add",
            Op::Remove { .. } => "remove",
            Op::Clear { .. } => "clear",
            Op::Move { .. } => "move",
            Op::Set { .. } => "set",
            Op::Rename { .. } => "rename",
            Op::SolidAdd { .. } => "solid.add",
            Op::SolidSet { .. } => "solid.set",
            Op::TerrainGen { .. } => "terrain.gen",
            Op::TerrainRaise { .. } => "terrain.raise",
            Op::TerrainLower { .. } => "terrain.lower",
            Op::TerrainSmooth { .. } => "terrain.smooth",
            Op::TerrainFlatten { .. } => "terrain.flatten",
            Op::MaterialSet { .. } => "material.set",
            Op::ImportAdd { .. } => "import.add",
            Op::PrefabAdd { .. } => "prefab.add",
            Op::SettingsSet { .. } => "settings.set",
            Op::MetaSet { .. } => "meta.set",
        }
    }

    /// Every op name, in schema order.
    pub const NAMES: &'static [&'static str] = &[
        "add",
        "remove",
        "clear",
        "move",
        "set",
        "rename",
        "solid.add",
        "solid.set",
        "terrain.gen",
        "terrain.raise",
        "terrain.lower",
        "terrain.smooth",
        "terrain.flatten",
        "material.set",
        "import.add",
        "prefab.add",
        "settings.set",
        "meta.set",
    ];
}

/// One example of every variant with every field present (the schema test and the round-trip test walk it).
pub fn example_ops() -> Vec<Op> {
    let mut props = BTreeMap::new();
    props.insert(v::PROP_TEAM.to_string(), PropValue::Text("1".into()));
    vec![
        Op::Add {
            class: v::CLASS_PLAYER_SPAWN.into(),
            name: Some("spawn_a".into()),
            pos: [0.0, 0.0, 0.0],
            rot: Some([0.0, 90.0, 0.0]),
            props,
            ground: true,
        },
        Op::Remove {
            id: "player_spawn_1".into(),
        },
        Op::Clear {
            group: Some("nothing_here".into()),
            class: Some(v::CLASS_ITEM_SPAWN.into()),
        },
        Op::Move {
            id: "player_spawn_1".into(),
            pos: Some([10.0, 0.0, 10.0]),
            delta: None,
            ground: true,
        },
        Op::Set {
            id: "player_spawn_1".into(),
            prop: v::PROP_TEAM.into(),
            value: PropValue::Text("2".into()),
        },
        Op::Rename {
            id: "player_spawn_1".into(),
            name: "spawn_b".into(),
        },
        Op::SolidAdd {
            kind: SolidSpec::Cylinder { sides: 8 },
            pos: [0.0, 0.0, 0.0],
            rot_y_deg: 0.0,
            size: [64.0, 64.0, 64.0],
            material: "ground".into(),
            faces: BTreeMap::from([("top".to_string(), "ground".to_string())]),
            group: Some("cover".into()),
            brush: None,
            ground: true,
            rises: None,
        },
        Op::SolidSet {
            id: "solid_1".into(),
            pos: Some([0.0; 3]),
            rot_y_deg: Some(45.0),
            size: Some([64.0; 3]),
            material: Some("ground".into()),
            faces: Some(BTreeMap::new()),
            group: Some("cover".into()),
            brush: Some(BrushRole::WorldModel),
            rises: None,
        },
        Op::TerrainGen {
            preset: Preset::Valley,
            seed: Some(1),
            amplitude: 128.0,
            base: 0.0,
            scale: 1024.0,
            octaves: 5,
            cells_x: Some(64),
            cells_z: Some(64),
            cell_size: Some(64.0),
        },
        Op::TerrainRaise {
            x: 0.0,
            z: 0.0,
            radius: 256.0,
            amount: 32.0,
            falloff: Falloff::Smooth,
        },
        Op::TerrainLower {
            x: 0.0,
            z: 0.0,
            radius: 256.0,
            amount: 32.0,
            falloff: Falloff::Linear,
        },
        Op::TerrainSmooth {
            x: Some(0.0),
            z: Some(0.0),
            radius: Some(256.0),
            passes: 2,
        },
        Op::TerrainFlatten {
            x: 0.0,
            z: 0.0,
            radius: 256.0,
            height: 0.0,
            falloff: Falloff::Flat,
        },
        Op::MaterialSet {
            id: "ground".into(),
            texture: Some(TextureRef::Own("assets/ground.png".into())),
            tile: Some(128.0),
            surface_id: Some(0),
            flags: Some(MaterialFlags::default()),
            fit: Some([0.0, 0.0, 1.0, 1.0]),
        },
        Op::ImportAdd {
            file: "assets/rock.obj".into(),
            pos: [0.0; 3],
            rot_y_deg: 0.0,
            scale: 1.0,
            material: "ground".into(),
            collision: true,
            ground: true,
        },
        Op::PrefabAdd {
            prefab: "crate".into(),
            pos: [0.0; 3],
            rot_y_deg: 0.0,
            scale: 1.0,
            ground: true,
        },
        Op::SettingsSet {
            fog_enable: Some(true),
            fog_near: Some(400.0),
            fog_far: Some(6000.0),
            fog_color: Some([150, 160, 180]),
            far_z: Some(12000.0),
            sun_dir: Some([0.3, -0.8, 0.5]),
            sun_color: Some([255, 244, 220]),
            sun_bright: Some(1.0),
            ambient: Some([90, 95, 110]),
            sky_size: Some([1024.0; 3]),
            sky_material: Some("ground".into()),
        },
        Op::MetaSet {
            name: Some("My Map".into()),
            version: Some("0.1.0".into()),
            author: Some("me".into()),
            description: Some("A community map.".into()),
            tags: Some(vec!["map".into()]),
            homepage: Some(String::new()),
            modes: Some(vec!["TDM".into(), "FFA".into()]),
            goals: Some(BTreeMap::from([("TDM".to_string(), 100)])),
            round_seconds: Some(600),
            players: Some(crate::project::Players { min: 2, max: 16 }),
        },
    ]
}

/// What one op did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedOp {
    pub op: String,
    pub ids: Vec<String>,
}

/// The result of a successful batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    pub applied: Vec<AppliedOp>,
    pub diagnostics: Diagnostics,
    /// The files a save would change: `map.json`, `terrain.r16`.
    pub changed: Vec<String>,
}

/// Apply a batch, all or nothing. On `Err` the project and heights are untouched.
pub fn apply(
    project: &mut Project,
    heights: &mut Heights,
    ops: &[Op],
) -> Result<Applied, Diagnostics> {
    let mut p = project.clone();
    let mut h = heights.clone();
    let mut applied = Vec::with_capacity(ops.len());
    let mut diagnostics = Diagnostics::new();
    for (i, op) in ops.iter().enumerate() {
        match apply_one(&mut p, &mut h, op, &mut diagnostics) {
            Ok(a) => applied.push(a),
            Err(mut d) => {
                d.path = format!(
                    "ops[{i}]{}{}",
                    if d.path.is_empty() { "" } else { "." },
                    d.path
                );
                let mut out = Diagnostics::one(d);
                out.extend(diagnostics);
                return Err(out);
            }
        }
    }
    let mut changed = Vec::new();
    if p != *project {
        changed.push(crate::project::MAP_FILE.to_string());
    }
    if h != *heights {
        changed.push(crate::project::TERRAIN_FILE.to_string());
    }
    *project = p;
    *heights = h;
    Ok(Applied {
        applied,
        diagnostics,
        changed,
    })
}

fn invalid(path: &str, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::new(Code::OpInvalid, path, msg)
}

fn unknown_id(id: &str) -> Diagnostic {
    Diagnostic::new(
        Code::IdUnknown,
        "id",
        format!("no placed thing has the id `{id}`"),
    )
    .with_fix("run `dc-mapkit get <dir> --json` to list the ids")
}

fn finite3(path: &str, p: Vec3f) -> Result<(), Diagnostic> {
    if p.iter().all(|c| c.is_finite()) {
        Ok(())
    } else {
        Err(invalid(path, format!("{p:?} is not finite")))
    }
}

fn apply_one(
    p: &mut Project,
    h: &mut Heights,
    op: &Op,
    diags: &mut Diagnostics,
) -> Result<AppliedOp, Diagnostic> {
    let name = op.name().to_string();
    let done = |ids: Vec<String>| {
        Ok(AppliedOp {
            op: name.clone(),
            ids,
        })
    };
    match op {
        Op::Add {
            class,
            name: obj_name,
            pos,
            rot,
            props,
            ground,
        } => {
            if !v::is_point_class(class) {
                return Err(Diagnostic::new(
                    Code::ObjectClass,
                    "class",
                    format!("`{class}` is not a point class of the DeCombat vocabulary"),
                )
                .with_fix(format!("one of: {}", v::POINT_CLASSES.join(", "))));
            }
            finite3("pos", *pos)?;
            let id = p.alloc_id(v::id_prefix(class));
            let mut o = MapObject {
                id: id.clone(),
                class: class.clone(),
                name: obj_name.clone().unwrap_or_else(|| id.clone()),
                pos: *pos,
                rot: rot.unwrap_or([0.0; 3]),
                props: props.clone(),
            };
            if *ground {
                snap_object(p, h, &mut o);
            }
            p.objects.push(o);
            done(vec![id])
        }
        // DC-DEFAULT: MAP-94 what a generator starts with so it can run again; materials, terrain, settings and
        // metadata stay, the assets stay on disk, and an emptied project hands out the same ids again
        Op::Clear { group, class } => {
            let mut ids = Vec::new();
            if group.is_none() && class.is_none() {
                ids = p.all_ids().into_iter().map(String::from).collect();
                p.objects.clear();
                p.solids.clear();
                p.prefabs.clear();
                p.imports.clear();
                p.next_id = 1;
            }
            if let Some(g) = group {
                ids.extend(
                    p.solids
                        .iter()
                        .filter(|s| s.group.as_deref() == Some(g.as_str()))
                        .map(|s| s.id.clone()),
                );
                p.solids.retain(|s| s.group.as_deref() != Some(g.as_str()));
            }
            if let Some(c) = class {
                ids.extend(
                    p.objects
                        .iter()
                        .filter(|o| o.class == *c)
                        .map(|o| o.id.clone()),
                );
                p.objects.retain(|o| o.class != *c);
            }
            done(ids)
        }
        Op::Remove { id } => {
            let before = p.all_ids().len();
            p.objects.retain(|o| o.id != *id);
            p.solids.retain(|s| s.id != *id);
            p.prefabs.retain(|s| s.id != *id);
            p.imports.retain(|s| s.id != *id);
            if p.all_ids().len() == before {
                return Err(unknown_id(id));
            }
            done(vec![id.clone()])
        }
        Op::Move {
            id,
            pos,
            delta,
            ground,
        } => {
            match (pos, delta) {
                (Some(_), Some(_)) => {
                    return Err(invalid("pos", "give `pos` or `delta`, not both"))
                }
                (None, None) if !ground => {
                    return Err(invalid("pos", "give `pos`, `delta` or `ground: true`"))
                }
                _ => {}
            }
            if let Some(q) = pos {
                finite3("pos", *q)?;
            }
            if let Some(q) = delta {
                finite3("delta", *q)?;
            }
            let apply_to = |cur: &mut Vec3f| {
                if let Some(q) = pos {
                    *cur = *q;
                } else if let Some(d) = delta {
                    *cur = [cur[0] + d[0], cur[1] + d[1], cur[2] + d[2]];
                }
            };
            if let Some(i) = p.objects.iter().position(|o| o.id == *id) {
                let mut o = p.objects[i].clone();
                apply_to(&mut o.pos);
                if *ground {
                    snap_object(p, h, &mut o);
                }
                p.objects[i] = o;
            } else if let Some(i) = p.solids.iter().position(|s| s.id == *id) {
                let mut s = p.solids[i].clone();
                apply_to(&mut s.pos);
                if *ground {
                    snap_solid(p, h, &mut s);
                }
                p.solids[i] = s;
            } else if let Some(i) = p.prefabs.iter().position(|s| s.id == *id) {
                apply_to(&mut p.prefabs[i].pos);
                if *ground {
                    let q = p.prefabs[i].pos;
                    if let Some(f) = ground_height(p, h, q[0], q[2]) {
                        p.prefabs[i].pos[1] = f;
                    }
                }
            } else if let Some(i) = p.imports.iter().position(|s| s.id == *id) {
                apply_to(&mut p.imports[i].pos);
                if *ground {
                    let q = p.imports[i].pos;
                    if let Some(f) = ground_height(p, h, q[0], q[2]) {
                        p.imports[i].pos[1] = f;
                    }
                }
            } else {
                return Err(unknown_id(id));
            }
            done(vec![id.clone()])
        }
        Op::Set { id, prop, value } => {
            if p.solid(id).is_some() {
                return Err(invalid(
                    "prop",
                    "`set` edits objects; use `solid.set` for a solid",
                ));
            }
            let Some(o) = p.object_mut(id) else {
                return Err(unknown_id(id));
            };
            match prop.as_str() {
                "name" => match value {
                    PropValue::Text(t) => o.name = t.clone(),
                    other => {
                        return Err(invalid(
                            "value",
                            format!("`name` takes text, not {}", other.kind()),
                        ))
                    }
                },
                "pos" | "rot" => match value {
                    PropValue::Vec3(q) => {
                        finite3("value", *q)?;
                        if prop == "pos" {
                            o.pos = *q
                        } else {
                            o.rot = *q
                        }
                    }
                    other => {
                        return Err(invalid(
                            "value",
                            format!("`{prop}` takes a vec3, not {}", other.kind()),
                        ))
                    }
                },
                "id" | "class" => return Err(invalid("prop", format!("`{prop}` cannot be set"))),
                _ => {
                    if let PropValue::Vec3(q) = value {
                        finite3("value", *q)?;
                    }
                    o.props.insert(prop.clone(), value.clone());
                }
            }
            done(vec![id.clone()])
        }
        Op::Rename { id, name: new_name } => {
            let Some(o) = p.object_mut(id) else {
                return Err(if p.has_id(id) {
                    invalid(
                        "id",
                        "only objects have a name (a solid's brush name is its id)",
                    )
                } else {
                    unknown_id(id)
                });
            };
            o.name = new_name.clone();
            done(vec![id.clone()])
        }
        Op::SolidAdd {
            kind,
            pos,
            rot_y_deg,
            size,
            material,
            faces,
            group,
            brush,
            ground,
            rises,
        } => {
            finite3("pos", *pos)?;
            finite3("size", *size)?;
            let rot_y_deg = match rises {
                None => *rot_y_deg,
                Some(_) if !matches!(kind, SolidSpec::Wedge) => {
                    return Err(invalid("rises", "`rises` is for a wedge"));
                }
                Some(_) if *rot_y_deg != 0.0 => {
                    return Err(invalid(
                        "rises",
                        "give `rises` or `rot_y_deg`, not both (rises sets the yaw)",
                    ));
                }
                Some(r) => r.rot_y_deg(),
            };
            let id = p.alloc_id("solid");
            let mut s = Solid {
                id: id.clone(),
                kind: kind.to_kind(),
                pos: *pos,
                rot_y_deg,
                size: *size,
                material: material.clone(),
                faces: faces.clone(),
                group: group.clone(),
                brush: *brush,
            };
            check_solid(p, &s, diags)?;
            if *ground {
                snap_solid(p, h, &mut s);
            }
            p.solids.push(s);
            done(vec![id])
        }
        Op::SolidSet {
            id,
            pos,
            rot_y_deg,
            size,
            material,
            faces,
            group,
            brush,
            rises,
        } => {
            let Some(mut s) = p.solid(id).cloned() else {
                return Err(unknown_id(id));
            };
            if let Some(r) = rises {
                if s.kind != SolidKind::Wedge {
                    return Err(invalid("rises", "`rises` is for a wedge"));
                }
                if rot_y_deg.is_some() {
                    return Err(invalid(
                        "rises",
                        "give `rises` or `rot_y_deg`, not both (rises sets the yaw)",
                    ));
                }
                s.rot_y_deg = r.rot_y_deg();
            }
            if let Some(q) = pos {
                finite3("pos", *q)?;
                s.pos = *q;
            }
            if let Some(r) = rot_y_deg {
                s.rot_y_deg = *r;
            }
            if let Some(q) = size {
                finite3("size", *q)?;
                s.size = *q;
            }
            if let Some(m) = material {
                s.material = m.clone();
            }
            if let Some(f) = faces {
                s.faces = f.clone();
            }
            if let Some(g) = group {
                s.group = Some(g.clone());
            }
            if let Some(b) = brush {
                s.brush = Some(*b);
            }
            check_solid(p, &s, diags)?;
            *p.solid_mut(id).expect("checked") = s;
            done(vec![id.clone()])
        }
        Op::TerrainGen {
            preset,
            seed,
            amplitude,
            base,
            scale,
            octaves,
            cells_x,
            cells_z,
            cell_size,
        } => {
            // DC-DEFAULT: MAP-93 amplitude 0 is a flat floor, the same as the `flat` preset
            if !(*amplitude >= 0.0 && amplitude.is_finite()) {
                return Err(invalid(
                    "amplitude",
                    "amplitude must be 0 (flat) or positive",
                ));
            }
            if !(*scale > 0.0 && scale.is_finite()) {
                return Err(invalid("scale", "scale must be positive"));
            }
            let flat = *preset == Preset::Flat || *amplitude == 0.0;
            let amplitude = if flat { &0.0 } else { amplitude };
            if !(1..=12).contains(octaves) {
                return Err(invalid("octaves", "octaves must be 1..12"));
            }
            let seed = seed.unwrap_or(p.seed);
            if p.terrain.is_none() || cells_x.is_some() || cells_z.is_some() || cell_size.is_some()
            {
                let cx = cells_x.unwrap_or(64);
                let cz = cells_z.unwrap_or(64);
                let cs = cell_size.unwrap_or(64.0);
                if cx == 0 || cz == 0 || cx > 1024 || cz > 1024 || cs <= 0.0 || !cs.is_finite() {
                    return Err(invalid(
                        "cells_x",
                        "cells must be 1..1024 and cell_size positive",
                    ));
                }
                let old = p.terrain.clone().unwrap_or_default();
                // a flat floor keeps the default height range, so the brushes can still raise and lower it
                let span = if flat {
                    default_amplitude()
                } else {
                    *amplitude
                };
                let t = Terrain {
                    cells_x: cx,
                    cells_z: cz,
                    cell_size: cs,
                    origin: [
                        -(cx as f32 * cs) / 2.0,
                        base - 2.0 * span,
                        -(cz as f32 * cs) / 2.0,
                    ],
                    height_scale: 4.0 * span,
                    ..old
                };
                *h = Heights::for_terrain(&t, 0);
                p.terrain = Some(t);
            }
            let t = p.terrain.as_ref().expect("terrain set");
            if h.cols != t.cols() || h.rows != t.rows() {
                *h = Heights::for_terrain(t, 0);
            }
            generate(t, h, *preset, seed, *amplitude, *base, *scale, *octaves);
            done(Vec::new())
        }
        Op::TerrainRaise {
            x,
            z,
            radius,
            amount,
            falloff,
        } => {
            let t = need_terrain(p, h)?;
            check_brush(*x, *z, *radius)?;
            brush(t, h, *x, *z, *radius, *falloff, |y, w| y + amount * w);
            done(Vec::new())
        }
        Op::TerrainLower {
            x,
            z,
            radius,
            amount,
            falloff,
        } => {
            let t = need_terrain(p, h)?;
            check_brush(*x, *z, *radius)?;
            brush(t, h, *x, *z, *radius, *falloff, |y, w| y - amount * w);
            done(Vec::new())
        }
        Op::TerrainFlatten {
            x,
            z,
            radius,
            height,
            falloff,
        } => {
            let t = need_terrain(p, h)?;
            check_brush(*x, *z, *radius)?;
            if !height.is_finite() {
                return Err(invalid("height", "height must be finite"));
            }
            brush(t, h, *x, *z, *radius, *falloff, |y, w| y + (height - y) * w);
            done(Vec::new())
        }
        Op::TerrainSmooth {
            x,
            z,
            radius,
            passes,
        } => {
            let t = need_terrain(p, h)?;
            if !(1..=64).contains(passes) {
                return Err(invalid("passes", "passes must be 1..64"));
            }
            let region = match (x, z, radius) {
                (Some(x), Some(z), Some(r)) => {
                    check_brush(*x, *z, *r)?;
                    Some((*x, *z, *r))
                }
                (None, None, None) => None,
                _ => {
                    return Err(invalid(
                        "x",
                        "give x, z and radius together, or none for the whole terrain",
                    ))
                }
            };
            for _ in 0..*passes {
                smooth(t, h, region);
            }
            done(Vec::new())
        }
        Op::MaterialSet {
            id,
            texture,
            tile,
            surface_id,
            flags,
            fit,
        } => {
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(invalid("id", "a material id is letters, digits and `_`"));
            }
            if let Some(TextureRef::Own(path)) = texture {
                crate::project::check_rel_path(path)
                    .map_err(|e| Diagnostic::new(Code::PathInvalid, "texture", e))?;
            }
            if let Some(t) = tile {
                if !(*t > 0.0 && t.is_finite()) {
                    return Err(invalid("tile", "tile must be positive"));
                }
            }
            // DC-DEFAULT: MAP-97 a reversed region mirrors the picture, an empty one has nothing to show
            if let Some(r) = fit {
                if r.iter().any(|c| !c.is_finite()) || r[0] == r[2] || r[1] == r[3] {
                    return Err(invalid(
                        "fit",
                        "fit is [u0, v0, u1, v1] with u0 != u1 and v0 != v1",
                    ));
                }
            }
            let created = p.material(id).is_none();
            if created {
                p.materials.push(Material {
                    id: id.clone(),
                    ..Default::default()
                });
            }
            let m = p
                .materials
                .iter_mut()
                .find(|m| m.id == *id)
                .expect("just pushed");
            if let Some(t) = texture {
                m.texture = t.clone();
            }
            if let Some(t) = tile {
                m.tile = *t;
            }
            if let Some(r) = fit {
                m.fit = Some(*r);
            }
            if let Some(s) = surface_id {
                m.surface_id = *s;
            }
            if let Some(f) = flags {
                m.flags = f.clone();
            }
            done(vec![id.clone()])
        }
        Op::ImportAdd {
            file,
            pos,
            rot_y_deg,
            scale,
            material,
            collision,
            ground,
        } => {
            crate::project::check_rel_path(file)
                .map_err(|e| Diagnostic::new(Code::PathInvalid, "file", e))?;
            finite3("pos", *pos)?;
            if !(*scale > 0.0 && scale.is_finite()) {
                return Err(invalid("scale", "scale must be positive"));
            }
            if p.material(material).is_none() {
                return Err(material_missing(material));
            }
            let id = p.alloc_id("import");
            let mut q = *pos;
            if *ground {
                if let Some(f) = ground_height(p, h, q[0], q[2]) {
                    q[1] = f;
                }
            }
            p.imports.push(Import {
                id: id.clone(),
                file: file.clone(),
                pos: q,
                rot_y_deg: *rot_y_deg,
                scale: *scale,
                material: material.clone(),
                collision: *collision,
            });
            done(vec![id])
        }
        Op::PrefabAdd {
            prefab,
            pos,
            rot_y_deg,
            scale,
            ground,
        } => {
            if prefab.is_empty() {
                return Err(invalid("prefab", "prefab name is empty"));
            }
            finite3("pos", *pos)?;
            if !(*scale > 0.0 && scale.is_finite()) {
                return Err(invalid("scale", "scale must be positive"));
            }
            let id = p.alloc_id("prefab");
            let mut q = *pos;
            if *ground {
                if let Some(f) = ground_height(p, h, q[0], q[2]) {
                    q[1] = f;
                }
            }
            p.prefabs.push(PrefabInstance {
                id: id.clone(),
                prefab: prefab.clone(),
                pos: q,
                rot_y_deg: *rot_y_deg,
                scale: *scale,
            });
            done(vec![id])
        }
        // DC-DEFAULT: MAP-61 (world settings and mod metadata are ops too, so the GUI, the CLI and scripts share
        // one write path and one history)
        Op::SettingsSet {
            fog_enable,
            fog_near,
            fog_far,
            fog_color,
            far_z,
            sun_dir,
            sun_color,
            sun_bright,
            ambient,
            sky_size,
            sky_material,
        } => {
            let finite = |path: &str, x: Option<f32>| -> Result<(), Diagnostic> {
                match x {
                    Some(v) if !v.is_finite() => Err(invalid(path, format!("{v} is not finite"))),
                    _ => Ok(()),
                }
            };
            finite("fog_near", *fog_near)?;
            finite("fog_far", *fog_far)?;
            finite("sun_bright", *sun_bright)?;
            if let Some(f) = far_z {
                if !(*f > 0.0 && f.is_finite()) {
                    return Err(invalid("far_z", "far_z must be positive"));
                }
            }
            if let Some(d) = sun_dir {
                finite3("sun_dir", *d)?;
                if d.iter().all(|c| c.abs() < 1e-6) {
                    return Err(invalid("sun_dir", "sun_dir is the zero vector"));
                }
            }
            if let Some(s) = sky_size {
                finite3("sky_size", *s)?;
                if s.iter().any(|c| *c <= 0.0) {
                    return Err(invalid(
                        "sky_size",
                        "sky_size must be positive on every axis",
                    ));
                }
            }
            if let Some(m) = sky_material {
                if p.material(m).is_none() {
                    return Err(Diagnostic::new(
                        Code::MaterialMissing,
                        "sky_material",
                        format!("material `{m}` is not defined"),
                    )
                    .with_fix("add it with material.set first"));
                }
            }
            let s = &mut p.settings;
            if let Some(x) = fog_enable {
                s.fog_enable = *x;
            }
            if let Some(x) = fog_near {
                s.fog_near = *x;
            }
            if let Some(x) = fog_far {
                s.fog_far = *x;
            }
            if let Some(x) = fog_color {
                s.fog_color = *x;
            }
            if let Some(x) = far_z {
                s.far_z = *x;
            }
            if let Some(x) = sun_dir {
                s.sun_dir = *x;
            }
            if let Some(x) = sun_color {
                s.sun_color = *x;
            }
            if let Some(x) = sun_bright {
                s.sun_bright = *x;
            }
            if let Some(x) = ambient {
                s.ambient = *x;
            }
            if let Some(x) = sky_size {
                s.sky_size = *x;
            }
            if let Some(x) = sky_material {
                s.sky_material = x.clone();
            }
            done(Vec::new())
        }
        Op::MetaSet {
            name,
            version,
            author,
            description,
            tags,
            homepage,
            modes,
            goals,
            round_seconds,
            players,
        } => {
            if let Some(n) = name {
                if n.trim().is_empty() {
                    return Err(invalid("name", "name is empty"));
                }
            }
            let mode_ok = |m: &str| v::MODE_NAMES.iter().any(|k| k.eq_ignore_ascii_case(m));
            if let Some(ms) = modes {
                if let Some(bad) = ms.iter().find(|m| !mode_ok(m)) {
                    return Err(Diagnostic::new(
                        Code::ModeUnknown,
                        "modes",
                        format!("`{bad}` is not a DeCombat mode name"),
                    )
                    .with_severity(crate::diag::Severity::Error)
                    .with_fix(format!("one of: {}", v::MODE_NAMES.join(", "))));
                }
            }
            if let Some(gs) = goals {
                if let Some((bad, _)) = gs.iter().find(|(m, _)| !mode_ok(m)) {
                    return Err(Diagnostic::new(
                        Code::ModeUnknown,
                        "goals",
                        format!("`{bad}` is not a DeCombat mode name"),
                    )
                    .with_severity(crate::diag::Severity::Error));
                }
            }
            if let Some(pl) = players {
                if pl.min == 0 || pl.max < pl.min || pl.max > 16 {
                    return Err(invalid("players", "want 1 <= min <= max <= 16"));
                }
            }
            if let Some(n) = name {
                p.name = n.trim().to_string();
            }
            let m = &mut p.mod_meta;
            if let Some(x) = version {
                m.version = x.clone();
            }
            if let Some(x) = author {
                m.author = x.clone();
            }
            if let Some(x) = description {
                m.description = x.clone();
            }
            if let Some(x) = tags {
                m.tags = x.clone();
            }
            if let Some(x) = homepage {
                m.homepage = (!x.trim().is_empty()).then(|| x.trim().to_string());
            }
            if let Some(x) = modes {
                m.modes = x.iter().map(|s| s.to_ascii_uppercase()).collect();
            }
            if let Some(x) = goals {
                m.goals = x
                    .iter()
                    .map(|(k, g)| (k.to_ascii_uppercase(), *g))
                    .collect();
            }
            if let Some(x) = round_seconds {
                m.round_seconds = (*x > 0).then_some(*x);
            }
            if let Some(x) = players {
                m.players = Some(x.clone());
            }
            done(Vec::new())
        }
    }
}

fn material_missing(id: &str) -> Diagnostic {
    Diagnostic::new(Code::MaterialMissing, "material", format!("material `{id}` is not defined"))
        .with_fix(format!("add it first: {{\"op\": \"material.set\", \"id\": \"{id}\", \"texture\": {{\"own\": \"assets/<file>.png\"}}}}"))
}

/// A solid's shape and material references.
fn check_solid(p: &Project, s: &Solid, _diags: &mut Diagnostics) -> Result<(), Diagnostic> {
    if s.size.iter().any(|c| !(*c > 0.0 && c.is_finite())) {
        return Err(Diagnostic::new(
            Code::SolidSize,
            "size",
            format!("size {:?} is not positive", s.size),
        ));
    }
    crate::project::check_solid_corners(s)
        .map_err(|e| Diagnostic::new(Code::SolidShape, "kind", e))?;
    geom::solid_faces(s).map_err(|e| Diagnostic::new(Code::SolidShape, "kind", e))?;
    if p.material(&s.material).is_none() {
        return Err(material_missing(&s.material));
    }
    for (face, m) in &s.faces {
        if p.material(m).is_none() {
            let mut d = material_missing(m);
            d.path = format!("faces.{face}");
            return Err(d);
        }
    }
    Ok(())
}

fn need_terrain<'a>(p: &'a Project, h: &Heights) -> Result<&'a Terrain, Diagnostic> {
    let Some(t) = p.terrain.as_ref() else {
        return Err(Diagnostic::new(
            Code::TerrainMissing,
            "terrain",
            "the project has no terrain",
        )
        .with_fix("run {\"op\": \"terrain.gen\", \"preset\": \"hills\"} first"));
    };
    if h.cols != t.cols() || h.rows != t.rows() {
        return Err(Diagnostic::new(
            Code::TerrainSize,
            "terrain",
            format!(
                "heights are {} x {}, terrain wants {} x {}",
                h.cols,
                h.rows,
                t.cols(),
                t.rows()
            ),
        ));
    }
    Ok(t)
}

fn check_brush(x: f32, z: f32, radius: f32) -> Result<(), Diagnostic> {
    if !x.is_finite() || !z.is_finite() || !(radius > 0.0 && radius.is_finite()) {
        return Err(invalid(
            "radius",
            "x and z must be finite and radius positive",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// ground snapping

/// The top of a solid under world (x, z): boxes, cylinders and prisms (their top is flat); `None` for a wedge
/// (TODO(mapkit-stage2): ray cast against the brush polygons once the build geometry is at hand) or outside.
pub fn solid_top_at(s: &Solid, x: f32, z: f32) -> Option<f32> {
    if matches!(
        s.brush,
        Some(BrushRole::KillVolume) | Some(BrushRole::Sky) | Some(BrushRole::Ladder)
    ) {
        return None;
    }
    let xf = geom::Xform::new(s.pos, s.rot_y_deg, 1.0);
    let l = xf.unapply(V3::new(x, 0.0, z));
    let (hx, hz) = (s.size[0] / 2.0, s.size[2] / 2.0);
    let inside = match &s.kind {
        SolidKind::Box => l.x.abs() <= hx && l.z.abs() <= hz,
        SolidKind::Wedge => return None,
        SolidKind::Cylinder { circle, .. } => {
            let pts: Vec<[f32; 2]> = circle.iter().map(|c| [c[0] * hx, c[1] * hz]).collect();
            inside_outline(&pts, l.x, l.z)
        }
        SolidKind::Prism { points } => inside_outline(points, l.x, l.z),
    };
    if inside {
        Some(s.pos[1] + s.size[1] / 2.0)
    } else {
        None
    }
}

fn inside_outline(pts: &[[f32; 2]], x: f32, z: f32) -> bool {
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut pos = false;
    let mut neg = false;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let t = (b[0] - a[0]) * (z - a[1]) - (b[1] - a[1]) * (x - a[0]);
        if t > 0.0 {
            pos = true;
        } else if t < 0.0 {
            neg = true;
        }
    }
    !(pos && neg)
}

/// A level solid whose top face carries a sky material (the templates' sky lid): never a floor (MAP-11; the
/// stage-2 build found the lid snapping every spawn onto the sky).
pub fn is_sky_solid(p: &Project, s: &Solid) -> bool {
    p.material(geom::face_material(s, "top"))
        .is_some_and(|m| m.flags.sky)
}

/// The floor height under world (x, z): the terrain, or the highest solid top, whichever is higher.
pub fn ground_height(p: &Project, h: &Heights, x: f32, z: f32) -> Option<f32> {
    let mut best = p.terrain.as_ref().and_then(|t| h.sample(t, x, z));
    for s in p.solids.iter().filter(|s| !is_sky_solid(p, s)) {
        if let Some(top) = solid_top_at(s, x, z) {
            best = Some(best.map_or(top, |b: f32| b.max(top)));
        }
    }
    best
}

/// The standing offset of an object above the floor (DC-DEFAULT: MAP-11).
pub fn stand_offset(o: &MapObject) -> f32 {
    if o.class == v::CLASS_PLAYER_SPAWN {
        return STAND_OFFSET;
    }
    if let Some(e) = o.props.get(v::PROP_EXTENT).and_then(PropValue::as_vec3) {
        return e[1];
    }
    0.0
}

/// Snap an object's y to the floor plus its standing offset (kept when nothing is under it).
pub fn snap_object(p: &Project, h: &Heights, o: &mut MapObject) {
    if let Some(f) = ground_height(p, h, o.pos[0], o.pos[2]) {
        o.pos[1] = f + stand_offset(o);
    }
}

/// Snap a solid so its bottom rests on the floor (the solid itself is not counted).
pub fn snap_solid(p: &Project, h: &Heights, s: &mut Solid) {
    let mut best = p
        .terrain
        .as_ref()
        .and_then(|t| h.sample(t, s.pos[0], s.pos[2]));
    for other in p
        .solids
        .iter()
        .filter(|o| o.id != s.id && !is_sky_solid(p, o))
    {
        if let Some(top) = solid_top_at(other, s.pos[0], s.pos[2]) {
            best = Some(best.map_or(top, |b: f32| b.max(top)));
        }
    }
    if let Some(f) = best {
        s.pos[1] = f + s.size[1] / 2.0;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// terrain

#[allow(clippy::too_many_arguments)]
fn generate(
    t: &Terrain,
    h: &mut Heights,
    preset: Preset,
    seed: u64,
    amplitude: f32,
    base: f32,
    scale: f32,
    octaves: u32,
) {
    let n = noise::Noise::new(seed);
    let (x0, x1) = t.span_x();
    let half_w = (x1 - x0) / 2.0;
    let centre_x = (x0 + x1) / 2.0;
    for z in 0..t.rows() {
        for x in 0..t.cols() {
            let wx = t.origin[0] + x as f32 * t.cell_size;
            let wz = t.origin[2] + z as f32 * t.cell_size;
            let mut y = base + amplitude * n.fbm(wx / scale, wz / scale, octaves);
            // the band: a smooth profile over the central half of the width
            let u = ((wx - centre_x).abs() / (half_w * 0.5)).min(1.0);
            let band = 1.0 - (3.0 * u * u - 2.0 * u * u * u);
            match preset {
                Preset::Hills | Preset::Flat => {}
                Preset::Valley => y -= amplitude * band,
                Preset::Ridge => y += amplitude * band,
            }
            h.set(x, z, t.y_to_raw(y));
        }
    }
}

fn brush(
    t: &Terrain,
    h: &mut Heights,
    cx: f32,
    cz: f32,
    radius: f32,
    falloff: Falloff,
    f: impl Fn(f32, f32) -> f32,
) {
    for z in 0..t.rows() {
        for x in 0..t.cols() {
            let wx = t.origin[0] + x as f32 * t.cell_size;
            let wz = t.origin[2] + z as f32 * t.cell_size;
            let d = ((wx - cx) * (wx - cx) + (wz - cz) * (wz - cz)).sqrt();
            if d >= radius {
                continue;
            }
            let w = falloff.weight(d / radius);
            let y = t.raw_to_y(h.get(x, z));
            h.set(x, z, t.y_to_raw(f(y, w)));
        }
    }
}

fn smooth(t: &Terrain, h: &mut Heights, region: Option<(f32, f32, f32)>) {
    let src = h.clone();
    let (cols, rows) = (t.cols() as i64, t.rows() as i64);
    for z in 0..t.rows() {
        for x in 0..t.cols() {
            let mut w = 1.0;
            if let Some((cx, cz, r)) = region {
                let wx = t.origin[0] + x as f32 * t.cell_size;
                let wz = t.origin[2] + z as f32 * t.cell_size;
                let d = ((wx - cx) * (wx - cx) + (wz - cz) * (wz - cz)).sqrt();
                if d >= r {
                    continue;
                }
                w = Falloff::Smooth.weight(d / r);
            }
            let mut sum = 0.0f32;
            let mut count = 0.0f32;
            for dz in -1..=1i64 {
                for dx in -1..=1i64 {
                    let (xx, zz) = (x as i64 + dx, z as i64 + dz);
                    if xx < 0 || zz < 0 || xx >= cols || zz >= rows {
                        continue;
                    }
                    sum += src.get(xx as usize, zz as usize) as f32;
                    count += 1.0;
                }
            }
            let cur = src.get(x, z) as f32;
            let target = sum / count;
            let v = cur + (target - cur) * w;
            h.set(x, z, v.round().clamp(0.0, 65535.0) as u16);
        }
    }
}

/// Seeded value noise / fBm with a run-time permutation (no literal table).
pub mod noise {
    /// splitmix64: a small, well-known PRNG step (public algorithm; Steele, Lea, Flood 2014).
    pub fn splitmix64(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Value noise over a 256-entry permutation shuffled from the seed (Fisher-Yates over splitmix64).
    pub struct Noise {
        perm: Vec<u16>,
    }

    impl Noise {
        pub fn new(seed: u64) -> Noise {
            let mut perm: Vec<u16> = (0..256).collect();
            let mut s = seed ^ 0x5DEE_CE66_D1CE_B00Cu64;
            for i in (1..256usize).rev() {
                let j = (splitmix64(&mut s) % (i as u64 + 1)) as usize;
                perm.swap(i, j);
            }
            let mut doubled = perm.clone();
            doubled.extend_from_slice(&perm);
            Noise { perm: doubled }
        }

        /// Lattice value in [-1, 1] at integer (x, y).
        fn lattice(&self, x: i32, y: i32) -> f32 {
            let xi = (x & 255) as usize;
            let yi = (y & 255) as usize;
            let v = self.perm[self.perm[xi] as usize + yi];
            v as f32 / 127.5 - 1.0
        }

        /// Smooth value noise in [-1, 1] (quintic interpolation; same operations in the same order every call).
        pub fn value2(&self, x: f32, y: f32) -> f32 {
            let fx = x.floor();
            let fy = y.floor();
            let (xi, yi) = (fx as i32, fy as i32);
            let (tx, ty) = (x - fx, y - fy);
            let sx = tx * tx * tx * (tx * (tx * 6.0 - 15.0) + 10.0);
            let sy = ty * ty * ty * (ty * (ty * 6.0 - 15.0) + 10.0);
            let a = self.lattice(xi, yi);
            let b = self.lattice(xi + 1, yi);
            let c = self.lattice(xi, yi + 1);
            let d = self.lattice(xi + 1, yi + 1);
            let top = a + (b - a) * sx;
            let bottom = c + (d - c) * sx;
            top + (bottom - top) * sy
        }

        /// Fractional Brownian motion: `octaves` layers, lacunarity 2, gain 0.5, normalised to about [-1, 1].
        pub fn fbm(&self, x: f32, y: f32, octaves: u32) -> f32 {
            let mut sum = 0.0;
            let mut amp = 1.0;
            let mut norm = 0.0;
            let mut fx = x;
            let mut fy = y;
            for _ in 0..octaves.max(1) {
                sum += amp * self.value2(fx, fy);
                norm += amp;
                amp *= 0.5;
                fx *= 2.0;
                fy *= 2.0;
            }
            sum / norm
        }
    }
}

/// The schema document (`dc-mapkit schema`): see [`crate::schema::schema`].
pub fn schema(ops_only: bool) -> serde_json::Value {
    crate::schema::schema(ops_only)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Material;

    fn base_project() -> (Project, Heights) {
        let mut p = Project {
            terrain: Some(Terrain {
                cells_x: 8,
                cells_z: 8,
                cell_size: 64.0,
                origin: [-256.0, -256.0, -256.0],
                height_scale: 1024.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        p.materials.push(Material {
            id: "ground".into(),
            ..Default::default()
        });
        let t = p.terrain.as_ref().unwrap();
        let h = Heights::for_terrain(t, t.y_to_raw(0.0));
        (p, h)
    }

    #[test]
    fn every_op_round_trips_through_json() {
        for op in example_ops() {
            let text = serde_json::to_string(&op).unwrap();
            let back: Op = serde_json::from_str(&text).unwrap();
            assert_eq!(back, op, "{text}");
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["op"], op.name());
        }
        let names: Vec<&str> = example_ops().iter().map(|o| o.name()).collect();
        assert_eq!(names, Op::NAMES);
        // defaults let an agent write the short form
        let op: Op =
            serde_json::from_str("{\"op\":\"add\",\"class\":\"dc.item_spawn\",\"pos\":[1,2,3]}")
                .unwrap();
        assert!(matches!(op, Op::Add { ground: false, .. }));
        let op: Op = serde_json::from_str("{\"op\":\"terrain.gen\",\"preset\":\"hills\"}").unwrap();
        assert!(matches!(op, Op::TerrainGen { amplitude, octaves: 5, .. } if amplitude == 256.0));
        assert!(serde_json::from_str::<Op>("{\"op\":\"fly\"}").is_err());
    }

    #[test]
    fn every_variant_applies_and_echoes_ids() {
        let (mut p, mut h) = base_project();
        let ops = example_ops();
        // the example batch refers to player_spawn_1 and solid_1: add puts the spawn at id 1, the solid at 2;
        // rewrite the solid.set target accordingly
        let ops: Vec<Op> = ops
            .into_iter()
            .map(|o| match o {
                Op::SolidSet {
                    id: _,
                    pos,
                    rot_y_deg,
                    size,
                    material,
                    faces,
                    group,
                    brush,
                    rises,
                } => Op::SolidSet {
                    id: "solid_2".into(),
                    pos,
                    rot_y_deg,
                    size,
                    material,
                    faces,
                    group,
                    brush,
                    rises,
                },
                Op::Remove { .. } => Op::Remove {
                    id: "player_spawn_1".into(),
                },
                o => o,
            })
            .collect();
        // `remove` of the spawn would break the later move/set/rename: apply remove last instead
        let mut reordered: Vec<Op> = ops
            .iter()
            .filter(|o| !matches!(o, Op::Remove { .. }))
            .cloned()
            .collect();
        reordered.push(Op::Remove {
            id: "player_spawn_1".into(),
        });
        let before = p.clone();
        let r = apply(&mut p, &mut h, &reordered).unwrap();
        assert_eq!(r.applied.len(), reordered.len());
        assert_eq!(r.applied[0].ids, vec!["player_spawn_1"]);
        assert_eq!(r.applied[0].op, "add");
        assert_eq!(r.changed, vec!["map.json", "terrain.r16"]);
        assert!(p.objects.is_empty(), "removed at the end");
        assert_eq!(p.solids.len(), 1);
        assert_eq!(p.solids[0].rot_y_deg, 45.0);
        assert_eq!(p.solids[0].brush, Some(BrushRole::WorldModel));
        assert_eq!(p.imports.len(), 1);
        assert_eq!(p.prefabs.len(), 1);
        assert!(p.next_id > before.next_id);
        assert!(
            matches!(p.solids[0].kind, SolidKind::Cylinder { sides: 8, ref circle } if circle.len() == 8)
        );
    }

    #[test]
    fn a_bad_op_refuses_the_whole_batch() {
        let (mut p, mut h) = base_project();
        let before = (p.clone(), h.clone());
        let ops = vec![
            Op::Add {
                class: v::CLASS_ITEM_SPAWN.into(),
                name: None,
                pos: [0.0; 3],
                rot: None,
                props: BTreeMap::new(),
                ground: false,
            },
            Op::Remove {
                id: "nothing_9".into(),
            },
        ];
        let e = apply(&mut p, &mut h, &ops).unwrap_err();
        assert_eq!(e.items[0].code, Code::IdUnknown);
        assert_eq!(e.items[0].path, "ops[1].id");
        assert_eq!((p, h), before);
    }

    #[test]
    fn add_refuses_unknown_classes_and_set_edits_props_and_fixed_fields() {
        let (mut p, mut h) = base_project();
        let e = apply(
            &mut p,
            &mut h,
            &[Op::Add {
                class: "dc.kill_volume".into(),
                name: None,
                pos: [0.0; 3],
                rot: None,
                props: BTreeMap::new(),
                ground: false,
            }],
        )
        .unwrap_err();
        assert_eq!(e.items[0].code, Code::ObjectClass);
        let r = apply(
            &mut p,
            &mut h,
            &[
                Op::Add {
                    class: v::CLASS_BOMB_SITE.into(),
                    name: None,
                    pos: [0.0; 3],
                    rot: None,
                    props: BTreeMap::new(),
                    ground: false,
                },
                Op::Set {
                    id: "bomb_site_1".into(),
                    prop: "number".into(),
                    value: PropValue::Num(1.0),
                },
                Op::Set {
                    id: "bomb_site_1".into(),
                    prop: "pos".into(),
                    value: PropValue::Vec3([1.0, 2.0, 3.0]),
                },
                Op::Rename {
                    id: "bomb_site_1".into(),
                    name: "site_b".into(),
                },
            ],
        )
        .unwrap();
        assert_eq!(r.changed, vec!["map.json"]);
        let o = p.object("bomb_site_1").unwrap();
        assert_eq!(o.num("number"), Some(1.0));
        assert_eq!(o.pos, [1.0, 2.0, 3.0]);
        assert_eq!(o.name, "site_b");
        let e = apply(
            &mut p,
            &mut h,
            &[Op::Set {
                id: "bomb_site_1".into(),
                prop: "pos".into(),
                value: PropValue::Text("x".into()),
            }],
        )
        .unwrap_err();
        assert_eq!(e.items[0].code, Code::OpInvalid);
    }

    #[test]
    fn ground_snaps_to_terrain_and_solid_tops() {
        let (mut p, mut h) = base_project();
        // terrain at y = 0 everywhere; a box 100 tall centred at y = 50 covers x,z in [-50, 50]
        let r = apply(
            &mut p,
            &mut h,
            &[
                Op::SolidAdd {
                    kind: SolidSpec::Box,
                    pos: [0.0, 999.0, 0.0],
                    rot_y_deg: 0.0,
                    size: [100.0, 100.0, 100.0],
                    material: "ground".into(),
                    faces: BTreeMap::new(),
                    group: None,
                    brush: None,
                    ground: true,
                    rises: None,
                },
                Op::Add {
                    class: v::CLASS_PLAYER_SPAWN.into(),
                    name: None,
                    pos: [10.0, 999.0, 10.0],
                    rot: None,
                    props: BTreeMap::new(),
                    ground: true,
                },
                Op::Add {
                    class: v::CLASS_PLAYER_SPAWN.into(),
                    name: None,
                    pos: [200.0, 999.0, 200.0],
                    rot: None,
                    props: BTreeMap::new(),
                    ground: true,
                },
                Op::Add {
                    class: v::CLASS_PLAYER_SPAWN.into(),
                    name: None,
                    pos: [5000.0, 999.0, 5000.0],
                    rot: None,
                    props: BTreeMap::new(),
                    ground: true,
                },
            ],
        )
        .unwrap();
        assert_eq!(r.applied[0].ids, vec!["solid_1"]);
        // the terrain's u16 quantisation puts "y = 0" within 1/64 unit of 0
        let near = |a: f32, b: f32| (a - b).abs() < 0.02;
        assert!(
            near(p.solids[0].pos[1], 50.0),
            "solid bottom on the terrain: {}",
            p.solids[0].pos[1]
        );
        assert!(
            near(p.objects[0].pos[1], 100.0 + STAND_OFFSET),
            "on the box top: {}",
            p.objects[0].pos[1]
        );
        assert!(
            near(p.objects[1].pos[1], STAND_OFFSET),
            "on the terrain: {}",
            p.objects[1].pos[1]
        );
        assert_eq!(p.objects[2].pos[1], 999.0, "outside the terrain: kept");
        // a move with delta and ground
        apply(
            &mut p,
            &mut h,
            &[Op::Move {
                id: "player_spawn_2".into(),
                pos: None,
                delta: Some([10.0, 0.0, 0.0]),
                ground: true,
            }],
        )
        .unwrap();
        assert_eq!((p.objects[0].pos[0], p.objects[0].pos[2]), (20.0, 10.0));
        assert!(near(p.objects[0].pos[1], 100.0 + STAND_OFFSET));
        // an extent object rests with its half height above the floor
        let mut props = BTreeMap::new();
        props.insert("extent".to_string(), PropValue::Vec3([128.0, 48.0, 128.0]));
        apply(
            &mut p,
            &mut h,
            &[Op::Add {
                class: v::CLASS_BOMB_SITE.into(),
                name: None,
                pos: [200.0, 0.0, 200.0],
                rot: None,
                props,
                ground: true,
            }],
        )
        .unwrap();
        assert!(near(p.objects[3].pos[1], 48.0), "{}", p.objects[3].pos[1]);
        // a cylinder top counts, a kill volume does not
        let cyl = Solid {
            id: "solid_9".into(),
            kind: SolidKind::Cylinder {
                sides: 8,
                circle: unit_circle(8),
            },
            pos: [300.0, 0.0, 300.0],
            size: [100.0, 40.0, 100.0],
            ..Default::default()
        };
        assert_eq!(solid_top_at(&cyl, 300.0, 300.0), Some(20.0));
        assert_eq!(
            solid_top_at(&cyl, 349.0, 349.0),
            None,
            "corner of the box outside the circle"
        );
        let kill = Solid {
            brush: Some(BrushRole::KillVolume),
            ..cyl.clone()
        };
        assert_eq!(solid_top_at(&kill, 300.0, 300.0), None);
    }

    #[test]
    fn terrain_ops_are_deterministic_and_bounded() {
        let (mut p, mut h) = base_project();
        let gen_with = |seed: u64| Op::TerrainGen {
            preset: Preset::Valley,
            seed: Some(seed),
            amplitude: 100.0,
            base: 0.0,
            scale: 256.0,
            octaves: 4,
            cells_x: None,
            cells_z: None,
            cell_size: None,
        };
        let gen = gen_with(42);
        apply(&mut p, &mut h, std::slice::from_ref(&gen)).unwrap();
        let first = h.clone();
        let mut p2 = p.clone();
        let mut h2 = Heights::for_terrain(p.terrain.as_ref().unwrap(), 0);
        apply(&mut p2, &mut h2, std::slice::from_ref(&gen)).unwrap();
        assert_eq!(first, h2, "same seed = same heights");
        let mut h3 = h2.clone();
        apply(&mut p2, &mut h3, &[gen_with(43)]).unwrap();
        assert_ne!(first, h3, "another seed = other heights");
        let t = p.terrain.clone().unwrap();
        let t = &t;
        let (lo, hi) = h.raw_range().unwrap();
        assert!(
            t.raw_to_y(lo) >= -200.1 && t.raw_to_y(hi) <= 100.1,
            "{} {}",
            t.raw_to_y(lo),
            t.raw_to_y(hi)
        );
        // the valley: the centre column is lower on average than the edge columns
        let col_mean = |x: usize| {
            (0..t.rows()).map(|z| t.raw_to_y(h.get(x, z))).sum::<f32>() / t.rows() as f32
        };
        assert!(col_mean(4) < col_mean(0) && col_mean(4) < col_mean(8));
        // raise / lower / flatten / smooth
        let centre = h.sample(t, 0.0, 0.0).unwrap();
        apply(
            &mut p,
            &mut h,
            &[Op::TerrainRaise {
                x: 0.0,
                z: 0.0,
                radius: 100.0,
                amount: 50.0,
                falloff: Falloff::Flat,
            }],
        )
        .unwrap();
        assert!((h.sample(t, 0.0, 0.0).unwrap() - (centre + 50.0)).abs() < 0.1);
        apply(
            &mut p,
            &mut h,
            &[Op::TerrainLower {
                x: 0.0,
                z: 0.0,
                radius: 100.0,
                amount: 50.0,
                falloff: Falloff::Flat,
            }],
        )
        .unwrap();
        assert!((h.sample(t, 0.0, 0.0).unwrap() - centre).abs() < 0.1);
        apply(
            &mut p,
            &mut h,
            &[Op::TerrainFlatten {
                x: 0.0,
                z: 0.0,
                radius: 10000.0,
                height: 7.0,
                falloff: Falloff::Flat,
            }],
        )
        .unwrap();
        let (lo, hi) = h.raw_range().unwrap();
        assert_eq!(lo, hi);
        assert!((t.raw_to_y(lo) - 7.0).abs() < 0.02);
        // a spike smooths down
        h.set(4, 4, 65535);
        let r = apply(
            &mut p,
            &mut h,
            &[Op::TerrainSmooth {
                x: None,
                z: None,
                radius: None,
                passes: 3,
            }],
        )
        .unwrap();
        assert_eq!(r.changed, vec!["terrain.r16"]);
        assert!(h.get(4, 4) < 65535 / 2);
        // a terrain op without terrain is refused
        let mut flat = Project::default();
        let mut none = Heights::none();
        let e = apply(
            &mut flat,
            &mut none,
            &[Op::TerrainRaise {
                x: 0.0,
                z: 0.0,
                radius: 1.0,
                amount: 1.0,
                falloff: Falloff::Smooth,
            }],
        )
        .unwrap_err();
        assert_eq!(e.items[0].code, Code::TerrainMissing);
        // terrain.gen creates one
        apply(&mut flat, &mut none, &[gen]).unwrap();
        assert!(flat.terrain.is_some());
        assert_eq!(none.data.len(), 65 * 65);
    }

    #[test]
    fn noise_is_seeded_and_in_range() {
        let a = noise::Noise::new(5);
        let b = noise::Noise::new(5);
        let c = noise::Noise::new(6);
        let mut differs = false;
        let mut x = -10.0f32;
        while x < 10.0 {
            let va = a.fbm(x, x * 0.3, 5);
            assert_eq!(va, b.fbm(x, x * 0.3, 5));
            assert!((-1.0..=1.0).contains(&va));
            if va != c.fbm(x, x * 0.3, 5) {
                differs = true;
            }
            x += 0.37;
        }
        assert!(differs);
        assert_eq!(Falloff::Smooth.weight(0.0), 1.0);
        assert_eq!(Falloff::Smooth.weight(1.0), 0.0);
        assert_eq!(Falloff::Linear.weight(0.25), 0.75);
        assert_eq!(Falloff::Flat.weight(0.9), 1.0);
    }

    #[test]
    fn material_set_upserts_and_checks_paths() {
        let (mut p, mut h) = base_project();
        apply(
            &mut p,
            &mut h,
            &[Op::MaterialSet {
                id: "wall".into(),
                texture: Some(TextureRef::Own("assets/wall.png".into())),
                tile: Some(64.0),
                surface_id: Some(4),
                flags: None,
                fit: None,
            }],
        )
        .unwrap();
        assert_eq!(p.materials.len(), 2);
        assert_eq!(p.material("wall").unwrap().tile, 64.0);
        apply(
            &mut p,
            &mut h,
            &[Op::MaterialSet {
                id: "wall".into(),
                texture: None,
                tile: Some(32.0),
                surface_id: None,
                flags: None,
                fit: None,
            }],
        )
        .unwrap();
        assert_eq!(p.materials.len(), 2);
        assert_eq!(p.material("wall").unwrap().tile, 32.0);
        assert_eq!(p.material("wall").unwrap().surface_id, 4);
        let e = apply(
            &mut p,
            &mut h,
            &[Op::MaterialSet {
                id: "bad".into(),
                texture: Some(TextureRef::Own("..\\x.png".into())),
                tile: None,
                surface_id: None,
                flags: None,
                fit: None,
            }],
        )
        .unwrap_err();
        assert_eq!(e.items[0].code, Code::PathInvalid);
        let e = apply(
            &mut p,
            &mut h,
            &[Op::SolidAdd {
                kind: SolidSpec::Box,
                pos: [0.0; 3],
                rot_y_deg: 0.0,
                size: [1.0; 3],
                material: "nope".into(),
                faces: BTreeMap::new(),
                group: None,
                brush: None,
                ground: false,
                rises: None,
            }],
        )
        .unwrap_err();
        assert_eq!(e.items[0].code, Code::MaterialMissing);
    }
}
