//! Layered virtual file system. A lookup of entry `name` of archive `archive` tries, in order:
//!
//! 0. the **packs** ([`PackTable`], [`Vfs::add_pack`]): a `@mod:<id>/assets/<rel>` name is the file of that pack and
//!    nothing else (a refused or unknown one is [`VfsError::ModPath`]); for every other name the packs' `overrides/`
//!    folders come first (the last pack first), with the candidates of the override folders below;
//! 1. the **override folders** (`add_override_dir`; the last one added first), with the archive-relative and
//!    basename candidates mods have always used;
//! 2. the **base content folders** (`add_root`; the last one added first): `<root>/<name>` with `name` lower case and
//!    `/`-separated, the archive name ignored, exact path only (no basename fallback, so a loose file never shadows an
//!    archive entry it was not named for);
//! 3. the **archive layer**, when there is one ([`ArchiveLayer`]: archives, their key and their own caches).
//!
//! With no pack and no base folder this reads exactly what the archive layer and the override folders read before
//! this crate existed ([`install`] with an empty [`Content`] builds that file system). Text entries (`.txt`, `.gms`,
//! `.bms`) found in a folder are returned as they are when plain text and handed to the archive layer to decrypt
//! otherwise; with no archive layer an encrypted text file is an error.
// Provenance: ORIGINAL (our own code; no 2009 behaviour)

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod pack;

pub use pack::{clean_asset_path, is_mod_path, Pack, PackTable, MOD_SCHEME};

/// Everything a lookup can fail with.
#[derive(Debug, Clone)]
pub enum VfsError {
    /// Entry or archive name is empty, absolute, drive-qualified or contains `..` or NUL.
    BadName(String),
    /// A file of an override or base folder could not be read.
    Io { path: PathBuf, message: String },
    /// `list` of an archive that does not exist (or no archive layer).
    ArchiveNotFound(String),
    /// An encrypted text file in a folder and no archive layer to decrypt it.
    NoKey(String),
    /// An error of the archive layer; that layer's crate converts it back into its own error type.
    Layer(Arc<dyn std::error::Error + Send + Sync>),
    /// A `@mod:` name the packs refuse (unknown id, outside `assets/`, traversal); only with at least one pack.
    ModPath(String),
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VfsError::BadName(n) => write!(f, "invalid entry name {n:?}"),
            VfsError::Io { path, message } => {
                write!(f, "I/O error on {}: {}", path.display(), message)
            }
            VfsError::ArchiveNotFound(n) => write!(f, "archive {n} not found"),
            VfsError::NoKey(n) => write!(
                f,
                "{n} is encrypted text and there is no archive key to read it"
            ),
            VfsError::Layer(e) => write!(f, "{e}"),
            VfsError::ModPath(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for VfsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            VfsError::Layer(e) => Some(&**e),
            _ => None,
        }
    }
}

/// Archives behind the folders: implemented by the crate that reads them (their format, key and caches).
pub trait ArchiveLayer: Send + Sync + fmt::Debug {
    /// Entry `name` of `archive` (`by_basename`: resolve a unique basename instead of the full path; `decode`:
    /// decrypt a text entry); `Ok(None)` when the archive or the entry does not exist.
    fn read(
        &self,
        archive: &str,
        name: &str,
        by_basename: bool,
        decode: bool,
    ) -> Result<Option<Vec<u8>>, VfsError>;
    /// The first `n` stored bytes of an entry, without reading the rest.
    fn head(&self, archive: &str, name: &str, n: usize) -> Result<Option<Vec<u8>>, VfsError>;
    /// Every `(full path, offset, size)` of `archive`; [`VfsError::ArchiveNotFound`] when there is none.
    fn list(&self, archive: &str) -> Result<Vec<(String, u64, u32)>, VfsError>;
    /// Decrypts `raw`, the bytes of a text entry found in a folder that is not plain text.
    fn decrypt_text(&self, name: &str, raw: &[u8]) -> Result<Vec<u8>, VfsError>;
    /// The folder the archives live under.
    fn root(&self) -> &Path;
    /// Raw bytes of the entry whose full relative path is `name` (ASCII case and `/` against `\` ignored) in
    /// whichever archive under the root holds it; `Ok(None)` when none does. World section texture names are such
    /// paths. A layer without an index over all its archives finds nothing.
    fn find_any(&self, name: &[u8]) -> Result<Option<Vec<u8>>, VfsError> {
        let _ = name;
        Ok(None)
    }
    /// Whether [`ArchiveLayer::find_any`] finds `name`, without reading it.
    fn has_any(&self, name: &[u8]) -> bool {
        let _ = name;
        false
    }
    /// How many entries [`ArchiveLayer::find_any`] can find.
    fn any_count(&self) -> usize {
        0
    }
}

/// The layered file system ([crate docs](crate)).
#[derive(Debug, Default)]
pub struct Vfs {
    packs: PackTable,
    override_dirs: Vec<PathBuf>,
    roots: Vec<PathBuf>,
    archive: Option<Arc<dyn ArchiveLayer>>,
    /// Negative entry cache of archive lookups: `(UPPER_ARCHIVE, "P:"|"B:" + UPPER_PATH)` known to be absent.
    missing: Mutex<HashSet<(String, String)>>,
}

/// `true` when `name` has one of the text-table extensions (`.txt`, `.gms`, `.bms`, case-insensitive).
pub fn is_text_name(name: &str) -> bool {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rsplit_once('.') {
        Some((_, ext)) => ["txt", "gms", "bms"]
            .iter()
            .any(|t| ext.eq_ignore_ascii_case(t)),
        None => false,
    }
}

/// `true` if `data` looks like unencrypted plain text (first 64 bytes only `\r`, `\n`, `\t` or printable ASCII).
pub fn is_plain(data: &[u8]) -> bool {
    let head = &data[..data.len().min(64)];
    !head.is_empty()
        && head
            .iter()
            .all(|&c| matches!(c, b'\r' | b'\n' | b'\t' | 32..=126))
}

/// Rejects empty, absolute, drive-qualified, NUL- or `..`-containing names (they are joined onto folders).
pub fn validate_name(name: &str) -> Result<(), VfsError> {
    let norm = name.replace('\\', "/");
    let bad = norm.is_empty()
        || norm.starts_with('/')
        || norm.contains(':')
        || norm.contains('\0')
        || norm.split('/').any(|c| c == "..");
    if bad {
        Err(VfsError::BadName(name.to_string()))
    } else {
        Ok(())
    }
}

/// Canonical full-path key: separators folded to `/`, leading separators dropped, ASCII upper case.
fn relpath_key(name: &str) -> String {
    name.replace('\\', "/")
        .trim_start_matches('/')
        .to_ascii_uppercase()
}

fn missing_key(archive: &str, name: &str, by_basename: bool) -> (String, String) {
    (
        archive.to_ascii_uppercase(),
        format!(
            "{}{}",
            if by_basename { "B:" } else { "P:" },
            relpath_key(name)
        ),
    )
}

fn latin1_text(mut bytes: Vec<u8>) -> String {
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    bytes.iter().map(|&b| b as char).collect()
}

fn read_noted(path: &Path) -> Result<Vec<u8>, VfsError> {
    let raw = fs::read(path).map_err(|e| VfsError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    dc_core::journal::note(|| format!("file:{}", path.display()), &raw);
    dc_core::trace::note("file", path, "", raw.len() as u64);
    Ok(raw)
}

impl Vfs {
    /// No folders and no archives: every lookup answers `None` until folders are added.
    pub fn new() -> Self {
        Self::default()
    }

    /// Folders over `archive` (the archive layer answers last).
    pub fn with_archive(archive: Arc<dyn ArchiveLayer>) -> Self {
        Self {
            archive: Some(archive),
            ..Self::default()
        }
    }

    /// The archive layer's folder, when there is one.
    pub fn archive_root(&self) -> Option<&Path> {
        self.archive.as_deref().map(|a| a.root())
    }

    pub fn has_archive(&self) -> bool {
        self.archive.is_some()
    }

    /// The archive layer itself, shared (a texture index over [`ArchiveLayer::find_any`] keeps one).
    pub fn archive_layer(&self) -> Option<Arc<dyn ArchiveLayer>> {
        self.archive.clone()
    }

    /// Registers an override folder that takes precedence over the base folders, the archives and the override
    /// folders registered before it.
    pub fn add_override_dir(&mut self, path: impl Into<PathBuf>) {
        self.override_dirs.push(path.into());
    }

    pub fn override_dirs(&self) -> &[PathBuf] {
        &self.override_dirs
    }

    /// Registers a base content folder: entries by exact lower-case relative path, after the override folders and
    /// before the archives; a later folder takes precedence over an earlier one.
    pub fn add_root(&mut self, path: impl Into<PathBuf>) {
        self.roots.push(path.into());
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Registers a pack after the others: its `@mod:<id>/assets/` names, and its `overrides/` folder ahead of every
    /// override folder and of the packs registered before it.
    pub fn add_pack(&mut self, pack: Pack) {
        self.packs.push(pack);
    }

    pub fn packs(&self) -> &PackTable {
        &self.packs
    }

    /// The file a `@mod:<id>/assets/<rel>` name stands for ([`PackTable::resolve`]).
    pub fn resolve_mod(&self, uri: &str) -> Result<PathBuf, VfsError> {
        self.packs.resolve(uri).map_err(VfsError::ModPath)
    }

    /// `Some` for a `@mod:` name while there is a pack: the pack file's bytes, or the refusal. With no pack a `@mod:`
    /// name goes on to the name check, as before packs existed.
    fn mod_file(&self, name: &str) -> Option<Result<Vec<u8>, VfsError>> {
        if self.packs.is_empty() || !is_mod_path(name) {
            return None;
        }
        Some(self.resolve_mod(name).and_then(|p| read_noted(&p)))
    }

    fn find_override_raw(&self, archive: &str, name: &str) -> Result<Option<Vec<u8>>, VfsError> {
        let pack_dirs = self.packs.override_dirs();
        if self.override_dirs.is_empty() && pack_dirs.is_empty() {
            return Ok(None);
        }
        let rel_norm = name.replace('\\', "/");
        let basename = rel_norm.rsplit('/').next().unwrap_or(&rel_norm);
        let stem = archive
            .strip_suffix(".rez")
            .or_else(|| archive.strip_suffix(".REZ"))
            .unwrap_or(archive);
        // The packs' folders first, then the override folders; later ones have higher priority in each group.
        for dir in pack_dirs
            .iter()
            .rev()
            .chain(self.override_dirs.iter().rev())
        {
            let candidates = [
                dir.join(archive).join(&rel_norm),
                dir.join(archive).join(basename),
                dir.join(stem).join(&rel_norm),
                dir.join(stem).join(basename),
                dir.join(&rel_norm),
                dir.join(basename),
            ];
            for cand in &candidates {
                if cand.is_file() {
                    return read_noted(cand).map(Some);
                }
            }
        }
        Ok(None)
    }

    fn find_root_raw(&self, name: &str) -> Result<Option<Vec<u8>>, VfsError> {
        if self.roots.is_empty() {
            return Ok(None);
        }
        let rel = name
            .replace('\\', "/")
            .trim_start_matches('/')
            .to_ascii_lowercase();
        for root in self.roots.iter().rev() {
            let p = root.join(&rel);
            if p.is_file() {
                return read_noted(&p).map(Some);
            }
        }
        Ok(None)
    }

    /// An override file, else a base-folder file: the stored bytes.
    fn find_folder_raw(&self, archive: &str, name: &str) -> Result<Option<Vec<u8>>, VfsError> {
        match self.find_override_raw(archive, name)? {
            Some(raw) => Ok(Some(raw)),
            None => self.find_root_raw(name),
        }
    }

    fn decode_folder_file(&self, name: &str, raw: Vec<u8>) -> Result<Vec<u8>, VfsError> {
        if !is_text_name(name) || is_plain(&raw) {
            return Ok(raw);
        }
        match &self.archive {
            Some(a) => a.decrypt_text(name, &raw),
            None => Err(VfsError::NoKey(name.to_string())),
        }
    }

    fn fetch(
        &self,
        archive: &str,
        name: &str,
        by_basename: bool,
        decode: bool,
    ) -> Result<Option<Vec<u8>>, VfsError> {
        if let Some(r) = self.mod_file(name) {
            let raw = r?;
            return if decode {
                self.decode_folder_file(name, raw).map(Some)
            } else {
                Ok(Some(raw))
            };
        }
        validate_name(name)?;
        validate_name(archive)?;
        if let Some(raw) = self.find_folder_raw(archive, name)? {
            return if decode {
                self.decode_folder_file(name, raw).map(Some)
            } else {
                Ok(Some(raw))
            };
        }
        let Some(layer) = &self.archive else {
            return Ok(None);
        };
        let neg_key = missing_key(archive, name, by_basename);
        if self
            .missing
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&neg_key)
        {
            return Ok(None);
        }
        let found = layer.read(archive, name, by_basename, decode)?;
        if found.is_none() {
            self.missing
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(neg_key);
        }
        Ok(found)
    }

    /// Entry `name` (full relative path) of `archive`; text tables (`.txt`, `.gms`, `.bms`) are decrypted unless
    /// already plain, every other entry is returned as stored. `Ok(None)` if no layer has it.
    pub fn entry(&self, archive: &str, name: &str) -> Result<Option<Vec<u8>>, VfsError> {
        self.fetch(archive, name, false, true)
    }

    /// Stored bytes of an entry with no decryption at all.
    pub fn entry_raw(&self, archive: &str, name: &str) -> Result<Option<Vec<u8>>, VfsError> {
        self.fetch(archive, name, false, false)
    }

    /// The first `n` stored bytes of an entry, without reading the rest of an archive entry.
    pub fn entry_head(
        &self,
        archive: &str,
        name: &str,
        n: usize,
    ) -> Result<Option<Vec<u8>>, VfsError> {
        if let Some(r) = self.mod_file(name) {
            let mut raw = r?;
            raw.truncate(n);
            return Ok(Some(raw));
        }
        validate_name(name)?;
        validate_name(archive)?;
        if let Some(mut raw) = self.find_folder_raw(archive, name)? {
            raw.truncate(n);
            return Ok(Some(raw));
        }
        match &self.archive {
            Some(a) => a.head(archive, name, n),
            None => Ok(None),
        }
    }

    /// Like [`Vfs::entry`], but an archive resolves `name` by its basename (unique within the archive only).
    pub fn entry_by_basename(
        &self,
        archive: &str,
        name: &str,
    ) -> Result<Option<Vec<u8>>, VfsError> {
        self.fetch(archive, name, true, true)
    }

    /// Latin-1 text of an entry resolved through the unique-basename fallback, trailing NULs stripped.
    pub fn entry_text_by_basename(
        &self,
        archive: &str,
        name: &str,
    ) -> Result<Option<String>, VfsError> {
        Ok(self.entry_by_basename(archive, name)?.map(latin1_text))
    }

    /// Text entry (Latin-1, trailing NULs stripped); alias of [`Vfs::entry_text_latin1`].
    pub fn entry_text(&self, archive: &str, name: &str) -> Result<Option<String>, VfsError> {
        self.entry_text_latin1(archive, name)
    }

    /// The entry decoded as Latin-1 text with trailing NUL bytes stripped.
    pub fn entry_text_latin1(&self, archive: &str, name: &str) -> Result<Option<String>, VfsError> {
        Ok(self.entry(archive, name)?.map(latin1_text))
    }

    /// Every `(full path, offset, size)` entry of `archive` (archives only; folders are not listed).
    pub fn list_entries(&self, archive: &str) -> Result<Vec<(String, u64, u32)>, VfsError> {
        validate_name(archive)?;
        match &self.archive {
            Some(a) => a.list(archive),
            None => Err(VfsError::ArchiveNotFound(archive.to_string())),
        }
    }

    /// `true` when an archive lookup of `name` in `archive` is cached as absent (tests and diagnostics).
    pub fn is_cached_missing(&self, archive: &str, name: &str, by_basename: bool) -> bool {
        self.missing
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&missing_key(archive, name, by_basename))
    }
}

/// The folders a program reads content from, as plain data (a content profile resolves to one): packs, override
/// folders and base content folders, each list in load order (a later entry shadows an earlier one).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Content {
    pub packs: PackTable,
    pub overrides: Vec<PathBuf>,
    pub roots: Vec<PathBuf>,
}

impl Content {
    /// No pack, no override folder, no base folder.
    pub fn is_empty(&self) -> bool {
        self.packs.is_empty() && self.overrides.is_empty() && self.roots.is_empty()
    }
}

/// The layered file system of `content` over an optional archive layer: packs, then override folders, then base
/// folders, then the archives ([crate docs](crate)). With no archive layer it reads the folders only; with an empty
/// `content` it is `Vfs::new()` / `Vfs::with_archive(archive)`.
// DC-DEFAULT: ENG-2 (proposed) the layer order pack > override > base folder > archive, see DC_DEFAULTS.md
pub fn install(content: &Content, archive: Option<Arc<dyn ArchiveLayer>>) -> Vfs {
    let mut vfs = match archive {
        Some(a) => Vfs::with_archive(a),
        None => Vfs::new(),
    };
    for p in content.packs.packs() {
        vfs.add_pack(p.clone());
    }
    for d in &content.overrides {
        vfs.add_override_dir(d.clone());
    }
    for r in &content.roots {
        vfs.add_root(r.clone());
    }
    vfs
}

#[cfg(test)]
mod tests;
