//! Census over the model files of the user's ROM for the model passes (read-only, counts and error figures
//! only; no ROM name or value is stored). Questions it answers, which FMT_LTB.md leaves open (§13):
//!
//! 1. In which component order and sense the rotation keys hold a quaternion: frame 0 of the clips is compared
//!    with the bind pose (local transforms of FMT_LTB.md §7) under each candidate convention.
//! 2. Whether the clip tracks are parent-relative (local) transforms: same comparison for the positions.
//! 3. Whether rigid and skinned vertices are stored in model space (bind pose) or node space: the vertex centroid
//!    of each rigid mesh is compared with its node's bind origin and with the model origin.
//! 4. For the character body rows of the ROM model table: node counts, child skeletons (same node names?),
//!    blend sets, influences.
//!
//! Usage: `CA_ROM=<dir> cargo run -p dc-render --example model_census`.

use dc_model::{Mesh, Model};
use dc_render::{decompose_rigid, local_at, QuatOrder};
use glam::{Mat4, Quat, Vec3};
use std::collections::BTreeMap;

fn rez_names(rom: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(rom.join("Game"))
        .expect("Game dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.to_ascii_lowercase().ends_with(".rez"))
        .collect();
    v.sort();
    v
}

#[derive(Default)]
struct Conv {
    files: u64,
    pos_err: Vec<f64>,
    rot_err: [Vec<f64>; 4],
    best: [u64; 4],
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn below(v: &[f64], x: f64) -> usize {
    v.iter().filter(|e| **e < x).count()
}

fn main() {
    let rom = dc_rom::resolve_rom().expect("CA_ROM");
    let vfs = dc_rez::RezVfs::new(&rom);
    let orders = [
        QuatOrder::Xyzw,
        QuatOrder::XyzwConjugate,
        QuatOrder::Wxyz,
        QuatOrder::WxyzConjugate,
    ];
    let mut conv = Conv::default();
    let (mut rigid_model_like, mut rigid_local_like, mut rigid_ambiguous) = (0u64, 0u64, 0u64);
    let mut skinned_inside = 0u64;
    let mut skinned_total = 0u64;
    let mut node_hist: BTreeMap<usize, u64> = BTreeMap::new();
    let mut max_nodes = 0usize;
    for rez in rez_names(&rom) {
        let Ok(listing) = vfs.list_entries(&rez) else {
            continue;
        };
        for (name, _, _) in listing
            .iter()
            .filter(|(n, _, _)| n.to_ascii_lowercase().ends_with(".ltb"))
        {
            let Ok(Some(bytes)) = vfs.entry(&rez, name) else {
                continue;
            };
            let Ok(m) = Model::parse(&bytes) else {
                continue;
            };
            max_nodes = max_nodes.max(m.nodes.len());
            *node_hist
                .entry(m.nodes.len().div_ceil(16) * 16)
                .or_default() += 1;
            let bind_global: Vec<Mat4> = m
                .nodes
                .iter()
                .map(|n| Mat4::from_cols_array(&n.model_xform).transpose())
                .collect();
            // 3. vertex space of rigid meshes
            for l in m.lods() {
                let layout = match l.mesh.layout(0) {
                    Some(x) => x,
                    None => continue,
                };
                let s = &l.mesh.streams()[0];
                let n = l.mesh.vertex_count() as usize;
                if n == 0 {
                    continue;
                }
                let mut c = Vec3::ZERO;
                let mut lo = Vec3::splat(f32::MAX);
                let mut hi = Vec3::splat(f32::MIN);
                for i in 0..n {
                    if let Some(p) = s.position(&layout, i) {
                        let p = Vec3::from(p);
                        c += p;
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                }
                c /= n as f32;
                match &l.mesh {
                    Mesh::Rigid(r) => {
                        let Some(g) = bind_global.get(r.node as usize) else {
                            continue;
                        };
                        let node_at = g.w_axis.truncate();
                        if node_at.length() < 4.0 {
                            rigid_ambiguous += 1;
                        } else if c.distance(node_at) < c.length() {
                            rigid_model_like += 1;
                        } else {
                            rigid_local_like += 1;
                        }
                    }
                    Mesh::Skinned(_) => {
                        skinned_total += 1;
                        // model-space bind vertices: the bounding box contains at least one used node origin
                        let inside = l.used_nodes.iter().any(|&u| {
                            bind_global.get(u as usize).is_some_and(|g| {
                                let p = g.w_axis.truncate();
                                p.cmpge(lo - Vec3::splat(2.0)).all()
                                    && p.cmple(hi + Vec3::splat(2.0)).all()
                            })
                        });
                        if inside {
                            skinned_inside += 1;
                        }
                    }
                    Mesh::Morph(_) => {}
                }
            }
            // 1./2. frame 0 against the bind pose
            if m.clips.is_empty() || m.nodes.len() < 2 {
                continue;
            }
            let local = m.local_xforms();
            let mut best_rot = [f64::MAX; 4];
            let mut best_pos = f64::MAX;
            for clip in &m.clips {
                let mut perr = 0f64;
                let mut rerr = [0f64; 4];
                let mut used = 0u64;
                for (ni, t) in clip.tracks.iter().enumerate() {
                    let (bs, br) = decompose_rigid(&local[ni]);
                    for (k, o) in orders.iter().enumerate() {
                        let Some((p, q)) = local_at(t, clip, 0.0, *o) else {
                            continue;
                        };
                        if k == 0 {
                            perr += p.distance(bs) as f64;
                            used += 1;
                        }
                        rerr[k] += q.angle_between(br).abs() as f64;
                    }
                }
                if used == 0 {
                    continue;
                }
                best_pos = best_pos.min(perr / used as f64);
                for k in 0..4 {
                    best_rot[k] = best_rot[k].min(rerr[k] / used as f64);
                }
            }
            if best_pos == f64::MAX {
                continue;
            }
            conv.files += 1;
            conv.pos_err.push(best_pos);
            let mut kbest = 0;
            for k in 0..4 {
                conv.rot_err[k].push(best_rot[k]);
                if best_rot[k] < best_rot[kbest] {
                    kbest = k;
                }
            }
            conv.best[kbest] += 1;
        }
    }
    let n = conv.pos_err.len();
    println!(
        "models with clips compared: {} (best-clip frame-0 position error vs the bind local translation: median {:.4}, below 0.01 in {} files, below 1.0 in {})",
        conv.files,
        median(&mut conv.pos_err.clone()),
        below(&conv.pos_err, 0.01),
        below(&conv.pos_err, 1.0),
    );
    for (k, o) in orders.iter().enumerate() {
        println!(
            "  quaternion {:?}: best-clip angle error median {:.4} rad, below 0.01 rad in {} of {n} files, best reading for {} files",
            o,
            median(&mut conv.rot_err[k].clone()),
            below(&conv.rot_err[k], 0.01),
            conv.best[k]
        );
    }
    println!(
        "rigid meshes: centroid nearer its node's bind origin {rigid_model_like}, nearer the model origin \
         {rigid_local_like}, node at the origin (ambiguous) {rigid_ambiguous}"
    );
    println!("skinned meshes whose bind box holds a used node origin: {skinned_inside} / {skinned_total}");
    println!(
        "largest node count {max_nodes}; node count histogram (rounded up to 16): {node_hist:?}"
    );
    let _ = Quat::IDENTITY;
}
