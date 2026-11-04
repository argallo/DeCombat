//! Census of the render light groups (FMT_DAT85.md §10.3, RENDER_2009.md §6.2) of every world in the user's
//! ROM: colours, slots against the block's lightmap sections, patch rectangles against the lightmap sizes,
//! patch byte statistics. Counts and ranges only.

use dc_render::WorldSource;
use dc_world::World;
use std::collections::BTreeMap;

fn main() {
    let rom = dc_rom::resolve_rom().expect("CA_ROM");
    let src = WorldSource::open(&rom).expect("world archive");
    for map in src.map_names() {
        let data = src.read(&map).unwrap().unwrap();
        let w = World::parse(&data).unwrap();
        let r = w.render.as_ref().unwrap();
        let mut groups = 0;
        let mut slot_match = 0;
        let mut slot_mismatch = 0;
        let mut patches = 0u64;
        let mut fits = 0u64;
        let mut exact_len = 0u64;
        let mut rle_len = 0u64;
        let mut colours = Vec::new();
        let mut data_lens: BTreeMap<usize, u64> = BTreeMap::new();
        let mut byte_hist = [0u64; 256];
        for b in &r.world.blocks {
            let _lm: Vec<(u32, u32)> = b
                .sections
                .iter()
                .filter(|s| s.shader_kind == 2)
                .map(|s| (s.lightmap_w, s.lightmap_h))
                .collect();
            for g in &b.light_groups {
                groups += 1;
                if colours.len() < 6 {
                    colours.push(g.colour);
                }
                *data_lens.entry(g.data.len()).or_default() += 1;
                if g.slots.len() == b.sections.len() {
                    *data_lens.entry(100000 + g.slots.len()).or_default() += 0;
                }
                if g.slots.len() == b.sections.len() {
                    slot_match += 1;
                } else {
                    slot_mismatch += 1;
                }
                for (si, slot) in g.slots.iter().enumerate() {
                    if !slot.is_empty() && b.sections.get(si).map(|s| s.shader_kind) != Some(2) {
                        slot_mismatch += 1000;
                    }
                    for p in slot {
                        patches += 1;
                        let [x, y, pw, ph] = p.words;
                        let sec = b.sections.get(si).map(|s| (s.lightmap_w, s.lightmap_h));
                        if let Some((lw, lh)) = sec {
                            if x + pw <= lw && y + ph <= lh {
                                fits += 1;
                            }
                        }
                        let n = (pw * ph) as usize;
                        if p.data.len() == n {
                            exact_len += 1;
                        }
                        // run escape: 0xff then a count repeats the previous value
                        let mut i = 0;
                        let mut texels = 0usize;
                        while i < p.data.len() {
                            if p.data[i] == 0xff && i + 1 < p.data.len() {
                                texels += p.data[i + 1] as usize;
                                i += 2;
                            } else {
                                texels += 1;
                                i += 1;
                            }
                        }
                        if texels == n {
                            rle_len += 1;
                        }
                        for &v in p.data {
                            byte_hist[v as usize] += 1;
                        }
                    }
                }
            }
        }
        if groups == 0 {
            continue;
        }
        let nz: Vec<(usize, u64)> = byte_hist
            .iter()
            .enumerate()
            .filter(|(_, &c)| c > 0)
            .map(|(v, &c)| (v, c))
            .collect();
        println!(
            "{} groups {groups} slots==all sections {slot_match} (else {slot_mismatch}) patches {patches} fit {fits} len==w*h {exact_len} rle==w*h {rle_len}",
            &dc_rom::sha256_hex(map.as_bytes())[..8]
        );
        println!("   colours {colours:?} data lens {data_lens:?}");
        println!(
            "   byte values: distinct {} min {:?} max {:?} count(0xff) {}",
            nz.len(),
            nz.first(),
            nz.last(),
            byte_hist[255]
        );
        let grids: Vec<_> = r.grids.iter().map(|g| (g.colour, g.dims)).collect();
        println!("   light-group grids {grids:?}");
    }
}
