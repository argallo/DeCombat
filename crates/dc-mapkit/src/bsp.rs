//! The BSP builder of the build pipeline (MAPKIT_PLAN.md D5, 5.3 step 3).
//!
//! Collision in DeCombat walks the node tree of a world model and never its polygon list (`dc-sim/src/collide.rs`
//! `CollModel::sweep`): a node supplies the plane of its polygon, the walk descends one child when the padded
//! swept segment lies on one side of that plane and both otherwise, and it sweeps the node's polygon itself only
//! when the polygon is solid (`SURF_SOLID`). A polygon no node references can never be hit. The reference
//! builder `dc_sim::build_bsp` (first polygon = splitter, straddlers referenced from both subtrees) is correct
//! but explodes on real geometry (hundreds of millions of nodes for one ROM level), so this module builds the
//! same kind of tree in two phases (DC-DEFAULT: MAP-40):
//!
//! 1. **kd phase** (only when [`BspOptions::helpers`] is set): while a cell holds more than
//!    [`BspOptions::cell_polys`] polygons and the kd depth is below [`BspOptions::max_depth`], the cell is split
//!    by an axis-aligned plane. The candidates are quantiles (1/4, 1/2, 3/4) of the polygons' per-axis minimum
//!    and maximum bounds on each of the three axes, so a split lands on a polygon bound; on a terrain grid every
//!    bound is a grid line and no grid triangle straddles the chosen plane. The winner minimises the larger child
//!    (straddlers count on both sides, so this also minimises straddling), ties by fewer straddlers, then by axis
//!    (x, y, z) and position. A split that leaves one child as large as the parent makes no progress and the cell
//!    falls through to phase 2. The split node references a **helper polygon**: 0 corners, flags 0, surface 0, an
//!    exact unit axis normal with `d` at the split. The walk uses it for its plane only (flags 0 = never swept).
//! 2. **cell phase**: a naive polygon BSP over the polygons of the cell. The splitter is the polygon whose plane
//!    straddles the fewest other polygons of the cell (ties: fewest polygons behind it, then input order; at most
//!    [`CANDIDATES`] polygons are scored, evenly strided through the cell, so a huge cell with `helpers: false`
//!    stays polynomial). The plane test is the one of `dc_sim::build_bsp` with [`EPS`] 0.01: a corner farther
//!    than EPS in front counts the polygon as in front, farther than EPS behind as behind; a polygon with corners
//!    on both sides is referenced from BOTH subtrees (no clipping, so polygon indices stay those of the input); a
//!    coplanar polygon goes to the positive side.
//!
//! Node convention: [`BspNode::neg`] / [`BspNode::pos`] are the children behind / in front of the node polygon's
//! plane; a missing negative child is [`NO_NEG`] (-2), a missing positive child [`NO_POS`] (-1); the file's
//! `node_front` field carries the NEGATIVE child ([`file_nodes`], `dc_sim::from_world::FILE_FIRST_CHILD_IS_NEGATIVE`).
//! Helper polygons live in the LEVEL model only (DC-DEFAULT: MAP-41): a ladder volume takes every polygon plane of
//! its brush and a kill volume every polygon with >= 3 corners, so brush models are built with `helpers: false`
//! and get the naive tree.
//!
//! Determinism (MAPKIT_PLAN.md section 7): no hashing, stable sorts, the same input gives the same tree on every
//! platform. The verification suite (DC-DEFAULT: MAP-42) in the tests below compares sweep, ray and overlap
//! results of a `dc_sim::CollModel` built from this tree (through the world FILE: `SynthModel` -> `World::parse`
//! -> `coll_model`) with one built by `dc_sim::build_bsp` over thousands of random queries.

use crate::geom::{Plane, Poly, SURF_SOLID, V3};

/// Sentinel of a missing negative child (the file's `node_front` field). DC-DEFAULT: MAP-40 (dc-sim accepts
/// either sentinel for either child; the ROM convention is -2 in front, -1 in back).
pub const NO_NEG: i32 = -2;
/// Sentinel of a missing positive child (the file's `node_back` field).
pub const NO_POS: i32 = -1;
/// Plane-side tolerance of the polygon classification, the value of `dc_sim::build_bsp`.
// SPEC: dc-sim collide.rs build_bsp (EPS 0.01; corners within EPS of the plane count as on it)
pub const EPS: f32 = 0.01;
/// Polygons scored as splitter candidates per cell (all of them when the cell is smaller).
pub const CANDIDATES: usize = 24;
/// Depth above which [`check`] reports a problem (our own bound; the walk itself has no limit, its stack spills
/// to the heap, and dc_sim::build_bsp truncated at 4096).
pub const DEPTH_LIMIT: usize = 1024;

/// One node: the polygon whose plane splits, and the children behind (`neg`) and in front (`pos`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BspNode {
    /// Index into the input polygons, or `input.len() + i` for helper `i`.
    pub poly: usize,
    /// Child behind the plane, or [`NO_NEG`].
    pub neg: i32,
    /// Child in front of the plane, or [`NO_POS`].
    pub pos: i32,
}

/// The built tree. Node order is pre-order (a node precedes its children); `root` is the first node, or
/// [`NO_POS`] for an empty input.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BspTree {
    pub nodes: Vec<BspNode>,
    pub root: i32,
    /// Helper polygons referenced by `node.poly >= input.len()` as `input.len() + i`.
    pub helpers: Vec<Poly>,
    /// Set when the build stopped at its node budget (DC-DEFAULT: MAP-40): the tree holds the nodes made so
    /// far (structurally sound, but polygons of the unfinished cells are unreferenced) and is NOT fit for a
    /// world file; the build turns it into a `bsp.budget` diagnostic.
    pub budget_exceeded: Option<BudgetHit>,
    /// The naive-phase cell that spent the most nodes: `(its polygons, its nodes)`; `None` for an empty input.
    pub cell_peak: Option<(usize, usize)>,
}

/// Where a build ran past its budget: the naive phase duplicates every straddling polygon into both subtrees
/// without clipping, so k polygons that straddle every candidate plane of one cell (long rotated walls crossing
/// a common area) make about 2^k nodes. A cell may spend at most `max(CELL_BUDGET_MIN, CELL_BUDGET_PER_POLY x
/// its polygons)` nodes and the whole tree at most `TREE_BUDGET_PER_POLY x polygons`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetHit {
    /// The input polygons of the cell whose naive phase blew the budget (the whole input when the tree-wide
    /// budget was hit first).
    pub cell_polys: Vec<usize>,
    /// Nodes in the tree when the build stopped.
    pub nodes: usize,
}

/// Smallest naive-phase node budget of one cell (DC-DEFAULT: MAP-40).
pub const CELL_BUDGET_MIN: usize = 64;
/// Naive-phase node budget of one cell per polygon of the cell (a convex cell makes one node per polygon; the
/// kd phase leaves cells of about `cell_polys`, where a few straddlers double a subtree a few times).
// OBSERVED (the suite with the budget disabled): every box / slope / heightfield set peaks at 16 .. 37 nodes for
// a 13 .. 16 polygon cell; the refuters' passing probe set (a 12 x 12 heightfield, two cylinders and three
// 5,000-unit diagonal walls) peaks at 146 nodes for a 16-polygon cell, so 8 x n would refuse it and 16 x n is
// the budget; the C13 probe (6 .. 20 walls through one point) needs 1,152 .. millions of nodes for 36 .. 120
// polygons and is refused from 6 walls on
pub const CELL_BUDGET_PER_POLY: usize = 16;
/// Node budget of the whole tree per input polygon (the templates measure about 1.1 nodes per polygon; the
/// probe set above 11.9).
pub const TREE_BUDGET_PER_POLY: usize = 16;

/// Builder options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BspOptions {
    /// A cell with at most this many polygons gets the naive BSP (default 16).
    pub cell_polys: usize,
    /// Allow axis-aligned non-solid helper splitters (the level model: true; brush models: false).
    pub helpers: bool,
    /// Maximum number of helper splits on any path (default 64; the naive phase below a cell is bounded by
    /// the cell's polygon count).
    pub max_depth: usize,
}

impl Default for BspOptions {
    fn default() -> Self {
        BspOptions {
            cell_polys: 16,
            helpers: true,
            max_depth: 64,
        }
    }
}

impl BspOptions {
    /// The options of a brush model: no helpers (DC-DEFAULT: MAP-41).
    pub fn brush() -> BspOptions {
        BspOptions {
            helpers: false,
            ..BspOptions::default()
        }
    }
}

/// Summary numbers of a tree (`dc-mapkit build` stats, tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BspStats {
    pub nodes: usize,
    pub helpers: usize,
    pub depth: usize,
    /// Nodes referencing a real (non-helper) polygon.
    pub references: usize,
    /// The largest number of nodes referencing one polygon.
    pub max_references: usize,
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    lo: [f32; 3],
    hi: [f32; 3],
}

fn bounds_of(p: &Poly) -> Bounds {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for c in &p.corners {
        let a = c.to_array();
        for k in 0..3 {
            lo[k] = lo[k].min(a[k]);
            hi[k] = hi[k].max(a[k]);
        }
    }
    Bounds { lo, hi }
}

/// Where a polygon lies relative to a plane: `(behind, in_front)` by the EPS rule; `(false, false)` = coplanar.
fn sides(plane: &Plane, p: &Poly) -> (bool, bool) {
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for c in &p.corners {
        let d = plane.dist(*c);
        lo = lo.min(d);
        hi = hi.max(d);
    }
    (lo < -EPS, hi > EPS)
}

/// Partition `list` by `plane`: coplanar polygons go to the positive side, straddlers to both.
// SPEC: dc-sim collide.rs build_bsp (coplanar -> positive, straddlers in both subtrees)
fn partition(polys: &[Poly], plane: &Plane, list: &[usize]) -> (Vec<usize>, Vec<usize>) {
    let mut neg = Vec::new();
    let mut pos = Vec::new();
    for &i in list {
        let (behind, front) = sides(plane, &polys[i]);
        if behind {
            neg.push(i);
        }
        if front || !behind {
            pos.push(i);
        }
    }
    (neg, pos)
}

/// An axis-aligned split of `list` (phase 1): `(axis, d, neg, pos)`, or `None` when no candidate makes progress.
fn kd_split(
    polys: &[Poly],
    bounds: &[Bounds],
    list: &[usize],
) -> Option<(usize, f32, Vec<usize>, Vec<usize>)> {
    let n = list.len();
    // (larger child, straddlers) to beat; a child as large as the parent is no progress
    let mut best: Option<(usize, usize, usize, f32)> = None;
    let mut vals: Vec<f32> = Vec::with_capacity(n);
    let quantiles = [n / 2, n / 4, (3 * n) / 4];
    for axis in 0..3 {
        for take_hi in [true, false] {
            vals.clear();
            vals.extend(list.iter().map(|&i| {
                if take_hi {
                    bounds[i].hi[axis]
                } else {
                    bounds[i].lo[axis]
                }
            }));
            vals.sort_by(f32::total_cmp);
            for &q in &quantiles {
                let d = vals[q.min(n - 1)];
                if !d.is_finite() {
                    continue;
                }
                let (mut neg, mut pos, mut both) = (0usize, 0usize, 0usize);
                for &i in list {
                    let b = &bounds[i];
                    let behind = b.lo[axis] < d - EPS;
                    let front = b.hi[axis] > d + EPS;
                    if behind {
                        neg += 1;
                    }
                    if front || !behind {
                        pos += 1;
                    }
                    if behind && front {
                        both += 1;
                    }
                }
                let larger = neg.max(pos);
                if larger >= n {
                    continue;
                }
                let better = match best {
                    None => true,
                    Some((l, s, _, _)) => larger < l || (larger == l && both < s),
                };
                if better {
                    best = Some((larger, both, axis, d));
                }
            }
        }
    }
    let (_, _, axis, d) = best?;
    let mut n_axis = [0.0f32; 3];
    n_axis[axis] = 1.0;
    let plane = Plane {
        n: V3::from_array(n_axis),
        d,
    };
    let (neg, pos) = partition(polys, &plane, list);
    Some((axis, d, neg, pos))
}

/// The helper polygon of an axis split: 0 corners, flags 0, surface 0, unit axis plane (DC-DEFAULT: MAP-40).
fn helper(axis: usize, d: f32) -> Poly {
    let mut n = [0.0f32; 3];
    n[axis] = 1.0;
    Poly {
        corners: Vec::new(),
        plane: Plane {
            n: V3::from_array(n),
            d,
        },
        material: 0,
        flags: 0,
        surface: 0,
        uv_scale: 1.0,
        uv_frame: None,
        corner_normals: None,
        corner_uvs: None,
        group: None,
    }
}

/// Position in `list` of the splitter of a cell (phase 2): fewest straddlers, then fewest behind, then first.
fn choose_splitter(polys: &[Poly], list: &[usize]) -> usize {
    let n = list.len();
    if n <= 2 {
        return 0;
    }
    let stride = n.div_ceil(CANDIDATES).max(1);
    let mut best = (usize::MAX, usize::MAX, 0usize);
    for c in (0..n).step_by(stride) {
        let plane = &polys[list[c]].plane;
        let (mut straddle, mut behind) = (0usize, 0usize);
        for (k, &i) in list.iter().enumerate() {
            if k == c {
                continue;
            }
            let (b, f) = sides(plane, &polys[i]);
            if b && f {
                straddle += 1;
            } else if b {
                behind += 1;
            }
            if straddle > best.0 {
                break;
            }
        }
        if (straddle, behind) < (best.0, best.1) {
            best = (straddle, behind, c);
        }
    }
    best.2
}

struct Work {
    list: Vec<usize>,
    /// Node to link the new node into, and whether as its positive child.
    parent: Option<(usize, bool)>,
    /// Helper splits above this cell.
    depth: usize,
    /// The naive-phase cell this item belongs to (index into the builder's cells), once phase 1 has ended.
    cell: Option<usize>,
}

/// One naive-phase cell: its input polygons and the nodes spent on it.
struct Cell {
    polys: Vec<usize>,
    nodes: usize,
    budget: usize,
}

/// Build the tree over `polys` (every polygon, solid or not, is referenced by at least one node, unless the
/// build stopped at its budget: then [`BspTree::budget_exceeded`] is set and the caller refuses the tree).
pub fn build(polys: &[Poly], opts: &BspOptions) -> BspTree {
    let mut tree = BspTree {
        nodes: Vec::with_capacity(polys.len() * 2),
        root: NO_POS,
        helpers: Vec::new(),
        budget_exceeded: None,
        cell_peak: None,
    };
    if polys.is_empty() {
        return tree;
    }
    let cell_polys = opts.cell_polys.max(1);
    let bounds: Vec<Bounds> = if opts.helpers {
        polys.iter().map(bounds_of).collect()
    } else {
        Vec::new()
    };
    let tree_budget = polys.len() * TREE_BUDGET_PER_POLY;
    let mut cells: Vec<Cell> = Vec::new();
    let mut work = vec![Work {
        list: (0..polys.len()).collect(),
        parent: None,
        depth: 0,
        cell: None,
    }];
    while let Some(w) = work.pop() {
        if w.list.is_empty() {
            continue;
        }
        let index = tree.nodes.len();
        if index >= tree_budget {
            tree.budget_exceeded = Some(BudgetHit {
                cell_polys: match w.cell {
                    Some(c) => cells[c].polys.clone(),
                    None => w.list.clone(),
                },
                nodes: index,
            });
            tree.cell_peak = cell_peak(&cells);
            return tree;
        }
        match w.parent {
            None => tree.root = index as i32,
            Some((p, true)) => tree.nodes[p].pos = index as i32,
            Some((p, false)) => tree.nodes[p].neg = index as i32,
        }
        let split = if w.cell.is_none()
            && opts.helpers
            && w.list.len() > cell_polys
            && w.depth < opts.max_depth
        {
            kd_split(polys, &bounds, &w.list)
        } else {
            None
        };
        let (poly, neg, pos, depth, cell) = match split {
            Some((axis, d, neg, pos)) => {
                let h = polys.len() + tree.helpers.len();
                tree.helpers.push(helper(axis, d));
                (h, neg, pos, w.depth + 1, None)
            }
            None => {
                // the naive phase of a cell: opened here when phase 1 ended, else continued
                let cell = w.cell.unwrap_or_else(|| {
                    cells.push(Cell {
                        polys: w.list.clone(),
                        nodes: 0,
                        budget: CELL_BUDGET_MIN.max(w.list.len() * CELL_BUDGET_PER_POLY),
                    });
                    cells.len() - 1
                });
                let c = &mut cells[cell];
                if c.nodes >= c.budget {
                    // unlink the node that was never made, so the partial tree stays structurally sound
                    match w.parent {
                        None => tree.root = NO_POS,
                        Some((p, true)) => tree.nodes[p].pos = NO_POS,
                        Some((p, false)) => tree.nodes[p].neg = NO_NEG,
                    }
                    tree.budget_exceeded = Some(BudgetHit {
                        cell_polys: c.polys.clone(),
                        nodes: index,
                    });
                    tree.cell_peak = cell_peak(&cells);
                    return tree;
                }
                c.nodes += 1;
                let k = choose_splitter(polys, &w.list);
                let poly = w.list[k];
                let rest: Vec<usize> = w
                    .list
                    .iter()
                    .enumerate()
                    .filter(|&(j, _)| j != k)
                    .map(|(_, &i)| i)
                    .collect();
                let (neg, pos) = partition(polys, &polys[poly].plane, &rest);
                (poly, neg, pos, w.depth, Some(cell))
            }
        };
        tree.nodes.push(BspNode {
            poly,
            neg: NO_NEG,
            pos: NO_POS,
        });
        // the negative child is popped (and numbered) first
        work.push(Work {
            list: pos,
            parent: Some((index, true)),
            depth,
            cell,
        });
        work.push(Work {
            list: neg,
            parent: Some((index, false)),
            depth,
            cell,
        });
    }
    tree.cell_peak = cell_peak(&cells);
    tree
}

/// `(polygons, nodes)` of the cell that spent the most nodes.
fn cell_peak(cells: &[Cell]) -> Option<(usize, usize)> {
    cells
        .iter()
        .max_by_key(|c| c.nodes)
        .map(|c| (c.polys.len(), c.nodes))
}

/// The node records of the world file, `(poly, reserved, node_front, node_back)`: the first child field
/// carries the NEGATIVE child (`dc_sim::from_world::FILE_FIRST_CHILD_IS_NEGATIVE`), and the root as the file's
/// `root_node`.
// SPEC: FMT_DAT85.md §5.2 (node record, root_node); DC-DEFAULT: MAP-40 (sentinels -2 front / -1 back)
pub fn file_nodes(tree: &BspTree) -> (Vec<(u32, u16, i32, i32)>, i32) {
    (
        tree.nodes
            .iter()
            .map(|n| (n.poly as u32, 0u16, n.neg, n.pos))
            .collect(),
        tree.root,
    )
}

fn child(c: i32, n: usize) -> Option<usize> {
    usize::try_from(c).ok().filter(|&i| i < n)
}

/// The longest root-to-leaf path in nodes (0 for an empty tree); a cycle counts as the node count.
pub fn depth(tree: &BspTree) -> usize {
    let n = tree.nodes.len();
    let Some(root) = child(tree.root, n) else {
        return 0;
    };
    let mut seen = vec![false; n];
    let mut best = 0;
    let mut stack = vec![(root, 1usize)];
    while let Some((i, d)) = stack.pop() {
        if seen[i] {
            return n;
        }
        seen[i] = true;
        best = best.max(d);
        let nd = tree.nodes[i];
        if let Some(c) = child(nd.neg, n) {
            stack.push((c, d + 1));
        }
        if let Some(c) = child(nd.pos, n) {
            stack.push((c, d + 1));
        }
    }
    best
}

/// Summary numbers of a tree over `n_polys` input polygons.
pub fn stats(tree: &BspTree, n_polys: usize) -> BspStats {
    let mut refs = vec![0usize; n_polys];
    let mut references = 0;
    for nd in &tree.nodes {
        if nd.poly < n_polys {
            refs[nd.poly] += 1;
            references += 1;
        }
    }
    BspStats {
        nodes: tree.nodes.len(),
        helpers: tree.helpers.len(),
        depth: depth(tree),
        references,
        max_references: refs.iter().copied().max().unwrap_or(0),
    }
}

/// Structural check of a tree over `polys`: helper shape, every index in range, the sentinels of the
/// convention, no self reference, every node reachable exactly once from the root (a tree, not a DAG or a
/// cycle), every solid polygon referenced by at least one node, depth within [`DEPTH_LIMIT`]. Empty = fine.
pub fn check(polys: &[Poly], tree: &BspTree) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(hit) = &tree.budget_exceeded {
        problems.push(format!(
            "the build stopped at its node budget ({} nodes over {} polygons; the offending cell holds {} polygons)",
            hit.nodes,
            polys.len(),
            hit.cell_polys.len()
        ));
    }
    let total = polys.len() + tree.helpers.len();
    for (i, h) in tree.helpers.iter().enumerate() {
        let a = h.plane.n.to_array();
        let axis_unit = a.iter().filter(|v| v.abs() == 1.0).count() == 1
            && a.iter().filter(|v| **v == 0.0).count() == 2;
        if !h.corners.is_empty() || h.flags != 0 || h.surface != 0 || !axis_unit {
            problems.push(format!(
                "helper {i}: must have 0 corners, flags 0, surface 0 and a unit axis plane (corners {}, flags {:#x}, surface {}, normal {a:?})",
                h.corners.len(),
                h.flags,
                h.surface
            ));
        }
    }
    let n = tree.nodes.len();
    let mut referenced = vec![false; total];
    let mut indegree = vec![0usize; n];
    for (i, nd) in tree.nodes.iter().enumerate() {
        if nd.poly >= total {
            problems.push(format!(
                "node {i}: polygon {} out of range ({total} polygons incl. helpers)",
                nd.poly
            ));
        } else {
            referenced[nd.poly] = true;
        }
        for (c, sentinel, name) in [(nd.neg, NO_NEG, "neg"), (nd.pos, NO_POS, "pos")] {
            if c == sentinel {
                continue;
            }
            match child(c, n) {
                Some(j) if j == i => problems.push(format!("node {i}: {name} child is itself")),
                Some(j) => indegree[j] += 1,
                None => problems.push(format!(
                    "node {i}: {name} child {c} out of range (sentinel is {sentinel})"
                )),
            }
        }
    }
    match child(tree.root, n) {
        Some(r) => {
            if indegree[r] != 0 {
                problems.push(format!("root {r} is also a child"));
            }
            for (j, &d) in indegree.iter().enumerate() {
                if j != r && d != 1 {
                    problems.push(format!(
                        "node {j} is referenced {d} times (every node but the root exactly once)"
                    ));
                }
            }
        }
        None if n == 0 && tree.root == NO_POS => {}
        None => problems.push(format!("root {} out of range ({n} nodes)", tree.root)),
    }
    let unreferenced: Vec<usize> = polys
        .iter()
        .enumerate()
        .filter(|(i, p)| p.flags & SURF_SOLID != 0 && !referenced[*i])
        .map(|(i, _)| i)
        .collect();
    if !unreferenced.is_empty() {
        problems.push(format!(
            "{} solid polygons are referenced by no node (first: {:?})",
            unreferenced.len(),
            &unreferenced[..unreferenced.len().min(8)]
        ));
    }
    let d = depth(tree);
    if d > DEPTH_LIMIT {
        problems.push(format!("depth {d} exceeds {DEPTH_LIMIT}"));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{solid_polys, terrain_polys, unit_circle, Surface};
    use crate::project::{Heights, Solid, SolidKind, Terrain};
    use dc_sim::from_world::{collision_world, WorldFilter};
    use dc_sim::{CollModel, Collider, CollisionWorld, ConvexPoly, Cylinder, Hit};
    use dc_world::synth::{SynthModel, SynthWorld};
    use dc_world::World;

    /// xorshift, as in the dc-sim equality tests.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
        fn range(&mut self, a: f32, b: f32) -> f32 {
            a + (b - a) * self.next()
        }
        fn v3(&mut self, lo: V3, hi: V3) -> V3 {
            V3::new(
                self.range(lo.x, hi.x),
                self.range(lo.y, hi.y),
                self.range(lo.z, hi.z),
            )
        }
    }

    fn surf(flags: u32) -> Surface {
        Surface {
            material: 0,
            flags,
            surface: 0,
            uv_scale: 128.0,
            fit: None,
        }
    }

    fn face(corners: &[V3], outward: V3, flags: u32) -> Poly {
        Poly::new(corners.to_vec(), outward, surf(flags), None).expect("a proper face")
    }

    /// The six outward faces of a box, every one solid, the top one also carrying `top_flags`.
    fn box_faces(lo: V3, hi: V3, top_flags: u32) -> Vec<Poly> {
        let c = V3::new;
        let (a, b) = (lo, hi);
        vec![
            face(
                &[
                    c(a.x, b.y, a.z),
                    c(a.x, b.y, b.z),
                    c(b.x, b.y, b.z),
                    c(b.x, b.y, a.z),
                ],
                V3::UP,
                SURF_SOLID | top_flags,
            ),
            face(
                &[
                    c(a.x, a.y, a.z),
                    c(b.x, a.y, a.z),
                    c(b.x, a.y, b.z),
                    c(a.x, a.y, b.z),
                ],
                -V3::UP,
                SURF_SOLID,
            ),
            face(
                &[
                    c(a.x, a.y, a.z),
                    c(a.x, a.y, b.z),
                    c(a.x, b.y, b.z),
                    c(a.x, b.y, a.z),
                ],
                c(-1.0, 0.0, 0.0),
                SURF_SOLID,
            ),
            face(
                &[
                    c(b.x, a.y, a.z),
                    c(b.x, b.y, a.z),
                    c(b.x, b.y, b.z),
                    c(b.x, a.y, b.z),
                ],
                c(1.0, 0.0, 0.0),
                SURF_SOLID,
            ),
            face(
                &[
                    c(a.x, a.y, a.z),
                    c(a.x, b.y, a.z),
                    c(b.x, b.y, a.z),
                    c(b.x, a.y, a.z),
                ],
                c(0.0, 0.0, -1.0),
                SURF_SOLID,
            ),
            face(
                &[
                    c(a.x, a.y, b.z),
                    c(b.x, a.y, b.z),
                    c(b.x, b.y, b.z),
                    c(a.x, b.y, b.z),
                ],
                c(0.0, 0.0, 1.0),
                SURF_SOLID,
            ),
        ]
    }

    /// A one-sided ramp quad rising along +z from `z0` (height `y0`) to `z1` (height `y1`), facing up.
    fn ramp(x0: f32, x1: f32, z0: f32, z1: f32, y0: f32, y1: f32) -> Poly {
        face(
            &[
                V3::new(x0, y0, z0),
                V3::new(x0, y1, z1),
                V3::new(x1, y1, z1),
                V3::new(x1, y0, z0),
            ],
            V3::UP,
            SURF_SOLID,
        )
    }

    fn floor(x0: f32, x1: f32, z0: f32, z1: f32, y: f32) -> Poly {
        ramp(x0, x1, z0, z1, y, y)
    }

    /// A heightfield of `cells x cells` through the real terrain generator (the grid convention of the build).
    fn heightfield(cells: u32, cell_size: f32, amplitude: f32, seed: u64) -> Vec<Poly> {
        let t = Terrain {
            cells_x: cells,
            cells_z: cells,
            cell_size,
            origin: [
                -(cells as f32) * cell_size * 0.5,
                -128.0,
                -(cells as f32) * cell_size * 0.5,
            ],
            height_scale: amplitude.max(1.0),
            ..Terrain::default()
        };
        let mut h = Heights::for_terrain(&t, 0);
        let mut rng = Rng(seed | 1);
        for z in 0..t.rows() {
            for x in 0..t.cols() {
                // smooth-ish: a ridge pattern plus noise, scaled into the raw range
                let fx = x as f32 / t.cols() as f32;
                let fz = z as f32 / t.rows() as f32;
                let base = 0.5 + 0.3 * (fx * 6.0).sin() * (fz * 4.0).cos();
                let v = (base + rng.range(-0.15, 0.15)).clamp(0.0, 1.0);
                h.set(x, z, (v * 65535.0) as u16);
            }
        }
        terrain_polys(&t, &h, &|_, _| surf(SURF_SOLID))
    }

    fn random_boxes(rng: &mut Rng, count: usize, snap: bool) -> Vec<Poly> {
        let mut out = Vec::new();
        for _ in 0..count {
            let mut lo = rng.v3(
                V3::new(-2000.0, -200.0, -2000.0),
                V3::new(2000.0, 400.0, 2000.0),
            );
            let size = rng.v3(V3::new(4.0, 4.0, 4.0), V3::new(500.0, 300.0, 500.0));
            if snap {
                lo = V3::new(
                    (lo.x / 64.0).round() * 64.0,
                    (lo.y / 64.0).round() * 64.0,
                    (lo.z / 64.0).round() * 64.0,
                );
            }
            let flags = if rng.next() < 0.3 { 0x10 } else { 0 };
            out.extend(box_faces(lo, lo + size, flags));
        }
        out
    }

    /// Floors, one-sided ramps of several pitches (walkable and steep), a wall and a few blocks.
    fn slopes() -> Vec<Poly> {
        let mut p = vec![floor(-3000.0, 3000.0, -3000.0, 3000.0, 0.0)];
        p.push(ramp(-1000.0, -600.0, 300.0, 800.0, 0.0, 288.0));
        p.push(floor(-1000.0, -600.0, 800.0, 2000.0, 288.0));
        p.push(ramp(0.0, 400.0, 300.0, 400.0, 0.0, 173.0));
        p.push(floor(0.0, 400.0, 400.0, 2000.0, 173.0));
        p.push(ramp(500.0, 900.0, -2000.0, -300.0, 0.0, 600.0));
        // a ramp that faces down (a sloped ceiling)
        p.push(face(
            &[
                V3::new(1200.0, 200.0, -500.0),
                V3::new(1200.0, 500.0, 500.0),
                V3::new(1800.0, 500.0, 500.0),
                V3::new(1800.0, 200.0, -500.0),
            ],
            -V3::UP,
            SURF_SOLID,
        ));
        p.push(face(
            &[
                V3::new(-3000.0, 0.0, 1500.0),
                V3::new(-3000.0, 2000.0, 1500.0),
                V3::new(3000.0, 2000.0, 1500.0),
                V3::new(3000.0, 0.0, 1500.0),
            ],
            V3::new(0.0, 0.0, -1.0),
            SURF_SOLID,
        ));
        p.extend(box_faces(
            V3::new(-200.0, 0.0, -1200.0),
            V3::new(200.0, 160.0, -800.0),
            0,
        ));
        // a non-solid decoration face that must still be a valid split plane
        p.push(face(
            &[
                V3::new(-150.0, 1.0, -1700.0),
                V3::new(-150.0, 1.0, -1500.0),
                V3::new(150.0, 1.0, -1500.0),
                V3::new(150.0, 1.0, -1700.0),
            ],
            V3::UP,
            0,
        ));
        p
    }

    fn convex(polys: &[Poly]) -> Vec<ConvexPoly> {
        polys
            .iter()
            .map(|p| ConvexPoly {
                normal: dc_sim::V3::from_array(p.plane.n.to_array()),
                dist: p.plane.d,
                verts: p
                    .corners
                    .iter()
                    .map(|c| dc_sim::V3::from_array(c.to_array()))
                    .collect(),
                flags: p.flags,
            })
            .collect()
    }

    /// Our tree through the world FILE: SynthModel -> World::parse -> check_layout -> coll_model.
    fn through_file(polys: &[Poly], tree: &BspTree) -> CollisionWorld {
        let mut m = SynthModel {
            flags: 0,
            name: b"level".to_vec(),
            textures: vec![b"none".to_vec()],
            ..SynthModel::default()
        };
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in polys.iter().chain(tree.helpers.iter()) {
            let s = (p.flags, 0u16, p.surface);
            let si = match m.surfaces.iter().position(|x| *x == s) {
                Some(i) => i,
                None => {
                    m.surfaces.push(s);
                    m.surfaces.len() - 1
                }
            };
            m.planes.push((p.plane.n.to_array(), p.plane.d));
            let first = m.points.len() as u32;
            for c in &p.corners {
                let a = c.to_array();
                for k in 0..3 {
                    lo[k] = lo[k].min(a[k]);
                    hi[k] = hi[k].max(a[k]);
                }
                m.points.push(a);
            }
            m.polys.push((
                si as u32,
                (m.planes.len() - 1) as u32,
                (first..first + p.corners.len() as u32).collect(),
            ));
        }
        let (nodes, root) = file_nodes(tree);
        m.nodes = nodes;
        m.root = root;
        m.box_min = lo;
        m.box_max = hi;
        let bytes = SynthWorld {
            extent_min: lo,
            extent_max: hi,
            models: vec![m],
            tree_shape: vec![false],
            grid_dims: [1, 1, 1],
            grid_rgb: vec![0, 0, 0],
            ..SynthWorld::default()
        }
        .build();
        let w = World::parse(&bytes).expect("the synthetic world parses");
        w.check_layout().expect("layout");
        assert_eq!(w.models[0].polygons.len(), polys.len() + tree.helpers.len());
        let cw = collision_world(&w, &WorldFilter::default());
        assert_eq!(cw.models.len(), 1, "the unnamed model is the level");
        cw
    }

    fn reference(polys: &[Poly]) -> CollisionWorld {
        CollisionWorld::from_parts(vec![CollModel::from_polys(&convex(polys))], &[])
    }

    fn extent(polys: &[Poly]) -> (V3, V3) {
        let mut lo = V3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut hi = V3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for p in polys {
            for c in &p.corners {
                lo = lo.min(*c);
                hi = hi.max(*c);
            }
        }
        (lo, hi)
    }

    /// Hits agree: the same presence and, for a hit, the same instant. Two faces reached at the same raw time
    /// (a shared edge or vertex of two terrain triangles, two faces of one plane, a start already overlapping
    /// geometry) are reported in walk order: the first one visited wins the strict `<` of the walk, and the
    /// 0.1-unit back-off `CollisionWorld::sweep` then applies along the contact normal makes the reported `t`
    /// of the two differ in the last digits. Such a hit, within half a unit along the move (`|dt| * len <=
    /// 0.5`), is `Ok(false)` (a tie, counted and bounded by the caller); anything farther apart is a real
    /// difference.
    fn same_hit(a: Option<Hit>, b: Option<Hit>, len: f32) -> Result<bool, String> {
        match (a, b) {
            (None, None) => Ok(true),
            (Some(a), Some(b)) => {
                if (a.t - b.t).abs() * len > 0.5 {
                    return Err(format!("{a:?} vs {b:?}"));
                }
                Ok(a == b)
            }
            (a, b) => Err(format!("{a:?} vs {b:?}")),
        }
    }

    /// The sweep-equality suite over one polygon set (DC-DEFAULT: MAP-42): random body sweeps, rays and
    /// overlap tests against our file tree and the reference tree.
    fn sweep_equality(
        name: &str,
        polys: &[Poly],
        opts: &BspOptions,
        queries: usize,
        seed: u64,
    ) -> usize {
        let tree = build(polys, opts);
        let problems = check(polys, &tree);
        assert!(problems.is_empty(), "{name}: {problems:?}");
        let ours = through_file(polys, &tree);
        let refw = reference(polys);
        let (lo, hi) = extent(polys);
        let pad = V3::new(300.0, 300.0, 300.0);
        let (lo, hi) = (lo - pad, hi + pad);
        let shapes = [
            Cylinder::from_dims(dc_sim::V3::new(16.0, 40.0, 16.0)),
            Cylinder::from_dims(dc_sim::V3::new(8.0, 12.0, 8.0)),
            Cylinder::from_dims(dc_sim::V3::new(40.0, 20.0, 40.0)),
        ];
        let mut rng = Rng(seed);
        let (mut hits, mut ties, mut stuck, mut overlaps) = (0usize, 0usize, 0usize, 0usize);
        for q in 0..queries {
            let from = rng.v3(lo, hi);
            let dir = rng.v3(V3::new(-1.0, -1.0, -1.0), V3::new(1.0, 1.0, 1.0));
            let reach = match q % 4 {
                0 => rng.range(0.5, 8.0),
                1 => rng.range(8.0, 200.0),
                2 => rng.range(200.0, 1500.0),
                _ => 9000.0,
            };
            let to = from + dir * reach;
            let (f, t) = (
                dc_sim::V3::from_array(from.to_array()),
                dc_sim::V3::from_array(to.to_array()),
            );
            let shape = &shapes[q % shapes.len()];
            let len = (to - from).length();
            let mut tally = |a: Option<Hit>, b: Option<Hit>, what: &str| match same_hit(a, b, len) {
                Ok(exact) => match a {
                    Some(h) if h.t == 0.0 => stuck += 1,
                    Some(_) => {
                        hits += 1;
                        if !exact {
                            ties += 1;
                        }
                    }
                    None => {}
                },
                Err(e) => panic!("{name}: {what} {from:?} -> {to:?} ({shape:?}): {e}"),
            };
            tally(
                ours.sweep(shape, f, t),
                refw.sweep(shape, f, t),
                "body sweep",
            );
            tally(ours.ray(f, t), refw.ray(f, t), "ray");
            let (a, b) = (ours.overlaps(shape, f), refw.overlaps(shape, f));
            assert_eq!(a, b, "{name}: overlap at {from:?} ({shape:?})");
            if a {
                overlaps += 1;
            }
        }
        let st = stats(&tree, polys.len());
        eprintln!(
            "{name}: {} polys, {} nodes ({} helpers), depth {}, max refs {}, peak cell {:?}, {hits} hits, {stuck} stuck starts, {overlaps} overlaps, {ties} ties over {queries} queries",
            polys.len(),
            st.nodes,
            st.helpers,
            st.depth,
            st.max_references,
            tree.cell_peak
        );
        assert!(
            hits >= queries / 50,
            "{name}: too few hits to mean anything: {hits}"
        );
        // DC-DEFAULT: MAP-42 (ties at most 5 % of hits: on a heightfield every edge is shared)
        assert!(
            ties * 20 <= hits,
            "{name}: too many same-time ties: {ties} of {hits}"
        );
        hits
    }

    #[test]
    fn sweep_equality_random_boxes() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for world in 0..4 {
            let polys = random_boxes(&mut rng, 40, world % 2 == 0);
            sweep_equality(
                &format!("boxes {world}"),
                &polys,
                &BspOptions::default(),
                2500,
                0x1234_5678 + world as u64,
            );
        }
    }

    #[test]
    fn sweep_equality_slopes_including_a_one_sided_ramp() {
        let polys = slopes();
        sweep_equality("slopes", &polys, &BspOptions::default(), 4000, 42);
        // a cell size of 2 forces many helper splits over the same geometry
        sweep_equality(
            "slopes small cells",
            &polys,
            &BspOptions {
                cell_polys: 2,
                ..BspOptions::default()
            },
            2000,
            43,
        );
        // the one-sided ramp alone: hits from above, nothing from below
        let one = vec![ramp(-400.0, 400.0, -400.0, 400.0, 0.0, 300.0)];
        sweep_equality("one-sided ramp", &one, &BspOptions::default(), 2000, 44);
        let tree = build(&one, &BspOptions::default());
        let w = through_file(&one, &tree);
        let down = w.ray(
            dc_sim::V3::new(0.0, 500.0, 0.0),
            dc_sim::V3::new(0.0, -500.0, 0.0),
        );
        assert!(down.is_some_and(|h| h.face_normal.y > 0.7), "{down:?}");
        let up = w.ray(
            dc_sim::V3::new(0.0, -500.0, 0.0),
            dc_sim::V3::new(0.0, 500.0, 0.0),
        );
        assert_eq!(up, None, "the back of a one-sided face is not hit");
    }

    #[test]
    fn sweep_equality_heightfield() {
        let polys = heightfield(32, 64.0, 400.0, 7);
        assert_eq!(polys.len(), 32 * 32 * 2);
        sweep_equality("heightfield 32", &polys, &BspOptions::default(), 3000, 99);
        // heightfield plus blocks on it (a level with terrain and solids)
        let mut mixed = polys.clone();
        let mut rng = Rng(5);
        mixed.extend(random_boxes(&mut rng, 20, true));
        sweep_equality(
            "heightfield + boxes",
            &mixed,
            &BspOptions::default(),
            3000,
            100,
        );
    }

    #[test]
    fn brush_trees_without_helpers_agree_too() {
        let polys = slopes();
        sweep_equality("slopes naive", &polys, &BspOptions::brush(), 2000, 7);
        let b = box_faces(V3::new(0.0, 0.0, 0.0), V3::new(64.0, 96.0, 64.0), 0);
        sweep_equality("one box naive", &b, &BspOptions::brush(), 2000, 8);
    }

    #[test]
    fn every_polygon_is_referenced_and_no_helper_without_the_option() {
        let polys = slopes();
        let naive = build(&polys, &BspOptions::brush());
        assert!(naive.helpers.is_empty());
        assert!(naive.nodes.iter().all(|n| n.poly < polys.len()));
        assert!(check(&polys, &naive).is_empty());
        let st = stats(&naive, polys.len());
        assert_eq!(st.references, naive.nodes.len());
        let hf = heightfield(16, 64.0, 300.0, 3);
        for opts in [BspOptions::default(), BspOptions::brush()] {
            let tree = build(&hf, &opts);
            let st = stats(&tree, hf.len());
            assert!(check(&hf, &tree).is_empty());
            let mut seen = vec![false; hf.len()];
            for n in &tree.nodes {
                if n.poly < hf.len() {
                    seen[n.poly] = true;
                }
            }
            assert!(
                seen.iter().all(|s| *s),
                "every polygon referenced ({opts:?})"
            );
            assert_eq!(st.helpers == 0, !opts.helpers);
        }
    }

    #[test]
    fn heightfield_64_tree_shape() {
        let polys = heightfield(64, 64.0, 500.0, 11);
        assert_eq!(polys.len(), 8192);
        let tree = build(&polys, &BspOptions::default());
        assert!(check(&polys, &tree).is_empty());
        let st = stats(&tree, polys.len());
        eprintln!("heightfield 64: {st:?}");
        // DC-DEFAULT: MAP-40 (nodes <= 2.5x polygons, depth within 300 on a 64x64 heightfield)
        assert!(
            st.nodes * 2 <= polys.len() * 5,
            "{} nodes for {} polygons",
            st.nodes,
            polys.len()
        );
        assert!(st.depth <= 300, "depth {}", st.depth);
        // grid triangles never straddle a helper plane: every x / z helper splits on a grid line (64 units)
        for h in &tree.helpers {
            let axis = h.plane.n.to_array();
            if axis[0] == 1.0 || axis[2] == 1.0 {
                assert_eq!(
                    (h.plane.d / 64.0).fract(),
                    0.0,
                    "helper at {} is off the grid",
                    h.plane.d
                );
            }
        }
    }

    #[test]
    fn fifty_thousand_triangles_build_quickly() {
        let polys = heightfield(158, 32.0, 600.0, 23);
        assert!(polys.len() >= 49_000, "{}", polys.len());
        let t0 = std::time::Instant::now();
        let tree = build(&polys, &BspOptions::default());
        let dt = t0.elapsed();
        let st = stats(&tree, polys.len());
        eprintln!(
            "{} triangles: {} nodes ({} helpers), depth {}, built in {:.3} s",
            polys.len(),
            st.nodes,
            st.helpers,
            st.depth,
            dt.as_secs_f64()
        );
        assert!(check(&polys, &tree).is_empty());
        assert!(st.nodes * 2 <= polys.len() * 5);
        // DC-DEFAULT: MAP-42 (< 2 s in debug is the target; the assertion is generous for slow machines)
        assert!(dt.as_secs_f64() < 20.0, "build took {dt:?}");
    }

    #[test]
    fn deterministic_and_pre_ordered() {
        let polys = slopes();
        let a = build(&polys, &BspOptions::default());
        let b = build(&polys, &BspOptions::default());
        assert_eq!(a, b);
        assert_eq!(a.root, 0);
        for (i, n) in a.nodes.iter().enumerate() {
            for c in [n.neg, n.pos] {
                if let Some(j) = child(c, a.nodes.len()) {
                    assert!(j > i, "pre-order: child {j} after parent {i}");
                }
            }
        }
        let (nodes, root) = file_nodes(&a);
        assert_eq!(root, 0);
        assert_eq!(nodes[0].0, a.nodes[0].poly as u32);
        assert_eq!(nodes[0].2, a.nodes[0].neg, "node_front = negative child");
        assert_eq!(nodes[0].3, a.nodes[0].pos, "node_back = positive child");
    }

    #[test]
    fn empty_and_single_inputs() {
        let empty = build(&[], &BspOptions::default());
        assert_eq!(empty.root, NO_POS);
        assert!(empty.nodes.is_empty());
        assert!(check(&[], &empty).is_empty());
        let one = vec![floor(-10.0, 10.0, -10.0, 10.0, 0.0)];
        let tree = build(&one, &BspOptions::default());
        assert_eq!(
            tree.nodes,
            vec![BspNode {
                poly: 0,
                neg: NO_NEG,
                pos: NO_POS
            }]
        );
        assert!(check(&one, &tree).is_empty());
    }

    #[test]
    fn check_catches_broken_trees() {
        let polys = box_faces(V3::new(0.0, 0.0, 0.0), V3::new(10.0, 10.0, 10.0), 0);
        let good = build(&polys, &BspOptions::brush());
        assert!(check(&polys, &good).is_empty());
        // a polygon no node references
        let mut t = good.clone();
        t.nodes.truncate(1);
        let p = check(&polys, &t);
        assert!(
            p.iter().any(|m| m.contains("referenced by no node")),
            "{p:?}"
        );
        // self reference, out of range, shared child, bad root, swapped sentinels
        let mut t = good.clone();
        t.nodes[1].neg = 1;
        assert!(check(&polys, &t).iter().any(|m| m.contains("itself")));
        let mut t = good.clone();
        t.nodes[0].pos = 99;
        assert!(check(&polys, &t).iter().any(|m| m.contains("out of range")));
        let mut t = good.clone();
        t.nodes[0].neg = t.nodes[0].pos;
        assert!(check(&polys, &t)
            .iter()
            .any(|m| m.contains("referenced 2 times") || m.contains("referenced 0 times")));
        let mut t = good.clone();
        t.root = 77;
        assert!(check(&polys, &t).iter().any(|m| m.contains("root")));
        let mut t = good.clone();
        let leaf = t
            .nodes
            .iter()
            .position(|n| n.neg == NO_NEG && n.pos == NO_POS)
            .unwrap();
        t.nodes[leaf].neg = NO_POS;
        assert!(check(&polys, &t)
            .iter()
            .any(|m| m.contains("sentinel is -2")));
        // a malformed helper
        let mut t = good.clone();
        t.helpers.push(floor(0.0, 1.0, 0.0, 1.0, 0.0));
        assert!(check(&polys, &t).iter().any(|m| m.contains("helper 0")));
        // a cycle
        let mut t = good.clone();
        let last = t.nodes.len() - 1;
        t.nodes[last].pos = 0;
        let p = check(&polys, &t);
        assert!(
            p.iter().any(|m| m.contains("root 0 is also a child")),
            "{p:?}"
        );
        assert_eq!(depth(&t), t.nodes.len());
    }

    #[test]
    fn helpers_split_coplanar_faces_to_the_positive_side() {
        // two floors meeting at x = 0 and a wall standing exactly on x = 0: the kd split at x = 0 puts the
        // wall (coplanar) on the positive side only, so a sweep from the +x side still finds it
        let polys = vec![
            floor(-1000.0, 0.0, -1000.0, 1000.0, 0.0),
            floor(0.0, 1000.0, -1000.0, 1000.0, 0.0),
            face(
                &[
                    V3::new(0.0, 0.0, -500.0),
                    V3::new(0.0, 200.0, -500.0),
                    V3::new(0.0, 200.0, 500.0),
                    V3::new(0.0, 0.0, 500.0),
                ],
                V3::new(1.0, 0.0, 0.0),
                SURF_SOLID,
            ),
        ];
        let opts = BspOptions {
            cell_polys: 1,
            ..BspOptions::default()
        };
        let tree = build(&polys, &opts);
        assert!(!tree.helpers.is_empty());
        assert!(check(&polys, &tree).is_empty());
        sweep_equality("coplanar wall", &polys, &opts, 2000, 5);
    }

    /// The faces of a project solid (box, wedge, cylinder, prism) at `pos`, `size`, rotated `rot` about y.
    fn solid(kind: SolidKind, pos: [f32; 3], size: [f32; 3], rot: f32) -> Vec<Poly> {
        let s = Solid {
            id: "s".into(),
            kind,
            pos,
            size,
            rot_y_deg: rot,
            material: "m".into(),
            ..Default::default()
        };
        solid_polys(&s, &|_| surf(SURF_SOLID)).unwrap()
    }

    /// `n` long thin walls through the origin, rotated 180 / n degrees apart: every wall straddles every
    /// candidate axis plane, so the kd phase makes no progress and the naive phase duplicates each wall into
    /// both subtrees of every other (the C13 probe: 14 walls made 6.5 M nodes, 20 ran past 10 minutes).
    fn spanning_walls(n: usize) -> Vec<Poly> {
        let mut polys = Vec::new();
        for k in 0..n {
            polys.extend(solid(
                SolidKind::Box,
                [0.0, 300.0, 0.0],
                [5000.0, 600.0, 8.0],
                180.0 * k as f32 / n as f32 + 1.0,
            ));
        }
        polys
    }

    #[test]
    fn mutually_crossing_rotated_walls_stop_at_the_budget_instead_of_hanging() {
        // DC-DEFAULT: MAP-40 (the budget): 20 walls = 120 polygons answer within a second with the hit
        let polys = spanning_walls(20);
        let t0 = std::time::Instant::now();
        let tree = build(&polys, &BspOptions::default());
        let dt = t0.elapsed();
        let hit = tree
            .budget_exceeded
            .clone()
            .expect("the budget stops the build");
        assert!(dt.as_secs_f64() < 5.0, "took {dt:?}");
        assert!(
            tree.nodes.len() <= polys.len() * TREE_BUDGET_PER_POLY,
            "{} nodes",
            tree.nodes.len()
        );
        assert_eq!(hit.nodes, tree.nodes.len());
        assert!(!hit.cell_polys.is_empty());
        assert!(hit.cell_polys.iter().all(|&i| i < polys.len()));
        // the partial tree is structurally sound apart from the budget line and the unreferenced polygons
        let problems = check(&polys, &tree);
        assert!(
            problems.iter().any(|m| m.contains("node budget")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .all(|m| m.contains("node budget") || m.contains("referenced by no node")),
            "{problems:?}"
        );
        // the same geometry without helpers (a brush) hits the budget too
        let naive = build(&polys, &BspOptions::brush());
        assert!(naive.budget_exceeded.is_some());
        // a few crossing walls stay well within the budget and verify against the reference builder
        let few = spanning_walls(4);
        let tree = build(&few, &BspOptions::default());
        assert!(
            tree.budget_exceeded.is_none(),
            "{:?}",
            stats(&tree, few.len())
        );
        sweep_equality("4 spanning walls", &few, &BspOptions::default(), 2000, 9);
    }

    #[test]
    fn sweep_equality_rotated_solids() {
        // DC-DEFAULT: MAP-42 (the refuters' probe: rotated boxes, 0.5-unit slabs, 24-sided cylinders, wedges,
        // prisms and spanning diagonals gave 0 mismatches)
        let mut rng = Rng(77);
        let mut polys = solid(
            SolidKind::Box,
            [0.0, -32.0, 0.0],
            [6000.0, 64.0, 6000.0],
            0.0,
        );
        for i in 0..10 {
            let pos = [
                rng.range(-2500.0, 2500.0),
                rng.range(0.0, 200.0),
                rng.range(-2500.0, 2500.0),
            ];
            let size = [
                rng.range(32.0, 600.0),
                rng.range(32.0, 300.0),
                rng.range(32.0, 600.0),
            ];
            polys.extend(solid(
                SolidKind::Box,
                pos,
                size,
                rng.range(0.0, 360.0) + i as f32,
            ));
        }
        sweep_equality("rotated boxes", &polys, &BspOptions::default(), 2500, 1);

        let mut polys = solid(
            SolidKind::Box,
            [0.0, -32.0, 0.0],
            [6000.0, 64.0, 6000.0],
            0.0,
        );
        for i in 0..8 {
            let a = i as f32 * 12.0 + 7.0;
            polys.extend(solid(
                SolidKind::Box,
                [
                    rng.range(-2000.0, 2000.0),
                    100.0,
                    rng.range(-2000.0, 2000.0),
                ],
                [1.0, 200.0, 900.0],
                a,
            ));
            polys.extend(solid(
                SolidKind::Box,
                [rng.range(-2000.0, 2000.0), 0.3, rng.range(-2000.0, 2000.0)],
                [700.0, 0.5, 700.0],
                a * 3.0,
            ));
        }
        sweep_equality(
            "thin walls and slabs",
            &polys,
            &BspOptions::default(),
            2500,
            2,
        );

        let mut polys = heightfield(12, 256.0, 500.0, 5);
        polys.extend(solid(
            SolidKind::Cylinder {
                sides: 24,
                circle: unit_circle(24),
            },
            [0.0, 200.0, 0.0],
            [1800.0, 400.0, 1800.0],
            0.0,
        ));
        polys.extend(solid(
            SolidKind::Cylinder {
                sides: 16,
                circle: unit_circle(16),
            },
            [900.0, 100.0, -900.0],
            [600.0, 300.0, 600.0],
            15.0,
        ));
        for k in 0..3 {
            polys.extend(solid(
                SolidKind::Box,
                [0.0, 300.0, 0.0],
                [5000.0, 700.0, 16.0],
                30.0 * k as f32 + 10.0,
            ));
        }
        sweep_equality(
            "heightfield, cylinders, spanning diagonals",
            &polys,
            &BspOptions::default(),
            2500,
            3,
        );

        let mut polys = solid(
            SolidKind::Box,
            [0.0, -32.0, 0.0],
            [4000.0, 64.0, 4000.0],
            0.0,
        );
        for i in 0..8 {
            polys.extend(solid(
                SolidKind::Wedge,
                [rng.range(-1800.0, 1800.0), 48.0, rng.range(-1800.0, 1800.0)],
                [192.0, 96.0, 192.0],
                i as f32 * 23.0,
            ));
            polys.extend(solid(
                SolidKind::Prism {
                    points: vec![
                        [-40.0, -40.0],
                        [40.0, -40.0],
                        [60.0, 0.0],
                        [40.0, 40.0],
                        [-40.0, 40.0],
                    ],
                },
                [rng.range(-1800.0, 1800.0), 50.0, rng.range(-1800.0, 1800.0)],
                [100.0, 100.0, 100.0],
                i as f32 * 41.0,
            ));
        }
        sweep_equality("wedges and prisms", &polys, &BspOptions::default(), 2500, 6);
    }
}
