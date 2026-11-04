//! Synthetic round-trip and hostile-input tests. All content is invented here; nothing comes from the ROM.

use dc_world::synth::*;
use dc_world::*;

const NAMES: [&[u8]; 3] = [b"Nm", b"Pos", b"Rot"];

fn sample_world() -> SynthWorld {
    let quad = SynthModel {
        flags: 2,
        name: b"brush_a".to_vec(),
        textures: vec![b"tex/one".to_vec(), b"tex/two".to_vec()],
        planes: vec![([0.0, 0.0, 1.0], 4.0), ([1.0, 0.0, 0.0], -2.0)],
        surfaces: vec![(0x11, 1, 7), (0, 0, 0)],
        polys: vec![(0, 0, vec![0, 1, 2, 3]), (1, 1, vec![3, 2, 1])],
        nodes: vec![(0, 5, 1, -1), (1, 0, -2, -1)],
        points: vec![
            [0.0, 0.0, 4.0],
            [1.0, 0.0, 4.0],
            [1.0, 1.0, 4.0],
            [0.0, 1.0, 4.0],
        ],
        root: 0,
        box_min: [0.0, 0.0, 4.0],
        box_max: [1.0, 1.0, 4.0],
        origin: [0.5, 0.5, 4.0],
    };
    let single = SynthModel {
        flags: 0,
        name: b"main".to_vec(),
        root: -1,
        ..SynthModel::default()
    };
    let mut spawn = SynthObject::point(
        b"SynthSpawn",
        NAMES,
        b"spawn01",
        [1.0, 2.0, 3.0],
        [0.0, 1.5, 0.0],
    );
    spawn.props.push(SynthProp {
        name: b"Hp".to_vec(),
        flags: 0,
        value: SynthValue::Real(12.5),
    });
    spawn.props.push(SynthProp {
        name: b"Mask".to_vec(),
        flags: 0,
        value: SynthValue::Bits(-7.9),
    });
    spawn.props.push(SynthProp {
        name: b"On".to_vec(),
        flags: 0,
        value: SynthValue::Bool(true),
    });
    spawn.props.push(SynthProp {
        name: b"Count".to_vec(),
        flags: 0,
        value: SynthValue::Int(41.99),
    });
    spawn.props.push(SynthProp {
        name: b"Tint".to_vec(),
        flags: 0,
        value: SynthValue::Rgb([0.25, 0.5, 1.0]),
    });
    spawn.props.push(SynthProp {
        name: b"Cmd".to_vec(),
        flags: 0x0080_0000,
        value: SynthValue::Text(b"do something".to_vec()),
    });
    let brush = SynthObject::point(b"SynthBrush", NAMES, b"brush_a", [0.0; 3], [0.0; 3]);
    let block = SynthBlock {
        center: [1.0, 2.0, 3.0],
        half: [4.0, 5.0, 6.0],
        sections: vec![SynthSection {
            textures: [b"a.dtx".to_vec(), Vec::new()],
            shader_kind: 3,
            triangles: 1,
            effect: b"fx".to_vec(),
            lightmap_w: 2,
            lightmap_h: 2,
            lightmap: vec![9; 12],
        }],
        vertices: vec![[1u8; 68], [2u8; 68], [3u8; 68]],
        triangles: vec![[0, 1, 2, 0]],
        sky_portals: vec![(
            vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0.0, 0.0, 1.0],
            0.0,
        )],
        occluders: vec![((vec![[0.0; 3]; 4], [0.0, 1.0, 0.0], 2.0), 77)],
        light_groups: vec![(
            b"lg".to_vec(),
            [1.0; 3],
            vec![1, 2],
            vec![vec![([1, 2, 3, 4], vec![5, 6, 7])], vec![]],
        )],
        child_mask: 1,
        children: [3, 0xFFFF_FFFF],
    };
    let nested = SynthRenderWorld {
        blocks: vec![SynthBlock::default()],
        models: vec![],
    };
    let mut rgb = Vec::new();
    for i in 0..(2 * 3 * 4) {
        let v = if i < 10 { [7, 7, 7] } else { [i as u8, 0, 255] };
        rgb.extend_from_slice(&v);
    }
    SynthWorld {
        info: b"key value".to_vec(),
        extent_min: [-1.0; 3],
        extent_max: [1.0; 3],
        world_offset: [0.0; 3],
        tree_shape: vec![true, false, true, false, false, false, false, false, false],
        models: vec![single, quad],
        objects: vec![spawn, brush],
        blind: vec![(9, vec![1, 2, 3]), (10, vec![])],
        grid_origin: [0.0; 3],
        grid_cell: [32.0; 3],
        grid_dims: [2, 3, 4],
        grid_rgb: rgb,
        blockers: vec![([0.0, 0.0, 1.0], 1.0, vec![[0.0; 3], [1.0; 3], [2.0; 3]])],
        particle_blockers: vec![],
        render: SynthRenderWorld {
            blocks: vec![block],
            models: vec![(b"brush_a".to_vec(), nested)],
        },
        grids: vec![(
            b"g".to_vec(),
            [1.0; 3],
            [-1, 0, 1],
            [1, 2, 2],
            vec![0, 1, 2, 3],
        )],
    }
}

#[test]
fn round_trip_all_sections() {
    let desc = sample_world();
    let bytes = desc.build();
    let w = World::parse(&bytes).expect("parse");
    w.check_layout().expect("layout");
    assert_eq!(w.header.version, WORLD_VERSION);
    assert_eq!(w.spans.last().unwrap().end, bytes.len());
    assert_eq!(w.info_text, b"key value");
    assert_eq!(w.quadtree.node_count, 9);
    assert_eq!(w.quadtree.is_split(0), Some(true));
    assert_eq!(w.quadtree.is_split(1), Some(false));
    assert_eq!(w.quadtree.is_split(2), Some(true));
    assert_eq!(w.quadtree.is_split(9), None);

    assert_eq!(w.models.len(), 2);
    assert!(!w.models[0].is_dual());
    assert_eq!(w.models[0].root, NodeRef::TermA);
    let m = &w.models[1];
    assert!(m.is_dual());
    assert_eq!(m.name, b"brush_a");
    assert_eq!(m.texture_names, vec![&b"tex/one"[..], &b"tex/two"[..]]);
    assert_eq!(
        m.planes[1],
        Plane {
            normal: [1.0, 0.0, 0.0],
            dist: -2.0
        }
    );
    assert_eq!(
        m.surfaces[0],
        Surface {
            flags: 0x11,
            texture: 1,
            texflags: 7
        }
    );
    assert_eq!(m.surface_texture(&m.surfaces[0]), Some(&b"tex/two"[..]));
    assert_eq!(m.polygons.len(), 2);
    assert_eq!(m.polygon_points(&m.polygons[0]), &[0, 1, 2, 3]);
    assert_eq!(m.polygon_points(&m.polygons[1]), &[3, 2, 1]);
    assert_eq!(m.declared_poly_verts, 7);
    assert_eq!(
        m.nodes[0],
        BspNode {
            poly: 0,
            reserved: 5,
            front: NodeRef::Node(1),
            back: NodeRef::TermA
        }
    );
    assert_eq!(m.nodes[1].front, NodeRef::TermB);
    assert_eq!(m.points[2], [1.0, 1.0, 4.0]);
    assert_eq!(m.root, NodeRef::Node(0));
    assert_eq!(m.origin, [0.5, 0.5, 4.0]);

    assert_eq!(w.objects.len(), 2);
    let o = w.objects_of_class(b"SynthSpawn").next().unwrap();
    assert_eq!(o.name(), Some(&b"spawn01"[..]));
    assert_eq!(o.position(), Some([1.0, 2.0, 3.0]));
    assert_eq!(o.rotation(), Some([0.0, 1.5, 0.0]));
    assert_eq!(o.props[1].flags, 0x0010_0000);
    assert_eq!(o.props[2].declared_len, 16);
    assert_eq!(o.prop(b"Hp").unwrap().value, FieldValue::Real(12.5));
    assert_eq!(o.prop(b"Mask").unwrap().value, FieldValue::Bits(-7));
    assert_eq!(o.prop(b"On").unwrap().value, FieldValue::Bool(true));
    assert_eq!(o.prop(b"Count").unwrap().value, FieldValue::Int(41));
    assert_eq!(
        o.prop(b"Tint").unwrap().value,
        FieldValue::Rgb([0.25, 0.5, 1.0])
    );
    let cmd = o.prop(b"Cmd").unwrap();
    assert_eq!(
        (cmd.value, cmd.declared_len, cmd.flags),
        (FieldValue::Text(b"do something"), 14, 0x0080_0000)
    );
    assert_eq!(o.role, None);
    assert_eq!(w.objects_with_role(ObjectRole::PlayerSpawn).count(), 0);
    let b = w.object_named(b"brush_a").unwrap();
    assert_eq!(w.brush_of(b).map(|m| m.name), Some(&b"brush_a"[..]));
    assert!(w.brush_of(o).is_none());
    assert_eq!(
        w.class_names(),
        vec![&b"SynthBrush"[..], &b"SynthSpawn"[..]]
    );

    assert_eq!(
        w.blind,
        vec![
            BlindBlob {
                id: 9,
                data: &[1, 2, 3]
            },
            BlindBlob { id: 10, data: &[] }
        ]
    );
    assert_eq!(w.light_grid.dims, [2, 3, 4]);
    assert_eq!(
        w.light_grid.packed.len() as u32,
        w.light_grid.declared_packed_len
    );
    assert_eq!(w.light_grid.expand(), desc.grid_rgb);
    assert_eq!(w.blockers.polys.len(), 1);
    assert_eq!(w.blockers.polys[0].verts.len(), 3);
    assert!(w.particle_blockers.polys.is_empty());

    let r = w.render.as_ref().unwrap();
    let blk = &r.world.blocks[0];
    assert_eq!(blk.sections[0].textures[0], b"a.dtx");
    assert_eq!(blk.sections[0].lightmap.len(), 12);
    assert_eq!(blk.vertex_count, 3);
    assert_eq!(blk.vertices.len(), 3 * RENDER_VERTEX_BYTES);
    assert_eq!(blk.triangle(0), Some([0, 1, 2, 0]));
    assert_eq!(blk.sky_portals[0].verts.len(), 3);
    assert_eq!(blk.occluders[0].1, 77);
    assert_eq!(blk.light_groups[0].slots[0][0].data, &[5, 6, 7]);
    assert_eq!(blk.children, [3, 0xFFFF_FFFF]);
    assert_eq!(r.world.models[0].name, b"brush_a");
    assert_eq!(r.grids[0].data, &[0, 1, 2, 3]);
    assert_eq!(r.grids[0].offset, [-1, 0, 1]);

    let s = w.stats();
    assert_eq!(
        (
            s.models,
            s.dual,
            s.objects,
            s.properties,
            s.render_top.blocks,
            s.render.blocks
        ),
        (2, 1, 2, 12, 1, 2)
    );
    assert_eq!(s.render_top.models, s.dual);

    let lean = World::parse_with(&bytes, ParseOptions { render: false }).unwrap();
    assert!(lean.render.is_none());
    lean.check_layout().unwrap();
}

#[test]
fn engine_rejection_rules() {
    // SPEC: FMT_DAT85.md §12: the bounds rules of §5.2 are errors
    let mut d = sample_world();
    d.models[1].polys[0].0 = 2;
    assert!(matches!(
        World::parse(&d.build()),
        Err(WorldError::IndexOutOfRange {
            what: "poly_surface",
            ..
        })
    ));
    let mut d = sample_world();
    d.models[1].polys[1].1 = 9;
    assert!(matches!(
        World::parse(&d.build()),
        Err(WorldError::IndexOutOfRange {
            what: "poly_plane",
            ..
        })
    ));
    let mut d = sample_world();
    d.models[1].polys[1].2[0] = 4;
    assert!(matches!(
        World::parse(&d.build()),
        Err(WorldError::IndexOutOfRange {
            what: "poly_point",
            ..
        })
    ));
    let mut d = sample_world();
    d.models[1].nodes[0].2 = 2;
    assert!(matches!(
        World::parse(&d.build()),
        Err(WorldError::IndexOutOfRange {
            what: "node_front",
            ..
        })
    ));
    let mut d = sample_world();
    d.models[1].nodes[1].3 = -3;
    assert!(matches!(
        World::parse(&d.build()),
        Err(WorldError::IndexOutOfRange {
            what: "node_back",
            ..
        })
    ));
    let mut d = sample_world();
    d.models[1].nodes[0].0 = 2;
    assert!(matches!(
        World::parse(&d.build()),
        Err(WorldError::IndexOutOfRange {
            what: "node_poly",
            ..
        })
    ));
    let mut d = sample_world();
    d.render.blocks[0].triangles[0][2] = 3;
    assert!(matches!(
        World::parse(&d.build()),
        Err(WorldError::IndexOutOfRange {
            what: "triangle vertex",
            ..
        })
    ));

    let mut bytes = sample_world().build();
    bytes[0] = 84;
    assert_eq!(World::parse(&bytes), Err(WorldError::BadVersion(84)));
    let mut bytes = sample_world().build();
    bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        World::parse(&bytes),
        Err(WorldError::BadOffset {
            section: Section::Objects,
            ..
        })
    ));
    assert!(matches!(
        World::parse(&[]),
        Err(WorldError::Truncated {
            section: Section::Header,
            ..
        })
    ));

    let mut d = sample_world();
    d.tree_shape.pop();
    assert!(matches!(
        World::parse(&d.build()),
        Err(WorldError::QuadtreeShape { .. })
    ));
}

#[test]
fn portals_and_trailer_rejected() {
    // Locate the dual model's portal count by building, then patch it: body starts after the single model.
    let d = sample_world();
    let bytes = d.build();
    let w = World::parse(&bytes).unwrap();
    let at = w.models[1].body_at;
    let name_len = w.models[1].name.len();
    let portals_at = at + 4 + 2 + name_len + 12; // flags, lstr, points, planes, surfaces
    let mut b = bytes.clone();
    b[portals_at..portals_at + 4].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        World::parse(&b),
        Err(WorldError::PortalsPresent { model: 1, count: 1 })
    );
    // The trailing count is the last word before the object list.
    let mut b = bytes.clone();
    let tail = w.header.objects_at as usize - 4;
    b[tail] = 3;
    assert_eq!(
        World::parse(&b),
        Err(WorldError::TrailingCount { model: 1, value: 3 })
    );
}

#[test]
fn layout_gap_detected() {
    let mut bytes = sample_world().build();
    // Pad one byte after the blind data and shift the later offsets.
    let w = World::parse(&bytes).unwrap();
    let at = w.header.lightgrid_at as usize;
    bytes.insert(at, 0xAA);
    for slot in 3..7 {
        let off = 4 * slot;
        let v = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) + 1;
        bytes[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    let w = World::parse(&bytes).unwrap();
    assert!(matches!(
        w.check_layout(),
        Err(WorldError::Layout {
            section: Section::BlindData,
            ..
        })
    ));
}

#[test]
fn light_grid_packing_roundtrip() {
    let mut rgb = Vec::new();
    for i in 0..1000u32 {
        let v = if i % 300 < 200 {
            [1, 2, 3]
        } else {
            [(i % 251) as u8, (i % 7) as u8, 0]
        };
        rgb.extend_from_slice(&v);
    }
    let mut d = sample_world();
    d.grid_dims = [10, 10, 10];
    d.grid_rgb = rgb.clone();
    let bytes = d.build();
    let w = World::parse(&bytes).unwrap();
    assert_eq!(w.light_grid.expand(), rgb);
}

#[test]
fn render_nesting_is_bounded() {
    let mut d = sample_world();
    let mut inner = SynthRenderWorld::default();
    for _ in 0..(MAX_RENDER_DEPTH + 2) {
        inner = SynthRenderWorld {
            blocks: vec![],
            models: vec![(b"n".to_vec(), inner)],
        };
    }
    d.render = inner;
    assert!(matches!(
        World::parse(&d.build()),
        Err(WorldError::NestingTooDeep { .. })
    ));
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn fuzz_mutated_buffers_never_panic() {
    let base = sample_world().build();
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    let mut outcomes = [0u32; 2];
    for _ in 0..10_000 {
        let mut b = base.clone();
        let edits = 1 + rng.below(8);
        for _ in 0..edits {
            if b.is_empty() {
                b.push(rng.next() as u8);
            }
            match rng.below(6) {
                0 => {
                    let i = rng.below(b.len());
                    b[i] ^= 1 << rng.below(8);
                }
                1 => {
                    let i = rng.below(b.len());
                    b[i] = rng.next() as u8;
                }
                2 if b.len() >= 4 => {
                    let i = rng.below(b.len() - 3);
                    let v = match rng.below(4) {
                        0 => u32::MAX,
                        1 => 0x8000_0000,
                        2 => rng.next() as u32,
                        _ => (rng.next() % 4096) as u32,
                    };
                    b[i..i + 4].copy_from_slice(&v.to_le_bytes());
                }
                3 => {
                    let n = rng.below(b.len() + 1);
                    b.truncate(n);
                }
                4 => {
                    let i = rng.below(b.len() + 1);
                    b.insert(i, rng.next() as u8);
                }
                _ if !b.is_empty() => {
                    let i = rng.below(b.len());
                    b.remove(i);
                }
                _ => {}
            }
        }
        match World::parse(&b) {
            Ok(w) => {
                outcomes[0] += 1;
                let _ = w.check_layout();
                let _ = w.stats();
                let _ = w.light_grid.expand();
                for m in &w.models {
                    for p in &m.polygons {
                        let _ = m.polygon_points(p);
                    }
                }
            }
            Err(e) => {
                outcomes[1] += 1;
                let _ = e.to_string();
            }
        }
    }
    assert_eq!(outcomes[0] + outcomes[1], 10_000);
    assert!(outcomes[1] > 0, "mutations should produce errors");
}
