//! PE resource directory walker (PUBLIC: Microsoft PE/COFF specification, `.rsrc` section: three directory
//! levels type / name-or-id / language; a leaf is a data entry with data rva and size).
//!
//! Works on a [`dc_rom::PeFile`]; every offset is bounds- and overflow-checked and every child table must lie
//! after its parent, so hostile images yield [`UiDataError::BadResource`] instead of a panic or a loop.
// SPEC: FMT_CRES_UI.md §2

use crate::error::UiDataError;
use dc_rom::PeFile;

/// Resource type id of string tables (PUBLIC: `RT_STRING`).
pub const RES_TYPE_STRING: u32 = 6;
/// Resource type id of a cursor image (PUBLIC: `RT_CURSOR`).
pub const RES_TYPE_CURSOR: u32 = 1;
/// Resource type id of a cursor group directory (PUBLIC: `RT_GROUP_CURSOR`).
pub const RES_TYPE_GROUP_CURSOR: u32 = 12;

/// Index of the resource table in the optional header's data directory (PUBLIC).
const DIR_RESOURCE: usize = 2;
/// Upper bound on the entries of one directory table we accept (hostile-input guard).
const MAX_ENTRIES: usize = 0x4000;

/// A resource name: numeric id or a UTF-16 name string.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResName {
    Id(u32),
    Name(String),
}

impl ResName {
    pub fn id(&self) -> Option<u32> {
        match self {
            ResName::Id(i) => Some(*i),
            ResName::Name(_) => None,
        }
    }
}

/// One leaf of the resource tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResLeaf {
    pub kind: ResName,
    pub name: ResName,
    pub lang: ResName,
    /// Rva of the resource bytes.
    pub data_rva: u32,
    pub size: u32,
}

fn bad(what: &'static str, offset: usize) -> UiDataError {
    UiDataError::BadResource {
        what,
        offset: offset as u64,
    }
}

fn u16_at(d: &[u8], o: usize) -> Result<u16, UiDataError> {
    let s = o
        .checked_add(2)
        .and_then(|e| d.get(o..e))
        .ok_or_else(|| bad("u16", o))?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(d: &[u8], o: usize) -> Result<u32, UiDataError> {
    let s = o
        .checked_add(4)
        .and_then(|e| d.get(o..e))
        .ok_or_else(|| bad("u32", o))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn add(a: usize, b: usize) -> Result<usize, UiDataError> {
    a.checked_add(b).ok_or_else(|| bad("offset overflow", a))
}

/// `(rva, size)` of the resource table from the optional header's data directory, `None` when the image
/// has no resource table.
pub fn resource_table(pe: &PeFile) -> Result<Option<(u32, u32)>, UiDataError> {
    let d = &pe.data;
    let nt = u32_at(d, 0x3c)? as usize;
    let opt = add(nt, 24)?;
    let magic = u16_at(d, opt)?;
    // PUBLIC: PE32 (0x10b) data directories start at optional-header offset 96, PE32+ (0x20b) at 112;
    // the directory count is the u32 just before them.
    let (count_off, dirs_off) = match magic {
        0x10b => (92usize, 96usize),
        0x20b => (108, 112),
        _ => return Err(bad("optional header magic", opt)),
    };
    let count = u32_at(d, add(opt, count_off)?)? as usize;
    if count <= DIR_RESOURCE {
        return Ok(None);
    }
    let e = add(opt, dirs_off + DIR_RESOURCE * 8)?;
    let rva = u32_at(d, e)?;
    let size = u32_at(d, add(e, 4)?)?;
    if rva == 0 || size == 0 {
        return Ok(None);
    }
    Ok(Some((rva, size)))
}

/// Walks the whole resource tree of `pe` and returns every leaf in directory order.
pub fn resource_leaves(pe: &PeFile) -> Result<Vec<ResLeaf>, UiDataError> {
    let Some((rva, size)) = resource_table(pe)? else {
        return Ok(Vec::new());
    };
    let sec = match pe.rva(rva, size as usize) {
        Ok(s) => s,
        Err(_) => {
            // The directory size may exceed the raw bytes of the section: use the largest mapped prefix.
            let mut n = size as usize;
            loop {
                n /= 2;
                if n < 16 {
                    return Err(bad("resource section", rva as usize));
                }
                if let Ok(s) = pe.rva(rva, n) {
                    break s;
                }
            }
        }
    };
    walk(sec)
}

/// Walks a resource section image `sec` (offsets in the directory are relative to its start).
pub fn walk(sec: &[u8]) -> Result<Vec<ResLeaf>, UiDataError> {
    let mut out = Vec::new();
    for (kind, l1) in dir_entries(sec, 0)? {
        let Target::Dir(t1) = l1 else {
            return Err(bad("type level is a leaf", 0));
        };
        for (name, l2) in dir_entries(sec, t1)? {
            let Target::Dir(t2) = l2 else {
                return Err(bad("name level is a leaf", t1));
            };
            for (lang, l3) in dir_entries(sec, t2)? {
                let Target::Leaf(leaf) = l3 else {
                    return Err(bad("language level is a directory", t2));
                };
                let data_rva = u32_at(sec, leaf)?;
                let size = u32_at(sec, add(leaf, 4)?)?;
                out.push(ResLeaf {
                    kind: kind.clone(),
                    name: name.clone(),
                    lang,
                    data_rva,
                    size,
                });
            }
        }
    }
    Ok(out)
}

enum Target {
    Dir(usize),
    Leaf(usize),
}

fn dir_entries(sec: &[u8], at: usize) -> Result<Vec<(ResName, Target)>, UiDataError> {
    let named = u16_at(sec, add(at, 12)?)? as usize;
    let ids = u16_at(sec, add(at, 14)?)? as usize;
    let n = named + ids;
    if n > MAX_ENTRIES {
        return Err(bad("directory entry count", at));
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let e = add(at, 16 + i * 8)?;
        let name_word = u32_at(sec, e)?;
        let off_word = u32_at(sec, add(e, 4)?)?;
        let name = if name_word & 0x8000_0000 != 0 {
            let so = (name_word & 0x7fff_ffff) as usize;
            let len = u16_at(sec, so)? as usize;
            let mut units = Vec::with_capacity(len);
            for k in 0..len {
                units.push(u16_at(sec, add(so, 2 + 2 * k)?)?);
            }
            ResName::Name(String::from_utf16_lossy(&units))
        } else {
            ResName::Id(name_word & 0xffff)
        };
        let off = (off_word & 0x7fff_ffff) as usize;
        // A child must lie after its parent table: this bounds the walk on hostile (cyclic) trees.
        if off <= at {
            return Err(bad("backward directory link", e));
        }
        let target = if off_word & 0x8000_0000 != 0 {
            Target::Dir(off)
        } else {
            Target::Leaf(off)
        };
        out.push((name, target));
    }
    Ok(out)
}

/// Test helper: builds a resource section image from `(type, id, lang, bytes)` leaves (numeric names only)
/// laid out at `sec_rva`, in the public directory format.
#[cfg(test)]
pub(crate) fn synth_rsrc(sec_rva: u32, leaves: &[(u32, u32, u32, Vec<u8>)]) -> Vec<u8> {
    use std::collections::BTreeMap;
    type Tree<'a> = BTreeMap<u32, BTreeMap<u32, BTreeMap<u32, &'a Vec<u8>>>>;
    let mut tree: Tree = BTreeMap::new();
    for (t, n, l, b) in leaves {
        tree.entry(*t)
            .or_default()
            .entry(*n)
            .or_default()
            .insert(*l, b);
    }
    let dir_size = |n: usize| 16 + 8 * n;
    let mut off = dir_size(tree.len());
    let mut t_off = BTreeMap::new();
    for (t, names) in &tree {
        t_off.insert(*t, off);
        off += dir_size(names.len());
    }
    let mut n_off = BTreeMap::new();
    for (t, names) in &tree {
        for (n, langs) in names {
            n_off.insert((*t, *n), off);
            off += dir_size(langs.len());
        }
    }
    let mut leaf_off = BTreeMap::new();
    for (t, names) in &tree {
        for (n, langs) in names {
            for l in langs.keys() {
                leaf_off.insert((*t, *n, *l), off);
                off += 16;
            }
        }
    }
    let mut data_off = BTreeMap::new();
    for (t, names) in &tree {
        for (n, langs) in names {
            for (l, b) in langs {
                data_off.insert((*t, *n, *l), off);
                off += (b.len() + 3) & !3;
            }
        }
    }
    let mut s = vec![0u8; off];
    fn put16(s: &mut [u8], o: usize, v: u16) {
        s[o..o + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put32(s: &mut [u8], o: usize, v: u32) {
        s[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    put16(&mut s, 14, tree.len() as u16);
    for (i, (t, names)) in tree.iter().enumerate() {
        put32(&mut s, 16 + 8 * i, *t);
        put32(&mut s, 20 + 8 * i, 0x8000_0000 | t_off[t] as u32);
        let to = t_off[t];
        put16(&mut s, to + 14, names.len() as u16);
        for (j, (n, langs)) in names.iter().enumerate() {
            put32(&mut s, to + 16 + 8 * j, *n);
            put32(
                &mut s,
                to + 20 + 8 * j,
                0x8000_0000 | n_off[&(*t, *n)] as u32,
            );
            let no = n_off[&(*t, *n)];
            put16(&mut s, no + 14, langs.len() as u16);
            for (k, (l, b)) in langs.iter().enumerate() {
                let lo = leaf_off[&(*t, *n, *l)];
                put32(&mut s, no + 16 + 8 * k, *l);
                put32(&mut s, no + 20 + 8 * k, lo as u32);
                let dof = data_off[&(*t, *n, *l)];
                put32(&mut s, lo, sec_rva + dof as u32);
                put32(&mut s, lo + 4, b.len() as u32);
                s[dof..dof + b.len()].copy_from_slice(b);
            }
        }
    }
    s
}

/// Test helper: a minimal PE32 image with one section at `sec_rva` holding `sec` and the resource data
/// directory pointing at it.
#[cfg(test)]
pub(crate) fn synth_pe_with_rsrc(sec_rva: u32, sec: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 0x200];
    d[0..2].copy_from_slice(b"MZ");
    d[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    let pe = 0x80;
    d[pe..pe + 4].copy_from_slice(b"PE\0\0");
    d[pe + 6..pe + 8].copy_from_slice(&1u16.to_le_bytes());
    d[pe + 20..pe + 22].copy_from_slice(&0xe0u16.to_le_bytes());
    let opt = pe + 24;
    d[opt..opt + 2].copy_from_slice(&0x10bu16.to_le_bytes());
    d[opt + 28..opt + 32].copy_from_slice(&0x1000_0000u32.to_le_bytes());
    d[opt + 92..opt + 96].copy_from_slice(&16u32.to_le_bytes());
    let e = opt + 96 + 16;
    d[e..e + 4].copy_from_slice(&sec_rva.to_le_bytes());
    d[e + 4..e + 8].copy_from_slice(&(sec.len() as u32).to_le_bytes());
    let s = opt + 0xe0;
    d[s..s + 5].copy_from_slice(b".rsrc");
    d[s + 8..s + 12].copy_from_slice(&(sec.len() as u32).to_le_bytes());
    d[s + 12..s + 16].copy_from_slice(&sec_rva.to_le_bytes());
    d[s + 16..s + 20].copy_from_slice(&(sec.len() as u32).to_le_bytes());
    d[s + 20..s + 24].copy_from_slice(&0x400u32.to_le_bytes());
    d.resize(0x400, 0);
    d.extend_from_slice(sec);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_synthetic_tree() {
        let sec = synth_rsrc(
            0x4000,
            &[
                (6, 1, 0x409, vec![1, 2, 3]),
                (6, 3, 0x409, vec![4; 9]),
                (1, 1, 0x409, vec![9; 5]),
            ],
        );
        let pe = PeFile::from_bytes(synth_pe_with_rsrc(0x4000, &sec)).unwrap();
        let leaves = resource_leaves(&pe).unwrap();
        assert_eq!(leaves.len(), 3);
        assert_eq!(leaves[0].kind, ResName::Id(1));
        assert_eq!(leaves[1].name, ResName::Id(1));
        assert_eq!(leaves[2].name, ResName::Id(3));
        assert_eq!(leaves[2].lang, ResName::Id(0x409));
        assert_eq!(pe.rva(leaves[2].data_rva, 9).unwrap(), &[4u8; 9][..]);
    }

    #[test]
    fn hostile_trees_do_not_panic() {
        let sec = synth_rsrc(0x4000, &[(6, 1, 0x409, vec![1, 2, 3, 4])]);
        let mut x: u64 = 0x1234_5678_9abc_def1;
        for i in 0..4000usize {
            let mut s = sec.clone();
            for _ in 0..1 + i % 6 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let p = (x as usize) % s.len();
                s[p] = (x >> 32) as u8;
            }
            if i % 7 == 0 {
                s.truncate((x as usize) % s.len());
            }
            let _ = walk(&s);
        }
        // A backward link is refused.
        let mut s = sec.clone();
        s[20..24].copy_from_slice(&0x8000_0000u32.to_le_bytes());
        assert!(walk(&s).is_err());
    }

    #[test]
    fn image_without_resources_is_empty() {
        let mut img = synth_pe_with_rsrc(0x4000, &[0u8; 32]);
        let e = 0x80 + 24 + 96 + 16;
        img[e..e + 8].fill(0);
        let pe = PeFile::from_bytes(img).unwrap();
        assert!(resource_leaves(&pe).unwrap().is_empty());
    }
}
