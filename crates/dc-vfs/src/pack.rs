//! Packs: mod folders addressed by id. A pack table answers `@mod:<id>/assets/<rel>` names with the file
//! `<pack folder>/assets/<rel>` and lists the packs' `overrides/` folders, which the [`crate::Vfs`] consults before its
//! own override folders.
// Provenance: ORIGINAL (DeCombat's own mod packaging; the 2009 client reads its archives only)
// DC-DEFAULT: ENG-2 (proposed) packs and `@mod:` names in the layered file system, see DC_DEFAULTS.md

use std::path::{Component, Path, PathBuf};

/// The scheme of a pack asset path.
pub const MOD_SCHEME: &str = "@mod:";

/// One pack: its id and its unpacked folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub id: String,
    pub dir: PathBuf,
}

impl Pack {
    pub fn new(id: impl Into<String>, dir: impl Into<PathBuf>) -> Pack {
        Pack {
            id: id.into(),
            dir: dir.into(),
        }
    }
}

impl From<(String, PathBuf)> for Pack {
    fn from((id, dir): (String, PathBuf)) -> Pack {
        Pack { id, dir }
    }
}

/// Packs in load order (a later pack's `overrides/` shadows an earlier one's).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackTable {
    packs: Vec<Pack>,
}

/// True for a `@mod:` path.
pub fn is_mod_path(path: &str) -> bool {
    path.starts_with(MOD_SCHEME)
}

/// A pack-relative asset path, cleaned: trimmed, `\` folded to `/`, `.` parts dropped. Refused: empty, `..`
/// anywhere, absolute, drive letters or any `:`, NUL. The same rule the mod runtime applies to `mod.assets:path`.
pub fn clean_asset_path(rel_path: &str) -> Result<String, String> {
    let trimmed = rel_path.trim();
    if trimmed.is_empty() {
        return Err("Asset path cannot be empty".to_string());
    }
    if trimmed.contains("..")
        || trimmed.starts_with('/')
        || trimmed.starts_with('\\')
        || trimmed.contains(':')
        || trimmed.contains('\0')
    {
        return Err(format!(
            "Invalid asset path '{rel_path}': path traversal or absolute path is forbidden"
        ));
    }
    let normalized = trimmed.replace('\\', "/");
    let mut clean_parts = Vec::new();
    for comp in Path::new(&normalized).components() {
        match comp {
            Component::Normal(os) => {
                let s = os.to_string_lossy();
                if s == ".." || s.is_empty() {
                    return Err(format!(
                        "Invalid asset path '{rel_path}': path traversal is forbidden"
                    ));
                }
                clean_parts.push(s.into_owned());
            }
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "Invalid asset path '{rel_path}': path traversal or root component is forbidden"
                ))
            }
        }
    }
    if clean_parts.is_empty() {
        return Err(format!("Invalid asset path '{rel_path}'"));
    }
    Ok(clean_parts.join("/"))
}

impl PackTable {
    /// The table of `packs`, in load order.
    pub fn new(packs: impl IntoIterator<Item = Pack>) -> PackTable {
        PackTable {
            packs: packs.into_iter().collect(),
        }
    }

    /// Adds a pack after the others (its `overrides/` shadows theirs).
    pub fn push(&mut self, pack: Pack) {
        self.packs.push(pack);
    }

    pub fn is_empty(&self) -> bool {
        self.packs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.packs.len()
    }

    pub fn packs(&self) -> &[Pack] {
        &self.packs
    }

    /// The pack of id `id` (the first one when two share it).
    pub fn get(&self, id: &str) -> Option<&Pack> {
        self.packs.iter().find(|p| p.id == id)
    }

    /// The `overrides/` folders that exist now, in load order (the last one shadows the others). Checked on each
    /// call: a pack may gain the folder at run time.
    pub fn override_dirs(&self) -> Vec<PathBuf> {
        self.packs
            .iter()
            .map(|p| p.dir.join("overrides"))
            .filter(|d| d.is_dir())
            .collect()
    }

    /// The file `@mod:<id>/assets/<rel>` names. Refused: another scheme, an id not in the table, a path outside
    /// `assets/`, `..`, absolute paths and drive letters, a file that resolves outside the pack's folder. The file
    /// need not exist.
    pub fn resolve(&self, uri: &str) -> Result<PathBuf, String> {
        let rest = uri
            .strip_prefix(MOD_SCHEME)
            .ok_or_else(|| format!("'{uri}' is not a {MOD_SCHEME}<id>/assets/<file> path"))?;
        let (id, rel) = rest
            .split_once('/')
            .ok_or_else(|| format!("'{uri}' names no file after the mod id"))?;
        let root = self.get(id).map(|p| &p.dir).ok_or_else(|| {
            format!("'{uri}' names mod '{id}', which is not in the loaded mod set")
        })?;
        let clean = clean_asset_path(rel)?;
        if !clean.starts_with("assets/") || clean.len() == "assets/".len() {
            return Err(format!(
                "'{uri}': a mod asset lives under assets/ (mod.assets:path(\"assets/<file>\"))"
            ));
        }
        let path = root.join(&clean);
        // a link inside the folder must not lead out of it
        if let (Ok(real), Ok(base)) = (path.canonicalize(), root.canonicalize()) {
            if !real.starts_with(&base) {
                return Err(format!("'{uri}' resolves outside the folder of mod '{id}'"));
            }
        }
        Ok(path)
    }
}
