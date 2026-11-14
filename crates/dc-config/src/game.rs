//! Content profiles: run the engine with your own content and no original game folder.
//!
//! A content profile is a folder holding [`PROFILE_FILE`] (`game.json`), a small JSON document:
//!
//! ```json
//! {
//!   "format": 1,
//!   "id": "my-game",
//!   "title": "My Game",
//!   "roots": ["base"],
//!   "packs": ["packs/arena"],
//!   "default_map": "@mod:arena/assets/maps/arena.dat"
//! }
//! ```
//!
//! - `format` (required): [`FORMAT`]; any other number is refused, so a newer file is never half read.
//! - `id` (required): 1-64 of `A-Z a-z 0-9 _ -`.
//! - `title` (optional): shown to players; the id when absent.
//! - `roots` (optional): base content folders, read by exact lower-case relative path; a later one shadows an
//!   earlier one.
//! - `packs` (optional): mod folders (each holds a `manifest.json` with its `id`); they answer
//!   `@mod:<id>/assets/<file>` names, and their `overrides/` folders shadow everything else, a later pack first.
//! - `default_map` (optional): the `@mod:` path of the map a match uses when nothing else names one; it must name
//!   a file of one of the profile's packs.
//!
//! Relative folders are relative to the profile's folder. Unknown fields are refused. [`GameProfile::content`]
//! turns the profile into the plain data [`dc_vfs::install`] layers into a file system.
// Provenance: ORIGINAL (DeCombat's own profile format; no 2009 behaviour)
// DC-DEFAULT: ENG-1 (proposed) the content profile document, see DC_DEFAULTS.md

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

/// The profile document's file name inside a profile folder.
pub const PROFILE_FILE: &str = "game.json";
/// The one `format` this build reads.
pub const FORMAT: u32 = 1;
/// Longest accepted profile id.
pub const MAX_ID_LEN: usize = 64;
/// Largest accepted profile document (it names folders; anything bigger is not a profile).
pub const MAX_PROFILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDoc {
    format: u32,
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    packs: Vec<String>,
    #[serde(default)]
    default_map: Option<String>,
}

/// A loaded and checked content profile: every folder resolved and present, every pack's id read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProfile {
    /// The `game.json` read.
    pub file: PathBuf,
    /// The folder relative paths were resolved against (the file's folder).
    pub dir: PathBuf,
    pub id: String,
    pub title: String,
    /// Base content folders, in load order.
    pub roots: Vec<PathBuf>,
    /// Packs in load order, ids from their manifests.
    pub packs: dc_vfs::PackTable,
    /// `@mod:<id>/assets/<file>`, checked against the packs.
    pub default_map: Option<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn folder(dir: &Path, field: &str, raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err(format!("{field}: an empty folder name"));
    }
    let p = dir.join(raw);
    if !p.is_dir() {
        return Err(format!("{field}: {} is not a folder", p.display()));
    }
    Ok(p)
}

/// The `id` of the pack folder `dir` (its `manifest.json`).
fn pack_id(dir: &Path) -> Result<String, String> {
    let m = dir.join("manifest.json");
    let text = std::fs::read_to_string(&m)
        .map_err(|e| format!("packs: {}: {e} (a pack is a mod folder)", m.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("packs: {}: {e}", m.display()))?;
    let id = v
        .get("id")
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("packs: {}: no string \"id\"", m.display()))?;
    if !valid_id(id) {
        return Err(format!(
            "packs: {}: id {id:?} is not 1-{MAX_ID_LEN} of A-Z a-z 0-9 _ -",
            m.display()
        ));
    }
    Ok(id.to_string())
}

impl GameProfile {
    /// Loads the profile at `path`: a profile folder (its [`PROFILE_FILE`] is read) or the document itself.
    pub fn load(path: &Path) -> Result<GameProfile, String> {
        let file = if path.is_dir() {
            path.join(PROFILE_FILE)
        } else {
            path.to_path_buf()
        };
        let len = std::fs::metadata(&file)
            .map_err(|e| format!("content profile {}: {e}", file.display()))?
            .len();
        if len > MAX_PROFILE_BYTES {
            return Err(format!(
                "content profile {}: {len} bytes, more than {MAX_PROFILE_BYTES}",
                file.display()
            ));
        }
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("content profile {}: {e}", file.display()))?;
        let dir = match file.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let mut p = GameProfile::parse(&text, &dir)
            .map_err(|e| format!("content profile {}: {e}", file.display()))?;
        p.file = file;
        Ok(p)
    }

    /// Parses and checks a profile document whose relative folders are relative to `dir` (the folders must
    /// exist; packs must carry a manifest with a unique id).
    pub fn parse(text: &str, dir: &Path) -> Result<GameProfile, String> {
        let doc: ProfileDoc = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if doc.format != FORMAT {
            return Err(format!(
                "format {} is not supported (this build reads format {FORMAT})",
                doc.format
            ));
        }
        if !valid_id(&doc.id) {
            return Err(format!(
                "id {:?} is not 1-{MAX_ID_LEN} of A-Z a-z 0-9 _ -",
                doc.id
            ));
        }
        let title = match doc.title {
            Some(t) if t.trim().is_empty() => return Err("title: empty".into()),
            Some(t) => t,
            None => doc.id.clone(),
        };
        let roots = doc
            .roots
            .iter()
            .map(|r| folder(dir, "roots", r))
            .collect::<Result<Vec<_>, _>>()?;
        let mut packs = dc_vfs::PackTable::default();
        for raw in &doc.packs {
            let d = folder(dir, "packs", raw)?;
            let id = pack_id(&d)?;
            if packs.get(&id).is_some() {
                return Err(format!("packs: two packs with id {id:?}"));
            }
            packs.push(dc_vfs::Pack::new(id, d));
        }
        if let Some(m) = &doc.default_map {
            let f = packs.resolve(m).map_err(|e| format!("default_map: {e}"))?;
            if !f.is_file() {
                return Err(format!("default_map: {} does not exist", f.display()));
            }
        }
        Ok(GameProfile {
            file: dir.join(PROFILE_FILE),
            dir: dir.to_path_buf(),
            id: doc.id,
            title,
            roots,
            packs,
            default_map: doc.default_map,
        })
    }

    /// The profile's folders as the data [`dc_vfs::install`] takes (no override folders: a program adds its own).
    pub fn content(&self) -> dc_vfs::Content {
        dc_vfs::Content {
            packs: self.packs.clone(),
            overrides: Vec::new(),
            roots: self.roots.clone(),
        }
    }

    /// The layered file system of this profile over `archive` (none: the profile's folders only).
    pub fn vfs(&self, archive: Option<Arc<dyn dc_vfs::ArchiveLayer>>) -> dc_vfs::Vfs {
        dc_vfs::install(&self.content(), archive)
    }

    /// One start-up line: `game: <id>, <n> roots, <m> packs`.
    pub fn summary(&self) -> String {
        format!(
            "game: {}, {} roots, {} packs",
            self.id,
            self.roots.len(),
            self.packs.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc_config_game_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    /// A profile folder with one root and one pack `arena` holding a map.
    fn sample(tag: &str) -> PathBuf {
        let d = temp(tag);
        put(&d, "base/models/box.mdl", b"model");
        put(
            &d,
            "packs/arena/manifest.json",
            br#"{"id":"arena","name":"Arena"}"#,
        );
        put(&d, "packs/arena/assets/maps/arena.dat", b"world");
        d
    }

    const GOOD: &str = r#"{"format":1,"id":"my-game","title":"My Game","roots":["base"],
        "packs":["packs/arena"],"default_map":"@mod:arena/assets/maps/arena.dat"}"#;

    #[test]
    fn a_profile_loads_from_its_folder_or_its_file() {
        let d = sample("load");
        put(&d, PROFILE_FILE, GOOD.as_bytes());
        let p = GameProfile::load(&d).unwrap();
        assert_eq!(p.id, "my-game");
        assert_eq!(p.title, "My Game");
        assert_eq!(p.roots, vec![d.join("base")]);
        assert_eq!(p.packs.len(), 1);
        assert_eq!(p.packs.packs()[0].id, "arena");
        assert_eq!(p.packs.packs()[0].dir, d.join("packs/arena"));
        assert_eq!(p.file, d.join(PROFILE_FILE));
        assert_eq!(p.summary(), "game: my-game, 1 roots, 1 packs");
        assert_eq!(GameProfile::load(&d.join(PROFILE_FILE)).unwrap(), p);
        // the profile's file system: no archive, root and pack readable
        let v = p.vfs(None);
        assert!(!v.has_archive());
        assert_eq!(
            v.entry("any.rez", "models/box.mdl").unwrap(),
            Some(b"model".to_vec())
        );
        assert_eq!(
            v.entry("any.rez", p.default_map.as_deref().unwrap())
                .unwrap(),
            Some(b"world".to_vec())
        );
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn a_minimal_profile_needs_only_format_and_id() {
        let d = temp("min");
        let p = GameProfile::parse(r#"{"format":1,"id":"x"}"#, &d).unwrap();
        assert_eq!(p.title, "x");
        assert!(p.roots.is_empty() && p.packs.is_empty() && p.default_map.is_none());
        assert!(p.content().is_empty());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn bad_profiles_are_refused() {
        let d = sample("bad");
        put(&d, "packs/twin/manifest.json", br#"{"id":"arena"}"#);
        put(&d, "packs/noid/manifest.json", br#"{"name":"x"}"#);
        put(&d, "packs/badid/manifest.json", br#"{"id":"a b"}"#);
        fs::create_dir_all(d.join("packs/nomanifest")).unwrap();
        let cases: &[(&str, &str)] = &[
            ("not json", "expected"),
            (r#"{"id":"x"}"#, "format"),
            (r#"{"format":2,"id":"x"}"#, "format 2"),
            (r#"{"format":1}"#, "id"),
            (r#"{"format":1,"id":""}"#, "id"),
            (r#"{"format":1,"id":"a/b"}"#, "id"),
            (r#"{"format":1,"id":"x","extra":1}"#, "unknown field"),
            (r#"{"format":1,"id":"x","title":" "}"#, "title"),
            (r#"{"format":1,"id":"x","roots":["missing"]}"#, "roots"),
            (r#"{"format":1,"id":"x","roots":[""]}"#, "roots"),
            (
                r#"{"format":1,"id":"x","packs":["packs/nomanifest"]}"#,
                "manifest",
            ),
            (r#"{"format":1,"id":"x","packs":["packs/noid"]}"#, "id"),
            (r#"{"format":1,"id":"x","packs":["packs/badid"]}"#, "id"),
            (
                r#"{"format":1,"id":"x","packs":["packs/arena","packs/twin"]}"#,
                "two packs",
            ),
            (
                r#"{"format":1,"id":"x","default_map":"@mod:arena/assets/maps/arena.dat"}"#,
                "default_map",
            ),
            (
                r#"{"format":1,"id":"x","packs":["packs/arena"],"default_map":"@mod:arena/assets/none.dat"}"#,
                "default_map",
            ),
            (
                r#"{"format":1,"id":"x","packs":["packs/arena"],"default_map":"maps/arena.dat"}"#,
                "default_map",
            ),
        ];
        for (text, needle) in cases {
            let e = GameProfile::parse(text, &d).expect_err(text);
            assert!(e.contains(needle), "{text}: {e}");
        }
        // a missing profile and an oversized one
        assert!(GameProfile::load(&d.join("nowhere")).is_err());
        let big = d.join("big.json");
        fs::write(&big, vec![b' '; MAX_PROFILE_BYTES as usize + 1]).unwrap();
        assert!(GameProfile::load(&big).unwrap_err().contains("bytes"));
        let _ = fs::remove_dir_all(d);
    }
}
