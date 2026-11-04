//! Installed fonts of the host by face name: the menu's text of font records that name a face but no font file.
//!
//! The 2009 engine creates such fonts by face name (GDI `CreateFontIndirect` with the record's face, height and
//! weight), so their glyphs come from the font the system has installed under that name, not from the ROM's font
//! archive. DeCombat looks the face up the same way: it reads only the table directory, the `name` table and the
//! `OS/2` weight of each TrueType / OpenType file of the host's font folder, then loads the one file whose family
//! name is the face and whose weight matches the request.
//!
//! PUBLIC: OpenType specification (offset table, TTC header `ttcf`, table records, `name` table ids 1 family, 2
//! subfamily, 16 typographic family; `OS/2` usWeightClass at +4, fsSelection at +62, bit 0 italic, bit 5 bold);
//! GDI LOGFONT (`lfFaceName` is a family name, weight 700 = bold).
// SPEC: FMT_CRES_UI.md §7.1 (records without a file use a system face; engine+0xe77b0 builds the logical font from
// face, height, weight)
// DC-DEFAULT: CLI-462 (proposed) a menu font record without a font file in the ROM is drawn with the host's
// installed font of that face name and weight; without one, the menu's default font as before

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// One face of an installed font file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledFace {
    pub path: PathBuf,
    /// Face index inside a collection (0 for a single font).
    pub index: usize,
    pub family: String,
    pub bold: bool,
    pub italic: bool,
}

/// The host's font folders GDI looks in: `%WINDIR%\Fonts` and the per-user `%LOCALAPPDATA%\Microsoft\Windows\Fonts`;
/// on macOS the system, supplemental, local and per-user font folders; on iPadOS the system's (none elsewhere).
pub fn font_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    // DC-DEFAULT: CLI-462 (proposed) macOS: the same face lookup over the standard font folders
    #[cfg(target_os = "macos")]
    {
        for d in [
            "/System/Library/Fonts",
            "/System/Library/Fonts/Supplemental",
            "/Library/Fonts",
        ] {
            out.push(PathBuf::from(d));
        }
        if let Some(home) = std::env::var_os("HOME") {
            out.push(PathBuf::from(home).join("Library").join("Fonts"));
        }
    }
    // DC-DEFAULT: CLI-1604 (proposed) iPadOS: the system's font folders (readable by apps; the ROM's own fonts come
    // first anyway)
    #[cfg(target_os = "ios")]
    {
        for d in [
            "/System/Library/Fonts",
            "/System/Library/Fonts/Core",
            "/System/Library/Fonts/CoreUI",
            "/System/Library/Fonts/CoreAddition",
        ] {
            out.push(PathBuf::from(d));
        }
    }
    if let Some(win) = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")) {
        out.push(PathBuf::from(win).join("Fonts"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        out.push(
            PathBuf::from(local)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    out.retain(|p| p.is_dir());
    out
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        *b.get(at + 3)?,
    ]))
}

fn read_at(f: &mut File, at: u64, len: usize) -> Option<Vec<u8>> {
    f.seek(SeekFrom::Start(at)).ok()?;
    let mut b = vec![0u8; len];
    f.read_exact(&mut b).ok()?;
    Some(b)
}

/// Family names (ids 1 and 16) of a `name` table.
fn families(name: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let (Some(count), Some(base)) = (be16(name, 2), be16(name, 4)) else {
        return out;
    };
    for i in 0..count as usize {
        let r = 6 + i * 12;
        let (Some(platform), Some(id), Some(len), Some(off)) = (
            be16(name, r),
            be16(name, r + 6),
            be16(name, r + 8),
            be16(name, r + 10),
        ) else {
            break;
        };
        if id != 1 && id != 16 {
            continue;
        }
        let start = base as usize + off as usize;
        let Some(raw) = name.get(start..start + len as usize) else {
            continue;
        };
        let s = if platform == 3 || platform == 0 {
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            raw.iter().map(|&c| c as char).collect()
        };
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// The faces of one font file (only its directory, `name` and `OS/2` tables are read).
pub fn faces_of(path: &Path) -> Vec<InstalledFace> {
    let mut out = Vec::new();
    let Ok(mut f) = File::open(path) else {
        return out;
    };
    let Some(head) = read_at(&mut f, 0, 12) else {
        return out;
    };
    let offsets: Vec<u32> = if &head[0..4] == b"ttcf" {
        let n = be32(&head, 8).unwrap_or(0).min(64) as usize;
        match read_at(&mut f, 12, n * 4) {
            Some(b) => (0..n).filter_map(|i| be32(&b, i * 4)).collect(),
            None => return out,
        }
    } else {
        vec![0]
    };
    for (index, off) in offsets.into_iter().enumerate() {
        let Some(dir) = read_at(&mut f, off as u64, 12) else {
            continue;
        };
        let n = be16(&dir, 4).unwrap_or(0) as usize;
        let Some(recs) = read_at(&mut f, off as u64 + 12, n * 16) else {
            continue;
        };
        let table = |tag: &[u8; 4]| {
            (0..n).find_map(|i| {
                let r = i * 16;
                (recs.get(r..r + 4)? == tag).then(|| (be32(&recs, r + 8), be32(&recs, r + 12)))
            })
        };
        let Some((Some(name_at), Some(name_len))) = table(b"name") else {
            continue;
        };
        let Some(name) = read_at(&mut f, name_at as u64, name_len.min(1 << 20) as usize) else {
            continue;
        };
        let (mut bold, mut italic) = (false, false);
        if let Some((Some(os2_at), Some(os2_len))) = table(b"OS/2") {
            if let Some(os2) = read_at(&mut f, os2_at as u64, os2_len.min(64) as usize) {
                let weight = be16(&os2, 4).unwrap_or(400);
                let sel = be16(&os2, 62).unwrap_or(0);
                bold = weight >= 600 || sel & 0x20 != 0;
                italic = sel & 1 != 0;
            }
        }
        for family in families(&name) {
            out.push(InstalledFace {
                path: path.to_path_buf(),
                index,
                family,
                bold,
                italic,
            });
        }
    }
    out
}

/// Every face of the TrueType / OpenType files in `dir`.
pub fn scan(dir: &Path) -> Vec<InstalledFace> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| ["ttf", "ttc", "otf"].contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect();
    files.sort();
    files.iter().flat_map(|p| faces_of(p)).collect()
}

/// The face GDI would pick for `face` at this weight: same family (case-insensitive), upright, the requested
/// weight first, the other weight when only that one is installed.
pub fn pick<'a>(faces: &'a [InstalledFace], face: &str, bold: bool) -> Option<&'a InstalledFace> {
    let same = |f: &&InstalledFace| f.family.eq_ignore_ascii_case(face) && !f.italic;
    faces
        .iter()
        .filter(same)
        .find(|f| f.bold == bold)
        .or_else(|| faces.iter().find(same))
}

/// The bytes and face index of the installed font for `face` / `bold`.
pub fn load(faces: &[InstalledFace], face: &str, bold: bool) -> Option<(Vec<u8>, usize)> {
    let f = pick(faces, face, bold)?;
    Some((std::fs::read(&f.path).ok()?, f.index))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(family: &str, bold: bool, italic: bool) -> InstalledFace {
        InstalledFace {
            path: PathBuf::from(format!("{family}{bold}{italic}")),
            index: 0,
            family: family.into(),
            bold,
            italic,
        }
    }

    #[test]
    fn picks_the_family_and_weight_like_gdi() {
        let all = [
            face("Sans A", false, false),
            face("Sans A", true, true),
            face("Sans A", true, false),
            face("Sans B", false, false),
        ];
        assert_eq!(pick(&all, "sans a", true), Some(&all[2]));
        assert_eq!(pick(&all, "Sans A", false), Some(&all[0]));
        // only the regular weight installed: that one
        assert_eq!(pick(&all, "Sans B", true), Some(&all[3]));
        assert_eq!(pick(&all, "Sans C", false), None);
    }

    #[test]
    fn name_table_families_are_read() {
        // one record: platform 3, encoding 1, language 0x409, name id 1, UTF-16BE "Ab"
        let mut t = vec![0, 0, 0, 1, 0, 18];
        t.extend_from_slice(&[0, 3, 0, 1, 4, 9, 0, 1, 0, 4, 0, 0]);
        t.extend_from_slice(&[0, b'A', 0, b'b']);
        assert_eq!(families(&t), vec!["Ab".to_string()]);
    }
}
