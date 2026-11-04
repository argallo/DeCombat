//! Minimal, hostile-input-safe 32-bit Portable Executable reader (public PE/COFF format).

use crate::error::RomError;
use std::fs;
use std::path::Path;

#[inline]
pub(crate) fn read_u16_le(d: &[u8], off: usize) -> Result<u16, RomError> {
    let end = off
        .checked_add(2)
        .ok_or(RomError::pe("u16 read", off as u64))?;
    let s = d
        .get(off..end)
        .ok_or(RomError::pe("u16 read", off as u64))?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

#[inline]
pub(crate) fn read_u32_le(d: &[u8], off: usize) -> Result<u32, RomError> {
    let end = off
        .checked_add(4)
        .ok_or(RomError::pe("u32 read", off as u64))?;
    let s = d
        .get(off..end)
        .ok_or(RomError::pe("u32 read", off as u64))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

#[inline]
pub(crate) fn read_i32_le(d: &[u8], off: usize) -> Result<i32, RomError> {
    read_u32_le(d, off).map(|v| v as i32)
}

#[inline]
pub(crate) fn read_i8(d: &[u8], off: usize) -> Result<i8, RomError> {
    d.get(off)
        .map(|&b| b as i8)
        .ok_or(RomError::pe("i8 read", off as u64))
}

/// Parsed 32-bit PE image: raw bytes plus the section table.
#[derive(Clone, Debug)]
pub struct PeFile {
    pub data: Vec<u8>,
    pub base: u32,
    /// `(va, vs, raw, rs)` per section
    pub secs: Vec<(u32, u32, u32, u32)>,
}

impl PeFile {
    pub fn from_file(path: &Path) -> Result<Self, RomError> {
        let data = fs::read(path).map_err(|e| RomError::io(path, &e))?;
        crate::trace::note("file", path, "", data.len() as u64);
        crate::journal::note(|| format!("file:{}", path.display()), &data);
        Self::from_bytes(data)
    }

    /// Parses the DOS/NT headers and the section table. Every offset is bounds- and overflow-checked.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, RomError> {
        let pe = read_u32_le(&data, 0x3c)? as usize;
        let at = |d: usize| -> Result<usize, RomError> {
            pe.checked_add(d)
                .ok_or(RomError::pe("nt header", pe as u64))
        };
        let nsec = read_u16_le(&data, at(6)?)? as usize;
        let nopt = read_u16_le(&data, at(20)?)? as usize;
        let opt = at(24)?;
        let base = read_u32_le(
            &data,
            opt.checked_add(28)
                .ok_or(RomError::pe("image base", opt as u64))?,
        )?;
        let mut secs = Vec::with_capacity(nsec.min(96));
        for i in 0..nsec {
            let s = opt
                .checked_add(nopt)
                .and_then(|v| v.checked_add(i.checked_mul(40)?))
                .ok_or(RomError::pe("section table", opt as u64))?;
            let f = |d: usize| -> Result<u32, RomError> {
                read_u32_le(
                    &data,
                    s.checked_add(d)
                        .ok_or(RomError::pe("section entry", s as u64))?,
                )
            };
            let vs = f(8)?;
            let va = f(12)?;
            let rs = f(16)?;
            let raw = f(20)?;
            secs.push((va, vs, raw, rs));
        }
        Ok(Self { data, base, secs })
    }

    /// Returns a slice of `len` bytes at Relative Virtual Address `rva`.
    pub fn rva(&self, rva: u32, len: usize) -> Result<&[u8], RomError> {
        let oob = || RomError::RvaOutOfRange {
            rva: rva as u64,
            len: len as u64,
        };
        let r = rva as u64;
        let l = len as u64;
        for &(va, vs, raw, rs) in &self.secs {
            let ext = (if vs != 0 { vs.min(rs) } else { rs }) as u64;
            let va = va as u64;
            // Saturating: `len` may be up to u64::MAX on 64-bit targets.
            if va <= r && r.saturating_add(l) <= va + ext {
                let start = (raw as u64) + (r - va);
                let end = start + l;
                // usize is 32-bit on the i686 target: convert checked.
                let s = usize::try_from(start).map_err(|_| oob())?;
                let e = usize::try_from(end).map_err(|_| oob())?;
                return self.data.get(s..e).ok_or_else(oob);
            }
        }
        Err(oob())
    }

    /// Maps a VA (or RVA if `< base`) to an RVA.
    pub(crate) fn to_rva(&self, va: u32) -> u32 {
        if va >= self.base {
            va - self.base
        } else {
            va
        }
    }

    /// Reads `len` bytes at Virtual Address `va` (or RVA if `< self.base`), returning zeros if the
    /// range falls inside the uninitialized tail (`vs > rs`) of a section.
    pub fn rva_or_zero(&self, va: u32, len: usize) -> Result<Vec<u8>, RomError> {
        let r = self.to_rva(va);
        if let Ok(slice) = self.rva(r, len) {
            return Ok(slice.to_vec());
        }
        let (r64, l64) = (r as u64, len as u64);
        for &(sva, vs, _raw, rs) in &self.secs {
            let (sva, vs, rs) = (sva as u64, vs as u64, rs as u64);
            if vs > rs && sva + rs <= r64 && r64.saturating_add(l64) <= sva + vs {
                return Ok(vec![0u8; len]);
            }
        }
        Err(RomError::RvaOutOfRange { rva: r64, len: l64 })
    }

    /// Reads a NUL-terminated UTF-16LE wide string at `rva` (or VA if `>= self.base`).
    pub fn wstr(&self, rva: u32) -> Result<String, RomError> {
        let mut r = self.to_rva(rva);
        let mut units = Vec::new();
        loop {
            let bytes = self.rva(r, 2)?;
            let c = u16::from_le_bytes([bytes[0], bytes[1]]);
            if c == 0 {
                break;
            }
            units.push(c);
            r = r.checked_add(2).ok_or(RomError::RvaOutOfRange {
                rva: r as u64,
                len: 2,
            })?;
        }
        Ok(String::from_utf16(&units).unwrap_or_else(|_| {
            units
                .iter()
                .filter_map(|&u| char::from_u32(u as u32))
                .collect()
        }))
    }

    /// Reads a NUL-terminated Latin-1 C string at Virtual Address `va` (or RVA if `< self.base`).
    pub fn cstr(&self, va: u32) -> Result<String, RomError> {
        let mut r = self.to_rva(va);
        let mut out = String::new();
        loop {
            let c = self.rva(r, 1)?[0];
            if c == 0 {
                return Ok(out);
            }
            out.push(c as char);
            r = r.checked_add(1).ok_or(RomError::RvaOutOfRange {
                rva: r as u64,
                len: 1,
            })?;
        }
    }
}

/// Test helper: builds a minimal valid PE with one section `(va, raw bytes)` and image base `base`.
#[cfg(test)]
pub(crate) fn synth_pe(base: u32, va: u32, payload: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 0x200];
    d[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    let pe = 0x80;
    d[pe..pe + 4].copy_from_slice(b"PE\0\0");
    d[pe + 6..pe + 8].copy_from_slice(&1u16.to_le_bytes());
    d[pe + 20..pe + 22].copy_from_slice(&0xe0u16.to_le_bytes());
    d[pe + 24 + 28..pe + 24 + 32].copy_from_slice(&base.to_le_bytes());
    let s = pe + 24 + 0xe0;
    d[s + 8..s + 12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    d[s + 12..s + 16].copy_from_slice(&va.to_le_bytes());
    d[s + 16..s + 20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    d[s + 20..s + 24].copy_from_slice(&0x400u32.to_le_bytes());
    d.resize(0x400, 0);
    d.extend_from_slice(payload);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synthetic_pe_and_reads_rva() {
        let mut payload = b"hello\0".to_vec();
        payload.extend_from_slice(&[b'w', 0, 0, 0]);
        let pe = PeFile::from_bytes(synth_pe(0x40_0000, 0x1000, &payload)).unwrap();
        assert_eq!(pe.rva(0x1000, 5).unwrap(), b"hello");
        assert_eq!(pe.cstr(0x40_1000).unwrap(), "hello");
        assert_eq!(pe.wstr(0x1006).unwrap(), "w");
    }

    #[test]
    fn rejects_out_of_range_and_overflow() {
        let pe = PeFile::from_bytes(synth_pe(0x40_0000, 0x1000, b"abcd")).unwrap();
        assert!(pe.rva(0x1000, 5).is_err());
        assert!(pe.rva(0xffff_fffe, 16).is_err());
        assert!(pe.rva(0x1000, usize::MAX).is_err());
        assert!(pe.rva_or_zero(0xffff_ffff, 4).is_err());
    }

    #[test]
    fn rejects_hostile_headers() {
        let mut d = vec![0u8; 0x100];
        d[0x3c..0x40].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        assert!(PeFile::from_bytes(d).is_err());
        assert!(PeFile::from_bytes(Vec::new()).is_err());
        let mut d = synth_pe(0x40_0000, 0x1000, b"abcd");
        d[0x80 + 20..0x80 + 22].copy_from_slice(&0xffffu16.to_le_bytes());
        assert!(PeFile::from_bytes(d).is_err());
    }
}
