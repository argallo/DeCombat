//! A made-up "ROM" world and a map that places it as a piece, for the composer's tests here and in dc-sim and
//! dc-mapkit. Every name and number is our own; nothing comes from the ROM.

use super::clip::{self, Aabb, V};
use super::{Piece, Spawns};
use crate::synth::{
    pack_light_grid, SynthBlock, SynthModel, SynthObject, SynthRenderWorld, SynthSection,
    SynthValue, SynthWorld,
};
use crate::RENDER_VERTEX_BYTES;

/// The path the fixture's piece names.
pub const WORLD: &str = "WORLDS\\FIXTURE.DAT";
/// Where the fixture map puts the piece, and the region it keeps (composed coordinates).
pub const OFFSET: V = [10_000.0, 0.0, 0.0];
pub fn region() -> Aabb {
    Aabb::new([9_000.0, -600.0, -1_000.0], [10_600.0, 1_100.0, 1_000.0])
}

/// A polygon for the naive builder: corners counter-clockwise from the front, flags, texflags.
#[derive(Debug, Clone)]
pub struct Face {
    pub corners: Vec<V>,
    pub flags: u32,
    pub texflags: u16,
}

fn plane_of(c: &[V]) -> (V, f32) {
    let n = clip::cross(clip::sub(c[1], c[0]), clip::sub(c[2], c[1]));
    let n = clip::scale(n, 1.0 / clip::length(n));
    (n, clip::dot(n, c[0]))
}

/// A model over faces with a naive BSP: the first face splits, the rest go down the side(s) they lie on.
pub fn model(name: &str, flags: u32, origin: V, faces: &[Face]) -> SynthModel {
    let mut m = SynthModel {
        flags,
        name: name.as_bytes().to_vec(),
        textures: vec![b"fixture/stone".to_vec()],
        origin,
        ..SynthModel::default()
    };
    for f in faces {
        let (n, d) = plane_of(&f.corners);
        m.planes.push((n, d));
        m.surfaces.push((f.flags, 0, f.texflags));
        let first = m.points.len() as u32;
        m.points.extend(f.corners.iter().copied());
        let idx: Vec<u32> = (first..first + f.corners.len() as u32).collect();
        let i = m.polys.len() as u32;
        m.polys.push((i, i, idx));
    }
    fn rec(m: &mut SynthModel, list: &[usize]) -> i32 {
        let Some((&s, rest)) = list.split_first() else {
            return -2;
        };
        let (n, d) = m.planes[s];
        let (mut neg, mut pos) = (Vec::new(), Vec::new());
        for &o in rest {
            let pts: Vec<V> = m.polys[o].2.iter().map(|&i| m.points[i as usize]).collect();
            let (mut a, mut b) = (false, false);
            for p in pts {
                let sd = clip::dot(n, p) - d;
                if sd > 0.01 {
                    a = true;
                } else if sd < -0.01 {
                    b = true;
                }
            }
            if b {
                neg.push(o);
            }
            if a || !b {
                pos.push(o);
            }
        }
        let id = m.nodes.len();
        m.nodes.push((s as u32, 0, -2, -1));
        let nn = rec(m, &neg);
        let pp = rec(m, &pos);
        m.nodes[id].2 = nn;
        m.nodes[id].3 = if pp == -2 { -1 } else { pp };
        id as i32
    }
    let list: Vec<usize> = (0..faces.len()).collect();
    m.root = rec(&mut m, &list);
    let b = Aabb::of_points(m.points.iter()).unwrap_or(Aabb::new([0.0; 3], [0.0; 3]));
    m.box_min = b.min;
    m.box_max = b.max;
    m
}

/// An axis-aligned box as six outward faces.
pub fn box_faces(b: &Aabb, flags: u32) -> Vec<Face> {
    let (l, h) = (b.min, b.max);
    let q = |a: V, bb: V, c: V, d: V| Face {
        corners: vec![a, bb, c, d],
        flags,
        texflags: 3,
    };
    vec![
        q(
            [h[0], l[1], l[2]],
            [h[0], h[1], l[2]],
            [h[0], h[1], h[2]],
            [h[0], l[1], h[2]],
        ),
        q(
            [l[0], l[1], l[2]],
            [l[0], l[1], h[2]],
            [l[0], h[1], h[2]],
            [l[0], h[1], l[2]],
        ),
        q(
            [l[0], h[1], l[2]],
            [l[0], h[1], h[2]],
            [h[0], h[1], h[2]],
            [h[0], h[1], l[2]],
        ),
        q(
            [l[0], l[1], l[2]],
            [h[0], l[1], l[2]],
            [h[0], l[1], h[2]],
            [l[0], l[1], h[2]],
        ),
        q(
            [l[0], l[1], h[2]],
            [h[0], l[1], h[2]],
            [h[0], h[1], h[2]],
            [l[0], h[1], h[2]],
        ),
        q(
            [l[0], l[1], l[2]],
            [l[0], h[1], l[2]],
            [h[0], h[1], l[2]],
            [h[0], l[1], l[2]],
        ),
    ]
}

fn vertex(p: V, uv: [f32; 2], n: V) -> [u8; RENDER_VERTEX_BYTES] {
    let mut v = [0u8; RENDER_VERTEX_BYTES];
    let mut put = |o: usize, x: f32| v[o..o + 4].copy_from_slice(&x.to_le_bytes());
    for (k, &x) in p.iter().enumerate() {
        put(4 * k, x);
    }
    put(12, uv[0]);
    put(16, uv[1]);
    for (k, &x) in n.iter().enumerate() {
        put(32 + 4 * k, x);
    }
    v[28..32].copy_from_slice(&0xFF80_8080u32.to_le_bytes());
    v
}

/// A block drawing the faces (two triangles per quad), one kind-1 section.
pub fn block(faces: &[Face], texture: &[u8]) -> SynthBlock {
    let mut b = SynthBlock::default();
    for f in faces {
        let (n, _) = plane_of(&f.corners);
        let base = b.vertices.len() as u32;
        for (i, c) in f.corners.iter().enumerate() {
            b.vertices.push(vertex(*c, [i as f32, 0.0], n));
        }
        for k in 1..f.corners.len() as u32 - 1 {
            b.triangles.push([base, base + k, base + k + 1, 0]);
        }
    }
    b.sections.push(SynthSection {
        textures: [texture.to_vec(), Vec::new()],
        shader_kind: 1,
        triangles: b.triangles.len() as u32,
        ..SynthSection::default()
    });
    b
}

fn quad(a: V, b: V, c: V, d: V, flags: u32) -> Face {
    Face {
        corners: vec![a, b, c, d],
        flags,
        texflags: 7,
    }
}

/// The "ROM" world, in its own coordinates around the origin: a floor 2,000 wide at y 0, a wall across it at
/// x 500 (facing -x), a sky lid at y 1,000; a world model, a kill slab wider than any region, two player spawns
/// (one past the wall) plus a near duplicate, an item spawn, a world-settings object; one blocker on each side.
pub fn rom_world() -> Vec<u8> {
    let floor = quad(
        [-1_000.0, 0.0, -1_000.0],
        [-1_000.0, 0.0, 1_000.0],
        [1_000.0, 0.0, 1_000.0],
        [1_000.0, 0.0, -1_000.0],
        1,
    );
    let wall = quad(
        [500.0, 0.0, -1_000.0],
        [500.0, 0.0, 1_000.0],
        [500.0, 200.0, 1_000.0],
        [500.0, 200.0, -1_000.0],
        1,
    );
    let lid = quad(
        [-1_000.0, 1_000.0, -1_000.0],
        [1_000.0, 1_000.0, -1_000.0],
        [1_000.0, 1_000.0, 1_000.0],
        [-1_000.0, 1_000.0, 1_000.0],
        0x11,
    );
    let level = model("", 0, [0.0; 3], &[floor.clone(), wall.clone(), lid]);
    let wm_box = Aabb::new([80.0, 0.0, 80.0], [120.0, 40.0, 120.0]);
    let wm = model("crate", 2, wm_box.centre(), &box_faces(&wm_box, 1));
    let kill_box = Aabb::new([-3_000.0, -500.0, -3_000.0], [3_000.0, -400.0, 3_000.0]);
    let kill = model("pit", 2, kill_box.centre(), &box_faces(&kill_box, 0));
    let names: [&[u8]; 3] = [b"name", b"pos", b"rot"];
    let mut objects = vec![
        SynthObject::dc(
            "dc.player_spawn",
            "s1",
            [0.0, 50.0, 0.0],
            [0.0, 1.0, 0.0],
            &[("team", SynthValue::Text(b"1".to_vec()))],
        ),
        SynthObject::dc("dc.player_spawn", "s1b", [10.0, 50.0, 0.0], [0.0; 3], &[]),
        SynthObject::dc("dc.player_spawn", "s2", [700.0, 50.0, 0.0], [0.0; 3], &[]),
        SynthObject::dc("dc.item_spawn", "i1", [-200.0, 20.0, 0.0], [0.0; 3], &[]),
        SynthObject::dc(
            "dc.world_settings",
            "ws",
            [0.0; 3],
            [0.0; 3],
            &[("far_z", SynthValue::Real(9_000.0))],
        ),
        SynthObject::point(
            b"dc.world_model",
            names,
            b"crate",
            wm_box.centre(),
            [0.0; 3],
        ),
        SynthObject::point(
            b"dc.kill_volume",
            names,
            b"pit",
            kill_box.centre(),
            [0.0; 3],
        ),
    ];
    // a point light naming the crate, to see references follow a rename
    let mut light = SynthObject::dc(
        "dc.point_light",
        "lamp",
        [0.0, 300.0, 0.0],
        [0.0; 3],
        &[("radius", SynthValue::Real(200.0))],
    );
    light.props.push(crate::synth::SynthProp::dc(
        "target",
        SynthValue::Text(b"crate".to_vec()),
    ));
    objects.push(light);
    let mut grid = Vec::new();
    for i in 0..27u8 {
        grid.extend_from_slice(&[i * 9, 100, 200]);
    }
    let w = SynthWorld {
        info: b"fixture".to_vec(),
        extent_min: [-3_100.0, -600.0, -3_100.0],
        extent_max: [3_100.0, 1_100.0, 3_100.0],
        tree_shape: vec![false],
        models: vec![level, wm, kill],
        objects,
        grid_origin: [-1_000.0, 0.0, -1_000.0],
        grid_cell: [1_000.0, 500.0, 1_000.0],
        grid_dims: [3, 3, 3],
        grid_rgb: grid,
        blockers: vec![
            (
                [1.0, 0.0, 0.0],
                -900.0,
                vec![
                    [-900.0, 0.0, -100.0],
                    [-900.0, 0.0, 100.0],
                    [-900.0, 200.0, 100.0],
                    [-900.0, 200.0, -100.0],
                ],
            ),
            (
                [1.0, 0.0, 0.0],
                900.0,
                vec![
                    [900.0, 0.0, -100.0],
                    [900.0, 0.0, 100.0],
                    [900.0, 200.0, 100.0],
                    [900.0, 200.0, -100.0],
                ],
            ),
        ],
        render: SynthRenderWorld {
            blocks: vec![block(&[floor, wall], b"fixture/stone")],
            models: vec![(
                b"crate".to_vec(),
                SynthRenderWorld {
                    blocks: vec![block(&box_faces(&wm_box, 1), b"fixture/wood")],
                    models: Vec::new(),
                },
            )],
        },
        ..SynthWorld::default()
    };
    w.build()
}

/// The piece the fixture map places.
pub fn piece(cuts: Vec<Aabb>) -> Piece {
    Piece {
        name: "piece_1".into(),
        world: WORLD.into(),
        sha256: dc_rom::sha256_hex(&rom_world()),
        offset: OFFSET,
        region: region(),
        cuts,
        spawns: Spawns::Ffa,
        items: true,
        sky: false,
        blockers: true,
        fill: None,
        min_light: 0,
        drop_spawns: Vec::new(),
    }
}

/// The map: its own floor east of the piece (x 12,000 .. 14,000), a crate of its own inside the piece's region at
/// x 10,200, its own world settings, and the piece record.
pub fn map(p: &Piece) -> Vec<u8> {
    let floor = quad(
        [12_000.0, 0.0, -1_000.0],
        [12_000.0, 0.0, 1_000.0],
        [14_000.0, 0.0, 1_000.0],
        [14_000.0, 0.0, -1_000.0],
        1,
    );
    let cover = Aabb::new([10_180.0, 0.0, 300.0], [10_220.0, 100.0, 340.0]);
    let mut faces = vec![floor.clone()];
    faces.extend(box_faces(&cover, 1));
    let level = model("level", 0, [0.0; 3], &faces);
    let objects = vec![
        SynthObject::dc(
            "dc.world_settings",
            "world_settings",
            [13_000.0, 0.0, 0.0],
            [0.0; 3],
            &[("far_z", SynthValue::Real(20_000.0))],
        ),
        p.to_object(),
    ];
    let w = SynthWorld {
        info: b"decombat mapkit v1".to_vec(),
        extent_min: [11_872.0, -128.0, -1_128.0],
        extent_max: [14_128.0, 228.0, 1_128.0],
        tree_shape: vec![false],
        models: vec![level],
        objects,
        grid_origin: [11_872.0, -128.0, -1_128.0],
        grid_cell: [2_256.0, 356.0, 2_256.0],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![40, 40, 40],
        render: SynthRenderWorld {
            blocks: vec![block(&faces, b"fixture/grass")],
            models: Vec::new(),
        },
        ..SynthWorld::default()
    };
    let _ = pack_light_grid;
    w.build()
}

/// A reader that answers the fixture world for [`WORLD`].
pub fn reader() -> impl FnMut(&str) -> Result<Vec<u8>, String> {
    |path: &str| {
        if path.eq_ignore_ascii_case(WORLD) {
            Ok(rom_world())
        } else {
            Err(format!("no world `{path}`"))
        }
    }
}
