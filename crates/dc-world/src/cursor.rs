//! Bounds-checked little-endian reader over a borrowed byte slice.

use crate::error::{Result, Section, WorldError};

/// Three little-endian f32 (12 bytes).
// SPEC: FMT_DAT85.md (preamble: vec3)
pub type Vec3 = [f32; 3];

#[derive(Clone)]
pub(crate) struct Cur<'a> {
    data: &'a [u8],
    pos: usize,
    pub section: Section,
}

impl<'a> Cur<'a> {
    pub fn at(data: &'a [u8], pos: usize, section: Section) -> Self {
        Cur { data, pos, section }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    /// The bytes read since position `start` (empty if `start` is ahead).
    pub fn since(&self, start: usize) -> &'a [u8] {
        self.data.get(start..self.pos).unwrap_or(&[])
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or(WorldError::Truncated {
                section: self.section,
                at: self.pos,
                need: n,
            })?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// `count` records of `size` bytes as one slice, bounds-checked before anything is allocated.
    pub fn take_n(&mut self, count: u32, size: usize) -> Result<&'a [u8]> {
        let total = (count as u64)
            .checked_mul(size as u64)
            .and_then(|t| usize::try_from(t).ok())
            .ok_or(WorldError::CountOverflow {
                section: self.section,
                at: self.pos,
                count: count as u64,
            })?;
        self.take(total)
    }

    /// Capacity hint that never exceeds what the remaining bytes could hold.
    pub fn cap(&self, count: u32, min_size: usize) -> usize {
        (count as usize).min(self.remaining() / min_size.max(1))
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub fn vec3(&mut self) -> Result<Vec3> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }

    /// u16 byte count, then that many bytes, no terminator.
    // SPEC: FMT_DAT85.md (preamble: lstr; reader engine+0x44b80)
    pub fn lstr(&mut self) -> Result<&'a [u8]> {
        let n = self.u16()? as usize;
        self.take(n)
    }
}

pub(crate) fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

pub(crate) fn le_f32(b: &[u8], off: usize) -> f32 {
    f32::from_bits(le_u32(b, off))
}

pub(crate) fn le_vec3(b: &[u8], off: usize) -> Vec3 {
    [le_f32(b, off), le_f32(b, off + 4), le_f32(b, off + 8)]
}
