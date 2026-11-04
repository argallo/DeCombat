//! The world's global directional light (DSV-40, proposed): the object whose class the render addendum names is
//! read from the world file at load time and sent to every client as `S2C_DIR_LIGHT` once its world is loaded.
//!
//! The property keys and the formula mirror what `dc-render` does for the offline viewer; nothing here depends on
//! that crate. Property names and the class are the `dc_world::names` keys (`sha256[:16]` and length of the ROM
//! name, or DeCombat's own `dc.global_light` / `color` / `bright` / `to_ambient` on a community map, MAP-2, MAP-20).
// SPEC: RENDER_2009_addendum_styles.md §S2.1; UDP_SESSION_2009_B.md §8.4 (writer engine+0x8d6c0)
// DC-DEFAULT: DSV-40 (proposed); MAP-20 (the alias adoption)

use dc_world::names::{
    CLASS_GLOBAL_LIGHT, PROP_BRIGHT, PROP_LIGHT_COLOR, PROP_LIGHT_INNER_COLOR, PROP_OBJECT_BRIGHT,
    PROP_TO_AMBIENT,
};
use dc_world::{FieldValue, Key, Object, World};

/// Class of the world's global light object.
// SPEC: RENDER_2009_addendum_styles.md §S2.1 item 1 (class `7cea843ab1383e94/14`; the class choice is PLAUSIBLE)
// OBSERVED: ROM worlds, one such object on 19 of the 21 worlds (hashes only)
const CLASS_GLOBAL: &Key = &CLASS_GLOBAL_LIGHT;
/// Colour (rgb, 0..255), alternative colour, bright scale, object bright scale, convert-to-ambient.
// OBSERVED: property set of that class in the ROM worlds (hashes only); the same keys `dc-render` reads
const P_COLOUR: &Key = &PROP_LIGHT_COLOR;
const P_INNER_COLOUR: &Key = &PROP_LIGHT_INNER_COLOR;
const P_BRIGHT: &Key = &PROP_BRIGHT;
const P_OBJECT_BRIGHT: &Key = &PROP_OBJECT_BRIGHT;
const P_TO_AMBIENT: &Key = &PROP_TO_AMBIENT;

/// What `S2C_DIR_LIGHT` carries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobalLight {
    /// Rotated +Z of the object's rotation (unit length).
    pub dir: [f32; 3],
    /// Colour x bright scale x object bright scale, 0..255 per channel before the scales.
    pub colour: [f32; 3],
    /// Convert-to-ambient (0 when the object has none).
    pub extra: f32,
}

/// Whether the server sends the world's light (`-globallight world|off`).
// DC-DEFAULT: DSV-40 (proposed) world
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightMode {
    World,
    Off,
}

fn prop<'o, 'a>(o: &'o Object<'a>, key: &Key) -> Option<&'o FieldValue<'a>> {
    o.prop_named(key).map(|p| &p.value)
}

fn real(o: &Object<'_>, key: &Key) -> Option<f32> {
    match prop(o, key)? {
        FieldValue::Real(v) => Some(*v),
        _ => None,
    }
}

fn vec3(o: &Object<'_>, key: &Key) -> Option<[f32; 3]> {
    match prop(o, key)? {
        FieldValue::Vec3(v) | FieldValue::Rgb(v) => Some(*v),
        _ => None,
    }
}

/// Hamilton product `(x, y, z, w)`.
fn qmul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// The rotation of an euler triple: about Y, then X, then Z (the composition of the other light objects).
// SPEC: FMT_DAT85.md §6.1 (euler radians); DC-DEFAULT: REN-11 (proposed) same composition as dc-render
fn euler_quat(e: [f32; 3]) -> [f32; 4] {
    let ax = |a: f32, i: usize| {
        let (s, c) = (a * 0.5).sin_cos();
        let mut q = [0.0, 0.0, 0.0, c];
        q[i] = s;
        q
    };
    qmul(qmul(ax(e[1], 1), ax(e[0], 0)), ax(e[2], 2))
}

/// Rotated +Z axis of a unit quaternion `(x, y, z, w)`.
// SPEC: UDP_SESSION_2009_B.md §8.4 (`(2(w y + z x), 2(z y - w x), 1 - 2(y^2 + x^2))`)
pub fn rotated_z(q: [f32; 4]) -> [f32; 3] {
    let [x, y, z, w] = q;
    [
        2.0 * (w * y + z * x),
        2.0 * (z * y - w * x),
        1.0 - 2.0 * (y * y + x * x),
    ]
}

/// The first global light object of the world: none -> `None` (no message is sent).
// SPEC: RENDER_2009_addendum_styles.md §S2.1 items 1, 2; UDP_SESSION_2009_B.md §8.4
pub fn global_light(w: &World<'_>) -> Option<GlobalLight> {
    let o = w.objects.iter().find(|o| o.is_class(CLASS_GLOBAL))?;
    let q = o.rotation().map_or([0.0, 0.0, 0.0, 1.0], euler_quat);
    let scale = real(o, P_OBJECT_BRIGHT).unwrap_or(1.0) * real(o, P_BRIGHT).unwrap_or(1.0);
    // the alternative colour first, the colour as the fallback (the order of today's reader; `color` of a
    // community map is the second key, so it is read either way)
    let c = vec3(o, P_INNER_COLOUR)
        .or_else(|| vec3(o, P_COLOUR))
        .unwrap_or([255.0; 3]);
    let l = GlobalLight {
        dir: rotated_z(q),
        colour: [c[0] * scale, c[1] * scale, c[2] * scale],
        extra: real(o, P_TO_AMBIENT).unwrap_or(0.0),
    };
    let finite = l.dir.iter().chain(&l.colour).all(|v| v.is_finite()) && l.extra.is_finite();
    finite.then_some(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotations_of_the_forward_axis() {
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(near(rotated_z(euler_quat([0.0; 3])), [0.0, 0.0, 1.0]));
        let h = std::f32::consts::FRAC_PI_2;
        // pitch about X by +90 degrees tips +Z to -Y (down) with the writer's formula
        assert!(near(rotated_z(euler_quat([h, 0.0, 0.0])), [0.0, -1.0, 0.0]));
        // yaw about Y by +90 degrees tips +Z to +X
        assert!(near(rotated_z(euler_quat([0.0, h, 0.0])), [1.0, 0.0, 0.0]));
        // the roll about Z leaves +Z alone
        assert!(near(
            rotated_z(euler_quat([0.0, 0.0, 1.0])),
            [0.0, 0.0, 1.0]
        ));
        let d = rotated_z(euler_quat([0.7, -2.0, 0.3]));
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_world_without_the_object_has_no_light() {
        let bytes = crate::world::synthetic_world_bytes();
        let w = World::parse_with(&bytes, dc_world::ParseOptions { render: false }).unwrap();
        assert!(global_light(&w).is_none());
    }
}
