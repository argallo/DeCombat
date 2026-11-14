//! The world archive of the user's ROM: map names and world bytes. Nothing read here is stored in the tree.

use crate::{RezArchive, RezError, RezVfs};
use std::path::{Path, PathBuf};

/// Name of the world archive inside the game folder.
// OBSERVED: ROM table key WORLDS (the archive holding all 21 `.dat` worlds, FMT_DAT85.md §11)
pub const WORLD_ARCHIVE: &str = "WORLDS.rez";

/// The version word a world file starts with.
// SPEC: FMT_DAT85.md §3 (version 85)
const WORLD_VERSION: u32 = 85;

fn has_ext(name: &str, ext: &str) -> bool {
    name.len() > ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext)
}

/// The world archive of the user's ROM.
#[derive(Debug)]
pub struct WorldArchive {
    rom: PathBuf,
    archive: RezArchive,
}

impl WorldArchive {
    pub fn open(rom: &Path) -> Result<WorldArchive, RezError> {
        let archive = RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE))?;
        Ok(WorldArchive {
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
        let raw = self.archive.read_raw(off, size)?;
        if raw.starts_with(&WORLD_VERSION.to_le_bytes()) {
            return Ok(Some(raw));
        }
        RezVfs::new(&self.rom).entry(WORLD_ARCHIVE, path)
    }
}
