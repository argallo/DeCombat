//! The downloaded server mods on this PC, for the Mods screen (CLI-1617 amendment, project decision): every
//! package of the by-hash cache (`<data>/mods/by-hash/<sha256>.zip`) with what its unpacked manifest
//! (`<data>/mods/unpacked/<sha256>/manifest.json`) says, and the removal of one package (zip and unpacked folder; the
//! next join that needs it downloads it again). The link thread's cache code (`crate::mods`) is not touched: this
//! module only reads the same folders and deletes one package on the player's request.
// DC-DEFAULT: CLI-1617 (proposed, amended 2026-11-01) the cache list and its Remove action, see DC_DEFAULTS.md

use std::path::{Path, PathBuf};

use crate::mods::{is_sha256_hex, mod_key, ModPaths};

/// One package of the by-hash cache.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedMod {
    pub sha256: String,
    /// The zip's size in bytes.
    pub bytes: u64,
    /// From the unpacked manifest; the id is empty when the package is not unpacked or its manifest is broken.
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub author: String,
    pub description: String,
    pub capabilities: Vec<String>,
    /// CLI-1943: the manifest's tags.
    pub tags: Vec<String>,
    /// MOD-27: the manifest's command help.
    pub commands: Vec<dc_mod_lua::ModCommand>,
    /// The first screenshot's file inside the unpacked folder, when the manifest names one.
    pub shot: Option<PathBuf>,
}

impl CachedMod {
    /// The consent key `id@version:sha256` (`None` without a readable manifest).
    pub fn key(&self) -> Option<String> {
        (!self.id.is_empty()).then(|| mod_key(&self.id, &self.version, &self.sha256))
    }
}

/// Every package of the cache, sorted by name then hash.
pub fn list_cache(paths: &ModPaths) -> Vec<CachedMod> {
    let Ok(rd) = std::fs::read_dir(paths.by_hash()) else {
        return Vec::new();
    };
    let mut out: Vec<CachedMod> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let sha = file.strip_suffix(".zip").filter(|h| is_sha256_hex(h))?.to_string();
            let bytes = e.metadata().ok().filter(|m| m.is_file())?.len();
            let dir = paths.unpacked(&sha);
            let man = dc_mod_lua::ModManifest::from_file(&dir.join("manifest.json")).ok();
            Some(match man {
                Some(m) => CachedMod {
                    sha256: sha,
                    bytes,
                    id: m.id.clone(),
                    name: m.name.clone(),
                    version: m.version.to_string(),
                    kind: m.kind.as_str().to_string(),
                    author: m.author.clone(),
                    description: m.description.clone(),
                    capabilities: m.capabilities.clone(),
                    tags: m.tags.clone(),
                    commands: m.commands.clone(),
                    shot: m
                        .screenshots
                        .first()
                        .map(|s| s.split('/').fold(dir.clone(), |p, c| p.join(c))),
                },
                None => CachedMod {
                    name: format!("Package {}", &sha[..12]),
                    sha256: sha,
                    bytes,
                    id: String::new(),
                    version: String::new(),
                    kind: String::new(),
                    author: String::new(),
                    description: "Not unpacked yet: the next join that needs it verifies and unpacks it.".into(),
                    capabilities: Vec::new(),
                    tags: Vec::new(),
                    commands: Vec::new(),
                    shot: None,
                },
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.sha256.cmp(&b.sha256)));
    out
}

/// Removes package `sha` (the zip and the unpacked folder). An error names what could not be removed.
pub fn remove_cached(paths: &ModPaths, sha: &str) -> Result<(), String> {
    if !is_sha256_hex(sha) {
        return Err("not a package hash".into());
    }
    let zip = paths.zip(sha);
    if zip.exists() {
        std::fs::remove_file(&zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    }
    let dir = paths.unpacked(sha);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

/// A size in plain words ("12 KB", "3.4 MB").
pub fn size_words(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} bytes")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Opens the OS file browser on `dir` (created first). Never fails the screen: an error is only logged.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(dir).spawn() {
        eprintln!("dc-client mods: cannot open {}: {e}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_removed_package_loses_its_zip_and_unpacked_folder() {
        let root = std::env::temp_dir().join(format!("dc_modcache_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = ModPaths::new(&root);
        let sha = "ab".repeat(32);
        std::fs::create_dir_all(paths.by_hash()).unwrap();
        std::fs::write(paths.zip(&sha), b"zip").unwrap();
        std::fs::create_dir_all(paths.unpacked(&sha)).unwrap();
        std::fs::write(paths.by_hash().join("notes.txt"), b"x").unwrap();
        let l = list_cache(&paths);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].bytes, 3);
        assert!(l[0].key().is_none(), "no manifest, no key");
        remove_cached(&paths, &sha).unwrap();
        assert!(!paths.zip(&sha).exists() && !paths.unpacked(&sha).exists());
        assert!(list_cache(&paths).is_empty());
        assert!(remove_cached(&paths, "../x").is_err());
        assert_eq!(size_words(2048), "2 KB");
        let _ = std::fs::remove_dir_all(&root);
    }
}
