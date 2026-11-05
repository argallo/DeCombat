//! Kill volumes on the server (DSV-52): the closed model of each kill volume of the world file as a point-inside
//! test, and the delay its pending property asks for. The session kills a living player whose position lies inside
//! one of the kill volumes it created on the clients. The hazard volumes (DSV-607) use the same test and hurt a
//! living player inside them by their damage property per second, of their damage kind.
// SPEC: FMT_DAT85.md §5 (BSP model polygons and planes, stored in place), §6.2 (the kill volume role), §6.4 (model
// link by name)
// OBSERVED: the ROM worlds' kill volumes are closed six-polygon boxes (the rust3 / tdm6 world: 38 of 38; scratch run
// over dc-world 2026-10-25)
// DC-DEFAULT: DSV-52 (proposed)

use dc_world::{ObjectRole, World};

/// One polygon of a volume: plane and corners.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeFace {
    pub normal: [f32; 3],
    pub dist: f32,
    pub corners: Vec<[f32; 3]>,
}

/// The closed model of a kill volume.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeShape {
    pub name: Vec<u8>,
    pub lo: [f32; 3],
    pub hi: [f32; 3],
    pub faces: Vec<VolumeFace>,
    /// Time a player must stay inside before it dies (the volume's pending property; 0 = at once).
    pub pending_ms: u32,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// A ray direction off every axis and diagonal, so a ray does not run along the edges of a box volume.
const RAY: [f32; 3] = [0.000_37, 1.0, 0.000_71];

impl VolumeShape {
    /// Whether the point lies inside the closed model: inside the box, and an odd number of the model's polygons cross
    /// the ray from the point.
    pub fn contains(&self, p: [f32; 3]) -> bool {
        if !(0..3).all(|i| p[i] >= self.lo[i] && p[i] <= self.hi[i]) {
            return false;
        }
        let mut crossings = 0u32;
        for f in &self.faces {
            let den = dot(f.normal, RAY);
            if den.abs() < 1e-9 || f.corners.len() < 3 {
                continue;
            }
            let t = (f.dist - dot(f.normal, p)) / den;
            if t <= 0.0 {
                continue;
            }
            let q = [p[0] + RAY[0] * t, p[1] + RAY[1] * t, p[2] + RAY[2] * t];
            // inside the convex polygon: the same side of every edge (either winding)
            let n = f.corners.len();
            let mut pos = false;
            let mut neg = false;
            for k in 0..n {
                let a = f.corners[k];
                let b = f.corners[(k + 1) % n];
                let s = dot(cross(sub(b, a), sub(q, a)), f.normal);
                if s > 0.0 {
                    pos = true;
                } else if s < 0.0 {
                    neg = true;
                }
            }
            if !(pos && neg) {
                crossings += 1;
            }
        }
        crossings % 2 == 1
    }
}

/// The polygons of a volume's model as faces of the point-inside test.
fn model_faces(m: &dc_world::BspModel<'_>) -> Vec<VolumeFace> {
    m.polygons
        .iter()
        .filter_map(|p| {
            let pl = m.planes.get(p.plane as usize)?;
            Some(VolumeFace {
                normal: pl.normal,
                dist: pl.dist,
                corners: m
                    .polygon_points(p)
                    .iter()
                    .filter_map(|&i| m.points.get(i as usize).copied())
                    .collect(),
            })
        })
        .collect()
}

/// The kill volumes of a parsed world with their models.
pub fn kill_shapes(w: &World<'_>) -> Vec<VolumeShape> {
    let mut out = Vec::new();
    for o in w.objects_with_role(ObjectRole::KillVolume) {
        let Some(m) = w.brush_of(o) else {
            continue;
        };
        let faces = model_faces(m);
        // the pending property by ROM hash or its vocabulary name `pending_sec` (MAP-20)
        let pending = o
            .prop_named(&dc_world::names::PROP_PENDING_SEC)
            .and_then(|p| match p.value {
                dc_world::FieldValue::Real(r) if r.is_finite() && r > 0.0 => Some(r),
                _ => None,
            })
            .unwrap_or(0.0);
        out.push(VolumeShape {
            name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
            lo: m.box_min,
            hi: m.box_max,
            faces,
            pending_ms: (pending * 1000.0).min(u32::MAX as f32) as u32,
        });
    }
    out
}

/// The damage type a volume's damage-kind text names: the id of the client's damage-type row whose name it is, for
/// the kinds the ROM worlds' hurting volumes carry (poison); any other text, or none, is the unspecified type 0.
// EVIDENCE: cshell+0x282b60, cshell+0x282c20 (the client's lookups over its damage-type table at rva 0x457498: rows
// of 0x28 bytes, the id at +0, the name at +0x10; the row whose name hashes to `names::DAMAGE_KIND_POISON` has id 9)
// OBSERVED: the ROM worlds' hazard volumes all carry that poison text (2026-11-05 scratch survey over dc-world: 21
// worlds, 20 hazard volumes); kill volumes carry the endless-fall text, ladders and water the unspecified one
pub fn damage_type_of(kind: Option<&[u8]>) -> u8 {
    match kind {
        Some(t) if dc_world::names::DAMAGE_KIND_POISON.matches(t) => DAMAGE_TYPE_POISON,
        _ => DAMAGE_TYPE_UNSPECIFIED,
    }
}

/// The client's unspecified damage type (row 0 of its damage-type table).
// EVIDENCE: cshell+0x282b60 (row 0 of the table at rva 0x457498)
pub const DAMAGE_TYPE_UNSPECIFIED: u8 = 0;
/// The client's poison damage type.
// EVIDENCE: cshell+0x282b60 (row 9 of the table at rva 0x457498)
pub const DAMAGE_TYPE_POISON: u8 = 9;

/// A hazard volume: its closed model, the damage it does per second to a player inside and the damage type.
#[derive(Debug, Clone, PartialEq)]
pub struct HazardShape {
    pub shape: VolumeShape,
    pub damage_per_sec: f32,
    pub dtype: u8,
}

/// The hazard volumes of a parsed world that hurt (a finite damage property above 0), with their models. The
/// damage property is read as damage per second (DSV-607).
// SPEC: FMT_DAT85.md §6.2 (the hazard role, brush, name-linked), §6.4 (model link by name)
// OBSERVED: ROM worlds (2026-11-05 scratch survey: the 20 hazard volumes of 5 worlds carry damage 2..10000 and the
// poison kind; WarHead's and Two Towers' cover their water)
// DC-DEFAULT: DSV-607 (proposed)
pub fn hazard_shapes(w: &World<'_>) -> Vec<HazardShape> {
    let mut out = Vec::new();
    for o in w.objects_with_role(ObjectRole::HazardVolume) {
        let Some(m) = w.brush_of(o) else {
            continue;
        };
        let damage = match o.prop_named(&dc_world::names::PROP_DAMAGE).map(|p| p.value) {
            Some(dc_world::FieldValue::Real(r)) if r.is_finite() && r > 0.0 => r,
            _ => continue,
        };
        let kind = match o
            .prop_named(&dc_world::names::PROP_DAMAGE_KIND)
            .map(|p| p.value)
        {
            Some(dc_world::FieldValue::Text(t)) => Some(t),
            _ => None,
        };
        out.push(HazardShape {
            shape: VolumeShape {
                name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
                lo: m.box_min,
                hi: m.box_max,
                faces: model_faces(m),
                pending_ms: 0,
            },
            damage_per_sec: damage,
            dtype: damage_type_of(kind),
        });
    }
    out
}

/// A box volume (tests and tools): six faces of the box from `lo` to `hi`.
pub fn box_shape(name: &[u8], lo: [f32; 3], hi: [f32; 3], pending_ms: u32) -> VolumeShape {
    let face = |normal: [f32; 3], dist: f32, corners: [[f32; 3]; 4]| VolumeFace {
        normal,
        dist,
        corners: corners.to_vec(),
    };
    let (a, b) = (lo, hi);
    VolumeShape {
        name: name.to_vec(),
        lo,
        hi,
        faces: vec![
            face(
                [0.0, 1.0, 0.0],
                b[1],
                [
                    [a[0], b[1], a[2]],
                    [a[0], b[1], b[2]],
                    [b[0], b[1], b[2]],
                    [b[0], b[1], a[2]],
                ],
            ),
            face(
                [0.0, -1.0, 0.0],
                -a[1],
                [
                    [a[0], a[1], a[2]],
                    [b[0], a[1], a[2]],
                    [b[0], a[1], b[2]],
                    [a[0], a[1], b[2]],
                ],
            ),
            face(
                [1.0, 0.0, 0.0],
                b[0],
                [
                    [b[0], a[1], a[2]],
                    [b[0], b[1], a[2]],
                    [b[0], b[1], b[2]],
                    [b[0], a[1], b[2]],
                ],
            ),
            face(
                [-1.0, 0.0, 0.0],
                -a[0],
                [
                    [a[0], a[1], a[2]],
                    [a[0], a[1], b[2]],
                    [a[0], b[1], b[2]],
                    [a[0], b[1], a[2]],
                ],
            ),
            face(
                [0.0, 0.0, 1.0],
                b[2],
                [
                    [a[0], a[1], b[2]],
                    [b[0], a[1], b[2]],
                    [b[0], b[1], b[2]],
                    [a[0], b[1], b[2]],
                ],
            ),
            face(
                [0.0, 0.0, -1.0],
                -a[2],
                [
                    [a[0], a[1], a[2]],
                    [a[0], b[1], a[2]],
                    [b[0], b[1], a[2]],
                    [b[0], a[1], a[2]],
                ],
            ),
        ],
        pending_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_inside_outside() {
        let b = box_shape(b"Pit", [-10.0, -5.0, -10.0], [10.0, 5.0, 10.0], 0);
        assert!(b.contains([0.0, 0.0, 0.0]));
        assert!(b.contains([9.9, 4.9, -9.9]));
        assert!(!b.contains([0.0, 6.0, 0.0]));
        assert!(!b.contains([11.0, 0.0, 0.0]));
        // an L of two boxes is not convex: a point under the notch is outside even inside the joint box
        let mut l = box_shape(b"L", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0], 0);
        l.lo = [0.0, 0.0, 0.0];
        l.hi = [10.0, 20.0, 10.0];
        assert!(!l.contains([5.0, 15.0, 5.0]));
    }

    /// DSV-607: the damage-kind text names the poison type (the vocabulary's own word too); anything else, or none,
    /// is the unspecified type.
    #[test]
    fn damage_kind_texts_map_to_the_client_types() {
        assert_eq!(damage_type_of(Some(b"poison")), DAMAGE_TYPE_POISON);
        assert_eq!(damage_type_of(Some(b"Poison")), DAMAGE_TYPE_POISON);
        assert_eq!(damage_type_of(Some(b"lava")), DAMAGE_TYPE_UNSPECIFIED);
        assert_eq!(damage_type_of(Some(b"")), DAMAGE_TYPE_UNSPECIFIED);
        assert_eq!(damage_type_of(None), DAMAGE_TYPE_UNSPECIFIED);
    }
}
