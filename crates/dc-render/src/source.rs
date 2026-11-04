//! Read-only access to the user's ROM for the renderer: the world archive and a texture index over every
//! archive of the client's game folder. Nothing read here is stored in the tree.

use dc_rez::{RezArchive, RezError, RezVfs};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Name of the world archive inside the game folder.
// OBSERVED: ROM table key WORLDS (the archive holding all 21 `.dat` worlds, FMT_DAT85.md §11)
const WORLD_ARCHIVE: &str = "WORLDS.rez";

fn game_dir(rom: &Path) -> PathBuf {
    rom.join("Game")
}

fn upper_backslash(name: &[u8]) -> String {
    name.iter()
        .map(|&c| {
            let c = if c == b'/' { b'\\' } else { c };
            c.to_ascii_uppercase() as char
        })
        .collect()
}

fn has_ext(name: &str, ext: &str) -> bool {
    name.len() > ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext)
}

/// The world archive of the user's ROM.
#[derive(Debug)]
pub struct WorldSource {
    rom: PathBuf,
    archive: RezArchive,
}

impl WorldSource {
    pub fn open(rom: &Path) -> Result<WorldSource, RezError> {
        let archive = RezArchive::open(&game_dir(rom).join(WORLD_ARCHIVE))?;
        Ok(WorldSource {
            rom: rom.to_path_buf(),
            archive,
        })
    }

    /// `(map name, full entry path, offset, size)` of every world entry: the name is the entry's basename
    /// without extension, upper case, read from the user's archive.
    fn world_entries(&self) -> Vec<(String, &str, u64, u32)> {
        self.archive
            .entries()
            .iter()
            .filter(|e| has_ext(&e.0, ".dat"))
            .map(|e| {
                let base = e.0.rsplit(['\\', '/']).next().unwrap_or(&e.0);
                (
                    base[..base.len() - 4].to_ascii_uppercase(),
                    e.0.as_str(),
                    e.1,
                    e.2,
                )
            })
            .collect()
    }

    /// Map names, sorted.
    pub fn map_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.world_entries().into_iter().map(|e| e.0).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Bytes of the world `map` (case-insensitive name without extension), `Ok(None)` when absent. The entry
    /// is found by walking the archive listing (independent of the archive reader's name-lookup rules).
    ///
    /// World entries are stored unencrypted (FMT_DAT85.md addendum: the first word is the version), so the raw
    /// bytes are used when they carry the version word; anything else goes through the archive VFS.
    // SPEC: FMT_DAT85.md Addendum 2026-10-24 (world entries stored plain), §3 (version 85)
    pub fn read(&self, map: &str) -> Result<Option<Vec<u8>>, RezError> {
        let want = map.to_ascii_uppercase();
        let Some((_, path, off, size)) = self.world_entries().into_iter().find(|e| e.0 == want)
        else {
            return Ok(None);
        };
        let _io = crate::perf::span(crate::perf::Counter::IoNs);
        crate::perf::add(crate::perf::Counter::IoBytes, size as u64);
        let raw = self.archive.read_raw(off, size)?;
        if raw.starts_with(&dc_world::WORLD_VERSION.to_le_bytes()) {
            return Ok(Some(raw));
        }
        RezVfs::new(&self.rom).entry(WORLD_ARCHIVE, path)
    }
}

/// Index of every entry of every `.rez` archive in the game folder, keyed by its full relative path.
///
/// OBSERVED over the 21 ROM worlds: section texture names are full relative paths into the union of the
/// game folder's archives (for example `<top>\<dir>\<file>.dtx` found in exactly one archive); 1 or 2 names
/// per world resolve to nothing and fall back (RENDER_2009.md / FMT_DTX.md §1: the engine substitutes a
/// default texture, engine+0x19c40).
#[derive(Debug, Default)]
pub struct TextureIndex {
    archives: Vec<RezArchive>,
    by_path: HashMap<String, (usize, u64, u32)>,
}

impl TextureIndex {
    /// Walks every archive of `<rom>/Game` (sorted by file name; the first archive that has a path wins).
    pub fn open(rom: &Path) -> Result<TextureIndex, RezError> {
        let dir = game_dir(rom);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| RezError::Io {
                path: dir.clone(),
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
        let mut idx = TextureIndex::default();
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

    /// `(archive, offset, size)` of a texture name as stored in a world section.
    pub fn locate(&self, name: &[u8]) -> Option<(usize, u64, u32)> {
        if name.is_empty() {
            return None;
        }
        self.by_path.get(&upper_backslash(name)).copied()
    }

    /// Raw bytes of the entry (textures are stored plain; FMT_DTX.md §8 census reads them raw).
    // SPEC: FMT_DTX.md §8 (header-only census over raw entries)
    pub fn read(&self, name: &[u8]) -> Result<Option<Vec<u8>>, RezError> {
        let _io = crate::perf::span(crate::perf::Counter::IoNs);
        match self.locate(name) {
            Some((a, off, size)) if size > 0 => {
                crate::perf::add(crate::perf::Counter::IoBytes, size as u64);
                self.archives[a].read_raw(off, size).map(Some)
            }
            _ => Ok(None),
        }
    }
}
