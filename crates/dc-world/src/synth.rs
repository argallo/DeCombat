//! Synthetic world writer for tests and tools: builds a version-85 file from an owned description using the
//! layouts of FMT_DAT85.md. All content is made up by the caller; nothing comes from the ROM.

use crate::Vec3;

#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn vec3(&mut self, v: Vec3) {
        v.iter().for_each(|&f| self.f32(f));
    }
    fn lstr(&mut self, s: &[u8]) {
        self.u16(s.len() as u16);
        self.0.extend_from_slice(s);
    }
    fn len(&self) -> u32 {
        self.0.len() as u32
    }
}

/// Owned property value for the writer.
// SPEC: FMT_DAT85.md §6 (property types 0-7)
#[derive(Debug, Clone, PartialEq)]
pub enum SynthValue {
    Text(Vec<u8>),
    Vec3(Vec3),
    Rgb(Vec3),
    Real(f32),
    Bits(f32),
    Bool(bool),
    Int(f32),
    Euler([f32; 4]),
    /// Any type code with its payload exactly as stored (a property copied from another world).
    Raw(u8, Vec<u8>),
}

// SPEC: FMT_DAT85.md §6 (property record)
#[derive(Debug, Clone, PartialEq)]
pub struct SynthProp {
    pub name: Vec<u8>,
    pub flags: u32,
    pub value: SynthValue,
}

impl SynthProp {
    /// A property under a DeCombat vocabulary name (docs/MAPKIT_VOCAB.md): flags 0x4 for a vec3 extent (the
    /// box classes' extent property, FMT_DAT85.md §6.2), else 0.
    pub fn dc(name: &str, value: SynthValue) -> Self {
        let flags = match value {
            SynthValue::Vec3(_) if crate::names::PROP_EXTENT.matches_ours(name.as_bytes()) => 0x4,
            _ => 0,
        };
        SynthProp {
            name: name.as_bytes().to_vec(),
            flags,
            value,
        }
    }
}

// SPEC: FMT_DAT85.md §6 (object record), §6.1
#[derive(Debug, Clone, PartialEq)]
pub struct SynthObject {
    pub class: Vec<u8>,
    pub props: Vec<SynthProp>,
}

impl SynthObject {
    /// Object with the three fixed leading properties (name, position, rotation) of FMT_DAT85.md §6.1, using
    /// the caller-supplied property names.
    pub fn point(class: &[u8], names: [&[u8]; 3], name: &[u8], pos: Vec3, rot: Vec3) -> Self {
        SynthObject {
            class: class.to_vec(),
            props: vec![
                SynthProp {
                    name: names[0].to_vec(),
                    flags: 0,
                    value: SynthValue::Text(name.to_vec()),
                },
                SynthProp {
                    name: names[1].to_vec(),
                    flags: 0x0010_0000,
                    value: SynthValue::Vec3(pos),
                },
                SynthProp {
                    name: names[2].to_vec(),
                    flags: 0,
                    value: SynthValue::Euler([rot[0], rot[1], rot[2], 0.0]),
                },
            ],
        }
    }

    /// An object in DeCombat's map vocabulary (docs/MAPKIT_VOCAB.md): the three leading properties under
    /// their `dc` names `name`, `pos`, `rot` with the flags of FMT_DAT85.md §6.1 (0, 0x00100000, 0), then
    /// `props` through [`SynthProp::dc`]. `class` is a `dc.*` class name; the writer does not check it, so a
    /// test can also write an unknown class.
    // SPEC: FMT_DAT85.md §6.1 (leading properties and their flags)
    // DC-DEFAULT: MAP-1, MAP-2 (property 0/1/2 by index; the names are aliases)
    pub fn dc(class: &str, name: &str, pos: Vec3, rot: Vec3, props: &[(&str, SynthValue)]) -> Self {
        let names = [
            crate::names::PROP_NAME.ours.as_bytes(),
            crate::names::PROP_POS.ours.as_bytes(),
            crate::names::PROP_ROT.ours.as_bytes(),
        ];
        let mut o = SynthObject::point(class.as_bytes(), names, name.as_bytes(), pos, rot);
        o.props
            .extend(props.iter().map(|(n, v)| SynthProp::dc(n, v.clone())));
        o
    }
}

/// Owned BSP model: one polygon fan per entry of `polys` (surface, plane, point indices).
// SPEC: FMT_DAT85.md §5, §5.2
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SynthModel {
    pub flags: u32,
    pub name: Vec<u8>,
    pub textures: Vec<Vec<u8>>,
    pub planes: Vec<(Vec3, f32)>,
    pub surfaces: Vec<(u32, u16, u16)>,
    pub polys: Vec<(u32, u32, Vec<u32>)>,
    /// (poly, reserved, front, back)
    pub nodes: Vec<(u32, u16, i32, i32)>,
    pub points: Vec<Vec3>,
    pub root: i32,
    pub box_min: Vec3,
    pub box_max: Vec3,
    pub origin: Vec3,
}

// SPEC: FMT_DAT85.md §10.2 (section)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SynthSection {
    pub textures: [Vec<u8>; 2],
    pub shader_kind: u8,
    pub triangles: u32,
    pub effect: Vec<u8>,
    pub lightmap_w: u32,
    pub lightmap_h: u32,
    pub lightmap: Vec<u8>,
}

// SPEC: FMT_DAT85.md §10.2
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SynthBlock {
    pub center: Vec3,
    pub half: Vec3,
    pub sections: Vec<SynthSection>,
    /// Raw 68-byte vertices.
    pub vertices: Vec<[u8; 68]>,
    pub triangles: Vec<[u32; 4]>,
    /// (verts, normal, dist)
    pub sky_portals: Vec<(Vec<Vec3>, Vec3, f32)>,
    pub occluders: Vec<((Vec<Vec3>, Vec3, f32), u32)>,
    /// (name, colour, data, slots of patches (words, data))
    pub light_groups: Vec<(Vec<u8>, Vec3, Vec<u8>, Vec<Vec<([u32; 4], Vec<u8>)>>)>,
    pub child_mask: u8,
    pub children: [u32; 2],
}

// SPEC: FMT_DAT85.md §10.1
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SynthRenderWorld {
    pub blocks: Vec<SynthBlock>,
    pub models: Vec<(Vec<u8>, SynthRenderWorld)>,
}

/// Owned description of a whole world file.
// SPEC: FMT_DAT85.md §3-§10
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SynthWorld {
    pub info: Vec<u8>,
    pub extent_min: Vec3,
    pub extent_max: Vec3,
    pub world_offset: Vec3,
    /// Pre-order quadtree split flags.
    pub tree_shape: Vec<bool>,
    pub models: Vec<SynthModel>,
    pub objects: Vec<SynthObject>,
    pub blind: Vec<(u32, Vec<u8>)>,
    pub grid_origin: Vec3,
    pub grid_cell: Vec3,
    pub grid_dims: [u32; 3],
    /// Expanded RGB samples, `x*y*z*3` bytes; packed by [`pack_light_grid`].
    pub grid_rgb: Vec<u8>,
    pub blockers: Vec<(Vec3, f32, Vec<Vec3>)>,
    pub particle_blockers: Vec<(Vec3, f32, Vec<Vec3>)>,
    pub render: SynthRenderWorld,
    /// (name, colour, offset, dims, data)
    pub grids: Vec<(Vec<u8>, Vec3, [i32; 3], [u32; 3], Vec<u8>)>,
}

/// Run-length packs RGB samples in the format of FMT_DAT85.md §8 (runs of equal samples, else literals).
pub fn pack_light_grid(rgb: &[u8]) -> Vec<u8> {
    let samples: Vec<&[u8]> = rgb.chunks_exact(3).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < samples.len() {
        let mut run = 1;
        while i + run < samples.len() && run < 128 && samples[i + run] == samples[i] {
            run += 1;
        }
        if run >= 2 {
            out.push(0x80 | (run - 1) as u8);
            out.extend_from_slice(samples[i]);
            i += run;
        } else {
            let mut lit = 1;
            while i + lit < samples.len()
                && lit < 128
                && !(i + lit + 1 < samples.len() && samples[i + lit] == samples[i + lit + 1])
            {
                lit += 1;
            }
            out.push((lit - 1) as u8);
            for s in &samples[i..i + lit] {
                out.extend_from_slice(s);
            }
            i += lit;
        }
    }
    out
}

fn write_model(o: &mut Out, m: &SynthModel) {
    o.u32(0);
    o.u32(m.flags);
    o.lstr(&m.name);
    o.u32(m.points.len() as u32);
    o.u32(m.planes.len() as u32);
    o.u32(m.surfaces.len() as u32);
    o.u32(0); // portals
    o.u32(m.polys.len() as u32);
    o.u32(0); // leaves
    o.u32(m.polys.iter().map(|p| p.2.len() as u32).sum());
    o.u32(0);
    o.u32(0);
    o.u32(m.nodes.len() as u32);
    o.vec3(m.box_min);
    o.vec3(m.box_max);
    o.vec3(m.origin);
    let names: Vec<u8> = m
        .textures
        .iter()
        .flat_map(|t| t.iter().copied().chain(std::iter::once(0)))
        .collect();
    o.u32(names.len() as u32);
    o.u32(m.textures.len() as u32);
    o.0.extend_from_slice(&names);
    for p in &m.polys {
        o.u8(p.2.len() as u8);
    }
    for (n, d) in &m.planes {
        o.vec3(*n);
        o.f32(*d);
    }
    for &(f, t, tf) in &m.surfaces {
        o.u32(f);
        o.u16(t);
        o.u16(tf);
    }
    for (s, pl, pts) in &m.polys {
        o.u32(*s);
        o.u32(*pl);
        pts.iter().for_each(|&p| o.u32(p));
    }
    for &(p, r, f, b) in &m.nodes {
        o.u32(p);
        o.u16(r);
        o.i32(f);
        o.i32(b);
    }
    m.points.iter().for_each(|&p| o.vec3(p));
    o.i32(m.root);
    o.u32(0);
}

fn write_plane_poly(o: &mut Out, p: &(Vec<Vec3>, Vec3, f32)) {
    o.u8(p.0.len() as u8);
    p.0.iter().for_each(|&v| o.vec3(v));
    o.vec3(p.1);
    o.f32(p.2);
}

fn write_render_world(o: &mut Out, w: &SynthRenderWorld) {
    o.u32(w.blocks.len() as u32);
    for b in &w.blocks {
        o.vec3(b.center);
        o.vec3(b.half);
        o.u32(b.sections.len() as u32);
        for s in &b.sections {
            o.lstr(&s.textures[0]);
            o.lstr(&s.textures[1]);
            o.u8(s.shader_kind);
            o.u32(s.triangles);
            o.lstr(&s.effect);
            o.u32(s.lightmap_w);
            o.u32(s.lightmap_h);
            o.u32(s.lightmap.len() as u32);
            o.0.extend_from_slice(&s.lightmap);
        }
        o.u32(b.vertices.len() as u32);
        b.vertices.iter().for_each(|v| o.0.extend_from_slice(v));
        o.u32(b.triangles.len() as u32);
        b.triangles
            .iter()
            .for_each(|t| t.iter().for_each(|&x| o.u32(x)));
        o.u32(b.sky_portals.len() as u32);
        b.sky_portals.iter().for_each(|p| write_plane_poly(o, p));
        o.u32(b.occluders.len() as u32);
        for (p, id) in &b.occluders {
            write_plane_poly(o, p);
            o.u32(*id);
        }
        o.u32(b.light_groups.len() as u32);
        for (name, col, data, slots) in &b.light_groups {
            o.lstr(name);
            o.vec3(*col);
            o.u32(data.len() as u32);
            o.0.extend_from_slice(data);
            o.u32(slots.len() as u32);
            for patches in slots {
                o.u32(patches.len() as u32);
                for (words, d) in patches {
                    words.iter().for_each(|&x| o.u32(x));
                    o.u32(d.len() as u32);
                    o.0.extend_from_slice(d);
                }
            }
        }
        o.u8(b.child_mask);
        o.u32(b.children[0]);
        o.u32(b.children[1]);
    }
    o.u32(w.models.len() as u32);
    for (name, sub) in &w.models {
        o.lstr(name);
        write_render_world(o, sub);
    }
}

fn write_soup(o: &mut Out, s: &[(Vec3, f32, Vec<Vec3>)]) {
    o.u32(s.len() as u32);
    for (n, d, vs) in s {
        o.vec3(*n);
        o.f32(*d);
        o.u32(vs.len() as u32);
        vs.iter().for_each(|&v| o.vec3(v));
    }
    o.u32(0);
}

impl SynthWorld {
    /// Serialises the description; header offsets are filled in so the sections tile the file.
    pub fn build(&self) -> Vec<u8> {
        let mut o = Out(vec![0u8; crate::HEADER_BYTES]);
        o.u32(self.info.len() as u32);
        o.0.extend_from_slice(&self.info);
        o.vec3(self.extent_min);
        o.vec3(self.extent_max);
        o.vec3(self.world_offset);
        o.vec3(self.extent_min);
        o.vec3(self.extent_max);
        o.u32(self.tree_shape.len() as u32);
        o.u32(0);
        for chunk in self.tree_shape.chunks(8) {
            o.u8(chunk
                .iter()
                .enumerate()
                .fold(0u8, |a, (i, &b)| a | (b as u8) << i));
        }
        o.u32(self.models.len() as u32);
        self.models.iter().for_each(|m| write_model(&mut o, m));

        let objects_at = o.len();
        o.u32(self.objects.len() as u32);
        for ob in &self.objects {
            let mut body = Out::default();
            body.lstr(&ob.class);
            body.u32(ob.props.len() as u32);
            for p in &ob.props {
                body.lstr(&p.name);
                let (code, payload): (u8, Vec<u8>) = match &p.value {
                    SynthValue::Text(t) => {
                        let mut v = (t.len() as u16).to_le_bytes().to_vec();
                        v.extend_from_slice(t);
                        (0, v)
                    }
                    SynthValue::Vec3(v) => (1, v.iter().flat_map(|f| f.to_le_bytes()).collect()),
                    SynthValue::Rgb(v) => (2, v.iter().flat_map(|f| f.to_le_bytes()).collect()),
                    SynthValue::Real(f) => (3, f.to_le_bytes().to_vec()),
                    SynthValue::Bits(f) => (4, f.to_le_bytes().to_vec()),
                    SynthValue::Bool(b) => (5, vec![*b as u8]),
                    SynthValue::Int(f) => (6, f.to_le_bytes().to_vec()),
                    SynthValue::Euler(e) => (7, e.iter().flat_map(|f| f.to_le_bytes()).collect()),
                    SynthValue::Raw(code, bytes) => (*code, bytes.clone()),
                };
                body.u8(code);
                body.u32(p.flags);
                body.u16(payload.len() as u16);
                body.0.extend_from_slice(&payload);
            }
            o.u16(body.0.len() as u16);
            o.0.extend_from_slice(&body.0);
        }

        let blind_at = o.len();
        o.u32(self.blind.len() as u32);
        for (id, d) in &self.blind {
            o.u32(d.len() as u32);
            o.u32(*id);
            o.0.extend_from_slice(d);
        }

        let lightgrid_at = o.len();
        o.vec3(self.grid_origin);
        o.vec3(self.grid_cell);
        self.grid_dims.iter().for_each(|&d| o.u32(d));
        let packed = pack_light_grid(&self.grid_rgb);
        o.u32(packed.len() as u32);
        o.0.extend_from_slice(&packed);

        let blockers_at = o.len();
        write_soup(&mut o, &self.blockers);
        let pblockers_at = o.len();
        write_soup(&mut o, &self.particle_blockers);

        let render_at = o.len();
        write_render_world(&mut o, &self.render);
        o.u32(self.grids.len() as u32);
        for (name, col, off, dims, data) in &self.grids {
            o.lstr(name);
            o.vec3(*col);
            off.iter().for_each(|&x| o.i32(x));
            dims.iter().for_each(|&x| o.u32(x));
            o.0.extend_from_slice(data);
        }

        let head = [
            crate::WORLD_VERSION,
            objects_at,
            blind_at,
            lightgrid_at,
            blockers_at,
            pblockers_at,
            render_at,
        ];
        for (i, v) in head.iter().enumerate() {
            o.0[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        o.0
    }
}
