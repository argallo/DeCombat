//! A writer for our own textures: one level of 32-bit texels in the container of FMT_DTX.md, so a mod can ship an
//! original picture the client's reader opens (CLI-1612). Provenance: ORIGINAL, from the spec's header layout.
// SPEC: FMT_DTX.md §2 (header), §4.1 (level sizes), §3 (format 3: B, G, R, A bytes per texel)

use crate::header::DtxHeader;

/// A single-level 32-bit DTX of `width` x `height` RGBA texels (`rgba` row-major, 4 bytes per texel). `None` when the
/// sizes do not match or are 0.
pub fn write_bgra8(width: u16, height: u16, rgba: &[u8]) -> Option<Vec<u8>> {
    let n = (width as usize).checked_mul(height as usize)?;
    if n == 0 || rgba.len() != n.checked_mul(4)? {
        return None;
    }
    let header = DtxHeader {
        width,
        height,
        level_count: 1,
        ..DtxHeader::default()
    };
    let mut out = header.to_bytes().to_vec();
    out.reserve(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DtxFile;

    #[test]
    fn a_written_texture_reads_back() {
        let rgba: Vec<u8> = (0..4 * 4 * 2).map(|i| i as u8).collect();
        let bytes = write_bgra8(4, 2, &rgba).unwrap();
        let f = DtxFile::parse(&bytes).unwrap();
        assert_eq!((f.header().width, f.header().height), (4, 2));
        assert_eq!(f.trailing(), 0);
        assert!(write_bgra8(4, 2, &rgba[..8]).is_none());
        assert!(write_bgra8(0, 2, &[]).is_none());
    }
}
