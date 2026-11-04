//! Model lighting inputs: the static lights of a world (its light objects), the ambient light-grid sample, and
//! the per-model light set (ambient plus the strongest lights, the weaker ones folded into the ambient).
//!
//! Evidence: RENDER_2009.md §6.3 (ambient from the light grid plus point lights with attenuation
//! `1 / (1 + 19 d^2 / r^2)`, at most 8 lights, `model_light_limit` 4) and the decompiler output / disassembly of the
//! routines named there; property names and light classes are the keys of `dc_world::names` (the ROM name by
//! `sha256[:16]` and length, or DeCombat's own `dc.point_light` / `dc.global_light` names, MAP-2).

use dc_world::names::{
    ATT_D3D, ATT_LINEAR, ATT_QUARTIC, CLASS_DIRECTIONAL_LIGHT, CLASS_GLOBAL_LIGHT,
    CLASS_OBJECT_LIGHT, CLASS_POINT_LIGHT, PROP_ATTENUATION, PROP_ATT_COEFS, PROP_ATT_EXPS,
    PROP_BRIGHT, PROP_CONE, PROP_FAST_LIGHTS_OBJECTS, PROP_LIGHTS_OBJECTS, PROP_LIGHT_COLOR,
    PROP_LIGHT_INNER_COLOR, PROP_OBJECT_BRIGHT, PROP_RADIUS, PROP_TO_AMBIENT,
};
use dc_world::{FieldValue, Key, LightGrid, LightGroupGrid, Object, World};
use glam::{Quat, Vec3};

/// Whether `name` is `key`: the ROM name by hash and length, or the vocabulary name ([`Key::matches`]).
// DC-DEFAULT: MAP-26 (the renderer reads the vocabulary names through `Key::matches`; the ROM-only keys named
// below keep their hash-only match because their `ours` is empty)
fn is(name: &[u8], key: &Key) -> bool {
    key.matches(name)
}

// Classes the client's light-object scan creates static lights for (case-sensitive compares):
// `CLASS_POINT_LIGHT` (ours `dc.point_light`), `CLASS_DIRECTIONAL_LIGHT` and `CLASS_OBJECT_LIGHT` (ROM-only).
// EVIDENCE: engine+0xcea90 (client object-list scan for light objects; three class-name compares)
//
// Properties the scan reads (type in brackets): radius (real, `PROP_RADIUS`), convert-to-ambient fraction (real,
// `PROP_TO_AMBIENT`), bright scale (real, `PROP_BRIGHT`), object bright scale (real, `PROP_OBJECT_BRIGHT`,
// ROM-only), cone angle in degrees (real, `PROP_CONE`), colour and inner colour (colour, `PROP_LIGHT_COLOR` ours
// `color`, `PROP_LIGHT_INNER_COLOR` ROM-only), attenuation coefficients and exponents (vec3, ROM-only), position
// (vec3), lights-objects / fast-lights-objects / cast-shadows (bool; `PROP_LIGHTS_OBJECTS` ours, the fast flag
// ROM-only), rotation (euler), light group (text), attenuation kind (text, compared without case, ROM-only, with
// the three ROM-only values `ATT_D3D` / `ATT_QUARTIC` / `ATT_LINEAR`: kinds 0 / 2 / 1).
// EVIDENCE: engine+0xcea90 (property-name compares per property type; three case-insensitive value compares)
//
// Class of the world's global directional light object (`CLASS_GLOBAL_LIGHT`, ours `dc.global_light`): one per
// world on 19 of the 21 ROM worlds, with exactly the inputs the server's `S2C_DIR_LIGHT` writer reads from its
// global light (rotation, colour, bright scale, object bright scale, convert-to-ambient) plus name / position / an
// int.
// OBSERVED: ROM-gated test `rom_global_light_and_light_group_grids` (class 7cea843ab1383e94/14: 19 objects on 19
// maps, property set {name, position, euler rotation, int, colour 9a072808beba8a7a/10, bright scale, object
// bright scale, convert-to-ambient}); SPEC: UDP_SESSION_2009_B.md §8.4 (writer engine+0x8d6c0: rotated +Z of the
// light's rotation, colour x two scalar properties, one further float); EVIDENCE: engine+0x9c030 /
// engine+0x97020 (the game module registers the light through server interface slot +0x1c8; the class it takes
// it from is in the server game module, which the 2009 client ROM does not contain: the class choice is
// PLAUSIBLE)

/// Falloff of a light record.
// EVIDENCE: engine+0x1ba460 (contribution of a light record at a point)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Falloff {
    /// `1 / (c0 + c1 d + c2 d^2)` inside the radius.
    D3d([f32; 3]),
    /// `1 - d / r`.
    Linear,
    /// `(1 - d^2 / r^2)^2`.
    Quartic,
}

/// What kind of light a record is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LightShape {
    /// No position: the colour applies along `dir` (the direction the light travels).
    Directional {
        dir: Vec3,
    },
    Point,
    /// Cone around `dir` with `cos_half` = cosine of the half angle.
    Spot {
        dir: Vec3,
        cos_half: f32,
    },
}

/// One light record as the model lighting uses it. Colour is 0..255 per channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelLight {
    pub shape: LightShape,
    pub pos: Vec3,
    pub radius: f32,
    pub colour: Vec3,
    pub falloff: Falloff,
    /// Fraction of the contribution that goes to the ambient term instead (static lights' convert-to-ambient).
    pub to_ambient: f32,
}

/// Coefficients of the dynamic-light law: attenuation `(1, 0, 19 / r^2)`, i.e. `1 / (1 + 19 d^2 / r^2)`.
// SPEC: RENDER_2009.md §6.3 (attenuation (1, 0, 19 / radius^2)); EVIDENCE: engine+0x1be930, engine+0x1b21b0
// (19.0 / (r * r) built for every dynamic light)
pub fn dynamic_coefs(radius: f32) -> [f32; 3] {
    let r2 = (radius * radius).max(1e-6);
    [1.0, 0.0, 19.0 / r2]
}

impl ModelLight {
    /// The global directional ("sun") light: `dir` is the direction the light travels, `colour` 0..255.
    ///
    /// The engine adds it to a model's light set when `model_sun` is on and the global light colour is not black,
    /// scaled by a visibility factor of the model against the sun (engine+0x1b21b0, engine+0x1b1cc0 with
    /// `model_sun_variance`); the colour and direction come from the server's `S2C_DIR_LIGHT`
    /// (UDP_SESSION_2009_B.md §8.4). The visibility factor is 1 (no sun trace yet).
    // DC-DEFAULT: REN-7 (proposed), see DC_DEFAULTS.md (sun visibility 1; the engine traces toward the sun,
    // engine+0x1b1ba0)
    pub fn sun(dir: Vec3, colour: Vec3) -> ModelLight {
        ModelLight {
            shape: LightShape::Directional {
                dir: dir.normalize_or_zero(),
            },
            pos: Vec3::ZERO,
            radius: f32::MAX,
            colour,
            falloff: Falloff::D3d([1.0, 0.0, 0.0]),
            to_ambient: 0.0,
        }
    }

    /// The sun with the global light's convert-to-ambient fraction (the `extra` float of `S2C_DIR_LIGHT`).
    // EVIDENCE: engine+0x1b21b0 (the sun record is inserted with the fraction at world interface +0x160, the
    // value engine+0xa31f0 hands to method +0x154)
    pub fn sun_with(dir: Vec3, colour: Vec3, to_ambient: f32) -> ModelLight {
        ModelLight {
            to_ambient: if to_ambient.is_finite() {
                to_ambient.clamp(0.0, 1.0)
            } else {
                0.0
            },
            ..ModelLight::sun(dir, colour)
        }
    }

    /// A dynamic point light (the law of RENDER_2009.md §6.3).
    pub fn dynamic(pos: Vec3, radius: f32, colour: Vec3) -> ModelLight {
        ModelLight {
            shape: LightShape::Point,
            pos,
            radius,
            colour,
            falloff: Falloff::D3d(dynamic_coefs(radius)),
            to_ambient: 0.0,
        }
    }

    /// Attenuation at squared distance `d2`; zero outside the radius.
    // EVIDENCE: engine+0x1ba460 (inside `d^2 < r^2`: kind 0 `1/(c0 + c1 d + c2 d^2)`, kind 1 `1 - d/r`,
    // kind 2 `(1 - d^2/r^2)^2`)
    pub fn attenuation(&self, d2: f32) -> f32 {
        let r = self.radius;
        // NaN distances or radii count as outside
        if d2.partial_cmp(&(r * r)) != Some(std::cmp::Ordering::Less) {
            return 0.0;
        }
        let d = d2.sqrt();
        match self.falloff {
            Falloff::D3d([c0, c1, c2]) => {
                let den = c0 + c1 * d + c2 * d2;
                if den > 1e-12 {
                    1.0 / den
                } else {
                    0.0
                }
            }
            Falloff::Linear => (1.0 - d / r).max(0.0),
            Falloff::Quartic => {
                let t = 1.0 - d2 / (r * r);
                t * t
            }
        }
    }

    /// Colour this light gives a point `p` (no surface normal): the ranking measure of the light set.
    // EVIDENCE: engine+0x1ba460 (directional: the colour; point: colour x attenuation; spot: also x
    // (cos - cos_half) / (1 - cos_half) when inside the cone)
    pub fn contribution(&self, p: Vec3) -> Vec3 {
        match self.shape {
            LightShape::Directional { .. } => self.colour,
            LightShape::Point => self.colour * self.attenuation(p.distance_squared(self.pos)),
            LightShape::Spot { dir, cos_half } => {
                let v = p - self.pos;
                let d2 = v.length_squared();
                let a = self.attenuation(d2);
                if a <= 0.0 || d2 <= 0.0 {
                    return Vec3::ZERO;
                }
                let c = dir.dot(v) / d2.sqrt();
                if c > cos_half && cos_half < 1.0 {
                    self.colour * a * ((c - cos_half) / (1.0 - cos_half))
                } else {
                    Vec3::ZERO
                }
            }
        }
    }
}

/// Case-insensitive match of a value against a hashed key: the value as stored, upper case, lower case and
/// capitalised (the engine compares these values without case).
fn is_folded(v: &[u8], key: &Key) -> bool {
    let low = v.to_ascii_lowercase();
    let mut cap = low.clone();
    if let Some(c) = cap.first_mut() {
        *c = c.to_ascii_uppercase();
    }
    [v.to_vec(), v.to_ascii_uppercase(), low, cap]
        .iter()
        .any(|x| is(x, key))
}

fn prop<'o>(o: &'o Object<'_>, key: &Key) -> Option<&'o FieldValue<'o>> {
    o.prop_named(key).map(|p| &p.value)
}

fn real(o: &Object<'_>, key: &Key) -> Option<f32> {
    match prop(o, key)? {
        FieldValue::Real(v) => Some(*v),
        _ => None,
    }
}

fn vec(o: &Object<'_>, key: &Key) -> Option<Vec3> {
    match prop(o, key)? {
        FieldValue::Vec3(v) | FieldValue::Rgb(v) => Some(Vec3::from(*v)),
        _ => None,
    }
}

fn flag(o: &Object<'_>, key: &Key) -> Option<bool> {
    match prop(o, key)? {
        FieldValue::Bool(b) => Some(*b),
        _ => None,
    }
}

/// Which light class an object is, if any (0 point, 1 directional, 2 object light; `dc.point_light` is 0).
pub fn light_class(o: &Object<'_>) -> Option<u8> {
    if is(o.class_name, &CLASS_POINT_LIGHT) {
        Some(0)
    } else if is(o.class_name, &CLASS_DIRECTIONAL_LIGHT) {
        Some(1)
    } else if is(o.class_name, &CLASS_OBJECT_LIGHT) {
        Some(2)
    } else {
        None
    }
}

/// Rotation of an object's euler property, with the same composition as the viewer camera
/// ([`crate::Camera::from_euler`]; the composition order is UNPROVEN, RENDER_2009.md §16 item 19).
fn euler_quat(e: [f32; 3]) -> Quat {
    Quat::from_rotation_y(e[1]) * Quat::from_rotation_x(e[0]) * Quat::from_rotation_z(e[2])
}

/// The static light the client creates for a light object, `None` when the object is not a light or does not
/// light objects (models).
///
/// Rules (EVIDENCE engine+0xcea90 unless noted):
/// - only when the lights-objects flag is true (the record is created only then), and not for the fast
///   variant (records with the fast flag are not created by this scan);
/// - colour = colour property (inner colour as alternative) x object bright scale x bright scale; DeCombat: a
///   missing scale counts as 1;
/// - attenuation coefficients `c_i = coef_i * radius ^ exp_i` (DISASM engine+0xcf10f..0xcf180: three `pow`
///   calls on the radius with the exponents, times the coefficients; the base being the radius is our reading
///   of the stack slot, which the record later stores as the radius); missing coefficients: the dynamic law;
/// - the kind text selects D3D / linear / quartic; DeCombat: absent = D3D;
/// - a cone angle gives `cos(angle / 2)` (constant pi/360 in the code); the record is a point light when that
///   cosine is <= -0.99, else a spot light along the object's +Z (engine+0x1b1800);
/// - position = the object position.
///
/// A `dc.point_light` of a community map takes the same path: `lights_objects` true and `radius` > 0 are
/// required (docs/MAPKIT_VOCAB.md), `color` is the colour, `bright` the scale, `cone` the angle, `to_ambient`
/// the fraction; the attenuation properties are ROM-only, so it always gets the dynamic law
/// (`1 / (1 + 19 d^2 / r^2)`).
// DC-DEFAULT: MAP-26
pub fn static_light(o: &Object<'_>) -> Option<ModelLight> {
    light_class(o)?;
    if flag(o, &PROP_LIGHTS_OBJECTS) != Some(true)
        || flag(o, &PROP_FAST_LIGHTS_OBJECTS) == Some(true)
    {
        return None;
    }
    let pos = Vec3::from(o.position()?);
    let radius = real(o, &PROP_RADIUS).unwrap_or(0.0);
    if radius.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let scale = real(o, &PROP_OBJECT_BRIGHT).unwrap_or(1.0) * real(o, &PROP_BRIGHT).unwrap_or(1.0);
    // read order kept: the colour first, the inner colour as the alternative
    let colour = vec(o, &PROP_LIGHT_COLOR)
        .or_else(|| vec(o, &PROP_LIGHT_INNER_COLOR))
        .unwrap_or(Vec3::splat(255.0))
        * scale;
    let kind = match prop(o, &PROP_ATTENUATION) {
        Some(FieldValue::Text(t)) => [(&ATT_QUARTIC, 2), (&ATT_LINEAR, 1), (&ATT_D3D, 0)]
            .iter()
            .find(|(k, _)| is_folded(t, k))
            .map_or(0, |x| x.1),
        _ => 0,
    };
    let falloff = match kind {
        1 => Falloff::Linear,
        2 => Falloff::Quartic,
        _ => match (vec(o, &PROP_ATT_COEFS), vec(o, &PROP_ATT_EXPS)) {
            (Some(c), Some(e)) => {
                let c = [0, 1, 2].map(|i| c[i] * radius.powf(e[i]));
                if c.iter().all(|v| v.is_finite()) {
                    Falloff::D3d(c)
                } else {
                    Falloff::D3d(dynamic_coefs(radius))
                }
            }
            _ => Falloff::D3d(dynamic_coefs(radius)),
        },
    };
    let rot = o.rotation().map(euler_quat).unwrap_or(Quat::IDENTITY);
    let dir = rot * Vec3::Z;
    let shape = match real(o, &PROP_CONE) {
        Some(deg) => {
            let c = (deg * std::f32::consts::PI / 360.0).cos();
            if c <= -0.99 {
                LightShape::Point
            } else {
                LightShape::Spot { dir, cos_half: c }
            }
        }
        None => LightShape::Point,
    };
    Some(ModelLight {
        shape,
        pos,
        radius,
        colour,
        falloff,
        to_ambient: real(o, &PROP_TO_AMBIENT).unwrap_or(0.0).clamp(0.0, 1.0),
    })
}

/// The global directional light of a world as the game server sends it (`S2C_DIR_LIGHT`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobalLight {
    /// Direction the light travels: the object rotation's +Z axis.
    pub dir: Vec3,
    /// Colour x bright scale x object bright scale (0..255 units, as the static lights).
    pub colour: Vec3,
    /// Convert-to-ambient fraction (the message's `extra`).
    pub to_ambient: f32,
}

impl GlobalLight {
    /// The model light the client builds from it.
    pub fn light(&self) -> ModelLight {
        ModelLight::sun_with(self.dir, self.colour, self.to_ambient)
    }
}

/// The world's global light object, if it has one (`CLASS_GLOBAL_LIGHT`, ours `dc.global_light`): direction =
/// rotated +Z of its euler rotation (the same composition as the other light objects), colour = its colour
/// property x bright scale x object bright scale (missing scales count as 1), convert-to-ambient from its
/// property (missing: 0). This reader takes the inner colour first and the colour as the alternative (its ROM
/// order, kept); a `dc.global_light` carries `color` only, so it lands on the alternative and both readers of the
/// tree see the same value.
// SPEC: UDP_SESSION_2009_B.md §8.4 (dir = rotated +Z, colour x brightness x scale, extra); OBSERVED: the class's
// property set (`style_census` world pass, ROM-gated test `rom_global_light_and_light_group_grids`);
// DC-DEFAULT: REN-11 (proposed), see DC_DEFAULTS.md (this object is the sun when no server sends one); MAP-26
pub fn global_light(w: &World<'_>) -> Option<GlobalLight> {
    let o = w
        .objects
        .iter()
        .find(|o| is(o.class_name, &CLASS_GLOBAL_LIGHT))?;
    let rot = o.rotation().map(euler_quat).unwrap_or(Quat::IDENTITY);
    let scale = real(o, &PROP_OBJECT_BRIGHT).unwrap_or(1.0) * real(o, &PROP_BRIGHT).unwrap_or(1.0);
    let colour = vec(o, &PROP_LIGHT_INNER_COLOR)
        .or_else(|| vec(o, &PROP_LIGHT_COLOR))
        .unwrap_or(Vec3::splat(255.0))
        * scale;
    Some(GlobalLight {
        dir: (rot * Vec3::Z).normalize_or_zero(),
        colour,
        to_ambient: real(o, &PROP_TO_AMBIENT).unwrap_or(0.0),
    })
}

/// Number of global light objects in a world (census).
pub fn global_light_count(w: &World<'_>) -> usize {
    w.objects
        .iter()
        .filter(|o| is(o.class_name, &CLASS_GLOBAL_LIGHT))
        .count()
}

/// Every static light of a world.
pub fn world_lights(w: &World<'_>) -> Vec<ModelLight> {
    w.objects.iter().filter_map(static_light).collect()
}

/// One light group's grid as the model ambient adds it: cells in the main grid's indices, one byte per cell.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupGrid {
    pub offset: [i32; 3],
    pub dims: [u32; 3],
    /// Group colour x intensity, per channel in [0, 1].
    pub colour: Vec3,
    /// `x * y * z` bytes, x fastest, then y, then z.
    pub data: Vec<u8>,
}

impl GroupGrid {
    /// Value the group adds at main-grid cell `c`, zero outside its box.
    // EVIDENCE: engine+0x4e370 (colour_c x byte at data[x + y * dims.x + z * dims.x * dims.y]), engine+0x4f6a0
    // (the strides: dims.x for y, dims.x * dims.y for z; the box = offset .. offset + dims)
    pub fn at(&self, c: [i64; 3]) -> Vec3 {
        let mut idx = 0usize;
        let mut stride = 1usize;
        for ((ck, ok), dk) in c.iter().zip(self.offset).zip(self.dims) {
            let r = ck - ok as i64;
            if r < 0 || r >= dk as i64 {
                return Vec3::ZERO;
            }
            idx += r as usize * stride;
            stride *= dk as usize;
        }
        match self.data.get(idx) {
            Some(&b) => self.colour * b as f32,
            None => Vec3::ZERO,
        }
    }
}

/// Expanded ambient light grid, with the light groups' grids it adds.
#[derive(Debug, Clone, PartialEq)]
pub struct AmbientGrid {
    pub origin: Vec3,
    pub cell: Vec3,
    pub dims: [u32; 3],
    /// `x * y * z` samples, 3 bytes each, x fastest, then y, then z.
    pub rgb: Vec<u8>,
    /// Light-group grids that add to the samples (switched on, colour not dark).
    pub groups: Vec<GroupGrid>,
}

impl AmbientGrid {
    pub fn of(g: &LightGrid<'_>) -> AmbientGrid {
        AmbientGrid {
            origin: Vec3::from(g.origin),
            cell: Vec3::from(g.cell),
            dims: g.dims,
            rgb: g.expand(),
            groups: Vec::new(),
        }
    }

    /// The grid plus the world's light-group grids at `intensity` (group colour x intensity). Groups with a
    /// colour channel below 1/255 add nothing: the engine keeps them at the end of its list and stops at the
    /// first one.
    // SPEC: RENDER_2009.md §6.2 (light-group grids are the light groups' contribution to the light grid used by
    // models), FMT_DAT85.md §10.5 (grid layout); EVIDENCE: engine+0x4f6a0 (dark groups appended at the tail,
    // others at the head), engine+0x4e6a0 (`return` at the first group with a channel < 1/255)
    // DC-DEFAULT: REN-9 (proposed), see DC_DEFAULTS.md (every group on at the given intensity, default 1)
    pub fn with_groups(
        g: &LightGrid<'_>,
        grids: &[LightGroupGrid<'_>],
        intensity: f32,
    ) -> AmbientGrid {
        let mut a = AmbientGrid::of(g);
        for lg in grids {
            let colour = Vec3::from(lg.colour) * intensity;
            // NaN colours count as dark
            if colour.min_element().partial_cmp(&(1.0 / 255.0)) == Some(std::cmp::Ordering::Less)
                || colour.is_nan()
            {
                continue;
            }
            a.groups.push(GroupGrid {
                offset: lg.offset,
                dims: lg.dims,
                colour,
                data: lg.data.to_vec(),
            });
        }
        a
    }

    /// What the light groups add at `p` (the grid sample with groups minus the one without).
    pub fn group_part(&self, p: Vec3) -> Vec3 {
        if self.groups.is_empty() {
            return Vec3::ZERO;
        }
        let bare = AmbientGrid {
            groups: Vec::new(),
            ..self.clone()
        };
        self.sample(p) - bare.sample(p)
    }

    /// Ambient colour (0..255) at `p`: trilinear blend of the eight surrounding samples.
    ///
    /// Cell index per axis = truncate((p - origin) / cell), clamped to [-1, dim - 1]; the neighbour step is 0 at
    /// the last index or below 0, and negative indices read cell 0; the blend fractions are those of the
    /// unclamped position. No samples: grey 0x80. Sample byte order: the first byte is red (the result is
    /// packed with channel 0 in bits 16..23 of the AMBIENT colour). Before the blend each of the eight corner
    /// samples gets the light-group grids' values at its cell (corner = clamped index + 0 / 1 per axis, without
    /// the main grid's edge rule).
    // EVIDENCE: engine+0x4ebd0 (sampler; corners handed to engine+0x4e6a0 before the blend), engine+0x33b70,
    // engine+0x1be930 (packing into AMBIENT); DISASM: engine+0x4f0fc..0x4f168 (channel 0 -> byte 2 of the
    // result, i.e. red)
    pub fn sample(&self, p: Vec3) -> Vec3 {
        let [dx, dy, dz] = self.dims;
        let total = (dx as usize) * (dy as usize) * (dz as usize) * 3;
        if dx == 0 || dy == 0 || dz == 0 || self.rgb.len() < total {
            return Vec3::splat(128.0);
        }
        let inv = |c: f32| if c != 0.0 { 1.0 / c } else { 0.0 };
        let f = (p - self.origin) * Vec3::new(inv(self.cell.x), inv(self.cell.y), inv(self.cell.z));
        let axis = |v: f32, dim: u32| -> (usize, usize, f32) {
            let last = dim as i64 - 1;
            let i = (v as i64).clamp(-1, last);
            let step = if i == last || i < 0 { 0 } else { 1 };
            let i0 = i.max(0) as usize;
            (i0, i0 + step, v - v.floor())
        };
        let (x0, x1, fx) = axis(f.x, dx);
        let (y0, y1, fy) = axis(f.y, dy);
        let (z0, z1, fz) = axis(f.z, dz);
        let at = |x: usize, y: usize, z: usize| {
            let o = ((z * dy as usize + y) * dx as usize + x) * 3;
            Vec3::new(
                self.rgb[o] as f32,
                self.rgb[o + 1] as f32,
                self.rgb[o + 2] as f32,
            )
        };
        // corner (ix, iy, iz) in {0, 1}^3: base sample plus the groups at the clamped index + the corner offset
        let corner = |ix: usize, iy: usize, iz: usize| {
            let base = at(
                if ix == 0 { x0 } else { x1 },
                if iy == 0 { y0 } else { y1 },
                if iz == 0 { z0 } else { z1 },
            );
            let c = [(x0 + ix) as i64, (y0 + iy) as i64, (z0 + iz) as i64];
            self.groups.iter().fold(base, |acc, g| acc + g.at(c))
        };
        let lx = |iy, iz| corner(0, iy, iz).lerp(corner(1, iy, iz), fx);
        let ly = |iz| lx(0, iz).lerp(lx(1, iz), fy);
        ly(0).lerp(ly(1), fz).clamp(Vec3::ZERO, Vec3::splat(255.0))
    }
}

/// Render settings of the model light set, with the shipped defaults.
// SPEC: RENDER_2009.md §13 (light_models 1, model_light_limit 4, model_ambient 1, model_saturation 1.0)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelLightSettings {
    pub light_models: bool,
    pub light_limit: u32,
    pub ambient: bool,
}

impl Default for ModelLightSettings {
    fn default() -> Self {
        ModelLightSettings {
            light_models: true,
            light_limit: 4,
            ambient: true,
        }
    }
}

/// The lights one model is drawn with: ambient (0..255 per channel) and at most 8 lights, strongest first.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LightSet {
    pub ambient: Vec3,
    pub lights: Vec<ModelLight>,
}

/// Most lights a model pass enables.
// SPEC: RENDER_2009.md §6.3 (at most 8 lights are enabled); EVIDENCE: engine+0x1b3b30 (capacity capped at 8)
pub const MAX_MODEL_LIGHTS: usize = 8;

impl LightSet {
    /// Builds the set for a model at `pos` with bounding radius `radius` from the ambient grid, the static
    /// lights and the dynamic lights.
    ///
    /// Rules: with `light_models` off, ambient = white and no lights. Otherwise ambient starts at the grid
    /// sample (`model_ambient`); a light is a candidate when its sphere overlaps the model's box
    /// (static) or sphere (dynamic); each candidate is ranked by `(1 - f) * max channel` of its
    /// [`ModelLight::contribution`] at `pos` (`f` its convert-to-ambient fraction, whose share `f` of the
    /// contribution goes to the ambient at once), the `light_limit` strongest (at most 8) are kept, and every
    /// light that drops out of the list, or never enters it, adds its whole contribution to the ambient.
    // EVIDENCE: engine+0x1b21b0 (builder: limit = min(model_light_limit, 8); ambient from the grid; dynamic lights
    // with 19 / r^2; static lights through the light tree over the box pos +- radius), engine+0x1b3ca0 (sorted
    // insert by (1 - f) * max channel; evicted / rejected lights fold into the ambient), engine+0x1b1800
    pub fn build(
        pos: Vec3,
        radius: f32,
        grid: Option<&AmbientGrid>,
        statics: &[ModelLight],
        dynamics: &[ModelLight],
        s: &ModelLightSettings,
    ) -> LightSet {
        LightSet::build_with(pos, radius, grid, statics, dynamics, s)
    }

    /// [`LightSet::build`] with the static lights from an iterator, offered in its order. A caller that leaves out
    /// static lights whose sphere cannot reach the box `pos +- radius` (and keeps the rest in list order) gets the
    /// same set, bit for bit, as `build` over the whole list: lights that fail the reach test are never offered.
    /// (render-core: the client's static-light index; engineering, no rule of its own.)
    pub fn build_with<'a>(
        pos: Vec3,
        radius: f32,
        grid: Option<&AmbientGrid>,
        statics: impl IntoIterator<Item = &'a ModelLight>,
        dynamics: &[ModelLight],
        s: &ModelLightSettings,
    ) -> LightSet {
        if !s.light_models {
            return LightSet {
                ambient: Vec3::splat(255.0),
                lights: Vec::new(),
            };
        }
        let limit = (s.light_limit as usize).min(MAX_MODEL_LIGHTS);
        let mut ambient = if s.ambient {
            grid.map_or(Vec3::ZERO, |g| g.sample(pos))
        } else {
            Vec3::ZERO
        };
        let mut kept: Vec<(f32, ModelLight)> = Vec::new();
        let mut offer = |l: &ModelLight, ambient: &mut Vec3| {
            let c = l.contribution(pos);
            let f = l.to_ambient.clamp(0.0, 1.0);
            let key = (1.0 - f) * c.max_element();
            let at = kept
                .iter()
                .position(|(k, _)| *k < key)
                .unwrap_or(kept.len());
            if at >= limit {
                *ambient += c;
                return;
            }
            if kept.len() >= limit {
                let (_, out) = kept.pop().expect("non-empty when full");
                *ambient += out.contribution(pos);
            }
            let mut l = *l;
            l.colour *= 1.0 - f;
            *ambient += c * f;
            kept.insert(at, (key, l));
        };
        for d in dynamics {
            let r = d.radius + radius;
            if d.pos.distance_squared(pos) < r * r {
                offer(d, &mut ambient);
            }
        }
        for st in statics.into_iter() {
            let near = match st.shape {
                LightShape::Directional { .. } => true,
                _ => {
                    let lo = pos - Vec3::splat(radius);
                    let hi = pos + Vec3::splat(radius);
                    let q = st.pos.clamp(lo, hi);
                    q.distance_squared(st.pos) < st.radius * st.radius
                }
            };
            if near {
                offer(st, &mut ambient);
            }
        }
        LightSet {
            ambient: ambient.clamp(Vec3::ZERO, Vec3::splat(255.0)),
            lights: kept.into_iter().map(|x| x.1).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_law() {
        let l = ModelLight::dynamic(Vec3::ZERO, 100.0, Vec3::splat(255.0));
        assert!((l.attenuation(0.0) - 1.0).abs() < 1e-6);
        // d = r / 2: 1 / (1 + 19/4)
        let a = l.attenuation(50.0 * 50.0);
        assert!((a - 1.0 / (1.0 + 19.0 / 4.0)).abs() < 1e-6, "{a}");
        assert_eq!(l.attenuation(100.0 * 100.0), 0.0);
        assert_eq!(l.attenuation(1e9), 0.0);
        let q = ModelLight {
            falloff: Falloff::Quartic,
            ..l
        };
        assert!((q.attenuation(2500.0) - 0.5625).abs() < 1e-6);
        let li = ModelLight {
            falloff: Falloff::Linear,
            ..l
        };
        assert!((li.attenuation(2500.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn strongest_lights_kept_rest_folded_into_ambient() {
        let near =
            |x: f32, c: f32| ModelLight::dynamic(Vec3::new(x, 0.0, 0.0), 1000.0, Vec3::splat(c));
        let lights: Vec<ModelLight> = (1..=6).map(|i| near(10.0, i as f32 * 10.0)).collect();
        let s = ModelLightSettings::default();
        let set = LightSet::build(Vec3::ZERO, 10.0, None, &[], &lights, &s);
        assert_eq!(set.lights.len(), 4);
        let cs: Vec<f32> = set.lights.iter().map(|l| l.colour.x).collect();
        assert_eq!(cs, vec![60.0, 50.0, 40.0, 30.0]);
        // the two weakest (10, 20) fold in with their attenuated contribution
        let a = 1.0 / (1.0 + 19.0 * 100.0 / 1e6);
        assert!((set.ambient.x - 30.0 * a).abs() < 1e-3, "{}", set.ambient.x);
        // lighting off: white, no lights
        let off = ModelLightSettings {
            light_models: false,
            ..s
        };
        let set = LightSet::build(Vec3::ZERO, 10.0, None, &[], &lights, &off);
        assert_eq!(set.ambient, Vec3::splat(255.0));
        assert!(set.lights.is_empty());
        // out of reach: not a candidate
        let far = ModelLight::dynamic(Vec3::new(5000.0, 0.0, 0.0), 100.0, Vec3::splat(255.0));
        let set = LightSet::build(Vec3::ZERO, 10.0, None, &[far], &[far], &s);
        assert!(set.lights.is_empty());
    }

    #[test]
    fn grid_trilinear_and_clamped() {
        // 2 x 1 x 1 grid: red 0 then 200; first byte = red
        let g = AmbientGrid {
            origin: Vec3::ZERO,
            cell: Vec3::splat(10.0),
            dims: [2, 1, 1],
            rgb: vec![0, 10, 20, 200, 10, 20],
            groups: Vec::new(),
        };
        assert!((g.sample(Vec3::new(5.0, 0.0, 0.0)).x - 100.0).abs() < 1e-3);
        assert_eq!(g.sample(Vec3::new(5.0, 0.0, 0.0)).y, 10.0);
        // past the last cell: no neighbour, the last sample
        assert!((g.sample(Vec3::new(50.0, 0.0, 0.0)).x - 200.0).abs() < 1e-3);
        let empty = AmbientGrid {
            rgb: vec![],
            ..g.clone()
        };
        assert_eq!(empty.sample(Vec3::ZERO), Vec3::splat(128.0));
    }

    /// A community map's light grid is one cell of the bake's ambient colour (docs/MAPKIT_PLAN.md 5.3 step 4):
    /// the sampler's edge rule (index clamped to [-1, 0], neighbour step 0 at the last index or below 0) returns
    /// that one sample at every position, inside and far outside the cell, whatever the cell size.
    // DC-DEFAULT: MAP-28
    #[test]
    fn one_cell_grid_returns_its_sample_everywhere() {
        let g = AmbientGrid {
            origin: Vec3::new(-2048.0, -256.0, -2048.0),
            cell: Vec3::splat(4096.0),
            dims: [1, 1, 1],
            rgb: vec![90, 100, 110],
            groups: Vec::new(),
        };
        let want = Vec3::new(90.0, 100.0, 110.0);
        for p in [
            Vec3::ZERO,
            Vec3::new(-2048.0, -256.0, -2048.0),
            Vec3::new(2047.0, 3000.0, 2047.0),
            Vec3::new(-1e6, 1e6, 5.0),
            Vec3::new(1e6, -1e6, -5.0),
            Vec3::new(2048.0, 0.0, 2048.0),
        ] {
            assert_eq!(g.sample(p), want, "{p}");
        }
        // a zero cell size (no scale) reads the same cell
        let flat = AmbientGrid {
            cell: Vec3::ZERO,
            ..g.clone()
        };
        assert_eq!(flat.sample(Vec3::new(123.0, 45.0, 6.0)), want);
    }

    #[test]
    fn light_group_grids_add_to_the_corners() {
        // 2 x 2 x 1 main grid of grey 40; one group of half red covering main cell x = 1 (all y) with byte 100
        let g = AmbientGrid {
            origin: Vec3::ZERO,
            cell: Vec3::splat(10.0),
            dims: [2, 2, 1],
            rgb: vec![40; 12],
            groups: vec![GroupGrid {
                offset: [1, 0, 0],
                dims: [1, 2, 1],
                colour: Vec3::new(0.5, 0.0, 0.0),
                data: vec![100, 100],
            }],
        };
        // at x = 0 (cell 0, fraction 0): corner x0 only -> no group part
        assert!((g.sample(Vec3::new(0.0, 0.0, 0.0)).x - 40.0).abs() < 1e-3);
        // halfway between cells 0 and 1: half of 0.5 * 100
        let s = g.sample(Vec3::new(5.0, 0.0, 0.0));
        assert!((s.x - 65.0).abs() < 1e-3, "{s}");
        assert!((s.y - 40.0).abs() < 1e-3);
        assert!((g.group_part(Vec3::new(5.0, 0.0, 0.0)).x - 25.0).abs() < 1e-3);
        // a cell outside the group's box adds nothing
        assert_eq!(g.groups[0].at([0, 0, 0]), Vec3::ZERO);
        assert_eq!(g.groups[0].at([1, 1, 0]).x, 50.0);
        assert_eq!(g.groups[0].at([1, 2, 0]), Vec3::ZERO);
    }

    #[test]
    fn sun_with_fraction() {
        let s = ModelLight::sun_with(Vec3::new(0.0, -2.0, 0.0), Vec3::splat(200.0), 0.4);
        assert_eq!(s.to_ambient, 0.4);
        assert!(matches!(s.shape, LightShape::Directional { dir } if (dir.y + 1.0).abs() < 1e-6));
        assert_eq!(
            ModelLight::sun_with(Vec3::Y, Vec3::ONE, f32::NAN).to_ambient,
            0.0
        );
        assert_eq!(
            ModelLight::sun_with(Vec3::Y, Vec3::ONE, 3.0).to_ambient,
            1.0
        );
    }

    #[test]
    fn spot_cone() {
        let l = ModelLight {
            shape: LightShape::Spot {
                dir: Vec3::Z,
                cos_half: 0.5,
            },
            ..ModelLight::dynamic(Vec3::ZERO, 1000.0, Vec3::splat(100.0))
        };
        assert!(l.contribution(Vec3::new(0.0, 0.0, 10.0)).x > 90.0);
        assert_eq!(l.contribution(Vec3::new(10.0, 0.0, 0.0)), Vec3::ZERO);
    }
}
