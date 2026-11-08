//! The composed level model: an owned BSP the composer appends trees to.
//!
//! - The map's own level model is copied as it is (its tree stays valid for any sweep: dc-mapkit already left the
//!   terrain out of every region).
//! - A piece's level model is appended with every polygon kept to the region and outside the cuts. The ROM's own
//!   tree is kept: a node's plane is its polygon's plane, which clipping does not move, so a node whose polygon
//!   is gone keeps a plane-only helper polygon, and a node whose polygon fell apart into several pieces becomes a
//!   chain of nodes on the same plane (the first keeps the negative child, the last the positive one, so a sweep
//!   that touches the plane still tests every piece and reaches both subtrees).
//! - The map's own solid polygons that reach into a region are inserted into that piece's tree (clipped to the
//!   region), so cover and floors a mapper adds inside a piece still stop players and shots.
//! - A kd partition of helper nodes on the region faces joins the trees: inside a region a sweep walks that
//!   piece's tree, outside every region the map's own tree. A sweep that crosses a region face walks both sides,
//!   which is why every polygon of a piece is clipped to its region (the walk tests the whole segment on both
//!   sides, dc-sim `collide.rs CollModel::sweep`).
// SPEC: FMT_DAT85.md §5.2 (BSP body records, node child order: the first child is the negative side, -1 / -2 sentinels)
// DC-DEFAULT: MAP-116

use super::clip::{self, Aabb, V};
use crate::synth::SynthModel;
use crate::{BspModel, NodeRef};
use std::collections::BTreeMap;

/// Missing negative / positive child, as the ROM writes them.
pub const NO_NEG: i32 = -2;
pub const NO_POS: i32 = -1;
/// Side test of an inserted polygon against a node plane.
const SIDE_EPS: f32 = 0.01;
/// Surface flag bit 0: the walk sweeps this polygon (FMT_DAT85.md §5.2).
pub const SURF_SOLID: u32 = 1;
/// Surface flag 0x10: a sky face.
pub const SURF_SKY: u32 = 0x10;

/// A polygon of the map's own level that a region may need: surface and plane in the composed model, corners.
#[derive(Debug, Clone)]
pub struct OwnPoly {
    pub surface: u32,
    pub plane: u32,
    pub corners: Vec<V>,
    pub bounds: Aabb,
}

/// The composed level model under construction.
#[derive(Debug, Default)]
pub struct Tree {
    pub flags: u32,
    pub name: Vec<u8>,
    pub textures: Vec<Vec<u8>>,
    tex_index: BTreeMap<Vec<u8>, u16>,
    pub surfaces: Vec<(u32, u16, u16)>,
    surf_index: BTreeMap<(u32, u16, u16), u32>,
    pub planes: Vec<(V, f32)>,
    helper_of_plane: BTreeMap<u32, u32>,
    helper_surface: Option<u32>,
    pub polys: Vec<(u32, u32, Vec<u32>)>,
    /// (polygon, reserved, negative child, positive child)
    pub nodes: Vec<(u32, u16, i32, i32)>,
    pub points: Vec<V>,
    point_index: BTreeMap<[u32; 3], u32>,
    pub origin: V,
}

fn bits(p: V) -> [u32; 3] {
    let z = |x: f32| {
        if x == 0.0 {
            0.0f32.to_bits()
        } else {
            x.to_bits()
        }
    };
    [z(p[0]), z(p[1]), z(p[2])]
}

/// A child reference moved by `base` nodes; the two sentinels keep the value the file had.
fn node_ref(r: NodeRef, base: usize) -> i32 {
    match r {
        NodeRef::Node(i) => (base + i as usize) as i32,
        NodeRef::TermA => -1,
        NodeRef::TermB => -2,
    }
}

impl Tree {
    /// An empty model with the given flags, name and origin.
    pub fn new(flags: u32, name: &[u8], origin: V) -> Tree {
        Tree {
            flags,
            name: name.to_vec(),
            origin,
            ..Tree::default()
        }
    }

    pub fn texture(&mut self, name: &[u8]) -> u16 {
        if let Some(&i) = self.tex_index.get(name) {
            return i;
        }
        let i = self.textures.len() as u16;
        self.textures.push(name.to_vec());
        self.tex_index.insert(name.to_vec(), i);
        i
    }

    pub fn surface(&mut self, flags: u32, texture: u16, texflags: u16) -> u32 {
        let key = (flags, texture, texflags);
        if let Some(&i) = self.surf_index.get(&key) {
            return i;
        }
        let i = self.surfaces.len() as u32;
        self.surfaces.push(key);
        self.surf_index.insert(key, i);
        i
    }

    pub fn plane(&mut self, n: V, d: f32) -> u32 {
        self.planes.push((n, d));
        (self.planes.len() - 1) as u32
    }

    pub fn point(&mut self, p: V) -> u32 {
        let key = bits(p);
        if let Some(&i) = self.point_index.get(&key) {
            return i;
        }
        let i = self.points.len() as u32;
        self.points.push(p);
        self.point_index.insert(key, i);
        i
    }

    pub fn poly(&mut self, surface: u32, plane: u32, corners: &[V]) -> u32 {
        let pts = corners.iter().map(|&c| self.point(c)).collect();
        self.polys.push((surface, plane, pts));
        (self.polys.len() - 1) as u32
    }

    /// The plane-only polygon of a plane (no corners, a flag-less surface), one per plane.
    pub fn helper(&mut self, plane: u32) -> u32 {
        if let Some(&p) = self.helper_of_plane.get(&plane) {
            return p;
        }
        let s = match self.helper_surface {
            Some(s) => s,
            None => {
                if self.textures.is_empty() {
                    self.texture(b"none");
                }
                let s = self.surface(0, 0, 0);
                self.helper_surface = Some(s);
                s
            }
        };
        self.polys.push((s, plane, Vec::new()));
        let p = (self.polys.len() - 1) as u32;
        self.helper_of_plane.insert(plane, p);
        p
    }

    /// Copies a model as it is (the map's own level); returns its root.
    pub fn add_plain(&mut self, m: &BspModel<'_>) -> i32 {
        let tex: Vec<u16> = m.texture_names.iter().map(|t| self.texture(t)).collect();
        let surf: Vec<u32> = m
            .surfaces
            .iter()
            .map(|s| {
                let t = tex.get(s.texture as usize).copied().unwrap_or(0);
                self.surface(s.flags, t, s.texflags)
            })
            .collect();
        let plane: Vec<u32> = m
            .planes
            .iter()
            .map(|p| self.plane(p.normal, p.dist))
            .collect();
        let poly_base = self.polys.len();
        for p in &m.polygons {
            let corners: Vec<V> = m
                .polygon_points(p)
                .iter()
                .filter_map(|&i| m.points.get(i as usize).copied())
                .collect();
            let s = surf.get(p.surface as usize).copied().unwrap_or(0);
            let pl = plane.get(p.plane as usize).copied().unwrap_or(0);
            self.poly(s, pl, &corners);
        }
        let base = self.nodes.len();
        for n in &m.nodes {
            self.nodes.push((
                (poly_base + n.poly as usize) as u32,
                n.reserved,
                node_ref(n.front, base),
                node_ref(n.back, base),
            ));
        }
        node_ref(m.root, base)
    }

    /// The solid polygons with corners now in the model (the map's own level), for insertion into regions.
    pub fn own_polys(&self) -> Vec<OwnPoly> {
        self.polys
            .iter()
            .filter(|(s, _, pts)| {
                pts.len() >= 3
                    && self
                        .surfaces
                        .get(*s as usize)
                        .is_some_and(|f| f.0 & SURF_SOLID != 0)
            })
            .filter_map(|(s, pl, pts)| {
                let corners: Vec<V> = pts.iter().map(|&i| self.points[i as usize]).collect();
                let bounds = Aabb::of_points(corners.iter())?;
                Some(OwnPoly {
                    surface: *s,
                    plane: *pl,
                    corners,
                    bounds,
                })
            })
            .collect()
    }

    /// Appends a piece's level model moved by `offset`, kept to `region` outside `cuts`, without its sky faces,
    /// with the map's own polygons that reach into the region inserted; returns the root of the piece's tree.
    pub fn add_piece(
        &mut self,
        m: &BspModel<'_>,
        offset: V,
        region: &Aabb,
        cuts: &[Aabb],
        own: &[OwnPoly],
    ) -> i32 {
        let mut root = self.add_clipped(m, offset, region, cuts, true);
        for o in own {
            if !region.overlaps(&o.bounds) {
                continue;
            }
            if let Some(c) = clip::clip_box(&o.corners, region) {
                let id = self.poly(o.surface, o.plane, &c);
                root = self.insert(root, id, &c);
            }
        }
        root
    }

    /// Whether any polygon with corners is in the model.
    pub fn has_faces(&self) -> bool {
        self.polys.iter().any(|p| p.2.len() >= 3)
    }

    /// Appends a model moved by `offset` with every polygon kept to `region` outside `cuts` (sky faces left out
    /// when `drop_sky`), on the model's own tree; returns its root.
    pub fn add_clipped(
        &mut self,
        m: &BspModel<'_>,
        offset: V,
        region: &Aabb,
        cuts: &[Aabb],
        drop_sky: bool,
    ) -> i32 {
        let tex: Vec<u16> = m.texture_names.iter().map(|t| self.texture(t)).collect();
        let surf: Vec<u32> = m
            .surfaces
            .iter()
            .map(|s| {
                let t = tex.get(s.texture as usize).copied().unwrap_or(0);
                self.surface(s.flags, t, s.texflags)
            })
            .collect();
        let plane: Vec<u32> = m
            .planes
            .iter()
            .map(|p| self.plane(p.normal, p.dist + clip::dot(p.normal, offset)))
            .collect();
        // each polygon -> its kept pieces (polygon indices in the composed model)
        let mut pieces: Vec<Vec<u32>> = Vec::with_capacity(m.polygons.len());
        for p in &m.polygons {
            let sflags = m.surfaces.get(p.surface as usize).map_or(0, |s| s.flags);
            let s = surf.get(p.surface as usize).copied().unwrap_or(0);
            let pl = plane.get(p.plane as usize).copied().unwrap_or(0);
            if drop_sky && sflags & SURF_SKY != 0 {
                pieces.push(Vec::new());
                continue;
            }
            let corners: Vec<V> = m
                .polygon_points(p)
                .iter()
                .filter_map(|&i| m.points.get(i as usize).map(|&q| clip::add(q, offset)))
                .collect();
            if corners.len() < 3 {
                pieces.push(Vec::new());
                continue;
            }
            let kept = clip::keep(&corners, region, cuts);
            let ids = kept.iter().map(|c| self.poly(s, pl, c)).collect();
            pieces.push(ids);
        }
        let base = self.nodes.len();
        self.nodes
            .resize(m.nodes.len() + base, (0, 0, NO_NEG, NO_POS));
        for (i, n) in m.nodes.iter().enumerate() {
            let neg = node_ref(n.front, base);
            let pos = node_ref(n.back, base);
            let src = n.poly as usize;
            let pl = m
                .polygons
                .get(src)
                .and_then(|p| plane.get(p.plane as usize).copied())
                .unwrap_or(0);
            let parts = pieces.get(src).cloned().unwrap_or_default();
            match parts.len() {
                0 => {
                    let h = self.helper(pl);
                    self.nodes[base + i] = (h, n.reserved, neg, pos);
                }
                1 => self.nodes[base + i] = (parts[0], n.reserved, neg, pos),
                k => {
                    // a chain on the same plane: head keeps the negative child, the last keeps the positive one
                    let mut next = pos;
                    for j in (1..k).rev() {
                        let id = self.nodes.len() as i32;
                        self.nodes.push((parts[j], 0, NO_NEG, next));
                        next = id;
                    }
                    self.nodes[base + i] = (parts[0], n.reserved, neg, next);
                }
            }
        }
        node_ref(m.root, base)
    }

    /// Inserts a polygon into the tree under `root` the way the ROM's trees hold polygons: down the side its
    /// corners lie on, into both subtrees when it crosses a plane (on the plane: the positive side), as a new
    /// node at each missing child it reaches. Returns the (possibly new) root.
    pub fn insert(&mut self, root: i32, poly: u32, corners: &[V]) -> i32 {
        if root < 0 {
            self.nodes.push((poly, 0, NO_NEG, NO_POS));
            return (self.nodes.len() - 1) as i32;
        }
        let mut stack = vec![root as usize];
        while let Some(ni) = stack.pop() {
            let (np, _, neg, pos) = self.nodes[ni];
            let pl = self.polys[np as usize].1 as usize;
            let (n, d) = self.planes[pl];
            let (mut above, mut below) = (false, false);
            for &c in corners {
                let s = clip::dot(n, c) - d;
                if s > SIDE_EPS {
                    above = true;
                } else if s < -SIDE_EPS {
                    below = true;
                }
            }
            let go_pos = above || !below;
            if below {
                if neg < 0 {
                    self.nodes.push((poly, 0, NO_NEG, NO_POS));
                    let id = (self.nodes.len() - 1) as i32;
                    self.nodes[ni].2 = id;
                } else {
                    stack.push(neg as usize);
                }
            }
            if go_pos {
                if pos < 0 {
                    self.nodes.push((poly, 0, NO_NEG, NO_POS));
                    let id = (self.nodes.len() - 1) as i32;
                    self.nodes[ni].3 = id;
                } else {
                    stack.push(pos as usize);
                }
            }
        }
        root
    }

    /// The kd partition over the regions: `items` are (region, root of its tree), `outside` the root walked
    /// outside every region. Returns the partition's root.
    pub fn partition(&mut self, items: &[(Aabb, i32)], outside: i32) -> i32 {
        let world = Aabb {
            min: [-1.0e9; 3],
            max: [1.0e9; 3],
        };
        let list: Vec<(Aabb, i32)> = items.to_vec();
        self.split(world, list, outside)
    }

    fn split(&mut self, cell: Aabb, items: Vec<(Aabb, i32)>, outside: i32) -> i32 {
        if items.is_empty() {
            return outside;
        }
        // candidate planes: item faces strictly inside the cell
        let mut best: Option<(usize, f32, usize, usize)> = None; // axis, value, boxes cut, imbalance
        for (b, _) in &items {
            for axis in 0..3 {
                for v in [b.min[axis], b.max[axis]] {
                    if !(v > cell.min[axis] && v < cell.max[axis]) {
                        continue;
                    }
                    let mut cut = 0usize;
                    let (mut lo, mut hi) = (0usize, 0usize);
                    for (o, _) in &items {
                        if o.max[axis] <= v {
                            lo += 1;
                        } else if o.min[axis] >= v {
                            hi += 1;
                        } else {
                            cut += 1;
                        }
                    }
                    let score = (cut, lo.abs_diff(hi));
                    if best.is_none_or(|(_, _, c, im)| score < (c, im)) {
                        best = Some((axis, v, score.0, score.1));
                    }
                }
            }
        }
        let Some((axis, v, _, _)) = best else {
            // no face inside the cell: the one item that covers it (regions do not overlap)
            return items[0].1;
        };
        let mut neg_cell = cell;
        neg_cell.max[axis] = v;
        let mut pos_cell = cell;
        pos_cell.min[axis] = v;
        let side = |c: &Aabb| -> Vec<(Aabb, i32)> {
            items
                .iter()
                .filter_map(|(b, r)| b.intersection(c).map(|x| (x, *r)))
                .collect()
        };
        let (neg_items, pos_items) = (side(&neg_cell), side(&pos_cell));
        let neg = self.split(neg_cell, neg_items, outside);
        let pos = self.split(pos_cell, pos_items, outside);
        let mut n = [0.0f32; 3];
        n[axis] = 1.0;
        let pl = self.plane(n, v);
        let h = self.helper(pl);
        self.nodes.push((h, 0, neg, pos));
        (self.nodes.len() - 1) as i32
    }

    /// Bounds of every corner.
    pub fn bounds(&self) -> Option<Aabb> {
        Aabb::of_points(self.points.iter())
    }

    /// The model for the writer.
    pub fn into_synth(self, root: i32) -> SynthModel {
        let (box_min, box_max) = match self.bounds() {
            Some(b) => (b.min, b.max),
            None => ([0.0; 3], [0.0; 3]),
        };
        SynthModel {
            flags: self.flags,
            name: self.name,
            textures: self.textures,
            planes: self.planes,
            surfaces: self.surfaces,
            polys: self.polys,
            nodes: self.nodes,
            points: self.points,
            root,
            box_min,
            box_max,
            origin: self.origin,
        }
    }
}

/// One face of a convex brush: surface flags, texture index, texflags, plane normal and distance, corners.
pub type BrushFace = (u32, u16, u16, V, f32, Vec<V>);

/// A convex brush from faces: one node per face in a chain down the negative (inside) side, which is a valid BSP
/// for a convex volume whose face normals point out.
pub fn convex_brush(
    name: &[u8],
    flags: u32,
    origin: V,
    textures: &[Vec<u8>],
    faces: &[BrushFace],
) -> SynthModel {
    let mut t = Tree::new(flags, name, origin);
    for tx in textures {
        t.texture(tx);
    }
    if t.textures.is_empty() {
        t.texture(b"none");
    }
    let mut ids = Vec::with_capacity(faces.len());
    for (sflags, tex, texflags, n, d, corners) in faces {
        let s = t.surface(*sflags, *tex, *texflags);
        let pl = t.plane(*n, *d);
        ids.push(t.poly(s, pl, corners));
    }
    let k = ids.len();
    for (j, &id) in ids.iter().enumerate() {
        let neg = if j + 1 < k { (j + 1) as i32 } else { NO_NEG };
        t.nodes.push((id, 0, neg, NO_POS));
    }
    let root = if k > 0 { 0 } else { NO_NEG };
    t.into_synth(root)
}
