//! The body's collision shape: a vertical cylinder built from the half extents.
// SPEC: MOVEMENT_2009.md §9.2 (engine+0x75110, support engine+0x74c90)

use crate::fmath;
use crate::vec3::V3;

/// Vertical cylinder: radius `r = max(dx, dz)`, half height `h = dy`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cylinder {
    pub radius: f32,
    pub half_height: f32,
    /// Bounding radius `sqrt(r^2 + h^2)`.
    pub bound: f32,
}

impl Cylinder {
    /// From the body's half extents `dims = (dx, dy, dz)`.
    // SPEC: MOVEMENT_2009.md §9.2 (r = max(dx, dz), h = dy, bounding radius sqrt(r^2 + h^2))
    pub fn from_dims(dims: V3) -> Cylinder {
        let r = if dims.x > dims.z { dims.x } else { dims.z };
        Cylinder::new(r, dims.y)
    }

    pub fn new(radius: f32, half_height: f32) -> Cylinder {
        Cylinder {
            radius,
            half_height,
            bound: fmath::sqrt(radius * radius + half_height * half_height),
        }
    }

    /// A point (zero-size shape), used for rays.
    pub const POINT: Cylinder = Cylinder {
        radius: 0.0,
        half_height: 0.0,
        bound: 0.0,
    };

    /// Support distance along a unit normal: `s r + sqrt(1 - s^2) h` with `s = min(1, |n.xz|)`.
    // SPEC: MOVEMENT_2009.md §9.2 (support(n), engine+0x74c90)
    pub fn support(&self, n: V3) -> f32 {
        let mut s = n.len_xz();
        if s > 1.0 {
            s = 1.0;
        }
        s * self.radius + fmath::sqrt(1.0 - s * s) * self.half_height
    }

    /// Offset from the centre to the shape's farthest point along `n` (rim point for tilted `n`):
    /// `(n.xz / |n.xz| * r, sign(n.y) h)`.
    // SPEC: MOVEMENT_2009.md §9.9 step 4 (k = the cylinder's support offset)
    pub fn support_offset(&self, n: V3) -> V3 {
        let l = n.len_xz();
        let (x, z) = if l > 0.0 {
            (n.x / l * self.radius, n.z / l * self.radius)
        } else {
            (0.0, 0.0)
        };
        let y = if n.y > 0.0 {
            self.half_height
        } else if n.y < 0.0 {
            -self.half_height
        } else {
            0.0
        };
        V3::new(x, y, z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_is_max_of_x_and_z() {
        let c = Cylinder::from_dims(V3::new(12.0, 40.0, 16.0));
        assert_eq!((c.radius, c.half_height), (16.0, 40.0));
        assert_eq!(c.bound, fmath::sqrt(16.0 * 16.0 + 40.0 * 40.0));
    }

    #[test]
    fn support_along_axes_and_diagonal() {
        let c = Cylinder::new(16.0, 40.0);
        assert_eq!(c.support(V3::UP), 40.0);
        assert_eq!(c.support(V3::new(1.0, 0.0, 0.0)), 16.0);
        let d = V3::new(0.6, 0.8, 0.0);
        assert!((c.support(d) - (0.6 * 16.0 + 0.8 * 40.0)).abs() < 1e-4);
        // the support equals dot(n, support_offset(n)) for a unit normal
        assert!((c.support(d) - d.dot(c.support_offset(d))).abs() < 1e-4);
    }
}
