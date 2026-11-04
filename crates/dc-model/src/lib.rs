//! Reader and writer for the 2009 model container (`.ltb`), plus the render-style files that share the
//! extension (file kind 5, module [`style`], written from our own disassembly of the 2009 loader).
//!
//! Written only from `rust/specs/FMT_LTB.md` (cited as `SPEC: FMT_LTB.md §n`) and the public Direct3D vertex
//! format size rules. Fields the spec leaves unidentified are kept as opaque values named `todo_*`, so a parsed
//! file can be written back byte for byte by [`Model::to_bytes`]. No ROM data lives in this crate.
//!
//! Entry point: [`Model::parse`]. The reader never panics on hostile input: every count is checked against the
//! remaining bytes before anything is allocated and all size arithmetic is checked.

mod cursor;
mod error;
mod model;
mod parse;
pub mod style;
pub mod vertex;
mod write;

pub use cursor::STR_MAX;
pub use error::ModelError;
pub use model::*;
pub use parse::FileKind;
pub use style::{StyleError, StyleFile, StyleMaterial, StylePass, StyleStage, StyleVariant};
pub use vertex::{VertexLayout, VertexStream};

/// A `u16`-length-prefixed byte string as stored in the file (no terminator, encoding not specified).
// SPEC: FMT_LTB.md preamble (str = u16 length + bytes)
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
pub struct RawStr(pub Vec<u8>);

impl RawStr {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Lossy text view (bytes >= 0x80 are mapped as Latin-1).
    pub fn to_text(&self) -> String {
        self.0.iter().map(|&b| b as char).collect()
    }
}

impl From<&str> for RawStr {
    fn from(s: &str) -> Self {
        RawStr(s.as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests;
