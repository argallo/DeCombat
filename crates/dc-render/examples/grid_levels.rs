//! Census: brightness of the ambient light grid against the lightmaps, per world of the user's ROM (mean of the
//! channel values; counts only). Tells in which units the grid is stored relative to the lightmaps.
//! Usage: `CA_ROM=<dir> cargo run -p dc-render --example grid_levels`.

use dc_render::{decode_lightmap, AmbientGrid, WorldSource};
use dc_world::{ObjectRole, World};

fn main() {
    let rom = dc_rom::resolve_rom().expect("CA_ROM");
    let src = WorldSource::open(&rom).expect("world archive");
    for map in src.map_names() {
        let data = src.read(&map).expect("read").expect("present");
        let w = World::parse(&data).expect("parse");
        let g = AmbientGrid::of(&w.light_grid);
        let n = g.rgb.len().max(1) as f64;
        let mean = g.rgb.iter().map(|&v| v as f64).sum::<f64>() / n;
        let nonzero: Vec<f64> = g
            .rgb
            .iter()
            .filter(|&&v| v > 0)
            .map(|&v| v as f64)
            .collect();
        let nz_mean = nonzero.iter().sum::<f64>() / nonzero.len().max(1) as f64;
        let max = g.rgb.iter().copied().max().unwrap_or(0);
        let mut lm_sum = 0f64;
        let mut lm_n = 0f64;
        let r = w.render.as_ref().expect("render");
        for b in &r.world.blocks {
            for s in &b.sections {
                if s.shader_kind == 2 {
                    if let Some(t) = decode_lightmap(s.lightmap, s.lightmap_w, s.lightmap_h) {
                        lm_sum += t.iter().map(|&v| v as f64).sum::<f64>();
                        lm_n += t.len() as f64;
                    }
                }
            }
        }
        let spawns: Vec<String> = w
            .objects_with_role(ObjectRole::PlayerSpawn)
            .take(3)
            .filter_map(|o| o.position())
            .map(|p| {
                let s = g.sample(glam::Vec3::from(p));
                format!("{:.0}", (s.x + s.y + s.z) / 3.0)
            })
            .collect();
        println!(
            "{map}: grid {:?} mean {mean:.1} (non-zero mean {nz_mean:.1}, max {max}), lightmap texel mean {:.1}, grid at spawns {spawns:?}",
            g.dims,
            lm_sum / lm_n.max(1.0)
        );
    }
}
