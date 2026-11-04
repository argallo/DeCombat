//! Synthetic fixtures only (no ROM data). The ROM-gated walk lives in `tests/rom_walk.rs`.

use super::*;
use crate::vertex::{stride, VertexStream};

fn xform(tx: f32, ty: f32, tz: f32) -> [f32; 16] {
    let mut m = IDENTITY;
    m[3] = tx;
    m[7] = ty;
    m[11] = tz;
    m
}

fn stream(blend: u32, flags: u32, verts: u32, seed: u8) -> VertexStream {
    let n = (stride(blend, flags) * verts) as usize;
    VertexStream {
        flags,
        data: (0..n)
            .map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed))
            .collect(),
    }
}

fn streams(blend: u32, verts: u32) -> [VertexStream; 4] {
    [
        stream(blend, 0x13, verts, 1),
        VertexStream::default(),
        VertexStream::default(),
        VertexStream::default(),
    ]
}

/// A small model touching every section and every body kind and codec.
pub(crate) fn synthetic() -> Model {
    let rigid = Mesh::Rigid(RigidMesh {
        vertex_count: 3,
        tri_count: 1,
        todo_influences: 1,
        todo_unused: 1,
        node: 1,
        streams: streams(0, 3),
        indices: vec![0, 1, 2],
    });
    let skinned = Mesh::Skinned(SkinnedMesh {
        vertex_count: 4,
        tri_count: 2,
        max_influences: 2,
        palette_influences: 2,
        has_palette_map: 0,
        streams: streams(1, 4),
        indices: vec![0, 1, 2, 2, 1, 3],
        tail: SkinTail::Spans(vec![
            SkinSpan {
                first_vertex: 0,
                vertex_count: 3,
                nodes: [0, 1, 0xff, 0xff],
                index_end: 3,
            },
            SkinSpan {
                first_vertex: 3,
                vertex_count: 1,
                nodes: [2, 0xff, 0xff, 0xff],
                index_end: 6,
            },
        ]),
    });
    let palette = Mesh::Skinned(SkinnedMesh {
        vertex_count: 3,
        tri_count: 1,
        max_influences: 3,
        palette_influences: 3,
        has_palette_map: 1,
        streams: streams(5, 3),
        indices: vec![2, 1, 0],
        tail: SkinTail::Palette {
            todo_cc: 7,
            todo_d0: 9,
            palette_map: Some(vec![0, 2, 1]),
        },
    });
    let morph = Mesh::Morph(MorphMesh {
        vertex_count: 3,
        base_vertex_count: 2,
        tri_count: 1,
        todo_influences: 0,
        todo_a8: 0,
        todo_a4: 2,
        node: 2,
        streams: streams(0, 3),
        indices: vec![0, 2, 1],
        vertex_map: vec![0, 1, 1],
    });
    let lod = |mesh: Mesh, used: Vec<u8>| Lod {
        slot_count: 1,
        slots: [3, 1, 2, 3],
        style_index: 1,
        priority: 0,
        declared_body_len: 0,
        mesh,
        used_nodes: used,
    };
    let nodes = vec![
        Node {
            name: "root".into(),
            index: 0,
            flags: 0,
            model_xform: IDENTITY,
            parent: None,
            children: vec![1, 3],
        },
        Node {
            name: "a".into(),
            index: 1,
            flags: 2,
            model_xform: xform(1.0, 0.0, 0.0),
            parent: Some(0),
            children: vec![2],
        },
        Node {
            name: "b".into(),
            index: 2,
            flags: 0,
            model_xform: xform(1.0, 2.0, 0.0),
            parent: Some(1),
            children: vec![],
        },
        Node {
            name: "c".into(),
            index: 3,
            flags: 0,
            model_xform: xform(0.0, 0.0, 5.0),
            parent: Some(0),
            children: vec![],
        },
    ];
    let markers = vec![
        Marker {
            time_ms: 0,
            command: "".into(),
        },
        Marker {
            time_ms: 100,
            command: "cmd".into(),
        },
    ];
    let raw_keys = Track::Keys {
        pos: PosKeys::F32(vec![[0.0, 1.0, 2.0], [3.0, 4.0, 5.0]]),
        rot: RotKeys::F32(vec![[0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 0.0]]),
    };
    let clip0 = Clip {
        extent: [1.0, 2.0, 3.0],
        name: "idle".into(),
        codec: CODEC_RAW,
        blend_ms: 200,
        markers: markers.clone(),
        tracks: vec![
            raw_keys.clone(),
            raw_keys.clone(),
            Track::Morph {
                flag: 1,
                frames: vec![vec![[0.5, 0.5, 0.5]; 3], vec![[1.5, 0.5, 0.5]; 3]],
            },
            raw_keys,
        ],
    };
    let clip2 = Clip {
        extent: [0.0; 3],
        name: "run".into(),
        codec: CODEC_I16,
        blend_ms: 0,
        markers: markers.clone(),
        tracks: (0..4)
            .map(|i| Track::Keys {
                pos: PosKeys::I16(if i == 0 { vec![] } else { vec![[16, -32, 8]] }),
                rot: RotKeys::I16(vec![[0, 0, 0, 32767], [0, 32767, 0, 0]]),
            })
            .collect(),
    };
    let clip3 = Clip {
        extent: [0.0; 3],
        name: "walk".into(),
        codec: CODEC_F32_POS_I16_ROT,
        blend_ms: 10,
        markers: markers.clone(),
        tracks: (0..4)
            .map(|_| Track::Keys {
                pos: PosKeys::F32(vec![[1.0, 2.0, 3.0]; 2]),
                rot: RotKeys::I16(vec![[1, 2, 3, 4]]),
            })
            .collect(),
    };
    let clip1 = Clip {
        extent: [0.0; 3],
        name: "aim".into(),
        codec: CODEC_F32,
        blend_ms: 10,
        markers,
        tracks: (0..4)
            .map(|_| Track::Keys {
                pos: PosKeys::F32(vec![[1.0, 2.0, 3.0]]),
                rot: RotKeys::F32(vec![]),
            })
            .collect(),
    };
    let mut m = Model {
        header: Header::default(),
        hints: SizeHints([2, 4, 4, 2, 2, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0]),
        command: "cmd text".into(),
        radius: 12.5,
        todo_boxes: vec![],
        pieces: vec![
            Piece {
                name: "body".into(),
                lod_dist: vec![0.0, 10.0, 20.0],
                todo_lod_lo: 0,
                todo_lod_hi: 2,
                lods: vec![
                    lod(skinned, vec![0, 1, 2]),
                    lod(rigid, vec![1]),
                    lod(palette, vec![0, 1, 2]),
                ],
            },
            Piece {
                name: "flag".into(),
                lod_dist: vec![0.0],
                todo_lod_lo: 0,
                todo_lod_hi: 0,
                lods: vec![lod(morph, vec![2])],
            },
        ],
        nodes,
        blend_sets: vec![BlendSet {
            name: "upper".into(),
            weights: vec![0.0, 1.0, 1.0, 0.5],
        }],
        child_models: vec!["other.ltb".into()],
        clips: vec![clip0, clip1, clip2, clip3],
        sockets: vec![Socket {
            node: 3,
            name: "hand".into(),
            rot: [0.0, 0.0, 0.0, 1.0],
            pos: [1.0, 2.0, 3.0],
            scale: [1.0, 1.0, 1.0],
        }],
        clip_overrides: vec![
            vec![],
            vec![ClipOverride {
                clip_name: "run".into(),
                extent: [1.0; 3],
                offset: [0.0; 3],
            }],
        ],
        trailing: vec![],
    };
    m.fix_body_lens();
    m
}

#[test]
fn synthetic_round_trip() {
    let m = synthetic();
    let bytes = m.to_bytes().expect("encode");
    let back = Model::parse(&bytes).expect("parse");
    assert_eq!(back, m);
    assert_eq!(back.to_bytes().unwrap(), bytes);
    assert!(back.trailing.is_empty());
    for l in back.lods() {
        assert_eq!(l.declared_body_len as u64, l.mesh.body_len());
    }
    assert!(back.nodes.iter().all(Node::last_row_affine));
    match &back.pieces[0].lods[0].mesh {
        Mesh::Skinned(s) => assert_eq!(s.spans_contiguous(), Some(true)),
        _ => panic!("expected skinned"),
    }
}

#[test]
fn body_len_matches_wire() {
    // body_len counts the bytes after itself up to the kind's last field.
    let m = synthetic();
    let bytes = m.to_bytes().unwrap();
    // find the first LOD: header 0x18, hints 60, str, f32, box count, piece count, piece name ...
    let mut off = 0x18 + 60;
    off += 2 + m.command.0.len() + 4 + 4 + 4;
    off += 2 + m.pieces[0].name.0.len() + 4 + 4 * 3 + 8;
    off += 4 + 16 + 4 + 1 + 4;
    let declared = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
    let used_at = off + 4 + declared;
    assert_eq!(bytes[used_at], 3, "used_node_count follows the body");
}

#[test]
fn local_transforms() {
    let m = synthetic();
    let l = m.local_xforms();
    assert_eq!(l[0], IDENTITY);
    assert_eq!(l[2], xform(0.0, 2.0, 0.0));
    assert_eq!(l[3], xform(0.0, 0.0, 5.0));
}

#[test]
fn strides() {
    use crate::vertex::{stride, VertexLayout};
    assert_eq!(stride(0, 0x13), 32);
    for infl in 1..=4u32 {
        assert_eq!(stride(infl - 1, 0x13), 32 + 4 * (infl - 1));
    }
    assert_eq!(stride(4, 0x13), 40);
    assert_eq!(stride(6, 0x13), 48);
    assert_eq!(stride(0, 0x13 | 0x100), 56);
    assert_eq!(stride(0, 0x83), 24 + 32);
    assert_eq!(stride(0, 0x10), 8);
    let l = VertexLayout::new(2, 0x13).unwrap();
    assert_eq!(
        (
            l.weights,
            l.palette_indices,
            l.normal_offset(),
            l.uv_offset(0)
        ),
        (2, false, Some(20), Some(32))
    );
    let l = VertexLayout::new(5, 0x13).unwrap();
    assert_eq!((l.weights, l.palette_indices, l.stride), (2, true, 44));
    assert!(VertexLayout::new(0, 0x113).is_none());
}

fn header(kind: u8, crev: u16, mrev: u32) -> Vec<u8> {
    let mut v = vec![kind, 0];
    v.extend_from_slice(&crev.to_le_bytes());
    v.extend_from_slice(&[0; 16]);
    v.extend_from_slice(&mrev.to_le_bytes());
    v
}

#[test]
fn header_refusals() {
    assert_eq!(
        Model::parse(&header(5, 3, 0)),
        Err(ModelError::NotAModel { file_kind: 5 })
    );
    let mut tagged = vec![9u8];
    tagged.extend_from_slice(b"XXXXXXXXXXXXXXXXXXXXXXX");
    assert_eq!(
        Model::parse(&tagged),
        Err(ModelError::NotAModel { file_kind: 9 })
    );
    assert_eq!(
        Model::parse(&header(1, 8, 25)),
        Err(ModelError::BadContainerRev { rev: 8 })
    );
    assert_eq!(
        Model::parse(&header(1, 9, 18)),
        Err(ModelError::UnsupportedRevision { rev: 18 })
    );
    assert!(matches!(
        Model::parse(&header(1, 9, 23)),
        Err(ModelError::Truncated { .. })
    ));
    assert!(matches!(
        Model::parse(&[]),
        Err(ModelError::Truncated { .. })
    ));
    assert_eq!(FileKind::of(&[5, 0]), Some(FileKind::RenderStyle));
}

#[test]
fn rev_23_parses() {
    let mut m = synthetic();
    m.header.model_rev = 23;
    let b = m.to_bytes().unwrap();
    assert_eq!(Model::parse(&b).unwrap().header.model_rev, 23);
}

#[test]
fn structural_refusals() {
    let base = synthetic();
    let mut m = base.clone();
    m.clips[1].codec = 4;
    assert!(m.to_bytes().is_err());

    // patch a codec above 3 directly into the bytes: first clip codec of the encoded synthetic
    let bytes = base.to_bytes().unwrap();
    let mut tail = base.clone();
    tail.clips.clear();
    tail.sockets.clear();
    tail.clip_overrides = vec![vec![], vec![]];
    let prefix = tail.to_bytes().unwrap();
    // prefix ends with: clip_count(4) socket_count(4) override counts(2*4)
    let clip_start = prefix.len() - 16 + 4;
    let codec_at = clip_start + 12 + 2 + base.clips[0].name.0.len();
    let mut bad = bytes.clone();
    bad[codec_at..codec_at + 4].copy_from_slice(&7u32.to_le_bytes());
    assert_eq!(
        Model::parse(&bad),
        Err(ModelError::BadCodec { clip: 0, codec: 7 })
    );

    let mut m = base.clone();
    m.sockets[0].node = 9;
    assert!(m.to_bytes().is_err());
    let mut m = base.clone();
    m.blend_sets[0].weights.pop();
    assert!(m.to_bytes().is_err());
    let mut m = base;
    m.trailing = vec![1, 2, 3];
    let b = m.to_bytes().unwrap();
    assert_eq!(Model::parse(&b).unwrap().trailing, vec![1, 2, 3]);
}

#[test]
fn too_many_pieces() {
    let mut b = header(1, 9, 25);
    b.extend_from_slice(&[0; 60]);
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&0f32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&65u32.to_le_bytes());
    assert_eq!(
        Model::parse(&b),
        Err(ModelError::TooManyPieces { count: 65 })
    );
}

/// Seeded xorshift so the fuzz run is reproducible.
struct Rng(u64);
impl Rng {
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
fn fuzz_no_panic_10k() {
    let seed_bytes = synthetic().to_bytes().unwrap();
    let mut rng = Rng(0x5eed_1ced_c0ff_ee01);
    let mut ok = 0usize;
    for round in 0..10_000 {
        let mut b = seed_bytes.clone();
        match round % 5 {
            0 => {
                let n = 1 + rng.below(8);
                for _ in 0..n {
                    let i = rng.below(b.len());
                    b[i] ^= 1 << rng.below(8);
                }
            }
            1 => {
                let i = rng.below(b.len());
                b[i] = [0x00, 0xff, 0x7f, 0x80, 0x41][rng.below(5)];
                let j = rng.below(b.len().saturating_sub(4));
                b[j..j + 4].copy_from_slice(&(rng.next() as u32).to_le_bytes());
            }
            2 => {
                let len = rng.below(b.len());
                b.truncate(len);
            }
            3 => {
                let i = rng.below(b.len());
                let extra: Vec<u8> = (0..rng.below(16)).map(|_| rng.next() as u8).collect();
                b.splice(i..i, extra);
            }
            _ => {
                let len = rng.below(512);
                b = header(1, 9, 25);
                b.extend((0..len).map(|_| rng.next() as u8));
            }
        }
        if let Ok(m) = Model::parse(&b) {
            ok += 1;
            // anything that parses must encode back to the same bytes
            assert_eq!(m.to_bytes().ok().as_deref(), Some(&b[..]), "round {round}");
        }
    }
    // some mutations land in float payloads and still parse
    assert!(ok > 0);
}

#[test]
fn parse_without_tracks_keeps_everything_but_the_key_frames() {
    let m = synthetic();
    let bytes = m.to_bytes().expect("encode");
    let light = Model::parse_without_tracks(&bytes).expect("parse");
    let mut want = Model::parse(&bytes).expect("parse");
    assert!(want.clips.iter().any(|c| !c.tracks.is_empty()));
    for c in &mut want.clips {
        c.tracks.clear();
    }
    assert_eq!(light, want);
    // a cut inside the key frames is still an error
    assert!(Model::parse_without_tracks(&bytes[..bytes.len() * 3 / 4]).is_err());
}
