//! Render data for the composer: blocks copied, moved by an offset, or clipped to a region outside its cuts.
//!
//! A clipped block keeps its sections in order with the triangles that survive (sections left empty are dropped,
//! with their light-group slot), interpolates every attribute of a vertex a box face cuts through (both uv pairs,
//! the colour, the three unit vectors renormalised), drops its sky portals and occluders (dc-render reads portals
//! from the map's own blocks only and occluders not at all) and clears its child tree (never read, the blocks are
//! drawn flat).
// SPEC: FMT_DAT85.md §10.2 (block, section, 68-byte vertex: position 0, uv 12 and 20, colour 28, unit vectors 32,
// 44, 56; OBSERVED split, Appendix Z D9), §10.3 (one light-group slot per section, in section order, D11)
// DC-DEFAULT: MAP-116

use super::clip::{self, Aabb, Corner, V};
use crate::synth::{SynthBlock, SynthRenderWorld, SynthSection};
use crate::{
    PlanePoly, RenderBlock, RenderLightGroup, RenderSection, RenderWorld, RENDER_VERTEX_BYTES,
};

/// One 68-byte render vertex.
#[derive(Debug, Clone, PartialEq)]
pub struct RVert(pub [u8; RENDER_VERTEX_BYTES]);

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn put_f32(b: &mut [u8], o: usize, v: f32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

impl RVert {
    fn moved(mut self, t: V) -> RVert {
        for (axis, &d) in t.iter().enumerate() {
            let p = f32_at(&self.0, 4 * axis);
            put_f32(&mut self.0, 4 * axis, p + d);
        }
        self
    }
}

impl Corner for RVert {
    fn pos(&self) -> V {
        [f32_at(&self.0, 0), f32_at(&self.0, 4), f32_at(&self.0, 8)]
    }

    fn lerp(&self, o: &RVert, t: f32) -> RVert {
        let mut out = [0u8; RENDER_VERTEX_BYTES];
        // position and the two uv pairs: seven floats
        for k in 0..7 {
            let (a, b) = (f32_at(&self.0, 4 * k), f32_at(&o.0, 4 * k));
            put_f32(&mut out, 4 * k, a + (b - a) * t);
        }
        // the colour, byte by byte
        for (k, slot) in out.iter_mut().enumerate().take(32).skip(28) {
            let (a, b) = (self.0[k] as f32, o.0[k] as f32);
            *slot = (a + (b - a) * t + 0.5).clamp(0.0, 255.0) as u8;
        }
        // normal and the two other unit vectors
        for tri in 0..3 {
            let base = 32 + 12 * tri;
            let mut v = [0.0f32; 3];
            for (c, slot) in v.iter_mut().enumerate() {
                let (a, b) = (f32_at(&self.0, base + 4 * c), f32_at(&o.0, base + 4 * c));
                *slot = a + (b - a) * t;
            }
            let l = clip::length(v);
            if l > 1.0e-6 {
                v = clip::scale(v, 1.0 / l);
            }
            for (c, &x) in v.iter().enumerate() {
                put_f32(&mut out, base + 4 * c, x);
            }
        }
        RVert(out)
    }

    fn snap(mut self, axis: usize, v: f32) -> RVert {
        put_f32(&mut self.0, 4 * axis, v);
        self
    }
}

fn section_of(s: &RenderSection<'_>) -> SynthSection {
    SynthSection {
        textures: [s.textures[0].to_vec(), s.textures[1].to_vec()],
        shader_kind: s.shader_kind,
        triangles: s.triangles,
        effect: s.effect.to_vec(),
        lightmap_w: s.lightmap_w,
        lightmap_h: s.lightmap_h,
        lightmap: s.lightmap.to_vec(),
    }
}

type Group = (Vec<u8>, [f32; 3], Vec<u8>, Vec<Vec<([u32; 4], Vec<u8>)>>);

/// A light group with the slots of `keep` (section indices), or every slot.
fn group_of(g: &RenderLightGroup<'_>, keep: Option<&[usize]>) -> Group {
    let slot = |s: &Vec<crate::LightPatch<'_>>| -> Vec<([u32; 4], Vec<u8>)> {
        s.iter().map(|p| (p.words, p.data.to_vec())).collect()
    };
    let slots = match keep {
        None => g.slots.iter().map(slot).collect(),
        Some(k) => k
            .iter()
            .map(|&i| g.slots.get(i).map(slot).unwrap_or_default())
            .collect(),
    };
    (g.name.to_vec(), g.colour, g.data.to_vec(), slots)
}

fn plane_poly(p: &PlanePoly, t: V) -> (Vec<[f32; 3]>, [f32; 3], f32) {
    (
        p.verts.iter().map(|&v| clip::add(v, t)).collect(),
        p.normal,
        p.dist + clip::dot(p.normal, t),
    )
}

fn vertex(b: &RenderBlock<'_>, i: u32) -> Option<RVert> {
    let o = (i as usize).checked_mul(RENDER_VERTEX_BYTES)?;
    let raw = b.vertices.get(o..o + RENDER_VERTEX_BYTES)?;
    let mut v = [0u8; RENDER_VERTEX_BYTES];
    v.copy_from_slice(raw);
    Some(RVert(v))
}

/// Expands a packed lightmap (the light grid's run-length packing) to `n` RGB texels; `None` when the payload is
/// short or overlong.
// SPEC: FMT_DAT85.md §8 (packing), addendum 2026-10-24 (lightmaps use the light grid's packing)
fn unpack(p: &[u8], n: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(n * 3);
    let mut i = 0;
    while out.len() < n * 3 {
        let c = *p.get(i)?;
        i += 1;
        let run = (c & 0x7f) as usize + 1;
        if c & 0x80 != 0 {
            let s = p.get(i..i + 3)?;
            for _ in 0..run {
                out.extend_from_slice(s);
            }
            i += 3;
        } else {
            out.extend_from_slice(p.get(i..i + 3 * run)?);
            i += 3 * run;
        }
    }
    (out.len() == n * 3).then_some(out)
}

/// A piece's section with its light raised to at least `min_light` on every channel: the lightmap's texels (kind
/// 2) and the vertex colours (the kinds that draw them) of `vertices` are lifted where they are darker (MAP-120:
/// the outer faces of a world, never seen from its play area, got the map's ambient alone).
// DC-DEFAULT: MAP-120
fn lifted(mut sec: SynthSection, min_light: u8) -> SynthSection {
    if min_light == 0 || sec.shader_kind != 2 || sec.lightmap_w == 0 || sec.lightmap_h == 0 {
        return sec;
    }
    let n = sec.lightmap_w as usize * sec.lightmap_h as usize;
    if let Some(mut rgb) = unpack(&sec.lightmap, n) {
        if rgb.iter().any(|&c| c < min_light) {
            for c in &mut rgb {
                *c = (*c).max(min_light);
            }
            sec.lightmap = crate::synth::pack_light_grid(&rgb);
        }
    }
    sec
}

/// A vertex with its colour's red, green and blue raised to at least `min_light` (the alpha kept).
fn lift_vertex(mut v: [u8; RENDER_VERTEX_BYTES], min_light: u8) -> [u8; RENDER_VERTEX_BYTES] {
    for c in &mut v[28..31] {
        *c = (*c).max(min_light);
    }
    v
}

/// A block moved by `t`. The map's own blocks (`piece` false, `t` zero) keep everything; a piece's blocks lose
/// their occluders and get their light raised to `min_light` ([`lifted`]).
pub fn moved_block(b: &RenderBlock<'_>, t: V, piece: bool, min_light: u8) -> SynthBlock {
    let lift = if piece { min_light } else { 0 };
    let vertices = b
        .vertices
        .as_chunks::<RENDER_VERTEX_BYTES>()
        .0
        .iter()
        .map(|c| lift_vertex(RVert(*c).moved(t).0, lift))
        .collect();
    let triangles = (0..b.triangle_count)
        .filter_map(|i| b.triangle(i))
        .collect();
    SynthBlock {
        center: clip::add(b.center, t),
        half: b.half,
        sections: b
            .sections
            .iter()
            .map(|sec| lifted(section_of(sec), lift))
            .collect(),
        vertices,
        triangles,
        sky_portals: b.sky_portals.iter().map(|p| plane_poly(p, t)).collect(),
        occluders: if piece {
            Vec::new()
        } else {
            b.occluders
                .iter()
                .map(|(p, id)| (plane_poly(p, t), *id))
                .collect()
        },
        light_groups: b.light_groups.iter().map(|g| group_of(g, None)).collect(),
        child_mask: b.child_mask,
        children: b.children,
    }
}

/// A render world moved by `t`, every block and nested model kept (names unchanged); `piece` and `min_light` as
/// [`moved_block`].
pub fn moved_world(w: &RenderWorld<'_>, t: V, piece: bool, min_light: u8) -> SynthRenderWorld {
    SynthRenderWorld {
        blocks: w
            .blocks
            .iter()
            .map(|b| moved_block(b, t, piece, min_light))
            .collect(),
        models: w
            .models
            .iter()
            .map(|m| (m.name.to_vec(), moved_world(&m.world, t, piece, min_light)))
            .collect(),
    }
}

/// A block moved by `t` and kept to `region` outside `cuts`; `None` when nothing is left.
pub fn clipped_block(
    b: &RenderBlock<'_>,
    t: V,
    region: &Aabb,
    cuts: &[Aabb],
    min_light: u8,
) -> Option<SynthBlock> {
    let mut out: Vec<[u8; RENDER_VERTEX_BYTES]> = Vec::new();
    let mut remap: Vec<u32> = vec![u32::MAX; b.vertex_count as usize];
    let mut tris: Vec<[u32; 4]> = Vec::new();
    let mut sections = Vec::new();
    let mut kept_sections = Vec::new();
    let mut first = 0u32;
    for (si, s) in b.sections.iter().enumerate() {
        let before = tris.len();
        for i in first..first.saturating_add(s.triangles) {
            let Some(tri) = b.triangle(i) else {
                continue;
            };
            let corners = [tri[0], tri[1], tri[2]];
            let mut vs: Vec<RVert> = Vec::with_capacity(3);
            for &c in &corners {
                match vertex(b, c) {
                    Some(v) => vs.push(v.moved(t)),
                    None => break,
                }
            }
            if vs.len() != 3 {
                continue;
            }
            let pts = [vs[0].pos(), vs[1].pos(), vs[2].pos()];
            let Some(bounds) = Aabb::of_points(pts.iter()) else {
                continue;
            };
            if !region.touches(&bounds) {
                continue;
            }
            if region.contains_box(&bounds) && !cuts.iter().any(|k| k.overlaps(&bounds)) {
                let mut idx = [0u32; 3];
                for (k, (&c, v)) in corners.iter().zip(vs.iter()).enumerate() {
                    let slot = &mut remap[c as usize];
                    if *slot == u32::MAX {
                        *slot = out.len() as u32;
                        out.push(v.0);
                    }
                    idx[k] = *slot;
                }
                tris.push([idx[0], idx[1], idx[2], tri[3]]);
                continue;
            }
            for part in clip::keep(&vs, region, cuts) {
                let base = out.len() as u32;
                out.extend(part.iter().map(|v| v.0));
                for k in 1..part.len() as u32 - 1 {
                    tris.push([base, base + k, base + k + 1, tri[3]]);
                }
            }
        }
        let count = (tris.len() - before) as u32;
        if count > 0 {
            let mut sec = lifted(section_of(s), min_light);
            sec.triangles = count;
            sections.push(sec);
            kept_sections.push(si);
        }
        first = first.saturating_add(s.triangles);
    }
    if tris.is_empty() {
        return None;
    }
    if min_light > 0 {
        for v in &mut out {
            *v = lift_vertex(*v, min_light);
        }
    }
    let pts: Vec<V> = out
        .iter()
        .map(|v| [f32_at(v, 0), f32_at(v, 4), f32_at(v, 8)])
        .collect();
    let bounds = Aabb::of_points(pts.iter())?;
    let centre = bounds.centre();
    Some(SynthBlock {
        center: centre,
        half: clip::scale(clip::sub(bounds.max, bounds.min), 0.5),
        sections,
        vertices: out,
        triangles: tris,
        sky_portals: Vec::new(),
        occluders: Vec::new(),
        light_groups: b
            .light_groups
            .iter()
            .map(|g| group_of(g, Some(&kept_sections)))
            .collect(),
        child_mask: 0,
        children: [0, 0],
    })
}

/// A texture at most this many texels a side is a placeholder when it covers a face back to back with a real wall
/// (MAP-120: the original maps put a tiny flat texture on the outer faces their players never saw).
pub const PLACEHOLDER_TEXELS: u32 = 16;

fn tri_plane(p: &[V; 3]) -> Option<(V, f32)> {
    let n = clip::cross(clip::sub(p[1], p[0]), clip::sub(p[2], p[0]));
    let l = clip::length(n);
    if l < 1.0e-6 {
        return None;
    }
    let n = clip::scale(n, 1.0 / l);
    Some((n, clip::dot(n, p[0])))
}

/// The texture coordinates of `q` (on the triangle's plane) by the triangle's own mapping, extended past its edges.
/// The part of a convex polygon where `dot(m, x - a) >= 0`.
fn clip_half(poly: &[V], m: V, a: V) -> Vec<V> {
    let mut out = Vec::with_capacity(poly.len() + 2);
    for k in 0..poly.len() {
        let (p, q) = (poly[k], poly[(k + 1) % poly.len()]);
        let (dp, dq) = (clip::dot(m, clip::sub(p, a)), clip::dot(m, clip::sub(q, a)));
        if dp >= 0.0 {
            out.push(p);
        }
        if (dp >= 0.0) != (dq >= 0.0) {
            out.push(clip::add(p, clip::scale(clip::sub(q, p), dp / (dp - dq))));
        }
    }
    out
}

/// The area of a convex polygon.
fn poly_area(poly: &[V]) -> f32 {
    let mut acc = [0.0f32; 3];
    for k in 1..poly.len().saturating_sub(1) {
        acc = clip::add(
            acc,
            clip::cross(clip::sub(poly[k], poly[0]), clip::sub(poly[k + 1], poly[0])),
        );
    }
    clip::length(acc) * 0.5
}

/// A vertex at `x` on triangle `tri` of `b` with `uv` for its first pair: the rest of its attributes (the second
/// pair, the colour, the three unit vectors) blended from the triangle's corners.
fn blended(b: &SynthBlock, tri: [u32; 4], x: V, uv: [f32; 2]) -> Option<[u8; RENDER_VERTEX_BYTES]> {
    let vs = [
        b.vertices[tri[0] as usize],
        b.vertices[tri[1] as usize],
        b.vertices[tri[2] as usize],
    ];
    let p = vs.map(|v| [f32_at(&v, 0), f32_at(&v, 4), f32_at(&v, 8)]);
    let w = weights(&p, x)?;
    let mix =
        |o: usize| w[0] * f32_at(&vs[0], o) + w[1] * f32_at(&vs[1], o) + w[2] * f32_at(&vs[2], o);
    let mut out = vs[0];
    for (k, &c) in x.iter().enumerate() {
        put_f32(&mut out, 4 * k, c);
    }
    put_f32(&mut out, 12, uv[0]);
    put_f32(&mut out, 16, uv[1]);
    for o in [20, 24] {
        put_f32(&mut out, o, mix(o));
    }
    for (k, slot) in out.iter_mut().enumerate().take(32).skip(28) {
        let c = w[0] * vs[0][k] as f32 + w[1] * vs[1][k] as f32 + w[2] * vs[2][k] as f32;
        *slot = (c + 0.5).clamp(0.0, 255.0) as u8;
    }
    for base in [32, 44, 56] {
        let v = [mix(base), mix(base + 4), mix(base + 8)];
        let l = clip::length(v);
        let v = if l > 1.0e-6 {
            clip::scale(v, 1.0 / l)
        } else {
            v
        };
        for (c, &x) in v.iter().enumerate() {
            put_f32(&mut out, base + 4 * c, x);
        }
    }
    Some(out)
}

/// The barycentric weights of `q` (on the triangle's plane) in triangle `p`.
fn weights(p: &[V; 3], q: V) -> Option<[f32; 3]> {
    let (v0, v1, v2) = (
        clip::sub(p[1], p[0]),
        clip::sub(p[2], p[0]),
        clip::sub(q, p[0]),
    );
    let (d00, d01, d11) = (clip::dot(v0, v0), clip::dot(v0, v1), clip::dot(v1, v1));
    let (d20, d21) = (clip::dot(v2, v0), clip::dot(v2, v1));
    let den = d00 * d11 - d01 * d01;
    if den.abs() < 1.0e-9 {
        return None;
    }
    let w1 = (d11 * d20 - d01 * d21) / den;
    let w2 = (d00 * d21 - d01 * d20) / den;
    Some([1.0 - w1 - w2, w1, w2])
}

fn uv_at(p: &[V; 3], uv: &[[f32; 2]; 3], q: V) -> Option<[f32; 2]> {
    let (v0, v1, v2) = (
        clip::sub(p[1], p[0]),
        clip::sub(p[2], p[0]),
        clip::sub(q, p[0]),
    );
    let (d00, d01, d11) = (clip::dot(v0, v0), clip::dot(v0, v1), clip::dot(v1, v1));
    let (d20, d21) = (clip::dot(v2, v0), clip::dot(v2, v1));
    let den = d00 * d11 - d01 * d01;
    if den.abs() < 1.0e-9 {
        return None;
    }
    let w1 = (d11 * d20 - d01 * d21) / den;
    let w2 = (d00 * d21 - d01 * d20) / den;
    let w0 = 1.0 - w1 - w2;
    Some([
        w0 * uv[0][0] + w1 * uv[1][0] + w2 * uv[2][0],
        w0 * uv[0][1] + w1 * uv[1][1] + w2 * uv[2][1],
    ])
}

/// Gives a piece's placeholder faces the texture of the wall they are back to back with (MAP-120). A face of a
/// vertex-lit section (kind 1) whose texture `placeholder` names is matched with a triangle of a real surface
/// (kind 1, 4 or 9) on the same plane facing the other way whose box overlaps it; it then draws that surface's
/// texture with that surface's mapping carried across, in a section of its own (a face over several such triangles
/// is cut along them, each piece in its own partner's texture, when the pieces cover it). A placeholder face with no such
/// partner that lies on a real surface's triangle of the same plane and side (its centre inside it) is a hidden
/// duplicate of that surface and is dropped (drawn, the two would fight). A placeholder face left over that shares an
/// edge with a real face takes that face's texture at its scale, projected along its own plane, and hands it on to
/// the placeholder faces it shares edges with (the outer slope of a hill takes the rock of its top); faces cut off from
/// every real face take the real texture covering the most area in the block. Only a block with no real face keeps
/// its placeholders. Returns how many faces changed.
// DC-DEFAULT: MAP-120
pub fn retexture(b: &mut SynthBlock, placeholder: &mut dyn FnMut(&[u8]) -> bool) -> usize {
    let mut ranges = Vec::with_capacity(b.sections.len());
    let mut first = 0usize;
    for s in &b.sections {
        ranges.push(first..first + s.triangles as usize);
        first += s.triangles as usize;
    }
    let is_ph: Vec<bool> = b
        .sections
        .iter()
        .map(|s| s.shader_kind == 1 && !s.textures[0].is_empty() && placeholder(&s.textures[0]))
        .collect();
    if !is_ph.iter().any(|&x| x) {
        return 0;
    }
    let pos = |b: &SynthBlock, i: u32| {
        let v = &b.vertices[i as usize];
        [f32_at(v, 0), f32_at(v, 4), f32_at(v, 8)]
    };
    let uv0 = |b: &SynthBlock, i: u32| {
        let v = &b.vertices[i as usize];
        [f32_at(v, 12), f32_at(v, 16)]
    };
    struct Partner {
        n: V,
        d: f32,
        bounds: Aabb,
        p: [V; 3],
        uv: [[f32; 2]; 3],
        section: usize,
    }
    let mut partners = Vec::new();
    for (si, r) in ranges.iter().enumerate() {
        let k = b.sections[si].shader_kind;
        if is_ph[si] || !matches!(k, 1 | 4 | 9) {
            continue;
        }
        for t in r.clone() {
            let tri = b.triangles[t];
            let p = [pos(b, tri[0]), pos(b, tri[1]), pos(b, tri[2])];
            let Some((n, d)) = tri_plane(&p) else {
                continue;
            };
            let Some(bounds) = Aabb::of_points(p.iter()) else {
                continue;
            };
            let uv = [uv0(b, tri[0]), uv0(b, tri[1]), uv0(b, tri[2])];
            partners.push(Partner {
                n,
                d,
                bounds,
                p,
                uv,
                section: si,
            });
        }
    }
    // the faces that find a partner: (triangle index, partner section, uv per corner); the hidden duplicates
    let mut moved: Vec<(usize, usize, [[f32; 2]; 3])> = Vec::new();
    let mut dropped: Vec<usize> = Vec::new();
    // the faces split among several partners: (triangle, partner section, the corners of one piece)
    let mut split: Vec<(usize, usize, Vec<[u8; RENDER_VERTEX_BYTES]>)> = Vec::new();
    for (si, r) in ranges.iter().enumerate() {
        if !is_ph[si] {
            continue;
        }
        for t in r.clone() {
            let tri = b.triangles[t];
            let q = [pos(b, tri[0]), pos(b, tri[1]), pos(b, tri[2])];
            let Some((n, d)) = tri_plane(&q) else {
                continue;
            };
            let Some(qb) = Aabb::of_points(q.iter()) else {
                continue;
            };
            let grow = Aabb {
                min: clip::sub(qb.min, [1.0; 3]),
                max: clip::add(qb.max, [1.0; 3]),
            };
            // a face over several partners (a wall whose inside changes texture part way across) is cut along
            // them, each piece in its partner's texture and mapping, when the pieces cover it
            let cands: Vec<&Partner> = partners
                .iter()
                .filter(|pt| {
                    clip::dot(n, pt.n) <= -0.99
                        && (d + pt.d).abs() <= 1.5
                        && grow.touches(&pt.bounds)
                })
                .collect();
            if cands.len() >= 2 {
                let total = poly_area(&q);
                let mut pieces: Vec<(&Partner, Vec<V>)> = Vec::new();
                let mut covered = 0.0f32;
                for pt in &cands {
                    let mut poly = q.to_vec();
                    for k in 0..3 {
                        let (pa, pb, pc) = (pt.p[k], pt.p[(k + 1) % 3], pt.p[(k + 2) % 3]);
                        let mut m = clip::cross(pt.n, clip::sub(pb, pa));
                        if clip::dot(m, clip::sub(pc, pa)) < 0.0 {
                            m = clip::scale(m, -1.0);
                        }
                        poly = clip_half(&poly, m, pa);
                        if poly.len() < 3 {
                            break;
                        }
                    }
                    let a = if poly.len() >= 3 {
                        poly_area(&poly)
                    } else {
                        0.0
                    };
                    if a > 1.0e-2 {
                        covered += a;
                        pieces.push((pt, poly));
                    }
                }
                if pieces.len() >= 2 && covered >= total * 0.98 && covered <= total * 1.02 {
                    let built: Option<Vec<_>> = pieces
                        .iter()
                        .map(|(pt, poly)| {
                            poly.iter()
                                .map(|&x| blended(b, tri, x, uv_at(&pt.p, &pt.uv, x)?))
                                .collect::<Option<Vec<_>>>()
                                .map(|corners| (pt.section, corners))
                        })
                        .collect();
                    if let Some(built) = built {
                        split.extend(built.into_iter().map(|(ps, corners)| (t, ps, corners)));
                        continue;
                    }
                }
            }
            let mut best: Option<(f32, &Partner)> = None;
            for pt in &partners {
                if clip::dot(n, pt.n) > -0.99 || (d + pt.d).abs() > 1.5 || !grow.touches(&pt.bounds)
                {
                    continue;
                }
                let ov = grow.intersection(&pt.bounds).map_or(0.0, |o| {
                    let e = clip::sub(o.max, o.min);
                    e[0].max(1.0) * e[1].max(1.0) * e[2].max(1.0)
                });
                if best.is_none_or(|(o, _)| ov > o) {
                    best = Some((ov, pt));
                }
            }
            let Some((_, pt)) = best else {
                let c = clip::scale(clip::add(clip::add(q[0], q[1]), q[2]), 1.0 / 3.0);
                let covered = partners.iter().any(|pt| {
                    clip::dot(n, pt.n) > 0.99
                        && (d - pt.d).abs() <= 1.5
                        && grow.touches(&pt.bounds)
                        && weights(&pt.p, c).is_some_and(|w| w.iter().all(|&x| x >= -1.0e-3))
                });
                if covered {
                    dropped.push(t);
                }
                continue;
            };
            let (Some(a), Some(c), Some(e)) = (
                uv_at(&pt.p, &pt.uv, q[0]),
                uv_at(&pt.p, &pt.uv, q[1]),
                uv_at(&pt.p, &pt.uv, q[2]),
            ) else {
                continue;
            };
            moved.push((t, pt.section, [a, c, e]));
        }
    }
    // the rest: a placeholder face that shares an edge with a real one takes its texture, projected along its own
    // plane at that face's scale, and hands it on across the placeholder faces it shares edges with (the outer
    // slope of a hill takes the rock of its top)
    {
        let taken: std::collections::BTreeSet<usize> = moved
            .iter()
            .map(|m| m.0)
            .chain(dropped.iter().copied())
            .chain(split.iter().map(|x| x.0))
            .collect();
        let key = |p: V| {
            [
                (p[0] * 2.0).round() as i64,
                (p[1] * 2.0).round() as i64,
                (p[2] * 2.0).round() as i64,
            ]
        };
        let mut section_of = vec![usize::MAX; b.triangles.len()];
        for (si, r) in ranges.iter().enumerate() {
            for t in r.clone() {
                section_of[t] = si;
            }
        }
        let mut edges: std::collections::HashMap<([i64; 3], [i64; 3]), Vec<usize>> =
            std::collections::HashMap::new();
        for (t, tri) in b.triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, c) = (key(pos(b, tri[k])), key(pos(b, tri[(k + 1) % 3])));
                let e = if a <= c { (a, c) } else { (c, a) };
                edges.entry(e).or_default().push(t);
            }
        }
        // a real face's texture: its section and its world units per texture repeat
        let real = |t: usize| {
            let si = section_of[t];
            si != usize::MAX && !is_ph[si] && matches!(b.sections[si].shader_kind, 1 | 4 | 9)
        };
        let scale_of = |t: usize| {
            let tri = b.triangles[t];
            let p = [pos(b, tri[0]), pos(b, tri[1]), pos(b, tri[2])];
            let u = [uv0(b, tri[0]), uv0(b, tri[1]), uv0(b, tri[2])];
            let world = clip::length(clip::cross(clip::sub(p[1], p[0]), clip::sub(p[2], p[0])));
            let tex = ((u[1][0] - u[0][0]) * (u[2][1] - u[0][1])
                - (u[2][0] - u[0][0]) * (u[1][1] - u[0][1]))
                .abs();
            if tex > 1.0e-6 && world > 1.0e-3 {
                (world / tex).sqrt().clamp(16.0, 4096.0)
            } else {
                256.0
            }
        };
        let mut got: std::collections::BTreeMap<usize, (usize, f32)> =
            std::collections::BTreeMap::new();
        let mut queue = std::collections::VecDeque::new();
        let open = |t: usize| {
            let si = section_of[t];
            si != usize::MAX && is_ph[si] && !taken.contains(&t)
        };
        let neighbours = |t: usize| -> Vec<usize> {
            let tri = b.triangles[t];
            let mut out = Vec::new();
            for k in 0..3 {
                let (a, c) = (key(pos(b, tri[k])), key(pos(b, tri[(k + 1) % 3])));
                let e = if a <= c { (a, c) } else { (c, a) };
                if let Some(v) = edges.get(&e) {
                    out.extend(v.iter().copied().filter(|&o| o != t));
                }
            }
            out
        };
        for t in 0..b.triangles.len() {
            if !open(t) {
                continue;
            }
            if let Some(o) = neighbours(t).into_iter().find(|&o| real(o)) {
                got.insert(t, (section_of[o], scale_of(o)));
                queue.push_back(t);
            }
        }
        while let Some(t) = queue.pop_front() {
            let look = got[&t];
            for o in neighbours(t) {
                if open(o) && !got.contains_key(&o) {
                    got.insert(o, look);
                    queue.push_back(o);
                }
            }
        }
        // faces cut off from every real face (a backdrop shape on its own) take the real texture that covers the
        // most area in the block (walls and ground, not the small cards of foliage), at its area-weighted scale
        let mut area = vec![(0.0f32, 0.0f32); b.sections.len()];
        for t in (0..b.triangles.len()).filter(|&t| real(t)) {
            let tri = b.triangles[t];
            let p = [pos(b, tri[0]), pos(b, tri[1]), pos(b, tri[2])];
            let a = clip::length(clip::cross(clip::sub(p[1], p[0]), clip::sub(p[2], p[0]))) * 0.5;
            area[section_of[t]].0 += a;
            area[section_of[t]].1 += a * scale_of(t);
        }
        let widest = (0..area.len())
            .filter(|&si| area[si].0 > 0.0)
            .max_by(|&x, &y| area[x].0.total_cmp(&area[y].0));
        if let Some(si) = widest {
            let look = (si, area[si].1 / area[si].0);
            for t in 0..b.triangles.len() {
                if open(t) && !got.contains_key(&t) {
                    got.insert(t, look);
                }
            }
        }
        for (t, (si, scale)) in got {
            let tri = b.triangles[t];
            let q = [pos(b, tri[0]), pos(b, tri[1]), pos(b, tri[2])];
            let Some((n, _)) = tri_plane(&q) else {
                continue;
            };
            // the projection along the plane's main axis (up the texture is up the world on a wall)
            let (su, sv, flip) = if n[1].abs() >= n[0].abs() && n[1].abs() >= n[2].abs() {
                (0, 2, 1.0)
            } else if n[0].abs() >= n[2].abs() {
                (2, 1, -1.0)
            } else {
                (0, 1, -1.0)
            };
            let uv = q.map(|p| [p[su] / scale, flip * p[sv] / scale]);
            moved.push((t, si, uv));
        }
    }
    if moved.is_empty() && dropped.is_empty() && split.is_empty() {
        return 0;
    }
    let gone: std::collections::BTreeSet<usize> = moved
        .iter()
        .map(|m| m.0)
        .chain(dropped.iter().copied())
        .chain(split.iter().map(|x| x.0))
        .collect();
    // the kept sections in order, then one section per partner texture
    let mut triangles = Vec::with_capacity(b.triangles.len());
    let mut sections = Vec::with_capacity(b.sections.len() + 4);
    let mut kept_slots = Vec::with_capacity(b.sections.len());
    for (si, r) in ranges.iter().enumerate() {
        let before = triangles.len();
        triangles.extend(
            r.clone()
                .filter(|t| !gone.contains(t))
                .map(|t| b.triangles[t]),
        );
        let n = triangles.len() - before;
        if n > 0 {
            let mut sec = b.sections[si].clone();
            sec.triangles = n as u32;
            sections.push(sec);
            kept_slots.push(Some(si));
        }
    }
    // the faces that change, by the section whose texture they take: (triangle, the corners of what is drawn)
    type Drawn = Vec<(usize, Vec<[u8; RENDER_VERTEX_BYTES]>)>;
    let mut by_partner: std::collections::BTreeMap<usize, Drawn> =
        std::collections::BTreeMap::new();
    for (t, ps, uv) in moved {
        let tri = b.triangles[t];
        let corners = [tri[0], tri[1], tri[2]]
            .iter()
            .enumerate()
            .map(|(k, &vi)| {
                let mut v = b.vertices[vi as usize];
                put_f32(&mut v, 12, uv[k][0]);
                put_f32(&mut v, 16, uv[k][1]);
                v
            })
            .collect();
        by_partner.entry(ps).or_default().push((t, corners));
    }
    for (t, ps, corners) in split {
        by_partner.entry(ps).or_default().push((t, corners));
    }
    let count = gone.len();
    for (ps, list) in by_partner {
        let tex = b.sections[ps].textures[0].clone();
        let before = triangles.len();
        for (t, corners) in list {
            let tri = b.triangles[t];
            let base = b.vertices.len() as u32;
            let n = corners.len() as u32;
            b.vertices.extend(corners);
            for k in 1..n.saturating_sub(1) {
                triangles.push([base, base + k, base + k + 1, tri[3]]);
            }
        }
        sections.push(SynthSection {
            textures: [tex, Vec::new()],
            shader_kind: 1,
            triangles: (triangles.len() - before) as u32,
            ..SynthSection::default()
        });
        kept_slots.push(None);
    }
    for g in &mut b.light_groups {
        let old = std::mem::take(&mut g.3);
        g.3 = kept_slots
            .iter()
            .map(|k| k.and_then(|si| old.get(si).cloned()).unwrap_or_default())
            .collect();
    }
    b.triangles = triangles;
    b.sections = sections;
    count
}

/// Bounds of a synthetic block's vertices.
pub fn block_bounds(b: &SynthBlock) -> Option<Aabb> {
    let pts: Vec<V> = b
        .vertices
        .iter()
        .map(|v| [f32_at(v, 0), f32_at(v, 4), f32_at(v, 8)])
        .collect();
    Aabb::of_points(pts.iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vert(p: V, uv: [f32; 2], argb: u32, n: V) -> RVert {
        let mut v = [0u8; RENDER_VERTEX_BYTES];
        for (k, &x) in p.iter().enumerate() {
            put_f32(&mut v, 4 * k, x);
        }
        put_f32(&mut v, 12, uv[0]);
        put_f32(&mut v, 16, uv[1]);
        v[28..32].copy_from_slice(&argb.to_le_bytes());
        for (k, &x) in n.iter().enumerate() {
            put_f32(&mut v, 32 + 4 * k, x);
        }
        RVert(v)
    }

    #[test]
    fn min_light_lifts_dark_texels_and_colours_and_leaves_bright_ones() {
        let mut rgb = vec![10u8; 48];
        rgb[3] = 200;
        let sec = SynthSection {
            shader_kind: 2,
            lightmap_w: 4,
            lightmap_h: 4,
            lightmap: crate::synth::pack_light_grid(&rgb),
            ..SynthSection::default()
        };
        let up = lifted(sec.clone(), 70);
        let got = unpack(&up.lightmap, 16).unwrap();
        assert_eq!(got[0], 70);
        assert_eq!(got[3], 200);
        assert!(got.iter().all(|&c| c >= 70));
        // 0 leaves it as it was, and a short payload is left alone
        assert_eq!(lifted(sec.clone(), 0), sec);
        assert!(unpack(&[0x85, 1, 2], 6).is_none());
        let v = vert([0.0; 3], [0.0; 2], 0x8010_2030, [0.0, 1.0, 0.0]);
        let l = lift_vertex(v.0, 64);
        assert_eq!(&l[28..32], &[64, 64, 64, 0x80]);
    }

    #[test]
    fn a_placeholder_face_takes_its_back_to_back_walls_texture() {
        // a wall at z 0 facing -z (kind 4, real texture, uv = x / 100, y / 100) and its outer face at z 0 facing +z
        // (kind 1, the placeholder), both 200 x 100
        let up = [0.0, 1.0, 0.0];
        let wall = |x: f32, y: f32| vert([x, y, 0.0], [x / 100.0, y / 100.0], 0xFF80_8080, up);
        let outer = |x: f32, y: f32| vert([x, y, 0.0], [0.0, 0.0], 0xFF40_4040, up);
        let mut b = SynthBlock::default();
        // facing -z: (0,0) (0,100) (200,100) seen from -z is counter-clockwise
        for v in [wall(0.0, 0.0), wall(0.0, 100.0), wall(200.0, 100.0)] {
            b.vertices.push(v.0);
        }
        for v in [outer(0.0, 0.0), outer(200.0, 100.0), outer(0.0, 100.0)] {
            b.vertices.push(v.0);
        }
        b.triangles = vec![[0, 1, 2, 0], [3, 4, 5, 1]];
        b.sections = vec![
            SynthSection {
                textures: [b"real/wall".to_vec(), Vec::new()],
                shader_kind: 4,
                triangles: 1,
                ..SynthSection::default()
            },
            SynthSection {
                textures: [b"tiny/grey".to_vec(), Vec::new()],
                shader_kind: 1,
                triangles: 1,
                ..SynthSection::default()
            },
        ];
        let n = retexture(&mut b, &mut |t| t == b"tiny/grey");
        assert_eq!(n, 1);
        assert_eq!(b.sections.len(), 2);
        assert_eq!(b.sections[1].textures[0], b"real/wall");
        assert_eq!(b.sections[1].shader_kind, 1);
        let t = b.triangles[1];
        let v = b.vertices[t[1] as usize];
        // the corner at (200, 100) maps to uv (2, 1) by the wall's own mapping
        assert!((f32_at(&v, 12) - 2.0).abs() < 1e-4 && (f32_at(&v, 16) - 1.0).abs() < 1e-4);
        // its colour is kept
        assert_eq!(v[28], 0x40);
        // nothing to do without a placeholder
        assert_eq!(retexture(&mut b, &mut |_| false), 0);
    }

    #[test]
    fn a_placeholder_face_on_a_real_floor_is_dropped() {
        // a real floor (kind 4) and a placeholder face on the same plane facing the same way, inside it
        let up = [0.0, 1.0, 0.0];
        let v = |x: f32, z: f32| vert([x, 0.0, z], [x / 100.0, z / 100.0], 0xFF80_8080, up).0;
        let mut b = SynthBlock {
            vertices: vec![
                v(0.0, 0.0),
                v(0.0, 400.0),
                v(400.0, 0.0),
                v(50.0, 50.0),
                v(50.0, 150.0),
                v(150.0, 50.0),
            ],
            triangles: vec![[0, 1, 2, 0], [3, 4, 5, 1]],
            sections: vec![
                SynthSection {
                    textures: [b"real/floor".to_vec(), Vec::new()],
                    shader_kind: 4,
                    triangles: 1,
                    ..SynthSection::default()
                },
                SynthSection {
                    textures: [b"tiny/grey".to_vec(), Vec::new()],
                    shader_kind: 1,
                    triangles: 1,
                    ..SynthSection::default()
                },
            ],
            ..SynthBlock::default()
        };
        assert_eq!(retexture(&mut b, &mut |t| t == b"tiny/grey"), 1);
        assert_eq!(b.triangles.len(), 1);
        assert_eq!(b.sections.len(), 1);
        assert_eq!(b.sections[0].textures[0], b"real/floor");
    }

    #[test]
    fn a_placeholder_slope_takes_the_texture_of_the_face_it_shares_an_edge_with() {
        // a real ridge top (kind 1, 100 units a repeat) and two placeholder faces down its far side, one sharing an
        // edge with the top, the other only with the first
        let up = [0.0, 1.0, 0.0];
        let v = |p: V, uv: [f32; 2]| vert(p, uv, 0xFF66_6666, up).0;
        let mut b = SynthBlock {
            vertices: vec![
                v([0.0, 100.0, 0.0], [0.0, 0.0]),
                v([0.0, 100.0, 100.0], [0.0, 1.0]),
                v([100.0, 100.0, 0.0], [1.0, 0.0]),
                // the far side: down to y 0 at x -100
                v([-100.0, 0.0, 0.0], [0.0, 0.0]),
                v([-100.0, 0.0, 100.0], [0.0, 0.0]),
            ],
            // the top; then (0,100,0) (-100,0,0) (0,100,100) shares the top's edge (0,100,0)-(0,100,100); then
            // (-100,0,0) (-100,0,100) (0,100,100) shares only the first placeholder's edge
            triangles: vec![[0, 1, 2, 0], [0, 3, 1, 1], [3, 4, 1, 1]],
            sections: vec![
                SynthSection {
                    textures: [b"real/rock".to_vec(), Vec::new()],
                    shader_kind: 1,
                    triangles: 1,
                    ..SynthSection::default()
                },
                SynthSection {
                    textures: [b"tiny/grey".to_vec(), Vec::new()],
                    shader_kind: 1,
                    triangles: 2,
                    ..SynthSection::default()
                },
            ],
            ..SynthBlock::default()
        };
        assert_eq!(retexture(&mut b, &mut |t| t == b"tiny/grey"), 2);
        assert_eq!(b.triangles.len(), 3);
        assert!(b.sections.iter().all(|s| s.textures[0] == b"real/rock"));
        // the projection keeps the top's scale: 100 units a repeat
        let t = b.triangles[2];
        let (p0, p1) = (b.vertices[t[0] as usize], b.vertices[t[1] as usize]);
        let du =
            (f32_at(&p0, 12) - f32_at(&p1, 12)).abs() + (f32_at(&p0, 16) - f32_at(&p1, 16)).abs();
        assert!((du - 1.0).abs() < 1e-3, "{du}");
    }

    #[test]
    fn a_placeholder_shape_on_its_own_takes_the_widest_real_texture() {
        // a wide real wall, a small real card, and a placeholder triangle away from both
        let up = [0.0, 1.0, 0.0];
        let v = |p: V| vert(p, [p[0] / 50.0, p[2] / 50.0], 0xFF66_6666, up).0;
        let mut b = SynthBlock {
            vertices: vec![
                v([0.0, 0.0, 0.0]),
                v([0.0, 0.0, 1000.0]),
                v([1000.0, 0.0, 0.0]),
                v([0.0, 50.0, 0.0]),
                v([0.0, 50.0, 10.0]),
                v([10.0, 50.0, 0.0]),
                v([3000.0, 500.0, 0.0]),
                v([3000.0, 500.0, 100.0]),
                v([3100.0, 600.0, 0.0]),
            ],
            triangles: vec![[0, 1, 2, 0], [3, 4, 5, 1], [6, 7, 8, 2]],
            sections: ["real/wall", "real/card", "tiny/grey"]
                .iter()
                .map(|t| SynthSection {
                    textures: [t.as_bytes().to_vec(), Vec::new()],
                    shader_kind: 1,
                    triangles: 1,
                    ..SynthSection::default()
                })
                .collect(),
            ..SynthBlock::default()
        };
        assert_eq!(retexture(&mut b, &mut |t| t == b"tiny/grey"), 1);
        assert_eq!(b.sections.last().unwrap().textures[0], b"real/wall");
        assert!(b.sections.iter().all(|s| s.textures[0] != b"tiny/grey"));
    }

    #[test]
    fn a_placeholder_face_over_two_textures_is_cut_along_them() {
        // the inside of a wall at z 0 faces -z: brick under y 100, concrete over it (two triangles each); the
        // outer face (+z) is one placeholder triangle across both
        let n = [0.0, 0.0, 1.0];
        let v = |x: f32, y: f32| vert([x, y, 0.0], [x / 100.0, y / 100.0], 0xFF50_5050, n).0;
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        let mut rect = |y0: f32, y1: f32, vertices: &mut Vec<[u8; RENDER_VERTEX_BYTES]>| {
            let base = vertices.len() as u32;
            vertices.extend([v(0.0, y0), v(0.0, y1), v(200.0, y1), v(200.0, y0)]);
            triangles.push([base, base + 1, base + 2, 0]);
            triangles.push([base, base + 2, base + 3, 0]);
        };
        rect(0.0, 100.0, &mut vertices);
        rect(100.0, 200.0, &mut vertices);
        let base = vertices.len() as u32;
        vertices.extend([v(0.0, 0.0), v(200.0, 0.0), v(0.0, 200.0)]);
        triangles.push([base, base + 1, base + 2, 0]);
        let mut b = SynthBlock {
            vertices,
            triangles,
            sections: [
                ("real/brick", 4u8, 2u32),
                ("real/concrete", 4, 2),
                ("tiny/grey", 1, 1),
            ]
            .iter()
            .map(|&(t, k, n)| SynthSection {
                textures: [t.as_bytes().to_vec(), Vec::new()],
                shader_kind: k,
                triangles: n,
                ..SynthSection::default()
            })
            .collect(),
            ..SynthBlock::default()
        };
        assert_eq!(retexture(&mut b, &mut |t| t == b"tiny/grey"), 1);
        // the two walls, then a section per texture for the pieces
        let names: Vec<&[u8]> = b.sections.iter().map(|s| &s.textures[0][..]).collect();
        assert_eq!(
            names,
            vec![
                &b"real/brick"[..],
                b"real/concrete",
                b"real/brick",
                b"real/concrete"
            ]
        );
        let mut first = 4usize;
        let mut area = 0.0f32;
        for (si, sec) in b.sections.iter().enumerate().skip(2) {
            for t in first..first + sec.triangles as usize {
                let tri = b.triangles[t];
                let p = [tri[0], tri[1], tri[2]].map(|i| {
                    let x = &b.vertices[i as usize];
                    [f32_at(x, 0), f32_at(x, 4), f32_at(x, 8)]
                });
                // a piece stays on its own side of y 100
                let (lo, hi) = (
                    p.iter().map(|q| q[1]).fold(f32::MAX, f32::min),
                    p.iter().map(|q| q[1]).fold(f32::MIN, f32::max),
                );
                if si == 2 {
                    assert!(hi <= 100.01, "{p:?}");
                } else {
                    assert!(lo >= 99.99, "{p:?}");
                }
                area += poly_area(&p);
            }
            first += sec.triangles as usize;
        }
        assert!((area - 20_000.0).abs() < 1.0, "{area}");
    }

    #[test]
    fn a_cut_vertex_takes_the_attributes_between_its_ends() {
        let a = vert([0.0, 0.0, 0.0], [0.0, 0.0], 0xFF00_0000, [0.0, 1.0, 0.0]);
        let b = vert([100.0, 0.0, 0.0], [1.0, 0.5], 0xFFFF_FFFF, [1.0, 0.0, 0.0]);
        let m = a.lerp(&b, 0.25);
        assert_eq!(m.pos(), [25.0, 0.0, 0.0]);
        assert_eq!(f32_at(&m.0, 12), 0.25);
        assert_eq!(f32_at(&m.0, 16), 0.125);
        assert_eq!(m.0[28], 64);
        assert_eq!(m.0[31], 255);
        let n = [f32_at(&m.0, 32), f32_at(&m.0, 36), f32_at(&m.0, 40)];
        assert!((clip::length(n) - 1.0).abs() < 1e-5);
    }
}
