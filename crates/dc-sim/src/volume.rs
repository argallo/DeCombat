//! Container volumes the body can be inside of (ladders first): each one a convex region given by its bounding
//! planes and the container record the mover gets while the body touches it.
//!
//! The 2009 client fills the mover's container list every frame from the containers the body touches (client
//! interface slot +0x24, MOVEMENT_2009.md §11.1); the dserver does not send it (the client is not told which ladder
//! it is on, §8.4: only a remote server's mover update 0x0004 carries a list, and ours sends none). DeCombat builds
//! the volumes from the world's ladder brushes (the BSP model named like the ladder object, FMT_DAT85.md §6.4) and
//! tests the body's cylinder against each one's planes.
// SPEC: MOVEMENT_2009.md §11.1 (container list, locally filled from touched containers), §11.2 (kind 11 = ladder),
// §11.4 (ladder movement); FMT_DAT85.md §6.2 (ladder role, brush geometry), §6.4 (object to model link)
// DC-DEFAULT: CLI-59 (proposed) the body is in a ladder when its cylinder touches the region inside every plane of
// the ladder brush (UNPROVEN how the 2009 engine tests "touches"; the record values are CLI-59's: no current, no
// viscosity, friction factor 1)

extern crate alloc;
use alloc::vec::Vec;

use crate::player::Container;
use crate::shape::Cylinder;
use crate::vec3::V3;

/// A convex region: the points `x` with `dot(n, x) - d <= 0` for every plane `(n, d)` (unit normals pointing out).
#[derive(Debug, Clone, PartialEq)]
pub struct ConvexVolume {
    pub planes: Vec<(V3, f32)>,
    /// Axis-aligned bounds of the brush points (a quick reject before the plane test).
    pub lo: V3,
    pub hi: V3,
}

impl ConvexVolume {
    /// From outward planes and the brush's bounding box.
    pub fn new(planes: Vec<(V3, f32)>, lo: V3, hi: V3) -> ConvexVolume {
        ConvexVolume { planes, lo, hi }
    }

    /// An axis-aligned box (tests, mods).
    pub fn from_box(lo: V3, hi: V3) -> ConvexVolume {
        let planes = alloc::vec![
            (V3::new(1.0, 0.0, 0.0), hi.x),
            (V3::new(-1.0, 0.0, 0.0), -lo.x),
            (V3::new(0.0, 1.0, 0.0), hi.y),
            (V3::new(0.0, -1.0, 0.0), -lo.y),
            (V3::new(0.0, 0.0, 1.0), hi.z),
            (V3::new(0.0, 0.0, -1.0), -lo.z),
        ];
        ConvexVolume { planes, lo, hi }
    }

    /// Whether `shape` centred at `at` touches the region (the shape reaches inside every plane).
    pub fn touches(&self, shape: &Cylinder, at: V3) -> bool {
        let r = V3::new(shape.radius, shape.half_height, shape.radius);
        let (lo, hi) = (at - r, at + r);
        if hi.x < self.lo.x
            || hi.y < self.lo.y
            || hi.z < self.lo.z
            || lo.x > self.hi.x
            || lo.y > self.hi.y
            || lo.z > self.hi.z
        {
            return false;
        }
        self.planes
            .iter()
            .all(|&(n, d)| n.dot(at) - d <= shape.support(n))
    }
}

/// Every container volume of a world with the record its touch gives.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContainerVolumes {
    pub volumes: Vec<(ConvexVolume, Container)>,
}

impl ContainerVolumes {
    pub fn is_empty(&self) -> bool {
        self.volumes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.volumes.len()
    }

    /// The containers the body touches, in volume order, at most 16 (the 2009 list size).
    // SPEC: MOVEMENT_2009.md §11.1 (up to 16 entries)
    pub fn touched(&self, shape: &Cylinder, at: V3, out: &mut Vec<Container>) {
        out.clear();
        for (v, c) in &self.volumes {
            if out.len() >= MAX_CONTAINERS {
                break;
            }
            if v.touches(shape, at) {
                out.push(*c);
            }
        }
    }
}

/// Size of the mover's container list.
// SPEC: MOVEMENT_2009.md §11.1 (mover+0x2f4 count, up to 16 entries)
pub const MAX_CONTAINERS: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_volume_is_touched_by_a_cylinder_reaching_into_it() {
        let v = ConvexVolume::from_box(V3::new(0.0, 0.0, 0.0), V3::new(10.0, 200.0, 40.0));
        let body = Cylinder::from_dims(V3::new(15.0, 50.0, 15.0));
        assert!(v.touches(&body, V3::new(5.0, 100.0, 20.0)), "inside");
        assert!(
            v.touches(&body, V3::new(-14.0, 100.0, 20.0)),
            "rim reaches in"
        );
        assert!(
            !v.touches(&body, V3::new(-16.0, 100.0, 20.0)),
            "just outside"
        );
        assert!(!v.touches(&body, V3::new(5.0, 251.0, 20.0)), "above");
        let mut out = Vec::new();
        let set = ContainerVolumes {
            volumes: alloc::vec![(v, Container::ladder())],
        };
        set.touched(&body, V3::new(5.0, 100.0, 20.0), &mut out);
        assert_eq!(out, [Container::ladder()]);
        set.touched(&body, V3::new(500.0, 100.0, 20.0), &mut out);
        assert!(out.is_empty());
    }
}
