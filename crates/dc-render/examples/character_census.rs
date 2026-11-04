//! Census of the character body models named by the ROM model table (read-only; counts, hashes and indices
//! only). Per body row: node count, child models and whether their nodes match the body's by name, blend sets
//! (weight sums and the mean bind height of the nodes each set weights, to tell an upper-body set from a
//! lower-body one without reading names), the combined clip count, and the combined indices of the four player
//! clips the server streams (found by the name keys of SESSION_B_S2C_addendum_dserver3.md S3-5).
//!
//! Usage: `CA_ROM=<dir> cargo run -p dc-render --example character_census`.

use dc_bute::ButeDoc;
use dc_render::{name_key, ModelFiles, ModelMesh, NameKey, PLAYER_CLIPS};
use std::collections::BTreeMap;

// OBSERVED: ROM table key MODELBUTES.TXT (entry of ATTRIBUTES.rez), sections [Model<n>] with key ModelFile
const TABLE_REZ: &str = "ATTRIBUTES.rez";
const TABLE_ENTRY: &str = "ATTRIBUTES\\MODELBUTES.TXT";

fn short(p: &str) -> String {
    let h = dc_rom::sha256(p.to_ascii_uppercase().replace('/', "\\").as_bytes());
    h[..4].iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    if std::env::args().any(|a| a == "--faces") {
        faces();
        return;
    }
    let rom = dc_rom::resolve_rom().expect("CA_ROM");
    let vfs = dc_rez::RezVfs::new(&rom);
    let doc = ButeDoc::try_from_vfs(&vfs, TABLE_REZ, TABLE_ENTRY)
        .expect("table")
        .expect("present");
    let mut rows: BTreeMap<u32, String> = BTreeMap::new();
    for (name, sec) in doc.iter() {
        let Some(n) = name
            .strip_prefix("Model")
            .and_then(|r| r.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(v) = sec.get("ModelFile") {
            rows.insert(n, v.trim().trim_matches('"').to_string());
        }
    }
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for (n, path) in &rows {
        if seen.insert(path.to_ascii_uppercase(), ()).is_some() {
            continue;
        }
        let files = match ModelFiles::load(&vfs, path) {
            Ok(f) => f,
            Err(e) => {
                println!("row {n} ({}): {e}", short(path));
                continue;
            }
        };
        let set = files.anim_set();
        let b = &files.body;
        let kids: Vec<String> = files
            .children
            .iter()
            .map(|(p, m)| {
                let same = m
                    .nodes
                    .iter()
                    .zip(&b.nodes)
                    .filter(|(x, y)| x.name.as_bytes().eq_ignore_ascii_case(y.name.as_bytes()))
                    .count();
                format!(
                    "{} nodes {} (same name at same index {same}), clips {}",
                    short(p),
                    m.nodes.len(),
                    m.clips.len()
                )
            })
            .collect();
        let sets: Vec<String> = b
            .blend_sets
            .iter()
            .map(|s| {
                let sum: f32 = s.weights.iter().sum();
                let (mut hy, mut hn) = (0f32, 0f32);
                for (i, w) in s.weights.iter().enumerate() {
                    if *w >= 0.5 {
                        hy += b.nodes[i].model_xform[7];
                        hn += 1.0;
                    }
                }
                format!(
                    "sum {sum:.1} mean height {:.1}",
                    if hn > 0.0 { hy / hn } else { f32::NAN }
                )
            })
            .collect();
        let keys: Vec<String> = PLAYER_CLIPS
            .iter()
            .map(|k: &NameKey| {
                set.find_clip(|nm| name_key(nm) == *k)
                    .map_or("-".into(), |i| i.to_string())
            })
            .collect();
        let mesh = ModelMesh::build(b);
        let mats: Vec<u32> = mesh
            .draws
            .iter()
            .filter(|d| d.lod == 0)
            .map(|d| d.material)
            .collect();
        println!(
            "row {n} ({}): nodes {}, pieces {} (rigid {} skinned {} morph {} skipped {}), materials {mats:?}, \
             own clips {}, combined clips {}, blend sets [{}], children [{}], player clips (upper stand, upper \
             walk, lower stand, lower walk) {keys:?}",
            short(path),
            b.nodes.len(),
            mesh.stats.pieces,
            mesh.stats.rigid,
            mesh.stats.skinned,
            mesh.stats.morph,
            mesh.stats.skipped,
            b.clips.len(),
            set.clip_count(),
            sets.join("; "),
            kids.join("; "),
        );
    }
}

/// Face rows: node names shared with body row 0's model (by name), piece count, clips.
#[allow(dead_code)]
fn faces() {
    let rom = dc_rom::resolve_rom().expect("CA_ROM");
    let vfs = dc_rez::RezVfs::new(&rom);
    let doc = ButeDoc::try_from_vfs(&vfs, TABLE_REZ, TABLE_ENTRY)
        .expect("table")
        .expect("present");
    let body_path = doc
        .get("Model0")
        .and_then(|s| s.get("ModelFile"))
        .map(|v| v.trim().trim_matches('"').to_string())
        .expect("body row 0");
    let body = ModelFiles::load(&vfs, &body_path).expect("body");
    for (name, sec) in doc.iter() {
        if !name.starts_with("Face") {
            continue;
        }
        let Some(p) = sec
            .get("ModelFile")
            .map(|v| v.trim().trim_matches('"').to_string())
        else {
            continue;
        };
        match ModelFiles::load(&vfs, &p) {
            Ok(f) => {
                let shared = f
                    .body
                    .nodes
                    .iter()
                    .filter(|n| {
                        body.body
                            .nodes
                            .iter()
                            .any(|b| b.name.as_bytes().eq_ignore_ascii_case(n.name.as_bytes()))
                    })
                    .count();
                let same_bind = f
                    .body
                    .nodes
                    .iter()
                    .filter(|n| {
                        body.body.nodes.iter().any(|b| {
                            b.name.as_bytes().eq_ignore_ascii_case(n.name.as_bytes())
                                && b.model_xform
                                    .iter()
                                    .zip(&n.model_xform)
                                    .all(|(x, y)| (x - y).abs() < 1e-3)
                        })
                    })
                    .count();
                println!(
                    "{name} ({}): nodes {}, shared with body row 0 by name {shared}, same bind {same_bind}, pieces {}, clips {}, children {}",
                    short(&p),
                    f.body.nodes.len(),
                    f.body.pieces.len(),
                    f.body.clips.len(),
                    f.children.len()
                );
            }
            Err(e) => println!("{name} ({}): {e}", short(&p)),
        }
    }
}
