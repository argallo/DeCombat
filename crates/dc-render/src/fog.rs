//! Linear distance fog of the 2009 renderer and the world settings it comes from.
//!
//! The engine applies Direct3D fixed-function linear fog per pixel (table fog) with the console values `fog_on`,
//! `fog_near`, `fog_far` and `fog_r/g/b` (0..255). The game module copies them, at world entry, from the server's
//! world-settings variables, which the server fills from the world's world-settings object. Each pass blends its
//! colour toward its own fog colour: the world fog colour, except the lightmapped multiply passes (neutral grey with
//! `saturate`, white without, so the fog is not applied twice) and additive objects (black).
//!
//! Property names are the keys of `dc_world::names` (the ROM name by `sha256(name)[:16]` and length, no ROM string
//! here, or DeCombat's own `dc.world_settings` property names, MAP-2).

use dc_world::names::{
    PROP_FAR_Z, PROP_FOG_COLOR, PROP_FOG_ENABLE, PROP_FOG_FAR, PROP_FOG_NEAR, PROP_SKY_FOG_FAR,
    PROP_SKY_FOG_NEAR,
};
use dc_world::{FieldValue, Key, World};

/// Linear fog: full colour up to `near`, full fog colour from `far`, view-space depth in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    pub near: f32,
    pub far: f32,
    /// Fog colour 0..255 per channel.
    pub colour: [f32; 3],
}

impl Fog {
    /// The share of the surface colour that stays at view depth `depth`: `(far - depth) / (far - near)` clamped
    /// to [0, 1] (public Direct3D 9 linear fog formula).
    // SPEC: RENDER_2009.md §3.3 (fog is always linear with distances in world units: FOGSTART = fog_near, FOGEND =
    // fog_far)
    pub fn factor(&self, depth: f32) -> f32 {
        let span = self.far - self.near;
        if span.is_nan() || span.abs() <= 1e-6 || !depth.is_finite() {
            return 1.0;
        }
        ((self.far - depth) / span).clamp(0.0, 1.0)
    }

    /// The fog uniform `[near, 1 / (far - near) or 0, on, 0]` and the colour in [0, 1] (shared by `world.wgsl`
    /// and `model.wgsl`).
    pub(crate) fn uniform(fog: Option<&Fog>) -> ([f32; 4], [f32; 4]) {
        match fog {
            Some(f) if (f.far - f.near).abs() > 1e-6 => {
                let c = f.colour.map(|v| (v / 255.0).clamp(0.0, 1.0));
                (
                    [f.far, 1.0 / (f.far - f.near), 1.0, 0.0],
                    [c[0], c[1], c[2], 1.0],
                )
            }
            _ => ([0.0; 4], [0.0, 0.0, 0.0, 1.0]),
        }
    }
}

// World-settings properties the fog comes from: enable (bool), near (real), far (real), colour (colour 0..255),
// and the sky pass's near / far: `dc_world::names::{PROP_FOG_ENABLE, PROP_FOG_NEAR, PROP_FOG_FAR, PROP_FOG_COLOR,
// PROP_SKY_FOG_NEAR, PROP_SKY_FOG_FAR}` (the same hashes this file held before stage 2, plus the `dc.world_settings`
// names `fog_enable`, `fog_near`, `fog_far`, `fog_color`, `sky_fog_near`, `sky_fog_far`).
// SPEC: CAPTURE_rust3_RUSTSTACK_VALIDATION.md §3.3 (world-settings variables and the properties they come from);
// EVIDENCE: cshell+0x1a6900 / cshell+0x1a6840 (at world entry the client runs "<name> <value>" console commands for
// the fog enable, near, far, red, green, blue and the sky-fog values when the fog capability bit 0x10 of its settings
// word +0x52b8 is set, else "fog off"; cshell+0x1b3370 clears the bit for a "disable fog" command-line switch)
// DC-DEFAULT: MAP-26 (the renderer reads the vocabulary names through `Key::matches`)
fn named(name: &[u8], key: &Key) -> bool {
    key.matches(name)
}

/// The fog of the world pass and of the sky pass from the world's world-settings object (the object of the
/// world-settings role; a world without one: the first object that carries the fog-enable property): `None` when
/// that object is missing or the fog is off.
///
/// A missing near / far / colour reads 0 (the client applies a missing server variable as 0, cshell+0x1a6840); a
/// fog whose near equals its far is off (RENDER_2009.md §3.3). The colour components are truncated to integers as
/// the engine's integer console values hold them.
///
/// The sky pass has no switch of its own: the engine's sky pass sets only the fog range (sky near / far) and keeps
/// the world pass's fog enable and colour, so the sky is fogged whenever the world is; the world file's sky-fog
/// enable reaches a console name the engine does not have.
// EVIDENCE: engine+0x1b4d50 (sky pass: SetRenderState FOGSTART / FOGEND (0x24 / 0x25) from the sky near / far values
// at engine 0x6d3b3c / 0x6d3b54, FOGENABLE (0x1c) only read and saved, set to 0 only for an object with flag 0x80);
// cshell+0x1a6900 (the sky-fog enable is sent as a console command like the others); the engine binary holds no
// sky-fog-enable variable name (searched; the near / far names are present)
// DC-DEFAULT: REN-22 (proposed), see the staged rows (the client takes the fog from the world file's world-settings
// object, the same values the DeCombat server sends as world-settings variables, DSV-47; a server's own variables
// are not read yet); VIS-20 (proposed) the world-settings role object first, the sky fogged with the world's enable
pub fn world_fog(world: &World<'_>) -> (Option<Fog>, Option<Fog>) {
    let has_enable =
        |o: &&dc_world::Object<'_>| o.props.iter().any(|p| named(p.name, &PROP_FOG_ENABLE));
    let Some(o) = world
        .objects
        .iter()
        .filter(has_enable)
        .find(|o| o.role == Some(dc_world::ObjectRole::WorldSettings))
        .or_else(|| world.objects.iter().find(has_enable))
    else {
        return (None, None);
    };
    let real = |key: &Key| {
        o.props
            .iter()
            .find(|p| named(p.name, key))
            .and_then(|p| match p.value {
                FieldValue::Real(v) if v.is_finite() => Some(v),
                FieldValue::Int(v) | FieldValue::Bits(v) => Some(v as f32),
                _ => None,
            })
            .unwrap_or(0.0)
    };
    let on = |key: &Key| {
        o.props
            .iter()
            .find(|p| named(p.name, key))
            .is_some_and(|p| match p.value {
                FieldValue::Bool(b) => b,
                FieldValue::Real(v) => v != 0.0,
                FieldValue::Int(v) | FieldValue::Bits(v) => v != 0,
                _ => false,
            })
    };
    let colour = o
        .props
        .iter()
        .find(|p| named(p.name, &PROP_FOG_COLOR))
        .and_then(|p| match p.value {
            FieldValue::Rgb(c) | FieldValue::Vec3(c) => Some(c),
            _ => None,
        })
        .unwrap_or([0.0; 3])
        .map(|v| {
            if v.is_finite() {
                v.trunc().clamp(0.0, 255.0)
            } else {
                0.0
            }
        });
    let make = |enabled: bool, near: f32, far: f32| {
        (enabled && (far - near).abs() > 1e-6).then_some(Fog { near, far, colour })
    };
    let enabled = on(&PROP_FOG_ENABLE);
    (
        make(enabled, real(&PROP_FOG_NEAR), real(&PROP_FOG_FAR)),
        make(enabled, real(&PROP_SKY_FOG_NEAR), real(&PROP_SKY_FOG_FAR)),
    )
}

/// The world's far plane from its world-settings object (the object of the world-settings role; a world without one:
/// the first object that carries the far-plane property): `Some` only for a finite value above 50, which is when the
/// 2009 client applies the server's far-plane variable to its `far_z` console value; otherwise the camera keeps its
/// own far plane ([`crate::DEFAULT_FAR_Z`]). The camera clamps the value to [3, 500000] ([`crate::Camera::far`]).
// SPEC: CAPTURE_rust3_RUSTSTACK_VALIDATION.md §3.3 (the far-plane variable = the world-settings property; client
// reader cshell+0x1a6900 via cshell+0x1a6840, applied only when > 50); RENDER_2009.md §4.1 (far_z default 12000,
// clamp engine+0xfb8c0)
// DC-DEFAULT: CLI-1851 (proposed) the client takes it from the world file, like the fog (REN-22): the same value the
// DeCombat server sends as a world-settings variable (DSV-47)
pub fn world_far_z(world: &World<'_>) -> Option<f32> {
    let has_far = |o: &&dc_world::Object<'_>| o.props.iter().any(|p| named(p.name, &PROP_FAR_Z));
    let o = world
        .objects
        .iter()
        .filter(has_far)
        .find(|o| o.role == Some(dc_world::ObjectRole::WorldSettings))
        .or_else(|| world.objects.iter().find(has_far))?;
    let v = o
        .props
        .iter()
        .find(|p| named(p.name, &PROP_FAR_Z))
        .and_then(|p| match p.value {
            FieldValue::Real(v) => Some(v),
            FieldValue::Int(v) | FieldValue::Bits(v) => Some(v as f32),
            _ => None,
        })?;
    (v.is_finite() && v > WORLD_FAR_Z_MIN).then_some(v)
}

/// The far-plane variable is applied only above this value.
// SPEC: CAPTURE_rust3_RUSTSTACK_VALIDATION.md §3.3 (cshell+0x1a6840: applied only when > 50)
pub const WORLD_FAR_Z_MIN: f32 = 50.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_factor() {
        let f = Fog {
            near: 600.0,
            far: 5000.0,
            colour: [10.0, 20.0, 30.0],
        };
        assert_eq!(f.factor(0.0), 1.0);
        assert_eq!(f.factor(600.0), 1.0);
        assert!((f.factor(2800.0) - 0.5).abs() < 1e-6);
        assert_eq!(f.factor(5000.0), 0.0);
        assert_eq!(f.factor(9000.0), 0.0);
        // a negative near (one ROM world) fogs from the eye on
        let g = Fog {
            near: -500.0,
            far: 1500.0,
            colour: [0.0; 3],
        };
        assert!((g.factor(0.0) - 0.75).abs() < 1e-6);
        // degenerate range: no fog
        let h = Fog {
            near: 100.0,
            far: 100.0,
            colour: [0.0; 3],
        };
        assert_eq!(h.factor(1e6), 1.0);
        assert_eq!(Fog::uniform(Some(&h)).0[2], 0.0);
        assert_eq!(Fog::uniform(None).0[2], 0.0);
        let (u, c) = Fog::uniform(Some(&f));
        assert_eq!(u[0], 5000.0);
        assert!((u[1] - 1.0 / 4400.0).abs() < 1e-9);
        assert_eq!(u[2], 1.0);
        assert!((c[2] - 30.0 / 255.0).abs() < 1e-6);
    }
}
