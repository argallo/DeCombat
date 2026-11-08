//! The composed light grid: one grid over the composed world, each sample taken from the piece whose region
//! holds it (trilinear over the piece's own grid, the rule dc-render samples with) and from the map's own grid
//! elsewhere. A piece's grid is expanded only over its region, so a ROM grid of a hundred million samples costs
//! one pass over its packed bytes and the memory of the region.
// SPEC: FMT_DAT85.md §8 (origin, cell, dims, run-length packed RGB; sample (x, y, z) at ((z * dy) + y) * dx + x)
// DC-DEFAULT: MAP-116

use super::clip::{Aabb, V};
use crate::LightGrid;

/// Samples the composed grid may hold; past it the cell grows.
pub const MAX_SAMPLES: u64 = 4 << 20;
/// The cell of the composed grid when no piece says otherwise (every ROM world's grid uses 250).
pub const DEFAULT_CELL: f32 = 250.0;

/// An expanded window of a grid: `dims` samples from `first` (sample indices of the source grid).
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub origin: V,
    pub cell: V,
    pub dims: [u32; 3],
    pub first: [u32; 3],
    pub size: [u32; 3],
    pub rgb: Vec<u8>,
}

impl Window {
    /// The samples of `g` over the box `want` (one cell of margin), expanded from the packed bytes.
    pub fn of(g: &LightGrid<'_>, want: &Aabb) -> Window {
        let dims = g.dims;
        let mut first = [0u32; 3];
        let mut size = [0u32; 3];
        for a in 0..3 {
            if dims[a] == 0 || g.cell[a] <= 0.0 {
                continue;
            }
            let lo = ((want.min[a] - g.origin[a]) / g.cell[a]).floor() as i64 - 1;
            let hi = ((want.max[a] - g.origin[a]) / g.cell[a]).ceil() as i64 + 1;
            let last = dims[a] as i64 - 1;
            let lo = lo.clamp(0, last);
            let hi = hi.clamp(0, last);
            first[a] = lo as u32;
            size[a] = (hi - lo + 1).max(0) as u32;
        }
        let total = size[0] as usize * size[1] as usize * size[2] as usize;
        let mut rgb = vec![128u8; total * 3];
        if total > 0 {
            /// The sample the expander is at, in file order (x fastest, then y, then z).
            struct At {
                xyz: [u32; 3],
                dims: [u32; 3],
            }
            impl At {
                fn step(&mut self) {
                    self.xyz[0] += 1;
                    if self.xyz[0] == self.dims[0] {
                        self.xyz[0] = 0;
                        self.xyz[1] += 1;
                        if self.xyz[1] == self.dims[1] {
                            self.xyz[1] = 0;
                            self.xyz[2] += 1;
                        }
                    }
                }
                fn done(&self) -> bool {
                    self.xyz[2] >= self.dims[2]
                }
            }
            let put = |rgb: &mut [u8], s: &[u8], at: &At| {
                let [x, y, z] = at.xyz;
                if x >= first[0]
                    && x < first[0] + size[0]
                    && y >= first[1]
                    && y < first[1] + size[1]
                    && z >= first[2]
                    && z < first[2] + size[2]
                {
                    let i = (((z - first[2]) * size[1] + (y - first[1])) * size[0] + (x - first[0]))
                        as usize
                        * 3;
                    rgb[i..i + 3].copy_from_slice(s);
                }
            };
            let mut at = At { xyz: [0; 3], dims };
            let mut p = g.packed;
            'outer: while !at.done() {
                let Some((&ctl, rest)) = p.split_first() else {
                    break;
                };
                let run = (ctl & 0x7f) as usize + 1;
                if ctl & 0x80 != 0 {
                    let Some(s) = rest.get(..3) else { break };
                    for _ in 0..run {
                        if at.done() {
                            break 'outer;
                        }
                        put(&mut rgb, s, &at);
                        at.step();
                    }
                    p = &rest[3..];
                } else {
                    let Some(lits) = rest.get(..3 * run) else {
                        break;
                    };
                    for s in lits.as_chunks::<3>().0 {
                        if at.done() {
                            break 'outer;
                        }
                        put(&mut rgb, s, &at);
                        at.step();
                    }
                    p = &rest[3 * run..];
                }
            }
        }
        Window {
            origin: g.origin,
            cell: g.cell,
            dims,
            first,
            size,
            rgb,
        }
    }

    /// The colour at `p`: trilinear over the eight samples around it, the cell index clamped to the grid the way
    /// dc-render clamps it (`lights.rs AmbientGrid::sample`), read inside the window.
    pub fn sample(&self, p: V) -> [f32; 3] {
        if self.size.contains(&0) {
            return [128.0; 3];
        }
        let mut idx = [(0u32, 0u32, 0.0f32); 3];
        for a in 0..3 {
            let f = if self.cell[a] != 0.0 {
                (p[a] - self.origin[a]) / self.cell[a]
            } else {
                0.0
            };
            let last = self.dims[a] as i64 - 1;
            let i = (f as i64).clamp(-1, last);
            let step = if i == last || i < 0 { 0 } else { 1 };
            let i0 = i.max(0) as u32;
            // into the window
            let w0 = i0.clamp(self.first[a], self.first[a] + self.size[a] - 1) - self.first[a];
            let w1 =
                (i0 + step).clamp(self.first[a], self.first[a] + self.size[a] - 1) - self.first[a];
            idx[a] = (w0, w1, f - f.floor());
        }
        let at = |x: u32, y: u32, z: u32| -> [f32; 3] {
            let i = ((z * self.size[1] + y) * self.size[0] + x) as usize * 3;
            [
                self.rgb[i] as f32,
                self.rgb[i + 1] as f32,
                self.rgb[i + 2] as f32,
            ]
        };
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
            [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ]
        };
        let (x, y, z) = (idx[0], idx[1], idx[2]);
        let row = |yy: u32, zz: u32| lerp(at(x.0, yy, zz), at(x.1, yy, zz), x.2);
        let plane = |zz: u32| lerp(row(y.0, zz), row(y.1, zz), y.2);
        lerp(plane(z.0), plane(z.1), z.2)
    }
}

/// One source of the composed grid: the region (and cuts) it serves, its offset and its window.
pub struct Source<'s> {
    pub region: Aabb,
    pub cuts: &'s [Aabb],
    pub offset: V,
    pub window: Window,
}

/// The composed grid over `extent`: `(origin, cell, dims, rgb)`.
pub fn compose(extent: &Aabb, own: &Window, pieces: &[Source<'_>]) -> (V, V, [u32; 3], Vec<u8>) {
    let mut cell = DEFAULT_CELL;
    let span = [
        extent.max[0] - extent.min[0],
        extent.max[1] - extent.min[1],
        extent.max[2] - extent.min[2],
    ];
    let dims_of = |c: f32| -> [u32; 3] {
        [
            (span[0] / c).ceil().max(0.0) as u32 + 1,
            (span[1] / c).ceil().max(0.0) as u32 + 1,
            (span[2] / c).ceil().max(0.0) as u32 + 1,
        ]
    };
    let mut dims = dims_of(cell);
    while dims.iter().map(|&d| d as u64).product::<u64>() > MAX_SAMPLES {
        cell *= 2.0;
        dims = dims_of(cell);
    }
    let mut rgb = Vec::with_capacity(dims.iter().map(|&d| d as usize).product::<usize>() * 3);
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let p = [
                    extent.min[0] + x as f32 * cell,
                    extent.min[1] + y as f32 * cell,
                    extent.min[2] + z as f32 * cell,
                ];
                let c = pieces
                    .iter()
                    .find(|s| s.region.contains(p) && !s.cuts.iter().any(|k| k.contains(p)))
                    .map(|s| {
                        s.window.sample([
                            p[0] - s.offset[0],
                            p[1] - s.offset[1],
                            p[2] - s.offset[2],
                        ])
                    })
                    .unwrap_or_else(|| own.sample(p));
                for ch in c {
                    rgb.push((ch + 0.5).clamp(0.0, 255.0) as u8);
                }
            }
        }
    }
    (extent.min, [cell; 3], dims, rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::pack_light_grid;

    #[test]
    fn a_window_reads_the_samples_of_its_box() {
        // 4 x 3 x 2 grid, sample value = index
        let dims = [4u32, 3, 2];
        let mut rgb = Vec::new();
        for i in 0..24u8 {
            rgb.extend_from_slice(&[i, i, i]);
        }
        let packed = pack_light_grid(&rgb);
        let g = LightGrid {
            origin: [0.0; 3],
            cell: [10.0; 3],
            dims,
            declared_packed_len: packed.len() as u32,
            packed: &packed,
        };
        let w = Window::of(&g, &Aabb::new([0.0; 3], [30.0, 20.0, 10.0]));
        assert_eq!(w.size, [4, 3, 2]);
        // sample (x=2, y=1, z=1) = ((1 * 3) + 1) * 4 + 2 = 18
        assert_eq!(w.sample([20.0, 10.0, 10.0]), [18.0; 3]);
        // halfway between x=1 and x=2 at y=0, z=0: 1.5
        assert_eq!(w.sample([15.0, 0.0, 0.0]), [1.5; 3]);
        // a narrow window still answers inside itself
        let w = Window::of(&g, &Aabb::new([19.0, 9.0, 9.0], [21.0, 11.0, 11.0]));
        assert_eq!(w.sample([20.0, 10.0, 10.0]), [18.0; 3]);
    }
}
