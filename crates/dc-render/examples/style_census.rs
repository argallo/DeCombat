//! Census of the render styles and model LODs of the user's ROM (read-only; counts, histograms and
//! `sha256[:4]` tags of upper-cased paths only, no ROM name or value is stored):
//!
//! 1. every entry of the style archive through `dc_model::StyleFile` (variants, passes, options block, extra
//!    blocks) and histograms of the pass and stage words after translation (`dc_render::style`);
//! 2. which styles the ROM tables name per key family (body / face / hair / weapon rows ...) with the stage-0
//!    colour op of each, and the default character's style slots per piece;
//! 3. world objects whose text properties name a style file;
//! 4. with `--lods`: every model file of every archive: pieces with more than one LOD, switch distances, and
//!    whether the LOD records of a piece agree on material slots / style slot / priority.
//!
//! Usage: `CA_ROM=<dir> cargo run -p dc-render --example style_census [-- --lods]`.

use dc_bute::ButeDoc;
use dc_model::{Model, StyleFile};
use dc_render::{style_key, CharacterTable, HandModels, RenderStyle, WorldSource};
use dc_world::{FieldValue, World};
use std::collections::{BTreeMap, BTreeSet};

// OBSERVED: the style archive of the ROM (RENDER_2009.md §13: 62 entries, 61 render-style files)
const STYLE_REZ: &str = "RS.rez";
// OBSERVED: ROM table keys MODELBUTES.TXT and WEAPONS.TXT (entries of ATTRIBUTES.rez)
const TABLE_REZ: &str = "ATTRIBUTES.rez";
const MODEL_TABLE: &str = "ATTRIBUTES\\MODELBUTES.TXT";

fn tag(path: &str) -> String {
    let h = dc_rom::sha256(style_key(path).as_bytes());
    h[..4].iter().map(|b| format!("{b:02x}")).collect()
}

fn bump(m: &mut BTreeMap<String, u64>, k: impl Into<String>) {
    *m.entry(k.into()).or_default() += 1;
}

type Hists = BTreeMap<&'static str, BTreeMap<String, u64>>;

fn add(h: &mut Hists, key: &'static str, v: impl Into<String>) {
    bump(h.entry(key).or_default(), v);
}

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

fn main() {
    let rom = dc_rom::resolve_rom().expect("CA_ROM");
    let vfs = dc_rez::RezVfs::new(&rom);

    // ---- 1. the style files ----
    let entries = vfs.list_entries(STYLE_REZ).expect("style archive");
    let mut styles: BTreeMap<String, RenderStyle> = BTreeMap::new();
    let mut h: Hists = BTreeMap::new();
    let mut sizes = (usize::MAX, 0usize);
    for (name, _, size) in &entries {
        let data = vfs
            .entry(STYLE_REZ, name)
            .ok()
            .flatten()
            .unwrap_or_default();
        let f = match StyleFile::parse(&data) {
            Ok(f) => f,
            Err(e) => {
                add(&mut h, "unparsed", format!("{} bytes: {e}", size));
                continue;
            }
        };
        sizes = (sizes.0.min(data.len()), sizes.1.max(data.len()));
        add(&mut h, "variants", f.variants.len().to_string());
        add(&mut h, "trailing", f.trailing.to_string());
        let v = &f.variants[0];
        add(
            &mut h,
            "data_len = variant len word",
            (f.data_len == v.len_word).to_string(),
        );
        add(
            &mut h,
            "variant len word = material..last pass",
            (v.len_word == StyleFile::variant_len(v)).to_string(),
        );
        add(
            &mut h,
            "options block",
            if v.options.is_some() {
                "present"
            } else {
                "absent"
            },
        );
        add(&mut h, "passes", v.passes.len().to_string());
        for p in &v.passes {
            add(&mut h, "extra block", p.extra.is_some().to_string());
        }
        let Some(rs) = RenderStyle::from_file(&f, name) else {
            add(&mut h, "unparsed", "no passes");
            continue;
        };
        add(
            &mut h,
            "stage-0 colour op (pass 0)",
            format!("{:?}", rs.passes[0].stages[0].colour_op),
        );
        add(
            &mut h,
            "material ambient white",
            (rs.material.ambient[..3] == [1.0, 1.0, 1.0]).to_string(),
        );
        add(
            &mut h,
            "material emissive black",
            (rs.material.emissive[..3] == [0.0, 0.0, 0.0]).to_string(),
        );
        for p in &rs.passes {
            add(&mut h, "pass blend", format!("{:?}", p.blend));
            add(&mut h, "pass lighting", p.lighting.to_string());
            add(&mut h, "pass cull", format!("{:?}", p.cull));
            add(
                &mut h,
                "pass depth (test, write)",
                format!("{:?}", (p.depth_test, p.depth_write)),
            );
            add(
                &mut h,
                "pass depth compare",
                format!("{:?}", p.depth_compare),
            );
            add(&mut h, "pass alpha test", format!("{:?}", p.alpha_test));
            add(&mut h, "pass wireframe", p.wireframe.to_string());
            add(&mut h, "pass bump", p.bump.to_string());
            add(&mut h, "pass active stages", p.active_stages().to_string());
            add(
                &mut h,
                "pass texture factor",
                format!("{:08x}", p.texture_factor),
            );
            for s in &p.stages[..p.active_stages()] {
                add(&mut h, "stage colour op", format!("{:?}", s.colour_op));
                add(&mut h, "stage alpha op", format!("{:?}", s.alpha_op));
                add(&mut h, "stage texture", format!("{:?}", s.texture));
                add(&mut h, "stage uv", format!("{:?}", s.uv));
                add(&mut h, "stage address", format!("{:?}", s.address));
                add(&mut h, "stage filter", format!("{:?}", s.filter));
                add(
                    &mut h,
                    "stage transform (count, projected)",
                    format!("{:?}", s.transform.map(|t| (t.1, t.2))),
                );
            }
        }
        styles.insert(style_key(name), rs);
    }
    println!(
        "style archive: {} entries, {} parsed, sizes {}..{} bytes",
        entries.len(),
        styles.len(),
        sizes.0,
        sizes.1
    );
    for (k, m) in &h {
        println!("  {k}: {m:?}");
    }

    // ---- 2. table usage ----
    let describe = |path: &str| -> String {
        match styles.get(&style_key(path)) {
            Some(s) => format!(
                "{} {:?} x{}",
                tag(path),
                s.passes[0].stages[0].colour_op,
                s.passes.len()
            ),
            None => format!("{} missing", tag(path)),
        }
    };
    let mut uses: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    for entry in [MODEL_TABLE, dc_render_weapon_table()] {
        let Ok(Some(doc)) = ButeDoc::try_from_vfs(&vfs, TABLE_REZ, entry) else {
            println!("table {} missing", tag(entry));
            continue;
        };
        for (sec, map) in doc.iter() {
            let family: String = sec
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            for (k, v) in map.iter() {
                if !k.to_ascii_lowercase().contains("renderstyle") {
                    continue;
                }
                let key: String = k.chars().filter(|c| !c.is_ascii_digit()).collect();
                let v = v.trim().trim_matches('"');
                if v.is_empty() {
                    continue;
                }
                bump(
                    uses.entry(format!("{family}.{key}")).or_default(),
                    describe(v),
                );
            }
        }
    }
    println!("table style names (family.key: style tag, stage-0 op, passes -> rows):");
    for (k, m) in &uses {
        println!("  {k}: {m:?}");
    }
    if let Ok(t) = CharacterTable::from_vfs(&vfs) {
        if let Some(b) = t.bodies.get(&0) {
            println!(
                "body row 0 styles: {:?}",
                b.styles.iter().map(|s| describe(s)).collect::<Vec<_>>()
            );
            if let Ok(files) = dc_render::ModelFiles::load(&vfs, &b.model) {
                for (i, p) in files.body.pieces.iter().enumerate() {
                    let last = p.lods.last().expect("lod");
                    println!(
                        "  piece {i}: lods {} dists {:?} style slot {} slots {:?}/{} kind {}",
                        p.lods.len(),
                        p.lod_dist,
                        last.style_index,
                        last.slots,
                        last.slot_count,
                        last.mesh.kind()
                    );
                }
                println!(
                    "  weapon socket {:?} of {}",
                    dc_render::grip_socket(&files.body),
                    files.body.sockets.len()
                );
            }
        }
        if let Some(f) = t.face_styles.get(&0) {
            println!(
                "face row 0 styles: {:?}",
                f.iter().map(|s| describe(s)).collect::<Vec<_>>()
            );
        }
    }
    if let Ok(hands) = HandModels::from_vfs(&vfs) {
        let mut hh: BTreeMap<String, u64> = BTreeMap::new();
        for r in hands.rows.values() {
            bump(
                &mut hh,
                r.styles
                    .first()
                    .map(|s| describe(s))
                    .unwrap_or_else(|| "none".into()),
            );
        }
        println!(
            "weapon rows with a third-person model: {}; style slot 0: {hh:?}; first row styles {:?}",
            hands.rows.len(),
            hands
                .rows
                .values()
                .next()
                .map(|r| r.styles.iter().map(|s| describe(s)).collect::<Vec<_>>())
        );
    }

    // ---- 3. world objects naming a style ----
    let src = WorldSource::open(&rom).expect("world archive");
    let mut world_uses: BTreeMap<String, u64> = BTreeMap::new();
    for map in src.map_names() {
        let data = src.read(&map).expect("read").expect("present");
        let w = World::parse(&data).expect("parse");
        for o in &w.objects {
            for p in &o.props {
                let FieldValue::Text(t) = &p.value else {
                    continue;
                };
                let v = String::from_utf8_lossy(t).to_string();
                for part in v.split(';') {
                    let k = style_key(part);
                    let with_folder = format!("RS{}{}", '\u{5c}', k);
                    if styles.contains_key(&k) || styles.contains_key(&with_folder) {
                        bump(
                            &mut world_uses,
                            format!(
                                "class {} property {} -> {}",
                                dc_world::hash16_hex(o.class_name),
                                dc_world::hash16_hex(p.name),
                                if styles.contains_key(&k) {
                                    describe(&k)
                                } else {
                                    describe(&with_folder)
                                }
                            ),
                        );
                    }
                }
            }
        }
    }
    println!("world objects naming a style: {world_uses:?}");

    // ---- 4. LODs over every model file ----
    if std::env::args().any(|a| a == "--lods") {
        let mut lod_counts: BTreeMap<usize, u64> = BTreeMap::new();
        let mut dists: BTreeMap<String, u64> = BTreeMap::new();
        let (mut multi, mut differ, mut models) = (0u64, 0u64, 0u64);
        let mut seen = BTreeSet::new();
        for rez in rez_names(&rom) {
            let Ok(list) = vfs.list_entries(&rez) else {
                continue;
            };
            for (name, _, _) in list {
                if !name.to_ascii_lowercase().ends_with(".ltb") || !seen.insert(name.clone()) {
                    continue;
                }
                let Ok(Some(data)) = vfs.entry(&rez, &name) else {
                    continue;
                };
                let Ok(m) = Model::parse(&data) else { continue };
                models += 1;
                for p in &m.pieces {
                    *lod_counts.entry(p.lods.len()).or_default() += 1;
                    if p.lods.len() > 1 {
                        multi += 1;
                        bump(&mut dists, format!("{:?}", p.lod_dist));
                        let f = &p.lods[0];
                        if p.lods.iter().any(|l| {
                            l.slots != f.slots
                                || l.slot_count != f.slot_count
                                || l.style_index != f.style_index
                                || l.priority != f.priority
                        }) {
                            differ += 1;
                        }
                    }
                }
            }
        }
        println!(
            "models {models}: pieces by LOD count {lod_counts:?}; multi-LOD pieces {multi}, LOD records that \
             disagree on slots / style / priority {differ}; switch distances {dists:?}"
        );
    }
}

/// The weapon table entry (same key as dc-render's character module).
fn dc_render_weapon_table() -> &'static str {
    // OBSERVED: ROM table key WEAPONS.TXT (entry of ATTRIBUTES.rez)
    "ATTRIBUTES\\WEAPONS.TXT"
}
