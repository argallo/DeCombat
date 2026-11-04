//! Reader and writer for the 2009 render-style file: the `.ltb` files with file kind 5 that a model piece's
//! render-style slot points at (material, up to four passes of fixed-function texture-stage and raster state).
//!
//! Written from our own disassembly of the 2009 `Engine.exe` (read-only capstone over the user's ROM):
//! the file loader is the render-style object's slot 12 (engine+0x161bb0, vtable VA 0x689870, constructor
//! engine+0x162710), the pass list is filled through slot 3 (engine+0x1626a0, at most four passes) and slot 10
//! (engine+0x162250, optional shader block), the trailing 8-byte block through slot 8 (engine+0x162480). The
//! meaning of the pass words comes from the pass setup engine+0x1a88f0 and the texture binding engine+0x1a8240
//! (see the draft spec `FMT_RENDERSTYLE_2009.md`). The values stay the engine's own enum numbers here; the
//! translation to Direct3D semantics lives in the renderer. No ROM data is in this module.
//!
//! Every count is checked against the remaining input before anything is allocated; the reader never panics.

use std::fmt;

/// Value of the file-kind byte of a render-style file.
// DISASM: engine+0x161bdc (`cmp byte [hdr], 5`)
pub const STYLE_FILE_KIND: u8 = 5;
/// Value of the container revision word of a render-style file.
// DISASM: engine+0x161be3 (`cmp word [hdr+2], 3`)
pub const STYLE_CONTAINER_REV: u16 = 3;
/// Length of the header block the loader reads first.
// DISASM: engine+0x161bc4 (`push 0x14` read)
pub const STYLE_HEADER_LEN: usize = 0x14;
/// Largest data length the loader accepts.
// DISASM: engine+0x161bf9 (`cmp dword [len], 0xf4240; ja fail`)
pub const STYLE_MAX_DATA_LEN: u32 = 1_000_000;
/// Largest variant count the loader accepts (it also refuses 0).
// DISASM: engine+0x161c15 (`cmp eax, 0x20; jbe`), engine+0x161c2f (0 -> fail)
pub const STYLE_MAX_VARIANTS: u32 = 32;
/// Most passes a render style keeps; the add-pass method refuses a fifth, which fails the whole load.
// DISASM: engine+0x1626a0 (`if count < 4` before the insert), engine+0x161c9d (a refused pass fails the load)
pub const STYLE_MAX_PASSES: u32 = 4;
/// Size of one pass record as stored (and as the engine copies it).
// DISASM: engine+0x161c72 (`push 0x214` read), engine+0x1625d7 (0x85 dwords copied out)
pub const PASS_RECORD_LEN: usize = 0x214;
/// Size of one texture-stage record inside a pass.
// DISASM: engine+0x1a8d8f (`imul edi, edi, 0x78`), engine+0x1a82b3 (`add ebx, 0x78`)
pub const STAGE_RECORD_LEN: usize = 0x78;
/// Size of the lighting material block of a variant.
// DISASM: engine+0x161c48 (`push 0x44` read into the style +0x0c)
pub const MATERIAL_LEN: usize = 0x44;

/// Everything that can go wrong while reading a render-style file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleError {
    /// The input ended before a field the loader reads unconditionally.
    Truncated { offset: usize, need: usize },
    /// Header byte 0 is not 5 or word 2 is not 3.
    NotAStyle { file_kind: u8, rev: u16 },
    /// The data length word is above 1,000,000.
    DataLen { value: u32 },
    /// The variant count is 0 or above 32.
    VariantCount { value: u32 },
    /// A variant declares more than four passes.
    TooManyPasses { variant: usize, count: u32 },
    /// The in-memory value cannot be encoded.
    Encode { what: &'static str },
}

impl fmt::Display for StyleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StyleError::Truncated { offset, need } => {
                write!(
                    f,
                    "render style truncated at {offset:#x}: need {need} more bytes"
                )
            }
            StyleError::NotAStyle { file_kind, rev } => {
                write!(
                    f,
                    "not a render-style file (kind {file_kind}, revision {rev})"
                )
            }
            StyleError::DataLen { value } => {
                write!(f, "render-style data length {value} too large")
            }
            StyleError::VariantCount { value } => {
                write!(f, "render-style variant count {value} outside 1..32")
            }
            StyleError::TooManyPasses { variant, count } => {
                write!(
                    f,
                    "render-style variant {variant} has {count} passes (at most 4)"
                )
            }
            StyleError::Encode { what } => write!(f, "cannot encode render style: {what}"),
        }
    }
}

impl std::error::Error for StyleError {}

struct Rd<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], StyleError> {
        let left = self.buf.len() - self.pos;
        if n > left {
            return Err(StyleError::Truncated {
                offset: self.pos,
                need: n - left,
            });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, StyleError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, StyleError> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
}

fn word(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn real(b: &[u8], off: usize) -> f32 {
    f32::from_bits(word(b, off))
}

/// The lighting material of a variant (four RGBA colours and the specular power), in file order.
///
/// File order -> device material: ambient, diffuse, emissive, specular, power (the pass setup copies style
/// +0x0c.. into the material's ambient, +0x1c.. into diffuse, +0x2c.. into emissive, +0x3c.. into specular,
/// +0x4c into power; with a black specular colour the power is forced to 0).
// DISASM: engine+0x1a89c4..0x1a8ac1 (copy of style +0x0c..+0x4c into the device material block)
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StyleMaterial {
    pub ambient: [f32; 4],
    pub diffuse: [f32; 4],
    pub emissive: [f32; 4],
    pub specular: [f32; 4],
    pub power: f32,
}

impl StyleMaterial {
    fn from_bytes(b: &[u8]) -> StyleMaterial {
        let v4 = |o: usize| [real(b, o), real(b, o + 4), real(b, o + 8), real(b, o + 12)];
        StyleMaterial {
            ambient: v4(0x00),
            diffuse: v4(0x10),
            emissive: v4(0x20),
            specular: v4(0x30),
            power: real(b, 0x40),
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        for v in [self.ambient, self.diffuse, self.emissive, self.specular] {
            for x in v {
                out.extend_from_slice(&x.to_le_bytes());
            }
        }
        out.extend_from_slice(&self.power.to_le_bytes());
    }
}

/// One texture stage of a pass (engine enum values, translated by the renderer).
///
/// Word offsets inside the 0x78-byte record: +0x00 texture source, +0x04 colour op, +0x08 / +0x0c colour
/// arguments 1 / 2, +0x10 alpha op, +0x14 / +0x18 alpha arguments 1 / 2, +0x1c texture-coordinate source,
/// +0x20 / +0x24 U / V addressing, +0x28 filter, +0x2c transform switch (byte), +0x30 4x4 texture matrix,
/// +0x70 projected switch (byte), +0x74 transform coordinate count.
// DISASM: engine+0x1a8d92..0x1a90ba (per-stage reads at stage +0x04..+0x74), engine+0x1a8278 (stage +0x00 picks
// the piece texture: 0 none, 1..4 the piece's texture 0..3)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleStage {
    pub texture: u32,
    pub colour_op: u32,
    pub colour_arg1: u32,
    pub colour_arg2: u32,
    pub alpha_op: u32,
    pub alpha_arg1: u32,
    pub alpha_arg2: u32,
    pub uv_source: u32,
    pub address_u: u32,
    pub address_v: u32,
    pub filter: u32,
    /// Whole word at +0x2c; the engine tests its low byte.
    pub transform_word: u32,
    /// Row-major 4x4 matrix as stored (handed to SetTransform as is).
    pub matrix: [f32; 16],
    /// Whole word at +0x70; the engine tests its low byte.
    pub projected_word: u32,
    pub transform_count: u32,
}

impl Default for StyleStage {
    fn default() -> Self {
        let mut matrix = [0f32; 16];
        for k in 0..4 {
            matrix[k * 5] = 1.0;
        }
        StyleStage {
            texture: 0,
            colour_op: 0,
            colour_arg1: 0,
            colour_arg2: 0,
            alpha_op: 0,
            alpha_arg1: 0,
            alpha_arg2: 0,
            uv_source: 0,
            address_u: 0,
            address_v: 0,
            filter: 0,
            transform_word: 0,
            matrix,
            projected_word: 0,
            transform_count: 0,
        }
    }
}

impl StyleStage {
    /// The texture-transform switch (low byte of the word at +0x2c).
    pub fn transform(&self) -> bool {
        self.transform_word & 0xff != 0
    }
    /// The projected-coordinates switch (low byte of the word at +0x70).
    pub fn projected(&self) -> bool {
        self.projected_word & 0xff != 0
    }

    fn from_bytes(b: &[u8]) -> StyleStage {
        let mut matrix = [0f32; 16];
        for (k, m) in matrix.iter_mut().enumerate() {
            *m = real(b, 0x30 + 4 * k);
        }
        StyleStage {
            texture: word(b, 0x00),
            colour_op: word(b, 0x04),
            colour_arg1: word(b, 0x08),
            colour_arg2: word(b, 0x0c),
            alpha_op: word(b, 0x10),
            alpha_arg1: word(b, 0x14),
            alpha_arg2: word(b, 0x18),
            uv_source: word(b, 0x1c),
            address_u: word(b, 0x20),
            address_v: word(b, 0x24),
            filter: word(b, 0x28),
            transform_word: word(b, 0x2c),
            matrix,
            projected_word: word(b, 0x70),
            transform_count: word(b, 0x74),
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        for w in [
            self.texture,
            self.colour_op,
            self.colour_arg1,
            self.colour_arg2,
            self.alpha_op,
            self.alpha_arg1,
            self.alpha_arg2,
            self.uv_source,
            self.address_u,
            self.address_v,
            self.filter,
            self.transform_word,
        ] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        for m in self.matrix {
            out.extend_from_slice(&m.to_le_bytes());
        }
        out.extend_from_slice(&self.projected_word.to_le_bytes());
        out.extend_from_slice(&self.transform_count.to_le_bytes());
    }
}

/// The optional 10-byte block after a pass record (a flag byte selects it): stored by the engine as four
/// words (byte, word, byte, word) and kept with the pass (engine+0x162250 copies 16 bytes into the pass node);
/// its use by the draw code was not traced, so the fields keep neutral names.
// DISASM: engine+0x161cab..0x161ced (reads of 1, 4, 1, 4 bytes, then slot 10 with the pass index)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PassExtra {
    pub todo_a: u8,
    pub todo_b: u32,
    pub todo_c: u8,
    pub todo_d: u32,
}

/// One pass: four texture stages and the raster state words (engine enum values).
///
/// Offsets after the stages: +0x1e0 blend mode, +0x1e4 depth mode, +0x1e8 cull mode, +0x1ec texture factor
/// (ARGB), +0x1f0 alpha reference, +0x1f4 lighting switch (byte), +0x1f8 depth compare, +0x1fc alpha-test
/// compare (0 = off), +0x200 fill mode, +0x204 bump switch (byte), +0x208 bump stage, +0x20c / +0x210 bump
/// luminance scale / offset.
// DISASM: engine+0x1a8bea..0x1a8d72 (raster state reads), engine+0x1a82c3 (ALPHAREF from +0x1f0),
// engine+0x1a90ca..0x1a912e (bump block)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StylePass {
    pub stages: [StyleStage; 4],
    pub blend: u32,
    pub depth_mode: u32,
    pub cull: u32,
    pub texture_factor: u32,
    pub alpha_reference: u32,
    /// Whole word at +0x1f4; the engine tests its low byte.
    pub lighting_word: u32,
    pub depth_compare: u32,
    pub alpha_test: u32,
    pub fill: u32,
    /// Whole word at +0x204; the engine tests its low byte.
    pub bump_word: u32,
    pub bump_stage: u32,
    pub bump_scale: f32,
    pub bump_offset: f32,
    pub extra: Option<PassExtra>,
}

impl Default for StylePass {
    fn default() -> Self {
        StylePass {
            stages: [StyleStage::default(); 4],
            blend: 0,
            depth_mode: 0,
            cull: 0,
            texture_factor: 0,
            alpha_reference: 0,
            lighting_word: 0,
            depth_compare: 0,
            alpha_test: 0,
            fill: 0,
            bump_word: 0,
            bump_stage: 0,
            bump_scale: 0.0,
            bump_offset: 0.0,
            extra: None,
        }
    }
}

impl StylePass {
    /// The lighting switch (low byte of the word at +0x1f4).
    pub fn lighting(&self) -> bool {
        self.lighting_word & 0xff != 0
    }
    /// The bump switch (low byte of the word at +0x204).
    pub fn bump(&self) -> bool {
        self.bump_word & 0xff != 0
    }

    fn from_bytes(b: &[u8]) -> StylePass {
        let mut stages = [StyleStage::default(); 4];
        for (k, s) in stages.iter_mut().enumerate() {
            *s = StyleStage::from_bytes(&b[k * STAGE_RECORD_LEN..(k + 1) * STAGE_RECORD_LEN]);
        }
        StylePass {
            stages,
            blend: word(b, 0x1e0),
            depth_mode: word(b, 0x1e4),
            cull: word(b, 0x1e8),
            texture_factor: word(b, 0x1ec),
            alpha_reference: word(b, 0x1f0),
            lighting_word: word(b, 0x1f4),
            depth_compare: word(b, 0x1f8),
            alpha_test: word(b, 0x1fc),
            fill: word(b, 0x200),
            bump_word: word(b, 0x204),
            bump_stage: word(b, 0x208),
            bump_scale: real(b, 0x20c),
            bump_offset: real(b, 0x210),
            extra: None,
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        for s in &self.stages {
            s.write(out);
        }
        for w in [
            self.blend,
            self.depth_mode,
            self.cull,
            self.texture_factor,
            self.alpha_reference,
            self.lighting_word,
            self.depth_compare,
            self.alpha_test,
            self.fill,
            self.bump_word,
            self.bump_stage,
        ] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.extend_from_slice(&self.bump_scale.to_le_bytes());
        out.extend_from_slice(&self.bump_offset.to_le_bytes());
    }
}

/// One variant: the loader tries the variants in order and keeps the first that its device accepts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StyleVariant {
    /// The variant's length word (read and not used by the engine; OBSERVED: the bytes from the material to
    /// the end of the last pass block, the trailing 8-byte block not included).
    pub len_word: u32,
    pub material: StyleMaterial,
    pub passes: Vec<StylePass>,
    /// The trailing 8-byte block; `None` when the file ends before it (the engine then logs a failure line
    /// and keeps the style). Byte 0 non-zero sends the pieces through the effect path instead of the passes.
    // DISASM: engine+0x161d01..0x161d2c (8-byte read; status 1 -> log line, else slot 8 stores it), engine+0x1624c0
    // (slot 9 returns it); EVIDENCE: engine+0x1b94e0 (byte 0 == 0 -> the passes, else the effect path)
    pub options: Option<[u8; 8]>,
}

/// A parsed render-style file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StyleFile {
    /// Header byte 1 between the kind byte and the revision word (not checked by the engine; OBSERVED
    /// non-zero in the ROM files).
    pub header_pad: u8,
    /// The 16 header bytes after the kind / revision words (not read by the engine).
    pub header_rest: [u8; 16],
    /// The data length word after the header (only checked against 1,000,000).
    pub data_len: u32,
    pub variants: Vec<StyleVariant>,
    /// Bytes after the last variant that the loader never reads.
    pub trailing: usize,
}

impl StyleFile {
    /// Whether `data` starts like a render-style file (kind byte 5, revision word 3).
    pub fn is_style(data: &[u8]) -> bool {
        data.len() >= STYLE_HEADER_LEN
            && data[0] == STYLE_FILE_KIND
            && u16::from_le_bytes([data[2], data[3]]) == STYLE_CONTAINER_REV
    }

    /// Reads a render-style file the way the 2009 loader does.
    ///
    /// Layout: header (0x14 bytes: kind byte 5, an unread byte, revision word 3, 16 unread bytes), `u32
    /// data_len` (<= 1,000,000), `u32 variant_count` (1..32), then per variant: `u32 len_word`, the 0x44-byte
    /// material, `u32 pass_count` (<= 4), per pass the 0x214-byte record, `u8 has_extra` and when set the
    /// 10-byte [`PassExtra`], and finally the 8-byte options block.
    // DISASM: engine+0x161bb0..0x161d75 (the whole loader)
    pub fn parse(data: &[u8]) -> Result<StyleFile, StyleError> {
        let mut r = Rd { buf: data, pos: 0 };
        let hdr = r.take(STYLE_HEADER_LEN)?;
        let rev = u16::from_le_bytes([hdr[2], hdr[3]]);
        if hdr[0] != STYLE_FILE_KIND || rev != STYLE_CONTAINER_REV {
            return Err(StyleError::NotAStyle {
                file_kind: hdr[0],
                rev,
            });
        }
        let header_pad = hdr[1];
        let mut header_rest = [0u8; 16];
        header_rest.copy_from_slice(&hdr[4..20]);
        let data_len = r.u32()?;
        if data_len > STYLE_MAX_DATA_LEN {
            return Err(StyleError::DataLen { value: data_len });
        }
        let count = r.u32()?;
        if count == 0 || count > STYLE_MAX_VARIANTS {
            return Err(StyleError::VariantCount { value: count });
        }
        let mut variants = Vec::with_capacity(count as usize);
        for vi in 0..count as usize {
            let len_word = r.u32()?;
            let material = StyleMaterial::from_bytes(r.take(MATERIAL_LEN)?);
            let n = r.u32()?;
            if n > STYLE_MAX_PASSES {
                return Err(StyleError::TooManyPasses {
                    variant: vi,
                    count: n,
                });
            }
            let mut passes = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let mut p = StylePass::from_bytes(r.take(PASS_RECORD_LEN)?);
                if r.u8()? != 0 {
                    p.extra = Some(PassExtra {
                        todo_a: r.u8()?,
                        todo_b: r.u32()?,
                        todo_c: r.u8()?,
                        todo_d: r.u32()?,
                    });
                }
                passes.push(p);
            }
            let options = if r.remaining() >= 8 {
                let mut o = [0u8; 8];
                o.copy_from_slice(r.take(8)?);
                Some(o)
            } else {
                None
            };
            variants.push(StyleVariant {
                len_word,
                material,
                passes,
                options,
            });
            if options.is_none() {
                // the stream is exhausted: a later variant could not be read either
                break;
            }
        }
        Ok(StyleFile {
            header_pad,
            header_rest,
            data_len,
            variants,
            trailing: r.remaining(),
        })
    }

    /// The variant the renderer uses: the first one (the loader keeps the first variant its device accepts;
    /// every ROM file has one, OBSERVED).
    pub fn first(&self) -> Option<&StyleVariant> {
        self.variants.first()
    }

    /// Encodes the file (synthetic fixtures; a parsed file encodes back to its own bytes when it has no
    /// trailing bytes).
    pub fn to_bytes(&self) -> Result<Vec<u8>, StyleError> {
        if self.variants.is_empty() || self.variants.len() > STYLE_MAX_VARIANTS as usize {
            return Err(StyleError::Encode {
                what: "variant count",
            });
        }
        let mut out = Vec::with_capacity(STYLE_HEADER_LEN + 8 + 650 * self.variants.len());
        out.push(STYLE_FILE_KIND);
        out.push(self.header_pad);
        out.extend_from_slice(&STYLE_CONTAINER_REV.to_le_bytes());
        out.extend_from_slice(&self.header_rest);
        out.extend_from_slice(&self.data_len.to_le_bytes());
        out.extend_from_slice(&(self.variants.len() as u32).to_le_bytes());
        for (i, v) in self.variants.iter().enumerate() {
            if v.passes.len() > STYLE_MAX_PASSES as usize {
                return Err(StyleError::Encode { what: "pass count" });
            }
            if v.options.is_none() && i + 1 != self.variants.len() {
                return Err(StyleError::Encode {
                    what: "options block missing before another variant",
                });
            }
            out.extend_from_slice(&v.len_word.to_le_bytes());
            v.material.write(&mut out);
            out.extend_from_slice(&(v.passes.len() as u32).to_le_bytes());
            for p in &v.passes {
                p.write(&mut out);
                match p.extra {
                    None => out.push(0),
                    Some(e) => {
                        out.push(1);
                        out.push(e.todo_a);
                        out.extend_from_slice(&e.todo_b.to_le_bytes());
                        out.push(e.todo_c);
                        out.extend_from_slice(&e.todo_d.to_le_bytes());
                    }
                }
            }
            if let Some(o) = v.options {
                out.extend_from_slice(&o);
            }
        }
        Ok(out)
    }

    /// The variant length word as the ROM files carry it: material through the last pass block (see
    /// [`StyleVariant::len_word`]).
    pub fn variant_len(v: &StyleVariant) -> u32 {
        let mut n = MATERIAL_LEN + 4;
        for p in &v.passes {
            n += PASS_RECORD_LEN + 1 + if p.extra.is_some() { 10 } else { 0 };
        }
        n as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StyleFile {
        let mut p = StylePass::default();
        p.stages[0].texture = 1;
        p.stages[0].colour_op = 4;
        p.stages[0].colour_arg1 = 2;
        p.stages[0].colour_arg2 = 1;
        p.stages[0].alpha_op = 3;
        p.stages[0].alpha_arg1 = 2;
        p.stages[0].alpha_arg2 = 1;
        p.stages[0].filter = 2;
        p.blend = 3;
        p.depth_mode = 1;
        p.cull = 1;
        p.texture_factor = 0x8040_2010;
        p.alpha_reference = 96;
        p.lighting_word = 1;
        p.depth_compare = 2;
        p.alpha_test = 4;
        p.fill = 1;
        let mut q = p;
        q.extra = Some(PassExtra {
            todo_a: 1,
            todo_b: 7,
            todo_c: 0,
            todo_d: 9,
        });
        let mut v = StyleVariant {
            len_word: 0,
            material: StyleMaterial {
                ambient: [1.0, 1.0, 1.0, 1.0],
                diffuse: [0.5, 0.6, 0.7, 1.0],
                emissive: [0.0, 0.0, 0.0, 1.0],
                specular: [0.0; 4],
                power: 0.0,
            },
            passes: vec![p, q],
            options: Some([1, 2, 3, 4, 5, 6, 7, 8]),
        };
        v.len_word = StyleFile::variant_len(&v);
        StyleFile {
            header_pad: 0x5a,
            header_rest: [0; 16],
            data_len: v.len_word,
            variants: vec![v],
            trailing: 0,
        }
    }

    #[test]
    fn round_trip_and_sizes() {
        let f = sample();
        let b = f.to_bytes().unwrap();
        // header 0x14 + data_len + count + len + material + pass count + 2 passes + extra + options
        assert_eq!(
            b.len(),
            0x14 + 4 + 4 + 4 + 0x44 + 4 + 2 * (0x214 + 1) + 10 + 8
        );
        assert!(StyleFile::is_style(&b));
        let g = StyleFile::parse(&b).unwrap();
        assert_eq!(g, f);
        assert_eq!(g.to_bytes().unwrap(), b);
        let p = &g.variants[0].passes[0];
        assert!(p.lighting());
        assert!(!p.bump());
        assert_eq!(p.stages[0].colour_op, 4);
        assert_eq!(p.stages[1].colour_op, 0);
        assert_eq!(g.variants[0].len_word as usize, 0x44 + 4 + 2 * 0x215 + 10);
    }

    #[test]
    fn options_block_may_be_missing() {
        let mut f = sample();
        f.variants[0].options = None;
        let b = f.to_bytes().unwrap();
        let g = StyleFile::parse(&b).unwrap();
        assert_eq!(g.variants[0].options, None);
        assert_eq!(g.trailing, 0);
        // a partial options block is left as trailing bytes
        let mut c = b.clone();
        c.extend_from_slice(&[0, 0, 0]);
        let g = StyleFile::parse(&c).unwrap();
        assert_eq!(g.variants[0].options, None);
        assert_eq!(g.trailing, 3);
    }

    #[test]
    fn refusals() {
        let b = sample().to_bytes().unwrap();
        let mut k = b.clone();
        k[0] = 1;
        assert!(matches!(
            StyleFile::parse(&k),
            Err(StyleError::NotAStyle { file_kind: 1, .. })
        ));
        let mut k = b.clone();
        k[2] = 9;
        assert!(matches!(
            StyleFile::parse(&k),
            Err(StyleError::NotAStyle { rev: 9, .. })
        ));
        let mut k = b.clone();
        k[0x14..0x18].copy_from_slice(&1_000_001u32.to_le_bytes());
        assert_eq!(
            StyleFile::parse(&k),
            Err(StyleError::DataLen { value: 1_000_001 })
        );
        for bad in [0u32, 33] {
            let mut k = b.clone();
            k[0x18..0x1c].copy_from_slice(&bad.to_le_bytes());
            assert_eq!(
                StyleFile::parse(&k),
                Err(StyleError::VariantCount { value: bad })
            );
        }
        let mut k = b.clone();
        let at = 0x1c + 4 + 0x44;
        k[at..at + 4].copy_from_slice(&5u32.to_le_bytes());
        assert_eq!(
            StyleFile::parse(&k),
            Err(StyleError::TooManyPasses {
                variant: 0,
                count: 5
            })
        );
        // every truncation is an error or a parse with a missing options block, never a panic
        for cut in 0..b.len() {
            let r = StyleFile::parse(&b[..cut]);
            if let Ok(f) = r {
                assert!(f.variants[0].options.is_none());
            }
        }
        assert!(StyleFile::parse(&[]).is_err());
    }
}
