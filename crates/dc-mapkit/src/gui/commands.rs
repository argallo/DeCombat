//! The command layer: every editor gesture becomes one [`Op`] batch that the editor applies through
//! [`crate::ops::apply`] at the gesture's end (one history snapshot, one save), so the file on disk, the CLI,
//! scripts and the MCP tools stay in step and share one undo. No egui types here: a gesture is plain data and
//! the mapping is unit-tested.
// DC-DEFAULT: MAP-61 (the GUI writes map.json only through ops; one batch per gesture)

use super::camera::V;
use crate::ops::{Falloff, Op, SolidSpec};
use crate::project::{BrushRole, Project, PropValue, TextureRef};
use crate::template::ZONE_EXTENT;
use crate::vocab_names as v;
use dc_world::vocab::{class_named, PropSpec};
use dc_world::PropKind;
use std::collections::BTreeMap;

/// A terrain brush.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushTool {
    Raise,
    Lower,
    Smooth,
    Flatten,
}

impl BrushTool {
    pub const ALL: [BrushTool; 4] = [
        BrushTool::Raise,
        BrushTool::Lower,
        BrushTool::Smooth,
        BrushTool::Flatten,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BrushTool::Raise => "raise",
            BrushTool::Lower => "lower",
            BrushTool::Smooth => "smooth",
            BrushTool::Flatten => "flatten",
        }
    }
}

/// What a texture is assigned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignTarget {
    /// The whole solid (every face without its own material).
    Solid { id: String },
    /// One face of a solid.
    Face { id: String, face: String },
    /// The terrain's paint layer `layer` (0 = the base).
    Terrain { layer: usize },
}

/// One gesture of the editor.
#[derive(Debug, Clone, PartialEq)]
pub enum Gesture {
    /// Place a point object of `class` on the floor at `at` with the class's default properties.
    Place {
        class: String,
        at: V,
    },
    /// Place a solid standing on the floor at `at`.
    PlaceSolid {
        kind: SolidSpec,
        at: V,
        size: V,
        material: String,
        brush: Option<BrushRole>,
    },
    /// Drag end of a point object.
    MoveObject {
        id: String,
        to: V,
        ground: bool,
    },
    /// Drag end of a solid.
    MoveSolid {
        id: String,
        to: V,
    },
    /// Delete key.
    Delete {
        id: String,
    },
    Rename {
        id: String,
        name: String,
    },
    /// A typed property, `pos` or `rot` of an object.
    SetProp {
        id: String,
        prop: String,
        value: PropValue,
    },
    /// Inspector fields of a solid (`None` = unchanged).
    SolidEdit {
        id: String,
        pos: Option<V>,
        rot_y_deg: Option<f32>,
        size: Option<V>,
        material: Option<String>,
        brush: Option<Option<BrushRole>>,
        group: Option<Option<String>>,
    },
    /// A brush stroke: one dab per `dabs` entry.
    Brush {
        tool: BrushTool,
        dabs: Vec<[f32; 2]>,
        radius: f32,
        strength: f32,
        /// The target height of a flatten stroke (the floor under the first dab).
        flatten_height: f32,
    },
    /// Assign a texture (an own PNG or a ROM path) to a target: a material for it is found or created.
    PutTexture {
        texture: TextureRef,
        target: AssignTarget,
    },
    /// Replace the terrain heights with a preset.
    TerrainGen {
        preset: crate::ops::Preset,
        amplitude: f32,
        base: f32,
        scale: f32,
        octaves: u32,
        seed: Option<u64>,
    },
    /// One or more world-settings fields (the op itself; only the given fields change).
    Settings(Box<Op>),
    /// The mod metadata op.
    Meta(Box<Op>),
}

/// Parse a vocabulary default text into a literal value of its kind: `true` / `false`, a number, three numbers
/// (`0 0 0`, `255 255 255`), or a quoted text (`"" (every mode)` -> the empty text). A descriptive default
/// (`any team`) is not a literal and answers `None` (absent = unset).
pub fn parse_default(kind: PropKind, text: &str) -> Option<PropValue> {
    let text = text.trim();
    let nums = || -> Option<Vec<f64>> {
        text.split([' ', ','])
            .filter(|s| !s.is_empty())
            .map(|s| s.parse::<f64>().ok())
            .collect()
    };
    match kind {
        PropKind::Bool => match text {
            "true" => Some(PropValue::Bool(true)),
            "false" => Some(PropValue::Bool(false)),
            _ => None,
        },
        PropKind::Real | PropKind::Int => text
            .split_whitespace()
            .next()
            .and_then(|t| t.parse::<f64>().ok())
            .map(PropValue::Num),
        PropKind::Vec3 | PropKind::Euler => {
            let n = nums()?;
            (n.len() == 3).then(|| PropValue::Vec3([n[0] as f32, n[1] as f32, n[2] as f32]))
        }
        PropKind::Rgb => {
            let n = nums()?;
            (n.len() == 3).then(|| PropValue::Rgb {
                rgb: [n[0] as u8, n[1] as u8, n[2] as u8],
            })
        }
        PropKind::Text => {
            let rest = text.strip_prefix('"')?;
            let end = rest.find('"')?;
            Some(PropValue::Text(rest[..end].to_string()))
        }
        PropKind::Bits | PropKind::Other(_) => None,
    }
}

/// The value a property gets when an object of `class` is placed: the vocabulary default when it parses, else a
/// sensible editor default of the property's kind (zone extents for `extent`, a visible radius for a light).
pub fn default_value(class: &str, spec: &PropSpec) -> PropValue {
    if let Some(v) = spec.default.and_then(|d| parse_default(spec.kind, d)) {
        return v;
    }
    match spec.name {
        v::PROP_EXTENT => PropValue::Vec3(ZONE_EXTENT),
        v::PROP_NUMBER => PropValue::Num(0.0),
        v::PROP_TEAM => PropValue::Text(
            if class == v::CLASS_PLAYER_SPAWN {
                ""
            } else {
                v::TEAM_1
            }
            .into(),
        ),
        v::PROP_RADIUS => PropValue::Num(256.0),
        v::PROP_COLOR => PropValue::Rgb {
            rgb: [255, 255, 255],
        },
        "far_z" => PropValue::Num(12_000.0),
        "lights_objects" => PropValue::Bool(true),
        _ => match spec.kind {
            PropKind::Bool => PropValue::Bool(false),
            PropKind::Real | PropKind::Int => PropValue::Num(0.0),
            PropKind::Vec3 | PropKind::Euler => PropValue::Vec3([0.0; 3]),
            PropKind::Rgb => PropValue::Rgb {
                rgb: [255, 255, 255],
            },
            _ => PropValue::Text(String::new()),
        },
    }
}

/// The properties a newly placed object of `class` carries: every required property of the vocabulary, plus
/// the optional ones whose default is a literal value (`modes` = `""` every mode, a spawn's `team` = `""` any,
/// `bright` = 1), so the file shows what the inspector offers; an optional property with a descriptive default
/// (`defuse_team`: any team) stays absent.
pub fn class_defaults(class: &str) -> BTreeMap<String, PropValue> {
    let mut out = BTreeMap::new();
    if let Some(spec) = class_named(class) {
        for p in spec.props {
            let literal = p.default.and_then(|d| parse_default(p.kind, d));
            if p.required || literal.is_some() {
                out.insert(p.name.to_string(), default_value(class, p));
            }
        }
    }
    out
}

/// A material id for a texture path: `tex_<stem>` with a-z 0-9 _ only, made unique against the project.
pub fn material_id_for(p: &Project, texture: &TextureRef) -> String {
    let path = match texture {
        TextureRef::Own(s) | TextureRef::Rom(s) => s.as_str(),
    };
    let stem = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .rsplit_once('.')
        .map_or(path, |(s, _)| s);
    let mut base: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if base.is_empty() {
        base = "tex".into();
    }
    let base = format!("tex_{base}");
    if p.material(&base).is_none() {
        return base;
    }
    (2..)
        .map(|n| format!("{base}_{n}"))
        .find(|id| p.material(id).is_none())
        .expect("an unused id")
}

/// The material of the project that already carries `texture`, if any.
pub fn material_with(p: &Project, texture: &TextureRef) -> Option<String> {
    p.materials
        .iter()
        .find(|m| m.texture == *texture)
        .map(|m| m.id.clone())
}

/// The op batch of a gesture against the project as it is before the gesture.
pub fn ops_of(g: &Gesture, p: &Project) -> Vec<Op> {
    match g {
        Gesture::Place { class, at } => vec![Op::Add {
            class: class.clone(),
            name: None,
            pos: *at,
            rot: None,
            props: class_defaults(class),
            ground: true,
        }],
        Gesture::PlaceSolid {
            kind,
            at,
            size,
            material,
            brush,
        } => vec![Op::SolidAdd {
            kind: kind.clone(),
            pos: *at,
            rot_y_deg: 0.0,
            size: *size,
            material: material.clone(),
            faces: BTreeMap::new(),
            group: None,
            brush: *brush,
            ground: true,
            rises: None,
        }],
        Gesture::MoveObject { id, to, ground } => vec![Op::Move {
            id: id.clone(),
            pos: Some(*to),
            delta: None,
            ground: *ground,
        }],
        Gesture::MoveSolid { id, to } => vec![Op::SolidSet {
            id: id.clone(),
            pos: Some(*to),
            rot_y_deg: None,
            size: None,
            material: None,
            faces: None,
            group: None,
            brush: None,
            rises: None,
        }],
        Gesture::Delete { id } => vec![Op::Remove { id: id.clone() }],
        Gesture::Rename { id, name } => vec![Op::Rename {
            id: id.clone(),
            name: name.clone(),
        }],
        Gesture::SetProp { id, prop, value } => vec![Op::Set {
            id: id.clone(),
            prop: prop.clone(),
            value: value.clone(),
        }],
        Gesture::SolidEdit {
            id,
            pos,
            rot_y_deg,
            size,
            material,
            brush,
            group,
        } => {
            let s = p.solid(id);
            vec![Op::SolidSet {
                id: id.clone(),
                pos: *pos,
                rot_y_deg: *rot_y_deg,
                size: *size,
                material: material.clone(),
                faces: None,
                // `solid.set` takes a value, not "unchanged", for these two: send the current one when the
                // gesture leaves them alone
                group: match group {
                    Some(g) => g.clone(),
                    None => s.and_then(|s| s.group.clone()),
                },
                brush: match brush {
                    Some(b) => *b,
                    None => s.and_then(|s| s.brush),
                },
                rises: None,
            }]
        }
        Gesture::Brush {
            tool,
            dabs,
            radius,
            strength,
            flatten_height,
        } => dabs
            .iter()
            .map(|[x, z]| match tool {
                BrushTool::Raise => Op::TerrainRaise {
                    x: *x,
                    z: *z,
                    radius: *radius,
                    amount: *strength,
                    falloff: Falloff::default(),
                },
                BrushTool::Lower => Op::TerrainLower {
                    x: *x,
                    z: *z,
                    radius: *radius,
                    amount: *strength,
                    falloff: Falloff::default(),
                },
                BrushTool::Smooth => Op::TerrainSmooth {
                    x: Some(*x),
                    z: Some(*z),
                    radius: Some(*radius),
                    passes: (*strength / 8.0).round().clamp(1.0, 8.0) as u32,
                },
                BrushTool::Flatten => Op::TerrainFlatten {
                    x: *x,
                    z: *z,
                    radius: *radius,
                    height: *flatten_height,
                    falloff: Falloff::default(),
                },
            })
            .collect(),
        Gesture::PutTexture { texture, target } => {
            let mut ops = Vec::new();
            let material = match target {
                AssignTarget::Terrain { layer } => {
                    // the terrain layer's own material takes the texture (no op changes a layer's material id)
                    let id = p
                        .terrain
                        .as_ref()
                        .and_then(|t| t.layers.get(*layer))
                        .map(|l| l.material.clone())
                        .unwrap_or_else(|| "ground".to_string());
                    ops.push(Op::MaterialSet {
                        id: id.clone(),
                        texture: Some(texture.clone()),
                        tile: None,
                        surface_id: None,
                        flags: None,
                        fit: None,
                    });
                    return ops;
                }
                _ => match material_with(p, texture) {
                    Some(id) => id,
                    None => {
                        let id = material_id_for(p, texture);
                        ops.push(Op::MaterialSet {
                            id: id.clone(),
                            texture: Some(texture.clone()),
                            tile: Some(128.0),
                            surface_id: Some(0),
                            flags: None,
                            fit: None,
                        });
                        id
                    }
                },
            };
            match target {
                AssignTarget::Solid { id } => ops.push(Op::SolidSet {
                    id: id.clone(),
                    pos: None,
                    rot_y_deg: None,
                    size: None,
                    material: Some(material),
                    faces: None,
                    group: p.solid(id).and_then(|s| s.group.clone()),
                    brush: p.solid(id).and_then(|s| s.brush),
                    rises: None,
                }),
                AssignTarget::Face { id, face } => {
                    let mut faces = p.solid(id).map(|s| s.faces.clone()).unwrap_or_default();
                    faces.insert(face.clone(), material);
                    ops.push(Op::SolidSet {
                        id: id.clone(),
                        pos: None,
                        rot_y_deg: None,
                        size: None,
                        material: None,
                        faces: Some(faces),
                        group: p.solid(id).and_then(|s| s.group.clone()),
                        brush: p.solid(id).and_then(|s| s.brush),
                        rises: None,
                    });
                }
                AssignTarget::Terrain { .. } => {}
            }
            ops
        }
        Gesture::TerrainGen {
            preset,
            amplitude,
            base,
            scale,
            octaves,
            seed,
        } => vec![Op::TerrainGen {
            preset: *preset,
            seed: *seed,
            amplitude: *amplitude,
            base: *base,
            scale: *scale,
            octaves: *octaves,
            cells_x: None,
            cells_z: None,
            cell_size: None,
        }],
        Gesture::Settings(op) | Gesture::Meta(op) => vec![(**op).clone()],
    }
}

/// A short label for the history entry a gesture takes.
pub fn label_of(g: &Gesture) -> String {
    match g {
        Gesture::Place { class, .. } => format!("gui place {class}"),
        Gesture::PlaceSolid { .. } => "gui place solid".into(),
        Gesture::MoveObject { id, .. } | Gesture::MoveSolid { id, .. } => format!("gui move {id}"),
        Gesture::Delete { id } => format!("gui delete {id}"),
        Gesture::Rename { id, .. } => format!("gui rename {id}"),
        Gesture::SetProp { id, prop, .. } => format!("gui set {id}.{prop}"),
        Gesture::SolidEdit { id, .. } => format!("gui edit {id}"),
        Gesture::Brush { tool, dabs, .. } => format!("gui terrain {} x{}", tool.name(), dabs.len()),
        Gesture::PutTexture { .. } => "gui texture".into(),
        Gesture::TerrainGen { .. } => "gui terrain gen".into(),
        Gesture::Settings(_) => "gui world settings".into(),
        Gesture::Meta(_) => "gui metadata".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::apply;
    use crate::template::{make, Template};

    #[test]
    fn placing_a_class_adds_it_grounded_with_the_vocabulary_defaults() {
        let made = make(Template::Flat, "cmd", "", 1);
        let g = Gesture::Place {
            class: v::CLASS_BOMB_SITE.into(),
            at: [100.0, 999.0, 200.0],
        };
        let ops = ops_of(&g, &made.project);
        assert_eq!(ops.len(), 1);
        let Op::Add {
            class,
            pos,
            props,
            ground,
            ..
        } = &ops[0]
        else {
            panic!("{ops:?}")
        };
        assert_eq!(class, v::CLASS_BOMB_SITE);
        assert_eq!(*pos, [100.0, 999.0, 200.0]);
        assert!(ground);
        assert_eq!(
            props.get(v::PROP_EXTENT),
            Some(&PropValue::Vec3(ZONE_EXTENT))
        );
        assert_eq!(props.get(v::PROP_NUMBER), Some(&PropValue::Num(0.0)));
        assert!(props.contains_key(v::PROP_MODES));
        assert!(
            !props.contains_key(v::PROP_DEFUSE_TEAM),
            "optional props stay out"
        );
        // every point class places and the result validates
        let (mut p, mut h) = (made.project.clone(), made.heights.clone());
        for class in v::POINT_CLASSES {
            let ops = ops_of(
                &Gesture::Place {
                    class: class.to_string(),
                    at: [0.0, 0.0, 0.0],
                },
                &p,
            );
            let a = apply(&mut p, &mut h, &ops).unwrap_or_else(|e| panic!("{class}: {e:?}"));
            assert_eq!(a.applied[0].ids.len(), 1);
        }
        let spawn = p
            .objects
            .iter()
            .rev()
            .find(|o| o.class == v::CLASS_PLAYER_SPAWN)
            .unwrap();
        assert_eq!(
            spawn.props.get(v::PROP_TEAM),
            Some(&PropValue::Text(String::new()))
        );
        assert!(
            (spawn.pos[1] - crate::project::STAND_OFFSET).abs() < 1.0,
            "grounded"
        );
        let light = p
            .objects
            .iter()
            .find(|o| o.class == v::CLASS_POINT_LIGHT)
            .unwrap();
        assert_eq!(
            light.props.get(v::PROP_RADIUS),
            Some(&PropValue::Num(256.0))
        );
        assert_eq!(
            light.props.get("lights_objects"),
            Some(&PropValue::Bool(true))
        );
        // default texts: literals parse, descriptions do not
        assert_eq!(
            parse_default(PropKind::Text, "\"\" (every mode)"),
            Some(PropValue::Text(String::new()))
        );
        assert_eq!(parse_default(PropKind::Text, "any team"), None);
        assert_eq!(
            parse_default(PropKind::Bool, "true"),
            Some(PropValue::Bool(true))
        );
        assert_eq!(
            parse_default(PropKind::Real, "1"),
            Some(PropValue::Num(1.0))
        );
        assert_eq!(
            parse_default(PropKind::Vec3, "0 0 0"),
            Some(PropValue::Vec3([0.0; 3]))
        );
        assert_eq!(
            parse_default(PropKind::Rgb, "255 255 255"),
            Some(PropValue::Rgb {
                rgb: [255, 255, 255]
            })
        );
        assert_eq!(parse_default(PropKind::Rgb, "white"), None);
    }

    #[test]
    fn drags_deletes_and_brush_strokes_map_to_their_ops() {
        let made = make(Template::Arena, "cmd", "", 1);
        let p = &made.project;
        let obj = &p.objects[0];
        let ops = ops_of(
            &Gesture::MoveObject {
                id: obj.id.clone(),
                to: [1.0, 2.0, 3.0],
                ground: true,
            },
            p,
        );
        assert_eq!(
            ops,
            vec![Op::Move {
                id: obj.id.clone(),
                pos: Some([1.0, 2.0, 3.0]),
                delta: None,
                ground: true
            }]
        );
        let solid = p.solids.iter().find(|s| s.brush.is_none()).unwrap();
        let ops = ops_of(
            &Gesture::MoveSolid {
                id: solid.id.clone(),
                to: [5.0, 6.0, 7.0],
            },
            p,
        );
        assert!(matches!(
            &ops[0],
            Op::SolidSet {
                pos: Some([5.0, 6.0, 7.0]),
                brush: None,
                ..
            }
        ));
        assert_eq!(
            ops_of(&Gesture::Delete { id: "x_1".into() }, p),
            vec![Op::Remove { id: "x_1".into() }]
        );
        let stroke = Gesture::Brush {
            tool: BrushTool::Raise,
            dabs: vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]],
            radius: 300.0,
            strength: 24.0,
            flatten_height: 0.0,
        };
        let ops = ops_of(&stroke, p);
        assert_eq!(ops.len(), 3);
        assert!(ops.iter().all(|o| matches!(o, Op::TerrainRaise { radius, amount, .. } if *radius == 300.0 && *amount == 24.0)));
        let (mut q, mut h) = (p.clone(), made.heights.clone());
        let a = apply(&mut q, &mut h, &ops).unwrap();
        assert_eq!(a.changed, vec!["terrain.r16".to_string()]);
        assert!(h.sample(q.terrain.as_ref().unwrap(), 10.0, 0.0).unwrap() > 20.0);
        let flat = Gesture::Brush {
            tool: BrushTool::Flatten,
            dabs: vec![[0.0, 0.0]],
            radius: 300.0,
            strength: 1.0,
            flatten_height: 64.0,
        };
        assert!(matches!(
            ops_of(&flat, p)[0],
            Op::TerrainFlatten { height: 64.0, .. }
        ));
        let smooth = Gesture::Brush {
            tool: BrushTool::Smooth,
            dabs: vec![[0.0, 0.0]],
            radius: 300.0,
            strength: 100.0,
            flatten_height: 0.0,
        };
        assert!(matches!(
            ops_of(&smooth, p)[0],
            Op::TerrainSmooth { passes: 8, .. }
        ));
        // a solid edit keeps the brush role and group the gesture leaves alone
        let brush = p.solids.iter().find(|s| s.brush.is_some()).unwrap();
        let ops = ops_of(
            &Gesture::SolidEdit {
                id: brush.id.clone(),
                pos: None,
                rot_y_deg: Some(45.0),
                size: None,
                material: None,
                brush: None,
                group: None,
            },
            p,
        );
        let Op::SolidSet {
            rot_y_deg,
            brush: role,
            group,
            ..
        } = &ops[0]
        else {
            panic!("{ops:?}")
        };
        assert_eq!(*rot_y_deg, Some(45.0));
        assert_eq!(*role, brush.brush);
        assert_eq!(*group, brush.group);
    }

    #[test]
    fn assigning_a_texture_finds_or_creates_the_material_and_sets_the_target() {
        let made = make(Template::Arena, "cmd", "", 1);
        let (mut p, mut h) = (made.project.clone(), made.heights.clone());
        let solid = p.solids.iter().find(|s| s.brush.is_none()).unwrap().clone();
        // a texture no material carries: material.set + solid.set
        let tex = TextureRef::Own("assets/crate top.png".into());
        let ops = ops_of(
            &Gesture::PutTexture {
                texture: tex.clone(),
                target: AssignTarget::Solid {
                    id: solid.id.clone(),
                },
            },
            &p,
        );
        assert_eq!(ops.len(), 2);
        let Op::MaterialSet { id, texture, .. } = &ops[0] else {
            panic!("{ops:?}")
        };
        assert_eq!(id, "tex_crate_top");
        assert_eq!(texture.as_ref(), Some(&tex));
        assert!(matches!(&ops[1], Op::SolidSet { material: Some(m), .. } if m == "tex_crate_top"));
        apply(&mut p, &mut h, &ops).unwrap();
        assert_eq!(p.solid(&solid.id).unwrap().material, "tex_crate_top");
        // the same texture again on a face: the material is reused, only the face changes
        let ops = ops_of(
            &Gesture::PutTexture {
                texture: tex.clone(),
                target: AssignTarget::Face {
                    id: solid.id.clone(),
                    face: "north".into(),
                },
            },
            &p,
        );
        assert_eq!(ops.len(), 1);
        assert!(
            matches!(&ops[0], Op::SolidSet { faces: Some(f), .. } if f.get("north").map(String::as_str) == Some("tex_crate_top"))
        );
        apply(&mut p, &mut h, &ops).unwrap();
        // the terrain layer takes the texture on its own material
        let rom = TextureRef::Rom("some/rom/path.dtx".into());
        let ops = ops_of(
            &Gesture::PutTexture {
                texture: rom.clone(),
                target: AssignTarget::Terrain { layer: 0 },
            },
            &p,
        );
        let layer = p.terrain.as_ref().unwrap().layers[0].material.clone();
        assert_eq!(ops.len(), 1);
        assert!(
            matches!(&ops[0], Op::MaterialSet { id, texture: Some(t), .. } if *id == layer && *t == rom)
        );
        // a second new material from a clashing stem gets a numbered id
        p.materials.push(crate::project::Material {
            id: "tex_rock".into(),
            ..Default::default()
        });
        assert_eq!(
            material_id_for(&p, &TextureRef::Own("assets/Rock.PNG".into())),
            "tex_rock_2"
        );
        assert!(label_of(&Gesture::Delete { id: "a_1".into() }).contains("a_1"));
    }

    #[test]
    fn settings_and_metadata_gestures_apply_through_their_ops() {
        let made = make(Template::Flat, "cmd", "", 1);
        let (mut p, mut h) = (made.project.clone(), made.heights.clone());
        let g = Gesture::Settings(Box::new(Op::SettingsSet {
            bake_shadow: None,
            bake_lightmap: None,
            fog_enable: Some(false),
            fog_near: None,
            fog_far: None,
            fog_color: None,
            far_z: Some(9000.0),
            sun_dir: None,
            sun_color: None,
            sun_bright: Some(0.5),
            ambient: Some([1, 2, 3]),
            sky_size: None,
            sky_material: None,
        }));
        let ops = ops_of(&g, &p);
        let a = apply(&mut p, &mut h, &ops).unwrap();
        assert_eq!(a.changed, vec!["map.json".to_string()]);
        assert!(!p.settings.fog_enable);
        assert_eq!(p.settings.far_z, 9000.0);
        assert_eq!(p.settings.ambient, [1, 2, 3]);
        assert_eq!(p.settings.sun_bright, 0.5);
        let g = Gesture::Meta(Box::new(Op::MetaSet {
            name: Some("Renamed".into()),
            version: None,
            author: Some("me".into()),
            description: None,
            tags: None,
            homepage: Some(String::new()),
            modes: Some(vec!["tdm".into(), "BOMB".into()]),
            goals: None,
            round_seconds: Some(0),
            players: None,
        }));
        let ops = ops_of(&g, &p);
        apply(&mut p, &mut h, &ops).unwrap();
        assert_eq!(p.name, "Renamed");
        assert_eq!(p.mod_meta.author, "me");
        assert_eq!(
            p.mod_meta.modes,
            vec!["TDM".to_string(), "BOMB".to_string()]
        );
        assert_eq!(p.mod_meta.round_seconds, None);
        assert_eq!(p.mod_meta.homepage, None);
        // refusals: an unknown mode, an empty name, a bad far plane, a missing sky material
        let bad = Op::MetaSet {
            name: None,
            version: None,
            author: None,
            description: None,
            tags: None,
            homepage: None,
            modes: Some(vec!["RACE".into()]),
            goals: None,
            round_seconds: None,
            players: None,
        };
        let e = apply(&mut p, &mut h, &[bad]).unwrap_err();
        assert_eq!(e.items[0].code, crate::diag::Code::ModeUnknown);
        assert!(e.has_errors());
        let bad = Op::SettingsSet {
            bake_shadow: None,
            bake_lightmap: None,
            fog_enable: None,
            fog_near: None,
            fog_far: None,
            fog_color: None,
            far_z: Some(-1.0),
            sun_dir: None,
            sun_color: None,
            sun_bright: None,
            ambient: None,
            sky_size: None,
            sky_material: None,
        };
        assert!(apply(&mut p, &mut h, &[bad]).is_err());
        let bad = Op::SettingsSet {
            bake_shadow: None,
            bake_lightmap: None,
            fog_enable: None,
            fog_near: None,
            fog_far: None,
            fog_color: None,
            far_z: None,
            sun_dir: None,
            sun_color: None,
            sun_bright: None,
            ambient: None,
            sky_size: None,
            sky_material: Some("nope".into()),
        };
        assert_eq!(
            apply(&mut p, &mut h, &[bad]).unwrap_err().items[0].code,
            crate::diag::Code::MaterialMissing
        );
        // the two ops round-trip through JSON with their tag
        for op in crate::ops::example_ops().iter().rev().take(2) {
            let text = serde_json::to_string(op).unwrap();
            assert!(
                text.contains("\"op\":\"settings.set\"") || text.contains("\"op\":\"meta.set\"")
            );
            assert_eq!(&serde_json::from_str::<Op>(&text).unwrap(), op);
        }
    }
}
