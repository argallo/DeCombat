//! The world models a match has: the 2009 client draws and collides with a linked world model (a BSP model whose
//! name an object of the world file carries: crates, barrels, mode-specific props, sky models) only while the server
//! has created an object of that name, and only with that object's visible / solid flags. The client itself never
//! instantiates the world file's objects: its one pass over the object list looks for light classes, and a kind-2
//! create binds the new object to the world model of its name. Which models exist in a mode is therefore the
//! server's choice (the start scripts of the room's mode, DSV-37), and a destroyed breakable is the server switching
//! that object's flags off.
//!
//! [`ServerObjects`] is what a server-backed source reports every frame (the named brushes with their flag words and
//! positions, and the placed-effect objects); [`WorldModels`] applies it to the loaded world's render scene and
//! collision set. A local source (no server) reports nothing and the world keeps the file's own visibility (the
//! offline viewer).
//!
//! Doors and switches (the mover classes of `dc_sim::from_world::WorldFilter::movers`) are in the collision set like any solid
//! world model, and are drawn and collided with where their object is: a door the server moved is drawn and solid at
//! its new place (the file geometry moved rigidly by the object's position and rotation).
// SPEC: FMT_DAT85.md §6 (object list: the server engine reads it at engine+0x93fd0; the client scans it for light
// classes only, engine+0xcea90), §6.4 (object name = model name); UDP_SESSION_2009_B.md §3.3 (kind 2 `[name]`), §4.2
// item 3 (flag words), §3.5.1 (the reference stack's world set; "the client collides with what the server creates");
// RENDER_2009.md §5.1 step 6 (world models are drawn by the object queue); GAME_MESSAGES_2009_S2C.md §8.4 (type 0x64)
// DISASM: engine+0xa2190 (kind-2 create: the object is bound to the world model of its name; a missing model fails
// the create with code 0x17)
// DC-DEFAULT: CLI-230..CLI-233 (proposed, staged world-objects)

use std::collections::{BTreeMap, BTreeSet};

use crate::fx::{FxEvent, WorldFx};
use dc_botclient::objects::{kind, ObjectTable};
use dc_render::GpuScene;
use dc_sim::from_world::{coll_parts, selected_models, CollParts, ModelPose, WorldFilter};
use dc_sim::V3;
use dc_sim::{CollModel, CollisionWorld};
use dc_world::{ObjectRole, World};

/// Flag bits of a named brush: visible and solid.
// SPEC: UDP_SESSION_2009_B.md §4.2 item 3; OBSERVED: CAPTURE_rust3_RUSTSTACK_VALIDATION.md / UDP_SESSION_2009_B.md
// §3.5.1 (the reference stack's breakables and world models 0x3101, invisible ones without 0x0001; REFERENCE, accepted)
// DC-DEFAULT: CLI-230 (proposed) 0x0001 draws, 0x2000 collides
pub const FLAG_VISIBLE: u16 = 0x0001;
pub const FLAG_SOLID: u16 = 0x2000;
/// Client object type of a placed effect.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.4 (type 0x64 `[name:zstr8][value:u32][has_target:u8]`)
pub const EFFECT_TYPE: u8 = 0x64;

/// One placed-effect object of the server (a death effect of a breakable, a world effect).
#[derive(Debug, Clone, PartialEq)]
pub struct WorldEffect {
    pub obj: u16,
    /// The effect name the object carries (a ROM effect name, resolved by the effect player).
    pub name: Vec<u8>,
    pub pos: [f32; 3],
    /// The object's rotation (x, y, z, w): the create's, identity when it carried none (client-destructfx).
    // DISASM: cshell+0x26e070 (the group takes the object's rotation)
    pub rot: [f32; 4],
    /// The blob's `value`: the group's create flags (w2-fx, [`crate::fx::WORLD_FX_LOOP`] and the other bits).
    // DISASM: cshell+0x26e1ce..0x26e1d5 (the u32 after the name), cshell+0x26e25e..0x26e27c (handed as the create flags
    // to cshell+0x26cfd0, which stores them at the create record's +0x20)
    pub value: u32,
    /// The object left the server's table this frame (w2-fx): the effect player stops the object's groups
    /// ([`crate::fx::FxEvent::WorldGone`]); `name` is empty then.
    pub removed: bool,
    /// The object's generation in the client's table (`ClientObject::created`).
    pub created: u64,
}

/// What the server's object table says about the world models this frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ServerObjects {
    /// Named brushes (kind 2): lower-case name -> first flag word.
    pub brushes: BTreeMap<Vec<u8>, u16>,
    /// Placed-effect objects (kind 0 with a type-0x64 blob).
    pub effects: Vec<WorldEffect>,
    /// Lower-case names of the named brushes whose object the server moved away from its first position (a door
    /// that opened): the shot trace does not test them at the file position (a model outside the collision set; the
    /// doors are in it and move, see `poses`).
    // DC-DEFAULT: CLI-251 (proposed)
    pub moved: BTreeSet<Vec<u8>>,
    /// Named brushes with a decoded position: lower-case name -> (latest position, latest rotation when the server
    /// sent one).
    // SPEC: UDP_SESSION_2009_B.md §4.2 items 6, 7 (position, rotation of a change record)
    pub poses: BTreeMap<Vec<u8>, BrushPose>,
}

/// A named brush's latest position and, when the server sent one, its rotation (x, y, z, w).
pub type BrushPose = ([f32; 3], Option<[f32; 4]>);

/// A named brush counts as moved once its latest position lies this far from its first decoded position.
// DC-DEFAULT: CLI-251 (proposed) 0.5 units
pub const MOVED_EPS: f32 = 0.5;

/// The named brushes and placed effects of a client object table.
pub fn server_objects(t: &ObjectTable) -> ServerObjects {
    let mut s = ServerObjects::default();
    for (&obj, o) in &t.objects {
        if o.kind == kind::BRUSH {
            if let Some(n) = &o.name {
                s.brushes.insert(n.to_ascii_lowercase(), o.flags);
                // the first decoded position is where it was created (a brush create carries its position in the
                // full-precision field, not in the create's own vector)
                let first = o.samples.first().map(|x| x.0);
                let moved = match (first, o.pos) {
                    (Some(a), Some(p)) => {
                        let d = [0, 1, 2].map(|k| p[k] - a[k]);
                        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() > MOVED_EPS
                    }
                    _ => false,
                };
                if moved {
                    s.moved.insert(n.to_ascii_lowercase());
                }
                if let Some(p) = o.pos {
                    s.poses.insert(n.to_ascii_lowercase(), (p, o.rot));
                }
            }
        } else if o.blob_type == Some(EFFECT_TYPE) {
            let Some(b) = &o.blob else {
                continue;
            };
            let mut r = b.reader();
            if r.remaining_bits() < 8 || r.read_u8() != EFFECT_TYPE {
                continue;
            }
            let name = r.read_zstr8();
            // SPEC: GAME_MESSAGES_2009_S2C.md §8.4 (`[name:zstr8(256)][value:u32]...`); a blob cut before the value
            // reads 0 (plays once)
            let value = if r.remaining_bits() >= 32 {
                r.read_u32()
            } else {
                0
            };
            s.effects.push(WorldEffect {
                obj,
                name,
                pos: o.pos.unwrap_or(o.create_pos),
                rot: o.rot.unwrap_or([0.0, 0.0, 0.0, 1.0]),
                value,
                removed: false,
                created: o.created,
            });
        }
    }
    s
}

/// The linked world models of a loaded world and their current state.
#[derive(Debug, Clone, Default)]
pub struct WorldModels {
    /// Lower-case names of the linked models (BSP models an object names, sky models left out), BSP order.
    names: Vec<Vec<u8>>,
    /// Index of each in the render scene's model list and in the collision set (`None`: not built there).
    render: Vec<Option<usize>>,
    collision: Vec<Option<usize>>,
    /// The collision model of each one while it is switched off.
    stash: Vec<Option<CollModel>>,
    shown: Vec<bool>,
    solid: Vec<bool>,
    /// The last applied server state (nothing is redone while it does not change).
    applied: Option<BTreeMap<Vec<u8>, u16>>,
    /// Placed-effect objects already handed to the effect player, with their generation.
    effects_seen: BTreeMap<u16, u64>,
    /// Shot trace (CLI-250): the geometry of each linked model outside the collision set and the surface type of
    /// each of its polygons; the server's first flag word of each linked model (0 without a named brush) and its
    /// moved state; the shot-only world of the current state with the linked model of each of its models.
    shot_only: Vec<Option<CollModel>>,
    shot_only_surfaces: Vec<Vec<u16>>,
    flags: Vec<u16>,
    moved: Vec<bool>,
    shot_world: CollisionWorld,
    shot_models: Vec<usize>,
    shot_applied: Option<ShotKey>,
    /// Movers (doors, switches) of the collision set: the model origin in the file, the collision parts to build
    /// them again at a new place, and the pose they have now (`None`: the file position).
    origin: Vec<[f32; 3]>,
    mover_parts: Vec<Option<CollParts>>,
    pose: Vec<Option<ModelPose>>,
    /// The file box (min, max) of each linked model (the use key's reach is measured to it, CLI-1795).
    file_box: Vec<([f32; 3], [f32; 3])>,
    /// Collision-set indices of the models the last [`WorldModels::apply`] switched off (CLI-1369).
    left: Vec<u32>,
}

/// A mover is placed at its file position while its object lies within this distance of the model origin with no
/// turn (the server's closed door is exactly there).
// DC-DEFAULT: CLI-420 (proposed, w1-doors) 0.01 units
pub const POSE_EPS: f32 = 0.01;

/// The pose of a model whose file origin is `origin` and whose object is at `pos` turned by `rot` (`None`: the file
/// position).
pub fn pose_of(origin: [f32; 3], pos: [f32; 3], rot: Option<[f32; 4]>) -> Option<ModelPose> {
    let d = [0, 1, 2].map(|k| pos[k] - origin[k]);
    let r = rot.unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let len = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2] + r[3] * r[3]).sqrt();
    let r = if len.is_finite() && len > 1e-6 {
        r.map(|c| c / len)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    let still =
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() <= POSE_EPS && r[3].abs() >= 1.0 - 1e-7;
    let finite = d.iter().all(|c| c.is_finite());
    (!still && finite).then(|| ModelPose {
        origin: V3::from_array(origin),
        pos: V3::from_array(pos),
        rot: r,
    })
}

/// The render transform of a pose: `p -> pos + rot * (p - origin)`.
pub fn pose_affine(p: &ModelPose) -> glam::Affine3A {
    let [x, y, z, w] = p.rot;
    glam::Affine3A::from_rotation_translation(
        glam::Quat::from_xyzw(x, y, z, w),
        glam::Vec3::new(p.pos.x, p.pos.y, p.pos.z),
    ) * glam::Affine3A::from_translation(-glam::Vec3::new(p.origin.x, p.origin.y, p.origin.z))
}

/// The server state the shot-only world was built from: the named brushes' flags and the moved ones.
type ShotKey = (BTreeMap<Vec<u8>, u16>, BTreeSet<Vec<u8>>);

impl WorldModels {
    /// The linked models of `world`, matched with the collision set built with `filter` and the render scene's model
    /// names (`None` without a render scene). Every model starts as the file has it (drawn unless its own properties
    /// hide it, solid when in the collision set).
    pub fn of(world: &World<'_>, filter: &WorldFilter, scene: Option<&[Vec<u8>]>) -> WorldModels {
        let coll = selected_models(world, filter);
        let mut m = WorldModels::default();
        for (i, model) in world.models.iter().enumerate() {
            let Some(o) = world.object_named(model.name) else {
                continue;
            };
            if o.role == Some(ObjectRole::SkyWorldModel) {
                continue;
            }
            let name = model.name.to_ascii_lowercase();
            let render = scene.and_then(|s| s.iter().position(|n| n.eq_ignore_ascii_case(&name)));
            let collision = coll.iter().position(|&c| c == i);
            m.shown
                .push(dc_render::model_visibility(Some(o)) != dc_render::ModelVisibility::Hidden);
            m.solid.push(collision.is_some());
            m.names.push(name);
            m.render.push(render);
            m.collision.push(collision);
            m.stash.push(None);
            // CLI-250: a linked model outside the collision set still stops rays while its flags say so
            let (shot, surfaces) = if collision.is_none() {
                (
                    Some(dc_sim::from_world::coll_model(model)),
                    model
                        .polygons
                        .iter()
                        .map(|p| {
                            model
                                .surfaces
                                .get(p.surface as usize)
                                .map_or(0, |s| s.texflags)
                        })
                        .collect(),
                )
            } else {
                (None, Vec::new())
            };
            m.shot_only.push(shot);
            m.shot_only_surfaces.push(surfaces);
            m.flags.push(0);
            m.moved.push(false);
            // DC-DEFAULT: CLI-420 (proposed, w1-doors) movers keep their parts to be built again where they move
            m.origin.push(model.origin);
            m.mover_parts
                .push((collision.is_some() && filter.is_mover(o)).then(|| coll_parts(model)));
            m.pose.push(None);
            m.file_box.push((model.box_min, model.box_max));
        }
        m
    }

    /// The file box (min, max) of the linked model of this name (ASCII case-insensitive).
    pub fn file_box(&self, name: &[u8]) -> Option<([f32; 3], [f32; 3])> {
        let i = self
            .names
            .iter()
            .position(|x| x.eq_ignore_ascii_case(name))?;
        self.file_box.get(i).copied()
    }

    /// The pose of the linked model of this name (`None` at the file position or without such a model).
    pub fn pose(&self, name: &[u8]) -> Option<ModelPose> {
        let n = name.to_ascii_lowercase();
        let i = self.names.iter().position(|x| *x == n)?;
        self.pose[i]
    }

    /// Number of linked models.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Collision-set indices of the models the last [`WorldModels::apply`] took out of the collision set (a
    /// destroyed breakable): a body standing on one must be detached ([`dc_sim::PlayerState::detach_from_models`]).
    // DC-DEFAULT: CLI-1369 (proposed)
    pub fn left(&self) -> &[u32] {
        &self.left
    }

    /// `(drawn, solid)` of the linked model of this name (ASCII case-insensitive).
    pub fn state(&self, name: &[u8]) -> Option<(bool, bool)> {
        let n = name.to_ascii_lowercase();
        let i = self.names.iter().position(|x| *x == n)?;
        Some((self.shown[i], self.solid[i]))
    }

    /// Names of the linked models that are drawn now.
    pub fn drawn(&self) -> Vec<&[u8]> {
        self.names
            .iter()
            .zip(&self.shown)
            .filter(|(_, s)| **s)
            .map(|(n, _)| n.as_slice())
            .collect()
    }

    /// Applies the server's objects: a linked model is drawn while a named brush of its name has the visible flag,
    /// and solid while it has the solid flag; without such a brush it is neither (the 2009 client has no other
    /// source of world models). The render scene and the collision set follow where the model was built in them.
    /// Returns the placed-effect objects that appeared since the last call.
    // DC-DEFAULT: CLI-230 (proposed) the rule; CLI-231 (proposed) a switched-off model leaves the collision set by an
    // empty stand-in at its index (the surface table stays aligned)
    pub fn apply(
        &mut self,
        objs: &ServerObjects,
        collision: &mut CollisionWorld,
        mut scene: Option<&mut GpuScene>,
    ) -> Vec<WorldEffect> {
        self.left.clear();
        if self.applied.as_ref() != Some(&objs.brushes) {
            for i in 0..self.names.len() {
                let flags = objs.brushes.get(&self.names[i]).copied();
                let shown = flags.is_some_and(|f| f & FLAG_VISIBLE != 0);
                let solid = flags.is_some_and(|f| f & FLAG_SOLID != 0);
                if let (Some(r), Some(s)) = (self.render[i], scene.as_deref_mut()) {
                    s.set_model_shown(r, shown);
                }
                self.shown[i] = shown;
                if let Some(c) = self.collision[i] {
                    if c < collision.models.len() {
                        match (solid, self.stash[i].is_some()) {
                            (false, false) => {
                                let empty = CollModel::new(&[], &[], None);
                                self.stash[i] =
                                    Some(std::mem::replace(&mut collision.models[c], empty));
                                self.left.push(c as u32);
                            }
                            (true, true) => {
                                if let Some(m) = self.stash[i].take() {
                                    collision.models[c] = m;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                self.solid[i] = solid && self.collision[i].is_some();
            }
            self.applied = Some(objs.brushes.clone());
        }
        self.apply_poses(objs, collision, scene);
        self.apply_shots(objs);
        let live: BTreeMap<u16, u64> = objs.effects.iter().map(|e| (e.obj, e.created)).collect();
        // w2-fx: the placed effects whose object left (a looping fire stops with its object, CLI-517); first, so an
        // id that is another effect now stops the old group before the new one starts
        // DC-DEFAULT: CLI-1582 (proposed, playtest report "no upload station appeared": the server frees the
        // id of an effect switched off and gives it to the effect switched on in the same tick, the Spy Hunt
        // station's model -> ready box and ready -> transfer box, so the second never started) an id whose object
        // was created again (a new generation) is the old effect leaving and a new one appearing
        let mut fresh: Vec<WorldEffect> = self
            .effects_seen
            .iter()
            .filter(|(obj, g)| live.get(obj) != Some(g))
            .map(|(&obj, _)| WorldEffect {
                obj,
                name: Vec::new(),
                pos: [0.0; 3],
                rot: [0.0, 0.0, 0.0, 1.0],
                value: 0,
                removed: true,
                created: 0,
            })
            .collect();
        fresh.extend(
            objs.effects
                .iter()
                .filter(|e| self.effects_seen.get(&e.obj) != Some(&e.created))
                .cloned(),
        );
        self.effects_seen = live;
        fresh
    }
}

impl WorldModels {
    /// gpu-recover: gives a render scene built anew (a new GPU device) the state this world's models have now: each
    /// linked model shown or hidden, and each mover at its pose.
    // DC-DEFAULT: CLI-460 (amended by gpu-recover)
    pub fn reapply_scene(&self, scene: &mut GpuScene) {
        for i in 0..self.names.len() {
            let Some(r) = self.render[i] else {
                continue;
            };
            // before the server's first state the fresh scene already shows what the file shows
            if self.applied.is_some() {
                scene.set_model_shown(r, self.shown[i]);
            }
            if let Some(p) = &self.pose[i] {
                scene.set_model_transform(r, Some(pose_affine(p)));
            }
        }
    }

    /// Places the movers where the server's objects are: a mover whose object moved or turned is drawn and solid at
    /// its new place (the render scene's model transform, the collision model built again from its parts; a mover
    /// switched off keeps the moved model in its stash). Only a pose change does work.
    // SPEC: MOVEMENT_2009.md §9.6 stage 2 (world-model objects are swept as BSP models); RENDER_2009.md §5.1 step 6
    // (world models drawn by the object queue); UDP_SESSION_2009_B.md §4.2 items 6, 7 (the object's position and
    // rotation)
    // DC-DEFAULT: CLI-420 (proposed, w1-doors)
    fn apply_poses(
        &mut self,
        objs: &ServerObjects,
        collision: &mut CollisionWorld,
        mut scene: Option<&mut GpuScene>,
    ) {
        for i in 0..self.names.len() {
            let Some(parts) = &self.mover_parts[i] else {
                continue;
            };
            let target = objs
                .poses
                .get(&self.names[i])
                .and_then(|&(p, r)| pose_of(self.origin[i], p, r));
            if target == self.pose[i] {
                continue;
            }
            let model = match &target {
                Some(p) => parts.build_posed(p),
                None => parts.build(),
            };
            if let Some(stashed) = self.stash[i].as_mut() {
                *stashed = model;
            } else if let Some(c) = self.collision[i].filter(|&c| c < collision.models.len()) {
                collision.models[c] = model;
            }
            if let (Some(r), Some(s)) = (self.render[i], scene.as_deref_mut()) {
                s.set_model_transform(r, target.as_ref().map(pose_affine));
            }
            self.pose[i] = target;
        }
    }

    /// Rebuilds the shot-only world when the server's brushes or their moved state changed: a linked model in the
    /// collision set that the server made ray-hit but not solid (its collision geometry is stashed), and a linked
    /// model outside the collision set that is ray-hit or solid and has not moved.
    // DISASM: engine+0x40920 (the segment query tests objects whose flag word has 0x1000 or 0x2000)
    // DC-DEFAULT: CLI-250 (proposed), CLI-251 (proposed)
    fn apply_shots(&mut self, objs: &ServerObjects) {
        let key = (objs.brushes.clone(), objs.moved.clone());
        if self.shot_applied.as_ref() == Some(&key) {
            return;
        }
        let mut models = Vec::new();
        let mut index = Vec::new();
        for i in 0..self.names.len() {
            let flags = objs.brushes.get(&self.names[i]).copied().unwrap_or(0);
            let moved = objs.moved.contains(&self.names[i]);
            self.flags[i] = flags;
            self.moved[i] = moved;
            let geometry = if self.collision[i].is_some() {
                // in the collision set: there while solid; stashed and ray-hit stops rays only
                self.stash[i]
                    .as_ref()
                    .filter(|_| flags & crate::shots::FLAG_RAYHIT != 0)
            } else {
                self.shot_only[i]
                    .as_ref()
                    .filter(|_| flags & crate::shots::RAY_FLAGS != 0 && !moved)
            };
            if let Some(g) = geometry {
                models.push(g.clone());
                index.push(i);
            }
        }
        self.shot_world = CollisionWorld::from_parts(models, &[]);
        self.shot_models = index;
        self.shot_applied = Some(key);
    }

    /// The shot-only models of the current state (`None` when there is none): [`crate::shots::ShotWorld::extra`].
    pub fn shot_world(&self) -> Option<&CollisionWorld> {
        (!self.shot_world.models.is_empty()).then_some(&self.shot_world)
    }

    /// Name of the linked model behind model `k` of [`WorldModels::shot_world`].
    pub fn shot_model_name(&self, k: usize) -> Option<&[u8]> {
        self.shot_models.get(k).map(|&i| self.names[i].as_slice())
    }

    /// The surface type of polygon `poly` of model `k` of [`WorldModels::shot_world`]; `collision` gives the collision
    /// set's own table for stashed models (indexed like [`crate::world::LoadedWorld::surface_types`]).
    pub fn shot_surface(&self, k: usize, poly: u32, collision: &[Vec<u16>]) -> u16 {
        let Some(&i) = self.shot_models.get(k) else {
            return 0;
        };
        let table = match self.collision[i] {
            Some(c) => collision.get(c),
            None => self.shot_only_surfaces.get(i),
        };
        table
            .and_then(|t| t.get(poly as usize))
            .copied()
            .unwrap_or(0)
    }

    /// The name of the linked model at collision-set index `c`, if one is there.
    pub fn name_at_collision(&self, c: usize) -> Option<&[u8]> {
        self.collision
            .iter()
            .position(|x| *x == Some(c))
            .map(|i| self.names[i].as_slice())
    }
}

/// The effect player's entry for the placed effects that appeared or left this frame: each new one becomes one
/// [`FxEvent::World`] for the frame's effect stage ([`crate::fx::FxRunner::feed`]), which plays the named group at the
/// object's position (looping by its value, w2-fx); each one that left becomes one [`FxEvent::WorldGone`]. Returns
/// the number of effects that appeared.
// DC-DEFAULT: CLI-234 (amended by w2-fx: the value's loop bit and the removal are used, CLI-516 / CLI-517), see crate::fx
pub fn play_world_effects(effects: &[WorldEffect], events: &mut Vec<FxEvent>) -> usize {
    let mut n = 0;
    for e in effects {
        if e.removed {
            events.push(FxEvent::WorldGone(e.obj));
            continue;
        }
        n += 1;
        events.push(FxEvent::World(WorldFx {
            name: String::from_utf8_lossy(&e.name).into_owned(),
            pos: e.pos,
            rot: e.rot,
            obj: Some(e.obj),
            value: e.value,
        }));
    }
    n
}

// ---- client-hudgaps: the server's sky table (the sky view box and the sky objects of the sky pass)

/// The sky table of the server: the 48-byte block (floats 6..11 the sky view box) and the sky objects it names,
/// resolved to the world-model names of objects that exist now, in table order, with their visible bit.
// SPEC: UDP_SESSION_2009_A.md §4.8; UDP_SESSION_2009_B.md §8.5 (the block: outer and inner box of a sky world model)
// EVIDENCE: engine+0xa44f0 (DISASM: block and ids kept), engine+0x188c0 (each frame's scene: the ids resolved to
// objects, missing ones left out, table order), engine+0x1b4d50 (sky objects drawn only with the visible bit)
#[derive(Debug, Clone, PartialEq)]
pub struct SkyTable {
    pub block: [u8; 48],
    /// Lower-case model names of the table's existing objects and whether each is visible.
    pub models: Vec<(Vec<u8>, bool)>,
}

impl SkyTable {
    /// The sky view box (min, max) of the block.
    pub fn view_box(&self) -> ([f32; 3], [f32; 3]) {
        let b = dc_render::SkyBlock::from_bytes(&self.block);
        (b.view_min, b.view_max)
    }
}

/// The sky table of a session (`None` before `S2C_SKY_TABLE`).
pub fn sky_table(block: Option<&[u8; 48]>, ids: &[u16], t: &ObjectTable) -> Option<SkyTable> {
    let block = *block?;
    let models = ids
        .iter()
        .filter_map(|id| t.objects.get(id))
        .filter_map(|o| {
            let n = o.name.as_ref()?;
            Some((n.to_ascii_lowercase(), o.flags & FLAG_VISIBLE != 0))
        })
        .collect();
    Some(SkyTable { block, models })
}

/// Applies a sky table to a render scene: the view box, and every sky model drawn only while the table names a
/// visible object of its name. Returns the number of sky models left on.
pub fn apply_sky(table: &SkyTable, scene: &mut GpuScene) -> usize {
    scene.sky_view_box = Some(table.view_box());
    let names: Vec<Vec<u8>> = scene.sky_model_names().to_vec();
    let mut on = 0;
    for (i, n) in names.iter().enumerate() {
        let n = n.to_ascii_lowercase();
        let shown = table.models.iter().any(|(m, v)| *v && *m == n);
        scene.set_sky_model_shown(i, shown);
        on += shown as usize;
    }
    on
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_wire::udp::BitWriter;

    #[test]
    fn server_objects_take_named_brushes_and_effect_objects() {
        let mut t = ObjectTable::default();
        let mut brush = dc_botclient::objects::ClientObject::default();
        brush.kind = kind::BRUSH;
        brush.name = Some(b"Crate_A".to_vec());
        brush.flags = 0x3101;
        t.objects.insert(70, brush);
        let mut blob = BitWriter::new();
        blob.write_u8(EFFECT_TYPE)
            .write_zstr8(b"Burst")
            .write_u32(0)
            .write_u8(0);
        let mut fx = dc_botclient::objects::ClientObject::default();
        fx.blob_type = Some(EFFECT_TYPE);
        fx.blob = Some(blob);
        fx.pos = Some([1.0, 2.0, 3.0]);
        t.objects.insert(71, fx);
        let s = server_objects(&t);
        assert_eq!(s.brushes.get(b"crate_a".as_slice()), Some(&0x3101));
        assert_eq!(
            s.effects,
            vec![WorldEffect {
                obj: 71,
                name: b"Burst".to_vec(),
                pos: [1.0, 2.0, 3.0],
                rot: [0.0, 0.0, 0.0, 1.0],
                value: 0,
                removed: false,
                created: 0,
            }]
        );
    }

    #[test]
    fn placed_effects_carry_their_value_and_report_their_removal() {
        // w2-fx: `[0x64][name][value:u32][has_target:u8]` (GAME_MESSAGES_2009_S2C.md §8.4)
        let mut blob = BitWriter::new();
        blob.write_u8(EFFECT_TYPE)
            .write_zstr8(b"Fire")
            .write_u32(3)
            .write_u8(0);
        let mut t = ObjectTable::default();
        let mut fx = dc_botclient::objects::ClientObject::default();
        fx.blob_type = Some(EFFECT_TYPE);
        fx.blob = Some(blob);
        fx.pos = Some([1.0, 2.0, 3.0]);
        t.objects.insert(40, fx);
        let s = server_objects(&t);
        assert_eq!(s.effects.len(), 1);
        assert_eq!((s.effects[0].value, s.effects[0].removed), (3, false));
        // appeared: one World event with the object and its value
        let mut events = Vec::new();
        assert_eq!(play_world_effects(&s.effects, &mut events), 1);
        match &events[0] {
            FxEvent::World(w) => assert_eq!((w.obj, w.value), (Some(40), 3)),
            e => panic!("{e:?}"),
        }
        // removed: one WorldGone event, not counted as appeared
        let gone = WorldEffect {
            obj: 40,
            name: Vec::new(),
            pos: [0.0; 3],
            rot: [0.0, 0.0, 0.0, 1.0],
            value: 0,
            removed: true,
            created: 0,
        };
        let mut events = Vec::new();
        assert_eq!(play_world_effects(&[gone], &mut events), 0);
        assert_eq!(events, vec![FxEvent::WorldGone(40)]);
    }

    /// The user's "I knife a box while standing on it and float until I move": the server switches the breakable
    /// off, the model leaves the collision set and the idle player standing on it falls on the next ticks.
    // DISASM: engine+0x70c70; DC-DEFAULT: CLI-1369 (proposed)
    #[test]
    fn a_player_on_a_destroyed_breakable_falls_without_input() {
        use crate::look::ViewAngles;
        use crate::player::LocalPlayer;
        use crate::world::SpawnPoint;
        use dc_sim::{ConvexPoly, MovementParams, SURF_SOLID};
        let top = |h: f32, y: f32| {
            CollModel::from_polys(&[ConvexPoly::from_corners(
                vec![
                    V3::new(-h, y, -h),
                    V3::new(-h, y, h),
                    V3::new(h, y, h),
                    V3::new(h, y, -h),
                ],
                SURF_SOLID,
            )])
        };
        // made-up world: a floor (collision model 0) and a crate top 48 units up (model 1, linked to "crate_a")
        let mut collision =
            CollisionWorld::from_parts(vec![top(5000.0, 0.0), top(50.0, 48.0)], &[]);
        let mut m = WorldModels {
            names: vec![b"crate_a".to_vec()],
            render: vec![None],
            collision: vec![Some(1)],
            stash: vec![None],
            shown: vec![true],
            solid: vec![true],
            shot_only: vec![None],
            shot_only_surfaces: vec![Vec::new()],
            flags: vec![0],
            moved: vec![false],
            origin: vec![[0.0; 3]],
            mover_parts: vec![None],
            pose: vec![None],
            ..WorldModels::default()
        };
        let mut objs = ServerObjects::default();
        objs.brushes.insert(b"crate_a".to_vec(), 0x3101);
        m.apply(&objs, &mut collision, None);
        assert!(m.left().is_empty());
        let dims = V3::new(16.0, 40.0, 16.0); // made-up test box
        let sp = SpawnPoint {
            pos: V3::new(0.0, 150.0, 0.0),
            yaw: 0.0,
        };
        let mut p = LocalPlayer::spawn(MovementParams::default(), &collision, &sp, dims, 60);
        let view = ViewAngles::default();
        let idle = |p: &mut LocalPlayer, w: &CollisionWorld| {
            for _ in 0..60 {
                p.tick(w, dc_sim::Commands::default(), &view);
            }
        };
        idle(&mut p, &collision);
        assert!(
            p.state.on_ground && (p.state.pos.y - 88.0).abs() < 0.5,
            "{:?}",
            p.state
        );
        // destroyed: the server switches the flags off
        objs.brushes.insert(b"crate_a".to_vec(), 0x0100);
        m.apply(&objs, &mut collision, None);
        assert_eq!(m.left(), &[1]);
        assert!(p.detach_from_models(m.left()));
        idle(&mut p, &collision);
        assert!(
            p.state.on_ground && (p.state.pos.y - 40.0).abs() < 0.5,
            "fell to the floor: {:?}",
            p.state
        );
        // the next apply with the same state takes nothing out
        m.apply(&objs, &mut collision, None);
        assert!(m.left().is_empty());
    }
}
