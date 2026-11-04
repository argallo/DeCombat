//! `ui_fonts`: system font records of the layout table as [`FontSpec`]s, the font table lookup by index, and
//! a resolver that finds a record's font file in the user's font archive.
//!
//! PUBLIC: GDI `LOGFONT` / `CreateFontIndirect` semantics (weight 400 normal / 700 bold; positive height =
//! cell height, negative = character height), TrueType / TTC containers (OpenType specification: TTC header
//! `ttcf` + face count, `name` table records, name ids 1 = family and 4 = full name).
// SPEC: FMT_CRES_UI.md §7.1, §7.3

use crate::error::UiDataError;
use crate::layout::{FontRecord, LayoutTable};
use dc_rez::RezArchive;
use std::path::{Path, PathBuf};

/// The font archive of the client folder. SPEC: FMT_CRES_UI.md §7.3
pub const FONT_ARCHIVE: &str = "FONTS.rez";

/// A font request built from one system font record. SPEC: FMT_CRES_UI.md §7.1 (cshell+0x1c6380)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontSpec {
    /// Record index (= font index used by widgets and per-screen keys).
    pub index: u32,
    /// The record's own name (logging only in the client).
    pub name: String,
    /// Face name (GDI face).
    pub face: String,
    /// Font file inside the archives, when the record names one.
    pub file: Option<String>,
    /// Requested height in pixels (GDI `lfHeight` semantics).
    pub height: i32,
    /// Requested width (0 = default aspect).
    pub width: i32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl FontSpec {
    /// Builds the request; `None` for a record the client skips: empty name (no entry) or neither file nor
    /// face (invalid request, no font created). EVIDENCE: cshell+0x1c1e70
    pub fn from_record(r: &FontRecord) -> Option<Self> {
        if r.name.is_empty() || (r.file.is_empty() && r.face.is_empty()) {
            return None;
        }
        Some(FontSpec {
            index: r.index,
            name: r.name.clone(),
            face: r.face.clone(),
            file: (!r.file.is_empty()).then(|| r.file.clone()),
            height: r.height,
            width: r.width,
            bold: r.bold,
            italic: r.italic,
            underline: r.underline,
        })
    }

    /// GDI weight: 700 when bold and the engine-side bold option is on, else 400. The option is UNPROVEN
    /// (SPEC §10.4); callers pass `true` to honour bold. EVIDENCE: engine+0xe77b0
    pub fn weight(&self, bold_option: bool) -> i32 {
        if self.bold && bold_option {
            700
        } else {
            400
        }
    }

    /// The point size to request from a rasterizer: `|height|` pixels.
    pub fn size_px(&self) -> u32 {
        self.height.unsigned_abs()
    }
}

/// The system font table: one slot per record (the recommended model of SPEC §7.1: a failed record keeps
/// its slot and lookups fall back to slot 0), plus the client's compacted order for bug-compatibility.
#[derive(Clone, Debug, Default)]
pub struct SystemFonts {
    slots: Vec<Option<FontSpec>>,
}

impl SystemFonts {
    /// Builds the table from all records of the layout table.
    pub fn from_layout(t: &LayoutTable) -> Self {
        let n = t.font_record_count();
        let slots = (0..n)
            .map(|i| t.font_record(i).as_ref().and_then(FontSpec::from_record))
            .collect();
        SystemFonts { slots }
    }

    /// Number of record slots.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The spec in slot `index`; with `allow_default`, a missing or failed slot gives slot 0.
    /// EVIDENCE: cshell+0x1c07b0
    pub fn get(&self, index: usize, allow_default: bool) -> Option<&FontSpec> {
        match self.slots.get(index).and_then(|s| s.as_ref()) {
            Some(s) => Some(s),
            None if allow_default => self.slots.first().and_then(|s| s.as_ref()),
            None => None,
        }
    }

    /// The client's own table: failed records add no entry, so later indices shift. EVIDENCE: cshell+0x1c1c80
    pub fn compacted(&self) -> Vec<&FontSpec> {
        self.slots.iter().flatten().collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = Option<&FontSpec>> {
        self.slots.iter().map(|s| s.as_ref())
    }
}

/// Container kind of a font file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontContainer {
    /// A single sfnt (TrueType or OpenType outlines).
    Single,
    /// A TrueType collection.
    Collection,
}

/// Parsed container header of a font file (PUBLIC: OpenType specification).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFileInfo {
    pub container: FontContainer,
    /// Offsets of the table directories of the faces.
    pub face_offsets: Vec<u32>,
}

fn be16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o.checked_add(2)?)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
}
fn be32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o.checked_add(4)?)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Reads the container header: `ttcf` collection or a single sfnt (version 0x00010000, `true`, `OTTO`).
pub fn font_file_info(d: &[u8]) -> Option<FontFileInfo> {
    let tag = d.get(0..4)?;
    if tag == b"ttcf" {
        let n = be32(d, 8)? as usize;
        if n == 0 || n > 4096 {
            return None;
        }
        let mut face_offsets = Vec::with_capacity(n);
        for i in 0..n {
            face_offsets.push(be32(d, 12 + 4 * i)?);
        }
        return Some(FontFileInfo {
            container: FontContainer::Collection,
            face_offsets,
        });
    }
    if tag == [0, 1, 0, 0] || tag == b"true" || tag == b"OTTO" {
        return Some(FontFileInfo {
            container: FontContainer::Single,
            face_offsets: vec![0],
        });
    }
    None
}

/// Family (name id 1) and full (name id 4) names of the face whose table directory is at `dir`.
/// Platform 3 (Windows) names are UTF-16BE; platform 1 (Macintosh) names are read as Latin-1.
pub fn face_names(d: &[u8], dir: u32) -> Vec<String> {
    let dir = dir as usize;
    let mut out = Vec::new();
    let Some(num_tables) = be16(d, dir + 4) else {
        return out;
    };
    let mut name_off = None;
    for i in 0..num_tables as usize {
        let r = dir + 12 + 16 * i;
        if d.get(r..r + 4) == Some(b"name") {
            name_off = be32(d, r + 8);
            break;
        }
    }
    let Some(nt) = name_off.map(|o| o as usize) else {
        return out;
    };
    let (Some(count), Some(storage)) = (be16(d, nt + 2), be16(d, nt + 4)) else {
        return out;
    };
    for i in 0..count as usize {
        let r = nt + 6 + 12 * i;
        let (Some(platform), Some(name_id), Some(len), Some(off)) =
            (be16(d, r), be16(d, r + 6), be16(d, r + 8), be16(d, r + 10))
        else {
            break;
        };
        if name_id != 1 && name_id != 4 {
            continue;
        }
        let start = nt + storage as usize + off as usize;
        let Some(bytes) = d.get(start..start + len as usize) else {
            continue;
        };
        let s = match platform {
            0 | 3 => String::from_utf16_lossy(
                &bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes(*c))
                    .collect::<Vec<_>>(),
            ),
            _ => bytes.iter().map(|&b| b as char).collect(),
        };
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// `true` when some face of the file has `face` as family or full name (ASCII case-insensitive).
pub fn file_has_face(d: &[u8], face: &str) -> bool {
    font_file_info(d).is_some_and(|info| {
        info.face_offsets.iter().any(|&o| {
            face_names(d, o)
                .iter()
                .any(|n| n.eq_ignore_ascii_case(face))
        })
    })
}

/// A font file located in the archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFont {
    /// Entry path inside the archive.
    pub entry: String,
    pub offset: u64,
    pub size: u32,
}

/// Finds font files in the client's font archive.
#[derive(Debug)]
pub struct FontResolver {
    archive: Option<RezArchive>,
}

impl FontResolver {
    /// Opens `Game/FONTS.rez` of the client folder (`archive` = `None` when the folder has none).
    pub fn open(rom: &Path) -> Result<Self, UiDataError> {
        let p = crate::find_game_file(rom, FONT_ARCHIVE)
            .unwrap_or_else(|| rom.join("Game").join(FONT_ARCHIVE));
        Self::open_archive(&p)
    }

    /// Opens a font archive at an explicit path (missing file = empty resolver).
    pub fn open_archive(path: &Path) -> Result<Self, UiDataError> {
        if !path.is_file() {
            return Ok(FontResolver { archive: None });
        }
        Ok(FontResolver {
            archive: Some(RezArchive::open(path)?),
        })
    }

    pub fn archive_path(&self) -> Option<PathBuf> {
        self.archive.as_ref().map(|a| a.path().to_path_buf())
    }

    /// Locates the record's file by its base name (archive lookup is case-insensitive). `None` when the
    /// record names no file or the archive lacks it (SPEC §7.1: 2 records of the ROM name an absent file).
    pub fn resolve(&self, spec: &FontSpec) -> Option<ResolvedFont> {
        let file = spec.file.as_deref()?;
        let a = self.archive.as_ref()?;
        let (offset, size) = a.locate(file)?;
        let entry = a
            .entries()
            .iter()
            .find(|(_, o, s)| *o == offset && *s == size)
            .map(|(n, _, _)| n.clone())
            .unwrap_or_else(|| file.to_string());
        Some(ResolvedFont {
            entry,
            offset,
            size,
        })
    }

    /// Bytes of a resolved file (font files are stored plain).
    pub fn read(&self, r: &ResolvedFont) -> Result<Vec<u8>, UiDataError> {
        let a = self.archive.as_ref().ok_or(UiDataError::MissingEntry {
            what: "font archive",
        })?;
        Ok(a.read_raw(r.offset, r.size)?)
    }

    /// Every font file of the archive: `(entry, container info)`; entries that are not fonts are skipped.
    pub fn inventory(&self) -> Result<Vec<(String, FontFileInfo)>, UiDataError> {
        let Some(a) = self.archive.as_ref() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for (name, off, size) in a.entries() {
            let head = a.read_raw(*off, (*size).min(16 + 4 * 4096))?;
            let info = if head.get(0..4) == Some(b"ttcf") {
                font_file_info(&head)
            } else {
                font_file_info(head.get(0..4).unwrap_or(&[]))
            };
            if let Some(info) = info {
                out.push((name.clone(), info));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal sfnt with only a `name` table holding `family` (platform 3, UTF-16BE, name id 1).
    pub(crate) fn synth_ttf(family: &str) -> Vec<u8> {
        let name_bytes: Vec<u8> = family
            .encode_utf16()
            .flat_map(|u| u.to_be_bytes())
            .collect();
        let mut d = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        let name_off = 12 + 16;
        d.extend_from_slice(b"name");
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(&(name_off as u32).to_be_bytes());
        d.extend_from_slice(&((6 + 12 + name_bytes.len()) as u32).to_be_bytes());
        // name table: format 0, count 1, storage offset 18
        d.extend_from_slice(&[0, 0, 0, 1, 0, 18]);
        d.extend_from_slice(&[0, 3, 0, 1, 0x04, 0x09, 0, 1]);
        d.extend_from_slice(&(name_bytes.len() as u16).to_be_bytes());
        d.extend_from_slice(&[0, 0]);
        d.extend_from_slice(&name_bytes);
        d
    }

    fn rec(i: u32, name: &str, file: &str, face: &str) -> FontRecord {
        FontRecord {
            index: i,
            name: name.into(),
            file: file.into(),
            face: face.into(),
            height: -14,
            bold: true,
            ..Default::default()
        }
    }

    #[test]
    fn spec_from_record_rules() {
        assert!(FontSpec::from_record(&rec(0, "", "", "F")).is_none());
        assert!(FontSpec::from_record(&rec(0, "n", "", "")).is_none());
        let s = FontSpec::from_record(&rec(2, "n", "d\\x.ttf", "")).unwrap();
        assert_eq!(s.file.as_deref(), Some("d\\x.ttf"));
        assert_eq!(s.weight(true), 700);
        assert_eq!(s.weight(false), 400);
        assert_eq!(s.size_px(), 14);
    }

    #[test]
    fn table_lookup_and_compaction() {
        let text = "[Fnt0]\nNm = \"a\"\nFace = \"A\"\n[Fnt1]\nNm = \"\"\nFace = \"B\"\n[Fnt2]\nNm = \"c\"\nFace = \"C\"\n";
        let t = LayoutTable::parse(text, crate::names::UiNames::placeholder());
        let f = SystemFonts::from_layout(&t);
        assert_eq!(f.len(), 3);
        assert_eq!(f.get(2, false).unwrap().face, "C");
        assert!(f.get(1, false).is_none());
        assert_eq!(f.get(1, true).unwrap().face, "A");
        assert_eq!(f.get(7, true).unwrap().face, "A");
        assert!(f.get(7, false).is_none());
        let c = f.compacted();
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].face, "C");
    }

    #[test]
    fn sfnt_headers_and_names() {
        let ttf = synth_ttf("Demo Sans");
        let info = font_file_info(&ttf).unwrap();
        assert_eq!(info.container, FontContainer::Single);
        assert_eq!(face_names(&ttf, 0), vec!["Demo Sans".to_string()]);
        assert!(file_has_face(&ttf, "demo sans"));
        assert!(!file_has_face(&ttf, "Other"));
        // a collection of two copies
        let mut ttc = b"ttcf".to_vec();
        ttc.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 2]);
        let base = 12 + 8;
        ttc.extend_from_slice(&(base as u32).to_be_bytes());
        ttc.extend_from_slice(&(base as u32).to_be_bytes());
        // table directory offsets inside a TTC are file-absolute: patch the name table offset
        let mut face = ttf.clone();
        let off = u32::from_be_bytes([face[20], face[21], face[22], face[23]]) + base as u32;
        face[20..24].copy_from_slice(&off.to_be_bytes());
        ttc.extend_from_slice(&face);
        let info = font_file_info(&ttc).unwrap();
        assert_eq!(info.container, FontContainer::Collection);
        assert_eq!(info.face_offsets.len(), 2);
        assert!(file_has_face(&ttc, "Demo Sans"));
        // hostile input
        assert!(font_file_info(b"tt").is_none());
        assert!(font_file_info(b"ttcf\0\0\0\0\xff\xff\xff\xff").is_none());
        assert!(face_names(&ttf[..20], 0).is_empty());
        for cut in 0..ttf.len() {
            let _ = face_names(&ttf[..cut], 0);
        }
    }

    #[test]
    fn resolver_without_archive_is_empty() {
        let r = FontResolver::open_archive(Path::new("definitely/not/here.rez")).unwrap();
        let s = FontSpec::from_record(&rec(0, "n", "x.ttf", "")).unwrap();
        assert!(r.resolve(&s).is_none());
        assert!(r.inventory().unwrap().is_empty());
    }
}
