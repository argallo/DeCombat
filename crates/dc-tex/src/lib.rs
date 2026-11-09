//! Texture container reader and RGBA8 decoder.
//!
//! Written from `rust/specs/FMT_DTX.md` only. All arithmetic on sizes is checked and no function panics on
//! hostile input; malformed buffers yield [`DtxError`].
//!
//! The ROM model applies: this crate carries no texture data and no names. Tests that touch real textures
//! are gated on `CA_ROM`.

mod command;
mod decode;
mod header;
pub mod pcx;
mod write;

pub use command::{parse_command, CommandSet, MaterialDirective, MaterialKind};
pub use header::{DtxHeader, HEADER_LEN};
pub use pcx::{decode_pcx, PcxError, PcxImage};
pub use write::{write_bgra8, write_dxt1_cutout, write_dxt1_mipped};

use std::fmt;

/// Flag bit: six-face (cube) texture. SPEC: FMT_DTX.md §5.
pub const FLAG_SIX_FACE: u32 = 0x400;
/// Bytes skipped after the first face of a six-face texture. SPEC: FMT_DTX.md §4.1.
pub const SIX_FACE_GAP: usize = 0x20;
/// Per-group level-skip bias for `lod_group` 0..=3. SPEC: FMT_DTX.md §4.2.
pub const GROUP_BIAS: [i32; 4] = [0, 0, 1, 1];

/// Errors from parsing or decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DtxError {
    /// Fewer than [`HEADER_LEN`] bytes.
    ShortHeader { len: usize },
    /// Header `kind` word is not 0.
    BadKind(u32),
    /// Header version is not -5.
    BadVersion(i32),
    /// `level_count` outside 1..=8.
    BadLevelCount(u16),
    /// Format code not in the 0..=8 range the engine knows.
    BadFormat(u8),
    /// Level data ends before the chain does.
    Truncated { need: u64, have: u64 },
    /// Size arithmetic overflowed.
    Overflow,
    /// Level index out of range.
    NoSuchLevel(usize),
    /// Format parses but this crate cannot decode it (codes 1, 2, 7, 8).
    UnsupportedDecode(u8),
    /// Level payload shorter than the format needs for decoding (sub-block levels).
    ShortLevel { need: usize, have: usize },
    /// A command statement matched a keyword but had too few arguments.
    CommandArgs(&'static str),
}

impl fmt::Display for DtxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DtxError::ShortHeader { len } => {
                write!(f, "dtx: header needs {HEADER_LEN} bytes, got {len}")
            }
            DtxError::BadKind(k) => write!(f, "dtx: invalid kind word {k:#x}"),
            DtxError::BadVersion(v) => write!(f, "dtx: invalid version {v}"),
            DtxError::BadLevelCount(n) => write!(f, "dtx: invalid level count {n}"),
            DtxError::BadFormat(c) => write!(f, "dtx: unknown format code {c}"),
            DtxError::Truncated { need, have } => {
                write!(f, "dtx: truncated, need {need} bytes, have {have}")
            }
            DtxError::Overflow => write!(f, "dtx: size overflow"),
            DtxError::NoSuchLevel(i) => write!(f, "dtx: no level {i}"),
            DtxError::UnsupportedDecode(c) => write!(f, "dtx: format code {c} cannot be decoded"),
            DtxError::ShortLevel { need, have } => {
                write!(f, "dtx: level has {have} bytes, decoding needs {need}")
            }
            DtxError::CommandArgs(k) => write!(f, "dtx: keyword {k} has too few arguments"),
        }
    }
}

impl std::error::Error for DtxError {}

/// Pixel format of the stored levels. SPEC: FMT_DTX.md §3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Code 1: one byte per texel (size rule only).
    Byte1,
    /// Code 2: 16-bit texels (size rule only).
    Texel16,
    /// Code 3 (and 0): 32-bit, bytes B,G,R,A in memory.
    Bgra8,
    /// Code 4: 4x4 blocks of 8 bytes.
    Dxt1,
    /// Code 5: 4x4 blocks of 16 bytes, explicit alpha.
    Dxt3,
    /// Code 6: 4x4 blocks of 16 bytes, interpolated alpha.
    Dxt5,
    /// Code 7: 8-bit single channel (size rule only).
    Channel8,
    /// Code 8: 24-bit (size rule only).
    Rgb24,
}

impl PixelFormat {
    /// Maps a header format code (0 means 3). SPEC: FMT_DTX.md §3.
    pub fn from_code(code: u8) -> Result<Self, DtxError> {
        Ok(match code {
            0 | 3 => PixelFormat::Bgra8,
            1 => PixelFormat::Byte1,
            2 => PixelFormat::Texel16,
            4 => PixelFormat::Dxt1,
            5 => PixelFormat::Dxt3,
            6 => PixelFormat::Dxt5,
            7 => PixelFormat::Channel8,
            8 => PixelFormat::Rgb24,
            c => return Err(DtxError::BadFormat(c)),
        })
    }

    /// Canonical header code (3 for the 32-bit format).
    pub fn code(self) -> u8 {
        match self {
            PixelFormat::Byte1 => 1,
            PixelFormat::Texel16 => 2,
            PixelFormat::Bgra8 => 3,
            PixelFormat::Dxt1 => 4,
            PixelFormat::Dxt3 => 5,
            PixelFormat::Dxt5 => 6,
            PixelFormat::Channel8 => 7,
            PixelFormat::Rgb24 => 8,
        }
    }

    /// The engine's per-level byte rule: plain `w*h*bpp` or `w*h/2`, no rounding to blocks.
    /// SPEC: FMT_DTX.md §3 (EVIDENCE: engine+0x82c70).
    pub fn level_bytes(self, w: u32, h: u32) -> Option<u64> {
        let px = (w as u64).checked_mul(h as u64)?;
        Some(match self {
            PixelFormat::Byte1 | PixelFormat::Channel8 => px,
            PixelFormat::Texel16 => px.checked_mul(2)?,
            PixelFormat::Bgra8 => px.checked_mul(4)?,
            PixelFormat::Rgb24 => px.checked_mul(3)?,
            PixelFormat::Dxt1 => px / 2,
            PixelFormat::Dxt3 | PixelFormat::Dxt5 => px,
        })
    }
}

/// One retained level of the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LevelRec {
    width: u32,
    height: u32,
    len: usize,
    /// Offsets into `DtxFile::data` of each face (only the first `faces` entries are used).
    face_start: [usize; 6],
}

/// A parsed texture. Owns the level payload bytes (header excluded).
#[derive(Debug, Clone, PartialEq)]
pub struct DtxFile {
    header: DtxHeader,
    format: PixelFormat,
    skip: usize,
    faces: usize,
    levels: Vec<LevelRec>,
    data: Vec<u8>,
    trailing: usize,
}

/// Level-skip count applied at load time. SPEC: FMT_DTX.md §4.2 (EVIDENCE: engine+0x3e0e0).
pub fn load_skip(lod_group: u8, level_count: u16, global_bias: i32) -> usize {
    let bias = match GROUP_BIAS.get(lod_group as usize) {
        Some(g) => global_bias.saturating_add(*g),
        None => global_bias,
    };
    let cap = (level_count as usize).saturating_sub(1);
    (bias.max(0) as usize).min(cap)
}

impl DtxFile {
    /// Parses a whole file with no global skip bias.
    pub fn parse(bytes: &[u8]) -> Result<DtxFile, DtxError> {
        Self::parse_with_bias(bytes, 0)
    }

    /// Parses a whole file; `global_bias` is the engine's run-time level-skip setting (SPEC: FMT_DTX.md §4.2).
    /// Skipped levels are not copied. Bytes after the chain are tolerated and counted by [`DtxFile::trailing`].
    pub fn parse_with_bias(bytes: &[u8], global_bias: i32) -> Result<DtxFile, DtxError> {
        let header = DtxHeader::parse(bytes)?;
        let format = PixelFormat::from_code(header.format)?;
        let faces = if header.flags & FLAG_SIX_FACE != 0 {
            6
        } else {
            1
        };
        let skip = load_skip(header.lod_group, header.level_count, global_bias);
        let body = bytes.get(HEADER_LEN..).unwrap_or(&[]);

        // Sizes of every stored level, skipped ones included (they are seeked over).
        // SPEC: FMT_DTX.md §4.1: w_i = width >> i, h_i = height >> i.
        let n = header.level_count as usize;
        let mut sizes = Vec::with_capacity(n);
        for i in 0..n {
            let w = (header.width as u32) >> i;
            let h = (header.height as u32) >> i;
            let len = format.level_bytes(w, h).ok_or(DtxError::Overflow)?;
            sizes.push((w, h, len));
        }
        let chain: u64 = sizes
            .iter()
            .try_fold(0u64, |a, s| a.checked_add(s.2))
            .ok_or(DtxError::Overflow)?;
        let total = if faces == 6 {
            chain
                .checked_mul(6)
                .and_then(|v| v.checked_add(SIX_FACE_GAP as u64))
                .ok_or(DtxError::Overflow)?
        } else {
            chain
        };
        if (body.len() as u64) < total {
            return Err(DtxError::Truncated {
                need: total,
                have: body.len() as u64,
            });
        }

        // File layout is face-major with one gap after the first face. SPEC: FMT_DTX.md §4.1.
        let mut file_off = vec![[0usize; 6]; n];
        let mut pos = 0usize;
        for face in 0..faces {
            for (off, s) in file_off.iter_mut().zip(&sizes) {
                off[face] = pos;
                pos += s.2 as usize;
            }
            if face == 0 && faces == 6 {
                pos += SIX_FACE_GAP;
            }
        }
        let mut levels = Vec::with_capacity(n - skip);
        let mut data = Vec::new();
        for i in skip..n {
            let (w, h, len) = sizes[i];
            let len = len as usize;
            let mut face_start = [0usize; 6];
            for face in 0..faces {
                face_start[face] = data.len();
                let fo = file_off[i][face];
                data.extend_from_slice(&body[fo..fo + len]);
            }
            levels.push(LevelRec {
                width: w,
                height: h,
                len,
                face_start,
            });
        }
        Ok(DtxFile {
            header,
            format,
            skip,
            faces,
            levels,
            data,
            trailing: body.len() - total as usize,
        })
    }

    /// The raw header.
    pub fn header(&self) -> &DtxHeader {
        &self.header
    }

    /// Stored pixel format.
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Levels dropped at load time.
    pub fn skipped(&self) -> usize {
        self.skip
    }

    /// Number of retained levels.
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Faces per level (1, or 6 for cube textures).
    pub fn face_count(&self) -> usize {
        self.faces
    }

    /// Width of retained level `i`.
    pub fn level_width(&self, i: usize) -> Option<u32> {
        self.levels.get(i).map(|l| l.width)
    }

    /// Height of retained level `i`.
    pub fn level_height(&self, i: usize) -> Option<u32> {
        self.levels.get(i).map(|l| l.height)
    }

    /// Payload of retained level `i` (face 0). Empty slice when `i` is out of range.
    pub fn level(&self, i: usize) -> &[u8] {
        self.face(i, 0)
    }

    /// Payload of face `f` of retained level `i`. Empty slice when out of range.
    pub fn face(&self, i: usize, f: usize) -> &[u8] {
        match self.levels.get(i) {
            Some(l) if f < self.faces => {
                let s = l.face_start[f];
                self.data.get(s..s + l.len).unwrap_or(&[])
            }
            _ => &[],
        }
    }

    /// Bytes after the level chain that were present in the input.
    pub fn trailing(&self) -> usize {
        self.trailing
    }

    /// Parsed command string. SPEC: FMT_DTX.md §7.
    pub fn commands(&self) -> Result<CommandSet, DtxError> {
        parse_command(&self.header.command_str())
    }

    /// Decodes retained level `i` (face 0) to RGBA8, row-major, `w*h*4` bytes.
    /// Out-of-range levels and formats without a decoder give an empty vector; short block data is
    /// zero-filled. Use [`DtxFile::try_decode_rgba8`] to see the reason.
    pub fn decode_rgba8(&self, level: usize) -> Vec<u8> {
        decode::decode_level(
            self.format,
            self.level_width(level).unwrap_or(0),
            self.level_height(level).unwrap_or(0),
            self.level(level),
            false,
        )
        .unwrap_or_default()
    }

    /// Strict variant of [`DtxFile::decode_rgba8`] for a given face.
    pub fn try_decode_rgba8(&self, level: usize, face: usize) -> Result<Vec<u8>, DtxError> {
        let l = self.levels.get(level).ok_or(DtxError::NoSuchLevel(level))?;
        if face >= self.faces {
            return Err(DtxError::NoSuchLevel(level));
        }
        decode::decode_level(self.format, l.width, l.height, self.face(level, face), true)
    }
}
