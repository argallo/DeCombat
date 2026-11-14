//! The index of every entry of every archive in the client's game folder, keyed by its full relative path: what
//! [`dc_vfs::ArchiveLayer::find_any`] of [`crate::RomLayer`] looks names up in. Nothing read here is stored in the
//! tree.

use crate::{RezArchive, RezError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `name` upper case with `/` as `\` (bytes as Latin-1).
fn upper_backslash(name: &[u8]) -> String {
    name.iter()
        .map(|&c| {
            let c = if c == b'/' { b'\\' } else { c };
            c.to_ascii_uppercase() as char
        })
        .collect()
}

/// Index of every entry of every `.rez` archive in the game folder, keyed by its full relative path.
///
/// OBSERVED over the 21 ROM worlds: section texture names are full relative paths into the union of the
/// game folder's archives (for example `<top>\<dir>\<file>.dtx` found in exactly one archive); 1 or 2 names
/// per world resolve to nothing and fall back (RENDER_2009.md / FMT_DTX.md §1: the engine substitutes a
/// default texture, engine+0x19c40).
#[derive(Debug, Default)]
pub struct AnyIndex {
    archives: Vec<RezArchive>,
    by_path: HashMap<String, (usize, u64, u32)>,
}

impl AnyIndex {
    /// Walks every archive of `game_dir` (sorted by file name; the first archive that has a path wins).
    pub fn open(game_dir: &Path) -> Result<AnyIndex, RezError> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(game_dir)
            .map_err(|e| RezError::Io {
                path: game_dir.to_path_buf(),
                message: e.to_string(),
            })?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .map(|x| x.eq_ignore_ascii_case("rez"))
                    .unwrap_or(false)
            })
            .collect();
        files.sort();
        let mut idx = AnyIndex::default();
        for f in files {
            let arch = RezArchive::open(&f)?;
            let ai = idx.archives.len();
            for (path, off, size) in arch.entries() {
                idx.by_path
                    .entry(upper_backslash(path.as_bytes()))
                    .or_insert((ai, *off, *size));
            }
            idx.archives.push(arch);
        }
        Ok(idx)
    }

    /// Number of indexed entries.
    pub fn len(&self) -> usize {
        self.by_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }

    /// `(archive, offset, size)` of a full relative path.
    pub fn locate(&self, name: &[u8]) -> Option<(usize, u64, u32)> {
        if name.is_empty() {
            return None;
        }
        self.by_path.get(&upper_backslash(name)).copied()
    }

    /// Raw bytes of the entry (textures are stored plain; FMT_DTX.md §8 census reads them raw); `Ok(None)` for an
    /// absent or empty entry.
    // SPEC: FMT_DTX.md §8 (header-only census over raw entries)
    pub fn read(&self, name: &[u8]) -> Result<Option<Vec<u8>>, RezError> {
        match self.locate(name) {
            Some((a, off, size)) if size > 0 => self.archives[a].read_raw(off, size).map(Some),
            _ => Ok(None),
        }
    }
}
