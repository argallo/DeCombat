//! `dc-mapkit asset`: turns a glTF binary (`.glb`) into what a map can use (MAP-104). Each material of the model becomes an OBJ with
//! texture coordinates and normals plus a PNG, simplified and shrunk to the budgets of the game's own assets. The result is ordinary
//! project data under `assets/`, placed with the `asset.place` op.
//!
//! It reads what web-compressed models use: `EXT_meshopt_compression`, `KHR_mesh_quantization` and WebP or PNG textures.
//!
//! Provenance: ORIGINAL (the public glTF 2.0 format and its public extensions; no game data).
// DC-DEFAULT: MAP-104 (proposed; USER DECISION 2026-11-03) the budgets and the conversion are ours

use crate::geom::{write_obj, ObjMesh, V3};
use meshopt_rs::vertex::Position;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// What the conversion aims for.
#[derive(Debug, Clone)]
pub struct GlbOptions {
    /// Allow more shape error to get near `max_tris`, for what is seen from a distance. Texture seams always hold.
    pub strict: bool,
    /// Replace the geometry by a simple solid with the model's look projected on it.
    pub proxy: Proxy,
    /// Only the textures are wanted: leave the geometry out, however heavy the model is.
    pub textures_only: bool,
    /// How strongly a texture's normal map is drawn into its colour, for walls and floors; 0 leaves it out.
    pub relief: f32,
    /// Triangles for the whole model.
    pub max_tris: usize,
    /// The largest side of a texture; a power of two is kept.
    pub tex: u32,
    /// Quarter or free turns applied before anything else, such as `x90` or `z-90,y45`.
    pub orient: String,
    /// Parts to leave out, as words of their material names separated by commas: a wire mesh, which a bake would turn
    /// into a solid sheet because this renderer has no cut-outs.
    pub leave_out: String,
}

/// A simple solid that stands in for a model (DC-DEFAULT: MAP-106).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Proxy {
    /// The model's own geometry, simplified.
    #[default]
    None,
    /// A box around the model: crates, cartons, cabinets.
    Box,
    /// A prism on the model's outline from above: drums, bins.
    Round,
    /// The model's own shape, simplified, with one new texture drawn from the full-detail source (DC-DEFAULT: MAP-107).
    Rebake,
}

impl Proxy {
    pub fn parse(text: &str) -> Result<Proxy, String> {
        match text {
            "" | "none" => Ok(Proxy::None),
            "box" => Ok(Proxy::Box),
            "round" => Ok(Proxy::Round),
            "rebake" => Ok(Proxy::Rebake),
            other => Err(format!("`{other}`: box, round, rebake or none")),
        }
    }
}

/// One material's part of a converted model.
#[derive(Debug, Clone, PartialEq)]
pub struct GlbPart {
    pub mesh: ObjMesh,
    pub png: Vec<u8>,
    pub tris: usize,
}

/// A converted model, in metres with its base centred on the origin.
#[derive(Debug, Clone, PartialEq)]
pub struct GlbModel {
    pub parts: Vec<GlbPart>,
    pub size: [f32; 3],
    pub source_tris: usize,
    /// See [`footprint`].
    pub footprint: Vec<[f32; 2]>,
    /// Boxes that follow the model's solid parts, `[min x, y, z, max x, y, z]` in metres, for its collision: what
    /// you can see through you can shoot through. Empty: the footprint serves (DC-DEFAULT: MAP-109).
    pub hull_boxes: Vec<[f32; 6]>,
}

type Mat = [f32; 16];
const IDENTITY: Mat = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut o = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            o[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    o
}

fn point(m: &Mat, p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// The matrix mirrors: its triangles turn the other way round.
fn winding_flips(m: &Mat) -> bool {
    let det = m[0] * (m[5] * m[10] - m[6] * m[9]) - m[4] * (m[1] * m[10] - m[2] * m[9])
        + m[8] * (m[1] * m[6] - m[2] * m[5]);
    det < 0.0
}

fn trs(node: &Value) -> Mat {
    if let Some(a) = node["matrix"].as_array() {
        let mut m = IDENTITY;
        for (i, v) in a.iter().take(16).enumerate() {
            m[i] = v.as_f64().unwrap_or(0.0) as f32;
        }
        return m;
    }
    let f3 = |k: &str, d: [f32; 3]| -> [f32; 3] {
        node[k].as_array().map_or(d, |a| {
            [0, 1, 2].map(|i| a.get(i).and_then(Value::as_f64).unwrap_or(d[i] as f64) as f32)
        })
    };
    let (t, s) = (f3("translation", [0.0; 3]), f3("scale", [1.0; 3]));
    let q: [f32; 4] = node["rotation"]
        .as_array()
        .map_or([0.0, 0.0, 0.0, 1.0], |a| {
            [0, 1, 2, 3].map(|i| a.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32)
        });
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    [
        (1.0 - 2.0 * (y * y + z * z)) * s[0],
        (2.0 * (x * y + z * w)) * s[0],
        (2.0 * (x * z - y * w)) * s[0],
        0.0,
        (2.0 * (x * y - z * w)) * s[1],
        (1.0 - 2.0 * (x * x + z * z)) * s[1],
        (2.0 * (y * z + x * w)) * s[1],
        0.0,
        (2.0 * (x * z + y * w)) * s[2],
        (2.0 * (y * z - x * w)) * s[2],
        (1.0 - 2.0 * (x * x + y * y)) * s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

/// `x90,z-90`: turns in the order written, around the model's own axes.
pub fn parse_orient(text: &str) -> Result<Mat, String> {
    let mut m = IDENTITY;
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (axis, deg) = part.split_at(1);
        let deg: f32 = deg
            .parse()
            .map_err(|_| format!("--orient: `{part}` is not an axis and degrees, like x90"))?;
        let (s, c) = deg.to_radians().sin_cos();
        let r: Mat = match axis {
            "x" => [
                1.0, 0.0, 0.0, 0.0, 0.0, c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            "y" => [
                c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            "z" => [
                c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            _ => return Err(format!("--orient: `{part}` has no axis x, y or z")),
        };
        m = mul(&r, &m);
    }
    Ok(m)
}

struct Gltf<'a> {
    json: Value,
    bin: &'a [u8],
    views: HashMap<usize, Vec<u8>>,
}

fn decode_vertices<const N: usize>(
    out: &mut Vec<u8>,
    count: usize,
    src: &[u8],
) -> Result<(), String> {
    let mut v = vec![[0u8; N]; count];
    meshopt_rs::vertex::buffer::decode_vertex_buffer(&mut v, src)
        .map_err(|e| format!("meshopt vertices: {e:?}"))?;
    out.extend(v.iter().flatten());
    Ok(())
}

impl<'a> Gltf<'a> {
    /// The bytes of a buffer view, decompressed when it is meshopt data.
    fn view(&mut self, i: usize) -> Result<&[u8], String> {
        if !self.views.contains_key(&i) {
            let v = &self.json["bufferViews"][i];
            let data = if let Some(e) = v["extensions"].get("EXT_meshopt_compression") {
                let at = e["byteOffset"].as_u64().unwrap_or(0) as usize;
                let len = e["byteLength"].as_u64().unwrap_or(0) as usize;
                let src = self
                    .bin
                    .get(at..at + len)
                    .ok_or("meshopt data past the end of the file")?;
                decode_meshopt(e, src)?
            } else {
                let at = v["byteOffset"].as_u64().unwrap_or(0) as usize;
                let len = v["byteLength"].as_u64().unwrap_or(0) as usize;
                self.bin
                    .get(at..at + len)
                    .ok_or("a buffer view past the end of the file")?
                    .to_vec()
            };
            self.views.insert(i, data);
        }
        Ok(&self.views[&i])
    }

    /// An accessor as floats per element, normalized as glTF says; `None` when it is absent.
    fn floats(&mut self, accessor: &Value, n: usize) -> Result<Option<Vec<Vec<f32>>>, String> {
        let Some(i) = accessor.as_u64() else {
            return Ok(None);
        };
        let a = self.json["accessors"][i as usize].clone();
        let (count, ct) = (
            a["count"].as_u64().unwrap_or(0) as usize,
            a["componentType"].as_u64().unwrap_or(0),
        );
        let normalized = a["normalized"].as_bool().unwrap_or(false);
        let vi = a["bufferView"]
            .as_u64()
            .ok_or("an accessor with no buffer view")? as usize;
        let size = match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return Err(format!("component type {ct}")),
        };
        let stride = self.json["bufferViews"][vi]["byteStride"]
            .as_u64()
            .or_else(|| {
                self.json["bufferViews"][vi]["extensions"]["EXT_meshopt_compression"]["byteStride"]
                    .as_u64()
            })
            .map_or(size * n, |s| s as usize);
        let off = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let d = self.view(vi)?;
        let mut out = Vec::with_capacity(count);
        for e in 0..count {
            let mut row = Vec::with_capacity(n);
            for c in 0..n {
                let at = off + e * stride + c * size;
                let b = d
                    .get(at..at + size)
                    .ok_or("an accessor past its buffer view")?;
                row.push(match ct {
                    5120 => {
                        let v = b[0] as i8 as f32;
                        if normalized {
                            (v / 127.0).max(-1.0)
                        } else {
                            v
                        }
                    }
                    5121 => {
                        let v = b[0] as f32;
                        if normalized {
                            v / 255.0
                        } else {
                            v
                        }
                    }
                    5122 => {
                        let v = i16::from_le_bytes([b[0], b[1]]) as f32;
                        if normalized {
                            (v / 32767.0).max(-1.0)
                        } else {
                            v
                        }
                    }
                    5123 => {
                        let v = u16::from_le_bytes([b[0], b[1]]) as f32;
                        if normalized {
                            v / 65535.0
                        } else {
                            v
                        }
                    }
                    5125 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
                    _ => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                });
            }
            out.push(row);
        }
        Ok(Some(out))
    }

    fn indices(&mut self, accessor: &Value, vertices: usize) -> Result<Vec<u32>, String> {
        let Some(i) = accessor.as_u64() else {
            return Ok((0..vertices as u32).collect());
        };
        let a = self.json["accessors"][i as usize].clone();
        let (count, ct) = (
            a["count"].as_u64().unwrap_or(0) as usize,
            a["componentType"].as_u64().unwrap_or(0),
        );
        let vi = a["bufferView"]
            .as_u64()
            .ok_or("indices with no buffer view")? as usize;
        let off = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let size = match ct {
            5121 => 1,
            5123 => 2,
            5125 => 4,
            _ => return Err(format!("index component type {ct}")),
        };
        let d = self.view(vi)?;
        let b = d
            .get(off..off + count * size)
            .ok_or("indices past their buffer view")?;
        Ok(b.chunks_exact(size)
            .map(|c| match size {
                1 => c[0] as u32,
                2 => u16::from_le_bytes([c[0], c[1]]) as u32,
                _ => u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            })
            .collect())
    }
}

fn decode_meshopt(e: &Value, src: &[u8]) -> Result<Vec<u8>, String> {
    let (stride, count) = (
        e["byteStride"].as_u64().unwrap_or(0) as usize,
        e["count"].as_u64().unwrap_or(0) as usize,
    );
    let mode = e["mode"].as_str().unwrap_or("ATTRIBUTES");
    let mut out = Vec::with_capacity(stride * count);
    match mode {
        "ATTRIBUTES" => match stride {
            4 => decode_vertices::<4>(&mut out, count, src)?,
            8 => decode_vertices::<8>(&mut out, count, src)?,
            12 => decode_vertices::<12>(&mut out, count, src)?,
            16 => decode_vertices::<16>(&mut out, count, src)?,
            20 => decode_vertices::<20>(&mut out, count, src)?,
            24 => decode_vertices::<24>(&mut out, count, src)?,
            28 => decode_vertices::<28>(&mut out, count, src)?,
            32 => decode_vertices::<32>(&mut out, count, src)?,
            _ => return Err(format!("meshopt stride {stride}")),
        },
        "TRIANGLES" | "INDICES" => {
            let mut v = vec![0u32; count];
            let r = if mode == "TRIANGLES" {
                meshopt_rs::index::buffer::decode_index_buffer(&mut v, src)
            } else {
                meshopt_rs::index::sequence::decode_index_sequence(&mut v, src)
            };
            r.map_err(|e| format!("meshopt indices: {e:?}"))?;
            for i in v {
                match stride {
                    1 => out.push(i as u8),
                    2 => out.extend((i as u16).to_le_bytes()),
                    _ => out.extend(i.to_le_bytes()),
                }
            }
        }
        m => return Err(format!("meshopt mode {m}")),
    }
    match e["filter"].as_str().unwrap_or("NONE") {
        "NONE" => {}
        "OCTAHEDRAL" if stride == 4 => {
            let mut v: Vec<[u8; 4]> = out
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| [c[0], c[1], c[2], c[3]])
                .collect();
            meshopt_rs::vertex::filter::decode_filter_oct_8(&mut v);
            out = v.concat();
        }
        "OCTAHEDRAL" | "QUATERNION" => {
            let mut v: Vec<[u16; 4]> = out
                .as_chunks::<8>()
                .0
                .iter()
                .map(|c| [0, 2, 4, 6].map(|k| u16::from_le_bytes([c[k], c[k + 1]])))
                .collect();
            if e["filter"] == "OCTAHEDRAL" {
                meshopt_rs::vertex::filter::decode_filter_oct_16(&mut v);
            } else {
                meshopt_rs::vertex::filter::decode_filter_quat(&mut v);
            }
            out = v.iter().flatten().flat_map(|x| x.to_le_bytes()).collect();
        }
        "EXPONENTIAL" => {
            let mut v: Vec<u32> = out
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            meshopt_rs::vertex::filter::decode_filter_exp(&mut v);
            out = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        }
        f => return Err(format!("meshopt filter {f}")),
    }
    Ok(out)
}

#[derive(Clone, Copy)]
struct Vert {
    p: [f32; 3],
    uv: [f32; 2],
    /// COLOR_0, which glTF multiplies into the base colour.
    c: [f32; 3],
}

impl Position for Vert {
    fn pos(&self) -> [f32; 3] {
        self.p
    }
}

#[derive(Default)]
struct Gathered {
    verts: Vec<Vert>,
    tris: Vec<u32>,
}

/// KHR_texture_transform of a material's base colour as `u' = a u + b v + c`, `v' = d u + e v + f`: without it a
/// model that repeats or offsets its texture shows the wrong part of the picture.
fn uv_transform(json: &Value, material: i64) -> [f32; 6] {
    let t = &json["materials"][material.max(0) as usize]["pbrMetallicRoughness"]
        ["baseColorTexture"]["extensions"]["KHR_texture_transform"];
    let pair = |v: &Value, d: f32| {
        v.as_array().map_or([d, d], |a| {
            [0, 1].map(|i| a.get(i).and_then(Value::as_f64).unwrap_or(d as f64) as f32)
        })
    };
    let (o, sc) = (pair(&t["offset"], 0.0), pair(&t["scale"], 1.0));
    let r = t["rotation"].as_f64().unwrap_or(0.0) as f32;
    let (cos, sin) = (r.cos(), r.sin());
    [
        cos * sc[0],
        sin * sc[1],
        o[0],
        -sin * sc[0],
        cos * sc[1],
        o[1],
    ]
}

fn walk(
    g: &mut Gltf,
    node: usize,
    parent: &Mat,
    out: &mut BTreeMap<i64, Gathered>,
) -> Result<(), String> {
    let n = g.json["nodes"][node].clone();
    let m = mul(parent, &trs(&n));
    if let Some(mesh) = n["mesh"].as_u64() {
        let prims = g.json["meshes"][mesh as usize]["primitives"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for prim in prims {
            if prim["mode"].as_u64().unwrap_or(4) != 4 {
                continue;
            }
            let at = &prim["attributes"];
            let pos = g
                .floats(&at["POSITION"], 3)?
                .ok_or("a primitive with no POSITION")?;
            let uv = g.floats(&at["TEXCOORD_0"], 2)?;
            let colours = g.floats(&at["COLOR_0"], 3)?;
            let ind = g.indices(&prim["indices"], pos.len())?;
            let key = prim["material"].as_i64().unwrap_or(-1);
            let turn = uv_transform(&g.json, key);
            let e = out.entry(key).or_default();
            let base = e.verts.len() as u32;
            for (i, p) in pos.iter().enumerate() {
                let t = uv
                    .as_ref()
                    .and_then(|v| v.get(i))
                    .map_or([0.0, 0.0], |x| [x[0], x[1]]);
                let c = colours
                    .as_ref()
                    .and_then(|v| v.get(i))
                    .map_or([1.0; 3], |x| [x[0], x[1], x[2]]);
                e.verts.push(Vert {
                    p: point(&m, [p[0], p[1], p[2]]),
                    uv: [
                        turn[0] * t[0] + turn[1] * t[1] + turn[2],
                        turn[3] * t[0] + turn[4] * t[1] + turn[5],
                    ],
                    c,
                });
            }
            let flip = winding_flips(&m);
            for t in ind.as_chunks::<3>().0 {
                if t.iter().any(|i| *i as usize >= pos.len()) {
                    return Err("an index past its vertices".into());
                }
                if flip {
                    e.tris.extend([base + t[0], base + t[2], base + t[1]]);
                } else {
                    e.tris.extend([base + t[0], base + t[1], base + t[2]]);
                }
            }
        }
    }
    for c in n["children"].as_array().cloned().unwrap_or_default() {
        if let Some(c) = c.as_u64() {
            walk(g, c as usize, &m, out)?;
        }
    }
    Ok(())
}

/// Joins vertices that agree in position and uv, which a simplifier needs to see surfaces as connected.
fn weld(g: &Gathered) -> Gathered {
    // not the normal: on a hard-edged model every corner would count as a seam and nothing could collapse;
    // [`corner_normals`] brings the hard edges back afterwards (DC-DEFAULT: MAP-106)
    let mut seen: HashMap<[i32; 5], u32> = HashMap::new();
    let mut out = Gathered::default();
    let mut remap = Vec::with_capacity(g.verts.len());
    for v in &g.verts {
        let q = |x: f32, s: f32| (x * s).round() as i32;
        let key = [
            q(v.p[0], 1e5),
            q(v.p[1], 1e5),
            q(v.p[2], 1e5),
            q(v.uv[0], 1e4),
            q(v.uv[1], 1e4),
        ];
        let i = *seen.entry(key).or_insert_with(|| {
            out.verts.push(*v);
            (out.verts.len() - 1) as u32
        });
        remap.push(i);
    }
    out.tris = g.tris.iter().map(|i| remap[*i as usize]).collect();
    out
}

/// Fewer triangles: quality-keeping first with a growing error, then the coarse way for what will not go lower.
/// Rays per triangle in [`remove_hidden`]: the corners, edges and faces of a cube.
const HIDDEN_RAYS: [[f32; 3]; 26] = {
    let (a, b) = (0.577_350_3, std::f32::consts::FRAC_1_SQRT_2);
    [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
        [b, b, 0.0],
        [b, -b, 0.0],
        [-b, b, 0.0],
        [-b, -b, 0.0],
        [b, 0.0, b],
        [b, 0.0, -b],
        [-b, 0.0, b],
        [-b, 0.0, -b],
        [0.0, b, b],
        [0.0, b, -b],
        [0.0, -b, b],
        [0.0, -b, -b],
        [a, a, a],
        [a, a, -a],
        [a, -a, a],
        [a, -a, -a],
        [-a, a, a],
        [-a, a, -a],
        [-a, -a, a],
        [-a, -a, -a],
    ]
};
/// Cells per axis of the grid in [`remove_hidden`].
const HIDDEN_GRID: usize = 48;

/// Drop the triangles no ray from outside can reach: the metal body under a can's printed label, a box's inside. Such
/// a shell lies a hair behind the outer one, so simplifying pushes it through and it shows as jagged patches. A
/// triangle stays when any of [`HIDDEN_RAYS`] from its centre leaves the model (DC-DEFAULT: MAP-104).
fn remove_hidden(by_material: &mut BTreeMap<i64, Gathered>) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    let mut all: Vec<[[f32; 3]; 3]> = Vec::new();
    for e in by_material.values() {
        for t in e.tris.as_chunks::<3>().0 {
            let tri = [
                e.verts[t[0] as usize].p,
                e.verts[t[1] as usize].p,
                e.verts[t[2] as usize].p,
            ];
            for p in &tri {
                for c in 0..3 {
                    lo[c] = lo[c].min(p[c]);
                    hi[c] = hi[c].max(p[c]);
                }
            }
            all.push(tri);
        }
    }
    let size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let cell = (size[0].max(size[1]).max(size[2]) / HIDDEN_GRID as f32).max(1e-6);
    let dims = [0, 1, 2].map(|c| ((size[c] / cell) as usize + 1).min(HIDDEN_GRID));
    let at = |p: [f32; 3], c: usize| {
        (((p[c] - lo[c]) / cell) as isize).clamp(0, dims[c] as isize - 1) as usize
    };
    let index = |x: usize, y: usize, z: usize| x + dims[0] * (y + dims[1] * z);
    let mut grid: Vec<Vec<u32>> = vec![Vec::new(); dims[0] * dims[1] * dims[2]];
    for (i, t) in all.iter().enumerate() {
        let range = |c: usize| {
            let (a, b, d) = (at(t[0], c), at(t[1], c), at(t[2], c));
            a.min(b).min(d)..=a.max(b).max(d)
        };
        for z in range(2) {
            for y in range(1) {
                for x in range(0) {
                    grid[index(x, y, z)].push(i as u32);
                }
            }
        }
    }
    let mut stamp = vec![0u32; all.len()];
    let mut ray_id = 0u32;
    let mut first = 0usize;
    for e in by_material.values_mut() {
        let count = e.tris.len() / 3;
        let mut kept = Vec::with_capacity(e.tris.len());
        for k in 0..count {
            let id = first + k;
            let t = &all[id];
            let o = [0, 1, 2].map(|c| (t[0][c] + t[1][c] + t[2][c]) / 3.0);
            let escapes = HIDDEN_RAYS.iter().any(|d| {
                ray_id += 1;
                let mut p = o;
                loop {
                    let inside = (0..3).all(|c| p[c] >= lo[c] && p[c] <= hi[c]);
                    if !inside {
                        return true;
                    }
                    for &other in &grid[index(at(p, 0), at(p, 1), at(p, 2))] {
                        if other as usize != id
                            && stamp[other as usize] != ray_id
                            && ray_hits(o, *d, &all[other as usize])
                        {
                            return false;
                        }
                        stamp[other as usize] = ray_id;
                    }
                    for c in 0..3 {
                        p[c] += d[c] * cell * 0.5;
                    }
                }
            });
            if escapes {
                kept.extend_from_slice(&e.tris[k * 3..k * 3 + 3]);
            }
        }
        first += count;
        // a model that is nothing but hidden shells keeps them
        if !kept.is_empty() {
            e.tris = kept;
        }
    }
}

/// Whether the ray from `o` along `d` meets the triangle in front of `o` (Moller-Trumbore).
fn ray_hits(o: [f32; 3], d: [f32; 3], t: &[[f32; 3]; 3]) -> bool {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (e1, e2) = (sub(t[1], t[0]), sub(t[2], t[0]));
    let h = cross(d, e2);
    let det = dot(e1, h);
    if det.abs() < 1e-14 {
        return false;
    }
    let s = sub(o, t[0]);
    let u = dot(s, h) / det;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = cross(s, e1);
    let v = dot(d, q) / det;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    dot(e2, q) / det > 1e-7
}

/// The range per channel below which a texture counts as one plain colour.
const PLAIN_RANGE: u8 = 10;

/// The relative errors a simplification tries.
const ERRORS: [f32; 5] = [0.002, 0.005, 0.01, 0.02, 0.035];
/// With `strict`: for things seen from a distance.
const ERRORS_STRICT: [f32; 7] = [0.002, 0.005, 0.01, 0.02, 0.035, 0.06, 0.1];

fn simplify(g: &Gathered, target_tris: usize, errors: &[f32]) -> Vec<u32> {
    let target = target_tris.max(1) * 3;
    if g.tris.len() <= target {
        return g.tris.clone();
    }
    // Growing relative error, stopping short of what visibly damages a model: a collapse is judged on shape alone,
    // so past a few percent labels smear across the surface and a can goes jagged. Seams always hold. A model that
    // cannot reach its budget keeps more triangles rather than lose its look (DC-DEFAULT: MAP-106: the coarse
    // vertex-clustering pass ignores seams, and on a textured part it smears and bends; see `convert`).
    let mut dest = vec![0u32; g.tris.len()];
    let mut best = g.tris.clone();
    for error in errors {
        let n = meshopt_rs::simplify::simplify(&mut dest, &g.tris, &g.verts, target, *error);
        best = dest[..n].to_vec();
        if n <= target {
            return best;
        }
    }
    best
}

fn base_colour_image(
    g: &mut Gltf,
    material: i64,
    max: u32,
    relief: f32,
) -> Result<Vec<u8>, String> {
    let m = g.json["materials"]
        .get(material as usize)
        .cloned()
        .unwrap_or(Value::Null);
    let pbr = &m["pbrMetallicRoughness"];
    let factor: [f32; 4] = pbr["baseColorFactor"].as_array().map_or([1.0; 4], |a| {
        [0, 1, 2, 3].map(|i| a.get(i).and_then(Value::as_f64).unwrap_or(1.0) as f32)
    });
    let srgb = |linear: f32| -> f32 {
        let l = linear.clamp(0.0, 1.0);
        if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        }
    };
    let tint = [srgb(factor[0]), srgb(factor[1]), srgb(factor[2])];
    let image = match pbr["baseColorTexture"]["index"].as_u64() {
        None => None,
        Some(t) => {
            let tex = g.json["textures"][t as usize].clone();
            let source = tex["extensions"]["EXT_texture_webp"]["source"]
                .as_u64()
                .or_else(|| tex["source"].as_u64())
                .ok_or("a texture with no image")?;
            let view = g.json["images"][source as usize]["bufferView"]
                .as_u64()
                .ok_or("an image with no buffer view")? as usize;
            let bytes = g.view(view)?;
            Some(
                image::load_from_memory(bytes)
                    .map_err(|e| format!("texture: {e}"))?
                    .to_rgba8(),
            )
        }
    };
    let normal_map = if relief > 0.0 {
        match m["normalTexture"]["index"].as_u64() {
            None => None,
            Some(t) => {
                let tex = g.json["textures"][t as usize].clone();
                let source = tex["extensions"]["EXT_texture_webp"]["source"]
                    .as_u64()
                    .or_else(|| tex["source"].as_u64())
                    .ok_or("a texture with no image")?;
                let view = g.json["images"][source as usize]["bufferView"]
                    .as_u64()
                    .ok_or("an image with no buffer view")? as usize;
                let bytes = g.view(view)?;
                Some(
                    image::load_from_memory(bytes)
                        .map_err(|e| format!("texture: {e}"))?
                        .to_rgba8(),
                )
            }
        }
    } else {
        None
    };
    let mut out = match image {
        Some(i) => {
            let fit = |v: u32| v.min(max).max(1).next_power_of_two().min(max);
            let (w, h) = (fit(i.width()), fit(i.height()));
            let mut r = image::imageops::resize(&i, w, h, image::imageops::FilterType::Triangle);
            // the surface's relief, lit from one side: this renderer has no normal maps, and it is the difference
            // between a flat colour and concrete (DC-DEFAULT: MAP-107)
            if let Some(n) = &normal_map {
                let n = image::imageops::resize(n, w, h, image::imageops::FilterType::Triangle);
                for (p, q) in r.pixels_mut().zip(n.pixels()) {
                    let nx = q[0] as f32 / 127.5 - 1.0;
                    let ny = q[1] as f32 / 127.5 - 1.0;
                    let k = (1.0 + relief * (0.6 * nx + 0.8 * ny)).clamp(0.4, 1.6);
                    for c in 0..3 {
                        p[c] = (p[c] as f32 * k).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
            for p in r.pixels_mut() {
                for c in 0..3 {
                    p[c] = (p[c] as f32 * tint[c]).round().clamp(0.0, 255.0) as u8;
                }
            }
            r
        }
        None => image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([
                (tint[0] * 255.0).round() as u8,
                (tint[1] * 255.0).round() as u8,
                (tint[2] * 255.0).round() as u8,
                255,
            ]),
        ),
    };
    for p in out.pixels_mut() {
        p[3] = 255;
    }
    // a plain colour needs no more than a few pixels: most of a model's parts, and the bytes of a map (MAP-105)
    let (mut lo, mut hi, mut sum) = ([255u8; 3], [0u8; 3], [0u64; 3]);
    for p in out.pixels() {
        for c in 0..3 {
            lo[c] = lo[c].min(p[c]);
            hi[c] = hi[c].max(p[c]);
            sum[c] += p[c] as u64;
        }
    }
    if (0..3).all(|c| hi[c] - lo[c] <= PLAIN_RANGE) && out.width() > 4 {
        let n = (out.width() * out.height()) as u64;
        let mean = |c: usize| (sum[c] / n) as u8;
        out = image::RgbaImage::from_pixel(4, 4, image::Rgba([mean(0), mean(1), mean(2), 255]));
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(out)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("png: {e}"))?;
    Ok(png)
}

/// Converts a `.glb`.
pub fn convert(bytes: &[u8], opts: &GlbOptions) -> Result<GlbModel, String> {
    if bytes.len() < 20 || &bytes[0..4] != b"glTF" {
        return Err("not a glTF binary".into());
    }
    let u = |o: usize| {
        u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]) as usize
    };
    let jlen = u(12);
    let jend = 20usize
        .checked_add(jlen)
        .filter(|e| *e + 8 <= bytes.len())
        .ok_or("a truncated glTF binary")?;
    let json: Value =
        serde_json::from_slice(&bytes[20..jend]).map_err(|e| format!("glTF JSON: {e}"))?;
    let blen = u(jend);
    let bin = bytes
        .get(jend + 8..jend + 8 + blen)
        .ok_or("a truncated glTF binary")?;
    let mut g = Gltf {
        json,
        bin,
        views: HashMap::new(),
    };
    let scene = g.json["scene"].as_u64().unwrap_or(0) as usize;
    let roots = g.json["scenes"][scene]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let orient = parse_orient(&opts.orient)?;
    let mut by_material = BTreeMap::new();
    for r in roots {
        if let Some(r) = r.as_u64() {
            walk(&mut g, r as usize, &orient, &mut by_material)?;
        }
    }
    let words: Vec<&str> = opts
        .leave_out
        .split(',')
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .collect();
    if !words.is_empty() {
        by_material.retain(|m: &i64, _| {
            let name = usize::try_from(*m)
                .ok()
                .and_then(|m| g.json["materials"][m]["name"].as_str())
                .unwrap_or("");
            !words.iter().any(|w| name.contains(w))
        });
        if by_material.is_empty() {
            return Err(format!(
                "leaving out {} leaves nothing of the model",
                opts.leave_out
            ));
        }
    }
    let source_tris: usize = by_material
        .values()
        .map(|e: &Gathered| e.tris.len() / 3)
        .sum();
    if source_tris == 0 {
        return Err("the model has no triangles".into());
    }
    if opts.textures_only {
        for e in by_material.values_mut() {
            e.tris.truncate(3);
        }
    } else if opts.proxy == Proxy::None {
        remove_hidden(&mut by_material);
    }
    let kept_tris: usize = by_material.values().map(|e| e.tris.len() / 3).sum();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for e in by_material.values() {
        for v in &e.verts {
            for c in 0..3 {
                lo[c] = lo[c].min(v.p[c]);
                hi[c] = hi[c].max(v.p[c]);
            }
        }
    }
    let centre = [(lo[0] + hi[0]) / 2.0, lo[1], (lo[2] + hi[2]) / 2.0];
    if opts.proxy == Proxy::Rebake {
        let (part, hull_boxes) = rebake(&mut g, &by_material, opts, centre)?;
        let points: Vec<[f32; 2]> = part.mesh.positions.iter().map(|v| [v.x, v.z]).collect();
        return Ok(GlbModel {
            hull_boxes,
            footprint: footprint(&points),
            parts: vec![part],
            size: [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
            source_tris,
        });
    }
    if opts.proxy != Proxy::None {
        let part = bake_proxy(
            &mut g,
            &by_material,
            opts,
            centre,
            [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
        )?;
        let points: Vec<[f32; 2]> = part.mesh.positions.iter().map(|v| [v.x, v.z]).collect();
        return Ok(GlbModel {
            hull_boxes: Vec::new(),
            footprint: footprint(&points),
            parts: vec![part],
            size: [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
            source_tris,
        });
    }
    let model_size = (hi[0] - lo[0]).hypot(hi[1] - lo[1]).hypot(hi[2] - lo[2]);
    let mut parts = Vec::new();
    for (mat, e) in &by_material {
        let mut welded = weld(&drop_specks(e, model_size));
        // every part keeps a share by its own detail, with a floor so that a small part keeps its shape
        let share =
            ((e.tris.len() / 3) as f64 / kept_tris.max(1) as f64 * opts.max_tris as f64) as usize;
        let errors: &[f32] = if opts.strict { &ERRORS_STRICT } else { &ERRORS };
        let mut tris = simplify(&welded, share.max(8), errors);
        // a part stuck at its seams, with `strict`: one even colour needs no seams and can reach its budget, and no
        // texture can smear (DC-DEFAULT: MAP-106)
        let mut flat = None;
        if opts.strict && tris.len() / 3 > share.max(8) * 2 {
            let colour = mean_colour(&mut g, *mat, &welded)?;
            let mut open = drop_specks(e, model_size);
            for v in &mut open.verts {
                v.uv = [0.0, 0.0];
            }
            welded = weld(&open);
            tris = simplify(&welded, share.max(8), errors);
            // the coarse pass ignores seams, and now there are none to break
            if tris.len() / 3 > share.max(8) {
                let mut dest = vec![0u32; tris.len()];
                let n = meshopt_rs::simplify::simplify_sloppy(
                    &mut dest,
                    &tris,
                    &welded.verts,
                    share.max(8) * 3,
                );
                if n >= 3 {
                    tris = dest[..n].to_vec();
                }
            }
            flat = Some(colour);
        }
        // an open sheet marked double sided is seen from behind too, and the game draws one side only
        let sheet = g.json["materials"][(*mat).max(0) as usize]["doubleSided"]
            .as_bool()
            .unwrap_or(false)
            && open_edge_share(&welded, &tris) > OPEN_SHEET;
        if sheet {
            let back: Vec<u32> = tris
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|t| [t[0], t[2], t[1]])
                .collect();
            tris.extend(back);
        }
        if tris.is_empty() {
            continue;
        }
        let normals = corner_normals(&welded, &tris);
        // only what is still used, moved to game space: y up, base on the origin, z mirrored for its other handedness
        let mut map: HashMap<u32, usize> = HashMap::new();
        let mut mesh = ObjMesh::default();
        let mut index = |i: u32, mesh: &mut ObjMesh| -> usize {
            *map.entry(i).or_insert_with(|| {
                let v = welded.verts[i as usize];
                mesh.positions.push(V3::new(
                    v.p[0] - centre[0],
                    v.p[1] - centre[1],
                    -(v.p[2] - centre[2]),
                ));
                mesh.uvs.push([v.uv[0], 1.0 - v.uv[1]]);
                mesh.positions.len() - 1
            })
        };
        for (k, t) in tris.as_chunks::<3>().0.iter().enumerate() {
            let (a, b, c) = (
                index(t[0], &mut mesh),
                index(t[1], &mut mesh),
                index(t[2], &mut mesh),
            );
            let base = mesh.normals.len();
            for n in [normals[k * 3], normals[k * 3 + 2], normals[k * 3 + 1]] {
                mesh.normals.push(V3::new(n[0], n[1], -n[2]));
            }
            mesh.faces.push(vec![a, c, b]);
            mesh.face_uvs.push(vec![a, c, b]);
            mesh.face_normals.push(vec![base, base + 1, base + 2]);
        }
        parts.push(GlbPart {
            tris: mesh.faces.len(),
            png: match flat {
                Some(c) => plain_png(c)?,
                None => base_colour_image(&mut g, *mat, opts.tex, opts.relief)?,
            },
            mesh,
        });
    }
    let outline: Vec<[f32; 2]> = parts
        .iter()
        .flat_map(|p: &GlbPart| p.mesh.positions.iter().map(|v| [v.x, v.z]))
        .collect();
    Ok(GlbModel {
        hull_boxes: Vec::new(),
        footprint: footprint(&outline),
        parts,
        size: [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
        source_tris,
    })
}

/// A material's colour as the part shows it on average, each triangle counting by its area.
fn mean_colour(g: &mut Gltf, material: i64, e: &Gathered) -> Result<[u8; 3], String> {
    let s = sampler(g, material)?;
    let (mut sum, mut total) = ([0f64; 3], 0f64);
    for t in e.tris.as_chunks::<3>().0 {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| e.verts[i as usize]);
        let (e1, e2) = (
            [b.p[0] - a.p[0], b.p[1] - a.p[1], b.p[2] - a.p[2]],
            [c.p[0] - a.p[0], c.p[1] - a.p[1], c.p[2] - a.p[2]],
        );
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let area = ((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0) as f64;
        let colour = s.at(
            (a.uv[0] + b.uv[0] + c.uv[0]) / 3.0,
            (a.uv[1] + b.uv[1] + c.uv[1]) / 3.0,
            [1.0; 3],
        );
        for k in 0..3 {
            sum[k] += colour[k] as f64 * area;
        }
        total += area;
    }
    if total <= 0.0 {
        return Ok([128; 3]);
    }
    Ok([0, 1, 2].map(|k| (sum[k] / total).round().clamp(0.0, 255.0) as u8))
}

/// A 4 x 4 picture of one colour.
fn plain_png(c: [u8; 3]) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        4,
        4,
        image::Rgba([c[0], c[1], c[2], 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .map_err(|e| format!("png: {e}"))?;
    Ok(png)
}

/// Leaves out the separate pieces too small to see, bolts and rivets, which no simplification can reduce: each is an
/// island that keeps its shape to the last. `model_size` is the whole model's diagonal (DC-DEFAULT: MAP-106).
fn drop_specks(e: &Gathered, model_size: f32) -> Gathered {
    let key = |i: u32| {
        let p = e.verts[i as usize].p;
        [0, 1, 2].map(|c| (p[c] * 1e5).round() as i32)
    };
    fn find(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    let mut same: HashMap<[i32; 3], u32> = HashMap::new();
    let mut parent: Vec<u32> = (0..e.verts.len() as u32).collect();
    for t in e.tris.as_chunks::<3>().0 {
        for i in t {
            let first = *same.entry(key(*i)).or_insert(*i);
            let (a, b) = (find(&mut parent, *i), find(&mut parent, first));
            parent[a as usize] = b;
        }
        let (a, b) = (find(&mut parent, t[0]), find(&mut parent, t[1]));
        parent[a as usize] = b;
        let (a, b) = (find(&mut parent, t[1]), find(&mut parent, t[2]));
        parent[a as usize] = b;
    }
    let mut bounds: HashMap<u32, ([f32; 3], [f32; 3])> = HashMap::new();
    for t in e.tris.as_chunks::<3>().0 {
        let root = find(&mut parent, t[0]);
        let b = bounds.entry(root).or_insert(([f32::MAX; 3], [f32::MIN; 3]));
        for i in t {
            let p = e.verts[*i as usize].p;
            for (c, v) in p.iter().enumerate() {
                b.0[c] = b.0[c].min(*v);
                b.1[c] = b.1[c].max(*v);
            }
        }
    }
    let least = model_size * SPECK;
    let big = |b: &([f32; 3], [f32; 3])| {
        (b.1[0] - b.0[0])
            .hypot(b.1[1] - b.0[1])
            .hypot(b.1[2] - b.0[2])
            >= least
    };
    let mut tris = Vec::with_capacity(e.tris.len());
    for t in e.tris.as_chunks::<3>().0 {
        let root = find(&mut parent, t[0]);
        if bounds.get(&root).is_some_and(big) {
            tris.extend_from_slice(t);
        }
    }
    Gathered {
        verts: e.verts.clone(),
        // a material that is nothing but specks keeps them: the part would be gone
        tris: if tris.is_empty() {
            e.tris.clone()
        } else {
            tris
        },
    }
}

/// A piece counts as a speck below this share of the model's diagonal.
const SPECK: f32 = 0.02;
/// The cosine of the angle up to which neighbouring faces share a smooth normal.
const SMOOTH_COS: f32 = 0.74;

/// Normals per triangle corner, from the simplified faces: smooth across shallow angles, hard across sharp ones.
fn corner_normals(g: &Gathered, tris: &[u32]) -> Vec<[f32; 3]> {
    let key = |i: u32| {
        let p = g.verts[i as usize].p;
        [0, 1, 2].map(|c| (p[c] * 1e4).round() as i32)
    };
    let face: Vec<[f32; 3]> = tris
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| g.verts[i as usize].p);
            let (e1, e2) = (
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ]
        })
        .collect();
    let unit = |n: [f32; 3]| {
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
        [n[0] / l, n[1] / l, n[2] / l]
    };
    let mut around: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for (k, t) in tris.as_chunks::<3>().0.iter().enumerate() {
        for i in t {
            around.entry(key(*i)).or_default().push(k);
        }
    }
    let mut out = Vec::with_capacity(tris.len());
    for (k, t) in tris.as_chunks::<3>().0.iter().enumerate() {
        let me = unit(face[k]);
        for i in t {
            let mut sum = [0f32; 3];
            for other in &around[&key(*i)] {
                let n = face[*other];
                let u = unit(n);
                if me[0] * u[0] + me[1] * u[1] + me[2] * u[2] >= SMOOTH_COS {
                    for c in 0..3 {
                        sum[c] += n[c];
                    }
                }
            }
            out.push(unit(sum));
        }
    }
    out
}

/// The share of a triangle list's edges that belong to one triangle only, corners compared by position.
fn open_edge_share(g: &Gathered, tris: &[u32]) -> f32 {
    let key = |i: u32| {
        let p = g.verts[i as usize].p;
        [0, 1, 2].map(|c| (p[c] * 1e4).round() as i32)
    };
    let mut edges: HashMap<([i32; 3], [i32; 3]), u32> = HashMap::new();
    for t in tris.as_chunks::<3>().0 {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (a, b) = (key(a), key(b));
            *edges
                .entry(if a < b { (a, b) } else { (b, a) })
                .or_insert(0) += 1;
        }
    }
    if edges.is_empty() {
        return 0.0;
    }
    edges.values().filter(|n| **n == 1).count() as f32 / edges.len() as f32
}

/// The open-edge share above which a double-sided part counts as a sheet.
const OPEN_SHEET: f32 = 0.05;
/// Corners of a [`Proxy::Round`] prism.
const ROUND_CORNERS: usize = 12;
/// Pixels rendered per texture pixel along each side of a [`bake_proxy`] face.
const BAKE_SAMPLES: usize = 3;
/// How strongly a model's normal maps show in a baked texture.
const BAKED_RELIEF: f32 = 0.55;
/// The largest source texture side a bake samples from.
const BAKE_SOURCE: u32 = 1024;

/// A material's picture and tint, ready to sample (texture coordinates repeat, as glTF's default says).
struct Sampler {
    image: Option<image::RgbaImage>,
    tint: [f32; 3],
    /// The material's normal map, drawn into the colour as surface detail: this renderer has none of its own.
    normal: Option<image::RgbaImage>,
}

impl Sampler {
    fn at(&self, u: f32, v: f32, c: [f32; 3]) -> [f32; 3] {
        let base = match &self.image {
            Some(i) => {
                let (w, h) = (i.width() as f32, i.height() as f32);
                let x = ((u - u.floor()) * w) as u32;
                let y = ((v - v.floor()) * h) as u32;
                let p = i.get_pixel(x.min(i.width() - 1), y.min(i.height() - 1));
                [p[0] as f32, p[1] as f32, p[2] as f32]
            }
            None => [255.0; 3],
        };
        let relief = match &self.normal {
            Some(i) => {
                let (w, h) = (i.width() as f32, i.height() as f32);
                let x = ((u - u.floor()) * w) as u32;
                let y = ((v - v.floor()) * h) as u32;
                let p = i.get_pixel(x.min(i.width() - 1), y.min(i.height() - 1));
                let (nx, ny) = (p[0] as f32 / 127.5 - 1.0, p[1] as f32 / 127.5 - 1.0);
                (1.0 + BAKED_RELIEF * (0.6 * nx + 0.8 * ny)).clamp(0.55, 1.45)
            }
            None => 1.0,
        };
        [0, 1, 2].map(|k| base[k] * self.tint[k] * c[k] * relief)
    }
}

/// One flat face of a proxy: its corners on the solid and the rectangle of the atlas it gets.
struct BakeFace {
    corners: Vec<V3>,
    /// The face's plane axes; a point's place on the face is its offset from `origin` along them.
    origin: V3,
    u: V3,
    v: V3,
    n: V3,
    /// Its size along `u` and `v`.
    size: [f32; 2],
    /// x, y, width, height in atlas pixels.
    rect: [u32; 4],
    /// Per-corner normals, for a round prism's sides.
    normals: Option<Vec<V3>>,
}

/// Replaces a model by a simple solid: its faces get the model's look by looking at the full-detail source straight
/// on, nearest surface first, so the picture keeps every label and no simplification can smear it (DC-DEFAULT:
/// MAP-106). What it shows is also exactly what the hidden core blocks.
fn bake_proxy(
    g: &mut Gltf,
    by_material: &BTreeMap<i64, Gathered>,
    opts: &GlbOptions,
    centre: [f32; 3],
    size: [f32; 3],
) -> Result<GlbPart, String> {
    let to_game = |p: [f32; 3]| V3::new(p[0] - centre[0], p[1] - centre[1], -(p[2] - centre[2]));
    let height = size[1].max(1e-4);
    let corners2: Vec<[f32; 2]> = if opts.proxy == Proxy::Box {
        let (w, d) = (size[0] / 2.0, size[2] / 2.0);
        vec![[-w, -d], [w, -d], [w, d], [-w, d]]
    } else {
        let all: Vec<[f32; 2]> = by_material
            .values()
            .flat_map(|e| e.verts.iter())
            .map(|v| {
                let q = to_game(v.p);
                [q.x, q.z]
            })
            .collect();
        outline(&all, ROUND_CORNERS)
    };
    if corners2.len() < 3 {
        return Err("the model is flat from above: no proxy".into());
    }
    let tex = opts.tex.clamp(16, 1024);
    let n = corners2.len();
    let (mut x0, mut x1, mut z0, mut z1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for c in &corners2 {
        x0 = x0.min(c[0]);
        x1 = x1.max(c[0]);
        z0 = z0.min(c[1]);
        z1 = z1.max(c[1]);
    }
    let (span_x, span_z) = ((x1 - x0).max(1e-4), (z1 - z0).max(1e-4));
    // the atlas: the sides along the top, in proportion to their widths; the two caps below them
    let lens: Vec<f32> = (0..n)
        .map(|i| {
            let (a, b) = (corners2[i], corners2[(i + 1) % n]);
            (b[0] - a[0]).hypot(b[1] - a[1])
        })
        .collect();
    let around: f32 = lens.iter().sum();
    let cap_h = ((span_z / (height + span_z)) * tex as f32)
        .clamp(tex as f32 / 4.0, tex as f32 / 2.0) as u32;
    let side_h = tex - cap_h;
    let mut faces = Vec::with_capacity(n + 2);
    let mut at = 0.0f32;
    let up = V3::new(0.0, 1.0, 0.0);
    let ring = |i: usize, y: f32| V3::new(corners2[i % n][0], y, corners2[i % n][1]);
    let cx = corners2.iter().map(|c| c[0]).sum::<f32>() / n as f32;
    let cz = corners2.iter().map(|c| c[1]).sum::<f32>() / n as f32;
    let radial = |i: usize| {
        let c = corners2[i % n];
        V3::new(c[0] - cx, 0.0, c[1] - cz)
            .normalized()
            .unwrap_or(V3::new(1.0, 0.0, 0.0))
    };
    for (i, len) in lens.iter().copied().enumerate() {
        let (a, b) = (ring(i, height), ring(i + 1, height));
        let along = (b - a) * (1.0 / len.max(1e-6));
        // counter-clockwise in x / z: the outside is to the right of each edge
        let normal = V3::new(along.z, 0.0, -along.x);
        let x = at.round() as u32;
        at += len / around * tex as f32;
        let w = (at.round() as u32).saturating_sub(x).max(1);
        faces.push(BakeFace {
            corners: vec![
                ring(i, 0.0),
                ring(i + 1, 0.0),
                ring(i + 1, height),
                ring(i, height),
            ],
            origin: a,
            u: along,
            v: V3::new(0.0, -1.0, 0.0),
            n: normal,
            size: [len.max(1e-4), height],
            rect: [x, 0, w.min(tex - x.min(tex - 1)), side_h],
            normals: (opts.proxy == Proxy::Round)
                .then(|| vec![radial(i), radial(i + 1), radial(i + 1), radial(i)]),
        });
    }
    let half = tex / 2;
    for (top, x) in [(true, 0u32), (false, half)] {
        let y = if top { height } else { 0.0 };
        faces.push(BakeFace {
            corners: (0..n).map(|i| ring(i, y)).collect(),
            origin: V3::new(x0, y, z0),
            u: V3::new(1.0, 0.0, 0.0),
            v: V3::new(0.0, 0.0, 1.0),
            n: if top { up } else { up * -1.0 },
            size: [span_x, span_z],
            rect: [x, side_h, half, cap_h],
            normals: None,
        });
    }
    // the source: every triangle in game space, with its material's sampler
    let mut samplers: BTreeMap<i64, Sampler> = BTreeMap::new();
    for mat in by_material.keys() {
        samplers.insert(*mat, sampler(g, *mat)?);
    }
    let mut atlas = image::RgbaImage::from_pixel(tex, tex, image::Rgba([128, 128, 128, 255]));
    let mut bare = Vec::new();
    for face in &faces {
        if !paint_face(face, by_material, &samplers, &to_game, &mut atlas) {
            bare.push(face.rect);
        }
    }
    // a face that sees nothing of the model (the edge of a flat one) takes the model's mean colour
    if !bare.is_empty() {
        let is_bare = |x: u32, y: u32| {
            bare.iter()
                .any(|r: &[u32; 4]| x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3])
        };
        let (mut sum, mut count) = ([0u64; 3], 0u64);
        for (x, y, p) in atlas.enumerate_pixels() {
            if !is_bare(x, y) {
                for k in 0..3 {
                    sum[k] += p[k] as u64;
                }
                count += 1;
            }
        }
        if count > 0 {
            let mean = [0, 1, 2].map(|k| (sum[k] / count) as u8);
            for (x, y, p) in atlas.enumerate_pixels_mut() {
                if is_bare(x, y) {
                    *p = image::Rgba([mean[0], mean[1], mean[2], 255]);
                }
            }
        }
    }
    // the mesh
    let mut mesh = ObjMesh::default();
    for face in &faces {
        let base = mesh.positions.len();
        for (k, c) in face.corners.iter().enumerate() {
            let d = *c - face.origin;
            // half a pixel in from the edge, so that a neighbour's colour cannot bleed in
            let inset = |p: f32, at: u32, len: u32| {
                let px = at as f32 + 0.5 + p.clamp(0.0, 1.0) * (len as f32 - 1.0);
                px / tex as f32
            };
            let (uu, vv) = (
                inset(d.dot(face.u) / face.size[0], face.rect[0], face.rect[2]),
                inset(d.dot(face.v) / face.size[1], face.rect[1], face.rect[3]),
            );
            mesh.positions.push(*c);
            mesh.uvs.push([uu, 1.0 - vv]);
            mesh.normals
                .push(face.normals.as_ref().map_or(face.n, |r| r[k]));
        }
        let m = face.corners.len();
        let mut order: Vec<usize> = (0..m).map(|k| base + k).collect();
        // wound so that the plane's normal comes out on the outside, as a converted model's are
        let (e1, e2) = (
            face.corners[1] - face.corners[0],
            face.corners[2] - face.corners[0],
        );
        let cross = V3::new(
            e1.y * e2.z - e1.z * e2.y,
            e1.z * e2.x - e1.x * e2.z,
            e1.x * e2.y - e1.y * e2.x,
        );
        if cross.dot(face.n) < 0.0 {
            order.reverse();
        }
        mesh.faces.push(order.clone());
        mesh.face_uvs.push(order.clone());
        mesh.face_normals.push(order);
    }
    let tris = mesh.faces.iter().map(|f| f.len() - 2).sum();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(atlas)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("png: {e}"))?;
    Ok(GlbPart { mesh, png, tris })
}

/// Source triangles in a grid, to find the surface under a point of the simplified model.
struct Source {
    tris: Vec<SourceTri>,
    grid: Vec<Vec<u32>>,
    lo: [f32; 3],
    hi: [f32; 3],
    cell: f32,
    dims: [usize; 3],
}

struct SourceTri {
    p: [[f32; 3]; 3],
    uv: [[f32; 2]; 3],
    c: [[f32; 3]; 3],
    n: [f32; 3],
    material: i64,
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn unit3(a: [f32; 3]) -> [f32; 3] {
    let l = dot3(a, a).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}

impl Source {
    fn new(by_material: &BTreeMap<i64, Gathered>) -> Source {
        let mut tris = Vec::new();
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for (mat, e) in by_material {
            for t in e.tris.as_chunks::<3>().0 {
                let v = [t[0], t[1], t[2]].map(|i| e.verts[i as usize]);
                for q in &v {
                    for c in 0..3 {
                        lo[c] = lo[c].min(q.p[c]);
                        hi[c] = hi[c].max(q.p[c]);
                    }
                }
                tris.push(SourceTri {
                    p: v.map(|q| q.p),
                    uv: v.map(|q| q.uv),
                    c: v.map(|q| q.c),
                    n: unit3(cross3(sub3(v[1].p, v[0].p), sub3(v[2].p, v[0].p))),
                    material: *mat,
                });
            }
        }
        let size = sub3(hi, lo);
        let cell = (size[0].max(size[1]).max(size[2]) / SOURCE_GRID as f32).max(1e-6);
        let dims = [0, 1, 2].map(|c| ((size[c] / cell) as usize + 1).min(SOURCE_GRID));
        let mut grid = vec![Vec::new(); dims[0] * dims[1] * dims[2]];
        let at = |v: f32, c: usize| {
            (((v - lo[c]) / cell) as isize).clamp(0, dims[c] as isize - 1) as usize
        };
        for (i, t) in tris.iter().enumerate() {
            let range = |c: usize| {
                let (a, b, d) = (at(t.p[0][c], c), at(t.p[1][c], c), at(t.p[2][c], c));
                a.min(b).min(d)..=a.max(b).max(d)
            };
            for z in range(2) {
                for y in range(1) {
                    for x in range(0) {
                        grid[x + dims[0] * (y + dims[1] * z)].push(i as u32);
                    }
                }
            }
        }
        Source {
            tris,
            grid,
            lo,
            hi,
            cell,
            dims,
        }
    }

    /// The nearest source triangle along a ray of length `len`, with its distance and barycentric weights. `stamp`
    /// and `ray` keep one triangle from being tested twice; each thread has its own.
    fn hit(
        &self,
        o: [f32; 3],
        d: [f32; 3],
        len: f32,
        stamp: &mut [u32],
        ray: &mut u32,
    ) -> Option<(usize, f32, [f32; 3])> {
        *ray = ray.wrapping_add(1);
        let mut best: Option<(usize, f32, [f32; 3])> = None;
        let step = self.cell * 0.5;
        let mut t = 0.0;
        while t <= len + step {
            if let Some(b) = &best {
                if t > b.1 + self.cell * 2.0 {
                    break;
                }
            }
            let p = [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
            t += step;
            if (0..3).any(|c| p[c] < self.lo[c] - self.cell || p[c] > self.hi[c] + self.cell) {
                continue;
            }
            let at = |c: usize| {
                (((p[c] - self.lo[c]) / self.cell) as isize).clamp(0, self.dims[c] as isize - 1)
                    as usize
            };
            for &i in &self.grid[at(0) + self.dims[0] * (at(1) + self.dims[1] * at(2))] {
                if stamp[i as usize] == *ray {
                    continue;
                }
                stamp[i as usize] = *ray;
                let tri = &self.tris[i as usize];
                let (e1, e2) = (sub3(tri.p[1], tri.p[0]), sub3(tri.p[2], tri.p[0]));
                let h = cross3(d, e2);
                let det = dot3(e1, h);
                if det.abs() < 1e-14 {
                    continue;
                }
                let s = sub3(o, tri.p[0]);
                let u = dot3(s, h) / det;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let q = cross3(s, e1);
                let v = dot3(d, q) / det;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let at_t = dot3(e2, q) / det;
                if at_t > 1e-6 && at_t <= len && best.as_ref().is_none_or(|b| at_t < b.1) {
                    best = Some((i as usize, at_t, [1.0 - u - v, u, v]));
                }
            }
        }
        best
    }
}

/// Cells per axis of [`Source`].
const SOURCE_GRID: usize = 64;
/// Directions around a surface for [`shade_at`]'s contact shading.
const AO_RAYS: [[f32; 3]; 12] = [
    [0.9, 0.44, 0.0],
    [-0.9, 0.44, 0.0],
    [0.0, 0.44, 0.9],
    [0.0, 0.44, -0.9],
    [0.5, 0.7, 0.5],
    [-0.5, 0.7, 0.5],
    [0.5, 0.7, -0.5],
    [-0.5, 0.7, -0.5],
    [0.64, 0.2, 0.64],
    [-0.64, 0.2, 0.64],
    [0.64, 0.2, -0.64],
    [-0.64, 0.2, -0.64],
];

/// One group of neighbouring faces of the simplified model, facing about the same way, flat in the new texture.
struct Chart {
    tris: Vec<usize>,
    origin: [f32; 3],
    u: [f32; 3],
    v: [f32; 3],
    size: [f32; 2],
    rect: [u32; 4],
}

/// The source's picture at a point of one of its triangles.
fn picture_at(
    src: &Source,
    samplers: &BTreeMap<i64, Sampler>,
    tri: usize,
    w: [f32; 3],
) -> [f32; 3] {
    let t = &src.tris[tri];
    let mix2 = |k: usize| w[0] * t.uv[0][k] + w[1] * t.uv[1][k] + w[2] * t.uv[2][k];
    let mix3 = |k: usize| w[0] * t.c[0][k] + w[1] * t.c[1][k] + w[2] * t.c[2][k];
    samplers.get(&t.material).map_or([128.0; 3], |s| {
        s.at(mix2(0), mix2(1), [mix3(0), mix3(1), mix3(2)])
    })
}

/// How a point of the source is lit by its own shape: the fine detail light from above would show, and how shut in it
/// sits. Those two this renderer's vertex light on a simplified model could never give.
fn shade_at(
    src: &Source,
    tri: usize,
    w: [f32; 3],
    low_normal: [f32; 3],
    reach: f32,
    stamp: &mut [u32],
    ray: &mut u32,
) -> f32 {
    let t = &src.tris[tri];
    // the same surface's normal as the side it faces: seen from outside
    let n = if dot3(t.n, low_normal) < 0.0 {
        [-t.n[0], -t.n[1], -t.n[2]]
    } else {
        t.n
    };
    let light = unit3([0.3, 0.9, 0.25]);
    let wrap = |m: [f32; 3]| 0.6 + 0.4 * dot3(m, light).max(-0.5);
    let relief = (wrap(n) / wrap(low_normal).max(0.2)).clamp(0.6, 1.25);
    let p = [0, 1, 2].map(|c| w[0] * t.p[0][c] + w[1] * t.p[1][c] + w[2] * t.p[2][c]);
    let start = [0, 1, 2].map(|c| p[c] + n[c] * reach * 0.02);
    // a frame around the normal for the fixed directions
    let side = unit3(cross3(
        if n[1].abs() < 0.9 {
            [0.0, 1.0, 0.0]
        } else {
            [1.0, 0.0, 0.0]
        },
        n,
    ));
    let up = cross3(n, side);
    let mut shut = 0;
    for a in AO_RAYS {
        let d = [0, 1, 2].map(|c| side[c] * a[0] + n[c] * a[1] + up[c] * a[2]);
        if src.hit(start, d, reach, stamp, ray).is_some() {
            shut += 1;
        }
    }
    relief * (1.0 - 0.65 * shut as f32 / AO_RAYS.len() as f32)
}

/// A model on a grid: what the outside can reach is empty and everything else is solid, so any gap in a tangle of CAD
/// parts is sealed and an inside stays inside (DC-DEFAULT: MAP-109).
struct Voxels {
    dims: [usize; 3],
    cell: f32,
    lo: [f32; 3],
    solid: Vec<bool>,
}

impl Voxels {
    fn of(src: &Source, longest: usize) -> Voxels {
        let size = sub3(src.hi, src.lo);
        let cell = (size[0].max(size[1]).max(size[2]) / longest as f32).max(1e-6);
        // two empty layers all round, so that the flood reaches everywhere outside
        let lo = [0, 1, 2].map(|c| src.lo[c] - cell * 2.0);
        let dims = [0, 1, 2].map(|c| (size[c] / cell).ceil() as usize + 4);
        let index = |x: usize, y: usize, z: usize| x + dims[0] * (y + dims[1] * z);
        let mut wall = vec![false; dims[0] * dims[1] * dims[2]];
        for t in &src.tris {
            let span = |a: [f32; 3], b: [f32; 3]| {
                let d = sub3(a, b);
                dot3(d, d).sqrt()
            };
            let longest = span(t.p[0], t.p[1])
                .max(span(t.p[1], t.p[2]))
                .max(span(t.p[2], t.p[0]));
            let steps = (longest / (cell * 0.5)).ceil().max(1.0) as usize;
            for i in 0..=steps {
                for j in 0..=steps - i {
                    let (a, b) = (i as f32 / steps as f32, j as f32 / steps as f32);
                    let at = |c: usize| {
                        let v =
                            t.p[0][c] + (t.p[1][c] - t.p[0][c]) * a + (t.p[2][c] - t.p[0][c]) * b;
                        (((v - lo[c]) / cell) as isize).clamp(0, dims[c] as isize - 1) as usize
                    };
                    wall[index(at(0), at(1), at(2))] = true;
                }
            }
        }
        let mut outside = vec![false; wall.len()];
        let mut todo = vec![(0usize, 0usize, 0usize)];
        outside[0] = true;
        while let Some((x, y, z)) = todo.pop() {
            let mut go = |nx: usize, ny: usize, nz: usize| {
                let i = index(nx, ny, nz);
                if !outside[i] && !wall[i] {
                    outside[i] = true;
                    todo.push((nx, ny, nz));
                }
            };
            if x > 0 {
                go(x - 1, y, z);
            }
            if y > 0 {
                go(x, y - 1, z);
            }
            if z > 0 {
                go(x, y, z - 1);
            }
            if x + 1 < dims[0] {
                go(x + 1, y, z);
            }
            if y + 1 < dims[1] {
                go(x, y + 1, z);
            }
            if z + 1 < dims[2] {
                go(x, y, z + 1);
            }
        }
        Voxels {
            dims,
            cell,
            lo,
            solid: outside.iter().map(|o| !o).collect(),
        }
    }

    fn at(&self, x: isize, y: isize, z: isize) -> bool {
        let d = self.dims;
        x >= 0
            && y >= 0
            && z >= 0
            && (x as usize) < d[0]
            && (y as usize) < d[1]
            && (z as usize) < d[2]
            && self.solid[x as usize + d[0] * (y as usize + d[1] * z as usize)]
    }

    /// The faces between solid and empty, joined at their corners: a closed surface.
    fn surface(&self) -> Gathered {
        let mut g = Gathered::default();
        let mut ids: HashMap<[i32; 3], u32> = HashMap::new();
        let (lo, cell) = (self.lo, self.cell);
        let mut corner = |p: [i32; 3], g: &mut Gathered| -> u32 {
            *ids.entry(p).or_insert_with(|| {
                g.verts.push(Vert {
                    p: [0, 1, 2].map(|c| lo[c] + p[c] as f32 * cell),
                    uv: [0.0, 0.0],
                    c: [1.0; 3],
                });
                g.verts.len() as u32 - 1
            })
        };
        // each side as its four corners, seen from outside
        const SIDES: [([i32; 3], [[i32; 3]; 4]); 6] = [
            ([1, 0, 0], [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]]),
            ([-1, 0, 0], [[0, 0, 1], [0, 1, 1], [0, 1, 0], [0, 0, 0]]),
            ([0, 1, 0], [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]]),
            ([0, -1, 0], [[1, 0, 0], [1, 0, 1], [0, 0, 1], [0, 0, 0]]),
            ([0, 0, 1], [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]]),
            ([0, 0, -1], [[0, 1, 0], [1, 1, 0], [1, 0, 0], [0, 0, 0]]),
        ];
        for z in 0..self.dims[2] as i32 {
            for y in 0..self.dims[1] as i32 {
                for x in 0..self.dims[0] as i32 {
                    if !self.at(x as isize, y as isize, z as isize) {
                        continue;
                    }
                    for (dir, quad) in SIDES {
                        if self.at(
                            (x + dir[0]) as isize,
                            (y + dir[1]) as isize,
                            (z + dir[2]) as isize,
                        ) {
                            continue;
                        }
                        let v = quad.map(|q| corner([x + q[0], y + q[1], z + q[2]], &mut g));
                        g.tris.extend([v[0], v[1], v[2], v[0], v[2], v[3]]);
                    }
                }
            }
        }
        g
    }

    /// Boxes over the solid cells at a coarser grid, as few as the shape allows, in game space around the model's base.
    fn hull_boxes(&self, centre: [f32; 3]) -> Vec<[f32; 6]> {
        // a mostly solid thing (a crate, a drum, a bin) or a small one has nothing to shoot through: its outline serves, one simple shape
        // instead of a staircase of boxes
        let solid = self.solid.iter().filter(|c| **c).count() as f32;
        let d = self.dims.map(|n| n.saturating_sub(4).max(1));
        let longest = d.iter().max().copied().unwrap_or(1) as f32 * self.cell;
        // the columns the model stands on: a prism over them would fill the air above a low part, a locker's bench
        let mut columns = 0usize;
        for z in 0..self.dims[2] {
            for x in 0..self.dims[0] {
                if (0..self.dims[1]).any(|y| self.at(x as isize, y as isize, z as isize)) {
                    columns += 1;
                }
            }
        }
        let fills_its_prism = solid / (columns * d[1]).max(1) as f32 >= HULL_PRISM;
        // a low thing has no air above it to speak of
        let low = d[1] as f32 * self.cell < HULL_LOW;
        if longest < HULL_SMALL
            || low
            || (solid / (d[0] * d[1] * d[2]) as f32 >= HULL_OPEN && fills_its_prism)
        {
            return Vec::new();
        }
        let mut factor = (self.dims.iter().max().copied().unwrap_or(1) / HULL_CELLS).max(1);
        loop {
            let boxes = self.boxes_at(factor, centre);
            if boxes.len() <= HULL_BOXES {
                return boxes;
            }
            factor += factor.div_ceil(3);
        }
    }

    fn boxes_at(&self, factor: usize, centre: [f32; 3]) -> Vec<[f32; 6]> {
        let d = self.dims.map(|n| n.div_ceil(factor));
        let index = |x: usize, y: usize, z: usize| x + d[0] * (y + d[1] * z);
        // a coarse cell is solid when any real part of it is: a thin bar across two cells stays, a gap wider than a
        // cell stays open
        let mut filled = vec![false; d[0] * d[1] * d[2]];
        for z in 0..d[2] {
            for y in 0..d[1] {
                for x in 0..d[0] {
                    let mut n = 0;
                    for fz in 0..factor {
                        for fy in 0..factor {
                            for fx in 0..factor {
                                let at = |v: usize, f: usize| (v * factor + f) as isize;
                                if self.at(at(x, fx), at(y, fy), at(z, fz)) {
                                    n += 1;
                                }
                            }
                        }
                    }
                    filled[index(x, y, z)] = n * HULL_FILL >= factor * factor * factor;
                }
            }
        }
        let mut out = Vec::new();
        for z in 0..d[2] {
            for y in 0..d[1] {
                for x in 0..d[0] {
                    if !filled[index(x, y, z)] {
                        continue;
                    }
                    let (mut x1, mut z1, mut y1) = (x + 1, z + 1, y + 1);
                    while x1 < d[0] && filled[index(x1, y, z)] {
                        x1 += 1;
                    }
                    while z1 < d[2] && (x..x1).all(|a| filled[index(a, y, z1)]) {
                        z1 += 1;
                    }
                    while y1 < d[1] && (x..x1).all(|a| (z..z1).all(|b| filled[index(a, y1, b)])) {
                        y1 += 1;
                    }
                    for b in z..z1 {
                        for c in y..y1 {
                            for a in x..x1 {
                                filled[index(a, c, b)] = false;
                            }
                        }
                    }
                    let edge = |v: usize, c: usize| {
                        self.lo[c] + (v * factor) as f32 * self.cell - centre[c]
                    };
                    // z is mirrored in game space, so its two ends swap
                    out.push([
                        edge(x, 0),
                        edge(y, 1).max(0.0),
                        -edge(z1, 2),
                        edge(x1, 0),
                        edge(y1, 1),
                        -edge(z, 2),
                    ]);
                }
            }
        }
        out
    }
}

/// Cells along the longest side of the shell, and of the coarser grid its collision boxes come from.
const SHELL_VOXELS: usize = 160;
/// The same for a model given many triangles: its thin parts are worth the finer grid.
const SHELL_VOXELS_FINE: usize = 256;
/// From how many triangles a model gets the finer grid.
const SHELL_FINE_FROM: usize = 6000;
const HULL_CELLS: usize = 40;
/// The most collision boxes one model may cost.
const HULL_BOXES: usize = 80;
/// The share of its bounds above which a model counts as solid all through and needs no boxes.
const HULL_OPEN: f32 = 0.4;
/// Metres of height below which a prism serves whatever the model's shape.
const HULL_LOW: f32 = 0.5;
/// The share of the prism over its outline a model must fill for that prism to serve as its collision.
const HULL_PRISM: f32 = 0.6;
/// Metres below which a model is too small for a shot through it to matter.
const HULL_SMALL: f32 = 1.0;
/// A coarse cell counts as solid from one part in this many.
const HULL_FILL: usize = 16;
/// The relative errors a shell's simplification tries: it is sealed, so more error only rounds it off.
const ERRORS_SHELL: [f32; 8] = [0.002, 0.004, 0.008, 0.015, 0.025, 0.04, 0.07, 0.12];

/// Takes the stair steps out of a voxel surface without shrinking it (Taubin's two-step smoothing), then moves it in
/// half a cell, as the voxels around a surface stand proud of it.
fn smooth_shell(g: &mut Gathered, cell: f32) {
    let mut near: Vec<Vec<u32>> = vec![Vec::new(); g.verts.len()];
    for t in g.tris.as_chunks::<3>().0 {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if !near[a as usize].contains(&b) {
                near[a as usize].push(b);
            }
            if !near[b as usize].contains(&a) {
                near[b as usize].push(a);
            }
        }
    }
    for k in [0.5f32, -0.53, 0.5, -0.53, 0.5, -0.53] {
        let before: Vec<[f32; 3]> = g.verts.iter().map(|v| v.p).collect();
        for (i, v) in g.verts.iter_mut().enumerate() {
            let n = near[i].len().max(1) as f32;
            let mut mean = [0f32; 3];
            for j in &near[i] {
                for c in 0..3 {
                    mean[c] += before[*j as usize][c] / n;
                }
            }
            for c in 0..3 {
                v.p[c] = before[i][c] + (mean[c] - before[i][c]) * k;
            }
        }
    }
    let mut normals = vec![[0f32; 3]; g.verts.len()];
    for t in g.tris.as_chunks::<3>().0 {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| g.verts[i as usize].p);
        let n = cross3(sub3(b, a), sub3(c, a));
        for i in t {
            for k in 0..3 {
                normals[*i as usize][k] += n[k];
            }
        }
    }
    for (v, n) in g.verts.iter_mut().zip(&normals) {
        let u = unit3(*n);
        for (c, part) in u.iter().enumerate() {
            v.p[c] -= part * cell * 0.5;
        }
    }
}

/// Wraps a model in a sealed shell, simplifies that, unfolds it into one new texture and draws that from the full-detail
/// source, so nothing of the original's picture depends on its seams: a low-poly copy that keeps its labels, its small
/// parts as paint, and its contact shading (DC-DEFAULT: MAP-107, MAP-109). Also gives the boxes for its collision.
fn rebake(
    g: &mut Gltf,
    by_material: &BTreeMap<i64, Gathered>,
    opts: &GlbOptions,
    centre: [f32; 3],
) -> Result<(GlbPart, Vec<[f32; 6]>), String> {
    let mut samplers: BTreeMap<i64, Sampler> = BTreeMap::new();
    for mat in by_material.keys() {
        samplers.insert(*mat, sampler(g, *mat)?);
    }
    let src = Source::new(by_material);
    let diag = dot3(sub3(src.hi, src.lo), sub3(src.hi, src.lo))
        .sqrt()
        .max(1e-6);
    // the shape: a sealed shell around the source, however open or tangled its CAD parts are, which then simplifies
    // cleanly. The picture comes from the source afterwards, so no seam or hole of the original matters.
    let shell = Voxels::of(
        &src,
        if opts.max_tris >= SHELL_FINE_FROM {
            SHELL_VOXELS_FINE
        } else {
            SHELL_VOXELS
        },
    );
    let mut low = shell.surface();
    if low.tris.is_empty() {
        return Err("the model has no volume".into());
    }
    smooth_shell(&mut low, shell.cell);
    let target = opts.max_tris.max(8);
    let mut tris = simplify(&low, target, &ERRORS_SHELL);
    if tris.len() / 3 > target * 2 {
        let mut dest = vec![0u32; tris.len()];
        let n = meshopt_rs::simplify::simplify_sloppy(&mut dest, &tris, &low.verts, target * 3);
        if n >= 3 {
            tris = dest[..n].to_vec();
        }
    }
    let hull_boxes = shell.hull_boxes(centre);
    let count = tris.len() / 3;
    if count == 0 {
        return Err("nothing is left of the model".into());
    }
    let corner = |k: usize, c: usize| low.verts[tris[k * 3 + c] as usize].p;
    let normals: Vec<[f32; 3]> = (0..count)
        .map(|k| {
            unit3(cross3(
                sub3(corner(k, 1), corner(k, 0)),
                sub3(corner(k, 2), corner(k, 0)),
            ))
        })
        .collect();
    let area = |k: usize| {
        let c = cross3(
            sub3(corner(k, 1), corner(k, 0)),
            sub3(corner(k, 2), corner(k, 0)),
        );
        dot3(c, c).sqrt() / 2.0
    };
    // charts: grow from the largest face over shared edges while the faces turn less than 45 degrees from it
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for k in 0..count {
        for c in 0..3 {
            let (a, b) = (tris[k * 3 + c], tris[k * 3 + (c + 1) % 3]);
            by_edge.entry((a.min(b), a.max(b))).or_default().push(k);
        }
    }
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|a, b| area(*b).total_cmp(&area(*a)).then(a.cmp(b)));
    let mut taken = vec![false; count];
    let mut charts = Vec::new();
    for seed in order {
        if taken[seed] {
            continue;
        }
        taken[seed] = true;
        let mut members = vec![seed];
        let mut i = 0;
        while i < members.len() {
            let k = members[i];
            i += 1;
            for c in 0..3 {
                let (a, b) = (tris[k * 3 + c], tris[k * 3 + (c + 1) % 3]);
                for &other in &by_edge[&(a.min(b), a.max(b))] {
                    if !taken[other] && dot3(normals[other], normals[seed]) >= 0.7 {
                        taken[other] = true;
                        members.push(other);
                    }
                }
            }
        }
        let n = normals[seed];
        let u = unit3(cross3(
            if n[1].abs() < 0.9 {
                [0.0, 1.0, 0.0]
            } else {
                [1.0, 0.0, 0.0]
            },
            n,
        ));
        let v = cross3(n, u);
        let (mut a0, mut a1, mut b0, mut b1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &k in &members {
            for c in 0..3 {
                let p = corner(k, c);
                let (x, y) = (dot3(p, u), dot3(p, v));
                a0 = a0.min(x);
                a1 = a1.max(x);
                b0 = b0.min(y);
                b1 = b1.max(y);
            }
        }
        charts.push(Chart {
            tris: members,
            origin: [a0, b0, 0.0],
            u,
            v,
            size: [(a1 - a0).max(1e-5), (b1 - b0).max(1e-5)],
            rect: [0; 4],
        });
    }
    // shelves, the tallest first, with the scale shrunk until they fit
    let tex = opts.tex.clamp(16, 2048);
    const PAD: u32 = 3;
    let mut pixels_per_unit = {
        let total: f32 = charts.iter().map(|c| c.size[0] * c.size[1]).sum();
        (tex as f32 * tex as f32 * 0.8 / total.max(1e-9)).sqrt()
    };
    let mut sorted: Vec<usize> = (0..charts.len()).collect();
    sorted.sort_by(|a, b| {
        charts[*b].size[1]
            .total_cmp(&charts[*a].size[1])
            .then(a.cmp(b))
    });
    loop {
        let (mut x, mut y, mut row) = (PAD, PAD, 0u32);
        let mut fits = true;
        for &i in &sorted {
            let w = (charts[i].size[0] * pixels_per_unit).ceil() as u32 + 2;
            let h = (charts[i].size[1] * pixels_per_unit).ceil() as u32 + 2;
            if x + w + PAD > tex {
                x = PAD;
                y += row + PAD;
                row = 0;
            }
            if x + w + PAD > tex || y + h + PAD > tex {
                fits = false;
                break;
            }
            charts[i].rect = [x, y, w, h];
            x += w + PAD;
            row = row.max(h);
        }
        if fits {
            break;
        }
        pixels_per_unit *= 0.95;
    }
    // draw each chart: the surface nearest the outside along the low face's normal, a few samples a pixel
    let reach = shell.cell * 4.0;
    let ao_reach = diag * 0.18;
    let work = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(12);
    let mut done: Vec<(usize, Vec<[f32; 4]>)> = Vec::with_capacity(charts.len());
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..threads {
            handles.push(scope.spawn(|| {
                let mut stamp = vec![0u32; src.tris.len()];
                let mut ray = 0u32;
                let mut out = Vec::new();
                loop {
                    let i = work.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(ch) = charts.get(i) else { break };
                    let (w, h) = (ch.rect[2] as usize, ch.rect[3] as usize);
                    let mut px = vec![[0f32; 4]; w * h];
                    for &k in &ch.tris {
                        let q = [0, 1, 2].map(|c| {
                            let p = corner(k, c);
                            [
                                (dot3(p, ch.u) - ch.origin[0]) * pixels_per_unit + 1.0,
                                (dot3(p, ch.v) - ch.origin[1]) * pixels_per_unit + 1.0,
                            ]
                        });
                        let det = (q[1][0] - q[0][0]) * (q[2][1] - q[0][1])
                            - (q[2][0] - q[0][0]) * (q[1][1] - q[0][1]);
                        if det.abs() < 1e-6 {
                            continue;
                        }
                        let x0 = q[0][0].min(q[1][0]).min(q[2][0]).floor().max(0.0) as usize;
                        let x1 = (q[0][0].max(q[1][0]).max(q[2][0]).ceil() as usize).min(w);
                        let y0 = q[0][1].min(q[1][1]).min(q[2][1]).floor().max(0.0) as usize;
                        let y1 = (q[0][1].max(q[1][1]).max(q[2][1]).ceil() as usize).min(h);
                        for y in y0..y1 {
                            for x in x0..x1 {
                                let mut sum = [0f32; 3];
                                let mut got = 0f32;
                                let mut shade = None;
                                for (sx, sy) in
                                    [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)]
                                {
                                    let (fx, fy) = (x as f32 + sx, y as f32 + sy);
                                    let w1 = ((fx - q[0][0]) * (q[2][1] - q[0][1])
                                        - (q[2][0] - q[0][0]) * (fy - q[0][1]))
                                        / det;
                                    let w2 = ((q[1][0] - q[0][0]) * (fy - q[0][1])
                                        - (fx - q[0][0]) * (q[1][1] - q[0][1]))
                                        / det;
                                    let w0 = 1.0 - w1 - w2;
                                    // on the face only: past its edge a ray lands on whatever lies there, which drew
                                    // dark dashes along the seams; the colour behind a seam is grown out below
                                    if w0 < -0.001 || w1 < -0.001 || w2 < -0.001 {
                                        continue;
                                    }
                                    let c = [corner(k, 0), corner(k, 1), corner(k, 2)];
                                    let p = [0, 1, 2]
                                        .map(|a| w0 * c[0][a] + w1 * c[1][a] + w2 * c[2][a]);
                                    let n = normals[k];
                                    let o = [0, 1, 2].map(|a| p[a] + n[a] * reach);
                                    let d = [-n[0], -n[1], -n[2]];
                                    let found =
                                        src.hit(o, d, reach * 2.0, &mut stamp, &mut ray).or_else(
                                            || src.hit(p, n, reach * 2.5, &mut stamp, &mut ray),
                                        );
                                    if let Some((tri, _, b)) = found {
                                        // the picture from every sample, the shading from the first: it is soft
                                        // anyway, and its rays are most of the time a bake takes
                                        let k = *shade.get_or_insert_with(|| {
                                            shade_at(
                                                &src, tri, b, n, ao_reach, &mut stamp, &mut ray,
                                            )
                                        });
                                        let colour = picture_at(&src, &samplers, tri, b);
                                        for a in 0..3 {
                                            sum[a] += colour[a] * k;
                                        }
                                        got += 1.0;
                                    }
                                }
                                if got > 0.0 {
                                    let cell = &mut px[y * w + x];
                                    if got > cell[3] {
                                        *cell = [sum[0] / got, sum[1] / got, sum[2] / got, got];
                                    }
                                }
                            }
                        }
                    }
                    out.push((i, px));
                }
                out
            }));
        }
        for h in handles {
            if let Ok(part) = h.join() {
                done.extend(part);
            }
        }
    });
    done.sort_by_key(|d| d.0);
    // the atlas, with colour grown outward from what was drawn, so that mip levels do not pull in the background
    let t = tex as usize;
    let mut colour = vec![[0f32; 3]; t * t];
    let mut known = vec![false; t * t];
    let (mut sum, mut seen) = ([0f64; 3], 0f64);
    for (i, px) in &done {
        let r = charts[*i].rect;
        for y in 0..r[3] as usize {
            for x in 0..r[2] as usize {
                let c = px[y * r[2] as usize + x];
                if c[3] > 0.0 {
                    let at = (r[1] as usize + y) * t + r[0] as usize + x;
                    colour[at] = [c[0], c[1], c[2]];
                    known[at] = true;
                    for k in 0..3 {
                        sum[k] += c[k] as f64;
                    }
                    seen += 1.0;
                }
            }
        }
    }
    if seen == 0.0 {
        return Err("no surface of the source was found".into());
    }
    let mean = [0, 1, 2].map(|k| (sum[k] / seen) as f32);
    for _ in 0..PAD * 3 {
        let before = known.clone();
        for y in 0..t {
            for x in 0..t {
                let at = y * t + x;
                if before[at] {
                    continue;
                }
                let near = [
                    (x > 0).then(|| at - 1),
                    (x + 1 < t).then(|| at + 1),
                    (y > 0).then(|| at - t),
                    (y + 1 < t).then(|| at + t),
                ];
                if let Some(j) = near.into_iter().flatten().find(|j| before[*j]) {
                    colour[at] = colour[j];
                    known[at] = true;
                }
            }
        }
    }
    let mut atlas = image::RgbaImage::new(tex, tex);
    for (x, y, p) in atlas.enumerate_pixels_mut() {
        let at = y as usize * t + x as usize;
        let c = if known[at] { colour[at] } else { mean };
        *p = image::Rgba([c[0] as u8, c[1] as u8, c[2] as u8, 255]);
    }
    // the mesh, in game space
    let smooth = corner_normals(&low, &tris);
    let mut mesh = ObjMesh::default();
    for ch in &charts {
        for &k in &ch.tris {
            let base = mesh.positions.len();
            for c in [0, 2, 1] {
                let p = corner(k, c);
                mesh.positions.push(V3::new(
                    p[0] - centre[0],
                    p[1] - centre[1],
                    -(p[2] - centre[2]),
                ));
                let m = smooth[k * 3 + c];
                mesh.normals.push(V3::new(m[0], m[1], -m[2]));
                let x = (dot3(p, ch.u) - ch.origin[0]) * pixels_per_unit + 1.0 + ch.rect[0] as f32;
                let y = (dot3(p, ch.v) - ch.origin[1]) * pixels_per_unit + 1.0 + ch.rect[1] as f32;
                mesh.uvs.push([x / tex as f32, 1.0 - y / tex as f32]);
            }
            let f = vec![base, base + 1, base + 2];
            mesh.faces.push(f.clone());
            mesh.face_uvs.push(f.clone());
            mesh.face_normals.push(f);
        }
    }
    let tris_out = mesh.faces.len();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(atlas)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("png: {e}"))?;
    Ok((
        GlbPart {
            mesh,
            png,
            tris: tris_out,
        },
        hull_boxes,
    ))
}

/// A material's full picture and tint for [`bake_proxy`].
fn sampler(g: &mut Gltf, material: i64) -> Result<Sampler, String> {
    let m = g.json["materials"]
        .get(material.max(0) as usize)
        .cloned()
        .unwrap_or(Value::Null);
    let pbr = &m["pbrMetallicRoughness"];
    let factor: [f32; 4] = pbr["baseColorFactor"].as_array().map_or([1.0; 4], |a| {
        [0, 1, 2, 3].map(|i| a.get(i).and_then(Value::as_f64).unwrap_or(1.0) as f32)
    });
    let tint = [0, 1, 2].map(|k| srgb(factor[k]));
    let image = match pbr["baseColorTexture"]["index"].as_u64() {
        None => None,
        Some(t) => {
            let tex = g.json["textures"][t as usize].clone();
            let source = tex["extensions"]["EXT_texture_webp"]["source"]
                .as_u64()
                .or_else(|| tex["source"].as_u64())
                .ok_or("a texture with no image")?;
            let view = g.json["images"][source as usize]["bufferView"]
                .as_u64()
                .ok_or("an image with no buffer view")? as usize;
            let bytes = g.view(view)?;
            let i = image::load_from_memory(bytes)
                .map_err(|e| format!("texture: {e}"))?
                .to_rgba8();
            Some(if i.width() > BAKE_SOURCE || i.height() > BAKE_SOURCE {
                image::imageops::resize(
                    &i,
                    i.width().min(BAKE_SOURCE),
                    i.height().min(BAKE_SOURCE),
                    image::imageops::FilterType::Triangle,
                )
            } else {
                i
            })
        }
    };
    let normal = match m["normalTexture"]["index"].as_u64() {
        None => None,
        Some(t) => {
            let tex = g.json["textures"][t as usize].clone();
            let source = tex["extensions"]["EXT_texture_webp"]["source"]
                .as_u64()
                .or_else(|| tex["source"].as_u64())
                .ok_or("a texture with no image")?;
            let view = g.json["images"][source as usize]["bufferView"]
                .as_u64()
                .ok_or("an image with no buffer view")? as usize;
            let bytes = g.view(view)?;
            Some(
                image::load_from_memory(bytes)
                    .map_err(|e| format!("normal map: {e}"))?
                    .to_rgba8(),
            )
        }
    };
    Ok(Sampler {
        image,
        tint,
        normal,
    })
}

/// Linear to the display value a texture holds.
fn srgb(linear: f32) -> f32 {
    let l = linear.clamp(0.0, 1.0);
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

/// Draws the source onto a face's rectangle: straight on, keeping what lies farthest out along the face's normal.
fn paint_face(
    face: &BakeFace,
    by_material: &BTreeMap<i64, Gathered>,
    samplers: &BTreeMap<i64, Sampler>,
    to_game: &dyn Fn([f32; 3]) -> V3,
    atlas: &mut image::RgbaImage,
) -> bool {
    let (rw, rh) = (
        face.rect[2] as usize * BAKE_SAMPLES,
        face.rect[3] as usize * BAKE_SAMPLES,
    );
    if rw == 0 || rh == 0 {
        return true;
    }
    let mut depth = vec![f32::MIN; rw * rh];
    let mut colour = vec![[0f32; 3]; rw * rh];
    for (mat, e) in by_material {
        let Some(s) = samplers.get(mat) else { continue };
        for t in e.tris.as_chunks::<3>().0 {
            let vs = [t[0], t[1], t[2]].map(|i| e.verts[i as usize]);
            let ps = vs.map(|v| {
                let d = to_game(v.p) - face.origin;
                [
                    d.dot(face.u) / face.size[0] * rw as f32,
                    d.dot(face.v) / face.size[1] * rh as f32,
                    d.dot(face.n),
                ]
            });
            let area = (ps[1][0] - ps[0][0]) * (ps[2][1] - ps[0][1])
                - (ps[2][0] - ps[0][0]) * (ps[1][1] - ps[0][1]);
            if area.abs() < 1e-9 {
                continue;
            }
            let lo = |k: usize| ps[0][k].min(ps[1][k]).min(ps[2][k]);
            let hi = |k: usize| ps[0][k].max(ps[1][k]).max(ps[2][k]);
            if hi(0) < 0.0 || hi(1) < 0.0 || lo(0) >= rw as f32 || lo(1) >= rh as f32 {
                continue;
            }
            let (ax, bx) = (
                lo(0).floor().max(0.0) as usize,
                (hi(0).ceil() as usize).min(rw - 1),
            );
            let (ay, by) = (
                lo(1).floor().max(0.0) as usize,
                (hi(1).ceil() as usize).min(rh - 1),
            );
            for y in ay..=by {
                for x in ax..=bx {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w1 = ((px - ps[0][0]) * (ps[2][1] - ps[0][1])
                        - (ps[2][0] - ps[0][0]) * (py - ps[0][1]))
                        / area;
                    let w2 = ((ps[1][0] - ps[0][0]) * (py - ps[0][1])
                        - (px - ps[0][0]) * (ps[1][1] - ps[0][1]))
                        / area;
                    let w0 = 1.0 - w1 - w2;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let z = w0 * ps[0][2] + w1 * ps[1][2] + w2 * ps[2][2];
                    let i = y * rw + x;
                    if z > depth[i] {
                        depth[i] = z;
                        let mix =
                            |f: fn(&Vert) -> f32| w0 * f(&vs[0]) + w1 * f(&vs[1]) + w2 * f(&vs[2]);
                        colour[i] = s.at(
                            mix(|v| v.uv[0]),
                            mix(|v| v.uv[1]),
                            [mix(|v| v.c[0]), mix(|v| v.c[1]), mix(|v| v.c[2])],
                        );
                    }
                }
            }
        }
    }
    // what no surface covered takes the colour of its nearest neighbour, so the solid has no bare patches
    let mut known: Vec<bool> = depth.iter().map(|d| *d > f32::MIN).collect();
    let any = known.iter().filter(|k| **k).count();
    if any == 0 {
        return false;
    }
    let mut left = rw * rh - any;
    while left > 0 {
        let before = known.clone();
        for y in 0..rh {
            for x in 0..rw {
                let i = y * rw + x;
                if before[i] {
                    continue;
                }
                let mut near = None;
                if x > 0 && before[i - 1] {
                    near = Some(i - 1);
                } else if x + 1 < rw && before[i + 1] {
                    near = Some(i + 1);
                } else if y > 0 && before[i - rw] {
                    near = Some(i - rw);
                } else if y + 1 < rh && before[i + rw] {
                    near = Some(i + rw);
                }
                if let Some(j) = near {
                    colour[i] = colour[j];
                    known[i] = true;
                    left -= 1;
                }
            }
        }
    }
    // down to the atlas's pixels
    for y in 0..face.rect[3] as usize {
        for x in 0..face.rect[2] as usize {
            let mut sum = [0f32; 3];
            for sy in 0..BAKE_SAMPLES {
                for sx in 0..BAKE_SAMPLES {
                    let c = colour[(y * BAKE_SAMPLES + sy) * rw + x * BAKE_SAMPLES + sx];
                    for k in 0..3 {
                        sum[k] += c[k];
                    }
                }
            }
            let div = (BAKE_SAMPLES * BAKE_SAMPLES) as f32;
            let px = image::Rgba([
                (sum[0] / div).round().clamp(0.0, 255.0) as u8,
                (sum[1] / div).round().clamp(0.0, 255.0) as u8,
                (sum[2] / div).round().clamp(0.0, 255.0) as u8,
                255,
            ]);
            let (ax, ay) = (face.rect[0] + x as u32, face.rect[1] + y as u32);
            if ax < atlas.width() && ay < atlas.height() {
                atlas.put_pixel(ax, ay, px);
            }
        }
    }
    true
}

/// A part's OBJ text.
/// How far a hidden core stands outside its model: a bullet mark lands on the core, so inside the model it would not
/// show (DC-DEFAULT: MAP-111).
pub const CORE_MARGIN: f32 = 1.01;

/// Corners of the outline [`footprint`] keeps.
const FOOTPRINT_CORNERS: usize = 10;

/// The convex outline of points on the x / z plane, counter-clockwise (the order a prism solid takes), with at most
/// [`FOOTPRINT_CORNERS`] corners and grown just past the model, so that the bullet marks on a hidden core show outside its model (DC-DEFAULT: MAP-105).
pub fn footprint(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let hull = outline(points, FOOTPRINT_CORNERS);
    let n = hull.len() as f32;
    let c = hull
        .iter()
        .fold([0.0, 0.0], |s, q| [s[0] + q[0] / n, s[1] + q[1] / n]);
    // how far the outline must grow to hold every point: a few corners cut across a round model
    let mut grow = 1.0f32;
    for i in 0..hull.len() {
        let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
        let (ex, ez) = (b[0] - a[0], b[1] - a[1]);
        let len = (ex * ex + ez * ez).sqrt();
        if len < 1e-6 {
            continue;
        }
        let (nx, nz) = (ez / len, -ex / len);
        let edge = (a[0] - c[0]) * nx + (a[1] - c[1]) * nz;
        if edge.abs() < 1e-6 {
            continue;
        }
        for p in points {
            grow = grow.max(((p[0] - c[0]) * nx + (p[1] - c[1]) * nz) / edge);
        }
    }
    let k = grow.min(1.15) * CORE_MARGIN;
    hull.iter()
        .map(|q| [c[0] + (q[0] - c[0]) * k, c[1] + (q[1] - c[1]) * k])
        .collect()
}

/// The convex outline of points on the x / z plane, counter-clockwise, with at most `corners` corners.
fn outline(points: &[[f32; 2]], corners: usize) -> Vec<[f32; 2]> {
    let mut p = points.to_vec();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    if p.len() < 3 {
        return Vec::new();
    }
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let half = |it: &mut dyn Iterator<Item = [f32; 2]>| {
        let mut h: Vec<[f32; 2]> = Vec::new();
        for q in it {
            while h.len() >= 2 && cross(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
                h.pop();
            }
            h.push(q);
        }
        h.pop();
        h
    };
    let mut hull = half(&mut p.iter().copied());
    hull.extend(half(&mut p.iter().rev().copied()));
    // the corner whose removal loses the least area goes first
    while hull.len() > corners {
        let n = hull.len();
        let lost = |i: usize| cross(hull[(i + n - 1) % n], hull[i], hull[(i + 1) % n]).abs();
        let worst = (0..n)
            .min_by(|a, b| lost(*a).total_cmp(&lost(*b)))
            .unwrap_or(0);
        hull.remove(worst);
    }
    hull
}

/// A picture as a model: a flat sheet `width` metres wide, standing on the origin and facing +z, for paint on a wall.
/// Its holes stay holes when the map is exported, so that a sheet of its own has the wall behind it. It is cut into
/// cells because a model is lit at its corners (DC-DEFAULT: MAP-110).
pub fn decal(png: &[u8], width: f32) -> Result<GlbModel, String> {
    if !(width > 0.0 && width.is_finite()) {
        return Err("a picture needs a width in metres".into());
    }
    let img = image::load_from_memory(png).map_err(|e| e.to_string())?;
    let (w, h) = (img.width(), img.height());
    if !w.is_power_of_two() || !h.is_power_of_two() || w.min(h) < 4 {
        return Err(format!(
            "{w} x {h}: a picture on a wall needs power-of-two sides of at least 4"
        ));
    }
    let height = width * h as f32 / w as f32;
    let (nx, ny) = (
        ((width / 0.5).ceil() as usize).clamp(1, 32),
        ((height / 0.5).ceil() as usize).clamp(1, 32),
    );
    let mut mesh = ObjMesh {
        normals: vec![V3::new(0.0, 0.0, 1.0)],
        ..Default::default()
    };
    for j in 0..=ny {
        for i in 0..=nx {
            let (s, t) = (i as f32 / nx as f32, j as f32 / ny as f32);
            // seen from the front the picture's left is at +x
            mesh.positions
                .push(V3::new((0.5 - s) * width, (1.0 - t) * height, 0.0));
            // an OBJ's v runs up from the picture's foot
            mesh.uvs.push([s, 1.0 - t]);
        }
    }
    let at = |i: usize, j: usize| j * (nx + 1) + i;
    for j in 0..ny {
        for i in 0..nx {
            let face = vec![at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            mesh.face_uvs.push(face.clone());
            mesh.face_normals.push(vec![0; 4]);
            mesh.faces.push(face);
        }
    }
    Ok(GlbModel {
        parts: vec![GlbPart {
            tris: nx * ny * 2,
            mesh,
            png: png.to_vec(),
        }],
        size: [width, height, 0.0],
        source_tris: nx * ny * 2,
        footprint: Vec::new(),
        hull_boxes: Vec::new(),
    })
}

pub fn part_obj(p: &GlbPart) -> String {
    write_obj(&p.mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-triangle glb with plain float buffers and no texture.
    fn tiny() -> Vec<u8> {
        tiny_deep(0.0)
    }

    /// The same triangle, its last corner `z` deep, so that it has a volume around it.
    fn tiny_deep(z: f32) -> Vec<u8> {
        let mut bin: Vec<u8> = Vec::new();
        for v in [0.0f32, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 3.0, z] {
            bin.extend(v.to_le_bytes());
        }
        for i in [0u16, 1, 2] {
            bin.extend(i.to_le_bytes());
        }
        bin.extend([0u8, 0]);
        let json = serde_json::json!({
            "scene": 0, "scenes": [{"nodes": [0]}], "nodes": [{"mesh": 0, "translation": [0.0, 1.0, 0.0]}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "material": 0}]}],
            "materials": [{"name": "red_plastic", "pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.0, 0.0, 1.0]}}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"}
            ],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 36}, {"buffer": 0, "byteOffset": 36, "byteLength": 6}],
            "buffers": [{"byteLength": bin.len()}]
        });
        let mut j = serde_json::to_vec(&json).unwrap();
        while !j.len().is_multiple_of(4) {
            j.push(b' ');
        }
        let mut out = Vec::new();
        out.extend(b"glTF");
        out.extend(2u32.to_le_bytes());
        out.extend(((12 + 8 + j.len() + 8 + bin.len()) as u32).to_le_bytes());
        out.extend((j.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(&j);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(&bin);
        out
    }

    fn opts() -> GlbOptions {
        GlbOptions {
            leave_out: String::new(),
            relief: 0.0,
            proxy: Proxy::None,
            strict: false,
            textures_only: false,
            max_tris: 100,
            tex: 64,
            orient: String::new(),
        }
    }

    #[test]
    fn a_model_lands_with_its_base_on_the_origin_and_its_winding_mirrored() {
        let m = convert(&tiny(), &opts()).unwrap();
        assert_eq!(m.source_tris, 1);
        assert_eq!(m.size, [2.0, 3.0, 0.0]);
        let p = &m.parts[0];
        assert_eq!(p.tris, 1);
        assert_eq!(
            p.mesh.positions[0],
            V3::new(-1.0, 0.0, 0.0),
            "centred, its node's lift gone"
        );
        assert_eq!(
            p.mesh.faces[0],
            vec![0, 2, 1],
            "z is mirrored, so the turn order flips"
        );
        let png = image::load_from_memory(&p.png).unwrap().to_rgba8();
        assert_eq!(
            png.get_pixel(0, 0).0,
            [255, 0, 0, 255],
            "a colour with no texture becomes a small picture"
        );
    }

    #[test]
    fn a_picture_becomes_a_sheet_standing_on_the_origin() {
        let mut png = Vec::new();
        image::RgbaImage::new(64, 32)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let m = decal(&png, 4.0).unwrap();
        assert_eq!(m.size, [4.0, 2.0, 0.0]);
        let p = &m.parts[0];
        assert_eq!(p.tris, 8 * 4 * 2);
        let ys: Vec<f32> = p.mesh.positions.iter().map(|q| q.y).collect();
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), 0.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 2.0);
        assert!(decal(&png, 0.0).is_err());
        let mut odd = Vec::new();
        image::RgbaImage::new(60, 32)
            .write_to(&mut std::io::Cursor::new(&mut odd), image::ImageFormat::Png)
            .unwrap();
        assert!(decal(&odd, 4.0).is_err());
    }

    #[test]
    fn leave_out_drops_parts_by_their_material_name() {
        let mut o = opts();
        o.leave_out = "wire, grid".into();
        assert_eq!(convert(&tiny(), &o).unwrap().source_tris, 1, "no such part");
        o.leave_out = "wire,red".into();
        let e = convert(&tiny(), &o).unwrap_err();
        assert!(e.contains("leaves nothing"), "{e}");
    }

    #[test]
    fn orient_turns_the_model_and_refuses_nonsense() {
        let mut o = opts();
        o.orient = "x90".into();
        let m = convert(&tiny(), &o).unwrap();
        assert!(
            (m.size[1] - 0.0).abs() < 1e-4 && (m.size[2] - 3.0).abs() < 1e-4,
            "{:?}",
            m.size
        );
        o.orient = "q90".into();
        assert!(convert(&tiny(), &o).is_err());
        o.orient = "xx".into();
        assert!(convert(&tiny(), &o).is_err());
    }

    #[test]
    fn broken_files_are_errors_not_panics() {
        assert!(convert(b"nope", &opts()).is_err());
        let mut t = tiny();
        t.truncate(30);
        assert!(convert(&t, &opts()).is_err());
    }

    #[test]
    fn a_shell_inside_a_shell_is_removed_and_the_outside_stays() {
        // a closed cube of side 2 around a closed cube of side 1
        let mut g = Gathered::default();
        let mut cube = |half: f32| {
            let base = g.verts.len() as u32;
            for i in 0..8 {
                let sign = |bit: u32| if i >> bit & 1 == 1 { half } else { -half };
                g.verts.push(Vert {
                    p: [sign(0), sign(1), sign(2)],
                    uv: [0.0, 0.0],
                    c: [1.0; 3],
                });
            }
            for q in [
                [0u32, 1, 3, 2],
                [4, 6, 7, 5],
                [0, 4, 5, 1],
                [2, 3, 7, 6],
                [0, 2, 6, 4],
                [1, 5, 7, 3],
            ] {
                g.tris
                    .extend([q[0], q[1], q[2], q[0], q[2], q[3]].map(|v| base + v));
            }
        };
        cube(2.0);
        cube(1.0);
        let mut m = BTreeMap::new();
        m.insert(0, g);
        remove_hidden(&mut m);
        let left = &m[&0];
        assert_eq!(
            left.tris.len() / 3,
            12,
            "the 12 triangles of the outer cube stay"
        );
        assert!(
            left.tris.iter().all(|&v| v < 8),
            "only outer corners are left"
        );
    }

    #[test]
    fn a_footprint_is_a_small_convex_outline_around_the_points() {
        let circle: Vec<[f32; 2]> = (0..64)
            .map(|i| {
                let a = i as f32 * std::f32::consts::TAU / 64.0;
                [a.cos(), a.sin()]
            })
            .chain([[0.0, 0.0], [0.5, 0.2]])
            .collect();
        let f = footprint(&circle);
        assert!(f.len() >= 3 && f.len() <= FOOTPRINT_CORNERS, "{f:?}");
        for i in 0..f.len() {
            let (a, b, c) = (f[i], f[(i + 1) % f.len()], f[(i + 2) % f.len()]);
            let turn = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
            assert!(turn > 0.0, "counter-clockwise and convex: {f:?}");
        }
        // every point inside, so that a mark on the core shows; not much further out than the model
        assert!(f.iter().all(|q| q[0].hypot(q[1]) <= 1.2), "{f:?}");
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            for p in &circle {
                let side = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
                assert!(side >= 0.0, "{p:?} outside {f:?}");
            }
        }
        assert!(footprint(&[[0.0, 0.0], [1.0, 1.0]]).is_empty());
    }

    #[test]
    fn a_box_proxy_is_twelve_outward_triangles_with_the_models_colour() {
        let mut o = opts();
        o.proxy = Proxy::Box;
        let m = convert(&tiny_deep(1.0), &o).unwrap();
        assert_eq!(m.parts.len(), 1);
        let p = &m.parts[0];
        assert_eq!(p.tris, 12, "four sides and two caps of two triangles");
        for u in &p.mesh.uvs {
            assert!(
                (0.0..=1.0).contains(&u[0]) && (0.0..=1.0).contains(&u[1]),
                "{u:?}"
            );
        }
        // every face's normal points away from the box's middle
        let mid = V3::new(0.0, m.size[1] / 2.0, 0.0);
        for f in &p.mesh.faces {
            let c: Vec<V3> = f.iter().map(|i| p.mesh.positions[*i]).collect();
            let (e1, e2) = (c[1] - c[0], c[2] - c[0]);
            let n = V3::new(
                e1.y * e2.z - e1.z * e2.y,
                e1.z * e2.x - e1.x * e2.z,
                e1.x * e2.y - e1.y * e2.x,
            );
            assert!(n.dot(c[0] - mid) > 0.0, "inside out: {c:?}");
        }
        // the source is pure red, and what it does not cover is filled from what it does
        let i = image::load_from_memory(&p.png).unwrap().to_rgba8();
        assert!(
            i.pixels().all(|q| q[0] == 255 && q[1] == 0),
            "no bare patches"
        );
    }

    fn square(x: f32, size: f32, g: &mut Gathered) {
        let base = g.verts.len() as u32;
        for (a, b) in [(0.0, 0.0), (size, 0.0), (0.0, size), (size, size)] {
            g.verts.push(Vert {
                p: [x + a, 0.0, b],
                uv: [0.0, 0.0],
                c: [1.0; 3],
            });
        }
        g.tris
            .extend([base, base + 2, base + 1, base + 1, base + 2, base + 3]);
    }

    #[test]
    fn specks_go_and_a_material_of_nothing_but_specks_stays() {
        let mut g = Gathered::default();
        square(0.0, 10.0, &mut g);
        square(50.0, 0.01, &mut g);
        assert_eq!(
            drop_specks(&g, 20.0).tris.len(),
            6,
            "the rivet goes, the plate stays"
        );
        let mut only = Gathered::default();
        square(0.0, 0.01, &mut only);
        assert_eq!(drop_specks(&only, 20.0).tris.len(), 6);
    }

    #[test]
    fn a_right_angle_stays_hard_and_a_flat_join_stays_smooth() {
        let mut g = Gathered::default();
        let v = |p: [f32; 3]| Vert {
            p,
            uv: [0.0; 2],
            c: [1.0; 3],
        };
        // two faces at a right angle along the edge from (0,0,0) to (0,0,1)
        g.verts = vec![
            v([0.0, 0.0, 0.0]),
            v([0.0, 0.0, 1.0]),
            v([1.0, 0.0, 0.0]),
            v([0.0, 1.0, 0.0]),
        ];
        g.tris = vec![0, 1, 2, 1, 0, 3];
        let n = corner_normals(&g, &g.tris);
        assert!(n[0][1].abs() > 0.99 && n[3][0].abs() > 0.99, "{n:?}");
    }

    #[test]
    fn hard_edges_can_collapse_but_a_texture_seam_stays() {
        let mut g = Gathered::default();
        let v = |p: [f32; 3], uv: [f32; 2]| Vert { p, uv, c: [1.0; 3] };
        g.verts = vec![
            v([0.0; 3], [0.0; 2]),
            v([0.0; 3], [0.0; 2]),
            v([0.0; 3], [0.5; 2]),
        ];
        g.tris = vec![0, 1, 2];
        let w = weld(&g);
        assert_eq!(
            w.verts.len(),
            2,
            "one corner for the normals, two for the texture coordinates"
        );
    }

    #[test]
    fn a_rebake_is_a_sealed_shell_with_one_texture_from_the_source() {
        let mut o = opts();
        o.proxy = Proxy::Rebake;
        let m = convert(&tiny_deep(1.0), &o).unwrap();
        assert_eq!(m.parts.len(), 1);
        let p = &m.parts[0];
        assert!(p.tris >= 4 && p.tris <= o.max_tris * 3, "{}", p.tris);
        for u in &p.mesh.uvs {
            assert!(
                (-0.01..=1.01).contains(&u[0]) && (-0.01..=1.01).contains(&u[1]),
                "{u:?}"
            );
        }
        // pure red: drawn from the source
        let i = image::load_from_memory(&p.png).unwrap().to_rgba8();
        assert!(
            i.pixels().all(|q| q[1] < 8 && q[2] < 8),
            "only red in the atlas"
        );
        assert!(
            i.pixels().any(|q| q[0] > 80),
            "the source's colour reached it"
        );
    }

    #[test]
    fn a_solid_block_needs_no_boxes() {
        let mut by = BTreeMap::new();
        let mut g = Gathered::default();
        for i in 0..8u32 {
            let at = |k: u32| if i >> k & 1 == 1 { 1.0 } else { 0.0 };
            g.verts.push(Vert {
                p: [at(0), at(1), at(2)],
                uv: [0.0; 2],
                c: [1.0; 3],
            });
        }
        for q in [
            [0u32, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ] {
            g.tris.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
        by.insert(0, g);
        let v = Voxels::of(&Source::new(&by), 40);
        assert!(v.hull_boxes([0.0; 3]).is_empty());
    }

    /// A frame of two posts and a top, like a forklift's mast: seen through, so shot through.
    #[test]
    fn collision_boxes_leave_an_open_frame_open() {
        let mut by = BTreeMap::new();
        let mut g = Gathered::default();
        let mut bar = |lo: [f32; 3], hi: [f32; 3]| {
            let base = g.verts.len() as u32;
            for i in 0..8u32 {
                let at = |k: u32, c: usize| if i >> k & 1 == 1 { hi[c] } else { lo[c] };
                g.verts.push(Vert {
                    p: [at(0, 0), at(1, 1), at(2, 2)],
                    uv: [0.0; 2],
                    c: [1.0; 3],
                });
            }
            for q in [
                [0u32, 2, 3, 1],
                [4, 5, 7, 6],
                [0, 1, 5, 4],
                [2, 6, 7, 3],
                [0, 4, 6, 2],
                [1, 3, 7, 5],
            ] {
                g.tris
                    .extend([q[0], q[1], q[2], q[0], q[2], q[3]].map(|v| base + v));
            }
        };
        // two posts 1 m apart and a beam across their tops
        bar([0.0, 0.0, 0.0], [0.1, 2.0, 0.1]);
        bar([1.0, 0.0, 0.0], [1.1, 2.0, 0.1]);
        bar([0.0, 1.9, 0.0], [1.1, 2.0, 0.1]);
        by.insert(0, g);
        let v = Voxels::of(&Source::new(&by), 80);
        let boxes = v.hull_boxes([0.0; 3]);
        assert!(!boxes.is_empty() && boxes.len() <= HULL_BOXES, "{boxes:?}");
        // the middle of the frame is open: a shot through it meets no box
        let inside = |p: [f32; 3]| {
            boxes.iter().any(|b| {
                (b[0]..=b[3]).contains(&p[0])
                    && (b[1]..=b[4]).contains(&p[1])
                    && (b[2]..=b[5]).contains(&p[2])
            })
        };
        assert!(!inside([0.55, 1.0, -0.05]), "open in the middle: {boxes:?}");
        assert!(inside([0.05, 1.0, -0.05]), "a post stops it: {boxes:?}");
        assert!(inside([0.55, 1.95, -0.05]), "the beam stops it: {boxes:?}");
    }

    #[test]
    fn a_shell_is_closed() {
        let mut by = BTreeMap::new();
        let mut g = Gathered::default();
        square(0.0, 1.0, &mut g);
        by.insert(0, g);
        let shell = Voxels::of(&Source::new(&by), 16).surface();
        assert!(!shell.tris.is_empty());
        assert_eq!(
            open_edge_share(&shell, &shell.tris),
            0.0,
            "every edge has two sides"
        );
    }

    #[test]
    fn a_texture_transform_moves_the_coordinates() {
        let json = serde_json::json!({"materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0,
            "extensions": {"KHR_texture_transform": {"offset": [0.25, 0.5], "scale": [2.0, 4.0]}}}}}]});
        let t = uv_transform(&json, 0);
        let (u, v) = (0.5, 0.25);
        assert!((t[0] * u + t[1] * v + t[2] - 1.25).abs() < 1e-5);
        assert!((t[3] * u + t[4] * v + t[5] - 1.5).abs() < 1e-5);
        assert_eq!(
            uv_transform(&serde_json::json!({}), -1),
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        );
    }

    #[test]
    fn simplifying_reaches_the_budget() {
        let mut g = Gathered::default();
        let n = 40u32;
        for y in 0..=n {
            for x in 0..=n {
                g.verts.push(Vert {
                    p: [x as f32, 0.0, y as f32],
                    uv: [0.0, 0.0],
                    c: [1.0; 3],
                });
            }
        }
        let at = |x: u32, y: u32| y * (n + 1) + x;
        for y in 0..n {
            for x in 0..n {
                g.tris.extend([at(x, y), at(x + 1, y), at(x, y + 1)]);
                g.tris
                    .extend([at(x + 1, y), at(x + 1, y + 1), at(x, y + 1)]);
            }
        }
        assert!(simplify(&g, 50, &ERRORS).len() / 3 <= 50);
    }
}
