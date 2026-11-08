//! Two-sided worlds (DC-DEFAULT: MAP-118): DeCombat's own `two_sided` world setting draws the back faces of the
//! opaque world after its front faces, with the depth biased away from the eye so that a front face on the same plane
//! wins, but filler (vertex-lit, one flat colour) loses to a textured back face, and filler and cut-out cards keep
//! their backs hidden.
//!
//! - No GPU: the setting comes from the world's first world-settings object (`dc.world_settings`); absent = off, and
//!   on any other object it does nothing; a composed map keeps its own.
//! - Synthetic (any headless adapter, skipped without one): a one-sided scene culls its back faces and records no
//!   back-face draw (the default path); a two-sided scene draws a lone back face exactly as the same quad facing the
//!   eye; a pair of opposite faces on one plane shows only the front face's pixels (near, far, at a grazing angle,
//!   either draw order, the other diagonal); flat and alpha-tested back faces stay culled; a textured back face shows
//!   over its flat twin; a back face's lightmap and multiply draws still meet at equal depth; a one-sided frame is the
//!   same bytes before and after the renderer drew a two-sided one.
//! - ROM-gated (`CA_ROM`): no ROM world turns it on; every map from its first spawn with the back faces forced on
//!   prints the draws and frame times both ways, and how many triangles of its render data have an exact opposite
//!   twin (the coplanar pairs the depth bias is for); from each map's spawn at eight directions the back faces change
//!   at most a few pixels (a bound on the share, the one ROM-backed assertion beyond "one-sided"). No image is written.

use dc_render::*;
use dc_world::synth::{SynthObject, SynthValue, SynthWorld};
use dc_world::World;
use glam::{Quat, Vec3};

const W: u32 = 160;
const H: u32 = 120;

fn context() -> Option<GpuContext> {
    match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => Some(c),
        Err(e) => {
            println!("skipped: {e}");
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the setting

fn settings(props: &[(&str, SynthValue)]) -> SynthObject {
    SynthObject::dc("dc.world_settings", "settings", [0.0; 3], [0.0; 3], props)
}

fn world_bytes(objects: Vec<SynthObject>) -> Vec<u8> {
    SynthWorld {
        extent_min: [-200.0; 3],
        extent_max: [200.0; 3],
        tree_shape: vec![false],
        grid_cell: [400.0; 3],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![90, 100, 110],
        objects,
        ..SynthWorld::default()
    }
    .build()
}

fn two_sided_of(objects: Vec<SynthObject>) -> bool {
    let data = world_bytes(objects);
    let world = World::parse(&data).expect("synthetic world parses");
    let read = world_two_sided(&world);
    let mesh = SceneMesh::build(&world, SceneOptions::default(), &mut |_| {
        TextureTraits::default()
    });
    assert_eq!(mesh.two_sided, read, "the scene build carries the setting");
    read
}

#[test]
fn the_setting_comes_from_the_first_world_settings_object() {
    let far = ("far_z", SynthValue::Real(9000.0));
    // absent: off (every ROM world and every map that does not ask)
    assert!(!two_sided_of(vec![]));
    assert!(!two_sided_of(vec![settings(std::slice::from_ref(&far))]));
    // on and off, and the number forms the other world-settings switches accept
    assert!(two_sided_of(vec![settings(&[
        far.clone(),
        ("two_sided", SynthValue::Bool(true))
    ])]));
    assert!(!two_sided_of(vec![settings(&[(
        "two_sided",
        SynthValue::Bool(false)
    )])]));
    assert!(two_sided_of(vec![settings(&[(
        "TWO_SIDED",
        SynthValue::Int(1.0)
    )])]));
    assert!(!two_sided_of(vec![settings(&[(
        "two_sided",
        SynthValue::Real(0.0)
    )])]));
    assert!(!two_sided_of(vec![settings(&[(
        "two_sided",
        SynthValue::Text(b"true".to_vec())
    )])]));
    // the first world-settings object decides
    assert!(!two_sided_of(vec![
        settings(std::slice::from_ref(&far)),
        settings(&[("two_sided", SynthValue::Bool(true))]),
    ]));
    assert!(two_sided_of(vec![
        settings(&[("two_sided", SynthValue::Bool(true))]),
        settings(&[far]),
    ]));
    // on another object it is nothing
    assert!(!two_sided_of(vec![SynthObject::dc(
        "dc.global_light",
        "sun",
        [0.0; 3],
        [0.0; 3],
        &[("two_sided", SynthValue::Bool(true))],
    )]));
}

/// A map built from ROM pieces is composed at load time (MAP-116): the pieces' own world settings stay out, so the
/// map's object is the first of its role and its `two_sided` reaches the renderer. On the composer's made-up fixture.
#[test]
fn a_composed_map_keeps_its_own_setting() {
    use dc_world::compose::{self, clip::Aabb, fixture};
    use dc_world::synth::SynthRenderWorld;
    use dc_world::ObjectRole;
    let ground = Aabb::new([12_000.0, -10.0, -1_000.0], [14_000.0, 0.0, 1_000.0]);
    let faces = fixture::box_faces(&ground, 1);
    let map = |two_sided: bool| {
        let mut props = vec![("far_z", SynthValue::Real(20_000.0))];
        if two_sided {
            props.push(("two_sided", SynthValue::Bool(true)));
        }
        SynthWorld {
            info: b"two-sided test".to_vec(),
            extent_min: [11_872.0, -128.0, -1_128.0],
            extent_max: [14_128.0, 228.0, 1_128.0],
            tree_shape: vec![false],
            models: vec![fixture::model("level", 0, [0.0; 3], &faces)],
            objects: vec![
                SynthObject::dc(
                    "dc.world_settings",
                    "world_settings",
                    [13_000.0, 0.0, 0.0],
                    [0.0; 3],
                    &props,
                ),
                fixture::piece(Vec::new()).to_object(),
            ],
            grid_origin: [11_872.0, -128.0, -1_128.0],
            grid_cell: [2_256.0, 356.0, 2_256.0],
            grid_dims: [1, 1, 1],
            grid_rgb: vec![40, 40, 40],
            render: SynthRenderWorld {
                blocks: vec![fixture::block(&faces, b"test/ground")],
                models: Vec::new(),
            },
            ..SynthWorld::default()
        }
        .build()
    };
    for on in [false, true] {
        let mut read = fixture::reader();
        let bytes = compose::compose(&map(on), &mut read)
            .expect("compose")
            .expect("the map has a piece")
            .bytes;
        let world = World::parse(&bytes).expect("the composed world parses");
        let ws: Vec<_> = world.objects_with_role(ObjectRole::WorldSettings).collect();
        assert_eq!(ws.len(), 1, "the piece's world settings stay out");
        assert_eq!(ws[0].name(), Some(&b"world_settings"[..]));
        assert_eq!(world_two_sided(&world), on);
        let mesh = SceneMesh::build(&world, SceneOptions::default(), &mut |_| {
            TextureTraits::default()
        });
        assert_eq!(mesh.two_sided, on);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// synthetic frames

/// One flat colour: filler.
const WHITE: u32 = 0;
/// Two colours: a textured surface.
const CHECKER: u32 = 1;
/// Two colours with an alpha-test reference (every texel passes it): a cut-out card.
const FENCE: u32 = 2;
/// The multiply texture of the lightmapped quad.
const SHADE: u32 = 3;

/// The four corners of a square of half size `s` centred at `c`, turned by `turn` about its centre; seen from the
/// eye at the origin looking along +Z, corner order 0-1-2-3 winds clockwise on screen (a front face) when `turn` keeps
/// the square facing the eye.
fn corners(c: Vec3, s: f32, turn: Quat) -> [Vec3; 4] {
    [(-s, -s), (-s, s), (s, s), (s, -s)].map(|(x, y)| c + turn * Vec3::new(x, y, 0.0))
}

#[derive(Clone, Copy)]
enum Faces {
    /// Triangles 0-1-2, 0-2-3: facing the eye.
    Front,
    /// The same triangles wound the other way: facing away.
    Back,
    /// Facing away, split along the other diagonal (1-3).
    BackOtherDiagonal,
}

/// Appends a quad and its batch: `pass` with texture `tex` (bucket 1 vertex-lit, 14 lightmap or 15 multiply).
fn quad(
    mesh: &mut SceneMesh,
    at: [Vec3; 4],
    faces: Faces,
    colour: [u8; 4],
    pass: PassKind,
    tex: u32,
) {
    let base = mesh.vertices.len() as u32;
    for (k, p) in at.iter().enumerate() {
        mesh.vertices.push(GpuVertex {
            pos: p.to_array(),
            colour,
            uv0: [(k / 2) as f32, (k % 2) as f32],
            uv1: [0.0, 0.0],
        });
    }
    let tri: [u32; 6] = match faces {
        Faces::Front => [0, 1, 2, 0, 2, 3],
        Faces::Back => [0, 2, 1, 0, 3, 2],
        Faces::BackOtherDiagonal => [0, 3, 1, 1, 3, 2],
    };
    let first = mesh.indices.len() as u32;
    mesh.indices.extend(tri.iter().map(|i| base + i));
    let (bucket, tex0, lightmap) = match pass {
        PassKind::Lightmap => (LIGHTMAP_BUCKET, None, Some(tex)),
        PassKind::LightmappedTexture => (15, Some(tex), None),
        _ => (1, Some(tex), None),
    };
    mesh.batches.push(Batch {
        bucket,
        pass,
        tex0,
        tex1: None,
        lightmap,
        first_index: first,
        index_count: 6,
    });
}

fn empty_mesh() -> SceneMesh {
    SceneMesh {
        textures: vec![
            b"white".to_vec(),
            b"checker".to_vec(),
            b"fence".to_vec(),
            b"shade".to_vec(),
        ],
        lightmaps: vec![LightmapImage {
            width: 1,
            height: 1,
            rgba: vec![200, 150, 100, 255],
        }],
        ..SceneMesh::default()
    }
}

fn images(name: &[u8]) -> Option<TextureImage> {
    let two = |a: [u8; 4], b: [u8; 4]| TextureImage::rgba(2, 2, [a, b, b, a].concat());
    Some(match name {
        b"white" => TextureImage::white(),
        b"checker" => two([250, 250, 250, 255], [170, 190, 210, 255]),
        b"fence" => TextureImage {
            alpha_cutoff: Some(128),
            ..two([250, 250, 250, 255], [90, 90, 90, 255])
        },
        _ => two([128, 200, 255, 255], [160, 120, 90, 255]),
    })
}

fn camera() -> Camera {
    let fx = 90f32.to_radians();
    Camera::looking(Vec3::ZERO, 0.0, 0.0, fx, fov_y_for(fx, W, H))
}

/// Uploads `mesh` (its `two_sided` as given) and renders the opaque frame.
fn frame(ctx: &GpuContext, r: &WorldRenderer, mesh: &SceneMesh) -> (FrameImage, FrameReport) {
    let scene = r.upload(ctx, mesh, &mut images);
    assert_eq!(scene.two_sided, mesh.two_sided);
    r.render_frame_offscreen(
        ctx,
        &scene,
        &camera(),
        &FrameOptions::opaque_only(),
        &[],
        W,
        H,
    )
    .expect("frame")
}

fn clear_px() -> [u8; 3] {
    let c = CLEAR_COLOUR.map(|c| (c * 255.0).round() as u8);
    [c[0], c[1], c[2]]
}

fn px(img: &FrameImage, x: u32, y: u32) -> [u8; 3] {
    let o = ((y * img.width + x) * 4) as usize;
    [img.rgba[o], img.rgba[o + 1], img.rgba[o + 2]]
}

fn differing(a: &FrameImage, b: &FrameImage) -> usize {
    a.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
        .filter(|(p, q)| p != q)
        .count()
}

const RED: [u8; 4] = [120, 20, 20, 255];
const GREEN: [u8; 4] = [20, 120, 20, 255];

#[test]
fn a_one_sided_scene_culls_back_faces_and_a_two_sided_one_draws_them() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let square = corners(Vec3::new(0.0, 0.0, 100.0), 30.0, Quat::IDENTITY);
    // the default path: the back face is culled, nothing else is recorded
    let mut away = empty_mesh();
    quad(
        &mut away,
        square,
        Faces::Back,
        GREEN,
        PassKind::VertexLit,
        CHECKER,
    );
    assert!(
        !away.two_sided,
        "a scene is one-sided unless its world asks"
    );
    let (img, rep) = frame(&ctx, &r, &away);
    assert_eq!(
        px(&img, W / 2, H / 2),
        clear_px(),
        "the back face is culled"
    );
    assert_eq!(rep.world_back_draws, 0);
    assert_eq!(rep.world_draws, 1);
    let (blank, _) = frame(&ctx, &r, &empty_mesh());
    assert!(img.rgba == blank.rgba, "the culled face changes no pixel");
    // two-sided: the same face is drawn, exactly as the quad facing the eye
    away.two_sided = true;
    let (img, rep) = frame(&ctx, &r, &away);
    assert_eq!(rep.world_back_draws, 1);
    assert_eq!(rep.world_draws, 1);
    assert_ne!(px(&img, W / 2, H / 2), clear_px(), "the back face is drawn");
    let mut toward = empty_mesh();
    quad(
        &mut toward,
        square,
        Faces::Front,
        GREEN,
        PassKind::VertexLit,
        CHECKER,
    );
    let (front, _) = frame(&ctx, &r, &toward);
    assert!(
        img.rgba == front.rgba,
        "a back face draws as its front would"
    );
}

#[test]
fn a_front_face_on_the_same_plane_always_wins() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    // (place, half size, turn): straight ahead near and far, and at grazing angles about both axes
    let cases = [
        (Vec3::new(0.0, 0.0, 12.0), 4.0, Quat::IDENTITY),
        (Vec3::new(0.0, 0.0, 100.0), 30.0, Quat::IDENTITY),
        (Vec3::new(0.0, 0.0, 3000.0), 900.0, Quat::IDENTITY),
        (Vec3::new(0.0, 0.0, 10000.0), 3000.0, Quat::IDENTITY),
        (
            Vec3::new(10.0, -5.0, 200.0),
            150.0,
            Quat::from_rotation_y(80f32.to_radians()),
        ),
        (
            Vec3::new(0.0, -40.0, 300.0),
            400.0,
            Quat::from_rotation_x(84f32.to_radians()),
        ),
        (
            Vec3::new(-20.0, 10.0, 2500.0),
            2000.0,
            Quat::from_rotation_y(-86f32.to_radians()) * Quat::from_rotation_x(20f32.to_radians()),
        ),
    ];
    let mut checked = 0;
    for (k, (c, s, turn)) in cases.into_iter().enumerate() {
        let at = corners(c, s, turn);
        let mut alone = empty_mesh();
        alone.two_sided = true;
        quad(
            &mut alone,
            at,
            Faces::Front,
            RED,
            PassKind::VertexLit,
            CHECKER,
        );
        let (want, _) = frame(&ctx, &r, &alone);
        let covered = want
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[..3] != clear_px())
            .count();
        assert!(
            covered > 50,
            "case {k}: the quad is on screen ({covered} px)"
        );
        for (twin, back_first) in [
            (Faces::Back, false),
            (Faces::Back, true),
            (Faces::BackOtherDiagonal, false),
            (Faces::BackOtherDiagonal, true),
        ] {
            let mut pair = empty_mesh();
            pair.two_sided = true;
            if back_first {
                quad(&mut pair, at, twin, GREEN, PassKind::VertexLit, CHECKER);
            }
            quad(
                &mut pair,
                at,
                Faces::Front,
                RED,
                PassKind::VertexLit,
                CHECKER,
            );
            if !back_first {
                quad(&mut pair, at, twin, GREEN, PassKind::VertexLit, CHECKER);
            }
            let (got, rep) = frame(&ctx, &r, &pair);
            assert!(rep.world_back_draws >= 1);
            let wrong = differing(&got, &want);
            assert_eq!(
                wrong, 0,
                "case {k} (twin first: {back_first}): {wrong} of {covered} px show the back face"
            );
            checked += 1;
        }
    }
    println!("{checked} coplanar pairs: the front face in every pixel");
}

#[test]
fn filler_and_cut_out_cards_keep_their_back_faces_culled() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let (blank, _) = frame(&ctx, &r, &empty_mesh());
    for tex in [WHITE, FENCE] {
        let mut away = empty_mesh();
        away.two_sided = true;
        quad(
            &mut away,
            corners(Vec3::new(0.0, 0.0, 100.0), 30.0, Quat::IDENTITY),
            Faces::Back,
            GREEN,
            PassKind::VertexLit,
            tex,
        );
        let (img, rep) = frame(&ctx, &r, &away);
        assert_eq!(rep.world_back_draws, 0, "texture {tex}");
        assert!(img.rgba == blank.rgba, "texture {tex}: nothing drawn");
    }
}

#[test]
fn a_textured_back_face_wins_over_filler_on_its_plane() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    // an outer wall: textured towards the play area (away from this eye), a flat filler twin facing out
    for (c, s, turn) in [
        (Vec3::new(0.0, 0.0, 150.0), 50.0, Quat::IDENTITY),
        (
            Vec3::new(0.0, 0.0, 900.0),
            700.0,
            Quat::from_rotation_y(75f32.to_radians()),
        ),
    ] {
        let at = corners(c, s, turn);
        let wall = |two_sided: bool| {
            let mut m = empty_mesh();
            m.two_sided = two_sided;
            quad(&mut m, at, Faces::Front, GREEN, PassKind::VertexLit, WHITE);
            quad(&mut m, at, Faces::Back, RED, PassKind::VertexLit, CHECKER);
            m
        };
        let alone = |faces: Faces, colour: [u8; 4], tex: u32| {
            let mut m = empty_mesh();
            m.two_sided = true;
            quad(&mut m, at, faces, colour, PassKind::VertexLit, tex);
            frame(&ctx, &r, &m).0
        };
        // one-sided: the filler, as the 2009 renderer draws it
        let (one, _) = frame(&ctx, &r, &wall(false));
        assert_eq!(differing(&one, &alone(Faces::Front, GREEN, WHITE)), 0);
        // two-sided: the textured face's back in every pixel
        let (two, rep) = frame(&ctx, &r, &wall(true));
        assert_eq!(rep.world_back_draws, 1, "the filler has no back-face draw");
        assert_eq!(differing(&two, &alone(Faces::Front, RED, CHECKER)), 0);
    }
}

#[test]
fn a_back_face_keeps_its_lightmap_and_multiply_pair() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let at = corners(
        Vec3::new(0.0, 0.0, 400.0),
        150.0,
        Quat::from_rotation_y(50f32.to_radians()),
    );
    let lit = |faces: Faces, two_sided: bool| {
        let mut m = empty_mesh();
        m.two_sided = two_sided;
        quad(&mut m, at, faces, [255; 4], PassKind::Lightmap, 0);
        quad(
            &mut m,
            at,
            faces,
            [255; 4],
            PassKind::LightmappedTexture,
            SHADE,
        );
        m.batches.sort_by_key(|b| b.bucket);
        m
    };
    let (front, _) = frame(&ctx, &r, &lit(Faces::Front, false));
    let (back, rep) = frame(&ctx, &r, &lit(Faces::Back, true));
    assert_eq!(
        rep.world_back_draws, 2,
        "the lightmap and the multiply draw"
    );
    // the lightmap alone, to see that the multiply is in the picture
    let mut only = empty_mesh();
    quad(&mut only, at, Faces::Front, [255; 4], PassKind::Lightmap, 0);
    let (lightmap, _) = frame(&ctx, &r, &only);
    let centre = px(&front, W / 2, H / 2);
    assert_ne!(centre, clear_px());
    assert_ne!(
        centre,
        px(&lightmap, W / 2, H / 2),
        "the multiply changed the lightmap"
    );
    assert!(
        back.rgba == front.rgba,
        "the back face draws both passes like the front"
    );
}

#[test]
fn one_sided_frames_do_not_change_once_a_two_sided_scene_was_drawn() {
    let Some(ctx) = context() else {
        return;
    };
    let mixed = |two_sided: bool| {
        let mut m = empty_mesh();
        m.two_sided = two_sided;
        quad(
            &mut m,
            corners(Vec3::new(-30.0, 0.0, 120.0), 25.0, Quat::IDENTITY),
            Faces::Front,
            RED,
            PassKind::VertexLit,
            CHECKER,
        );
        quad(
            &mut m,
            corners(Vec3::new(0.0, 30.0, 130.0), 20.0, Quat::IDENTITY),
            Faces::Front,
            RED,
            PassKind::VertexLit,
            WHITE,
        );
        quad(
            &mut m,
            corners(
                Vec3::new(30.0, 0.0, 140.0),
                25.0,
                Quat::from_rotation_y(0.4),
            ),
            Faces::Back,
            GREEN,
            PassKind::VertexLit,
            SHADE,
        );
        m
    };
    let fresh = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let (before, rb) = frame(&ctx, &fresh, &mixed(false));
    let used = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let (two, rt) = frame(&ctx, &used, &mixed(true));
    let (after, ra) = frame(&ctx, &used, &mixed(false));
    assert!(
        before.rgba == after.rgba,
        "the one-sided frame is unchanged"
    );
    assert_eq!(
        (
            rb.draw_calls,
            rb.pipeline_sets,
            rb.bind_sets,
            rb.world_back_draws
        ),
        (ra.draw_calls, ra.pipeline_sets, ra.bind_sets, 0),
        "and records the same commands"
    );
    assert!(
        two.rgba != before.rgba,
        "the two-sided frame shows the back face"
    );
    assert_eq!(rt.world_back_draws, 2, "the checker and the shade quad");
}

// ---------------------------------------------------------------------------------------------------------------
// ROM-gated

/// Triangles of the scene's opaque batches that have an exact opposite twin: the same three positions wound the
/// other way.
fn opposite_twins(mesh: &SceneMesh) -> (usize, usize) {
    use std::collections::HashMap;
    let key = |p: [f32; 3]| p.map(f32::to_bits);
    // (sorted positions) -> (count wound one way, count the other)
    let mut seen: HashMap<[[u32; 3]; 3], [usize; 2]> = HashMap::new();
    let mut tris = Vec::new();
    for b in &mesh.batches {
        let r = b.first_index as usize..(b.first_index + b.index_count) as usize;
        for t in mesh.indices[r].as_chunks::<3>().0 {
            let [a, b, c] = t.map(|i| key(mesh.vertices[i as usize].pos));
            if a == b || b == c || a == c {
                continue;
            }
            // the rotation that puts the smallest corner first keeps the winding
            let rot = if a <= b && a <= c {
                [a, b, c]
            } else if b <= a && b <= c {
                [b, c, a]
            } else {
                [c, a, b]
            };
            let wound = usize::from(rot[1] > rot[2]);
            let mut sorted = rot;
            sorted.sort();
            seen.entry(sorted).or_default()[wound] += 1;
            tris.push((sorted, wound));
        }
    }
    let twins = tris.iter().filter(|(s, w)| seen[s][1 - w] > 0).count();
    (twins, tris.len())
}

#[test]
fn rom_worlds_stay_one_sided_and_the_back_faces_cost_is_printed() {
    let rom = dc_rom::rom_or_skip!();
    let viewer = match RomViewer::open(&rom, AdapterChoice::Auto, RenderSettings::default()) {
        Ok(v) => v,
        Err(ViewError::Gpu(e)) => {
            println!("skipped: {e}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    // the coplanar pairs of the render data
    let (mut twins, mut tris) = (0usize, 0usize);
    for (name, _, _) in viewer.vfs().list_entries("WORLDS.rez").expect("list") {
        if !name.to_ascii_uppercase().ends_with(".DAT") {
            continue;
        }
        let data = viewer
            .vfs()
            .entry("WORLDS.rez", &name)
            .expect("entry")
            .expect("present");
        let world = World::parse(&data).expect("parse");
        assert!(!world_two_sided(&world), "{name}: a ROM world is one-sided");
        let mesh = SceneMesh::build(&world, SceneOptions::default(), &mut |_| TextureTraits {
            present: true,
            mode: 0,
            hi: false,
        });
        assert!(!mesh.two_sided, "{name}");
        let (t, n) = opposite_twins(&mesh);
        println!("{name}: {t} of {n} opaque triangles have an exact opposite twin");
        twins += t;
        tris += n;
    }
    println!("all worlds: {twins} of {tris} opaque triangles have an exact opposite twin");
    // the cost of the back faces at each map's first spawn: one-sided and two-sided frames alternate (the same GPU
    // clock for both), two warm-up pairs, then the median of the submit-to-completion times
    let (w, h) = (1280, 720);
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let (mut sum_off, mut sum_on) = (0f64, 0f64);
    for map in viewer.map_names() {
        let mut req = ViewRequest::new(&map);
        req.width = w;
        req.height = h;
        let mut p = viewer
            .prepare(&req)
            .unwrap_or_else(|e| panic!("{map}: {e}"));
        assert!(!p.scene.two_sided, "{map}: a ROM world is one-sided");
        let (mut t_off, mut t_on) = (Vec::new(), Vec::new());
        let (mut off, mut on) = (FrameReport::default(), FrameReport::default());
        for k in 0..11 {
            p.scene.two_sided = false;
            off = viewer.render_prepared(&p, w, h).expect("frame").1;
            p.scene.two_sided = true;
            on = viewer.render_prepared(&p, w, h).expect("frame").1;
            if k >= 2 {
                t_off.push(off.gpu_ms);
                t_on.push(on.gpu_ms);
            }
        }
        assert_eq!(off.world_back_draws, 0);
        assert!(on.world_back_draws <= on.world_draws, "{map}");
        let (a, b) = (median(t_off), median(t_on));
        sum_off += a;
        sum_on += b;
        println!(
            "{map}: one-sided {a:.2} ms ({} draws), two-sided {b:.2} ms (+{} back-face draws)",
            off.draw_calls, on.world_back_draws
        );
    }
    println!("sum of the medians: one-sided {sum_off:.1} ms, two-sided {sum_on:.1} ms");
}

/// Inside the original play areas a ROM map must keep its 2009 look with the back faces on: from every map's first
/// spawn at eight directions the two-sided frame may differ from the one-sided one in a few pixels only (what is left:
/// thin opaque props such as rails now seen from behind). Before filler and cut-out cards stayed one-sided, 1.6 % of
/// these pixels changed, up to 6 % on some maps (grey panels over windows, insets, craters and facades).
#[test]
fn rom_maps_keep_their_look_from_the_play_area() {
    let rom = dc_rom::rom_or_skip!();
    let viewer = match RomViewer::open(&rom, AdapterChoice::Auto, RenderSettings::default()) {
        Ok(v) => v,
        Err(ViewError::Gpu(e)) => {
            println!("skipped: {e}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let (w, h) = (512u32, 384u32);
    let (mut changed, mut total) = (0usize, 0usize);
    for map in viewer.map_names() {
        let mut req = ViewRequest::new(&map);
        req.width = w;
        req.height = h;
        let mut p = viewer
            .prepare(&req)
            .unwrap_or_else(|e| panic!("{map}: {e}"));
        let base = p.camera;
        let mut here = 0;
        for k in 0..8 {
            p.camera = Camera {
                rot: Quat::from_rotation_y(k as f32 * 45f32.to_radians()) * base.rot,
                ..base
            };
            p.scene.two_sided = false;
            let (off, _) = viewer.render_prepared(&p, w, h).expect("frame");
            p.scene.two_sided = true;
            let (on, _) = viewer.render_prepared(&p, w, h).expect("frame");
            here += differing(&off, &on);
        }
        println!(
            "{map}: {:.3} % of the pixels change",
            100.0 * here as f64 / (8 * w * h) as f64
        );
        changed += here;
        total += (8 * w * h) as usize;
    }
    let share = changed as f64 / total as f64;
    println!(
        "all maps: {changed} of {total} px change ({:.4} %)",
        100.0 * share
    );
    assert!(
        share < 0.002,
        "the back faces changed {:.3} % of the play-area views",
        100.0 * share
    );
}
