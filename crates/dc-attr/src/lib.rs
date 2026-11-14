//! The attribute-text format (`.txt` / `.gms` "bute" files): sections of `key = value` lines, Latin-1 text,
//! `//` comments. [`ButeDoc`] parses it (from text, bytes, a file or a [`Vfs`]); [`modpatch`] holds the mod
//! content-patch document that overwrites rows of a weapon table written in it. No game data: the tables of the
//! 2009 game (`dc-bute`) and own content definitions both read through this crate.
// Provenance: ORIGINAL (our own parser of a plain-text format; format notes in dc-bute)

pub mod modpatch;

use dc_vfs::{Vfs, VfsError};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;

/// Errors of the loaders of [`ButeDoc`].
#[derive(Clone, Debug)]
pub enum AttrError {
    /// A file on disk could not be read.
    Io { path: String, message: String },
    /// The file system (or its archive layer) failed.
    Vfs(VfsError),
}

impl fmt::Display for AttrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttrError::Io { path, message } => write!(f, "failed to read {path}: {message}"),
            AttrError::Vfs(e) => write!(f, "archive error: {e}"),
        }
    }
}

impl std::error::Error for AttrError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AttrError::Vfs(e) => Some(e),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

/// Latin-1-aware case folding of one character: `A-Z` and `0xC0-0xDE` except `0xD7` (the multiplication
/// sign) map to lowercase; everything else is unchanged. The decoded ROM text is Latin-1, so plain ASCII
/// folding would miss accented letters.
pub fn fold_latin1_char(c: char) -> char {
    match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        '\u{C0}'..='\u{DE}' if c != '\u{D7}' => char::from_u32(c as u32 + 0x20).unwrap_or(c),
        _ => c,
    }
}

/// Folds a whole string with [`fold_latin1_char`].
pub fn fold_latin1(s: &str) -> String {
    s.chars().map(fold_latin1_char).collect()
}

/// Case-insensitive equality under [`fold_latin1_char`].
pub fn eq_fold(a: &str, b: &str) -> bool {
    let mut x = a.chars();
    let mut y = b.chars();
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(p), Some(q)) if fold_latin1_char(p) == fold_latin1_char(q) => {}
            _ => return false,
        }
    }
}

/// Splits on `\r\n`, `\n` and lone `\r`.
fn split_lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push(&text[start..i]);
                i += 1;
                start = i;
            }
            b'\r' => {
                out.push(&text[start..i]);
                i += 1;
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        out.push(&text[start..]);
    }
    out
}

/// Number text rule used for room-line values: trim whitespace, strip ONE trailing `f`/`F` (a float
/// suffix such as `1.5f`), then parse. `"1.5 f"` therefore does not parse (the space survives the strip).
pub fn num_text(v: &str) -> &str {
    let t = v.trim();
    t.strip_suffix(|c| c == 'f' || c == 'F').unwrap_or(t)
}

// ---------------------------------------------------------------------------
// SectionMap: order-preserving key-value map
// ---------------------------------------------------------------------------

/// Key-value map for a single `[Section]`. Keys keep first-insertion order, so iteration is
/// deterministic; inserting an existing key replaces the value in place (last value wins).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SectionMap {
    entries: Vec<(String, String)>,
    index: HashMap<String, usize>,
}

impl SectionMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: String, value: String) {
        if let Some(&i) = self.index.get(&key) {
            self.entries[i].1 = value;
        } else {
            self.index.insert(key.clone(), self.entries.len());
            self.entries.push((key, value));
        }
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.index.get(key).map(|&i| &self.entries[i].1)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterates `(key, value)` in first-insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

// ---------------------------------------------------------------------------
// Bute `.txt` / `.gms` Parser (`ButeDoc`)
// ---------------------------------------------------------------------------

/// Alias for `ButeDoc`.
pub type AttrMap = ButeDoc;

/// Parse options for [`ButeDoc::parse_with`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParseOptions {
    /// Trim whitespace inside the brackets: `[ Foo ]` becomes section `Foo` instead of ` Foo `.
    /// Default `false` (names are kept verbatim); the GMS loader sets it to `true`.
    pub strip_section_names: bool,
}

/// Parsed Bute `.txt` or `.gms` document preserving section insertion order and section key-value pairs.
///
/// Syntax handled: lines end with `\r\n`, `\n` or a lone `\r`; everything after `//` is a comment (so a
/// value cannot contain `//`); a line `[Name]` opens a section; `key = value` lines before any section are
/// dropped; values are trimmed and have surrounding `"` removed; a repeated key keeps the last value.
#[derive(Clone, Debug, Default)]
pub struct ButeDoc {
    pub sections: HashMap<String, SectionMap>,
    pub order: Vec<String>,
}

impl ButeDoc {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses decoded text with default options.
    pub fn parse(text: &str) -> Self {
        Self::parse_with(text, ParseOptions::default())
    }

    /// Parses decoded text into sections and key-value pairs.
    pub fn parse_with(text: &str, opts: ParseOptions) -> Self {
        let mut sections: HashMap<String, SectionMap> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        let mut cur_sec: Option<String> = None;

        for raw_line in split_lines(text) {
            let without_comment = raw_line.split("//").next().unwrap_or("");
            let line = without_comment.trim();
            if line.is_empty() {
                continue;
            }
            if line.len() >= 2 && line.starts_with('[') && line.ends_with(']') {
                let inner = &line[1..line.len() - 1];
                let sec_name = if opts.strip_section_names {
                    inner.trim()
                } else {
                    inner
                }
                .to_string();
                if !sections.contains_key(&sec_name) {
                    order.push(sec_name.clone());
                    sections.insert(sec_name.clone(), SectionMap::new());
                }
                cur_sec = Some(sec_name);
            } else if let Some((k, v)) = line.split_once('=') {
                if let Some(ref sec_name) = cur_sec {
                    let key = k.trim().to_string();
                    let val = v.trim().trim_matches('"').to_string();
                    if let Some(sec_map) = sections.get_mut(sec_name) {
                        sec_map.insert(key, val);
                    }
                }
            }
        }

        Self { sections, order }
    }

    /// Decodes `bytes` as Latin-1 (`ISO-8859-1`, stripping trailing NULs) and parses with default options.
    pub fn parse_bytes_latin1(bytes: &[u8]) -> Self {
        Self::parse_bytes_latin1_with(bytes, ParseOptions::default())
    }

    pub fn parse_bytes_latin1_with(bytes: &[u8], opts: ParseOptions) -> Self {
        let mut end = bytes.len();
        while end > 0 && bytes[end - 1] == 0 {
            end -= 1;
        }
        let text: String = bytes[..end].iter().map(|&b| b as char).collect();
        Self::parse_with(&text, opts)
    }

    /// Loads and parses a Bute file from disk using Latin-1 encoding.
    pub fn from_file(path: &Path) -> Result<Self, AttrError> {
        let bytes = fs::read(path).map_err(|e| AttrError::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        Ok(Self::parse_bytes_latin1(&bytes))
    }

    /// Loads and parses a Bute entry from a [`Vfs`] with default options. Returns `None` for a missing
    /// entry AND for an archive error; use [`ButeDoc::try_from_vfs`] to tell them apart.
    pub fn from_vfs(vfs: &Vfs, rez_name: &str, name: &str) -> Option<Self> {
        Self::try_from_vfs(vfs, rez_name, name).ok().flatten()
    }

    /// Like [`ButeDoc::from_vfs`], but archive errors are reported: `Ok(None)` means the entry does not
    /// exist, `Err(AttrError::Vfs(..))` means the file system or its archive layer failed.
    pub fn try_from_vfs(vfs: &Vfs, rez_name: &str, name: &str) -> Result<Option<Self>, AttrError> {
        Self::try_from_vfs_with(vfs, rez_name, name, ParseOptions::default())
    }

    pub fn from_vfs_with(
        vfs: &Vfs,
        rez_name: &str,
        name: &str,
        opts: ParseOptions,
    ) -> Option<Self> {
        Self::try_from_vfs_with(vfs, rez_name, name, opts)
            .ok()
            .flatten()
    }

    /// Like [`ButeDoc::from_vfs_with`], but archive errors are reported.
    pub fn try_from_vfs_with(
        vfs: &Vfs,
        rez_name: &str,
        name: &str,
        opts: ParseOptions,
    ) -> Result<Option<Self>, AttrError> {
        let text = vfs
            .entry_text_latin1(rez_name, name)
            .map_err(AttrError::Vfs)?;
        Ok(text.map(|t| Self::parse_with(&t, opts)))
    }

    pub fn get(&self, section: &str) -> Option<&SectionMap> {
        self.sections.get(section)
    }

    pub fn contains_key(&self, section: &str) -> bool {
        self.sections.contains_key(section)
    }

    pub fn len(&self) -> usize {
        self.sections.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    pub fn get_str(&self, section: &str, key: &str) -> Option<&str> {
        self.sections.get(section)?.get(key).map(|s| s.as_str())
    }

    pub fn get_int(&self, section: &str, key: &str, default: i32) -> i32 {
        self.get_str(section, key)
            .and_then(|s| s.trim().trim_matches('"').parse::<i32>().ok())
            .unwrap_or(default)
    }

    /// Float lookup using the room-line number rule (trim, strip one trailing `f`/`F`, parse; non-finite
    /// results are rejected).
    pub fn get_f64(&self, section: &str, key: &str, default: f64) -> f64 {
        self.get_str(section, key)
            .map(|s| s.trim_matches('"'))
            .and_then(|s| num_text(s).parse::<f64>().ok())
            .filter(|x| x.is_finite())
            .unwrap_or(default)
    }

    /// Iterates `(section_name, section_map)` in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &SectionMap)> {
        self.order
            .iter()
            .filter_map(|name| self.sections.get(name).map(|m| (name.as_str(), m)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vfs_without_archives_reads_a_table_from_a_base_folder() {
        let dir = std::env::temp_dir().join(format!("dc_attr_root_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("attributes")).unwrap();
        std::fs::write(
            dir.join("attributes/weapons.txt"),
            "[Weapon0]\nName = \"Own\"\n",
        )
        .unwrap();
        let mut vfs = Vfs::new();
        vfs.add_root(&dir);
        let doc = ButeDoc::try_from_vfs(&vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\WEAPONS.TXT")
            .unwrap()
            .unwrap();
        assert_eq!(doc.get_str("Weapon0", "Name"), Some("Own"));
        assert!(ButeDoc::try_from_vfs(&vfs, "ATTRIBUTES.rez", "X.TXT")
            .unwrap()
            .is_none());
        assert!(matches!(
            ButeDoc::try_from_vfs(&vfs, "ATTRIBUTES.rez", "../X.TXT"),
            Err(AttrError::Vfs(_))
        ));
        let e = ButeDoc::from_file(&dir.join("missing.txt")).unwrap_err();
        assert!(e.to_string().starts_with("failed to read "));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn latin1_folding_and_the_number_rule() {
        assert!(eq_fold("\u{C9}T\u{C9}", "\u{E9}t\u{E9}"));
        assert!(!eq_fold("\u{D7}", "\u{F7}"));
        assert_eq!(num_text(" 1.5f "), "1.5");
        assert_eq!(split_lines("a\r\nb\rc\nd"), vec!["a", "b", "c", "d"]);
    }
}
