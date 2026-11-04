//! Census over every world of the user's ROM: for the objects that own a nested render model, the values of
//! the properties given on the command line (by name, as the user's files spell them), counted. Printed only.
//! Usage: `cargo run -p dc-render --example world_model_props -- <prop> [<prop> ...]`.

use dc_render::WorldSource;
use dc_world::{FieldValue, World};
use std::collections::BTreeMap;

fn main() {
    let props: Vec<String> = std::env::args().skip(1).collect();
    let rom = dc_rom::resolve_rom().expect("CA_ROM");
    let src = WorldSource::open(&rom).expect("world archive");
    let mut counts: BTreeMap<(String, String, String), u64> = BTreeMap::new();
    for map in src.map_names() {
        let data = src.read(&map).expect("read").expect("present");
        let w = World::parse(&data).expect("parse");
        let r = w.render.as_ref().expect("render");
        for m in &r.world.models {
            let Some(o) = w.objects.iter().find(|o| o.name() == Some(m.name)) else {
                continue;
            };
            let role = format!("{:?}", o.role);
            for want in &props {
                let v = match o.prop(want.as_bytes()).map(|p| p.value) {
                    Some(FieldValue::Bool(b)) => format!("{b}"),
                    Some(FieldValue::Text(t)) => String::from_utf8_lossy(t).into_owned(),
                    Some(FieldValue::Real(f)) => format!("{f}"),
                    Some(other) => format!("{other:?}"),
                    None => "-".into(),
                };
                *counts.entry((want.clone(), role.clone(), v)).or_default() += 1;
            }
        }
    }
    for ((p, role, v), n) in counts {
        println!("{p:<14} {role:<28} {v:<16} {n}");
    }
}
