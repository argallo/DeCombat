//! Skeleton and animation playback for the model passes: bind pose, clip sampling with key interpolation, the
//! three-tracker blend through the model's blend sets, and the bone (skin) matrices.
//!
//! Format facts come from `FMT_LTB.md` through `dc-model`: nodes carry model-space bind transforms (§7), clips
//! carry one track per node in node order with keys at the marker times (§10.2, §10.3), blend sets are per-node
//! weights "used when blending clips" (§8), and a model borrows the clips of its child models (§9). What the spec
//! leaves open (§13: interpolation, how blend sets are applied) is settled here as DeCombat readings, each marked,
//! and staged as RENDER_2009 addendum items.

use dc_model::{Clip, Model, PosKeys, RotKeys, Track};
use glam::{Mat3, Mat4, Quat, Vec3};

/// Component order and sense of a stored rotation key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuatOrder {
    /// `(x, y, z, w)` as stored.
    Xyzw,
    /// `(x, y, z, w)`, inverse rotation.
    XyzwConjugate,
    /// `(w, x, y, z)` as stored.
    Wxyz,
    /// `(w, x, y, z)`, inverse rotation.
    WxyzConjugate,
}

/// The convention the renderer uses for rotation keys.
///
/// OBSERVED over the ROM (the `model_census` example): frame 0 of the clips matches the bind pose of FMT_LTB.md
/// §7 with keys read as `(x, y, z, w)` (see the census figures in the render-models report); the other three
/// readings do not.
// OBSERVED: dc-render example model_census over every `.ltb` of the ROM (2026-10-25)
pub const KEY_ORDER: QuatOrder = QuatOrder::Xyzw;

/// A stored rotation key as a unit quaternion (normalised; a zero key gives the identity).
pub fn quat_from_key(k: [f32; 4], order: QuatOrder) -> Quat {
    let q = match order {
        QuatOrder::Xyzw => Quat::from_xyzw(k[0], k[1], k[2], k[3]),
        QuatOrder::XyzwConjugate => Quat::from_xyzw(-k[0], -k[1], -k[2], k[3]),
        QuatOrder::Wxyz => Quat::from_xyzw(k[1], k[2], k[3], k[0]),
        QuatOrder::WxyzConjugate => Quat::from_xyzw(-k[1], -k[2], -k[3], k[0]),
    };
    let l = q.length();
    if l.is_finite() && l > 1e-6 {
        q / l
    } else {
        Quat::IDENTITY
    }
}

/// A row-major 4x4 of FMT_LTB.md §7 (translation in the last column, column vectors) as a glam matrix.
pub fn mat_of_rows(m: &[f32; 16]) -> Mat4 {
    Mat4::from_cols_array(m).transpose()
}

/// Translation and rotation of a rigid row-major transform (the scale of the upper 3x3 is dropped).
pub fn decompose_rigid(m: &[f32; 16]) -> (Vec3, Quat) {
    let g = mat_of_rows(m);
    let t = g.w_axis.truncate();
    let r = Mat3::from_mat4(g);
    let cols = [r.x_axis, r.y_axis, r.z_axis].map(|c| {
        let l = c.length();
        if l > 1e-6 {
            c / l
        } else {
            c
        }
    });
    let q = Quat::from_mat3(&Mat3::from_cols(cols[0], cols[1], cols[2]));
    (t, q.normalize())
}

/// Length of a clip in milliseconds: the time of its last marker (the key times, FMT_LTB.md §10.2).
// SPEC: FMT_LTB.md §10.2 (marker times are the clip's key times)
pub fn clip_length_ms(clip: &Clip) -> u32 {
    clip.markers.iter().map(|m| m.time_ms).max().unwrap_or(0)
}

/// `(segment start key, fraction)` for time `t` (ms) over `n` keys at the clip's marker times. Times before the
/// first key or after the last clamp to the ends; equal key times give fraction 0.
fn segment(clip: &Clip, n: usize, t: f32) -> (usize, f32) {
    if n <= 1 {
        return (0, 0.0);
    }
    let times: Vec<f32> = clip
        .markers
        .iter()
        .take(n)
        .map(|m| m.time_ms as f32)
        .collect();
    let n = times.len();
    if n <= 1 || t <= times[0] {
        return (0, 0.0);
    }
    for i in 0..n - 1 {
        let (a, b) = (times[i], times[i + 1]);
        if t < b {
            let span = b - a;
            let f = if span > 0.0 { (t - a) / span } else { 0.0 };
            return (i, f.clamp(0.0, 1.0));
        }
    }
    (n - 1, 0.0)
}

/// Position channel at time `t` (ms). `None` when the channel is absent ("none", FMT_LTB.md §10.3 count 0).
///
/// DeCombat reading (FMT_LTB.md §13 leaves interpolation open): linear between the two neighbouring keys.
// SPEC: FMT_LTB.md §10.3 (count 0 = none, 1 = constant, else one key per marker)
pub fn pos_channel_at(keys: &PosKeys, clip: &Clip, t: f32) -> Option<Vec3> {
    let v = keys.decode();
    match v.len() {
        0 => None,
        1 => Some(Vec3::from(v[0])),
        n => {
            let (i, f) = segment(clip, n, t);
            let a = Vec3::from(v[i]);
            let b = Vec3::from(*v.get(i + 1).unwrap_or(&v[i]));
            Some(a.lerp(b, f))
        }
    }
}

/// Rotation channel at time `t` (ms), `None` when absent.
///
/// DeCombat reading: spherical interpolation along the shorter arc between the neighbouring keys.
pub fn rot_channel_at(keys: &RotKeys, clip: &Clip, t: f32, order: QuatOrder) -> Option<Quat> {
    let v = keys.decode();
    match v.len() {
        0 => None,
        1 => Some(quat_from_key(v[0], order)),
        n => {
            let (i, f) = segment(clip, n, t);
            let a = quat_from_key(v[i], order);
            let b = quat_from_key(*v.get(i + 1).unwrap_or(&v[i]), order);
            Some(a.slerp(b, f).normalize())
        }
    }
}

/// Both channels of a track at time `t` (ms). Morph tracks carry no transform (`(None, None)`).
pub fn channels_at(
    track: &Track,
    clip: &Clip,
    t: f32,
    order: QuatOrder,
) -> (Option<Vec3>, Option<Quat>) {
    match track {
        Track::Morph { .. } => (None, None),
        Track::Keys { pos, rot } => (
            pos_channel_at(pos, clip, t),
            rot_channel_at(rot, clip, t, order),
        ),
    }
}

/// Both channels when both are present.
pub fn local_at(track: &Track, clip: &Clip, t: f32, order: QuatOrder) -> Option<(Vec3, Quat)> {
    match channels_at(track, clip, t, order) {
        (Some(p), Some(q)) => Some((p, q)),
        _ => None,
    }
}

/// The node tree of one model with its bind pose.
#[derive(Debug, Clone)]
pub struct Skeleton {
    pub parents: Vec<Option<usize>>,
    /// Upper-cased node names (for matching the nodes of child models).
    pub names: Vec<Vec<u8>>,
    /// Model-space bind transforms (FMT_LTB.md §7 `model_xform`).
    pub bind_global: Vec<Mat4>,
    pub inverse_bind: Vec<Mat4>,
    /// Bind pose as parent-relative translation + rotation.
    pub bind_local: Vec<(Vec3, Quat)>,
}

/// One node's local transform.
pub type Local = (Vec3, Quat);

impl Skeleton {
    // SPEC: FMT_LTB.md §7 (model-space bind transforms; local = parent_inverse * model_xform; depth-first order,
    // so a parent always precedes its children)
    pub fn of(m: &Model) -> Skeleton {
        let bind_global: Vec<Mat4> = m
            .nodes
            .iter()
            .map(|n| mat_of_rows(&n.model_xform))
            .collect();
        let inverse_bind = bind_global
            .iter()
            .map(|g| {
                let inv = g.inverse();
                if inv.is_finite() {
                    inv
                } else {
                    Mat4::IDENTITY
                }
            })
            .collect();
        let bind_local = m.local_xforms().iter().map(decompose_rigid).collect();
        Skeleton {
            parents: m.nodes.iter().map(|n| n.parent).collect(),
            names: m
                .nodes
                .iter()
                .map(|n| n.name.as_bytes().to_ascii_uppercase())
                .collect(),
            bind_global,
            inverse_bind,
            bind_local,
        }
    }

    pub fn len(&self) -> usize {
        self.parents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
    }

    /// Model-space node transforms of a pose (parents first: the node list is depth-first).
    pub fn globals(&self, locals: &[Local]) -> Vec<Mat4> {
        let mut out: Vec<Mat4> = Vec::with_capacity(self.len());
        self.globals_into(locals, &mut out);
        out
    }

    /// [`Skeleton::globals`] into a reused vector (cleared first).
    pub fn globals_into(&self, locals: &[Local], out: &mut Vec<Mat4>) {
        out.clear();
        for i in 0..self.len() {
            let (p, q) = locals.get(i).copied().unwrap_or(self.bind_local[i]);
            let l = Mat4::from_rotation_translation(q, p);
            let g = match self.parents[i] {
                Some(pi) if pi < out.len() => out[pi] * l,
                _ => l,
            };
            out.push(g);
        }
    }

    /// Bone matrices: `global(pose) * inverse(global(bind))`, mapping bind-pose model-space vertices to the
    /// posed model space (vertices are stored in the bind pose's model space: OBSERVED, see `model_census`).
    pub fn skin_matrices(&self, locals: &[Local]) -> Vec<Mat4> {
        self.globals(locals)
            .iter()
            .zip(&self.inverse_bind)
            .map(|(g, ib)| *g * *ib)
            .collect()
    }

    /// The model-space node transforms of a model that follows `leader` (posed at `leader_globals`), with `map` from
    /// [`Skeleton::map_onto`]: a matched node takes the leader's node; with `leader_space` it is skinned with the
    /// leader's own skin matrix (leader pose x leader inverse bind x our bind), so our vertices are read in the
    /// leader's model space whatever bind our file gives the node. An unmatched node keeps our bind.
    pub fn follow_globals(
        &self,
        leader: &Skeleton,
        map: &[Option<usize>],
        leader_globals: &[Mat4],
        leader_space: bool,
        out: &mut Vec<Mat4>,
    ) {
        out.clear();
        out.extend(map.iter().enumerate().map(|(n, m)| {
            match m.and_then(|m| leader_globals.get(m).map(|g| (m, g))) {
                Some((m, g)) if leader_space => *g * leader.inverse_bind[m] * self.bind_global[n],
                Some((_, g)) => *g,
                None => self.bind_global[n],
            }
        }));
    }

    /// For each of our nodes, the node of `other` with the same name (case-insensitive), else the node at the
    /// same position when both trees have the same size.
    pub fn map_onto(&self, other: &Skeleton) -> Vec<Option<usize>> {
        let same_size = self.len() == other.len();
        self.names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                other
                    .names
                    .iter()
                    .position(|o| o == n)
                    .or(same_size.then_some(i))
            })
            .collect()
    }
}

/// One playing clip: the combined clip index (the body's own clips first, then each child model's, the order
/// the server's animation indices count), the time in ms, and the weight set (blend set) it is masked with.
// SPEC: SESSION_B_S2C_addendum_dserver3.md S3-5 (main tracker, tracker 1 / weight set 0, tracker 2 / weight set
// 1; indices count the composed model's clips: the body's own clips first, then the child model's)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tracker {
    pub clip: usize,
    pub time_ms: f32,
    /// `None` for the main tracker (no weight set: every node at weight 1).
    pub blend_set: Option<usize>,
}

impl Tracker {
    pub fn main(clip: usize, time_ms: f32) -> Tracker {
        Tracker {
            clip,
            time_ms,
            blend_set: None,
        }
    }
}

/// A body model plus the models it borrows clips from.
#[derive(Debug, Clone)]
pub struct AnimSet {
    pub skeleton: Skeleton,
    /// Per source model (0 = the body itself, then its children in file order): body node -> source node.
    node_maps: Vec<Vec<Option<usize>>>,
    /// Combined clip list: (source model, clip index in that model).
    clips: Vec<(usize, usize)>,
    /// Blend-set weights of the body (FMT_LTB.md §8), one per body node.
    blend_sets: Vec<Vec<f32>>,
    /// Shared between the copies of a set (instances of one model): cloning a set is cheap.
    sources: std::sync::Arc<Vec<Model>>,
    /// Per combined clip: the clip's offset from the per-child clip overrides (FMT_LTB.md §11.2), zero without one.
    clip_offsets: Vec<Vec3>,
}

impl AnimSet {
    /// `children` are the child models of `body` in its file order (FMT_LTB.md §9). Clip-less parts of a child
    /// (its pieces) are not used.
    // SPEC: FMT_LTB.md §9 (child slots lend their clips), §10 (clips), §8 (blend sets: n = node count)
    pub fn new(body: &Model, children: &[Model]) -> AnimSet {
        let skeleton = Skeleton::of(body);
        let mut sources: Vec<Model> = Vec::with_capacity(children.len() + 1);
        // keep only what playback needs from each source: nodes and clips
        let strip = |m: &Model| Model {
            nodes: m.nodes.clone(),
            clips: m.clips.clone(),
            ..Model::default()
        };
        sources.push(strip(body));
        sources.extend(children.iter().map(strip));
        let node_maps = sources
            .iter()
            .map(|s| skeleton.map_onto(&Skeleton::of(s)))
            .collect();
        let clips: Vec<(usize, usize)> = sources
            .iter()
            .enumerate()
            .flat_map(|(si, s)| (0..s.clips.len()).map(move |ci| (si, ci)))
            .collect();
        let blend_sets = body.blend_sets.iter().map(|b| b.weights.clone()).collect();
        // the offset of a clip: the body's override for the clip's child slot, else the source model's own
        // (slot 0) override, matched by clip name
        // SPEC: FMT_LTB.md §11.2 (per child slot: clip name, extent, offset -> clip info +0x10)
        // EVIDENCE: engine+0x6e0d0 (each override is looked up by name in that slot's clip list and written into the
        // clip's info record, extent at +0x04, offset at +0x10)
        let find = |list: Option<&Vec<dc_model::ClipOverride>>, name: &[u8]| -> Option<Vec3> {
            list?
                .iter()
                .find(|o| o.clip_name.as_bytes().eq_ignore_ascii_case(name))
                .map(|o| Vec3::from(o.offset))
        };
        let clip_offsets = clips
            .iter()
            .map(|&(si, ci)| {
                let name = sources[si].clips[ci].name.as_bytes();
                let own = if si == 0 {
                    None
                } else {
                    children
                        .get(si - 1)
                        .and_then(|c| find(c.clip_overrides.first(), name))
                };
                find(body.clip_overrides.get(si), name)
                    .or(own)
                    .filter(|v| v.is_finite())
                    .unwrap_or(Vec3::ZERO)
            })
            .collect();
        AnimSet {
            skeleton,
            node_maps,
            clips,
            blend_sets,
            sources: std::sync::Arc::new(sources),
            clip_offsets,
        }
    }

    /// The offset of the clip at a combined index (FMT_LTB.md §11.2; zero when the files give none).
    pub fn clip_offset(&self, index: usize) -> Vec3 {
        self.clip_offsets.get(index).copied().unwrap_or(Vec3::ZERO)
    }

    /// Number of clips in the combined list.
    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }

    /// The clip at a combined index.
    pub fn clip(&self, index: usize) -> Option<&Clip> {
        let (s, c) = *self.clips.get(index)?;
        self.sources.get(s)?.clips.get(c)
    }

    /// Length (ms) of the clip at a combined index.
    pub fn clip_length_ms(&self, index: usize) -> Option<u32> {
        self.clip(index).map(clip_length_ms)
    }

    /// First combined index whose clip name satisfies `pred`.
    pub fn find_clip(&self, pred: impl Fn(&[u8]) -> bool) -> Option<usize> {
        (0..self.clips.len()).find(|&i| self.clip(i).is_some_and(|c| pred(c.name.as_bytes())))
    }

    pub fn blend_set_count(&self) -> usize {
        self.blend_sets.len()
    }

    /// The body's local pose for a list of trackers, applied in order onto the bind pose.
    ///
    /// DeCombat reading (FMT_LTB.md §8 says only that blend sets are per-node weights used when blending clips):
    /// each tracker's sampled transform replaces the pose built so far by the node's weight `w` of the tracker's
    /// weight set (linear for positions, spherical for rotations: `pose = mix(pose, clip, w)`); the main tracker
    /// has weight 1 everywhere; a weight set the body lacks counts as weight 1; nodes the clip's model lacks and
    /// absent channels keep the pose built so far. Clip time wraps at the clip length (the server streams
    /// growing tracker times, CAPTURE_tdm2 §1.6).
    pub fn pose(&self, trackers: &[Tracker]) -> Vec<Local> {
        let mut pose = self.skeleton.bind_local.clone();
        for t in trackers {
            let Some(&(src, ci)) = self.clips.get(t.clip) else {
                continue;
            };
            let model = &self.sources[src];
            let clip = &model.clips[ci];
            let len = clip_length_ms(clip) as f32;
            let time = if len > 0.0 && t.time_ms.is_finite() {
                t.time_ms.rem_euclid(len)
            } else {
                0.0
            };
            let weights = t.blend_set.and_then(|w| self.blend_sets.get(w));
            for (ni, slot) in pose.iter_mut().enumerate() {
                let w = weights
                    .and_then(|ws| ws.get(ni).copied())
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0);
                if w <= 0.0 {
                    continue;
                }
                let Some(Some(sn)) = self.node_maps[src].get(ni) else {
                    continue;
                };
                let Some(track) = clip.tracks.get(*sn) else {
                    continue;
                };
                let (p, q) = channels_at(track, clip, time, KEY_ORDER);
                if let Some(p) = p {
                    slot.0 = slot.0.lerp(p, w);
                }
                if let Some(q) = q {
                    slot.1 = slot.1.slerp(q, w).normalize();
                }
            }
        }
        pose
    }

    /// Bone matrices of the pose of `trackers`.
    pub fn skin_matrices(&self, trackers: &[Tracker]) -> Vec<Mat4> {
        self.skeleton.skin_matrices(&self.pose(trackers))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_model::{BlendSet, Marker, Node, RawStr, IDENTITY};

    fn node(name: &str, x: f32, parent: Option<usize>, children: Vec<usize>) -> Node {
        let mut m = IDENTITY;
        m[3] = x;
        Node {
            name: RawStr::from(name),
            index: 0,
            flags: 0,
            model_xform: m,
            parent,
            children,
        }
    }

    fn clip(name: &str, times: &[u32], tracks: Vec<Track>) -> Clip {
        Clip {
            extent: [1.0; 3],
            name: RawStr::from(name),
            codec: 0,
            blend_ms: 0,
            markers: times
                .iter()
                .map(|&t| Marker {
                    time_ms: t,
                    command: RawStr::default(),
                })
                .collect(),
            tracks,
        }
    }

    fn keys(p: &[[f32; 3]], r: &[[f32; 4]]) -> Track {
        Track::Keys {
            pos: PosKeys::F32(p.to_vec()),
            rot: RotKeys::F32(r.to_vec()),
        }
    }

    const ID: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn two_nodes() -> Model {
        Model {
            nodes: vec![
                node("root", 0.0, None, vec![1]),
                node("arm", 10.0, Some(0), vec![]),
            ],
            ..Model::default()
        }
    }

    #[test]
    fn bind_pose_gives_identity_bones() {
        let m = two_nodes();
        let s = Skeleton::of(&m);
        let bones = s.skin_matrices(&s.bind_local);
        for b in bones {
            assert!(b.abs_diff_eq(Mat4::IDENTITY, 1e-5), "{b}");
        }
        assert_eq!(s.bind_local[1].0, Vec3::new(10.0, 0.0, 0.0));
    }

    #[test]
    fn keys_interpolate_and_wrap() {
        let mut m = two_nodes();
        let half_turn = Quat::from_rotation_y(std::f32::consts::PI);
        let q = [half_turn.x, half_turn.y, half_turn.z, half_turn.w];
        m.clips.push(clip(
            "c",
            &[0, 100],
            vec![
                keys(&[[0.0; 3], [0.0, 20.0, 0.0]], &[ID, ID]),
                keys(&[[10.0, 0.0, 0.0], [10.0, 0.0, 0.0]], &[ID, q]),
            ],
        ));
        let set = AnimSet::new(&m, &[]);
        assert_eq!(set.clip_length_ms(0), Some(100));
        let p = set.pose(&[Tracker::main(0, 50.0)]);
        assert!((p[0].0.y - 10.0).abs() < 1e-4);
        // a quarter turn halfway
        let a = p[1].1.angle_between(Quat::IDENTITY);
        assert!((a - std::f32::consts::FRAC_PI_2).abs() < 1e-3, "{a}");
        // wraps: 150 ms = 50 ms
        let w = set.pose(&[Tracker::main(0, 150.0)]);
        assert!((w[0].0.y - 10.0).abs() < 1e-4);
        // the bone of the arm at 50 ms: world point of a bind vertex at the arm (10, 0, 0) stays at (10, 10, 0)
        let bones = set.skin_matrices(&[Tracker::main(0, 50.0)]);
        let v = bones[1].transform_point3(Vec3::new(10.0, 0.0, 0.0));
        assert!(v.abs_diff_eq(Vec3::new(10.0, 10.0, 0.0), 1e-3), "{v}");
    }

    #[test]
    fn weight_sets_mask_trackers_and_children_lend_clips() {
        let mut body = two_nodes();
        body.blend_sets = vec![
            BlendSet {
                name: RawStr::from("upper"),
                weights: vec![0.0, 1.0],
            },
            BlendSet {
                name: RawStr::from("lower"),
                weights: vec![1.0, 0.0],
            },
        ];
        body.clips.push(clip(
            "base",
            &[0],
            vec![keys(&[[0.0; 3]], &[ID]), keys(&[[10.0, 0.0, 0.0]], &[ID])],
        ));
        // the child lists its nodes in the other order: matched by name
        let child = Model {
            nodes: vec![
                node("ARM", 10.0, None, vec![1]),
                node("Root", 0.0, Some(0), vec![]),
            ],
            clips: vec![
                clip(
                    "up",
                    &[0],
                    vec![
                        keys(&[[10.0, 5.0, 0.0]], &[ID]),
                        keys(&[[0.0, 5.0, 0.0]], &[ID]),
                    ],
                ),
                clip(
                    "down",
                    &[0],
                    vec![
                        keys(&[[10.0, 7.0, 0.0]], &[ID]),
                        keys(&[[0.0, 7.0, 0.0]], &[ID]),
                    ],
                ),
            ],
            ..Model::default()
        };
        let set = AnimSet::new(&body, &[child]);
        assert_eq!(set.clip_count(), 3);
        assert_eq!(set.find_clip(|n| n == b"down"), Some(2));
        let p = set.pose(&[
            Tracker::main(0, 0.0),
            Tracker {
                clip: 1,
                time_ms: 0.0,
                blend_set: Some(0),
            },
            Tracker {
                clip: 2,
                time_ms: 0.0,
                blend_set: Some(1),
            },
        ]);
        // root from the "down" clip (weight set 1), arm from the "up" clip (weight set 0)
        assert!((p[0].0.y - 7.0).abs() < 1e-5, "{:?}", p[0]);
        assert!((p[1].0.y - 5.0).abs() < 1e-5, "{:?}", p[1]);
    }

    #[test]
    fn key_orders() {
        let q = Quat::from_rotation_z(0.5);
        let k = [q.x, q.y, q.z, q.w];
        assert!(quat_from_key(k, QuatOrder::Xyzw).abs_diff_eq(q, 1e-6));
        assert!(quat_from_key(k, QuatOrder::XyzwConjugate).abs_diff_eq(q.conjugate(), 1e-6));
        let k2 = [q.w, q.x, q.y, q.z];
        assert!(quat_from_key(k2, QuatOrder::Wxyz).abs_diff_eq(q, 1e-6));
        assert_eq!(quat_from_key([0.0; 4], QuatOrder::Xyzw), Quat::IDENTITY);
    }
}
