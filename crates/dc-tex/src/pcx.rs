//! PCX image decoder (8 bits per plane; 1 plane with a 256-colour palette, or 3 planes of red, green and blue),
//! run-length encoded, as the 2009 front end's mouse cursor image needs.
//!
//! Written from `rust/specs/FMT_PCX.md` only (public format knowledge: the ZSoft PCX technical reference). All size
//! arithmetic is checked and no function panics on hostile input; malformed buffers yield [`PcxError`].
// SPEC: FMT_PCX.md §2 (header), §3 (scan lines and runs), §4 (palette)
// EVIDENCE: engine+0x50570 (the 2009 engine's decoder: 8 bits per plane only, 1 or 3 planes, width xmax - xmin + 1,
// a run longer than what is left of the scan line rejects the file)

use std::fmt;

/// Header length. SPEC: FMT_PCX.md §2.
pub const PCX_HEADER_LEN: usize = 128;
/// The byte before a trailing 256-colour palette. SPEC: FMT_PCX.md §4.
pub const PALETTE_MARKER: u8 = 0x0c;
/// Largest side this decoder accepts (bounds the allocation on hostile input). SPEC: FMT_PCX.md §5.
pub const PCX_MAX_SIDE: u32 = 4096;

/// Errors from decoding a PCX image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcxError {
    /// Fewer than [`PCX_HEADER_LEN`] bytes.
    ShortHeader { len: usize },
    /// The first byte is not the manufacturer tag 0x0A.
    BadTag(u8),
    /// Encoding byte is not 1 (run-length).
    BadEncoding(u8),
    /// Bits per plane other than 8, or a plane count other than 1 or 3.
    Unsupported { bits: u8, planes: u8 },
    /// The window gives an empty or oversized image, or the bytes per line are fewer than the width.
    BadSize {
        width: i32,
        height: i32,
        bytes_per_line: u16,
    },
    /// A run is longer than what is left of its scan line.
    RunOverflow { line: u32 },
    /// The data ends before the last scan line.
    Truncated { line: u32 },
    /// A 1-plane image without the trailing palette.
    NoPalette,
}

impl fmt::Display for PcxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PcxError::ShortHeader { len } => {
                write!(f, "pcx: header needs {PCX_HEADER_LEN} bytes, got {len}")
            }
            PcxError::BadTag(t) => write!(f, "pcx: bad manufacturer tag {t:#x}"),
            PcxError::BadEncoding(e) => write!(f, "pcx: unsupported encoding {e}"),
            PcxError::Unsupported { bits, planes } => {
                write!(f, "pcx: unsupported {bits} bits x {planes} planes")
            }
            PcxError::BadSize {
                width,
                height,
                bytes_per_line,
            } => write!(
                f,
                "pcx: bad size {width} x {height} ({bytes_per_line} bytes per line)"
            ),
            PcxError::RunOverflow { line } => {
                write!(f, "pcx: run crosses the end of line {line}")
            }
            PcxError::Truncated { line } => write!(f, "pcx: data ends in line {line}"),
            PcxError::NoPalette => write!(f, "pcx: palette image without its palette"),
        }
    }
}

impl std::error::Error for PcxError {}

/// A decoded image: `width` x `height` RGBA8 pixels, rows top to bottom, alpha 255 everywhere (PCX has no alpha).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcxImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

/// Decodes a PCX file (see the module notes for what is accepted).
pub fn decode_pcx(data: &[u8]) -> Result<PcxImage, PcxError> {
    if data.len() < PCX_HEADER_LEN {
        return Err(PcxError::ShortHeader { len: data.len() });
    }
    if data[0] != 0x0a {
        return Err(PcxError::BadTag(data[0]));
    }
    if data[2] != 1 {
        return Err(PcxError::BadEncoding(data[2]));
    }
    let bits = data[3];
    let planes = data[65];
    if bits != 8 || !(planes == 1 || planes == 3) {
        return Err(PcxError::Unsupported { bits, planes });
    }
    let (xmin, ymin) = (le16(data, 4) as i16 as i32, le16(data, 6) as i16 as i32);
    let (xmax, ymax) = (le16(data, 8) as i16 as i32, le16(data, 10) as i16 as i32);
    let bytes_per_line = le16(data, 66);
    let width = xmax - xmin + 1;
    let height = ymax - ymin + 1;
    if width <= 0
        || height <= 0
        || width as u32 > PCX_MAX_SIDE
        || height as u32 > PCX_MAX_SIDE
        || (bytes_per_line as i32) < width
    {
        return Err(PcxError::BadSize {
            width,
            height,
            bytes_per_line,
        });
    }
    let (w, h) = (width as usize, height as usize);
    let bpl = bytes_per_line as usize;
    let line_len = bpl * planes as usize;

    // the 256-colour palette of a 1-plane image: the last 769 bytes, a marker then 256 x (r, g, b)
    let palette = if planes == 1 {
        let n = data.len();
        if n < PCX_HEADER_LEN + 769 || data[n - 769] != PALETTE_MARKER {
            return Err(PcxError::NoPalette);
        }
        Some(&data[n - 768..])
    } else {
        None
    };

    let mut rgba = vec![0u8; w * h * 4];
    let mut line = vec![0u8; line_len];
    let mut at = PCX_HEADER_LEN;
    for y in 0..h {
        let mut filled = 0usize;
        while filled < line_len {
            let Some(&b) = data.get(at) else {
                return Err(PcxError::Truncated { line: y as u32 });
            };
            let (count, value) = if b & 0xc0 == 0xc0 {
                let Some(&v) = data.get(at + 1) else {
                    return Err(PcxError::Truncated { line: y as u32 });
                };
                at += 2;
                ((b & 0x3f) as usize, v)
            } else {
                at += 1;
                (1, b)
            };
            if count > line_len - filled {
                return Err(PcxError::RunOverflow { line: y as u32 });
            }
            line[filled..filled + count].fill(value);
            filled += count;
        }
        let row = &mut rgba[y * w * 4..(y + 1) * w * 4];
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            match palette {
                Some(pal) => {
                    let i = line[x] as usize * 3;
                    px[..3].copy_from_slice(&pal[i..i + 3]);
                }
                None => {
                    px[0] = line[x];
                    px[1] = line[bpl + x];
                    px[2] = line[2 * bpl + x];
                }
            }
            px[3] = 255;
        }
    }
    Ok(PcxImage {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header for a `w` x `h` image with `planes` planes and `bpl` bytes per line.
    fn header(w: u16, h: u16, planes: u8, bpl: u16) -> Vec<u8> {
        let mut b = vec![0u8; PCX_HEADER_LEN];
        b[0] = 0x0a;
        b[1] = 5;
        b[2] = 1;
        b[3] = 8;
        b[8..10].copy_from_slice(&(w - 1).to_le_bytes());
        b[10..12].copy_from_slice(&(h - 1).to_le_bytes());
        b[65] = planes;
        b[66..68].copy_from_slice(&bpl.to_le_bytes());
        b
    }

    #[test]
    fn rgb_three_planes_with_runs_and_padding() {
        // 3 x 2, 4 bytes per line (one pad byte per plane line)
        let mut f = header(3, 2, 3, 4);
        // line 0: R = run of 4 x 10, G = 20 21 22 pad, B = run of 4 x 0xC5 (a value >= 0xC0 needs a run)
        f.extend_from_slice(&[0xc4, 10, 20, 21, 22, 0, 0xc4, 0xc5]);
        // line 1: R = 1 2 3 pad, G = run of 4 x 7, B = run of 4 x 0
        f.extend_from_slice(&[1, 2, 3, 0, 0xc4, 7, 0xc4, 0]);
        let img = decode_pcx(&f).unwrap();
        assert_eq!((img.width, img.height), (3, 2));
        assert_eq!(
            img.rgba,
            vec![
                10, 20, 0xc5, 255, 10, 21, 0xc5, 255, 10, 22, 0xc5, 255, //
                1, 7, 0, 255, 2, 7, 0, 255, 3, 7, 0, 255,
            ]
        );
    }

    #[test]
    fn palette_one_plane() {
        let mut f = header(2, 1, 1, 2);
        f.extend_from_slice(&[0xc2, 1]);
        f.push(PALETTE_MARKER);
        let mut pal = vec![0u8; 768];
        pal[3..6].copy_from_slice(&[9, 8, 7]);
        f.extend_from_slice(&pal);
        let img = decode_pcx(&f).unwrap();
        assert_eq!(img.rgba, vec![9, 8, 7, 255, 9, 8, 7, 255]);
    }

    #[test]
    fn hostile_input_is_refused() {
        assert_eq!(decode_pcx(&[0; 10]), Err(PcxError::ShortHeader { len: 10 }));
        let mut f = header(2, 1, 3, 2);
        f.extend_from_slice(&[0xc7, 1]); // a run of 7 over a 6-byte line
        assert_eq!(decode_pcx(&f), Err(PcxError::RunOverflow { line: 0 }));
        let f = header(2, 1, 3, 2);
        assert_eq!(decode_pcx(&f), Err(PcxError::Truncated { line: 0 }));
        let mut f = header(2, 1, 3, 2);
        f[3] = 4;
        assert!(matches!(decode_pcx(&f), Err(PcxError::Unsupported { .. })));
        let f = header(2, 1, 1, 2);
        assert_eq!(decode_pcx(&f), Err(PcxError::NoPalette));
        let mut f = header(2, 1, 3, 1);
        f.extend_from_slice(&[0; 6]);
        assert!(matches!(decode_pcx(&f), Err(PcxError::BadSize { .. })));
    }
}
