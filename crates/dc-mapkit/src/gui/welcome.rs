//! The welcome screen's rules, with no egui types: where a new map goes by default and how a name becomes a
//! folder name.
// DC-DEFAULT: MAP-63 (welcome screen: New map under Documents/DeCombat Maps/<name>, Open, Choose ROM folder)

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The folder under the user's documents that holds new maps.
pub const MAPS_FOLDER: &str = "DeCombat Maps";
/// The name a new map gets when the name field is empty.
pub const UNTITLED: &str = "untitled";

/// The user's documents folder: `DC_DOCUMENTS` (tests), else `USERPROFILE\Documents` (Windows), else
/// `$HOME/Documents`; `None` when neither variable is set.
pub fn documents_dir_with(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let var = |k: &str| env(k).filter(|d| !d.is_empty());
    if let Some(d) = var("DC_DOCUMENTS") {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = var("USERPROFILE") {
        return Some(PathBuf::from(d).join("Documents"));
    }
    var("HOME").map(|h| PathBuf::from(h).join("Documents"))
}

pub fn documents_dir() -> Option<PathBuf> {
    documents_dir_with(&|k: &str| std::env::var_os(k))
}

/// The folder name of a map name: letters, digits, space, `-`, `_` and `.` kept (ASCII; other characters become
/// `_`), trimmed, never a path separator or a leading dot, [`UNTITLED`] when nothing is left.
pub fn folder_name(name: &str) -> String {
    let mut out: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    while out.starts_with('.') {
        out.remove(0);
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        UNTITLED.to_string()
    } else {
        out
    }
}

/// `<documents>/DeCombat Maps/<folder name of the map name>`.
pub fn default_folder(documents: &Path, name: &str) -> PathBuf {
    documents.join(MAPS_FOLDER).join(folder_name(name))
}

/// Whether a folder may take a new map: it does not exist yet, or it is an empty folder.
pub fn folder_is_free(dir: &Path) -> bool {
    if !dir.exists() {
        return true;
    }
    dir.is_dir()
        && std::fs::read_dir(dir)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_folder_is_documents_decombat_maps_name() {
        let docs = PathBuf::from("/Users/me/Documents");
        assert_eq!(
            default_folder(&docs, "Crossfire Valley"),
            docs.join("DeCombat Maps").join("Crossfire Valley")
        );
        assert_eq!(folder_name("  my/map: v2?  "), "my_map_ v2_");
        assert_eq!(folder_name(""), UNTITLED);
        assert_eq!(folder_name("..."), UNTITLED);
        assert_eq!(folder_name("..hidden"), "hidden");
        assert_eq!(folder_name("a\\b"), "a_b");
        assert!(!folder_name("x..y").contains('/'));
    }

    #[test]
    fn the_documents_folder_follows_the_platform_variables() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| -> Option<OsString> {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        assert_eq!(
            documents_dir_with(&env(&[("DC_DOCUMENTS", "/tmp/docs"), ("HOME", "/h")])),
            Some(PathBuf::from("/tmp/docs"))
        );
        assert_eq!(
            documents_dir_with(&env(&[("USERPROFILE", "C:\\Users\\me")])),
            Some(PathBuf::from("C:\\Users\\me").join("Documents"))
        );
        assert_eq!(
            documents_dir_with(&env(&[("HOME", "/Users/me")])),
            Some(PathBuf::from("/Users/me/Documents"))
        );
        assert_eq!(documents_dir_with(&env(&[("HOME", "")])), None);
    }

    #[test]
    fn a_free_folder_is_missing_or_empty() {
        let base = std::env::temp_dir().join(format!("mapkit-welcome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        assert!(folder_is_free(&base.join("new")));
        std::fs::create_dir_all(base.join("empty")).unwrap();
        assert!(folder_is_free(&base.join("empty")));
        std::fs::write(base.join("empty").join("x"), b"1").unwrap();
        assert!(!folder_is_free(&base.join("empty")));
        assert!(!folder_is_free(&base.join("empty").join("x")));
        let _ = std::fs::remove_dir_all(&base);
    }
}
