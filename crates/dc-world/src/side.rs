//! Blind data (section 7), light grid (section 8) and the two blocker polygon soups (section 9).

use crate::cursor::{le_vec3, Cur, Vec3};
use crate::error::{Result, Section, WorldError};

/// Opaque per-object payload for the game module.
// SPEC: FMT_DAT85.md §7 (engine+0xc2cd0)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlindBlob<'a> {
    pub id: u32,
    pub data: &'a [u8],
}

pub(crate) fn read_blind<'a>(c: &mut Cur<'a>) -> Result<Vec<BlindBlob<'a>>> {
    c.section = Section::BlindData;
    // SPEC: FMT_DAT85.md §7 items 1-2 (engine+0xc2cf0, 0xc2d4b, 0xc2d59, 0xc2d83)
    let n = c.u32()?;
    let mut out = Vec::with_capacity(c.cap(n, 8));
    for _ in 0..n {
        let size = c.u32()?;
        let id = c.u32()?;
        let data = c.take(size as usize)?;
        out.push(BlindBlob { id, data });
    }
    Ok(out)
}

/// Ambient light grid: RGB samples over a regular 3D grid, run-length packed in the file.
// SPEC: FMT_DAT85.md §8 (engine+0x4e470)
#[derive(Debug, Clone, PartialEq)]
pub struct LightGrid<'a> {
    pub origin: Vec3,
    pub cell: Vec3,
    pub dims: [u32; 3],
    /// `grid_packed_len` as declared (dropped by the engine; OBSERVED equal to `packed.len()`).
    pub declared_packed_len: u32,
    /// The packed bytes actually consumed by the expander.
    pub packed: &'a [u8],
}

impl LightGrid<'_> {
    pub fn sample_count(&self) -> u64 {
        self.dims
            .iter()
            .fold(1u64, |a, &d| a.saturating_mul(d as u64))
    }

    /// Expands the packing to `x*y*z*3` bytes (RGB per cell). The packing was validated by the parser, so
    /// the output size is bounded by 32 samples per packed byte.
    // SPEC: FMT_DAT85.md §8 (expander engine+0x4dff0)
    pub fn expand(&self) -> Vec<u8> {
        let total = usize::try_from(self.sample_count().saturating_mul(3)).unwrap_or(0);
        let mut out = Vec::with_capacity(total.min(self.packed.len().saturating_mul(96)));
        let mut p = self.packed;
        while out.len() < total {
            let Some((&ctl, rest)) = p.split_first() else {
                break;
            };
            let run = (ctl & 0x7f) as usize + 1;
            if ctl & 0x80 != 0 {
                let Some(s) = rest.get(..3) else { break };
                for _ in 0..run {
                    out.extend_from_slice(s);
                }
                p = &rest[3..];
            } else {
                let Some(s) = rest.get(..3 * run) else { break };
                out.extend_from_slice(s);
                p = &rest[3 * run..];
            }
        }
        out.truncate(total);
        out
    }
}

pub(crate) fn read_light_grid<'a>(c: &mut Cur<'a>) -> Result<LightGrid<'a>> {
    c.section = Section::LightGrid;
    // SPEC: FMT_DAT85.md §8 items 1-4 (engine+0x4e486, 0x4e494, 0x4e4a1, 0x4e4de)
    let origin = c.vec3()?;
    let cell = c.vec3()?;
    let dims = [c.u32()?, c.u32()?, c.u32()?];
    let declared_packed_len = c.u32()?;
    let total = dims
        .iter()
        .try_fold(1u64, |a, &d| a.checked_mul(d as u64))
        .ok_or(WorldError::CountOverflow {
            section: Section::LightGrid,
            at: c.pos(),
            count: u64::MAX,
        })?;
    // SPEC: FMT_DAT85.md §8 item 5: control byte c, run = (c & 0x7f) + 1; bit 7 = one sample repeated,
    // else `run` literal samples. Scanned here without expanding; repeat until the grid is full.
    let start = c.pos();
    let mut produced = 0u64;
    while produced < total {
        let ctl = c.u8()?;
        let run = (ctl & 0x7f) as u64 + 1;
        if ctl & 0x80 != 0 {
            c.take(3)?;
        } else {
            c.take(3 * run as usize)?;
        }
        produced += run;
    }
    if produced != total {
        return Err(WorldError::LightGridPacking {
            at: start,
            expected_samples: total,
            produced,
        });
    }
    Ok(LightGrid {
        origin,
        cell,
        dims,
        declared_packed_len,
        packed: c.since(start),
    })
}

/// One blocker polygon: plane plus a vertex loop.
// SPEC: FMT_DAT85.md §9.1 (engine+0xc5c70) and §9.2 (engine+0xcd530)
#[derive(Debug, Clone, PartialEq)]
pub struct BlockerPoly {
    pub normal: Vec3,
    /// Distance as stored. The particle-blocker loader negates it on load (FMT_DAT85.md §9.2).
    pub dist: f32,
    pub verts: Vec<Vec3>,
}

/// A blocker polygon soup with its trailing word.
// SPEC: FMT_DAT85.md §9.1 / §9.2 (u32 count, polygons, u32 tail)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PolySoup {
    pub polys: Vec<BlockerPoly>,
    /// Trailing u32 (0 in the ROM).
    pub tail: u32,
}

// SPEC: FMT_DAT85.md §9.1 (engine+0xc7050: count 0xc7065, tail 0xc709f), §9.2 (engine+0xcde30: count
// 0xcde4c, tail 0xcdef5); both use the layout normal, dist, n_verts, verts.
pub(crate) fn read_soup(c: &mut Cur<'_>, section: Section) -> Result<PolySoup> {
    c.section = section;
    let n = c.u32()?;
    let mut polys = Vec::with_capacity(c.cap(n, 20));
    for _ in 0..n {
        let normal = c.vec3()?;
        let dist = c.f32()?;
        let nv = c.u32()?;
        let raw = c.take_n(nv, 12)?;
        let verts = raw.chunks_exact(12).map(|r| le_vec3(r, 0)).collect();
        polys.push(BlockerPoly {
            normal,
            dist,
            verts,
        });
    }
    let tail = c.u32()?;
    Ok(PolySoup { polys, tail })
}
