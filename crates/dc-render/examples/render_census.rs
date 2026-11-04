//! Census of the render data of every world in the user's ROM, printed as counts and ranges only (no names,
//! no geometry): section kinds, the section-to-triangle partition, kind-2 / kind-4 / kind-9 pairing, uv
//! ranges per kind, nested render models against their BSP boxes, texture resolution.
//! Usage: `CA_ROM=<client dir> cargo run -p dc-render --example render_census`.

use dc_render::{decode_lightmap, RenderVertex, TextureIndex, WorldSource};
use dc_world::{ObjectRole, RenderBlock, World};
use std::collections::{BTreeMap, HashSet};

#[derive(Default, Debug)]
struct Range {
    lo: [f32; 2],
    hi: [f32; 2],
    n: u64,
}

impl Range {
    fn add(&mut self, v: [f32; 2]) {
        if self.n == 0 {
            self.lo = v;
            self.hi = v;
        }
        for (k, &x) in v.iter().enumerate() {
            self.lo[k] = self.lo[k].min(x);
            self.hi[k] = self.hi[k].max(x);
        }
        self.n += 1;
    }
}

fn tri_set(b: &RenderBlock<'_>, first: u32, count: u32) -> HashSet<[u32; 3]> {
    (first..first + count)
        .filter_map(|i| b.triangle(i))
        .map(|t| {
            let mut k = [t[0], t[1], t[2]];
            k.sort_unstable();
            k
        })
        .collect()
}

/// Height at which the vertical line through `p` crosses triangle `v` (x/z barycentric test).
fn vertical_hit(p: [f32; 3], v: &[[f32; 3]]) -> Option<f32> {
    let (ax, az) = (v[0][0], v[0][2]);
    let (bx, bz) = (v[1][0] - ax, v[1][2] - az);
    let (cx, cz) = (v[2][0] - ax, v[2][2] - az);
    let det = bx * cz - cx * bz;
    if det.abs() < 1e-6 {
        return None;
    }
    let (px, pz) = (p[0] - ax, p[2] - az);
    let u = (px * cz - cx * pz) / det;
    let w = (bx * pz - px * bz) / det;
    if u < 0.0 || w < 0.0 || u + w > 1.0 {
        return None;
    }
    Some(v[0][1] + u * (v[1][1] - v[0][1]) + w * (v[2][1] - v[0][1]))
}

fn main() {
    let rom = dc_rom::resolve_rom().expect("CA_ROM");
    let src = WorldSource::open(&rom).expect("world archive");
    let tex = TextureIndex::open(&rom).expect("texture index");
    let mut all_tex_names: HashSet<Vec<u8>> = HashSet::new();
    for name in src.map_names() {
        let data = src.read(&name).expect("read").expect("present");
        let w = World::parse(&data).expect("parse");
        let r = w.render.as_ref().expect("render data");
        let mut kinds: BTreeMap<u8, u64> = BTreeMap::new();
        let mut partition_ok = 0u64;
        let mut partition_bad = 0u64;
        let mut lm_same_tris = 0u64;
        let mut lm_blocks = 0u64;
        let mut uv: BTreeMap<(u8, u8), Range> = BTreeMap::new();
        let mut alpha_not_ff = 0u64;
        let mut verts = 0u64;
        let mut src_max = 0u32;
        let mut lm_bad = 0u64;
        let mut lm_pairs_same_verts = 0u64;
        for b in &r.world.blocks {
            let mut base = 0u32;
            let mut ranges = Vec::new();
            for s in &b.sections {
                *kinds.entry(s.shader_kind).or_default() += 1;
                ranges.push((s.shader_kind, base, s.triangles));
                base += s.triangles;
                all_tex_names.insert(s.textures[0].to_vec());
                if !s.textures[1].is_empty() {
                    all_tex_names.insert(s.textures[1].to_vec());
                }
                if s.shader_kind == 2 {
                    let lm = decode_lightmap(s.lightmap, s.lightmap_w, s.lightmap_h);
                    if lm.is_none() {
                        lm_bad += 1;
                    }
                }
            }
            if base == b.triangle_count {
                partition_ok += 1;
            } else {
                partition_bad += 1;
            }
            // Pairing: are the kind-2 triangles the same vertex triples as the kind 4 / 9 triangles?
            let lm: HashSet<[u32; 3]> = ranges
                .iter()
                .filter(|r| r.0 == 2)
                .flat_map(|r| tri_set(b, r.1, r.2))
                .collect();
            let tx: HashSet<[u32; 3]> = ranges
                .iter()
                .filter(|r| r.0 == 4 || r.0 == 9)
                .flat_map(|r| tri_set(b, r.1, r.2))
                .collect();
            if !lm.is_empty() {
                lm_blocks += 1;
                if lm == tx {
                    lm_same_tris += 1;
                }
                if !lm.is_disjoint(&tx) {
                    lm_pairs_same_verts += 1;
                }
            }
            for &(kind, first, count) in &ranges {
                for i in first..first + count {
                    let Some(t) = b.triangle(i) else { continue };
                    src_max = src_max.max(t[3]);
                    for &vi in &t[..3] {
                        let v = RenderVertex::read(b.vertices, vi as usize).expect("vertex");
                        uv.entry((kind, 0)).or_default().add(v.uv0);
                        uv.entry((kind, 1)).or_default().add(v.uv1);
                    }
                }
            }
            for vi in 0..b.vertex_count as usize {
                let v = RenderVertex::read(b.vertices, vi).expect("vertex");
                verts += 1;
                if v.colour >> 24 != 0xff {
                    alpha_not_ff += 1;
                }
            }
        }
        // Nested render models against the BSP box of the same name.
        let mut inside = 0u64;
        let mut outside = 0u64;
        let mut sky_models = 0u64;
        for m in &r.world.models {
            if w.objects
                .iter()
                .any(|o| o.name() == Some(m.name) && o.role == Some(ObjectRole::SkyWorldModel))
            {
                sky_models += 1;
            }
            let Some(bsp) = w.model_named(m.name) else {
                continue;
            };
            for b in &m.world.blocks {
                let c = b.center;
                let ok = (0..3).all(|k| {
                    c[k] >= bsp.box_min[k] - b.half[k] - 1.0
                        && c[k] <= bsp.box_max[k] + b.half[k] + 1.0
                });
                if ok {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
        let spawn = w.objects_with_role(ObjectRole::PlayerSpawn).next();
        // Distance from the first spawn point down to the nearest render triangle and up to the nearest one.
        let (mut down, mut up) = (f32::INFINITY, f32::INFINITY);
        if let Some(p) = spawn.and_then(|o| o.position()) {
            for b in &r.world.blocks {
                for i in 0..b.triangle_count {
                    let Some(t) = b.triangle(i) else { continue };
                    let v: Vec<[f32; 3]> = t[..3]
                        .iter()
                        .map(|&k| RenderVertex::read(b.vertices, k as usize).unwrap().pos)
                        .collect();
                    if let Some(y) = vertical_hit(p, &v) {
                        if y <= p[1] {
                            down = down.min(p[1] - y);
                        } else {
                            up = up.min(y - p[1]);
                        }
                    }
                }
            }
        }
        println!("   first spawn: floor below {down:.1}, surface above {up:.1}");
        println!(
            "{} kinds={:?} partition ok={} bad={} lm_blocks={} lm==tex {} overlap {} lm_bad={} verts={} alpha!=ff {} src_max={} nested inside={} outside={} sky_models={} spawn_rot={:?}",
            dc_rom::sha256_hex(name.as_bytes()).get(..8).unwrap_or(""),
            kinds,
            partition_ok,
            partition_bad,
            lm_blocks,
            lm_same_tris,
            lm_pairs_same_verts,
            lm_bad,
            verts,
            alpha_not_ff,
            src_max,
            inside,
            outside,
            sky_models,
            spawn.and_then(|o| o.rotation()),
        );
        for ((k, set), rg) in &uv {
            println!(
                "   kind {k} uv{set}: lo {:?} hi {:?} n {}",
                rg.lo, rg.hi, rg.n
            );
        }
    }
    // Sections whose first texture resolves to nothing: kind, size, effect, lightmap, by name hash.
    let mut unresolved: BTreeMap<(String, u8, bool, bool), (u64, u64)> = BTreeMap::new();
    for name in src.map_names() {
        let data = src.read(&name).unwrap().unwrap();
        let w = World::parse(&data).unwrap();
        let r = w.render.as_ref().unwrap();
        let mut walk = vec![&r.world];
        while let Some(rw) = walk.pop() {
            for b in &rw.blocks {
                for s in &b.sections {
                    if !s.textures[0].is_empty() && tex.locate(s.textures[0]).is_none() {
                        let key = (
                            dc_rom::sha256_hex(&s.textures[0].to_ascii_uppercase())[..8]
                                .to_string(),
                            s.shader_kind,
                            !s.effect.is_empty(),
                            !s.textures[1].is_empty(),
                        );
                        let e = unresolved.entry(key).or_default();
                        e.0 += 1;
                        e.1 += s.triangles as u64;
                    }
                }
            }
            walk.extend(rw.models.iter().map(|m| &m.world));
        }
    }
    // Nested render models by the role of the object of the same name, and which carry the unresolved texture.
    let mut by_role: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    for name in src.map_names() {
        let data = src.read(&name).unwrap().unwrap();
        let w = World::parse(&data).unwrap();
        let r = w.render.as_ref().unwrap();
        for m in &r.world.models {
            let role = w
                .objects
                .iter()
                .find(|o| o.name() == Some(m.name))
                .map(|o| format!("{:?}", o.role))
                .unwrap_or_else(|| "no object".into());
            let e = by_role.entry(role).or_default();
            e.0 += 1;
            let mut unres = false;
            for b in &m.world.blocks {
                for s in &b.sections {
                    e.1 += s.triangles as u64;
                    if s.shader_kind != 2
                        && !s.textures[0].is_empty()
                        && tex.locate(s.textures[0]).is_none()
                    {
                        unres = true;
                    }
                }
            }
            e.2 += unres as u64;
        }
    }
    println!("nested render models by object role -> (models, triangles, with unresolved texture): {by_role:?}");
    println!("unresolved first textures (name hash, kind, has effect, has tex1) -> (sections, triangles): {unresolved:?}");
    let mut hit = 0;
    let mut miss = 0;
    let mut errors: BTreeMap<String, u64> = BTreeMap::new();
    for n in &all_tex_names {
        if tex.locate(n).is_some() {
            hit += 1;
            match tex.read(n) {
                Ok(Some(b)) => {
                    if let Err(e) = dc_tex::DtxFile::parse(&b) {
                        let ext = n.rsplit(|&c| c == b'.').next().unwrap_or(b"");
                        let ext = String::from_utf8_lossy(ext).to_ascii_lowercase();
                        *errors.entry(format!("{e:?} ext={ext}")).or_default() += 1;
                    }
                }
                Ok(None) => *errors.entry("empty entry".into()).or_default() += 1,
                Err(e) => *errors.entry(format!("read: {e}")).or_default() += 1,
            }
        } else {
            miss += 1;
        }
    }
    println!(
        "texture names: {} resolved, {} missing; parse failures {:?}",
        hit, miss, errors
    );
}
