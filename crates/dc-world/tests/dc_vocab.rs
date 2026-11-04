//! The DeCombat map vocabulary end to end: a synthetic world written in the `dc.*` naming is parsed and every
//! class resolves to its role, every property reads back through the alias keys, and a world that mixes the
//! vocabulary with the fixtures' unknown classes keeps both apart. No ROM is needed.
// DC-DEFAULT: MAP-1, MAP-2, MAP-3

use dc_world::names;
use dc_world::synth::*;
use dc_world::vocab::{self, CLASSES, LEADING};
use dc_world::*;

/// A closed unit box as a brush model named `name` (flags bit 1 = a model an object names).
fn brush_model(name: &str, origin: Vec3) -> SynthModel {
    SynthModel {
        flags: 2,
        name: name.as_bytes().to_vec(),
        textures: vec![b"@mod:test/assets/t.dtx".to_vec()],
        planes: vec![([0.0, 1.0, 0.0], origin[1] + 1.0)],
        surfaces: vec![(0x1, 0, 0)],
        polys: vec![(0, 0, vec![0, 1, 2, 3])],
        nodes: vec![(0, 0, -1, -2)],
        points: vec![
            [origin[0] - 1.0, origin[1] + 1.0, origin[2] - 1.0],
            [origin[0] + 1.0, origin[1] + 1.0, origin[2] - 1.0],
            [origin[0] + 1.0, origin[1] + 1.0, origin[2] + 1.0],
            [origin[0] - 1.0, origin[1] + 1.0, origin[2] + 1.0],
        ],
        root: 0,
        box_min: [origin[0] - 1.0, origin[1] - 1.0, origin[2] - 1.0],
        box_max: [origin[0] + 1.0, origin[1] + 1.0, origin[2] + 1.0],
        origin,
    }
}

fn text(s: &str) -> SynthValue {
    SynthValue::Text(s.as_bytes().to_vec())
}

/// One object per class of the vocabulary, with every property of the class set to a recognisable value.
fn vocabulary_world() -> SynthWorld {
    let mut models = vec![SynthModel {
        flags: 0,
        name: Vec::new(),
        root: -1,
        ..SynthModel::default()
    }];
    let mut objects = Vec::new();
    for (i, c) in CLASSES.iter().enumerate() {
        let name = format!("obj_{}", c.name.trim_start_matches("dc."));
        let pos = [i as f32, 10.0 + i as f32, 20.0 + i as f32];
        let rot = [0.0, 0.25 * i as f32, 0.0];
        let props: Vec<(&str, SynthValue)> = c
            .props
            .iter()
            .map(|p| {
                let v = match p.kind {
                    PropKind::Text => match p.name {
                        "team" | "defuse_team" => text("2"),
                        "modes" => text("TDM,FFA"),
                        "blend" => text("additive"),
                        "damage_kind" => text("fall"),
                        _ => text("@mod:test/assets/x.bin"),
                    },
                    PropKind::Vec3 => SynthValue::Vec3([3.0, 4.0, 5.0]),
                    PropKind::Rgb => SynthValue::Rgb([10.0, 20.0, 30.0]),
                    PropKind::Real => SynthValue::Real(1.5),
                    PropKind::Bool => SynthValue::Bool(true),
                    PropKind::Int => SynthValue::Int(7.0),
                    PropKind::Bits => SynthValue::Bits(1.0),
                    PropKind::Euler => SynthValue::Euler([0.0; 4]),
                    PropKind::Other(_) => unreachable!(),
                };
                (p.name, v)
            })
            .collect();
        if c.geometry == Geometry::Brush {
            models.push(brush_model(&name, pos));
        }
        objects.push(SynthObject::dc(c.name, &name, pos, rot, &props));
    }
    SynthWorld {
        info: Vec::new(),
        extent_min: [-64.0; 3],
        extent_max: [64.0; 3],
        tree_shape: vec![false],
        models,
        objects,
        grid_dims: [1, 1, 1],
        grid_rgb: vec![0x80, 0x80, 0x80],
        ..SynthWorld::default()
    }
}

#[test]
fn dc_world_one_object_per_class_resolves() {
    let bytes = vocabulary_world().build();
    let w = World::parse(&bytes).expect("parse");
    w.check_layout().expect("layout");
    assert_eq!(w.objects.len(), CLASSES.len());
    for (i, (o, c)) in w.objects.iter().zip(CLASSES.iter()).enumerate() {
        // class: role and key
        assert_eq!(o.class_name, c.name.as_bytes());
        assert_eq!(o.role, c.role, "{}", c.name);
        assert!(o.is_class(c.key()), "{}", c.name);
        assert_eq!(vocab::class_of(o.class_name).map(|s| s.name), Some(c.name));
        // the three leading properties by index, with the §6.1 flags
        assert_eq!(o.props[0].name, b"name");
        assert_eq!(o.props[1].name, b"pos");
        assert_eq!(o.props[2].name, b"rot");
        assert_eq!(o.props[0].flags, 0);
        assert_eq!(o.props[1].flags, 0x0010_0000);
        assert_eq!(o.props[2].flags, 0);
        let expect_pos = [i as f32, 10.0 + i as f32, 20.0 + i as f32];
        assert_eq!(o.position(), Some(expect_pos));
        assert_eq!(o.rotation(), Some([0.0, 0.25 * i as f32, 0.0]));
        // and by alias name
        assert_eq!(o.vec3(PropKey::Position), Some(expect_pos));
        assert_eq!(o.text(PropKey::Name), o.name());
        assert!(o.prop_by_key(PropKey::Rotation).is_some());
        // every property of the class reads back under its name
        for p in c.props {
            let got = o.prop_named(
                names::keys_named(p.name)
                    .next()
                    .expect("a key per vocabulary name"),
            );
            let got = got.unwrap_or_else(|| panic!("{}.{} missing", c.name, p.name));
            assert_eq!(got.kind, p.kind, "{}.{}", c.name, p.name);
            assert_eq!(got.flags, u32::from(p.name == "extent") * 0x4);
        }
        // brush classes link to their model by name, the model origin equals the position
        if c.geometry == Geometry::Brush {
            let m = w.brush_of(o).unwrap_or_else(|| panic!("{} brush", c.name));
            assert_eq!(m.origin, expect_pos);
            assert!(m.is_dual());
        } else {
            assert!(w.brush_of(o).is_none());
        }
    }
    // the level model is the one no object names
    let level: Vec<_> = w
        .models
        .iter()
        .filter(|m| w.object_named(m.name).is_none())
        .collect();
    assert_eq!(level.len(), 1);
    assert!(!level[0].is_dual());

    // the role-specific reads of the server
    let spawn = w.objects_with_role(ObjectRole::PlayerSpawn).next().unwrap();
    assert_eq!(spawn.text(PropKey::TeamFilter), Some(&b"2"[..]));
    assert_eq!(spawn.text(PropKey::ModeFilter), Some(&b"TDM,FFA"[..]));
    let site = w.objects_with_role(ObjectRole::BombSite).next().unwrap();
    assert_eq!(site.vec3(PropKey::BoxExtent), Some([3.0, 4.0, 5.0]));
    assert_eq!(
        site.prop_named(&names::PROP_NUMBER).map(|p| p.value),
        Some(FieldValue::Int(7))
    );
    assert_eq!(
        site.prop_named(&names::PROP_DEFUSE_TEAM).map(|p| p.value),
        Some(FieldValue::Text(b"2"))
    );
    let zone = w.objects_with_role(ObjectRole::FlagZone).next().unwrap();
    assert_eq!(zone.text(PropKey::Team), Some(&b"2"[..]));
    assert_eq!(zone.vec3(PropKey::BoxExtent), Some([3.0, 4.0, 5.0]));
    let flag = w.objects_with_role(ObjectRole::CaptureFlag).next().unwrap();
    assert_eq!(flag.text(PropKey::TeamFilter), Some(&b"2"[..]));
    assert!(flag.prop_named(&names::PROP_MODEL).is_some());
    let kill = w.objects_with_role(ObjectRole::KillVolume).next().unwrap();
    assert_eq!(
        kill.prop_named(&names::PROP_PENDING_SEC).map(|p| p.value),
        Some(FieldValue::Real(1.5))
    );
    assert_eq!(kill.real(PropKey::Damage), Some(1.5));
    let model = w
        .objects_with_role(ObjectRole::PlainWorldModel)
        .next()
        .unwrap();
    let blend = model.prop_named(&names::PROP_BLEND).unwrap();
    let FieldValue::Text(t) = blend.value else {
        panic!("blend is text")
    };
    assert!(names::BLEND_ADDITIVE.matches(t));
    assert!(!names::BLEND_TRANSLUCENT.matches(t));
    let sky = w
        .objects_with_role(ObjectRole::SkyWorldModel)
        .next()
        .unwrap();
    assert_eq!(
        sky.prop_named(&names::PROP_SKY_INNER).map(|p| p.value),
        Some(FieldValue::Vec3([3.0, 4.0, 5.0]))
    );
    assert!(sky.prop_named(&names::SKY_INNER_PERCENT[0]).is_none());
    let settings = w
        .objects_with_role(ObjectRole::WorldSettings)
        .next()
        .unwrap();
    assert_eq!(
        settings.prop_named(&names::PROP_FOG_COLOR).map(|p| p.value),
        Some(FieldValue::Rgb([10.0, 20.0, 30.0]))
    );
    assert_eq!(
        settings.prop_named(&names::PROP_WIND).map(|p| p.value),
        Some(FieldValue::Vec3([3.0, 4.0, 5.0]))
    );
    let light = w
        .objects
        .iter()
        .find(|o| o.is_class(&names::CLASS_POINT_LIGHT))
        .unwrap();
    assert_eq!(light.role, None);
    assert_eq!(
        light
            .prop_named(&names::PROP_LIGHTS_OBJECTS)
            .map(|p| p.value),
        Some(FieldValue::Bool(true))
    );
    assert_eq!(
        light.prop_named(&names::PROP_LIGHT_COLOR).map(|p| p.value),
        Some(FieldValue::Rgb([10.0, 20.0, 30.0]))
    );
    assert!(light.prop_named(&names::PROP_LIGHT_INNER_COLOR).is_none());
    let sound = w
        .objects
        .iter()
        .find(|o| o.is_class(&names::CLASS_SOUND))
        .unwrap();
    assert!(sound.prop_named(&names::PROP_SOUND_FILE).is_some());
    let sprite = w
        .objects
        .iter()
        .find(|o| o.is_class(&names::CLASS_SPRITE))
        .unwrap();
    assert!(sprite.prop_named(&names::PROP_SPRITE_FILE).is_some());
    assert!(sprite.prop_named(&names::PROP_SPRITE_COLOR).is_some());
    // the sprite's `color` is not the light colour key and vice versa
    assert!(sprite.prop_named(&names::PROP_LIGHT_COLOR).is_some()); // same text, alias matches both
    assert!(w
        .objects_with_role(ObjectRole::TriggerVolume)
        .next()
        .is_none());
}

#[test]
fn class_names_are_matched_without_case() {
    let mut d = vocabulary_world();
    for o in &mut d.objects {
        o.class = o.class.to_ascii_uppercase();
        for p in &mut o.props {
            p.name = p.name.to_ascii_uppercase();
        }
    }
    let bytes = d.build();
    let w = World::parse(&bytes).unwrap();
    for (o, c) in w.objects.iter().zip(CLASSES.iter()) {
        assert_eq!(o.role, c.role);
        assert!(o.is_class(c.key()));
        assert!(o.position().is_some());
        assert_eq!(o.text(PropKey::Name), o.name());
    }
    let spawn = w.objects_with_role(ObjectRole::PlayerSpawn).next().unwrap();
    assert_eq!(spawn.text(PropKey::TeamFilter), Some(&b"2"[..]));
}

#[test]
fn mixed_world_keeps_rom_style_and_dc_objects_apart() {
    const NAMES: [&[u8]; 3] = [b"Nm", b"Pos", b"Rot"];
    let mut d = vocabulary_world();
    // the synthetic fixtures' objects: unknown classes under their own property names (as a ROM world is to
    // a reader without the hash table), read by index only
    let mut legacy = SynthObject::point(
        b"SynthSpawn",
        NAMES,
        b"spawn01",
        [1.0, 2.0, 3.0],
        [0.0, 1.5, 0.0],
    );
    legacy.props.push(SynthProp {
        name: b"Side".to_vec(),
        flags: 0,
        value: SynthValue::Text(b"1".to_vec()),
    });
    d.objects.insert(0, legacy);
    d.objects.push(SynthObject::point(
        b"SynthBrush",
        NAMES,
        b"brush_z",
        [0.0; 3],
        [0.0; 3],
    ));
    d.models.push(brush_model("brush_z", [0.0; 3]));
    let bytes = d.build();
    let w = World::parse(&bytes).unwrap();
    w.check_layout().unwrap();
    let legacy = &w.objects[0];
    assert_eq!(legacy.role, None);
    assert_eq!(legacy.name(), Some(&b"spawn01"[..]));
    assert_eq!(legacy.position(), Some([1.0, 2.0, 3.0]));
    // "Side" is neither a ROM hash nor an own name: the keys do not see it; "Nm" is not an alias either
    assert_eq!(legacy.text(PropKey::TeamFilter), None);
    assert_eq!(legacy.text(PropKey::Team), None);
    assert_eq!(
        legacy.prop(b"Side").map(|p| p.value),
        Some(FieldValue::Text(b"1"))
    );
    assert!(legacy.prop_by_key(PropKey::Name).is_none());
    assert_eq!(vocab::class_of(legacy.class_name), None);
    let brush = w.object_named(b"brush_z").unwrap();
    assert_eq!(brush.role, None);
    assert!(w.brush_of(brush).is_some());
    for (o, c) in w.objects[1..].iter().zip(CLASSES.iter()) {
        assert_eq!(o.role, c.role);
        assert_eq!(o.text(PropKey::Name), o.name());
    }
    assert_eq!(w.objects_with_role(ObjectRole::PlayerSpawn).count(), 1);
    assert_eq!(w.class_names().len(), CLASSES.len() + 2);
}

#[test]
fn synth_dc_helpers() {
    let p = SynthProp::dc("extent", SynthValue::Vec3([1.0; 3]));
    assert_eq!(p.flags, 0x4);
    let p = SynthProp::dc("EXTENT", SynthValue::Vec3([1.0; 3]));
    assert_eq!(p.flags, 0x4);
    let p = SynthProp::dc("wind", SynthValue::Vec3([1.0; 3]));
    assert_eq!(p.flags, 0);
    let p = SynthProp::dc("extent", SynthValue::Real(1.0));
    assert_eq!(p.flags, 0);
    let o = SynthObject::dc(
        "dc.player_spawn",
        "s",
        [1.0; 3],
        [2.0; 3],
        &[("team", text("1"))],
    );
    assert_eq!(o.props.len(), 4);
    assert_eq!(o.props[3].name, b"team");
    assert_eq!(o.props[2].value, SynthValue::Euler([2.0, 2.0, 2.0, 0.0]));
}

/// MAP-2 (review C5): the own-name half of a property key applies on `dc.*` objects only. A `dc.world_model`
/// resolves `rayhit`, `visible` and `solid` by their own names (in any ASCII case); an object of an unknown class
/// that spells the same property names does not resolve them through the keys (as a ROM object whose ROM
/// property happens to spell a vocabulary word must not), though the plain name lookup still sees them.
#[test]
fn own_property_names_resolve_on_dc_objects_only() {
    const NAMES: [&[u8]; 3] = [b"name", b"pos", b"rot"];
    let mut d = vocabulary_world();
    let spelled = |class: &[u8], name: &[u8]| {
        let mut o = SynthObject::point(class, NAMES, name, [0.0; 3], [0.0; 3]);
        for (n, v) in [
            (&b"rayhit"[..], SynthValue::Bool(false)),
            (b"VISIBLE", SynthValue::Bool(false)),
            (b"solid", SynthValue::Bool(false)),
            (b"team", SynthValue::Text(b"2".to_vec())),
            (b"radius", SynthValue::Real(300.0)),
        ] {
            o.props.push(SynthProp {
                name: n.to_vec(),
                flags: 0,
                value: v,
            });
        }
        o
    };
    d.objects.push(spelled(b"DC.World_Model", b"brush_dc"));
    d.models.push(brush_model("brush_dc", [0.0; 3]));
    d.objects.push(spelled(b"SynthWater", b"brush_rom"));
    d.models.push(brush_model("brush_rom", [0.0; 3]));
    let bytes = d.build();
    let w = World::parse(&bytes).unwrap();
    let dc = w.object_named(b"brush_dc").unwrap();
    let rom = w.object_named(b"brush_rom").unwrap();
    assert!(dc.dc);
    assert_eq!(dc.role, Some(ObjectRole::PlainWorldModel));
    assert!(dc.is_class(&names::CLASS_WORLD_MODEL));
    assert!(!rom.dc);
    assert_eq!(rom.role, None);
    for k in [
        &names::PROP_RAYHIT,
        &names::PROP_VISIBLE,
        &names::PROP_SOLID,
    ] {
        assert_eq!(
            dc.prop_named(k).map(|p| p.value),
            Some(FieldValue::Bool(false)),
            "{:?}",
            k.ours
        );
        assert!(rom.prop_named(k).is_none(), "{:?}", k.ours);
    }
    assert_eq!(dc.text(PropKey::TeamFilter), Some(&b"2"[..]));
    assert_eq!(rom.text(PropKey::TeamFilter), None);
    assert_eq!(rom.text(PropKey::Team), None);
    assert!(dc.prop_named(&names::PROP_RADIUS).is_some());
    assert!(rom.prop_named(&names::PROP_RADIUS).is_none());
    // the leading three resolve by index on both, by key only on the vocabulary object
    assert_eq!(rom.name(), Some(&b"brush_rom"[..]));
    assert!(rom.position().is_some());
    assert!(rom.prop_by_key(PropKey::Name).is_none());
    assert_eq!(dc.text(PropKey::Name), dc.name());
    // the plain lookup by exact bytes still finds the spelled names on either
    assert!(rom.prop(b"rayhit").is_some());
    assert!(dc.prop(b"rayhit").is_some());
    // every object of the vocabulary world is flagged, and none of the fixtures' classes is
    for (o, c) in w.objects.iter().zip(CLASSES.iter()) {
        assert!(o.dc, "{}", c.name);
    }
}

/// ROM-gated: the hash half of the alias rule still resolves every ROM world's objects exactly as the
/// hash-only reader did (roles by `class_hash`, the leading properties by hash equal to the index reads).
#[test]
fn rom_worlds_resolve_through_the_hash_half() {
    let Ok(rom) = dc_rez::dc_rom::resolve_rom() else {
        eprintln!("CA_ROM unset: skipped");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let mut worlds = 0;
    let mut roles = 0;
    let mut misfires_checked = 0usize;
    let mut sky_polys = 0usize;
    for (name, _, _) in vfs.list_entries("WORLDS.rez").expect("list") {
        if !name.to_ascii_uppercase().ends_with(".DAT") {
            continue;
        }
        let data = vfs
            .entry("WORLDS.rez", &name)
            .expect("entry")
            .expect("present");
        let w = World::parse_with(&data, ParseOptions { render: false }).expect("parse");
        worlds += 1;
        // every sky face (surface flag 0x10) carries the sky surface id as its texflags: the number the client's
        // sky guard, the server's MAP-24 answer and dc-mapkit's sky faces share through `SURFACE_SKY`
        for m in &w.models {
            for s in m.surfaces.iter().filter(|s| s.flags & 0x10 != 0) {
                assert_eq!(s.texflags, SURFACE_SKY, "{name}: sky surface texflags");
                sky_polys += 1;
            }
        }
        // the misfire scan of review C4/C5: on a ROM object the own-name half must never pick a property the
        // hash half did not (3,546 (object, key) differences before the `Object::dc` gate), for every key that has
        // an own name
        for o in &w.objects {
            let by_hash = ObjectRole::ALL
                .into_iter()
                .find(|r| r.class_hash() == (hash16(o.class_name), o.class_name.len()));
            assert_eq!(o.role, by_hash, "{name}");
            roles += usize::from(o.role.is_some());
            assert_eq!(o.text(PropKey::Name), o.name(), "{name}");
            assert_eq!(o.vec3(PropKey::Position), o.position(), "{name}");
            assert!(o.prop_by_key(PropKey::Rotation).is_some(), "{name}");
            // a ROM world never carries a `dc.*` class
            assert!(!o.class_name.starts_with(b"dc."), "{name}");
            assert!(!o.dc, "{name}");
            for k in names::ALL.iter().filter(|k| k.has_ours()) {
                let first_by_hash = o.props.iter().find(|p| k.matches_hash(p.name));
                let first_by_key = o.prop_named(k);
                let same = match (first_by_hash, first_by_key) {
                    (Some(a), Some(b)) => std::ptr::eq(a, b),
                    (None, None) => true,
                    _ => false,
                };
                assert!(
                    same,
                    "{name}: class {} key {:?} resolves differently through the own name",
                    hash16_hex(o.class_name),
                    k.ours
                );
                misfires_checked += 1;
            }
            // a ROM class the vocabulary covers resolves to the same role through either half
            if let Some(c) = vocab::class_of(o.class_name) {
                assert_eq!(c.role, o.role, "{name}");
            }
        }
    }
    assert!(worlds > 0 && roles > 0);
    assert!(misfires_checked > 0);
    assert!(sky_polys > 0, "the ROM worlds carry sky surfaces");
}

#[test]
fn vocab_doc_carries_the_generated_tables() {
    let doc = include_str!("../../../docs/MAPKIT_VOCAB.md").replace("\r\n", "\n");
    let generated = vocab::markdown();
    assert!(
        doc.contains(&generated),
        "docs/MAPKIT_VOCAB.md is out of step: run `cargo run -p dc-world --example vocab_md` and paste the \
         output between the generated-block markers"
    );
    for c in CLASSES.iter() {
        assert!(doc.contains(&format!("`{}`", c.name)), "{}", c.name);
        for p in c.props {
            assert!(
                doc.contains(&format!("`{}`", p.name)),
                "{}.{}",
                c.name,
                p.name
            );
        }
    }
    for p in LEADING.iter() {
        assert!(doc.contains(&format!("`{}`", p.name)));
    }
    for id in ["MAP-1", "MAP-2", "MAP-3"] {
        assert!(doc.contains(id), "{id}");
    }
}
