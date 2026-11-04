//! Server-side visibility culling for vanilla servers (DeCombat rules VIS-1..14, proposed; design
//! `docs/VISIBILITY_CULLING_DESIGN.md`). The 2009 server did not cull by occlusion: its stream priority has no
//! line-of-sight term (UDP_SESSION_2009_B.md §4.3), so this whole module is a DeCombat addition that keeps to record
//! forms the 2009 client already accepted.
//!
//! Per tick, for every in-world viewer and every enemy character that is not a corpse, the pair is visible when
//! - a segment from the viewer's eye to one of up to 13 sample points of the enemy's box is clear in the occluder
//!   set (one-sided triangle BVH over the world's opaque collision polygons, [`Occluders`]), or
//! - such a segment was clear within the exposure window, or
//! - the two are within the always-visible radius (footsteps and close range), or
//! - the enemy made a sound within the event window (its shots, weapon sounds and relayed jumps are relayed to
//!   everyone, unculled; the enemy is shown first), or
//! - the pair was not tested recently (budget exhausted: fail-open).
//!
//! A pair that turns hidden gets one reliable change record that snaps the viewer's copy of the character to the
//! park point (`CF_POS | CF_POS_SNAP`, velocity 0) and no further stream entries; the same record again once after
//! the refresh time; a pair that turns visible again gets a snap back to the true position before its stream
//! entry of the same tick. Teammates, corpses, projectiles and world objects are never culled; the kill feed, the
//! roster and the scores are untouched.
//!
//! Pure and deterministic apart from the cost measurement (wall-clock timing for the statistics only).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use crate::game::{Game, Life};
use crate::modelobj::CF_POS_SNAP;
use crate::objects::{write_record_head, PosBox, CF_POS};
use crate::stream::pos48_encode;
use crate::world::{BrushInfo, BLEND_NONE};
use dc_net_engine::time::since;
use dc_net_engine::Millis;
use dc_wire::udp::{BitWriter, Vec72};

type V = [f32; 3];

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn mul(a: V, s: f32) -> V {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn len(a: V) -> f32 {
    dot(a, a).sqrt()
}

fn finite(a: V) -> bool {
    a.iter().all(|c| c.is_finite())
}

// ------------------------------------------------------------------------------------------- occluders

/// One occluder triangle: corner, two edges and the front normal of its polygon's plane.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tri {
    v0: V,
    e1: V,
    e2: V,
    n: V,
}

/// BVH node: bounds; a leaf holds `count` triangles from `first`, an inner node (`count` 0) has its two children at
/// `first` and `first + 1`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Node {
    lo: V,
    hi: V,
    first: u32,
    count: u32,
}

/// Triangles per BVH leaf.
const LEAF_TRIS: usize = 4;
/// Node bounds are padded by this (units) against rounding at the slab test.
const NODE_PAD: f32 = 0.01;
/// A hit closer than this (units) to either end of a segment does not block it (a sample point on a surface).
const END_SLACK: f32 = 0.05;
/// Barycentric tolerance: a segment through a shared edge of two triangles of one polygon still hits.
const BARY_EPS: f32 = 1e-5;

/// The occluder set of a world: a bounding-volume hierarchy (median split; a public algorithm) over the triangles
/// of the opaque collision polygons, queried with an any-hit segment test that counts front faces only.
// DC-DEFAULT: VIS-3 (proposed) occluder set; docs/VISIBILITY_CULLING_DESIGN.md §2.2, §3.2 (algorithm C)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Occluders {
    tris: Vec<Tri>,
    nodes: Vec<Node>,
    /// Models and polygons that went in (statistics).
    pub models: usize,
    pub polygons: usize,
}

/// One input polygon: front normal and corners (a convex fan).
#[derive(Debug, Clone, PartialEq)]
pub struct OccluderPoly {
    pub normal: V,
    pub corners: Vec<V>,
}

impl Occluders {
    /// From caller polygons (tests, synthetic worlds).
    pub fn from_polys(polys: &[OccluderPoly]) -> Occluders {
        let mut tris = Vec::new();
        for p in polys {
            push_fan(&mut tris, p.normal, &p.corners);
        }
        let mut o = Occluders::build(tris);
        o.polygons = polys.len();
        o.models = usize::from(!polys.is_empty());
        o
    }

    /// The VIS-3 selection from a parsed world: the level model (the model no object names) and `PlainWorldModel`
    /// brushes that are visible and opaque; of those only the polygons with surface flag bit 0 (collision). Not
    /// taken: blocker polygons (player clips), volumes, ladders, translucent / additive / invisible models,
    /// breakables, unlabelled models (doors and other movers the server does not simulate) and sky models.
    // SPEC: FMT_DAT85.md §5.2 (surface flags, polygon planes), §6.4 (models linked to objects by name), §9.1
    // (blockers are clipping); MOVEMENT_2009.md §9.8 (surface bit 0 = collision); RENDER_2009.md §5.1 step 6
    // (visible and blend properties of world models)
    // DC-DEFAULT: VIS-3 (proposed)
    pub fn from_world(w: &dc_world::World<'_>, brushes: &[BrushInfo]) -> Occluders {
        let mut tris = Vec::new();
        let (mut models, mut polygons) = (0, 0);
        for m in &w.models {
            let take = match w.object_named(m.name) {
                None => true,
                Some(o) => {
                    o.role == Some(dc_world::ObjectRole::PlainWorldModel)
                        && brushes
                            .iter()
                            .find(|b| o.name() == Some(b.name.as_slice()))
                            .is_some_and(|b| b.visible && b.blend == BLEND_NONE)
                }
            };
            if !take {
                continue;
            }
            models += 1;
            for p in &m.polygons {
                let solid = m
                    .surfaces
                    .get(p.surface as usize)
                    .is_some_and(|s| s.flags & SURFACE_SOLID != 0);
                let Some(pl) = m.planes.get(p.plane as usize) else {
                    continue;
                };
                if !solid {
                    continue;
                }
                let corners: Vec<V> = m
                    .polygon_points(p)
                    .iter()
                    .filter_map(|&i| m.points.get(i as usize).copied())
                    .collect();
                if corners.len() >= 3 {
                    polygons += 1;
                    push_fan(&mut tris, pl.normal, &corners);
                }
            }
        }
        let mut o = Occluders::build(tris);
        o.models = models;
        o.polygons = polygons;
        o
    }

    pub fn triangles(&self) -> usize {
        self.tris.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    fn build(mut tris: Vec<Tri>) -> Occluders {
        if tris.is_empty() {
            return Occluders::default();
        }
        let cent: Vec<V> = tris
            .iter()
            .map(|t| add(t.v0, mul(add(t.e1, t.e2), 1.0 / 3.0)))
            .collect();
        let mut idx: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = vec![Node {
            lo: [0.0; 3],
            hi: [0.0; 3],
            first: 0,
            count: 0,
        }];
        let mut work = vec![(0usize, 0usize, tris.len())];
        while let Some((ni, s, e)) = work.pop() {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            let (mut clo, mut chi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for &i in &idx[s..e] {
                let t = &tris[i as usize];
                for p in [t.v0, add(t.v0, t.e1), add(t.v0, t.e2)] {
                    for k in 0..3 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                    }
                }
                let c = cent[i as usize];
                for k in 0..3 {
                    clo[k] = clo[k].min(c[k]);
                    chi[k] = chi[k].max(c[k]);
                }
            }
            for k in 0..3 {
                lo[k] -= NODE_PAD;
                hi[k] += NODE_PAD;
            }
            let ext = sub(chi, clo);
            let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] {
                0
            } else if ext[1] >= ext[2] {
                1
            } else {
                2
            };
            if e - s <= LEAF_TRIS || ext[axis].is_nan() || ext[axis] <= 0.0 {
                nodes[ni] = Node {
                    lo,
                    hi,
                    first: s as u32,
                    count: (e - s) as u32,
                };
                continue;
            }
            let mid = s + (e - s) / 2;
            idx[s..e].select_nth_unstable_by(mid - s, |a, b| {
                cent[*a as usize][axis].total_cmp(&cent[*b as usize][axis])
            });
            let l = nodes.len();
            nodes.push(nodes[ni]);
            nodes.push(nodes[ni]);
            nodes[ni] = Node {
                lo,
                hi,
                first: l as u32,
                count: 0,
            };
            work.push((l, s, mid));
            work.push((l + 1, mid, e));
        }
        let ordered: Vec<Tri> = idx.iter().map(|&i| tris[i as usize]).collect();
        tris = ordered;
        Occluders {
            tris,
            nodes,
            models: 0,
            polygons: 0,
        }
    }

    /// True when a front face of an occluder lies strictly between `a` and `b` (any hit).
    pub fn blocked(&self, a: V, b: V) -> bool {
        if self.nodes.is_empty() || !finite(a) || !finite(b) {
            return false;
        }
        let d = sub(b, a);
        let l = len(d);
        if l < 2.0 * END_SLACK {
            return false;
        }
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let t_lo = END_SLACK / l;
        let t_hi = 1.0 - t_lo;
        let mut stack = [0u32; 64];
        let mut sp = 1usize;
        while sp > 0 {
            sp -= 1;
            let n = &self.nodes[stack[sp] as usize];
            if !slab(n.lo, n.hi, a, inv) {
                continue;
            }
            if n.count > 0 {
                let r = n.first as usize..(n.first + n.count) as usize;
                if self.tris[r].iter().any(|t| tri_hit(t, a, d, t_lo, t_hi)) {
                    return true;
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = n.first;
                stack[sp + 1] = n.first + 1;
                sp += 2;
            } else {
                // deeper than the fixed stack (never for a median split under 2^60 triangles): count it clear
                continue;
            }
        }
        false
    }
}

/// Surface flag bit 0: a collision polygon.
// SPEC: MOVEMENT_2009.md §9.8 (surface bit 0); FMT_DAT85.md §5.2
pub const SURFACE_SOLID: u32 = 0x1;

fn push_fan(out: &mut Vec<Tri>, normal: V, corners: &[V]) {
    let n = if finite(normal) && len(normal) > 1e-6 {
        mul(normal, 1.0 / len(normal))
    } else if corners.len() >= 3 {
        let c = cross(sub(corners[1], corners[0]), sub(corners[2], corners[0]));
        if len(c) <= 1e-12 {
            return;
        }
        mul(c, 1.0 / len(c))
    } else {
        return;
    };
    for i in 1..corners.len().saturating_sub(1) {
        let (p0, p1, p2) = (corners[0], corners[i], corners[i + 1]);
        if !(finite(p0) && finite(p1) && finite(p2)) {
            continue;
        }
        let e1 = sub(p1, p0);
        let e2 = sub(p2, p0);
        if len(cross(e1, e2)) <= 1e-9 {
            continue;
        }
        out.push(Tri { v0: p0, e1, e2, n });
    }
}

/// Segment `a + t d`, `t` in [0, 1], against a box (slab test).
fn slab(lo: V, hi: V, a: V, inv: V) -> bool {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for k in 0..3 {
        let p = (lo[k] - a[k]) * inv[k];
        let q = (hi[k] - a[k]) * inv[k];
        // f32::min / max ignore a NaN (0 * inf on a face of the box): that axis then does not cut the range
        t0 = t0.max(p.min(q));
        t1 = t1.min(p.max(q));
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// Moller-Trumbore, front faces only: the segment must run against the polygon's normal.
fn tri_hit(t: &Tri, a: V, d: V, t_lo: f32, t_hi: f32) -> bool {
    if dot(t.n, d) >= 0.0 {
        return false;
    }
    let p = cross(d, t.e2);
    let det = dot(t.e1, p);
    if det.abs() < 1e-12 {
        return false;
    }
    let inv = 1.0 / det;
    let s = sub(a, t.v0);
    let u = dot(s, p) * inv;
    if !(-BARY_EPS..=1.0 + BARY_EPS).contains(&u) {
        return false;
    }
    let q = cross(s, t.e1);
    let v = dot(d, q) * inv;
    if v < -BARY_EPS || u + v > 1.0 + BARY_EPS {
        return false;
    }
    let tt = dot(t.e2, q) * inv;
    tt > t_lo && tt < t_hi
}

// ------------------------------------------------------------------------------------------- tuning

/// The VIS parameters (all DeCombat defaults; none is ROM data or a reference value).
#[derive(Debug, Clone, PartialEq)]
pub struct VisTuning {
    /// `-visibility on|off`, `vis.enabled`.
    // DC-DEFAULT: VIS-1 (proposed) on for every vanilla server
    pub enabled: bool,
    /// Exposure window: a pair stays visible this long after its last clear test (`-visibility-window`).
    // DC-DEFAULT: VIS-7 (proposed) 1000 ms
    pub exposure_ms: u32,
    /// Always-visible radius between the two bodies (`-visibility-radius`): footsteps and close range.
    // DC-DEFAULT: VIS-12 (proposed) 800 units, here without the speed condition (the "visible" direction)
    pub always_radius: f32,
    /// A sound of the enemy (shot, weapon sound, relayed jump) keeps it visible this long.
    // DC-DEFAULT: VIS-10 (proposed) 500 ms
    pub event_ms: u32,
    /// Eye height above the box centre and the head / low sample heights, as fractions of the half height.
    // DC-DEFAULT: VIS-4 (proposed) 0.75 / 0.9 / -0.6
    pub eye_frac: f32,
    pub head_frac: f32,
    pub low_frac: f32,
    /// Side margin of the box and the largest peek offset.
    // DC-DEFAULT: VIS-6 (proposed) 16 / 64 units; run speed 1500 units/s (the DSV-14 bound)
    pub margin: f32,
    pub peek_max: f32,
    pub run_speed: f32,
    /// Lookahead `rtt + client_delay + 2 x tick + test interval`, clamped.
    // DC-DEFAULT: VIS-5 (proposed) 150 ms (2009 client), 100 .. 500 ms
    pub client_delay_ms: u32,
    pub lookahead_min_ms: u32,
    pub lookahead_max_ms: u32,
    /// Segments per tick, the fail-open age and the re-test interval of hidden pairs.
    // DC-DEFAULT: VIS-8 (proposed) 100 ms fail-open; hidden pairs at about 15 Hz; budget 1000 segments per tick
    // (the design's 400 is below its own 8v8 estimate at 30 Hz, see the report)
    pub max_segments_per_tick: u32,
    pub max_test_age_ms: u32,
    pub hidden_test_ms: u32,
    /// The park refresh comes once `2 x max(L, refresh_floor_ms)` after the hide.
    // DC-DEFAULT: VIS-9 (proposed) 200 ms floor; SPEC: UDP_SESSION_2009_B.md §12.2 item 5 (stale unreliable data
    // is dropped by the sender)
    pub refresh_floor_ms: u32,
}

impl Default for VisTuning {
    fn default() -> Self {
        VisTuning {
            enabled: true,
            exposure_ms: 1_000,
            always_radius: 800.0,
            event_ms: 500,
            eye_frac: 0.75,
            head_frac: 0.9,
            low_frac: -0.6,
            margin: 16.0,
            peek_max: 64.0,
            run_speed: 1_500.0,
            client_delay_ms: 150,
            lookahead_min_ms: 100,
            lookahead_max_ms: 500,
            max_segments_per_tick: 1_000,
            max_test_age_ms: 100,
            hidden_test_ms: 60,
            refresh_floor_ms: 200,
        }
    }
}

impl VisTuning {
    /// Culling off (`-visibility off`).
    pub fn off() -> VisTuning {
        VisTuning {
            enabled: false,
            ..VisTuning::default()
        }
    }
}

// ------------------------------------------------------------------------------------------- actors

/// What the visibility test needs of one player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Actor {
    pub pid: u8,
    pub obj: u16,
    pub team: u8,
    /// Alive (the viewer's own eye counts; otherwise the team's view is used).
    pub alive: bool,
    /// May be culled: alive or not spawned yet (a corpse never is, VIS-2).
    pub cullable: bool,
    pub pos: V,
    pub vel: V,
    /// Half extents of the box.
    pub half: V,
    pub rtt_ms: u32,
    /// Spawn count (a respawn restarts the pair's history).
    pub spawns: u32,
    /// The client's copy carries raw floats in change records (plain objects created with the full-precision flag).
    pub full_precision: bool,
    /// The player that killed this one in its current death (None while alive or killed by the world); VIS-14.
    pub killer: Option<u8>,
}

/// The players of the game layer as visibility actors; `plain_full_precision` = the session creates plain player
/// objects with the full-precision flag.
// DC-DEFAULT: VIS-2 (proposed) corpses (states 2 / 3) are never culled
pub fn actors(game: &Game, plain_full_precision: bool) -> Vec<Actor> {
    game.players()
        .values()
        .map(|p| {
            let corpse = matches!(p.life, Life::Dying { .. } | Life::Waiting { .. });
            Actor {
                pid: p.pid,
                obj: p.obj,
                team: p.team,
                alive: p.life == Life::Alive,
                cullable: !corpse,
                pos: p.pos,
                vel: p.vel,
                half: game.dims_of(p.pid),
                rtt_ms: u32::from(p.link_rtt_ms.or(p.client_ping_ms).unwrap_or(0)),
                spawns: p.spawns,
                full_precision: p.character.is_none() && plain_full_precision,
                killer: if p.life == Life::Alive {
                    None
                } else {
                    p.killed_by
                },
            }
        })
        .collect()
}

/// The FFA mode class: every other player is an enemy.
// SPEC: GAME_MODES_2009.md §5.4 (class 6)
pub const MODE_CLASS_FFA: u8 = 6;

fn enemies(v: &Actor, e: &Actor, ffa: bool) -> bool {
    v.pid != e.pid && (ffa || v.team != e.team)
}

// ------------------------------------------------------------------------------------------- state

/// What a viewer's client must be told about one enemy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Change {
    pub viewer: u8,
    pub enemy: u8,
    pub obj: u16,
    pub kind: ChangeKind,
    pub full_precision: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChangeKind {
    /// Snap to the park point (hide, or the one refresh).
    Park(V),
    /// Snap back to the true position with this velocity (show).
    Show { pos: V, vel: V },
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Pair {
    viewer_obj: u16,
    enemy_obj: u16,
    spawns: u32,
    parked: bool,
    parked_at: Millis,
    refreshed: bool,
    last_clear: Option<Millis>,
    last_test: Option<Millis>,
    event_until: Option<Millis>,
    /// Index of the segment that was clear last time (tried first).
    cached: u8,
    /// The viewer's own verdict this tick.
    own: bool,
}

/// Cost counters (the per-tick time is wall clock, for reports only).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct VisStats {
    pub updates: u64,
    pub nanos: u128,
    pub max_nanos: u128,
    pub segments: u64,
    pub max_segments: u32,
    pub tests: u64,
    pub fail_open: u64,
    pub parks: u64,
    pub refreshes: u64,
    pub shows: u64,
    pub pairs: u64,
}

impl VisStats {
    pub fn mean_us(&self) -> f64 {
        if self.updates == 0 {
            0.0
        } else {
            self.nanos as f64 / self.updates as f64 / 1000.0
        }
    }
}

/// Period of the statistics log line.
pub const REPORT_PERIOD_MS: u32 = 30_000;

/// The culling state of one session.
#[derive(Debug, Clone)]
pub struct Visibility {
    cfg: VisTuning,
    occ: Option<Arc<Occluders>>,
    park: V,
    pairs: BTreeMap<(u8, u8), Pair>,
    pub stats: VisStats,
    last_report: Option<Millis>,
}

impl Visibility {
    /// `spawn_points`: positions the park-point census looks from (the world's player spawn points).
    pub fn new(
        cfg: VisTuning,
        occ: Option<Arc<Occluders>>,
        pbox: &PosBox,
        spawn_points: &[V],
    ) -> Visibility {
        let park = park_point(occ.as_deref(), pbox, spawn_points);
        Visibility {
            cfg,
            occ,
            park,
            pairs: BTreeMap::new(),
            stats: VisStats::default(),
            last_report: None,
        }
    }

    pub fn config(&self) -> &VisTuning {
        &self.cfg
    }

    /// The start-up log line.
    pub fn describe(&self) -> String {
        if !self.cfg.enabled {
            return "visibility culling off (-visibility off / vis.enabled = false)".into();
        }
        let (tris, models) = self
            .occ
            .as_deref()
            .map_or((0, 0), |o| (o.triangles(), o.models));
        format!(
            "visibility culling on: window {} ms, radius {}, {} segments per tick; occluders {} triangles from {}              model(s); park point ({:.0}, {:.0}, {:.0})",
            self.cfg.exposure_ms,
            self.cfg.always_radius,
            self.cfg.max_segments_per_tick,
            tris,
            models,
            self.park[0],
            self.park[1],
            self.park[2]
        )
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub fn park_point(&self) -> V {
        self.park
    }

    pub fn occluders(&self) -> Option<&Occluders> {
        self.occ.as_deref()
    }

    /// The viewer's client has `enemy`'s object parked (its stream entries are withheld).
    pub fn parked(&self, viewer: u8, enemy_obj: u16) -> bool {
        self.pairs
            .range((viewer, 0)..=(viewer, u8::MAX))
            .any(|(_, p)| p.parked && p.enemy_obj == enemy_obj)
    }

    /// The last clear test of the pair (tests).
    pub fn last_clear(&self, viewer: u8, enemy: u8) -> Option<Millis> {
        self.pairs.get(&(viewer, enemy)).and_then(|p| p.last_clear)
    }

    /// A player left or was placed again: its pairs start over.
    pub fn forget(&mut self, pid: u8) {
        self.pairs.retain(|(v, e), _| *v != pid && *e != pid);
    }

    /// Lookahead of a viewer for a pair tested every `retest_ms`.
    // DC-DEFAULT: VIS-5 (proposed)
    fn lookahead_ms(&self, v: &Actor, tick_ms: u32, retest_ms: u32) -> u32 {
        let l = v.rtt_ms + self.cfg.client_delay_ms + 2 * tick_ms + retest_ms;
        l.clamp(self.cfg.lookahead_min_ms, self.cfg.lookahead_max_ms)
    }

    /// One tick: the verdict of every (in-world viewer, enemy) pair and the records the clients need.
    // DC-DEFAULT: VIS-1, VIS-2, VIS-4 .. VIS-9, VIS-11, VIS-12 (proposed)
    pub fn update(
        &mut self,
        actors: &[Actor],
        viewers: &[u8],
        ffa: bool,
        tick_ms: u32,
        now: Millis,
    ) -> Vec<Change> {
        let t0 = Instant::now();
        let mut out = Vec::new();
        if !self.cfg.enabled {
            // nothing is ever parked while culling is off
            return out;
        }
        let by_pid: BTreeMap<u8, &Actor> = actors.iter().map(|a| (a.pid, a)).collect();
        // pairs of players that left, changed object or are no longer enemies: show what is still parked, forget
        let mut gone = Vec::new();
        for (&(v, e), p) in &self.pairs {
            let keep = match (by_pid.get(&v), by_pid.get(&e)) {
                (Some(va), Some(ea)) => {
                    va.obj == p.viewer_obj
                        && ea.obj == p.enemy_obj
                        && viewers.contains(&v)
                        && enemies(va, ea, ffa)
                }
                _ => false,
            };
            if !keep {
                gone.push((v, e));
            }
        }
        for k in gone {
            if let Some(p) = self.pairs.remove(&k) {
                if let (true, Some(ea)) = (p.parked, by_pid.get(&k.1)) {
                    if by_pid.get(&k.0).is_some_and(|va| va.obj == p.viewer_obj) {
                        out.push(show(k.0, ea));
                        self.stats.shows += 1;
                    }
                }
            }
        }
        // pass 1: each viewer's own verdict; collect the pairs due for a test
        let mut due: Vec<(u32, f32, u8, u8)> = Vec::new();
        for &vp in viewers {
            let Some(v) = by_pid.get(&vp) else {
                continue;
            };
            for e in actors.iter().filter(|e| enemies(v, e, ffa)) {
                let p = self.pairs.entry((vp, e.pid)).or_insert(Pair {
                    viewer_obj: v.obj,
                    enemy_obj: e.obj,
                    spawns: e.spawns,
                    ..Pair::default()
                });
                if p.spawns != e.spawns {
                    // DC-DEFAULT: VIS-8 (proposed) a spawn restarts the pair (tested in the same tick)
                    p.spawns = e.spawns;
                    p.last_clear = None;
                    p.last_test = None;
                    p.event_until = None;
                }
                p.own = true;
                if !e.cullable {
                    continue;
                }
                let dist = len(sub(v.pos, e.pos));
                if dist <= self.cfg.always_radius {
                    p.last_clear = Some(now);
                    continue;
                }
                let interval = if p.parked { self.cfg.hidden_test_ms } else { 0 };
                let is_due = p.last_test.is_none_or(|t| since(now, t) >= interval);
                if is_due {
                    let age = p.last_test.map_or(u32::MAX, |t| since(now, t));
                    due.push((age, dist, vp, e.pid));
                }
            }
        }
        // most overdue first, then nearest
        due.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.total_cmp(&b.1)));
        let mut budget = self.cfg.max_segments_per_tick;
        let mut used = 0u32;
        for (_, _, vp, ep) in due {
            let (Some(v), Some(e)) = (by_pid.get(&vp), by_pid.get(&ep)) else {
                continue;
            };
            let interval = if self.pairs[&(vp, ep)].parked {
                self.cfg.hidden_test_ms
            } else {
                tick_ms
            };
            let l = self.lookahead_ms(v, tick_ms, interval);
            let cached = self.pairs[&(vp, ep)].cached;
            let Some((clear, n, idx)) = self.test_pair(v, e, l, cached, budget) else {
                used += budget;
                budget = 0;
                continue;
            };
            budget -= n;
            used += n;
            self.stats.tests += 1;
            let p = self.pairs.get_mut(&(vp, ep)).expect("pair");
            p.last_test = Some(now);
            if clear {
                p.last_clear = Some(now);
                p.cached = idx;
            }
        }
        // verdicts
        for &vp in viewers {
            let Some(v) = by_pid.get(&vp) else {
                continue;
            };
            for e in actors.iter().filter(|e| enemies(v, e, ffa) && e.cullable) {
                let p = self.pairs.get_mut(&(vp, e.pid)).expect("pair");
                let recent = |t: Option<Millis>, w: u32| t.is_some_and(|t| since(now, t) < w);
                let event = p.event_until.is_some_and(|t| later_or_eq(t, now));
                let stale = p
                    .last_test
                    .is_none_or(|t| since(now, t) > self.cfg.max_test_age_ms);
                let window = recent(p.last_clear, self.cfg.exposure_ms) && p.last_clear.is_some();
                if stale && !window && !event {
                    self.stats.fail_open += 1;
                }
                p.own = window || event || stale;
            }
        }
        // pass 2: a viewer that is not alive sees what its living teammates see (VIS-11), plus its own last eye
        let mut verdict: BTreeMap<(u8, u8), bool> = BTreeMap::new();
        for &vp in viewers {
            let Some(v) = by_pid.get(&vp) else {
                continue;
            };
            for e in actors.iter().filter(|e| enemies(v, e, ffa)) {
                let own = self.pairs[&(vp, e.pid)].own;
                let mut vis = own;
                if !v.alive && !vis && !ffa {
                    // DC-DEFAULT: VIS-11 (proposed) the team's view for dead and waiting players
                    vis = viewers.iter().any(|&tp| {
                        by_pid.get(&tp).is_some_and(|t| {
                            t.pid != v.pid
                                && t.team == v.team
                                && t.alive
                                && self.pairs.get(&(tp, e.pid)).is_some_and(|p| p.own)
                        })
                    });
                }
                // DC-DEFAULT: VIS-14 (proposed) a dead viewer always sees the player who killed him, until it respawns
                vis |= !v.alive && v.killer == Some(e.pid);
                verdict.insert((vp, e.pid), vis);
            }
        }
        // pass 3: records
        for ((vp, ep), vis) in verdict {
            let Some(e) = by_pid.get(&ep) else {
                continue;
            };
            let l = by_pid.get(&vp).map_or(self.cfg.lookahead_max_ms, |v| {
                self.lookahead_ms(v, tick_ms, self.cfg.hidden_test_ms)
            });
            let refresh = 2 * l.max(self.cfg.refresh_floor_ms);
            let park = self.park;
            let p = self.pairs.get_mut(&(vp, ep)).expect("pair");
            if vis && p.parked {
                p.parked = false;
                out.push(show(vp, e));
                self.stats.shows += 1;
            } else if !vis && !p.parked {
                p.parked = true;
                p.parked_at = now;
                p.refreshed = false;
                out.push(park_change(vp, e, park));
                self.stats.parks += 1;
            } else if p.parked && !p.refreshed && since(now, p.parked_at) >= refresh {
                p.refreshed = true;
                out.push(park_change(vp, e, park));
                self.stats.refreshes += 1;
            }
        }
        let ns = t0.elapsed().as_nanos();
        self.stats.updates += 1;
        self.stats.nanos += ns;
        self.stats.max_nanos = self.stats.max_nanos.max(ns);
        self.stats.segments += u64::from(used);
        self.stats.max_segments = self.stats.max_segments.max(used);
        self.stats.pairs += self.pairs.len() as u64;
        out
    }

    /// A sound of `source` goes to everyone (shot, weapon sound, relayed jump): every viewer that has it parked gets
    /// the show record first, and the pair stays visible for the event window.
    // DC-DEFAULT: VIS-10 (proposed) event exposure; the relays themselves are not culled
    pub fn on_event(&mut self, actors: &[Actor], source: u8, now: Millis) -> Vec<Change> {
        let mut out = Vec::new();
        if !self.cfg.enabled {
            return out;
        }
        let Some(e) = actors.iter().find(|a| a.pid == source) else {
            return out;
        };
        let until = now.wrapping_add(self.cfg.event_ms);
        for (&(vp, ep), p) in self.pairs.iter_mut() {
            if ep != source || p.enemy_obj != e.obj {
                continue;
            }
            p.event_until = Some(until);
            if p.parked {
                p.parked = false;
                out.push(show(vp, e));
                self.stats.shows += 1;
            }
        }
        out
    }

    /// The character of `enemy` becomes visible at its spawn on `viewer`'s client: whether that record must carry
    /// the park point instead of the true position (the pair is decided now, with a full test).
    // DC-DEFAULT: VIS-8 (proposed) a spawn forces a test in the same tick; VIS-9 (proposed) park
    pub fn spawn_parked(
        &mut self,
        actors: &[Actor],
        viewer: u8,
        enemy: u8,
        ffa: bool,
        tick_ms: u32,
        now: Millis,
    ) -> bool {
        if !self.cfg.enabled {
            return false;
        }
        let (Some(v), Some(e)) = (
            actors.iter().find(|a| a.pid == viewer),
            actors.iter().find(|a| a.pid == enemy),
        ) else {
            return false;
        };
        if !enemies(v, e, ffa) || !e.cullable {
            return false;
        }
        let sees = |me: &Visibility, from: &Actor| -> bool {
            if len(sub(from.pos, e.pos)) <= me.cfg.always_radius {
                return true;
            }
            let l = me.lookahead_ms(from, tick_ms, tick_ms);
            me.test_pair(from, e, l, 0, u32::MAX)
                .is_some_and(|(clear, _, _)| clear)
        };
        let mut vis = sees(self, v);
        if !vis && !v.alive && !ffa {
            vis = actors
                .iter()
                .filter(|t| t.pid != v.pid && t.team == v.team && t.alive)
                .any(|t| sees(self, t));
        }
        // DC-DEFAULT: VIS-14 (proposed)
        vis |= !v.alive && v.killer == Some(enemy);
        let p = self.pairs.entry((viewer, enemy)).or_default();
        *p = Pair {
            viewer_obj: v.obj,
            enemy_obj: e.obj,
            spawns: e.spawns,
            parked: !vis,
            parked_at: now,
            refreshed: false,
            last_clear: vis.then_some(now),
            last_test: Some(now),
            event_until: None,
            cached: 0,
            own: vis,
        };
        if !vis {
            self.stats.parks += 1;
        }
        !vis
    }

    /// The statistics line, once per [`REPORT_PERIOD_MS`].
    pub fn take_report(&mut self, now: Millis) -> Option<String> {
        if !self.cfg.enabled || self.stats.updates == 0 {
            return None;
        }
        match self.last_report {
            Some(t) if since(now, t) < REPORT_PERIOD_MS => return None,
            None => {
                self.last_report = Some(now);
                return None;
            }
            _ => {}
        }
        self.last_report = Some(now);
        let s = &self.stats;
        Some(format!(
            "visibility: {} ticks, mean {:.1} us, max {:.1} us, {:.1} segments/tick (max {}), {} tests, {} fail-open \
             verdicts, {} parks, {} refreshes, {} shows",
            s.updates,
            s.mean_us(),
            s.max_nanos as f64 / 1000.0,
            s.segments as f64 / s.updates as f64,
            s.max_segments,
            s.tests,
            s.fail_open,
            s.parks,
            s.refreshes,
            s.shows
        ))
    }

    /// The pair test: is one of the sample segments clear? `None` when the budget ran out before an answer;
    /// otherwise (clear, segments used, index of the clear segment).
    // DC-DEFAULT: VIS-4 (proposed) sample points; VIS-6 (proposed) margin and peek offset; VIS-5 (proposed) lookahead
    fn test_pair(
        &self,
        v: &Actor,
        e: &Actor,
        lookahead_ms: u32,
        cached: u8,
        budget: u32,
    ) -> Option<(bool, u32, u8)> {
        let Some(occ) = self.occ.as_deref().filter(|o| !o.is_empty()) else {
            return Some((true, 0, 0));
        };
        let segs = self.segments(v, e, lookahead_ms);
        let n = segs.len();
        if n == 0 {
            return Some((true, 0, 0));
        }
        let first = (cached as usize).min(n - 1);
        let mut used = 0u32;
        for k in 0..n {
            let i = (first + k) % n;
            if used >= budget {
                return None;
            }
            used += 1;
            if !occ.blocked(segs[i].0, segs[i].1) {
                return Some((true, used, i as u8));
            }
        }
        Some((false, used, cached))
    }

    /// The sample segments of a pair, in priority order.
    // DC-DEFAULT: VIS-4 (proposed) docs/VISIBILITY_CULLING_DESIGN.md §3.3
    fn segments(&self, v: &Actor, e: &Actor, lookahead_ms: u32) -> Vec<(V, V)> {
        let c = &self.cfg;
        let l = lookahead_ms as f32 / 1000.0;
        let eye = add(v.pos, [0.0, c.eye_frac * v.half[1], 0.0]);
        let p = e.pos;
        let h = e.half[1];
        let w = e.half[0].max(e.half[2]);
        // sideways unit vector, perpendicular to the line of sight in the horizontal plane
        let d = sub(p, eye);
        let hl = (d[0] * d[0] + d[2] * d[2]).sqrt();
        let s = if hl > 1e-3 {
            [-d[2] / hl, 0.0, d[0] / hl]
        } else {
            [1.0, 0.0, 0.0]
        };
        let up = |q: V, f: f32| add(q, [0.0, f * h, 0.0]);
        let mut out = vec![
            (eye, p),
            (eye, up(p, c.head_frac)),
            (eye, up(p, c.low_frac)),
            (eye, add(p, mul(s, w + c.margin))),
            (eye, add(p, mul(s, -(w + c.margin)))),
        ];
        // the enemy ahead along its velocity
        if len(e.vel) > 1.0 && finite(e.vel) {
            let q = add(p, mul(e.vel, l));
            out.push((eye, q));
            out.push((eye, up(q, c.head_frac)));
        }
        // the viewer's own possible eyes: ahead along its velocity and the two peek offsets
        let k = (c.run_speed * l).min(c.peek_max);
        let mut eyes = Vec::with_capacity(3);
        if len(v.vel) > 1.0 && finite(v.vel) {
            eyes.push(add(eye, mul(v.vel, l)));
        }
        eyes.push(add(eye, mul(s, k)));
        eyes.push(add(eye, mul(s, -k)));
        for e1 in eyes {
            out.push((e1, p));
            out.push((e1, up(p, c.head_frac)));
        }
        out
    }
}

/// `a` is at or after `b` on the wrapping clock.
fn later_or_eq(a: Millis, b: Millis) -> bool {
    a == b || dc_net_engine::time::later(a, b)
}

fn show(viewer: u8, e: &Actor) -> Change {
    Change {
        viewer,
        enemy: e.pid,
        obj: e.obj,
        kind: ChangeKind::Show {
            pos: e.pos,
            vel: if finite(e.vel) { e.vel } else { [0.0; 3] },
        },
        full_precision: e.full_precision,
    }
}

fn park_change(viewer: u8, e: &Actor, park: V) -> Change {
    Change {
        viewer,
        enemy: e.pid,
        obj: e.obj,
        kind: ChangeKind::Park(park),
        full_precision: e.full_precision,
    }
}

/// The change record of a hide / show: `cf = CF_POS | CF_POS_SNAP` (two-byte head), then the position block:
/// `pos48` + `vec72` over the world box, or raw floats for an object with the full-precision flag.
// SPEC: UDP_SESSION_2009_B.md §3.2 (head), §4.1 (CF_POS_SNAP: same fields as CF_POS, the reader tests 0x0202), §4.2
// item 6 (position block forms); CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §1.4 (CF_POS_SNAP in a character's change
// record, ACCEPTED)
// DC-DEFAULT: VIS-9 (proposed) park / show records
pub fn snap_record(c: &Change, pbox: &PosBox) -> BitWriter {
    let (pos, vel) = match c.kind {
        ChangeKind::Park(p) => (p, [0.0; 3]),
        ChangeKind::Show { pos, vel } => (pos, vel),
    };
    let mut w = BitWriter::new();
    write_record_head(&mut w, CF_POS | CF_POS_SNAP, c.obj);
    if c.full_precision {
        for v in pos.iter().chain(vel.iter()) {
            w.write_f32(*v);
        }
    } else {
        pos48_encode(pos, pbox).write(&mut w);
        Vec72::encode(vel).write(&mut w);
    }
    w
}

/// Park point: the point of a 5 x 5 grid on the bottom face of the world box (corners first) whose centre and head
/// are seen by the fewest of the sampled standing positions (each spawn point at two eye heights); the first such
/// point wins a tie. With no occluders every candidate is equal and the first corner is taken.
// DC-DEFAULT: VIS-9 (proposed) park point on the bottom face of the `pos48` box, by a load-time census
pub fn park_point(occ: Option<&Occluders>, pbox: &PosBox, spawns: &[V]) -> V {
    let y = pbox.min[1];
    let lerp = |k: usize, i: usize| pbox.min[k] + (pbox.max[k] - pbox.min[k]) * (i as f32 / 4.0);
    let mut cands: Vec<V> = Vec::new();
    for (i, j) in [(0, 0), (4, 0), (0, 4), (4, 4)] {
        cands.push([lerp(0, i), y, lerp(2, j)]);
    }
    for i in 0..5 {
        for j in 0..5 {
            let c = [lerp(0, i), y, lerp(2, j)];
            if !cands.contains(&c) {
                cands.push(c);
            }
        }
    }
    let Some(occ) = occ.filter(|o| !o.is_empty()) else {
        return cands[0];
    };
    let eyes: Vec<V> = spawns
        .iter()
        .flat_map(|s| [add(*s, [0.0, 30.0, 0.0]), add(*s, [0.0, 90.0, 0.0])])
        .collect();
    let mut best = (usize::MAX, cands[0]);
    for c in cands {
        let seen = eyes
            .iter()
            .map(|e| {
                [c, add(c, [0.0, 90.0, 0.0])]
                    .iter()
                    .filter(|t| !occ.blocked(*e, **t))
                    .count()
            })
            .sum::<usize>();
        if seen < best.0 {
            best = (seen, c);
            if seen == 0 {
                break;
            }
        }
    }
    best.1
}

/// Shell messages whose relays reveal the sender's position (relayed to everyone, unculled).
pub fn is_audible(up: &crate::up::Up) -> bool {
    use crate::up::Up;
    matches!(up, Up::Fire(_) | Up::WeaponSound { .. } | Up::Event { .. })
}

/// A synthetic world (made-up geometry) for the culling tests: the world box of `world::synthetic_world_bytes`
/// widened to 4096 x 320 x 4096, and one thick wall (two back-to-back faces 16 units apart) in the plane x = 0 from
/// the box's z minimum to z = 64, over the whole height (open beyond z = 64). Synthetic clients walk along x, so
/// they cross the wall.
pub fn synthetic_walled_world_bytes() -> Vec<u8> {
    use dc_world::synth::{SynthModel, SynthObject, SynthWorld};
    let names: [&[u8]; 3] = [b"Nm", b"Pos", b"Rot"];
    let (z0, z1, y0, y1) = (-2048.0f32, 64.0f32, -64.0f32, 256.0f32);
    // face A at x = -8 looks to -x, face B at x = +8 looks to +x
    let points = vec![
        [-8.0, y0, z0],
        [-8.0, y1, z0],
        [-8.0, y1, z1],
        [-8.0, y0, z1],
        [8.0, y0, z0],
        [8.0, y0, z1],
        [8.0, y1, z1],
        [8.0, y1, z0],
    ];
    SynthWorld {
        info: b"synthetic walled".to_vec(),
        extent_min: [-2048.0, -64.0, -2048.0],
        extent_max: [2048.0, 256.0, 2048.0],
        tree_shape: vec![false],
        models: vec![SynthModel {
            name: b"main".to_vec(),
            textures: vec![b"wall".to_vec()],
            planes: vec![([-1.0, 0.0, 0.0], 8.0), ([1.0, 0.0, 0.0], 8.0)],
            surfaces: vec![(SURFACE_SOLID, 0, 0)],
            polys: vec![(0, 0, vec![0, 1, 2, 3]), (0, 1, vec![4, 5, 6, 7])],
            points,
            root: -1,
            box_min: [-8.0, y0, z0],
            box_max: [8.0, y1, z1],
            ..SynthModel::default()
        }],
        objects: vec![SynthObject::point(
            b"SynthMarker",
            names,
            b"marker01",
            [16.0, 0.0, 32.0],
            [0.0, 1.0, 0.0],
        )],
        ..SynthWorld::default()
    }
    .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(normal: V, c: [V; 4]) -> OccluderPoly {
        OccluderPoly {
            normal,
            corners: c.to_vec(),
        }
    }

    /// A wall in the plane x = 0 (front to -x) from z -100 to 100, y 0 to 200, with a 64-unit window at z 0..64,
    /// y 80..144 (four quads around the hole).
    fn wall_with_window() -> Occluders {
        let n = [-1.0, 0.0, 0.0];
        let q = |z0: f32, z1: f32, y0: f32, y1: f32| {
            quad(
                n,
                [[0.0, y0, z0], [0.0, y1, z0], [0.0, y1, z1], [0.0, y0, z1]],
            )
        };
        Occluders::from_polys(&[
            q(-100.0, 0.0, 0.0, 200.0),
            q(64.0, 100.0, 0.0, 200.0),
            q(0.0, 64.0, 0.0, 80.0),
            q(0.0, 64.0, 144.0, 200.0),
        ])
    }

    #[test]
    fn wall_blocks_from_the_front_only_and_the_window_lets_through() {
        let o = wall_with_window();
        assert_eq!(o.triangles(), 8);
        // from the front (-x side) through the wall
        assert!(o.blocked([-50.0, 40.0, -50.0], [50.0, 40.0, -50.0]));
        // the back face does not occlude (one-sided, as the renderer culls back faces)
        assert!(!o.blocked([50.0, 40.0, -50.0], [-50.0, 40.0, -50.0]));
        // through the window
        assert!(!o.blocked([-50.0, 110.0, 30.0], [50.0, 110.0, 30.0]));
        // beside the wall
        assert!(!o.blocked([-50.0, 40.0, 150.0], [50.0, 40.0, 150.0]));
        // a segment ending on the face is not blocked by it (end slack)
        assert!(!o.blocked([-50.0, 40.0, -50.0], [0.0, 40.0, -50.0]));
        // degenerate and non-finite input
        assert!(!o.blocked([1.0, 1.0, 1.0], [1.0, 1.0, 1.0]));
        assert!(!o.blocked([f32::NAN, 0.0, 0.0], [1.0, 1.0, 1.0]));
        assert!(!Occluders::default().blocked([0.0; 3], [10.0, 0.0, 0.0]));
    }

    #[test]
    fn bvh_agrees_with_brute_force_on_many_triangles() {
        // a grid of small one-sided tiles facing -y, at random heights; the BVH and a linear scan must agree
        let mut rng = 0x1234_5678u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % 10_000) as f32 / 10_000.0
        };
        let mut polys = Vec::new();
        for i in 0..40 {
            for j in 0..40 {
                let (x, z, y) = (i as f32 * 25.0, j as f32 * 25.0, next() * 200.0);
                polys.push(quad(
                    [0.0, -1.0, 0.0],
                    [
                        [x, y, z],
                        [x + 20.0, y, z],
                        [x + 20.0, y, z + 20.0],
                        [x, y, z + 20.0],
                    ],
                ));
            }
        }
        let o = Occluders::from_polys(&polys);
        let mut tris = Vec::new();
        for p in &polys {
            push_fan(&mut tris, p.normal, &p.corners);
        }
        let brute = |a: V, b: V| {
            let d = sub(b, a);
            let l = len(d);
            tris.iter()
                .any(|t| tri_hit(t, a, d, END_SLACK / l, 1.0 - END_SLACK / l))
        };
        let mut hits = 0;
        for _ in 0..3000 {
            let a = [next() * 1000.0, next() * 300.0 - 50.0, next() * 1000.0];
            let b = [next() * 1000.0, next() * 300.0 - 50.0, next() * 1000.0];
            let x = brute(a, b);
            assert_eq!(o.blocked(a, b), x, "{a:?} -> {b:?}");
            hits += usize::from(x);
        }
        assert!(hits > 100 && hits < 2900, "{hits}");
    }

    fn actor(pid: u8, team: u8, pos: V) -> Actor {
        Actor {
            pid,
            obj: 100 + pid as u16,
            team,
            alive: true,
            cullable: true,
            pos,
            vel: [0.0; 3],
            half: [15.0, 50.0, 15.0],
            rtt_ms: 0,
            spawns: 1,
            full_precision: false,
            killer: None,
        }
    }

    fn pbox() -> PosBox {
        PosBox {
            min: [-512.0, -64.0, -512.0],
            max: [512.0, 256.0, 512.0],
        }
    }

    /// A long opaque wall in the plane x = 0 (thick: faces both ways), with gaps nowhere.
    fn long_wall() -> Arc<Occluders> {
        let q = |x: f32, nx: f32| {
            let c = if nx < 0.0 {
                [
                    [x, -64.0, -512.0],
                    [x, 256.0, -512.0],
                    [x, 256.0, 512.0],
                    [x, -64.0, 512.0],
                ]
            } else {
                [
                    [x, -64.0, -512.0],
                    [x, -64.0, 512.0],
                    [x, 256.0, 512.0],
                    [x, 256.0, -512.0],
                ]
            };
            quad([nx, 0.0, 0.0], c)
        };
        Arc::new(Occluders::from_polys(&[q(-8.0, -1.0), q(8.0, 1.0)]))
    }

    fn vis(cfg: VisTuning) -> Visibility {
        Visibility::new(cfg, Some(long_wall()), &pbox(), &[])
    }

    #[test]
    fn hidden_enemy_parks_after_its_window_and_shows_on_sight() {
        let mut v = vis(VisTuning::default());
        let mut a = vec![
            actor(1, 1, [-450.0, 0.0, 0.0]),
            actor(2, 2, [450.0, 0.0, 0.0]),
        ];
        // never seen: parked at the first test, both ways
        let c = v.update(&a, &[1, 2], false, 33, 0);
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(c.iter().all(|c| matches!(c.kind, ChangeKind::Park(_))));
        assert!(v.parked(1, 102) && v.parked(2, 101));
        // the park point is on the bottom face of the box
        assert_eq!(v.park_point()[1], -64.0);
        // the refresh comes once, 2 x max(L, 200) ms later (L = 0 + 150 + 66 + 60 = 276 ms)
        let mut refresh_at = None;
        for t in (33..2000).step_by(33) {
            for c in v.update(&a, &[1, 2], false, 33, t) {
                assert!(matches!(c.kind, ChangeKind::Park(_)));
                if c.viewer == 1 {
                    assert!(refresh_at.is_none(), "one refresh only");
                    refresh_at = Some(t);
                }
            }
        }
        assert!(
            refresh_at.is_some_and(|t| (552..=600).contains(&t)),
            "{refresh_at:?}"
        );
        // the enemy walks round the wall end: shown again (snap back to the true position)
        a[1].pos = [-300.0, 0.0, 700.0];
        a[1].vel = [0.0, 0.0, 0.0];
        let c = v.update(&a, &[1, 2], false, 33, 2000);
        assert_eq!(c.len(), 2);
        for ch in &c {
            match ch.kind {
                ChangeKind::Show { pos, .. } => {
                    let truth = if ch.viewer == 1 { a[1].pos } else { a[0].pos };
                    assert_eq!(pos, truth);
                }
                k => panic!("{k:?}"),
            }
        }
        assert!(!v.parked(1, 102));
        // back behind the wall: still visible for the exposure window, parked after it
        a[1].pos = [450.0, 0.0, 0.0];
        let mut parked_at = None;
        for t in (2033..4000).step_by(33) {
            if v.update(&a, &[1, 2], false, 33, t)
                .iter()
                .any(|c| c.viewer == 1 && matches!(c.kind, ChangeKind::Park(_)))
            {
                parked_at = Some(t);
                break;
            }
        }
        let t = parked_at.expect("parked again");
        assert!(
            (3000..3040).contains(&t),
            "window 1000 ms after the last clear test at 2000: {t}"
        );
        assert!(v.stats.parks >= 3 && v.stats.shows == 2);
    }

    #[test]
    fn teammates_corpses_radius_and_events_are_never_parked() {
        let mut v = vis(VisTuning::default());
        // a teammate behind the wall
        let mut a = vec![
            actor(1, 1, [-450.0, 0.0, 0.0]),
            actor(2, 1, [450.0, 0.0, 0.0]),
            actor(3, 2, [450.0, 0.0, 100.0]),
        ];
        let c = v.update(&a, &[1, 2, 3], false, 33, 0);
        // only the enemy pair across the wall: 1 <-> 3 (2 and 3 are on the same side)
        assert!(c
            .iter()
            .all(|c| (c.viewer, c.enemy) == (1, 3) || (c.viewer, c.enemy) == (3, 1)));
        assert!(!v.parked(1, 102) && !v.parked(2, 101));
        assert!(!v.parked(2, 103) && v.parked(1, 103));
        // FFA: the teammate rule does not apply
        let mut f = vis(VisTuning::default());
        f.update(&a, &[1, 2], true, 33, 0);
        assert!(f.parked(1, 102));
        // a corpse is shown and never parked
        a[2].cullable = false;
        a[2].alive = false;
        let c = v.update(&a, &[1, 2, 3], false, 33, 33);
        assert!(c
            .iter()
            .any(|c| (c.viewer, c.enemy) == (1, 3) && matches!(c.kind, ChangeKind::Show { .. })));
        assert!(!v.parked(1, 103));
        // within the always-visible radius through the wall
        let mut r = vis(VisTuning::default());
        let near = vec![
            actor(1, 1, [-450.0, 0.0, 0.0]),
            actor(2, 2, [450.0, 0.0, 0.0]),
        ];
        r.update(&near, &[1, 2], false, 33, 0);
        assert!(r.parked(1, 102));
        let near2 = vec![
            actor(1, 1, [-100.0, 0.0, 0.0]),
            actor(2, 2, [100.0, 0.0, 0.0]),
        ];
        let c = r.update(&near2, &[1, 2], false, 33, 33);
        assert_eq!(c.len(), 2);
        assert!(!r.parked(1, 102));
        // an event of a parked enemy: shown at once and kept visible for the event window
        let mut e = vis(VisTuning::default());
        e.update(&near, &[1, 2], false, 33, 0);
        assert!(e.parked(1, 102));
        let c = e.on_event(&near, 2, 40);
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].viewer, c[0].obj), (1, 102));
        assert!(!e.parked(1, 102));
        assert!(e.update(&near, &[1, 2], false, 33, 66).is_empty());
        assert!(e.update(&near, &[1, 2], false, 33, 500).is_empty());
        assert!(!e.parked(1, 102));
        // after the window: parked again
        let c = e.update(&near, &[1, 2], false, 33, 600);
        assert!(c.iter().any(|c| c.viewer == 1));
        assert!(e.parked(1, 102));
    }

    #[test]
    fn dead_viewer_always_sees_its_killer_until_it_respawns() {
        // VIS-14: 1 (team 1) is dead; its only teammate 2 sees nobody; 3 killed it, 4 is another hidden enemy
        let mut v = vis(VisTuning::default());
        let mut a = vec![
            actor(1, 1, [-450.0, 0.0, 0.0]),
            actor(2, 1, [-450.0, 0.0, 200.0]),
            actor(3, 2, [450.0, 0.0, 0.0]),
            actor(4, 2, [450.0, 0.0, 100.0]),
        ];
        a[0].alive = false;
        a[0].cullable = false;
        a[0].killer = Some(3);
        v.update(&a, &[1, 2], false, 33, 0);
        assert!(!v.parked(1, 103), "the killer is never parked");
        assert!(v.parked(1, 104), "another hidden enemy stays parked");
        // respawned: the killer is culled like any other
        a[0].alive = true;
        a[0].cullable = true;
        a[0].killer = None;
        v.update(&a, &[1, 2], false, 33, 1000);
        assert!(v.parked(1, 103));
    }

    #[test]
    fn dead_viewer_sees_the_team_union_and_budget_fails_open() {
        let mut v = vis(VisTuning::default());
        let mut a = vec![
            actor(1, 1, [-450.0, 0.0, 0.0]),
            actor(2, 1, [450.0, 0.0, -300.0]),
            actor(3, 2, [450.0, 0.0, 300.0]),
        ];
        // 1 is dead behind the wall; its teammate 2 sees 3 (same side): 1 gets 3 too
        a[0].alive = false;
        a[0].cullable = false;
        v.update(&a, &[1, 2, 3], false, 33, 0);
        assert!(!v.parked(1, 103), "team view");
        // alive again and still behind the wall: parked
        a[0].alive = true;
        a[0].cullable = true;
        v.update(&a, &[1, 2, 3], false, 33, 33);
        assert!(v.parked(1, 103));
        // budget 0: nothing can be tested, every pair fails open (visible)
        let mut z = vis(VisTuning {
            max_segments_per_tick: 0,
            ..VisTuning::default()
        });
        let c = z.update(&a, &[1, 3], false, 33, 0);
        assert!(c.is_empty(), "{c:?}");
        assert!(!z.parked(1, 103));
        assert!(z.stats.fail_open > 0);
        // culling off: never anything
        let mut off = vis(VisTuning::off());
        assert!(off.update(&a, &[1, 2, 3], false, 33, 0).is_empty());
        assert!(!off.parked(1, 103));
    }

    #[test]
    fn spawn_decision_and_lookahead_peek() {
        let mut v = vis(VisTuning::default());
        let a = vec![
            actor(1, 1, [-450.0, 0.0, 0.0]),
            actor(2, 2, [450.0, 0.0, 0.0]),
        ];
        assert!(v.spawn_parked(&a, 1, 2, false, 33, 0));
        assert!(v.parked(1, 102));
        // an enemy that runs toward the wall end is seen ahead of time (lookahead along its velocity)
        let mut w = vis(VisTuning::default());
        let wall_end = Arc::new(Occluders::from_polys(&[quad(
            [-1.0, 0.0, 0.0],
            [
                [0.0, -64.0, -512.0],
                [0.0, 256.0, -512.0],
                [0.0, 256.0, 0.0],
                [0.0, -64.0, 0.0],
            ],
        )]));
        w.occ = Some(wall_end);
        let mut e = actor(2, 2, [450.0, 0.0, -150.0]);
        let me = actor(1, 1, [-450.0, 0.0, -150.0]);
        let c = w.update(&[me, e], &[1], false, 33, 0);
        assert!(matches!(c[0].kind, ChangeKind::Park(_)), "{c:?}");
        assert!(w.parked(1, 102));
        // still behind the wall, but its velocity carries it past the end within the lookahead (276 ms)
        e.pos = [450.0, 0.0, -40.0];
        w.update(&[me, e], &[1], false, 33, 70);
        assert!(w.parked(1, 102), "standing there it stays hidden");
        e.vel = [0.0, 0.0, 1000.0];
        w.update(&[me, e], &[1], false, 33, 140);
        assert!(!w.parked(1, 102), "shown before the line of sight opens");
    }

    #[test]
    fn snap_records_layout() {
        let b = pbox();
        let c = Change {
            viewer: 1,
            enemy: 2,
            obj: 9,
            kind: ChangeKind::Park([-512.0, -64.0, -512.0]),
            full_precision: false,
        };
        let r = snap_record(&c, &b);
        // head 2 bytes (marker) + id 16 + pos48 + vec72
        assert_eq!(r.bit_len(), 16 + 16 + 48 + 72);
        let mut rd = r.reader();
        assert_eq!((rd.read_u8(), rd.read_u8(), rd.read_u16()), (0x82, 0x02, 9));
        let p = dc_wire::udp::Pos48::read(&mut rd);
        assert_eq!((p.px, p.py, p.pz), (0, 0, 0));
        let full = Change {
            full_precision: true,
            kind: ChangeKind::Show {
                pos: [1.0, 2.0, 3.0],
                vel: [4.0, 0.0, 0.0],
            },
            ..c
        };
        let r = snap_record(&full, &b);
        assert_eq!(r.bit_len(), 32 + 6 * 32);
        let mut rd = r.reader();
        rd.skip_bits(32);
        assert_eq!(
            [rd.read_f32(), rd.read_f32(), rd.read_f32()],
            [1.0, 2.0, 3.0]
        );
        assert_eq!(rd.read_f32(), 4.0);
    }

    #[test]
    fn park_point_census_avoids_a_seen_corner() {
        let b = pbox();
        // a floor at y = 50 facing up hides the bottom face from above, except beside its (-x, -z) edge
        let floor = Occluders::from_polys(&[quad(
            [0.0, 1.0, 0.0],
            [
                [-300.0, 50.0, -300.0],
                [-300.0, 50.0, 512.0],
                [512.0, 50.0, 512.0],
                [512.0, 50.0, -300.0],
            ],
        )]);
        let spawns = [[-400.0, 60.0, -400.0], [200.0, 60.0, 200.0]];
        let p = park_point(Some(&floor), &b, &spawns);
        assert_eq!(p[1], -64.0);
        // the first corner (-512, -512) is seen from the spawn standing beside the floor; another point is chosen
        assert_ne!((p[0], p[2]), (-512.0, -512.0));
        assert_eq!(park_point(None, &b, &spawns), [-512.0, -64.0, -512.0]);
    }

    #[test]
    fn synthetic_walled_world_has_the_wall_as_occluders() {
        let d = synthetic_walled_world_bytes();
        let w = crate::world::world_from_bytes(&d, "Worlds\\W.dat", "W").unwrap();
        let o = w.occluders.as_ref().expect("occluders");
        assert_eq!((o.models, o.polygons, o.triangles()), (1, 2, 4));
        assert!(o.blocked([-300.0, 40.0, -300.0], [300.0, 40.0, -300.0]));
        assert!(o.blocked([300.0, 40.0, -300.0], [-300.0, 40.0, -300.0]));
        assert!(!o.blocked([-300.0, 40.0, 300.0], [300.0, 40.0, 300.0]));
    }
}
