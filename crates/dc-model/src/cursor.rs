//! Bounds-checked little-endian reader and the matching writer helpers.

use crate::error::ModelError;
use crate::RawStr;

/// Longest string the reader accepts. The engine reads strings into a 0x1000-byte buffer; we refuse any
/// length that would not leave room for a terminator there (the exact engine bound is unverified).
// SPEC: FMT_LTB.md preamble (str = u16 length + bytes, 0x1000-byte buffer, engine+0x6db70)
pub const STR_MAX: usize = 0x0fff;

pub(crate) struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], ModelError> {
        if n > self.remaining() {
            return Err(ModelError::Truncated {
                offset: self.pos,
                need: n - self.remaining(),
            });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn arr<const N: usize>(&mut self) -> Result<[u8; N], ModelError> {
        let s = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }

    pub fn u8(&mut self) -> Result<u8, ModelError> {
        Ok(self.arr::<1>()?[0])
    }
    pub fn u16(&mut self) -> Result<u16, ModelError> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    pub fn i16(&mut self) -> Result<i16, ModelError> {
        Ok(i16::from_le_bytes(self.arr()?))
    }
    pub fn u32(&mut self) -> Result<u32, ModelError> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    pub fn f32(&mut self) -> Result<f32, ModelError> {
        Ok(f32::from_le_bytes(self.arr()?))
    }
    pub fn f32s<const N: usize>(&mut self) -> Result<[f32; N], ModelError> {
        let mut out = [0f32; N];
        for v in out.iter_mut() {
            *v = self.f32()?;
        }
        Ok(out)
    }
    pub fn i16s<const N: usize>(&mut self) -> Result<[i16; N], ModelError> {
        let mut out = [0i16; N];
        for v in out.iter_mut() {
            *v = self.i16()?;
        }
        Ok(out)
    }
    pub fn u32s<const N: usize>(&mut self) -> Result<[u32; N], ModelError> {
        let mut out = [0u32; N];
        for v in out.iter_mut() {
            *v = self.u32()?;
        }
        Ok(out)
    }

    pub fn string(&mut self) -> Result<RawStr, ModelError> {
        let at = self.pos;
        let len = self.u16()? as usize;
        if len > STR_MAX {
            return Err(ModelError::StringTooLong { offset: at, len });
        }
        Ok(RawStr(self.take(len)?.to_vec()))
    }

    /// Checks that `count` items of at least `min_item` bytes each can still fit, so a hostile count can
    /// never trigger a huge allocation.
    pub fn room(
        &self,
        what: &'static str,
        at: usize,
        count: u64,
        min_item: usize,
    ) -> Result<usize, ModelError> {
        let need = count
            .checked_mul(min_item as u64)
            .ok_or(ModelError::Overflow { offset: at })?;
        if need > self.remaining() as u64 {
            return Err(ModelError::CountTooLarge {
                what,
                offset: at,
                count,
            });
        }
        usize::try_from(count).map_err(|_| ModelError::Overflow { offset: at })
    }

    /// Reads a `u32` count and checks it against the remaining input.
    pub fn count(&mut self, what: &'static str, min_item: usize) -> Result<usize, ModelError> {
        let at = self.pos;
        let n = self.u32()?;
        self.room(what, at, n as u64, min_item)
    }

    /// Reads `a * b` bytes with checked multiplication.
    pub fn block(&mut self, a: u32, b: u32) -> Result<&'a [u8], ModelError> {
        let at = self.pos;
        let n = (a as u64)
            .checked_mul(b as u64)
            .ok_or(ModelError::Overflow { offset: at })?;
        if n > self.remaining() as u64 {
            let short = n - self.remaining() as u64;
            return Err(ModelError::Truncated {
                offset: at,
                need: usize::try_from(short).unwrap_or(usize::MAX),
            });
        }
        self.take(n as usize)
    }
}

/// Output buffer with the encodings matching [`Cursor`].
#[derive(Default)]
pub(crate) struct Out {
    pub buf: Vec<u8>,
}

impl Out {
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i16(&mut self, v: i16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f32s(&mut self, v: &[f32]) {
        for x in v {
            self.f32(*x);
        }
    }
    pub fn i16s(&mut self, v: &[i16]) {
        for x in v {
            self.i16(*x);
        }
    }
    pub fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }
    pub fn len_u32(&mut self, n: usize, what: &'static str) -> Result<(), ModelError> {
        self.u32(u32::try_from(n).map_err(|_| ModelError::Encode { what })?);
        Ok(())
    }
    pub fn string(&mut self, s: &RawStr) -> Result<(), ModelError> {
        if s.0.len() > STR_MAX {
            return Err(ModelError::Encode {
                what: "string longer than the engine buffer",
            });
        }
        self.u16(s.0.len() as u16);
        self.bytes(&s.0);
        Ok(())
    }
}
