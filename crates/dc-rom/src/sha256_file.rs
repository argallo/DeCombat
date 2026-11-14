//! SHA-256 of a file on disk, with the ROM read trace and `RomError` (the digest itself is `dc_core::Sha256`).
// Provenance: ORIGINAL (our own tooling; no 2009 behaviour)

use crate::error::RomError;
use std::fs;
use std::path::Path;

/// Computes the lowercase hex SHA-256 digest of a file on disk (streamed, 1 MiB at a time).
pub fn sha256_file(path: &Path) -> Result<String, RomError> {
    use std::io::Read;
    let mut f = fs::File::open(path).map_err(|e| RomError::io(path, &e))?;
    dc_core::trace::note("file", path, "", f.metadata().map(|m| m.len()).unwrap_or(0));
    let mut hasher = dc_core::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| RomError::io(path, &e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(64);
    for &b in &digest {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_file_hashes_like_its_bytes() {
        let dir = std::env::temp_dir().join(format!("dc_rom_sha256_file_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.bin");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i * 31 + 7) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        assert_eq!(super::sha256_file(&p).unwrap(), dc_core::sha256_hex(&data));
        std::fs::remove_dir_all(&dir).ok();
    }
}
