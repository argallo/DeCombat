//! Which part of a remote character a shot hits: the 2009 node-sphere test.
//!
//! The 2009 client does not hit a character by its box alone. Its pellet trace (one engine segment query from the
//! eye along the pellet over the weapon's range) asks the shell's object filter about every object it passes; for a
//! character the filter answers "yes" only if the segment crosses one of the **node spheres** of the character's
//! skeleton row (a node with a radius > 0, centred on that node of the posed model). Only then does the engine test
//! the object's box, and that box is the object's box **doubled** about its centre: the hit point is where the
//! segment enters the doubled box, the reported normal that face's normal. The impact step then picks the node the
//! fire message names: among the crossed spheres the one with the smallest `|centre - origin|^2 - radius^2`, or
//! 0xff when none is crossed or the character has no skeleton row (`Skeleton` missing: 0xff, the filter lets such a
//! character through on its box alone).
//!
//! Nothing of the ROM is stored here: the skeleton rows, their node names and radii and the body rows' skeleton
//! numbers are read from the user's model table at run time; the node positions come from the body model's pose.
// EVIDENCE: cshell+0x23a910 (the pellet query: segment origin -> origin + range x dir, object filter
// cshell+0x235220, box test cshell+0x235330), cshell+0x235220 (filter: a character object -> cshell+0x234fb0 with
// the character's skeleton number and the query's two end points), cshell+0x234fb0 (any node sphere crossed),
// cshell+0x235c00 (node pick), cshell+0x238eb0 (the impact step stores the pick, the hit point and the plane normal
// for the fire message), cshell+0x1e4760 (the skeleton rows: `Node<k>Name` .. `Node<k>Radius`, the body rows'
// `Skeleton` with the default 0xff), cshell+0x1e10b0 / +0x1e1250 / +0x1e1550 (node count, row, radius of a skeleton)
// DISASM: cshell+0x235c00 (the pick's metric is the squared length of node - origin less the squared radius,
// compared with `<` against a start value of the squared segment length; the perpendicular test uses the squared
// length of dir x t - (node - origin), helper cshell+0x1fc9c0), cshell+0x235330 (box half extents x the .data float
// at rva 0x456bb8, 2.0 in the ROM binary, the only reader of that float), engine+0x40650 (the box test gets the
// object's box corners at object +0x04 / +0x10 after the filter said yes)
// OBSERVED: arena/20261026_032858_duel/B and captures 20261025_065016_tdm6 / 20261025_002236_tdm4
// (decoded 0x82 extra blocks: every hit point lies on the face of the doubled box, 29.9 .. 30.0 units from the
// victim's centre, and `extra_b` is that face's outward normal: (-1, 0, 0) for the real client's +x shots, (0, 0, 1)
// for the reference stack's -z shots)
// DC-DEFAULT: CLI-300 .. CLI-305 (proposed), see DC_DEFAULTS.md

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use dc_bute::ButeDoc;
use dc_render::{quat_from_key, AnimSet, Local, ModelFiles, Tracker, KEY_ORDER};
use dc_rez::RezVfs;
use dc_sim::Collider;
use glam::{Mat4, Quat, Vec3};

use crate::anims::{AnimFrame, ClipBlends, Layer};
use crate::arms::{RayHit, Target};
use crate::world::{RemoteBody, RemoteTracker};

/// The node number of "no node": no sphere crossed, or no skeleton row.
// EVIDENCE: cshell+0x235c00 (start value and the result of an empty pick), cshell+0x1e4760 (`Skeleton` default 0xff)
pub const NO_NODE: u8 = 0xff;
/// The factor on the object box's half extents in the shot's box test.
// DISASM: cshell+0x235330 (centre = (min + max) / 2, half = (max - min) / 2 x the float at rva 0x456bb8 = 2.0)
pub const HIT_BOX_SCALE: f32 = 2.0;

/// The model table holding the body rows and the skeleton rows.
// OBSERVED: ROM table keys ATTRIBUTES.rez, MODELBUTES.TXT (the archive and entry dc-render's character table reads)
const TABLE_REZ: &str = "ATTRIBUTES.rez";
const MODEL_TABLE: &str = "ATTRIBUTES\\MODELBUTES.TXT";
/// `[Model<n>]` keys `ModelFile`, `Skeleton`; `[Skeleton<s>]` keys `Node<k>Name`, `Node<k>Radius`.
// OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] ModelFile, Skeleton; [Skeleton<s>] Node<k>Name, Node<k>Radius
// EVIDENCE: cshell+0x1e4760 (the reader takes `Node<k>Name` while it exists, then the node's other keys)
const BODY_SECTION: &str = "Model";
const SKELETON_SECTION: &str = "Skeleton";
const KEY_MODEL_FILE: &str = "ModelFile";
const KEY_SKELETON: &str = "Skeleton";
const KEY_NODE: &str = "Node";
const NODE_NAME_SUFFIX: &str = "Name";
const NODE_RADIUS_SUFFIX: &str = "Radius";
/// Upper bound of the node scan (node numbers are bytes, 0xff is "none").
const MAX_NODES: u32 = 255;

// --------------------------------------------------------------------------------------------- the pick

/// One node sphere of a posed character: the skeleton row's node number, its world centre and radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeSphere {
    pub node: u8,
    pub centre: [f32; 3],
    pub radius: f32,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The segment's unit direction and length (`None` for a zero-length segment).
fn segment(from: [f32; 3], to: [f32; 3]) -> Option<([f32; 3], f32)> {
    let d = sub(to, from);
    let len = dot(d, d).sqrt();
    (len != 0.0 && len.is_finite()).then(|| ([d[0] / len, d[1] / len, d[2] / len], len))
}

/// The sphere test of one node against the segment (unit direction `dir`, length `len`): the projection of the
/// centre lies within `[-r, len + r]` along the segment and the centre's squared distance from the line is at most
/// `r^2`. Returns the squared length of `centre - from`.
// EVIDENCE: cshell+0x234fb0, cshell+0x235c00 (the same test in both)
fn crossed(s: &NodeSphere, from: [f32; 3], dir: [f32; 3], len: f32) -> Option<f32> {
    let r = s.radius;
    if r.is_nan() || r <= 0.0 {
        return None;
    }
    let v = sub(s.centre, from);
    let t = dot(v, dir);
    if !(-r <= t && t <= r + len) {
        return None;
    }
    let p = [dir[0] * t - v[0], dir[1] * t - v[1], dir[2] * t - v[2]];
    (dot(p, p) <= r * r).then(|| dot(v, v))
}

/// The shot filter of a character: does the segment `from -> to` cross any node sphere?
// EVIDENCE: cshell+0x234fb0 (1 at the first crossed sphere, 0 when none; a zero-length segment gives 0)
pub fn crosses_any(spheres: &[NodeSphere], from: [f32; 3], to: [f32; 3]) -> bool {
    let Some((dir, len)) = segment(from, to) else {
        return false;
    };
    spheres.iter().any(|s| crossed(s, from, dir, len).is_some())
}

/// The node the fire message names: among the spheres the segment crosses, the one with the smallest
/// `|centre - from|^2 - r^2` below `|to - from|^2` (the first in row order on a tie); [`NO_NODE`] when none.
// EVIDENCE: cshell+0x235c00; DISASM: cshell+0x235e76..0x235e95 (metric = |v|^2 - r^2, kept when strictly below the best)
pub fn pick_node(spheres: &[NodeSphere], from: [f32; 3], to: [f32; 3]) -> u8 {
    let Some((dir, len)) = segment(from, to) else {
        return NO_NODE;
    };
    let mut best = len * len;
    let mut node = NO_NODE;
    for s in spheres {
        if let Some(v2) = crossed(s, from, dir, len) {
            let m = v2 - s.radius * s.radius;
            if m < best {
                best = m;
                node = s.node;
            }
        }
    }
    node
}

/// Where a segment meets a character's shot box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxHit {
    pub point: [f32; 3],
    /// Fraction of the segment from `from` to `point`.
    pub t: f32,
    /// The outward normal of the face the segment entered.
    pub normal: [f32; 3],
}

/// The 2009 shot box test: the box `centre +- HIT_BOX_SCALE x half`, tested face by face in the order x, y, z: a
/// segment starting beyond a face's plane meets that face when the point on the plane lies strictly inside the other
/// two extents (a segment that ends beyond the same plane misses the box at once); a segment that starts strictly
/// inside the box hits at its start.
// DISASM: cshell+0x235330 (face order x, y, z; the early exits; normals +-1 on the face axis; the inside case returns
// the start point)
// DC-DEFAULT: CLI-301 (proposed) the inside case's normal points from the box centre to the start point (the
// disassembly normalises a difference of two points there; which second point is PLAUSIBLE)
pub fn box_entry(from: [f32; 3], to: [f32; 3], centre: [f32; 3], half: [f32; 3]) -> Option<BoxHit> {
    let lo: [f32; 3] = std::array::from_fn(|k| centre[k] - half[k] * HIT_BOX_SCALE);
    let hi: [f32; 3] = std::array::from_fn(|k| centre[k] + half[k] * HIT_BOX_SCALE);
    let at = |t: f32| -> [f32; 3] { std::array::from_fn(|k| from[k] + (to[k] - from[k]) * t) };
    let inside_others = |p: &[f32; 3], a: usize| {
        (0..3)
            .filter(|&k| k != a)
            .all(|k| lo[k] < p[k] && p[k] < hi[k])
    };
    for a in 0..3 {
        let (plane, sign) = if lo[a] <= from[a] {
            if hi[a] < from[a] {
                if hi[a] < to[a] {
                    return None;
                }
                (hi[a], 1.0)
            } else {
                continue;
            }
        } else {
            if to[a] < lo[a] {
                return None;
            }
            (lo[a], -1.0)
        };
        let t = (plane - from[a]) / (to[a] - from[a]);
        let mut p = at(t);
        if inside_others(&p, a) {
            p[a] = plane;
            let mut normal = [0.0; 3];
            normal[a] = sign;
            return Some(BoxHit {
                point: p,
                t,
                normal,
            });
        }
    }
    if (0..3).all(|k| lo[k] < from[k] && from[k] < hi[k]) {
        let d = sub(from, centre);
        let l = dot(d, d).sqrt();
        let normal = if l > 0.0 {
            [d[0] / l, d[1] / l, d[2] / l]
        } else {
            [0.0, 1.0, 0.0]
        };
        return Some(BoxHit {
            point: from,
            t: 0.0,
            normal,
        });
    }
    None
}

/// The node spheres of this frame's characters, by object; an object that is not listed has no skeleton (its box
/// alone counts and the node is [`NO_NODE`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameNodes {
    pub spheres: BTreeMap<u16, Vec<NodeSphere>>,
}

impl FrameNodes {
    pub fn of(&self, obj: u16) -> Option<&[NodeSphere]> {
        self.spheres.get(&obj).map(Vec::as_slice)
    }
}

/// What the node test did with the pellets (reports and tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NodeStats {
    /// Character hits that named a node.
    pub node_hits: u32,
    /// Character hits without a skeleton (node 0xff).
    pub box_hits: u32,
    /// Times a pellet entered a character's shot box without crossing one of its node spheres (no hit there).
    pub sphere_misses: u32,
}

/// A pellet's trace: the nearest character whose node spheres the whole segment crosses (or that has no skeleton)
/// and whose shot box it enters, unless the world stops it first. The hit carries the box entry (distance and face
/// normal) and the picked node.
// EVIDENCE: cshell+0x23a910 (one query origin -> origin + range x dir), engine+0x40650 (filter, then the box test,
// the nearest hit point wins), cshell+0x238eb0 (node, point and normal of the hit)
// DC-DEFAULT: CLI-300 (proposed) replaces CLI-43's plain box with node 24
pub fn trace(
    origin: [f32; 3],
    dir: [f32; 3],
    range: f32,
    targets: &[Target],
    nodes: &FrameNodes,
    world: Option<&dyn Collider>,
    stats: &mut NodeStats,
) -> RayHit {
    let to: [f32; 3] = std::array::from_fn(|k| origin[k] + dir[k] * range);
    let mut best: Option<(f32, u16, BoxHit)> = None;
    for t in targets {
        let Some(b) = box_entry(origin, to, t.centre, t.half) else {
            continue;
        };
        if let Some(s) = nodes.of(t.obj) {
            if !crosses_any(s, origin, to) {
                stats.sphere_misses += 1;
                continue;
            }
        }
        let dist = b.t * range;
        if best.is_none_or(|(d, _, _)| dist < d) {
            best = Some((dist, t.obj, b));
        }
    }
    let Some((dist, obj, b)) = best else {
        return RayHit::Miss;
    };
    if let Some(w) = world {
        if crate::arms::world_hit(w, origin, dir, dist).is_some_and(|wd| wd < dist) {
            return RayHit::Miss;
        }
    }
    let node = match nodes.of(obj) {
        Some(s) => {
            stats.node_hits += 1;
            pick_node(s, origin, to)
        }
        None => {
            stats.box_hits += 1;
            NO_NODE
        }
    };
    RayHit::Character {
        obj,
        dist,
        node,
        normal: b.normal,
    }
}

// --------------------------------------------------------------------------------------------- the ROM rows

/// One node of a skeleton row.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeRow {
    pub name: String,
    pub radius: f32,
}

/// The skeleton rows of the model table and the body rows' model files with their skeleton numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HitSkeletons {
    /// `[Skeleton<s>]` nodes in row order (the node number is the position).
    pub skeletons: BTreeMap<u32, Vec<NodeRow>>,
    /// `[Model<n>]`: (model file, skeleton number; `None` = no `Skeleton` key, the 0xff of the 2009 reader).
    pub bodies: BTreeMap<u32, (String, Option<u32>)>,
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"').trim()
}

fn num(v: Option<&String>) -> Option<f32> {
    let t = unquote(v?);
    // a typed value such as `(DWORD)0`
    let t = match t.strip_prefix('(') {
        Some(r) => r.split_once(')').map_or(r, |x| x.1).trim(),
        None => t,
    };
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

fn index_of(section: &str, prefix: &str) -> Option<u32> {
    let rest = section.strip_prefix(prefix)?;
    (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| rest.parse().ok())
        .flatten()
}

fn path_key(p: &str) -> String {
    p.trim().replace('/', "\\").to_ascii_uppercase()
}

impl HitSkeletons {
    pub fn from_doc(doc: &ButeDoc) -> HitSkeletons {
        let mut t = HitSkeletons::default();
        for (name, sec) in doc.iter() {
            if let Some(s) = index_of(name, SKELETON_SECTION) {
                let nodes = (0..MAX_NODES)
                    .map_while(|k| {
                        let n = sec.get(&format!("{KEY_NODE}{k}{NODE_NAME_SUFFIX}"))?;
                        Some(NodeRow {
                            name: unquote(n).to_string(),
                            radius: num(sec.get(&format!("{KEY_NODE}{k}{NODE_RADIUS_SUFFIX}")))
                                .unwrap_or(0.0),
                        })
                    })
                    .collect();
                t.skeletons.insert(s, nodes);
            } else if let Some(n) = index_of(name, BODY_SECTION) {
                let Some(model) = sec.get(KEY_MODEL_FILE) else {
                    continue;
                };
                let skel = num(sec.get(KEY_SKELETON))
                    .filter(|v| *v >= 0.0 && *v < 255.0)
                    .map(|v| v as u32);
                t.bodies.insert(n, (unquote(model).to_string(), skel));
            }
        }
        t
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<HitSkeletons, String> {
        match ButeDoc::try_from_vfs(vfs, TABLE_REZ, MODEL_TABLE) {
            Ok(Some(d)) => Ok(HitSkeletons::from_doc(&d)),
            Ok(None) => Err("model table missing".into()),
            Err(e) => Err(format!("model table: {e}")),
        }
    }

    /// The skeleton row of the body model `path`: the `Skeleton` of the first body row (by row number) whose
    /// model file it is; `None` without such a row or key.
    // DC-DEFAULT: CLI-302 (proposed) the 2009 client knows the character's body row; this client knows the model
    // file the server named and takes that file's first row
    pub fn skeleton_of_model(&self, path: &str) -> Option<&[NodeRow]> {
        let s = self.skeleton_number_of_model(path)?;
        self.skeletons.get(&s).map(Vec::as_slice)
    }

    /// The skeleton row number [`HitSkeletons::skeleton_of_model`] uses.
    pub fn skeleton_number_of_model(&self, path: &str) -> Option<u32> {
        let want = path_key(path);
        self.bodies
            .values()
            .find(|(m, _)| path_key(m) == want)
            .and_then(|(_, s)| *s)
    }

    /// Body model files whose rows name different skeletons (the CLI-302 choice is then not unique).
    pub fn ambiguous_models(&self) -> usize {
        let mut by: HashMap<String, Vec<Option<u32>>> = HashMap::new();
        for (m, s) in self.bodies.values() {
            let v = by.entry(path_key(m)).or_default();
            if !v.contains(s) {
                v.push(*s);
            }
        }
        by.values().filter(|v| v.len() > 1).count()
    }
}

// --------------------------------------------------------------------------------------------- posed rigs

/// Where a sphere sits on the model: a node, or a socket (its node and the socket's local transform).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Attach {
    Node(usize),
    Socket(usize, Mat4),
}

/// A body model with the spheres of its skeleton row resolved onto its nodes.
#[derive(Debug, Clone)]
pub struct HitRig {
    pub set: AnimSet,
    spheres: Vec<(u8, f32, Attach)>,
    /// Row nodes with a radius > 0 that name neither a node nor a socket of the model.
    pub unresolved: usize,
}

impl HitRig {
    /// Resolves each row node with a radius > 0 by name: a model node first, else a socket of that name.
    // EVIDENCE: cshell+0x235c00 / +0x234fb0 (radius > 0 and a name; the node is looked up on the object by name
    // through the model interface (+0x34, then its transform +0x3c in world space), else a socket by name (+0x18,
    // its transform +0x1c); neither found: the node is skipped)
    // DC-DEFAULT: CLI-303 (proposed) names compare without case; the body model's nodes and sockets only
    pub fn new(files: &ModelFiles, row: &[NodeRow]) -> HitRig {
        let body = &files.body;
        let mut spheres = Vec::new();
        let mut unresolved = 0;
        for (k, n) in row.iter().enumerate().take(MAX_NODES as usize) {
            if n.radius.is_nan() || n.radius <= 0.0 || n.name.is_empty() {
                continue;
            }
            let name = n.name.as_bytes();
            let at = body
                .nodes
                .iter()
                .position(|m| m.name.as_bytes().eq_ignore_ascii_case(name))
                .map(Attach::Node)
                .or_else(|| {
                    body.sockets
                        .iter()
                        .find(|s| s.name.as_bytes().eq_ignore_ascii_case(name))
                        .filter(|s| (s.node as usize) < body.nodes.len())
                        .map(|s| {
                            let rot = quat_from_key(s.rot, KEY_ORDER);
                            Attach::Socket(
                                s.node as usize,
                                Mat4::from_rotation_translation(rot, Vec3::from(s.pos)),
                            )
                        })
                });
            match at {
                Some(a) => spheres.push((k as u8, n.radius, a)),
                None => unresolved += 1,
            }
        }
        HitRig {
            set: files.anim_set(),
            spheres,
            unresolved,
        }
    }

    pub fn sphere_count(&self) -> usize {
        self.spheres.len()
    }

    /// The world spheres of the model posed by `locals` with the world transform `world`.
    pub fn spheres_at(&self, locals: &[Local], world: Mat4) -> Vec<NodeSphere> {
        let globals = self.set.skeleton.globals(locals);
        self.spheres
            .iter()
            .filter_map(|&(node, radius, at)| {
                let m = match at {
                    Attach::Node(i) => *globals.get(i)?,
                    Attach::Socket(i, local) => *globals.get(i)? * local,
                };
                let c = (world * m).w_axis.truncate();
                Some(NodeSphere {
                    node,
                    centre: c.to_array(),
                    radius,
                })
            })
            .collect()
    }
}

/// The local pose of blend layers: the pose of the shown trackers, mixed per node toward the pose with a
/// blending tracker's previous clip by `1 - fraction` (the pose the character pass draws).
// DC-DEFAULT: CLI-182 (proposed) whole-pose mix (EVIDENCE engine+0x2e10 for the fraction)
pub fn layered_pose(set: &AnimSet, layers: &[Layer]) -> Vec<Local> {
    let tr = |t: &RemoteTracker| Tracker {
        clip: t.clip,
        time_ms: t.time_ms,
        blend_set: t.blend_set,
    };
    let current: Vec<Tracker> = layers.iter().map(|x| tr(&x.tracker)).collect();
    let mut locals = set.pose(&current);
    for (i, x) in layers.iter().enumerate() {
        let Some(from) = x.from.as_ref() else {
            continue;
        };
        let mut alt = current.clone();
        alt[i] = tr(from);
        let prev = set.pose(&alt);
        let f = x.fraction.clamp(0.0, 1.0);
        for (c, p) in locals.iter_mut().zip(prev) {
            c.0 = p.0.lerp(c.0, f);
            c.1 = p.1.slerp(c.1, f).normalize();
        }
    }
    locals
}

/// The world transform of a character model: the yaw about +Y at the model origin (CLI-40, the box bottom).
pub fn character_world(r: &RemoteBody) -> Mat4 {
    Mat4::from_rotation_translation(
        Quat::from_rotation_y(r.yaw.unwrap_or(0.0)),
        crate::characters::model_origin(r.pos, r.half),
    )
}

/// A rig as the frame sees it.
#[derive(Debug, Clone)]
enum RigSlot {
    Loading,
    Ready(Arc<HitRig>),
    /// No skeleton row for the model, a row without a resolvable sphere, or a model that does not load.
    Without,
}

type RigMap = Arc<Mutex<HashMap<String, RigSlot>>>;

/// The hit rigs of the characters in view, read from the user's ROM (body models with their skeleton rows), and
/// the per-frame node spheres.
///
/// A model loads once per file, on a thread of its own by default (until it is there its character is hit by its
/// box with node 0xff, as a character without a skeleton row) or inline ([`HitRigs::new`] with `asynchronous`
/// false, for tests and pictures).
// DC-DEFAULT: CLI-304 (proposed)
#[derive(Debug)]
pub struct HitRigs {
    rom: Option<PathBuf>,
    asynchronous: bool,
    table: Option<Arc<HitSkeletons>>,
    table_tried: bool,
    vfs: Option<RezVfs>,
    rigs: RigMap,
    blends: ClipBlends,
}

impl HitRigs {
    pub fn new(rom: Option<PathBuf>, asynchronous: bool) -> HitRigs {
        HitRigs {
            rom,
            asynchronous,
            table: None,
            table_tried: false,
            vfs: None,
            rigs: Arc::new(Mutex::new(HashMap::new())),
            blends: ClipBlends::default(),
        }
    }

    fn table(&mut self) -> Option<Arc<HitSkeletons>> {
        if !self.table_tried {
            self.table_tried = true;
            if let Some(rom) = &self.rom {
                let vfs = RezVfs::new(rom);
                self.table = HitSkeletons::from_vfs(&vfs).ok().map(Arc::new);
                self.vfs = Some(vfs);
            }
        }
        self.table.clone()
    }

    /// The body model of a remote character: its first model entry, else the default character's body row.
    fn body_model(r: &RemoteBody, table: &HitSkeletons) -> Option<String> {
        let look = r.look.as_ref()?;
        match look.models.first() {
            Some(m) => Some(m.model.clone()),
            None => table
                .bodies
                .get(&crate::characters::DEFAULT_BODY_ROW)
                .map(|b| b.0.clone()),
        }
    }

    fn rig(&mut self, model: &str, table: &Arc<HitSkeletons>) -> Option<Arc<HitRig>> {
        let key = path_key(model);
        let slot = self
            .rigs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned();
        match slot {
            Some(RigSlot::Ready(r)) => return Some(r),
            Some(RigSlot::Loading | RigSlot::Without) => return None,
            None => {}
        }
        let Some(row) = table.skeleton_of_model(model).map(<[NodeRow]>::to_vec) else {
            self.put(&key, RigSlot::Without);
            return None;
        };
        let build = move |vfs: &RezVfs, path: &str| match ModelFiles::load(vfs, path) {
            Ok(f) => {
                let rig = HitRig::new(&f, &row);
                if rig.sphere_count() == 0 {
                    RigSlot::Without
                } else {
                    RigSlot::Ready(Arc::new(rig))
                }
            }
            Err(_) => RigSlot::Without,
        };
        if self.asynchronous {
            let rom = self.rom.clone()?;
            self.put(&key, RigSlot::Loading);
            let rigs = self.rigs.clone();
            let path = model.to_string();
            let spawned = std::thread::Builder::new()
                .name("dc-hit-rig".into())
                .spawn(move || {
                    let s = build(&RezVfs::new(&rom), &path);
                    rigs.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(key, s);
                });
            if spawned.is_err() {
                self.put(&path_key(model), RigSlot::Without);
            }
            None
        } else {
            let s = build(self.vfs.as_ref()?, model);
            self.put(&key, s.clone());
            match s {
                RigSlot::Ready(r) => Some(r),
                _ => None,
            }
        }
    }

    fn put(&self, key: &str, s: RigSlot) {
        self.rigs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string(), s);
    }

    /// The node spheres of this frame's characters: each visible character whose body model has a resolved rig,
    /// posed by its trackers as the character pass poses it (the clip-switch blends, the play-once holds of
    /// `anims`), at its position and yaw.
    pub fn frame(&mut self, remotes: &[RemoteBody], anims: Option<&AnimFrame>) -> FrameNodes {
        let mut out = FrameNodes::default();
        let Some(table) = self.table() else {
            return out;
        };
        let live: Vec<u16> = remotes.iter().map(|r| r.obj).collect();
        self.blends.retain(&live);
        for r in remotes {
            let Some(look) = r.look.as_ref().filter(|l| l.visible) else {
                continue;
            };
            if !r.pos.iter().chain(r.half.iter()).all(|v| v.is_finite()) {
                continue;
            }
            let Some(model) = Self::body_model(r, &table) else {
                continue;
            };
            let Some(rig) = self.rig(&model, &table) else {
                continue;
            };
            let set = &rig.set;
            let mut layers = self.blends.step(
                r.obj,
                &look.trackers,
                |c| set.clip(c).map(|c| c.blend_ms),
                |c| set.clip_length_ms(c),
            );
            if let Some(a) = anims {
                let mut shown: Vec<RemoteTracker> = layers.iter().map(|x| x.tracker).collect();
                crate::anims::hold_once(&mut shown, a.once_of(r.obj), |c| set.clip_length_ms(c));
                for (x, t) in layers.iter_mut().zip(shown) {
                    x.tracker = t;
                }
            }
            let locals = layered_pose(set, &layers);
            out.spheres
                .insert(r.obj, rig.spheres_at(&locals, character_world(r)));
        }
        out
    }

    /// Rigs still loading (async mode).
    pub fn loading(&self) -> usize {
        self.rigs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|s| matches!(s, RigSlot::Loading))
            .count()
    }

    /// The resolved rig of a body model file, if it is loaded.
    pub fn loaded_rig(&self, model: &str) -> Option<Arc<HitRig>> {
        match self
            .rigs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&path_key(model))
        {
            Some(RigSlot::Ready(r)) => Some(r.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere(node: u8, c: [f32; 3], r: f32) -> NodeSphere {
        NodeSphere {
            node,
            centre: c,
            radius: r,
        }
    }

    #[test]
    fn the_pick_takes_the_smallest_tangent_metric_and_needs_a_crossing() {
        let o = [0.0, 0.0, 0.0];
        let to = [0.0, 0.0, 1000.0];
        // a big sphere far along, a small one near: the metric |v|^2 - r^2 picks the near one
        let s = [
            sphere(1, [0.0, 0.0, 500.0], 20.0),
            sphere(2, [0.0, 1.0, 100.0], 2.0),
        ];
        assert_eq!(pick_node(&s, o, to), 2);
        // a larger radius at the same place lowers the metric: 400^2 - 399^2 < 100^2 - 2^2
        let s2 = [s[1], sphere(3, [0.0, 0.0, 400.0], 399.0)];
        assert_eq!(pick_node(&s2, o, to), 3);
        // ties keep the first in row order
        let tie = [
            sphere(7, [0.0, 0.0, 50.0], 3.0),
            sphere(8, [0.0, 0.0, 50.0], 3.0),
        ];
        assert_eq!(pick_node(&tie, o, to), 7);
        // not crossed: beside the line, behind the start by more than r, beyond the end by more than r
        let none = [
            sphere(1, [0.0, 6.0, 100.0], 5.0),
            sphere(2, [0.0, 0.0, -6.0], 5.0),
            sphere(3, [0.0, 0.0, 1006.0], 5.0),
            sphere(4, [0.0, 0.0, 100.0], 0.0),
        ];
        assert_eq!(pick_node(&none, o, to), NO_NODE);
        assert!(!crosses_any(&none, o, to));
        // within r behind the start still counts for the filter (the projection test allows -r)
        assert!(crosses_any(&[sphere(5, [0.0, 0.0, -4.0], 5.0)], o, to));
        // a zero-length segment crosses nothing
        assert!(!crosses_any(&s, o, o));
        assert_eq!(pick_node(&s, o, o), NO_NODE);
    }

    #[test]
    fn the_box_test_uses_the_doubled_box_and_reports_the_face_normal() {
        let c = [100.0, 50.0, 0.0];
        let h = [15.0, 50.0, 15.0];
        // from -x: the x-min face of the doubled box at x = 70
        let b = box_entry([0.0, 60.0, 0.0], [300.0, 60.0, 0.0], c, h).unwrap();
        assert_eq!(b.normal, [-1.0, 0.0, 0.0]);
        assert!((b.point[0] - 70.0).abs() < 1e-4 && (b.t - 70.0 / 300.0).abs() < 1e-6);
        // from +z towards -z: the z-max face at z = 30
        let b = box_entry([100.0, 60.0, 200.0], [100.0, 60.0, -200.0], c, h).unwrap();
        assert_eq!(b.normal, [0.0, 0.0, 1.0]);
        assert!((b.point[2] - 30.0).abs() < 1e-4);
        // 25 units beside the centre: outside the 15-unit box, inside the doubled one
        assert!(box_entry([0.0, 60.0, 25.0], [300.0, 60.0, 25.0], c, h).is_some());
        assert!(box_entry([0.0, 60.0, 31.0], [300.0, 60.0, 31.0], c, h).is_none());
        // the segment ends before the box
        assert!(box_entry([0.0, 60.0, 0.0], [60.0, 60.0, 0.0], c, h).is_none());
        // starting inside: the start point
        let b = box_entry([110.0, 60.0, 0.0], [400.0, 60.0, 0.0], c, h).unwrap();
        assert_eq!((b.point, b.t), ([110.0, 60.0, 0.0], 0.0));
    }

    #[test]
    fn a_character_with_spheres_needs_a_crossed_sphere_one_without_its_box_only() {
        let t = Target {
            obj: 9,
            centre: [0.0, 50.0, 200.0],
            half: [15.0, 50.0, 15.0],
        };
        let mut nodes = FrameNodes::default();
        nodes.spheres.insert(
            9,
            vec![
                sphere(53, [0.0, 80.0, 200.0], 4.6),
                sphere(6, [0.0, 60.0, 200.0], 10.0),
            ],
        );
        let mut st = NodeStats::default();
        // the head line
        let h = trace(
            [0.0, 80.0, 0.0],
            [0.0, 0.0, 1.0],
            3000.0,
            &[t],
            &nodes,
            None,
            &mut st,
        );
        match h {
            RayHit::Character {
                obj,
                dist,
                node,
                normal,
            } => {
                assert_eq!((obj, node, normal), (9, 53, [0.0, 0.0, -1.0]));
                assert!((dist - 170.0).abs() < 1e-3, "{dist}");
            }
            RayHit::Miss => panic!("miss"),
        }
        // 6 units beside the head: in the box, in no sphere -> no hit
        let m = trace(
            [6.0, 80.0, 0.0],
            [0.0, 0.0, 1.0],
            3000.0,
            &[t],
            &nodes,
            None,
            &mut st,
        );
        assert_eq!(m, RayHit::Miss);
        assert_eq!(
            st,
            NodeStats {
                node_hits: 1,
                box_hits: 0,
                sphere_misses: 1
            }
        );
        // no skeleton: the box alone, node 0xff
        let b = trace(
            [6.0, 80.0, 0.0],
            [0.0, 0.0, 1.0],
            3000.0,
            &[t],
            &FrameNodes::default(),
            None,
            &mut st,
        );
        assert!(
            matches!(b, RayHit::Character { node: NO_NODE, .. }),
            "{b:?}"
        );
        assert_eq!(st.box_hits, 1);
    }

    /// The ROM row's `Node<k>Location` of skeleton `s` (read here, for the assertions only).
    fn location(rom: &std::path::Path, s: u32, k: u8) -> Option<u32> {
        let vfs = RezVfs::new(rom);
        let doc = ButeDoc::try_from_vfs(&vfs, TABLE_REZ, MODEL_TABLE).ok()??;
        let sec = doc.get(&format!("{SKELETON_SECTION}{s}"))?;
        num(sec.get(&format!("{KEY_NODE}{k}Location"))).map(|v| v as u32)
    }

    fn standing(rig: &HitRig, obj: u16, pos: [f32; 3], yaw: f32) -> RemoteBody {
        let clip = |k: dc_render::NameKey| {
            rig.set
                .find_clip(|n| dc_render::name_key(n) == k)
                .expect("stand clip")
        };
        RemoteBody {
            obj,
            pid: 1,
            name: "Stand".into(),
            team: 1,
            pos,
            half: [15.0, 50.0, 15.0],
            yaw: Some(yaw),
            look: Some(crate::world::RemoteLook {
                styles: Vec::new(),
                models: Vec::new(),
                weapon: None,
                trackers: vec![
                    RemoteTracker {
                        clip: 0,
                        time_ms: 0.0,
                        blend_set: None,
                    },
                    RemoteTracker {
                        clip: clip(dc_render::CLIP_UPPER_STAND),
                        time_ms: 0.0,
                        blend_set: Some(0),
                    },
                    RemoteTracker {
                        clip: clip(dc_render::CLIP_LOWER_STAND),
                        time_ms: 0.0,
                        blend_set: Some(1),
                    },
                ],
                visible: true,
                aim: None,
            }),
        }
    }

    /// ROM-gated: the default character standing (the server's stand clips) 200 units in front of an eye at the
    /// 2009 eye height: a level ray at the eye picks a head-location node, a ray at chest height a torso-location
    /// node, and a level ray 6 units beside the head crosses no sphere (no hit).
    #[test]
    fn rom_the_standing_default_body_takes_head_and_chest_rays() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let mut rigs = HitRigs::new(Some(rom.clone()), false);
        let table = rigs.table().expect("model table");
        println!(
            "body model files with conflicting skeleton rows: {}",
            table.ambiguous_models()
        );
        let (model, skel) = table
            .bodies
            .get(&crate::characters::DEFAULT_BODY_ROW)
            .cloned()
            .expect("default body row");
        let skel = skel.expect("the default body row names a skeleton row");
        let rig = rigs.rig(&model, &table).expect("rig");
        println!(
            "spheres resolved {} unresolved {}",
            rig.sphere_count(),
            rig.unresolved
        );
        // the character faces the eye (forward = (sin yaw, 0, cos yaw)): it stands at +z and looks along -z
        let pos = [0.0, 50.0, 200.0];
        let r = standing(&rig, 7, pos, std::f32::consts::PI);
        let eye_y = pos[1] + crate::todo::EYE_ABOVE_CENTRE_FRACTION * r.half[1];
        let eye = [0.0, eye_y, 0.0];
        let shoot = |r: &RemoteBody, nodes: &FrameNodes, at: [f32; 3]| {
            let d = sub(at, eye);
            let l = dot(d, d).sqrt();
            let dir = [d[0] / l, d[1] / l, d[2] / l];
            let t = Target {
                obj: r.obj,
                centre: r.pos,
                half: r.half,
            };
            let mut st = NodeStats::default();
            trace(eye, dir, 3000.0, &[t], nodes, None, &mut st)
        };
        let loc = |n: u8| location(&rom, skel, n);
        let biggest = |sp: &[NodeSphere], want: u32| -> NodeSphere {
            *sp.iter()
                .filter(|s| loc(s.node) == Some(want))
                .max_by(|a, b| a.radius.total_cmp(&b.radius))
                .expect("a sphere of that location")
        };
        let node_loc = |h: RayHit| match h {
            RayHit::Character { node, .. } => loc(node),
            RayHit::Miss => None,
        };
        // facing the eye: a level ray at the eye height through the drawn head names a head node
        let nodes = rigs.frame(std::slice::from_ref(&r), None);
        let spheres = nodes.of(7).expect("posed spheres").to_vec();
        let head = biggest(&spheres, 1);
        let h = shoot(&r, &nodes, [head.centre[0], eye_y, head.centre[2]]);
        println!(
            "head: sphere {head:?} -> {h:?}; the box axis at the eye height -> {:?}",
            shoot(&r, &nodes, [pos[0], eye_y, pos[2]])
        );
        assert_eq!(node_loc(h), Some(1), "{h:?}");
        // 6 units beside the head on either side, level at the eye height: no sphere, no hit
        let heads: Vec<NodeSphere> = spheres
            .iter()
            .copied()
            .filter(|s| loc(s.node) == Some(1))
            .collect();
        for sign in [1.0f32, -1.0] {
            let edge = heads
                .iter()
                .max_by(|a, b| (sign * a.centre[0]).total_cmp(&(sign * b.centre[0])))
                .expect("head spheres");
            let at = [edge.centre[0] + sign * 6.0, eye_y, edge.centre[2]];
            let m = shoot(&r, &nodes, at);
            let to = [at[0], at[1], at[2] + 3000.0];
            println!("beside ({sign}): {m:?}");
            assert_eq!(m, RayHit::Miss);
            assert!(!crosses_any(&heads, [at[0], eye_y, 0.0], to));
        }
        // turned away: a ray at the drawn torso names a torso node (facing the eye the rifle arms cover it)
        let back = standing(&rig, 8, pos, 0.0);
        let nodes = rigs.frame(std::slice::from_ref(&back), None);
        let torso = biggest(nodes.of(8).expect("posed spheres"), 2);
        let t = shoot(&back, &nodes, torso.centre);
        println!("torso: sphere {torso:?} -> {t:?}");
        assert_eq!(node_loc(t), Some(2), "{t:?}");
    }

    /// A known posed skeleton (ROM-free): a root, a spine 50 units up and a head 30 above the spine with a socket 5
    /// in front of it; the spine bent 90 degrees forward, the model turned 90 degrees and moved. The spheres sit
    /// where the pose puts the nodes, the pick takes the nearer tangent along each ray, a row node without a model
    /// node or socket is skipped, and a ray 6 units beside the head crosses nothing.
    #[test]
    fn spheres_follow_a_known_pose_and_the_pick_follows_the_ray() {
        use dc_model::{Model, Node, RawStr, Socket};
        let at = |x: f32, y: f32, z: f32| -> [f32; 16] {
            [
                1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, y, 0.0, 0.0, 1.0, z, 0.0, 0.0, 0.0, 1.0,
            ]
        };
        let node =
            |name: &str, i: u16, xf: [f32; 16], parent: Option<usize>, kids: Vec<usize>| Node {
                name: RawStr(name.as_bytes().to_vec()),
                index: i,
                flags: 0,
                model_xform: xf,
                parent,
                children: kids,
            };
        let m = Model {
            nodes: vec![
                node("Root", 0, at(0.0, 0.0, 0.0), None, vec![1]),
                node("Spine", 1, at(0.0, 50.0, 0.0), Some(0), vec![2]),
                node("Head", 2, at(0.0, 80.0, 0.0), Some(1), Vec::new()),
            ],
            sockets: vec![Socket {
                node: 2,
                name: RawStr(b"Brow".to_vec()),
                rot: [0.0, 0.0, 0.0, 1.0],
                pos: [0.0, 0.0, 5.0],
                scale: [1.0; 3],
            }],
            ..Model::default()
        };
        let files = ModelFiles::from_model("synthetic.ltb", m);
        let row = |n: &str, r: f32| NodeRow {
            name: n.into(),
            radius: r,
        };
        // node numbers are row positions: 0 root (radius 0: never a sphere), 1 spine, 2 head, 3 missing, 4 socket
        let rig = HitRig::new(
            &files,
            &[
                row("root", 0.0),
                row("SPINE", 10.0),
                row("head", 4.0),
                row("Tail", 3.0),
                row("brow", 1.5),
            ],
        );
        assert_eq!((rig.sphere_count(), rig.unresolved), (3, 1));
        let mut locals = rig.set.skeleton.bind_local.clone();
        locals[1].1 = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let world = Mat4::from_rotation_translation(
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            Vec3::new(100.0, 0.0, 0.0),
        );
        let sp = rig.spheres_at(&locals, world);
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-3);
        // spine (0, 50, 0) -> world (100, 50, 0); head: the bent spine turns its 30 units up into +z (model
        // (0, 50, 30)), the yaw turns +z into +x: world (130, 50, 0); the socket 5 further along the head's own +z,
        // which the bend turned into -y: world (130, 45, 0)
        let by = |n: u8| sp.iter().find(|s| s.node == n).copied().expect("sphere");
        assert!(close(by(1).centre, [100.0, 50.0, 0.0]), "{:?}", by(1));
        assert!(close(by(2).centre, [130.0, 50.0, 0.0]), "{:?}", by(2));
        assert!(close(by(4).centre, [130.0, 45.0, 0.0]), "{:?}", by(4));
        // from -x the spine comes first, from +x the head; along z through the head the head
        assert_eq!(pick_node(&sp, [0.0, 50.0, 0.0], [600.0, 50.0, 0.0]), 1);
        assert_eq!(pick_node(&sp, [600.0, 50.0, 0.0], [0.0, 50.0, 0.0]), 2);
        assert_eq!(
            pick_node(&sp, [130.0, 50.0, -500.0], [130.0, 50.0, 500.0]),
            2
        );
        // from below through the socket and the head: the socket sphere's tangent is nearer
        assert_eq!(pick_node(&sp, [130.0, -100.0, 0.0], [130.0, 300.0, 0.0]), 4);
        // 6 units beside the head (and above the socket): nothing
        let from = [136.0, 50.0, -500.0];
        let to = [136.0, 50.0, 500.0];
        assert!(!crosses_any(&sp, from, to));
        assert_eq!(pick_node(&sp, from, to), NO_NODE);
    }
}
