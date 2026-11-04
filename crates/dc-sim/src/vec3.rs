//! Minimal 3-vector and rotation types with a fixed, documented operation order (no SIMD, no FMA), so every
//! build computes the same bits. Frame: +Y up, local +Z forward, local +X right (MOVEMENT_2009.md
//! conventions).
// SPEC: MOVEMENT_2009.md (conventions: +Y up, +Z forward, +X right), §15 item 2 (order of summation)

use crate::fmath;
use core::ops::{Add, AddAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// A 3-vector of binary32.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl V3 {
    pub const ZERO: V3 = V3::new(0.0, 0.0, 0.0);
    pub const UP: V3 = V3::new(0.0, 1.0, 0.0);

    #[inline]
    pub const fn new(x: f32, y: f32, z: f32) -> V3 {
        V3 { x, y, z }
    }

    #[inline]
    pub const fn from_array(a: [f32; 3]) -> V3 {
        V3::new(a[0], a[1], a[2])
    }

    #[inline]
    pub const fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    /// Dot product summed x, y, z.
    #[inline]
    pub fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    /// Dot product summed x, z, y: the order the engine integrator uses in its steep-contact branch.
    // SPEC: MOVEMENT_2009.md §8.3 notes (dot products summed in the order x, z, y)
    #[inline]
    pub fn dot_xzy(self, o: V3) -> f32 {
        self.x * o.x + self.z * o.z + self.y * o.y
    }

    #[inline]
    pub fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    #[inline]
    pub fn len_sq(self) -> f32 {
        self.dot(self)
    }

    #[inline]
    pub fn len(self) -> f32 {
        fmath::sqrt(self.len_sq())
    }

    /// Horizontal part (y set to 0).
    #[inline]
    pub fn xz(self) -> V3 {
        V3::new(self.x, 0.0, self.z)
    }

    /// `|v.xz|`.
    #[inline]
    pub fn len_xz(self) -> f32 {
        fmath::sqrt(self.x * self.x + self.z * self.z)
    }

    /// `v / |v|`, or zero for the zero vector.
    #[inline]
    pub fn normalized(self) -> V3 {
        let l = self.len();
        if l == 0.0 {
            V3::ZERO
        } else {
            self * (1.0 / l)
        }
    }

    /// `v` scaled to length `m`; the zero vector stays zero.
    // SPEC: MOVEMENT_2009.md §8.3 ("a scaled to length m (when |a| != 0)")
    #[inline]
    pub fn with_len(self, m: f32) -> V3 {
        let l = self.len();
        if l == 0.0 {
            self
        } else {
            self * (m / l)
        }
    }

    #[inline]
    pub fn is_zero(self) -> bool {
        self.x == 0.0 && self.y == 0.0 && self.z == 0.0
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    #[inline]
    pub fn has_nan(self) -> bool {
        self.x.is_nan() || self.y.is_nan() || self.z.is_nan()
    }

    /// The three components' raw bits (for digests and bit-exact comparisons).
    #[inline]
    pub fn bits(self) -> [u32; 3] {
        [self.x.to_bits(), self.y.to_bits(), self.z.to_bits()]
    }
}

impl Add for V3 {
    type Output = V3;
    #[inline]
    fn add(self, o: V3) -> V3 {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for V3 {
    type Output = V3;
    #[inline]
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f32> for V3 {
    type Output = V3;
    #[inline]
    fn mul(self, s: f32) -> V3 {
        V3::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Neg for V3 {
    type Output = V3;
    #[inline]
    fn neg(self) -> V3 {
        V3::new(-self.x, -self.y, -self.z)
    }
}

impl AddAssign for V3 {
    #[inline]
    fn add_assign(&mut self, o: V3) {
        *self = *self + o;
    }
}

impl SubAssign for V3 {
    #[inline]
    fn sub_assign(&mut self, o: V3) {
        *self = *self - o;
    }
}

impl MulAssign<f32> for V3 {
    #[inline]
    fn mul_assign(&mut self, s: f32) {
        *self = *self * s;
    }
}

/// Rotation quaternion `(x, y, z, w)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    /// Rotation by `yaw` about +Y only (the body rotation: body pitch is forced to 0).
    // SPEC: MOVEMENT_2009.md §13.2 step 7 (body pitch := 0)
    pub fn from_yaw(yaw: f32) -> Quat {
        let (s, c) = fmath::sin_cos(yaw * 0.5);
        Quat {
            x: 0.0,
            y: s,
            z: 0.0,
            w: c,
        }
    }

    /// Camera rotation: pitch about +X applied first, then yaw about +Y (`q = q_yaw * q_pitch`). The 2009
    /// Euler builder's order is not fully decoded; this is the PLAUSIBLE reading (see
    /// [`crate::todo::EULER_YAW_THEN_PITCH`]).
    // SPEC: MOVEMENT_2009.md §13.2 step 7 (rotation order PLAUSIBLY yaw about +Y, pitch about +X), §16.1 item 10
    pub fn from_yaw_pitch(yaw: f32, pitch: f32) -> Quat {
        let (sy, cy) = fmath::sin_cos(yaw * 0.5);
        let (sp, cp) = fmath::sin_cos(pitch * 0.5);
        // (0, sy, 0, cy) * (sp, 0, 0, cp)
        Quat {
            x: cy * sp,
            y: sy * cp,
            z: -sy * sp,
            w: cy * cp,
        }
    }

    /// The rotated +Z axis: `F = (2(xz + wy), 2(yz - wx), 1 - 2(x^2 + y^2))`.
    // SPEC: MOVEMENT_2009.md §6.3 (basis F)
    pub fn forward(self) -> V3 {
        let Quat { x, y, z, w } = self;
        V3::new(
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        )
    }

    /// X and Z components of the rotated +X axis, not renormalised: `R = (1 - 2(y^2 + z^2), 0, 2(xz - wy))`.
    // SPEC: MOVEMENT_2009.md §6.3 (basis R)
    pub fn right_flat(self) -> V3 {
        let Quat { x, y, z, w } = self;
        V3::new(1.0 - 2.0 * (y * y + z * z), 0.0, 2.0 * (x * z - w * y))
    }

    /// The full rotated +X axis (spectator strafing).
    // SPEC: MOVEMENT_2009.md §6.6 (R3 = the full rotated +X axis)
    pub fn right(self) -> V3 {
        let Quat { x, y, z, w } = self;
        V3::new(
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        (a - b).len() < 1e-5
    }

    #[test]
    fn yaw_basis_is_right_handed_about_y() {
        let q = Quat::from_yaw(core::f32::consts::FRAC_PI_2);
        // +Z turned by +90 degrees about +Y is +X; +X becomes -Z.
        assert!(
            close(q.forward(), V3::new(1.0, 0.0, 0.0)),
            "{:?}",
            q.forward()
        );
        assert!(close(q.right_flat(), V3::new(0.0, 0.0, -1.0)));
        let q0 = Quat::from_yaw(0.0);
        assert!(close(q0.forward(), V3::new(0.0, 0.0, 1.0)));
        assert!(close(q0.right(), V3::new(1.0, 0.0, 0.0)));
    }

    #[test]
    fn pitch_tilts_forward_vertically() {
        let q = Quat::from_yaw_pitch(0.0, 0.5);
        let f = q.forward();
        assert!((f.len() - 1.0).abs() < 1e-5);
        assert!(f.y < 0.0, "positive pitch looks down: {f:?}");
        assert!((f.x).abs() < 1e-6);
    }

    #[test]
    fn helpers() {
        let v = V3::new(3.0, 4.0, 12.0);
        assert_eq!(v.len(), 13.0);
        assert_eq!(v.len_xz(), fmath::sqrt(153.0));
        assert_eq!(V3::ZERO.normalized(), V3::ZERO);
        assert!((v.with_len(26.0).len() - 26.0).abs() < 1e-4);
        assert_eq!(v.dot_xzy(v), v.dot(v));
        assert_eq!(
            V3::new(1.0, 0.0, 0.0).cross(V3::new(0.0, 1.0, 0.0)),
            V3::new(0.0, 0.0, 1.0)
        );
    }
}
