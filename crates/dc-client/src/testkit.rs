//! Made-up fixtures shared by the unit tests, the integration tests and the examples: a two-glyph TrueType
//! font and a small walkable world with render geometry. Nothing here comes from the ROM.

use crate::world::{SpawnPoint, SynthSource};
use dc_render::RenderVertex;
use dc_sim::from_world::FILE_FIRST_CHILD_IS_NEGATIVE;
use dc_sim::{build_bsp, ConvexPoly, SURF_SOLID, V3};
use dc_world::synth::{SynthBlock, SynthModel, SynthRenderWorld, SynthSection, SynthWorld};

/// Made-up standing half extents for the synthetic world.
pub const SYNTH_DIMS: V3 = V3::new(16.0, 40.0, 16.0);

/// Height of the made-up step and where it starts.
pub const STEP_HEIGHT: f32 = 20.0;
pub const STEP_Z: f32 = 400.0;

fn quad(c: [V3; 4]) -> ConvexPoly {
    ConvexPoly::from_corners(c.to_vec(), SURF_SOLID)
}

/// Horizontal rectangle at height `y` facing up (corners bottom-left, top-left, top-right, bottom-right as
/// seen from above with +Z up the screen).
fn top(x0: f32, x1: f32, z0: f32, z1: f32, y: f32) -> ConvexPoly {
    quad([
        V3::new(x0, y, z0),
        V3::new(x0, y, z1),
        V3::new(x1, y, z1),
        V3::new(x1, y, z0),
    ])
}

/// Vertical rectangle in the plane `z = z0` facing -Z.
fn wall_neg_z(x0: f32, x1: f32, y0: f32, y1: f32, z0: f32) -> ConvexPoly {
    quad([
        V3::new(x0, y0, z0),
        V3::new(x0, y1, z0),
        V3::new(x1, y1, z0),
        V3::new(x1, y0, z0),
    ])
}

/// The course: a floor at y = 0, a 20-unit step at z = 400 whose top runs on, a wall across at z = 1500 and a
/// wall at x = 1200 facing -X. Each polygon with the name of its (made-up) texture.
pub fn synth_course() -> Vec<(ConvexPoly, &'static [u8])> {
    let side = quad([
        V3::new(1200.0, 0.0, 2000.0),
        V3::new(1200.0, 400.0, 2000.0),
        V3::new(1200.0, 400.0, -2000.0),
        V3::new(1200.0, 0.0, -2000.0),
    ]);
    vec![
        (top(-2000.0, 2000.0, -2000.0, STEP_Z, 0.0), b"syn/floor"),
        (
            wall_neg_z(-2000.0, 2000.0, 0.0, STEP_HEIGHT, STEP_Z),
            b"syn/step",
        ),
        (
            top(-2000.0, 2000.0, STEP_Z, 2000.0, STEP_HEIGHT),
            b"syn/step",
        ),
        (wall_neg_z(-2000.0, 2000.0, 0.0, 400.0, 1500.0), b"syn/wall"),
        (side, b"syn/wall"),
    ]
}

fn collision_model(polys: &[ConvexPoly]) -> SynthModel {
    let (nodes, root) = build_bsp(polys);
    let mut m = SynthModel {
        flags: 0,
        name: b"synthetic level".to_vec(),
        textures: vec![b"none".to_vec()],
        surfaces: vec![(SURF_SOLID, 0, 0)],
        box_min: [-2000.0, -10.0, -2000.0],
        box_max: [2000.0, 400.0, 2000.0],
        ..SynthModel::default()
    };
    for (i, poly) in polys.iter().enumerate() {
        m.planes.push((poly.normal.to_array(), poly.dist));
        let first = m.points.len() as u32;
        m.points.extend(poly.verts.iter().map(|v| v.to_array()));
        m.polys.push((
            0,
            i as u32,
            (first..first + poly.verts.len() as u32).collect(),
        ));
    }
    let enc = |c: Option<u32>| c.map_or(-1, |i| i as i32);
    for (face, neg, pos) in nodes {
        let (first, second) = if FILE_FIRST_CHILD_IS_NEGATIVE {
            (neg, pos)
        } else {
            (pos, neg)
        };
        m.nodes.push((face, 0, enc(first), enc(second)));
    }
    m.root = enc(root);
    m
}

/// One render block: one vertex-lit section (kind 1) per course polygon, two triangles each, wound clockwise
/// on screen as seen from the walkable side.
fn render_block(course: &[(ConvexPoly, Vec<u8>)]) -> SynthBlock {
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    let mut sections = Vec::new();
    for (poly, tex) in course {
        let b = vertices.len() as u32;
        let uv = [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        for (k, v) in poly.verts.iter().enumerate() {
            vertices.push(
                RenderVertex {
                    pos: v.to_array(),
                    uv0: uv[k % 4],
                    uv1: uv[k % 4],
                    colour: 0xff80_8080,
                    normal: poly.normal.to_array(),
                    tangent: [1.0, 0.0, 0.0],
                    binormal: [0.0, 1.0, 0.0],
                }
                .to_bytes(),
            );
        }
        triangles.push([b, b + 1, b + 2, 0]);
        triangles.push([b, b + 2, b + 3, 0]);
        sections.push(SynthSection {
            textures: [tex.to_vec(), Vec::new()],
            shader_kind: 1,
            triangles: 2,
            ..SynthSection::default()
        });
    }
    SynthBlock {
        center: [0.0, 200.0, 0.0],
        half: [2000.0, 200.0, 2000.0],
        sections,
        vertices,
        triangles,
        ..SynthBlock::default()
    }
}

/// The version-85 bytes of the synthetic world (collision BSP and render block).
pub fn synth_world_bytes() -> Vec<u8> {
    synth_world_bytes_with(&|k| format!("syn/{k}").into_bytes(), Vec::new())
}

/// [`synth_world_bytes`] with the course's three texture names (`floor`, `step`, `wall`) given by `tex_name` and
/// `objects` in the object list: the community-map tests write `@mod:<id>/assets/...` names and `dc.*` objects
/// (docs/MAPKIT_VOCAB.md) into it.
pub fn synth_world_bytes_with(
    tex_name: &dyn Fn(&str) -> Vec<u8>,
    objects: Vec<dc_world::synth::SynthObject>,
) -> Vec<u8> {
    let course = synth_course();
    let polys: Vec<ConvexPoly> = course.iter().map(|(p, _)| p.clone()).collect();
    let named: Vec<(ConvexPoly, Vec<u8>)> = course
        .iter()
        .map(|(p, n)| {
            let key = std::str::from_utf8(n).unwrap_or("floor");
            let key = key.strip_prefix("syn/").unwrap_or(key);
            (p.clone(), tex_name(key))
        })
        .collect();
    SynthWorld {
        extent_min: [-2000.0, -100.0, -2000.0],
        extent_max: [2000.0, 400.0, 2000.0],
        models: vec![collision_model(&polys)],
        objects,
        tree_shape: vec![false],
        grid_cell: [64.0; 3],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![0, 0, 0],
        render: SynthRenderWorld {
            blocks: vec![render_block(&named)],
            models: vec![],
        },
        ..SynthWorld::default()
    }
    .build()
}

/// The synthetic world as a world source: one spawn at the origin facing +Z, 60 units up.
pub fn synth_source() -> SynthSource {
    SynthSource {
        name: "SYNTHETIC".into(),
        bytes: synth_world_bytes(),
        spawns: vec![SpawnPoint {
            pos: V3::new(0.0, 60.0, 0.0),
            yaw: 0.0,
        }],
        stand_dims: SYNTH_DIMS,
        textures: vec![
            (b"syn/floor".to_vec(), [150, 150, 150, 255]),
            (b"syn/step".to_vec(), [180, 120, 60, 255]),
            (b"syn/wall".to_vec(), [70, 90, 200, 255]),
        ],
    }
}

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}

/// A made-up two-glyph font: glyph 0 = missing (empty), glyph 1 = a 600-unit square for 'A' with an inner
/// counter-clockwise hole and a quadratic notch, glyph 2 = a composite of glyph 1 scaled by 0.5 and moved
/// for 'B'. Units per em 1000, ascender 800, descender -200.
pub fn synth_font() -> Vec<u8> {
    // glyph 1: outer square (clockwise in y-up = TrueType outer direction), inner square reversed
    let mut g1 = Vec::new();
    be16(&mut g1, 2); // contours
    for v in [100i16, 0, 700, 600] {
        be16(&mut g1, v as u16);
    }
    be16(&mut g1, 4); // end of contour 0 (5 points: one off-curve)
    be16(&mut g1, 8); // end of contour 1
    be16(&mut g1, 0); // no instructions
                      // points: outer (100,0) (100,600) (400,700 off) (700,600) (700,0); inner (300,200) (500,200) (500,400) (300,400)
    let pts: [(i16, i16, bool); 9] = [
        (100, 0, true),
        (100, 600, true),
        (400, 700, false),
        (700, 600, true),
        (700, 0, true),
        (300, 200, true),
        (500, 200, true),
        (500, 400, true),
        (300, 400, true),
    ];
    for p in &pts {
        g1.push(if p.2 { 0x01 } else { 0x00 });
    }
    let mut last = 0i16;
    for p in &pts {
        be16(&mut g1, (p.0 - last) as u16);
        last = p.0;
    }
    let mut last = 0i16;
    for p in &pts {
        be16(&mut g1, (p.1 - last) as u16);
        last = p.1;
    }
    if g1.len() % 2 == 1 {
        g1.push(0);
    }
    // glyph 2: composite of glyph 1, word args, xy values, scale 0.5
    let mut g2 = Vec::new();
    be16(&mut g2, 0xffff); // -1 contours
    for v in [0i16, 0, 400, 400] {
        be16(&mut g2, v as u16);
    }
    be16(&mut g2, 0x0001 | 0x0002 | 0x0008);
    be16(&mut g2, 1);
    be16(&mut g2, 50);
    be16(&mut g2, 10);
    be16(&mut g2, 8192); // 0.5 in F2Dot14
    let glyf: Vec<u8> = [g1.clone(), g2.clone()].concat();
    let loca: Vec<u8> = [0u16, 0, (g1.len() / 2) as u16, (glyf.len() / 2) as u16]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    let mut head = vec![0u8; 54];
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    let mut maxp = vec![0u8; 6];
    maxp[4..6].copy_from_slice(&3u16.to_be_bytes());
    let mut hhea = vec![0u8; 36];
    hhea[4..6].copy_from_slice(&800i16.to_be_bytes());
    hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes());
    hhea[34..36].copy_from_slice(&3u16.to_be_bytes());
    let mut hmtx = Vec::new();
    for adv in [500u16, 800, 450] {
        be16(&mut hmtx, adv);
        be16(&mut hmtx, 0);
    }
    // cmap: one (3,1) format 4 subtable: 'A'..'B' -> 1..2 via delta, plus the 0xffff end segment
    let mut sub = Vec::new();
    let segs = 2u16;
    for v in [4u16, 0, 0, segs * 2, 0, 0, 0] {
        be16(&mut sub, v);
    }
    be16(&mut sub, b'B' as u16);
    be16(&mut sub, 0xffff);
    be16(&mut sub, 0);
    be16(&mut sub, b'A' as u16);
    be16(&mut sub, 0xffff);
    be16(&mut sub, (1i32 - b'A' as i32) as u16);
    be16(&mut sub, 1);
    be16(&mut sub, 0);
    be16(&mut sub, 0);
    let len = sub.len() as u16;
    sub[2..4].copy_from_slice(&len.to_be_bytes());
    let mut cmap = Vec::new();
    for v in [0u16, 1, 3, 1] {
        be16(&mut cmap, v);
    }
    cmap.extend_from_slice(&12u32.to_be_bytes());
    cmap.extend_from_slice(&sub);
    let tables: [(&[u8; 4], Vec<u8>); 7] = [
        (b"cmap", cmap),
        (b"glyf", glyf),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"loca", loca),
        (b"maxp", maxp),
    ];
    let mut out = vec![0, 1, 0, 0];
    be16(&mut out, tables.len() as u16);
    out.extend_from_slice(&[0; 6]);
    let mut off = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, t) in &tables {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(t.len() as u32).to_be_bytes());
        body.extend_from_slice(t);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        off = 12 + 16 * tables.len() + body.len();
    }
    out.extend_from_slice(&body);
    out
}
