//! Census over every world of the user's ROM for the model, sky and translucent passes (read-only; counts and
//! value histograms only): light objects and the static lights they give models, the attenuation forms, the
//! section kinds of translucent / additive world models, the sky world models and sky portals.
//!
//! Usage: `CA_ROM=<dir> cargo run -p dc-render --example world_lights`.

use dc_render::{
    light_class, model_visibility, static_light, Falloff, LightShape, ModelVisibility, WorldSource,
};
use dc_world::{FieldValue, ObjectRole, World};
use std::collections::BTreeMap;

fn main() {
    let rom = dc_rom::resolve_rom().expect("CA_ROM");
    let src = WorldSource::open(&rom).expect("world archive");
    let mut classes: BTreeMap<u8, u64> = BTreeMap::new();
    let mut created = 0u64;
    let mut falloff: BTreeMap<String, u64> = BTreeMap::new();
    let mut shapes: BTreeMap<String, u64> = BTreeMap::new();
    let mut to_amb: BTreeMap<String, u64> = BTreeMap::new();
    let mut props: BTreeMap<(String, String), u64> = BTreeMap::new();
    let (mut rmin, mut rmax) = (f32::MAX, 0f32);
    let mut cmax = 0f32;
    let mut blended_kinds: BTreeMap<(String, u8), u64> = BTreeMap::new();
    let mut blended_models: BTreeMap<String, u64> = BTreeMap::new();
    let mut sky = (0u64, 0u64, 0u64, 0u64);
    let mut sky_kinds: BTreeMap<u8, u64> = BTreeMap::new();
    let mut portals = 0u64;
    for map in src.map_names() {
        let data = src.read(&map).expect("read").expect("present");
        let w = World::parse(&data).expect("parse");
        let mut map_lights = 0;
        for o in &w.objects {
            let Some(c) = light_class(o) else { continue };
            *classes.entry(c).or_default() += 1;
            for p in &o.props {
                let v = match p.value {
                    FieldValue::Real(_) => "real",
                    FieldValue::Vec3(_) => "vec3",
                    FieldValue::Rgb(_) => "rgb",
                    FieldValue::Bool(true) => "true",
                    FieldValue::Bool(false) => "false",
                    FieldValue::Text(_) => "text",
                    FieldValue::Euler(_) => "euler",
                    _ => "other",
                };
                *props
                    .entry((dc_world::hash16_hex(p.name), v.to_string()))
                    .or_default() += 1;
            }
            if let Some(l) = static_light(o) {
                created += 1;
                map_lights += 1;
                rmin = rmin.min(l.radius);
                rmax = rmax.max(l.radius);
                cmax = cmax.max(l.colour.max_element());
                let f = match l.falloff {
                    Falloff::D3d(c) => {
                        // coefficients relative to the dynamic law (c2 * r^2 / 19)
                        let rel = c[2] * l.radius * l.radius / 19.0;
                        format!(
                            "d3d c0 {:.2} c1*r {:.2} c2*r^2/19 {:.2}",
                            c[0],
                            c[1] * l.radius,
                            rel
                        )
                    }
                    Falloff::Linear => "linear".into(),
                    Falloff::Quartic => "quartic".into(),
                };
                *falloff.entry(f).or_default() += 1;
                let s = match l.shape {
                    LightShape::Point => "point".to_string(),
                    LightShape::Spot { cos_half, .. } => format!("spot cos {cos_half:.2}"),
                    LightShape::Directional { .. } => "directional".into(),
                };
                *shapes.entry(s).or_default() += 1;
                *to_amb.entry(format!("{:.2}", l.to_ambient)).or_default() += 1;
            }
        }
        let r = w.render.as_ref().expect("render data");
        for b in &r.world.blocks {
            portals += b.sky_portals.len() as u64;
        }
        for m in &r.world.models {
            let obj = w.objects.iter().find(|o| o.name() == Some(m.name));
            let vis = model_visibility(obj);
            let label = match vis {
                ModelVisibility::Translucent { additive: true, .. } => "additive",
                ModelVisibility::Translucent { .. } => "translucent",
                ModelVisibility::Sky => "sky",
                ModelVisibility::Hidden => "hidden",
                ModelVisibility::Drawn => "opaque",
            };
            *blended_models.entry(label.to_string()).or_default() += 1;
            for blk in &m.world.blocks {
                for s in &blk.sections {
                    if label == "sky" {
                        *sky_kinds.entry(s.shader_kind).or_default() += 1;
                    }
                    if matches!(vis, ModelVisibility::Translucent { .. }) {
                        *blended_kinds
                            .entry((label.to_string(), s.shader_kind))
                            .or_default() += 1;
                    }
                }
            }
        }
        let sky_objs = w.objects_with_role(ObjectRole::SkyWorldModel).count() as u64;
        let linked = w
            .objects_with_role(ObjectRole::SkyWorldModel)
            .filter(|o| r.world.models.iter().any(|m| Some(m.name) == o.name()))
            .count() as u64;
        sky.0 += sky_objs;
        sky.1 += linked;
        if sky_objs > 0 {
            sky.2 += 1;
        }
        if linked > 0 {
            sky.3 += 1;
        }
        println!("{map}: static model lights {map_lights}, sky objects {sky_objs} (render-linked {linked})");
    }
    println!("light objects per class (0 point, 1 directional, 2 object): {classes:?}; static lights created {created}");
    println!("radius range {rmin:.1}..{rmax:.1}, largest colour channel {cmax:.1}");
    println!("falloff forms: {falloff:#?}");
    println!("shapes: {shapes:?}");
    println!("convert-to-ambient: {to_amb:?}");
    println!("light object properties (hash, value form): {props:#?}");
    println!("world models by pass: {blended_models:?}");
    println!("section kinds of blended world models: {blended_kinds:?}");
    println!(
        "sky objects {} (render-linked {}), maps with sky objects {} (with a linked render model {}), sky \
         section kinds {sky_kinds:?}, sky portals {portals}",
        sky.0, sky.1, sky.2, sky.3
    );
}
