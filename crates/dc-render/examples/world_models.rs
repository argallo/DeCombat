//! Lists the nested render models of one world with the class of the object of the same name and that
//! object's boolean properties, to study which world models the game shows. Output is computed from the user's
//! ROM at run time and printed only.
//! Usage: `cargo run -p dc-render --example world_models -- <map> [<x> <y> <z> <radius>]`.

use dc_render::WorldSource;
use dc_world::{FieldValue, World};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let map = args.first().expect("map name");
    let near: Option<([f32; 3], f32)> = (args.len() >= 5).then(|| {
        let f = |i: usize| args[i].parse::<f32>().expect("number");
        ([f(1), f(2), f(3)], f(4))
    });
    let rom = dc_rom::resolve_rom().expect("CA_ROM");
    let src = WorldSource::open(&rom).expect("world archive");
    let data = src.read(map).expect("read").expect("present");
    let w = World::parse(&data).expect("parse");
    let r = w.render.as_ref().expect("render");
    for m in &r.world.models {
        let obj = w.objects.iter().find(|o| o.name() == Some(m.name));
        let (lo, hi) =
            m.world
                .blocks
                .iter()
                .fold(([f32::MAX; 3], [f32::MIN; 3]), |(mut lo, mut hi), b| {
                    for k in 0..3 {
                        lo[k] = lo[k].min(b.center[k] - b.half[k]);
                        hi[k] = hi[k].max(b.center[k] + b.half[k]);
                    }
                    (lo, hi)
                });
        if let Some((p, rad)) = near {
            let d2: f32 = (0..3)
                .map(|k| {
                    let c = p[k].clamp(lo[k], hi[k]);
                    (c - p[k]) * (c - p[k])
                })
                .sum();
            if d2 > rad * rad {
                continue;
            }
        }
        let tris: u32 = m
            .world
            .blocks
            .iter()
            .flat_map(|b| &b.sections)
            .map(|s| s.triangles)
            .sum();
        let kinds: Vec<u8> = m
            .world
            .blocks
            .iter()
            .flat_map(|b| &b.sections)
            .map(|s| s.shader_kind)
            .collect();
        println!(
            "model {} tris {} kinds {:?} box {:?}..{:?}",
            String::from_utf8_lossy(m.name),
            tris,
            kinds,
            lo,
            hi
        );
        if let Some(o) = obj {
            let all = std::env::var_os("ALL_PROPS").is_some();
            let bools: Vec<String> = o
                .props
                .iter()
                .filter_map(|p| match p.value {
                    FieldValue::Bool(b) => {
                        Some(format!("{}={}", String::from_utf8_lossy(p.name), b as u8))
                    }
                    FieldValue::Text(t) if all => Some(format!(
                        "{}='{}'",
                        String::from_utf8_lossy(p.name),
                        String::from_utf8_lossy(t)
                    )),
                    FieldValue::Real(v) if all => {
                        Some(format!("{}={v}", String::from_utf8_lossy(p.name)))
                    }
                    FieldValue::Int(v) if all => {
                        Some(format!("{}={v}", String::from_utf8_lossy(p.name)))
                    }
                    FieldValue::Rgb(v) if all => {
                        Some(format!("{}={v:?}", String::from_utf8_lossy(p.name)))
                    }
                    _ => None,
                })
                .collect();
            println!(
                "  class {} role {:?} bools {}",
                String::from_utf8_lossy(o.class_name),
                o.role,
                bools.join(" ")
            );
        }
    }
}
