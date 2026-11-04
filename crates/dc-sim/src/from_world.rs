//! Collision world from a parsed version-85 world file (`dc_world::World`).
// SPEC: FMT_DAT85.md §5.2 (BSP body), §6.2 / §6.4 (roles, object-to-model link), §9.1 (blocker polygons)

extern crate alloc;
use alloc::vec::Vec;

use dc_world::{BspModel, Key, NodeRef, Object, ObjectRole, World};

use crate::collide::{Child, CollModel, CollisionWorld, ConvexPoly, SURF_SOLID};
use crate::player::Container;
use crate::vec3::V3;
use crate::volume::{ContainerVolumes, ConvexVolume};

/// The file's first child field of a BSP node (dc-world `BspNode::front`) is the child on the NEGATIVE side
/// of the node polygon's plane. OBSERVED over the 21 ROM worlds: of the child-subtree root polygons that lie
/// strictly on one side of their parent's plane, 1,218,026 are placed this way and 8,630 the other way
/// (MOVEMENT_2009.md §9.8 open point / §16.1 item 5; staged as a spec addendum).
// OBSERVED: dc-sim convention probe over CA_ROM worlds 2026-10-24 (counts only; report
// reports/sim-movement.md)
pub const FILE_FIRST_CHILD_IS_NEGATIVE: bool = true;

/// Which world models take part in collision.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldFilter {
    /// Models no object links to by name. OBSERVED: exactly one per ROM map, 21 in all, each the map's only
    /// model without the dual flag (the level geometry).
    pub level: bool,
    /// Brush roles that are solid.
    pub solid_roles: Vec<ObjectRole>,
    /// Models linked to objects of classes without a role label (water and others; doors and switches are
    /// [`WorldFilter::movers`]).
    pub unlabelled: bool,
    /// Classes (`dc_world::Key`: ROM hash and length, or a DeCombat name) whose linked models are movers: solid
    /// world models that move with their object. Default [`MOVER_CLASSES`] (sliding doors, rotating doors, rotating
    /// switches); empty = none.
    pub movers: Vec<Key>,
    /// The blocker polygon soup (player clips).
    pub blockers: bool,
}

impl Default for WorldFilter {
    fn default() -> Self {
        WorldFilter {
            level: true,
            solid_roles: alloc::vec![ObjectRole::PlainWorldModel, ObjectRole::Breakable],
            unlabelled: false,
            movers: MOVER_CLASSES.to_vec(),
            blockers: true,
        }
    }
}

/// Classes of the world models that move with their object: sliding door, rotating door, rotating switch (the
/// `dc_world::names` keys; ROM-only, no DeCombat name: scope cut D12 of docs/MAPKIT_PLAN.md).
// OBSERVED: class names of the ROM world files (hashes only; 2026-10-26 scratch survey over dc-world: 23 sliding
// doors on 6 worlds, 42 rotating doors on 6 worlds, 2 rotating switches on 1 world, every one linked to a model and
// with its solid property set); the same keys as dc-dserver `world::CLASS_SLIDING_DOOR` / `CLASS_ROTATING_DOOR` /
// `CLASS_SWITCH`
// DC-DEFAULT: MAP-25 (the keys of `dc_world::names`)
pub const MOVER_CLASSES: [Key; 3] = dc_world::names::MOVER_CLASSES;

impl WorldFilter {
    /// Whether `o` is of one of the filter's mover classes (`Key::matches`: the ROM hash or a DeCombat name).
    pub fn is_mover(&self, o: &Object<'_>) -> bool {
        self.movers.iter().any(|k| k.matches(o.class_name))
    }
}

fn link(r: NodeRef) -> Child {
    match r {
        NodeRef::Node(i) => Some(i),
        NodeRef::TermA | NodeRef::TermB => None,
    }
}

/// One BSP model as a collision model (polygons, planes, surface flags, nodes).
// SPEC: FMT_DAT85.md §5.2 (plane, surface, polygon, node records)
pub fn coll_model(m: &BspModel<'_>) -> CollModel {
    coll_parts(m).build()
}

/// Where a world model is: every point `p` of its file geometry lies at `pos + rot * (p - origin)`, `origin` being
/// the model's origin in the file and `pos` / `rot` its object's position and rotation (x, y, z, w; the geometry
/// is stored as created, with no rotation).
// SPEC: MOVEMENT_2009.md §9.6 stage 2 (world-model objects take the BSP sweep), UDP_SESSION_2009_B.md §3.5.1 ("the
// client collides with what the server creates"); the brush create carries the model origin and no rotation
// (DC_DEFAULTS DSV-37 brush flag rule, REFERENCE)
// DC-DEFAULT: CLI-420 (proposed, w1-doors) the moved model is the file model moved rigidly by its object
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPose {
    pub origin: V3,
    pub pos: V3,
    pub rot: [f32; 4],
}

impl ModelPose {
    /// `v` turned by the rotation (unit quaternion; `v + 2w (q x v) + 2 q x (q x v)`).
    pub fn turn(&self, v: V3) -> V3 {
        let [x, y, z, w] = self.rot;
        let q = V3::new(x, y, z);
        let t = q.cross(v) * 2.0;
        v + t * w + q.cross(t)
    }

    /// Point `p` of the file geometry where the pose puts it.
    pub fn point(&self, p: V3) -> V3 {
        self.pos + self.turn(p - self.origin)
    }
}

/// The parts a collision model is built from: polygons and the file's BSP nodes. Kept for the models that move, to
/// build them again at each new pose.
#[derive(Debug, Clone, PartialEq)]
pub struct CollParts {
    faces: Vec<ConvexPoly>,
    nodes: Vec<(u32, Child, Child)>,
    root: Child,
}

impl CollParts {
    /// The collision model at the file position.
    pub fn build(&self) -> CollModel {
        CollModel::new(&self.faces, &self.nodes, self.root)
    }

    /// The collision model moved by `pose`: every corner moved, every plane turned (`n' = rot n`, `d' = d + n'.pos -
    /// n.origin`), the same nodes (a rigid move keeps the BSP's sides).
    pub fn build_posed(&self, pose: &ModelPose) -> CollModel {
        let faces: Vec<ConvexPoly> = self
            .faces
            .iter()
            .map(|f| {
                let normal = pose.turn(f.normal);
                ConvexPoly {
                    normal,
                    dist: f.dist + normal.dot(pose.pos) - f.normal.dot(pose.origin),
                    verts: f.verts.iter().map(|&v| pose.point(v)).collect(),
                    flags: f.flags,
                }
            })
            .collect();
        CollModel::new(&faces, &self.nodes, self.root)
    }
}

/// The collision parts of one BSP model.
// SPEC: FMT_DAT85.md §5.2 (plane, surface, polygon, node records)
pub fn coll_parts(m: &BspModel<'_>) -> CollParts {
    let faces: Vec<ConvexPoly> = m
        .polygons
        .iter()
        .map(|p| {
            let pl = m.planes[p.plane as usize];
            ConvexPoly {
                normal: V3::from_array(pl.normal),
                dist: pl.dist,
                verts: m
                    .polygon_points(p)
                    .iter()
                    .map(|&i| V3::from_array(m.points[i as usize]))
                    .collect(),
                flags: m.surfaces[p.surface as usize].flags,
            }
        })
        .collect();
    let nodes: Vec<(u32, Child, Child)> = m
        .nodes
        .iter()
        .map(|n| {
            let (neg, pos) = if FILE_FIRST_CHILD_IS_NEGATIVE {
                (n.front, n.back)
            } else {
                (n.back, n.front)
            };
            (n.poly, link(neg), link(pos))
        })
        .collect();
    CollParts {
        faces,
        nodes,
        root: link(m.root),
    }
}

/// Indices of the models `filter` selects.
pub fn selected_models(world: &World<'_>, filter: &WorldFilter) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, m) in world.models.iter().enumerate() {
        let take = match world.object_named(m.name) {
            None => filter.level,
            Some(o) => match o.role {
                Some(r) => filter.solid_roles.contains(&r),
                // DC-DEFAULT: SIM-18 (proposed, w1-doors) doors and switches are solid world models
                None if filter.is_mover(o) => true,
                None => filter.unlabelled,
            },
        };
        if take {
            out.push(i);
        }
    }
    out
}

/// Builds the collision world.
pub fn collision_world(world: &World<'_>, filter: &WorldFilter) -> CollisionWorld {
    let models = selected_models(world, filter)
        .into_iter()
        .map(|i| coll_model(&world.models[i]))
        .collect();
    let blockers: Vec<ConvexPoly> = if filter.blockers {
        world
            .blockers
            .polys
            .iter()
            .map(|b| ConvexPoly {
                normal: V3::from_array(b.normal),
                dist: b.dist,
                verts: b.verts.iter().map(|&v| V3::from_array(v)).collect(),
                flags: SURF_SOLID,
            })
            .collect()
    } else {
        Vec::new()
    };
    CollisionWorld::from_parts(models, &blockers)
}

/// The ladder volumes of a world: one convex region per ladder object, from the planes of the BSP model linked to
/// it by name (oriented so that the model's mean point lies inside), each giving the ladder container record.
// SPEC: FMT_DAT85.md §6.2 (ladder role, brush), §6.4 (object name = model name; 47 of 47 ladders linked);
// MOVEMENT_2009.md §11.1, §11.2 (kind 11), §11.4
// DC-DEFAULT: CLI-59 (proposed) ladder region = inside every plane of the ladder brush; record: kind 11, no current,
// no viscosity, friction factor 1
pub fn ladder_volumes(world: &World<'_>) -> ContainerVolumes {
    let mut out = ContainerVolumes::default();
    for m in &world.models {
        let is_ladder = world
            .object_named(m.name)
            .is_some_and(|o| o.role == Some(ObjectRole::Ladder));
        if !is_ladder || m.points.is_empty() {
            continue;
        }
        if let Some(v) = brush_volume(m) {
            out.volumes.push((v, Container::ladder()));
        }
    }
    out
}

/// The convex region inside every polygon plane of `m` (duplicate planes once), or `None` without planes.
pub fn brush_volume(m: &BspModel<'_>) -> Option<ConvexVolume> {
    let mut lo = V3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut hi = -lo;
    let mut sum = [0.0f64; 3];
    for p in m.points.iter() {
        let v = V3::from_array(*p);
        lo = V3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
        hi = V3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
        for (s, c) in sum.iter_mut().zip(p) {
            *s += *c as f64;
        }
    }
    let n = m.points.len() as f64;
    let mid = V3::new(
        (sum[0] / n) as f32,
        (sum[1] / n) as f32,
        (sum[2] / n) as f32,
    );
    let mut planes: Vec<(V3, f32)> = Vec::new();
    for p in m.polygons.iter() {
        let pl = m.planes[p.plane as usize];
        let (mut nrm, mut d) = (V3::from_array(pl.normal), pl.dist);
        if nrm.dot(mid) - d > 0.0 {
            nrm = -nrm;
            d = -d;
        }
        let dup = planes
            .iter()
            .any(|&(q, e)| (q - nrm).len() < 1e-4 && (e - d).abs() < 1e-2);
        if !dup {
            planes.push((nrm, d));
        }
    }
    (!planes.is_empty()).then(|| ConvexVolume::new(planes, lo, hi))
}

#[cfg(test)]
mod pose_tests {
    use super::*;
    use crate::collide::{build_bsp, SURF_SOLID};

    /// The six outward faces of an axis-aligned box.
    fn box_faces(a: V3, b: V3) -> Vec<ConvexPoly> {
        let c = V3::new;
        let quads = [
            [
                c(a.x, b.y, a.z),
                c(a.x, b.y, b.z),
                c(b.x, b.y, b.z),
                c(b.x, b.y, a.z),
            ],
            [
                c(a.x, a.y, a.z),
                c(b.x, a.y, a.z),
                c(b.x, a.y, b.z),
                c(a.x, a.y, b.z),
            ],
            [
                c(a.x, a.y, a.z),
                c(a.x, a.y, b.z),
                c(a.x, b.y, b.z),
                c(a.x, b.y, a.z),
            ],
            [
                c(b.x, a.y, a.z),
                c(b.x, b.y, a.z),
                c(b.x, b.y, b.z),
                c(b.x, a.y, b.z),
            ],
            [
                c(a.x, a.y, a.z),
                c(a.x, b.y, a.z),
                c(b.x, b.y, a.z),
                c(b.x, a.y, a.z),
            ],
            [
                c(a.x, a.y, b.z),
                c(b.x, a.y, b.z),
                c(b.x, b.y, b.z),
                c(a.x, b.y, b.z),
            ],
        ];
        quads
            .iter()
            .map(|q| ConvexPoly::from_corners(q.to_vec(), SURF_SOLID))
            .collect()
    }

    fn parts(a: V3, b: V3) -> CollParts {
        let faces = box_faces(a, b);
        let (nodes, root) = build_bsp(&faces);
        CollParts { faces, nodes, root }
    }

    fn world(m: CollModel) -> CollisionWorld {
        CollisionWorld::from_parts(alloc::vec![m], &[])
    }

    /// The same rays meet the posed model where they meet a model built at the moved place.
    fn same_hits(posed: &CollisionWorld, built: &CollisionWorld, rays: &[(V3, V3)]) {
        for &(f, t) in rays {
            let (p, b) = (posed.ray(f, t), built.ray(f, t));
            assert_eq!(p.is_some(), b.is_some(), "{f:?} -> {t:?}");
            if let (Some(p), Some(b)) = (p, b) {
                assert!((p.t - b.t).abs() < 1e-4, "{} {}", p.t, b.t);
                assert!((p.face_normal - b.face_normal).len() < 1e-4);
            }
        }
    }

    #[test]
    fn a_slid_door_is_solid_at_its_new_place_only() {
        // a door panel 4 thick (x), 100 high, 60 wide (z), origin at its centre, slid 60 along -z
        let (lo, hi) = (V3::new(-2.0, 0.0, -30.0), V3::new(2.0, 100.0, 30.0));
        let p = parts(lo, hi);
        let pose = ModelPose {
            origin: V3::new(0.0, 50.0, 0.0),
            pos: V3::new(0.0, 50.0, -60.0),
            rot: [0.0, 0.0, 0.0, 1.0],
        };
        let posed = world(p.build_posed(&pose));
        let off = V3::new(0.0, 0.0, -60.0);
        let built = world(CollModel::from_polys(&box_faces(lo + off, hi + off)));
        let rays = [
            (V3::new(-50.0, 40.0, 0.0), V3::new(50.0, 40.0, 0.0)),
            (V3::new(-50.0, 40.0, -60.0), V3::new(50.0, 40.0, -60.0)),
            (V3::new(-50.0, 40.0, -85.0), V3::new(50.0, 40.0, -85.0)),
            (V3::new(0.0, 150.0, -60.0), V3::new(0.0, -10.0, -60.0)),
        ];
        same_hits(&posed, &built, &rays);
        // the doorway (z = 0) is free once the door slid, blocked at the file position
        assert!(posed.ray(rays[0].0, rays[0].1).is_none());
        assert!(world(p.build()).ray(rays[0].0, rays[0].1).is_some());
    }

    #[test]
    fn a_turned_door_is_solid_where_it_swung() {
        // hinge at x = 0, z = 0; the panel runs along +z (0..60), 4 thick; turned 90 degrees about +y it runs
        // along +x (0..60): rot * (0, 0, 1) = (1, 0, 0)
        let (lo, hi) = (V3::new(-2.0, 0.0, 0.0), V3::new(2.0, 100.0, 60.0));
        let p = parts(lo, hi);
        let h = core::f32::consts::FRAC_1_SQRT_2;
        let pivot = V3::new(0.0, 0.0, 0.0);
        let origin = V3::new(0.0, 50.0, 30.0);
        let rot = [0.0, h, 0.0, h];
        let probe = ModelPose {
            origin: pivot,
            pos: pivot,
            rot,
        };
        assert!((probe.turn(V3::new(0.0, 0.0, 1.0)) - V3::new(1.0, 0.0, 0.0)).len() < 1e-6);
        // the object position goes around the hinge with the panel
        let pose = ModelPose {
            origin,
            pos: probe.point(origin),
            rot,
        };
        let posed = world(p.build_posed(&pose));
        let built = world(CollModel::from_polys(&box_faces(
            V3::new(0.0, 0.0, -2.0),
            V3::new(60.0, 100.0, 2.0),
        )));
        let rays = [
            (V3::new(-50.0, 40.0, 30.0), V3::new(50.0, 40.0, 30.0)),
            (V3::new(30.0, 40.0, -50.0), V3::new(30.0, 40.0, 50.0)),
            (V3::new(30.0, 150.0, 0.0), V3::new(30.0, -10.0, 0.0)),
            (V3::new(70.0, 40.0, -50.0), V3::new(70.0, 40.0, 50.0)),
        ];
        same_hits(&posed, &built, &rays);
        assert!(
            posed.ray(rays[0].0, rays[0].1).is_none(),
            "the doorway is free"
        );
        assert!(
            posed.ray(rays[1].0, rays[1].1).is_some(),
            "the swung panel blocks"
        );
    }

    #[test]
    fn the_default_filter_takes_the_movers() {
        let f = WorldFilter::default();
        assert_eq!(f.movers, MOVER_CLASSES.to_vec());
        assert!(!f.unlabelled);
        // the keys are the dserver's door and switch classes; a made-up own key matches by its name
        for k in &MOVER_CLASSES {
            assert!(!k.has_ours(), "{k:?} is ROM-only");
        }
        let own = Key::ours_only("dc.test_mover");
        let filter = WorldFilter {
            movers: alloc::vec![own],
            ..WorldFilter::default()
        };
        let bytes = dc_world::synth::SynthWorld {
            info: b"m".to_vec(),
            extent_min: [-1.0; 3],
            extent_max: [1.0; 3],
            tree_shape: alloc::vec![false],
            objects: alloc::vec![dc_world::synth::SynthObject::dc(
                "dc.test_mover",
                "m1",
                [0.0; 3],
                [0.0; 3],
                &[],
            )],
            ..Default::default()
        }
        .build();
        let w = World::parse_with(&bytes, dc_world::ParseOptions { render: false }).unwrap();
        assert!(filter.is_mover(&w.objects[0]));
        assert!(!WorldFilter::default().is_mover(&w.objects[0]));
    }
}
