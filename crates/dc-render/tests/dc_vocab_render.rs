//! The renderer's readers on a world written in DeCombat's own map vocabulary (docs/MAPKIT_VOCAB.md, MAP-26): the
//! fog from `dc.world_settings`, the sun from `dc.global_light`, static lights from `dc.point_light`, the
//! translucent pass from `dc.world_model`, and the sky-portal count of the scene stats. No ROM, no GPU: every
//! object here is synthetic and carries our own names only.

use dc_render::{
    global_light, model_visibility, static_light, world_far_z, world_fog, world_lights, LightShape,
    ModelVisibility, RenderVertex, SceneMesh, SceneOptions, TextureTraits,
};
use dc_world::synth::{
    SynthBlock, SynthObject, SynthRenderWorld, SynthSection, SynthValue, SynthWorld,
};
use dc_world::World;
use glam::Vec3;

fn text(s: &str) -> SynthValue {
    SynthValue::Text(s.as_bytes().to_vec())
}

fn vtx(p: [f32; 3]) -> [u8; 68] {
    RenderVertex {
        pos: p,
        uv0: [0.0, 0.0],
        uv1: [0.0, 0.0],
        colour: 0xff80_8080,
        normal: [0.0, 0.0, -1.0],
        tangent: [1.0, 0.0, 0.0],
        binormal: [0.0, 1.0, 0.0],
    }
    .to_bytes()
}

/// One kind-1 quad in a block, at depth `z`.
fn quad_block(z: f32) -> SynthBlock {
    SynthBlock {
        center: [0.0, 0.0, z],
        half: [50.0, 50.0, 1.0],
        sections: vec![SynthSection {
            textures: [b"own/base".to_vec(), Vec::new()],
            shader_kind: 1,
            triangles: 2,
            ..SynthSection::default()
        }],
        vertices: vec![
            vtx([-40.0, -20.0, z]),
            vtx([-40.0, 20.0, z]),
            vtx([40.0, 20.0, z]),
            vtx([40.0, -20.0, z]),
        ],
        triangles: vec![[0, 1, 2, 0], [0, 2, 3, 0]],
        ..SynthBlock::default()
    }
}

/// A world with a level block (one sky portal), a nested render model `crate_1`, and `objects`.
fn world_with(objects: Vec<SynthObject>) -> Vec<u8> {
    let mut level = quad_block(100.0);
    level.sky_portals = vec![(
        vec![
            [-10.0, 60.0, 100.0],
            [-10.0, 90.0, 100.0],
            [10.0, 90.0, 100.0],
            [10.0, 60.0, 100.0],
        ],
        [0.0, 0.0, -1.0],
        -100.0,
    )];
    SynthWorld {
        extent_min: [-200.0; 3],
        extent_max: [200.0; 3],
        tree_shape: vec![false],
        grid_cell: [400.0; 3],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![90, 100, 110],
        objects,
        render: SynthRenderWorld {
            blocks: vec![level],
            models: vec![(
                b"crate_1".to_vec(),
                SynthRenderWorld {
                    blocks: vec![quad_block(150.0)],
                    models: vec![],
                },
            )],
        },
        ..SynthWorld::default()
    }
    .build()
}

fn mesh_of(data: &[u8]) -> SceneMesh {
    let world = World::parse(data).expect("synthetic world parses");
    SceneMesh::build(&world, SceneOptions::default(), &mut |_| TextureTraits {
        present: true,
        mode: 0,
        hi: false,
    })
}

#[test]
fn dc_world_settings_yields_the_far_plane_above_fifty() {
    let settings = |far: f32| {
        world_with(vec![SynthObject::dc(
            "dc.world_settings",
            "settings",
            [0.0; 3],
            [0.0; 3],
            &[("far_z", SynthValue::Real(far))],
        )])
    };
    let far = |data: Vec<u8>| world_far_z(&World::parse(&data).unwrap());
    assert_eq!(far(settings(30000.0)), Some(30000.0));
    // the 2009 client applies the variable only above 50: the camera keeps its own far plane
    assert_eq!(far(settings(50.0)), None);
    assert_eq!(far(settings(f32::NAN)), None);
    // no world settings: none
    assert_eq!(far(world_with(vec![])), None);
}

#[test]
fn dc_world_settings_yields_the_fog_of_both_passes() {
    let data = world_with(vec![SynthObject::dc(
        "dc.world_settings",
        "settings",
        [0.0; 3],
        [0.0; 3],
        &[
            ("far_z", SynthValue::Real(6000.0)),
            ("fog_enable", SynthValue::Bool(true)),
            ("fog_near", SynthValue::Real(600.0)),
            ("fog_far", SynthValue::Real(5000.0)),
            ("fog_color", SynthValue::Rgb([10.0, 20.5, 300.0])),
            ("sky_fog_enable", SynthValue::Bool(true)),
            ("sky_fog_near", SynthValue::Real(1000.0)),
            ("sky_fog_far", SynthValue::Real(9000.0)),
        ],
    )]);
    let world = World::parse(&data).unwrap();
    let (fog, sky) = world_fog(&world);
    let fog = fog.expect("world fog on");
    assert_eq!((fog.near, fog.far), (600.0, 5000.0));
    // components truncated and clamped to 0..255, as the integer console values hold them
    assert_eq!(fog.colour, [10.0, 20.0, 255.0]);
    let sky = sky.expect("sky fog follows the world's enable");
    assert_eq!((sky.near, sky.far), (1000.0, 9000.0));
    assert_eq!(sky.colour, fog.colour);
    // the scene build carries it
    let mesh = mesh_of(&data);
    assert_eq!(mesh.fog, Some(fog));
    assert_eq!(mesh.sky_fog, Some(sky));

    // fog off: no fog, whatever the distances say
    let off = world_with(vec![SynthObject::dc(
        "dc.world_settings",
        "settings",
        [0.0; 3],
        [0.0; 3],
        &[
            ("fog_enable", SynthValue::Bool(false)),
            ("fog_near", SynthValue::Real(600.0)),
            ("fog_far", SynthValue::Real(5000.0)),
        ],
    )]);
    assert_eq!(world_fog(&World::parse(&off).unwrap()), (None, None));
    // no world settings at all: no fog
    let none = world_with(vec![]);
    assert_eq!(world_fog(&World::parse(&none).unwrap()), (None, None));
}

#[test]
fn dc_global_light_yields_the_sun() {
    // rot = euler radians; yaw pi/2 about y turns +Z into +X
    let data = world_with(vec![SynthObject::dc(
        "dc.global_light",
        "sun",
        [0.0, 500.0, 0.0],
        [0.0, std::f32::consts::FRAC_PI_2, 0.0],
        &[
            ("color", SynthValue::Rgb([200.0, 100.0, 50.0])),
            ("bright", SynthValue::Real(0.5)),
            ("to_ambient", SynthValue::Real(0.25)),
        ],
    )]);
    let world = World::parse(&data).unwrap();
    assert_eq!(dc_render::global_light_count(&world), 1);
    let g = global_light(&world).expect("the sun");
    assert!((g.dir - Vec3::X).length() < 1e-5, "{}", g.dir);
    assert_eq!(g.colour, Vec3::new(100.0, 50.0, 25.0));
    assert_eq!(g.to_ambient, 0.25);
    let sun = g.light();
    assert!(matches!(sun.shape, LightShape::Directional { .. }));
    assert_eq!(sun.to_ambient, 0.25);
    // a sun is not a static light of the scan (it is not a point / directional / object light class)
    assert!(world_lights(&world).is_empty());
    // and a world without one has no sun
    assert!(global_light(&World::parse(&world_with(vec![])).unwrap()).is_none());
}

#[test]
fn dc_point_light_needs_lights_objects_and_a_radius() {
    let light = |props: &[(&str, SynthValue)]| {
        let data = world_with(vec![SynthObject::dc(
            "dc.point_light",
            "lamp",
            [10.0, 20.0, 30.0],
            [0.0; 3],
            props,
        )]);
        let world = World::parse(&data).unwrap();
        let o = &world.objects[0];
        assert_eq!(dc_render::light_class(o), Some(0));
        let direct = static_light(o);
        assert_eq!(world_lights(&world).len(), direct.iter().count());
        direct
    };
    // the full set: a point light with the dynamic law, the colour scaled by `bright`
    let l = light(&[
        ("color", SynthValue::Rgb([255.0, 128.0, 64.0])),
        ("radius", SynthValue::Real(300.0)),
        ("bright", SynthValue::Real(2.0)),
        ("to_ambient", SynthValue::Real(0.5)),
        ("lights_objects", SynthValue::Bool(true)),
    ])
    .expect("a static light");
    assert_eq!(l.shape, LightShape::Point);
    assert_eq!(l.pos, Vec3::new(10.0, 20.0, 30.0));
    assert_eq!(l.radius, 300.0);
    assert_eq!(l.colour, Vec3::new(510.0, 256.0, 128.0));
    assert_eq!(l.to_ambient, 0.5);
    assert!((l.attenuation(0.0) - 1.0).abs() < 1e-6);
    assert!((l.attenuation(150.0 * 150.0) - 1.0 / (1.0 + 19.0 / 4.0)).abs() < 1e-6);
    // a narrow cone makes a spot light along the object's +Z
    let spot = light(&[
        ("radius", SynthValue::Real(300.0)),
        ("cone", SynthValue::Real(60.0)),
        ("lights_objects", SynthValue::Bool(true)),
    ])
    .expect("a spot light");
    assert!(
        matches!(spot.shape, LightShape::Spot { dir, cos_half } if (dir - Vec3::Z).length() < 1e-5 && (cos_half - 30f32.to_radians().cos()).abs() < 1e-5),
        "{:?}",
        spot.shape
    );
    // without `lights_objects` true, or without a radius above zero: no light
    assert!(light(&[("radius", SynthValue::Real(300.0))]).is_none());
    assert!(light(&[
        ("radius", SynthValue::Real(300.0)),
        ("lights_objects", SynthValue::Bool(false)),
    ])
    .is_none());
    assert!(light(&[("lights_objects", SynthValue::Bool(true))]).is_none());
    assert!(light(&[
        ("radius", SynthValue::Real(0.0)),
        ("lights_objects", SynthValue::Bool(true)),
    ])
    .is_none());
    // the names are matched without ASCII case
    assert!(light(&[
        ("RADIUS", SynthValue::Real(100.0)),
        ("Lights_Objects", SynthValue::Bool(true)),
    ])
    .is_some());
}

#[test]
fn dc_world_model_blend_classifies_the_model() {
    let model = |props: &[(&str, SynthValue)]| {
        let data = world_with(vec![SynthObject::dc(
            "dc.world_model",
            "crate_1",
            [0.0, 0.0, 150.0],
            [0.0; 3],
            props,
        )]);
        let world = World::parse(&data).unwrap();
        let vis = model_visibility(world.objects.first());
        let mesh = mesh_of(&data);
        (vis, mesh)
    };
    // translucent with an alpha: the translucent pass, alpha blended
    let (vis, mesh) = model(&[
        ("blend", text("translucent")),
        ("alpha", SynthValue::Real(0.5)),
    ]);
    assert_eq!(
        vis,
        ModelVisibility::Translucent {
            additive: false,
            alpha: 0.5
        }
    );
    assert_eq!(mesh.stats.blended_models, 1);
    assert_eq!(mesh.stats.blended_additive, 0);
    assert_eq!(mesh.stats.world_models, 0);
    assert_eq!(mesh.blended.len(), 1);
    assert_eq!(mesh.blended[0].alpha, 0.5);
    assert!(!mesh.blended[0].additive);
    assert_eq!(mesh.blended[0].origin, [0.0, 0.0, 150.0]);
    // additive
    let (vis, mesh) = model(&[("blend", text("additive"))]);
    assert_eq!(
        vis,
        ModelVisibility::Translucent {
            additive: true,
            alpha: 1.0
        }
    );
    assert_eq!(mesh.stats.blended_additive, 1);
    // opaque (and the default): drawn with the world
    let (vis, mesh) = model(&[("blend", text("opaque"))]);
    assert_eq!(vis, ModelVisibility::Drawn);
    assert_eq!(mesh.stats.world_models, 1);
    assert_eq!(mesh.stats.blended_models, 0);
    let (vis, _) = model(&[]);
    assert_eq!(vis, ModelVisibility::Drawn);
    // invisible: left out
    let (vis, mesh) = model(&[("visible", SynthValue::Bool(false))]);
    assert_eq!(vis, ModelVisibility::Hidden);
    assert_eq!(mesh.stats.hidden_models_skipped, 1);
    assert_eq!(mesh.stats.world_models, 0);
    // the value is matched without ASCII case too
    let (vis, _) = model(&[("bLEND", text("tRANSLUCENT"))]);
    assert!(matches!(
        vis,
        ModelVisibility::Translucent {
            additive: false,
            ..
        }
    ));
}

#[test]
fn scene_stats_count_the_level_blocks_sky_portals() {
    let mesh = mesh_of(&world_with(vec![]));
    assert_eq!(mesh.stats.sky_portals, 1);
    assert_eq!(mesh.sky_portals.len(), 1);
    assert_eq!(mesh.sky_portals[0].len(), 4);
    // a level block without portals: none (the nested models' blocks are never collected)
    let plain = SynthWorld {
        extent_min: [-200.0; 3],
        extent_max: [200.0; 3],
        tree_shape: vec![false],
        grid_cell: [400.0; 3],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![90, 100, 110],
        render: SynthRenderWorld {
            blocks: vec![quad_block(100.0)],
            models: vec![(
                b"crate_1".to_vec(),
                SynthRenderWorld {
                    blocks: vec![quad_block(150.0)],
                    models: vec![],
                },
            )],
        },
        ..SynthWorld::default()
    }
    .build();
    let plain = mesh_of(&plain);
    assert_eq!(plain.stats.sky_portals, 0);
    assert_eq!(plain.stats.world_models, 1);
}
