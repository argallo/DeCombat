//! One typed error for every parser in this crate.

use std::fmt;

/// Why a buffer was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FxError {
    /// Zero-length input (the ROM holds one empty sprite entry; the engine rejects it).
    Empty,
    /// The buffer ended inside `what` at byte `offset`.
    Truncated { what: &'static str, offset: usize },
    /// A sprite frame name longer than 1000 bytes (the engine rule), or a script path above its bound.
    NameTooLong { what: &'static str, len: usize },
    /// A version field with an unsupported value.
    BadVersion { what: &'static str, found: u32 },
    /// A field outside its allowed range.
    BadField { what: &'static str, value: u32 },
    /// A count above the parser bound.
    TooMany { what: &'static str, count: u32 },
    /// Text nesting deeper than the parser allows.
    TooDeep,
    /// A family the runtime never reads (tier DROP); `family` names it.
    Unsupported { family: &'static str },
}

impl fmt::Display for FxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FxError::Empty => write!(f, "empty input"),
            FxError::Truncated { what, offset } => {
                write!(f, "truncated in {what} at byte {offset}")
            }
            FxError::NameTooLong { what, len } => write!(f, "{what} too long ({len} bytes)"),
            FxError::BadVersion { what, found } => write!(f, "unsupported {what} version {found}"),
            FxError::BadField { what, value } => write!(f, "bad {what}: {value}"),
            FxError::TooMany { what, count } => write!(f, "too many {what}: {count}"),
            FxError::TooDeep => write!(f, "nesting too deep"),
            FxError::Unsupported { family } => {
                write!(
                    f,
                    "{family} is an authoring leftover the runtime never reads (tier DROP)"
                )
            }
        }
    }
}

impl std::error::Error for FxError {}

/// Little-endian cursor over a byte slice; every read is bounds-checked.
pub(crate) struct Bin<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Bin<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Bin { data, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    pub fn bytes(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], FxError> {
        if n > self.remaining() {
            return Err(FxError::Truncated {
                what,
                offset: self.pos,
            });
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self, what: &'static str) -> Result<u8, FxError> {
        Ok(self.bytes(1, what)?[0])
    }
    pub fn u16(&mut self, what: &'static str) -> Result<u16, FxError> {
        let b = self.bytes(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn u32(&mut self, what: &'static str) -> Result<u32, FxError> {
        let b = self.bytes(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn f32(&mut self, what: &'static str) -> Result<f32, FxError> {
        Ok(f32::from_bits(self.u32(what)?))
    }
}

/// Bytes up to the first NUL, one char per byte (Latin-1), at most `max` bytes.
pub(crate) fn latin1_cstr(b: &[u8], max: usize) -> String {
    let b = &b[..b.len().min(max)];
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect()
}
