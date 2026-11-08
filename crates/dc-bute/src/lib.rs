//! Combat Arms Bute (`.txt` / `.gms`) parser and structured game tables (`GameRoomData`, `ShopCatalog`,
//! `ItemsTable`, `WeaponsTable`).
//!
//! Format knowledge comes from the user's 2009 ROM files (`ATTRIBUTES.rez`, `GMS.rez`, `WORLDS.rez`, read at
//! run time, never embedded) and from the room-resolution suites in our own harness
//! (HARNESS: lobby_harness::test_world_for_room). The item-type to equip-slot mapping is
//! read off the 2009 CShell (EVIDENCE: cshell+0x259f00). Rules the 2009 client does not show (clan room
//! policy, shop flags) are DeCombat defaults (DC-DEFAULT: CD-09, CD-11) and live in policy structs.
//!
//! One external dependency (`serde_json`, for the mod content-patch document of [`modpatch`]); the library never
//! reads environment variables.

pub mod arms;
pub mod camo_crate;
pub mod modpatch;
pub mod seasonal;

pub use dc_rez::{self, dc_rom, RezVfs};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by the loaders in this crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ButeError {
    /// A file on disk could not be read.
    Io { path: String, message: String },
    /// A named entry was not found in a `.rez` archive.
    MissingEntry { rez: String, name: String },
    /// The archive layer failed (unreadable or malformed `.rez`, missing archive, bad key).
    Rez(dc_rez::RezError),
}

impl fmt::Display for ButeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ButeError::Io { path, message } => write!(f, "failed to read {path}: {message}"),
            ButeError::MissingEntry { rez, name } => write!(f, "failed to load {name} from {rez}"),
            ButeError::Rez(e) => write!(f, "archive error: {e}"),
        }
    }
}

impl std::error::Error for ButeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ButeError::Rez(e) => Some(e),
            _ => None,
        }
    }
}

impl From<dc_rez::RezError> for ButeError {
    fn from(e: dc_rez::RezError) -> Self {
        ButeError::Rez(e)
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
fn num_text(v: &str) -> &str {
    let t = v.trim();
    t.strip_suffix(|c| c == 'f' || c == 'F').unwrap_or(t)
}

fn num_i32(d: Option<&SectionMap>, k: &str, dflt: i32) -> i32 {
    d.and_then(|m| m.get(k))
        .and_then(|v| num_text(v).parse::<i32>().ok())
        .unwrap_or(dflt)
}

/// Non-finite results (`nan`, `inf`) are rejected and fall back to the default.
fn num_f64(d: Option<&SectionMap>, k: &str, dflt: f64) -> f64 {
    d.and_then(|m| m.get(k))
        .and_then(|v| num_text(v).parse::<f64>().ok())
        .filter(|x| x.is_finite())
        .unwrap_or(dflt)
}

fn float_text(v: f64) -> String {
    let s = format!("{}", v);
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{}.0", s)
    }
}

/// Plain integer parse (trim only).
fn plain_i32(s: &str) -> Option<i32> {
    s.trim().parse::<i32>().ok()
}

/// `ItemID` rule for use items: non-empty and ASCII digits only (no sign, no spaces).
fn digits_i32(s: &str) -> Option<i32> {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse::<i32>().ok()
    } else {
        None
    }
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
    pub fn from_file(path: &Path) -> Result<Self, ButeError> {
        let bytes = fs::read(path).map_err(|e| ButeError::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        Ok(Self::parse_bytes_latin1(&bytes))
    }

    /// Loads and parses a Bute entry from `<rom_dir>/Game/<rez_name>`.
    pub fn from_rez(rom_dir: &Path, rez_name: &str, name: &str) -> Option<Self> {
        let vfs = RezVfs::new(rom_dir);
        Self::from_vfs(&vfs, rez_name, name)
    }

    /// Loads and parses a Bute entry from a `RezVfs` with default options. Returns `None` for a missing
    /// entry AND for an archive error; use [`ButeDoc::try_from_vfs`] to tell them apart.
    pub fn from_vfs(vfs: &RezVfs, rez_name: &str, name: &str) -> Option<Self> {
        Self::try_from_vfs(vfs, rez_name, name).ok().flatten()
    }

    /// Like [`ButeDoc::from_vfs`], but archive errors are reported: `Ok(None)` means the entry does not
    /// exist, `Err(ButeError::Rez(..))` means the archive layer failed.
    pub fn try_from_vfs(
        vfs: &RezVfs,
        rez_name: &str,
        name: &str,
    ) -> Result<Option<Self>, ButeError> {
        Self::try_from_vfs_with(vfs, rez_name, name, ParseOptions::default())
    }

    pub fn from_vfs_with(
        vfs: &RezVfs,
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
        vfs: &RezVfs,
        rez_name: &str,
        name: &str,
        opts: ParseOptions,
    ) -> Result<Option<Self>, ButeError> {
        let text = vfs.entry_text_latin1(rez_name, name)?;
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

// ---------------------------------------------------------------------------
// GameRoomData (`GAMEROOM.TXT`) & World / GMS / Room Line Helpers
// ---------------------------------------------------------------------------

/// Structured wrapper around `GAMEROOM.TXT`.
#[derive(Clone, Debug)]
pub struct GameRoomData {
    doc: ButeDoc,
}

impl GameRoomData {
    pub fn from_doc(doc: ButeDoc) -> Self {
        Self { doc }
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, ButeError> {
        let doc = ButeDoc::try_from_vfs(vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\GAMEROOM.TXT")?
            .ok_or_else(|| ButeError::MissingEntry {
                rez: "ATTRIBUTES.rez".to_string(),
                name: "GAMEROOM.TXT".to_string(),
            })?;
        Ok(Self { doc })
    }

    pub fn from_rom(rom_dir: &Path) -> Result<Self, ButeError> {
        let vfs = RezVfs::new(rom_dir);
        Self::from_vfs(&vfs)
    }

    pub fn sec(&self) -> &ButeDoc {
        &self.doc
    }

    /// Returns `[Mode0..n]` (when `clan == false`) or `[ClanMode0..n]` (when `clan == true`).
    pub fn modes(&self, clan: bool) -> Vec<SectionMap> {
        let prefix = if clan { "ClanMode" } else { "Mode" };
        let mut out = Vec::new();
        let mut i = 0usize;
        while let Some(sec) = self.doc.get(&format!("{}{}", prefix, i)) {
            out.push(sec.clone());
            i += 1;
        }
        out
    }

    /// Returns `[Map0..n]`.
    pub fn maps(&self) -> Vec<SectionMap> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while let Some(sec) = self.doc.get(&format!("Map{}", i)) {
            out.push(sec.clone());
            i += 1;
        }
        out
    }

    pub fn default_map(&self, mode: usize, clan: bool) -> i32 {
        let tag = format!("{}{}", if clan { "ClanMode" } else { "Mode" }, mode);
        self.doc.get_int(&tag, "DefaultMapID", 0)
    }

    pub fn default_goal(&self, mode: usize, clan: bool) -> i32 {
        let tag = format!("{}{}", if clan { "ClanMode" } else { "Mode" }, mode);
        self.doc.get_int(&tag, "DefaultGoal", 100)
    }

    /// An integer key of mode row `mode` (`None`: no row, no key or no number).
    pub fn mode_int(&self, mode: usize, clan: bool, key: &str) -> Option<i32> {
        let tag = format!("{}{}", if clan { "ClanMode" } else { "Mode" }, mode);
        self.doc.get_str(&tag, key)?.trim().parse().ok()
    }

    /// A `TRUE` / `FALSE` key of mode row `mode` (`None`: no row or no key; any other text is `false`).
    // EVIDENCE: cshell+0x265780 (the mode row's flags +0x168 .. +0x16b are read as booleans)
    pub fn mode_flag(&self, mode: usize, clan: bool, key: &str) -> Option<bool> {
        let tag = format!("{}{}", if clan { "ClanMode" } else { "Mode" }, mode);
        let v = self.doc.get_str(&tag, key)?.trim().trim_matches('"').trim();
        Some(eq_fold(v, "TRUE") || v == "1")
    }

    /// Returns `[(map_id, mode_bit_mask)]` (at most 30 pairs). Only the first 32 modes get a mask bit.
    pub fn map_mode_masks(&self, clan: bool) -> Vec<(usize, u32)> {
        let modes = self.modes(clan);
        let maps = self.maps();
        let mut res = Vec::new();

        for (mi, m) in maps.iter().enumerate() {
            let enable = m.get("Enable").map(|s| s.as_str()).unwrap_or("TRUE");
            if !eq_fold(enable, "TRUE") {
                continue;
            }
            let mut mask = 0u32;
            for (i, md) in modes.iter().enumerate().take(32) {
                let default_ep = if clan { "FALSE" } else { "TRUE" };
                let ep = md
                    .get("EnablePlay")
                    .map(|s| s.as_str())
                    .unwrap_or(default_ep);
                let key = md.get("Key").map(|s| s.as_str()).unwrap_or("?");
                let gms_key = format!("GMS_{}", key);
                if eq_fold(ep, "TRUE") && m.contains_key(&gms_key) {
                    mask |= 1u32 << i;
                }
            }
            if mask != 0 {
                res.push((mi, mask));
            }
        }

        if res.is_empty() && !clan {
            res = (0..12).map(|i| (i, 0x1ff)).collect();
        }
        res.truncate(30);
        res
    }
}

/// Lists world names (`WORLDS\<NAME>.DAT` -> `<NAME>`) from `Game/WORLDS.rez`.
///
/// A bare `WORLDS\.DAT` entry (empty name) is deliberately NOT listed: it cannot name a world, and returning
/// an empty name would make `world_for_room` build the path `Worlds\`.
pub fn world_names(rom_dir: &Path) -> Vec<String> {
    let rez_path = rom_dir.join("Game").join("WORLDS.rez");
    let entries = match dc_rez::walk_rez(&rez_path) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    entries
        .into_iter()
        .filter_map(|(n, _p, _s)| world_name_from_entry(&n).map(str::to_string))
        .collect()
}

/// `WORLDS\<NAME>.DAT` -> `<NAME>` (prefix and suffix matched ASCII case-insensitively, name non-empty).
fn world_name_from_entry(n: &str) -> Option<&str> {
    const PRE: &str = "WORLDS\\";
    const SUF: &str = ".DAT";
    let b = n.as_bytes();
    if b.len() <= PRE.len() + SUF.len() {
        return None;
    }
    if !b[..PRE.len()].eq_ignore_ascii_case(PRE.as_bytes())
        || !b[b.len() - SUF.len()..].eq_ignore_ascii_case(SUF.as_bytes())
    {
        return None;
    }
    // Both cut points sit next to ASCII bytes, so they are char boundaries.
    Some(&n[PRE.len()..n.len() - SUF.len()])
}

/// Returns the Fireteam refusal message when `!clan && mode == 2`, or `None` when allowed.
pub fn fireteam_refusal(clan: bool, mode: usize) -> Option<&'static str> {
    if !clan && mode == 2 {
        Some("Fireteam is unsupported in this build. Please choose another game mode.")
    } else {
        None
    }
}

/// Parse options for GMS documents: section names are whitespace-stripped.
const GMS_PARSE: ParseOptions = ParseOptions {
    strip_section_names: true,
};

/// Loads the decrypted `.GMS` file for `(map, mode, clan)` from `Game/GMS.rez`.
pub fn gms_file_with_vfs(
    vfs: &RezVfs,
    game_room: &GameRoomData,
    map: usize,
    mode: usize,
    clan: bool,
) -> (SectionMap, ButeDoc) {
    let maps = game_room.maps();
    let modes = game_room.modes(clan);

    let md: SectionMap = modes.get(mode).cloned().unwrap_or_else(|| {
        let mut fallback = SectionMap::new();
        fallback.insert(
            "Key".to_string(),
            if clan { "CDM" } else { "TDM" }.to_string(),
        );
        fallback
    });
    let mp: SectionMap = maps.get(map).cloned().unwrap_or_default();
    let mode_key = md.get("Key").map(|s| s.as_str()).unwrap_or("TDM");
    let gms_ref = mp
        .get(&format!("GMS_{}", mode_key))
        .map(|s| s.as_str())
        .unwrap_or("");

    let doc = if !gms_ref.is_empty() {
        // The table value names the file; the full-path lookup is primary, a unique basename is the
        // explicit fallback for values that omit the folder.
        ButeDoc::from_vfs_with(vfs, "GMS.rez", gms_ref, GMS_PARSE)
            .or_else(|| {
                vfs.entry_text_by_basename("GMS.rez", gms_ref)
                    .ok()
                    .flatten()
                    .map(|t| ButeDoc::parse_with(&t, GMS_PARSE))
            })
            .unwrap_or_default()
    } else {
        ButeDoc::default()
    };

    (md, doc)
}

/// Loads the decrypted `.GMS` file for `(map, mode, clan)` from `<rom_dir>/Game/GMS.rez`.
pub fn gms_file(
    rom_dir: &Path,
    game_room: &GameRoomData,
    map: usize,
    mode: usize,
    clan: bool,
) -> (SectionMap, ButeDoc) {
    let vfs = RezVfs::new(rom_dir);
    gms_file_with_vfs(&vfs, game_room, map, mode, clan)
}

/// Resolves `Worlds\<world>` for `(map, mode, clan)`.
/// HARNESS: lobby_harness::test_world_for_room
pub fn world_for_room(
    rom_dir: &Path,
    game_room: &GameRoomData,
    map: usize,
    mode: usize,
    clan: bool,
    dserver_world_override: Option<&str>,
) -> String {
    if let Some(ov) = dserver_world_override {
        if !ov.is_empty() {
            return ov.to_string();
        }
    }
    let maps = game_room.maps();
    let key = maps
        .get(map)
        .and_then(|m| m.get("Key"))
        .map(|s| s.as_str())
        .unwrap_or("SnowValley");

    let norm = |s: &str| fold_latin1(&s.replace('_', ""));
    let names = world_names(rom_dir);

    let (_md, gm_doc) = gms_file(rom_dir, game_room, map, mode, clan);
    if let Some(gm_map) = gm_doc.get_str("MapInfo", "Map") {
        let norm_path = gm_map.replace('\\', "/");
        let base = norm_path.rsplit('/').next().unwrap_or(&norm_path);
        let base = match base.len().checked_sub(4) {
            Some(cut) if base.is_char_boundary(cut) && eq_fold(&base[cut..], ".DAT") => {
                &base[..cut]
            }
            _ => base,
        };
        if !base.is_empty() {
            for n in &names {
                if eq_fold(n, base) {
                    return format!("Worlds\\{}", n);
                }
            }
        }
    }

    let norm_key = norm(key);
    for n in &names {
        if norm(n) == norm_key {
            return format!("Worlds\\{}", n);
        }
    }
    for n in &names {
        if norm(n).starts_with(&norm_key) {
            return format!("Worlds\\{}", n);
        }
    }
    format!("Worlds\\{}", key)
}

/// Room parameters used to format the game server `room key=...` control line.
#[derive(Clone, Debug)]
pub struct RoomConfig {
    pub map: usize,
    pub mode: usize,
    pub clan: bool,
    pub goal: i32,
    pub ff: i32,
    pub arms: i32,
    pub home: u32,
    pub visitor: u32,
    pub home_team: i32,
    /// Non-zero overrides the `.gms` `TimeLimit` (callers decide where it comes from, e.g. a test setting).
    pub time_override: Option<i32>,
}

impl Default for RoomConfig {
    fn default() -> Self {
        Self {
            map: 0,
            mode: 0,
            clan: false,
            goal: 100,
            ff: 0,
            arms: 0,
            home: 0,
            visitor: 0,
            home_team: 1,
            time_override: None,
        }
    }
}

/// Clan room scoring policy.
/// DC-DEFAULT: CD-11 (config `clan.mode_weight.<KEY>`, hook `room.clan_policy`).
///
/// The defaults are DeCombat's own: every clan mode weighs `1.0`, and when the ROM `.gms` omits the clan point
/// keys no points are awarded. The ROM values always win when present.
#[derive(Clone, Debug, PartialEq)]
pub struct ClanRoomPolicy {
    /// Per-mode-key weight overrides (matched with Latin-1 case folding); first match wins.
    pub mode_weights: Vec<(String, f64)>,
    /// Weight for any mode key not listed in `mode_weights`.
    pub default_weight: f64,
    /// Fallback for `clan_basicpoint` when the `.gms` has no usable value.
    pub basic_point_default: i32,
    /// Fallback for `clan_winnerpoint` when the `.gms` has no usable value.
    pub winner_point_default: i32,
}

impl Default for ClanRoomPolicy {
    fn default() -> Self {
        Self {
            mode_weights: Vec::new(),
            default_weight: 1.0,
            basic_point_default: 0,
            winner_point_default: 0,
        }
    }
}

impl ClanRoomPolicy {
    /// Weight for a mode key; a non-finite configured weight is treated as `default_weight`.
    pub fn weight_for(&self, mode_key: &str) -> f64 {
        self.mode_weights
            .iter()
            .find(|(k, _)| eq_fold(k, mode_key))
            .map(|&(_, w)| w)
            .filter(|w| w.is_finite())
            .unwrap_or(self.default_weight)
    }
}

/// Round time of a room whose mode file gives none (GMD-2): every room has a time limit.
///
/// Resolution in [`room_line_from_gms`]: a non-zero [`RoomConfig::time_override`] first, then a non-zero class
/// entry the operator configured (`configured`, set through [`RoundTimePolicy::set`]), then a non-zero
/// `TimeLimit` of the mode file's objective section (not for the flag classes 7 / 8: GMD-2 amendment, their
/// `[Object0] TimeLimit` 180 is a mission-object template value, not a match length), then the entry of this
/// policy for the row's `GameType` (`per_class`, first match), else `default_secs`. A resolved 0 keeps DSV-20's
/// "no timer".
// DC-DEFAULT: GMD-2 (clarification, project decision "every room has a time limit"), see DC_DEFAULTS.md;
// config `room.round_secs.<class>` / `room.round_secs.default` (dc-lobby-server lobby.json)
// EVIDENCE: the 2009 room protocol carries no time (PROTOCOL_2009.md 0x382 / 0x389 / 0x3035 types 1..0xf); the
// 2009 client's round clock counts down what 0x44 / 0x45 send (0x44 shows it unconditionally; the co-op 0xd7 sub 4 also shows and sets it, never sent in the TDM captures) (cshell+0x49a40 case 0x45: zero hides it;
// cshell+0xbe070 draws only when shown; cshell+0xbe570 counts down)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundTimePolicy {
    /// `(mode class, seconds)`; the first entry for a class wins.
    pub per_class: Vec<(i32, i32)>,
    /// Seconds for a class not listed.
    pub default_secs: i32,
    /// Classes the operator set explicitly (`room.round_secs.<class>`); a non-zero one beats the mode file's
    /// `TimeLimit` (GMD-2 amendment). The built-in table entries are not in this list.
    pub configured: Vec<i32>,
}

impl Default for RoundTimePolicy {
    fn default() -> Self {
        // DC-DEFAULT: GMD-2 10 min for 1, 4, 6, 7, 8, 9; 2 min 30 s per round for 2, 3, 12, 13; 3 min per
        // round for 11; 0 for 5 (the co-op mission timers come from the mode file and 0xc9 / 0xd7 sub 4)
        let mut per_class = vec![(5, 0), (11, 180)];
        per_class.extend([2, 3, 12, 13].map(|c| (c, 150)));
        per_class.extend([1, 4, 6, 7, 8, 9].map(|c| (c, 600)));
        Self {
            per_class,
            default_secs: 600,
            configured: Vec::new(),
        }
    }
}

impl RoundTimePolicy {
    /// No fallback at all: a mode file without a time limit gives `time=0` (DSV-20, the reference stack's form).
    pub fn none() -> Self {
        Self {
            per_class: Vec::new(),
            default_secs: 0,
            configured: Vec::new(),
        }
    }

    /// The fallback seconds for a mode class.
    pub fn secs_for(&self, mode_class: i32) -> i32 {
        self.per_class
            .iter()
            .find(|(c, _)| *c == mode_class)
            .map_or(self.default_secs, |&(_, s)| s)
            .max(0)
    }

    /// The operator's non-zero entry for a mode class, if any (the built-in table never counts).
    pub fn override_for(&self, mode_class: i32) -> Option<i32> {
        self.configured
            .contains(&mode_class)
            .then(|| self.secs_for(mode_class))
            .filter(|s| *s > 0)
    }

    /// Sets (or replaces) the entry of one class as an operator override.
    pub fn set(&mut self, mode_class: i32, secs: i32) {
        self.per_class.retain(|(c, _)| *c != mode_class);
        self.per_class.insert(0, (mode_class, secs.max(0)));
        if !self.configured.contains(&mode_class) {
            self.configured.push(mode_class);
        }
    }
}

/// Extra room-line options: the clan policy, the round-time fallback and the optional HiSec prefix.
#[derive(Clone, Debug, Default)]
pub struct RoomOptions {
    pub clan_policy: ClanRoomPolicy,
    /// GMD-2 fallback round time when neither the room nor the mode file gives one.
    pub round_clock: RoundTimePolicy,
    /// HiSec is a DeCombat feature (not a 2009 client concept). `None` (default) or an empty/whitespace-only
    /// string adds nothing; otherwise the line becomes `<prefix> room key=...` with whitespace inside the
    /// prefix replaced by `_`.
    pub hisec_prefix: Option<String>,
}

/// Formats the `room key=...` control line from pre-loaded mode section `md` and GMS document `g`.
pub fn room_line_from_gms(
    md: &SectionMap,
    g: &ButeDoc,
    room: &RoomConfig,
    opts: &RoomOptions,
) -> String {
    let obj_sec_name = if !room.clan && room.mode == 2 {
        format!("Object{}", room.goal)
    } else {
        "Object0".to_string()
    };
    let o = g.get(&obj_sec_name);
    let ostr = |k: &str| -> &str {
        o.and_then(|sec| sec.get(k))
            .map(|s| s.as_str())
            .unwrap_or("")
    };

    let pts = |sec: &str| -> String {
        let d = g.get(sec);
        let basic = num_i32(d, "basic_point", 0);
        let kill = num_i32(d, "kill_point", 0);
        let death = num_i32(d, "death_point", 0);
        let mission = num_i32(d, "mission_point", 0);
        let win = num_i32(d, "win_team_point", 0);
        let lose = num_i32(d, "lose_team_point", 0);
        let corr = num_f64(d, "level_correction", 1.0);
        format!(
            "{},{},{},{},{},{},{:.2}",
            basic, kill, death, mission, win, lose, corr
        )
    };

    let e = g.get("MapExp");
    let m = g.get("MapMoney");

    let mode_key = md.get("Key").map(|s| s.as_str()).unwrap_or("TDM");
    let mode_kind = num_i32(Some(md), "GameType", 1);

    // DC-DEFAULT: GMD-2 (clarification + amendment) the order: room override, the operator's class entry, the mode
    // file's TimeLimit (never for the flag classes 7 / 8), the class fallback (RoundTimePolicy)
    let flag_class = matches!(mode_kind, 7 | 8);
    let file_time = if flag_class {
        0
    } else {
        num_i32(o, "TimeLimit", 0)
    };
    let tlim = match room.time_override {
        Some(t) if t != 0 => t,
        _ => match (opts.round_clock.override_for(mode_kind), file_time) {
            (Some(t), _) => t,
            (None, 0) => opts.round_clock.secs_for(mode_kind),
            (None, t) => t,
        },
    };

    let mut line = String::new();
    if let Some(p) = opts.hisec_prefix.as_deref() {
        let p: String = p.split_whitespace().collect::<Vec<_>>().join("_");
        if !p.is_empty() {
            line.push_str(&p);
            line.push(' ');
        }
    }
    line.push_str(&format!(
        "room key={} type={} goal={} ff={} arms={} time={} train={} exp={} gp={} cap={},{} rec={},{}",
        mode_key,
        mode_kind,
        room.goal,
        room.ff,
        room.arms,
        tlim,
        num_i32(o, "TrainingTimeLimit", 0),
        pts("MapExp"),
        pts("MapMoney"),
        num_i32(e, "capture_point", 0),
        num_i32(m, "capture_point", 0),
        num_i32(e, "recover_point", 0),
        num_i32(m, "recover_point", 0),
    ));

    if let Some(q) = g.get("ExpAndGPSetting") {
        if q.contains_key("QRT_BasicPoint") {
            let qrt_keys = [
                "BasicPoint",
                "KillPoint",
                "DeadPoint",
                "InfectPoint",
                "InfectedPoint",
                "HumanWinPoint",
                "InfectWinPoint",
            ];
            let vals: Vec<String> = qrt_keys
                .iter()
                .map(|k| num_i32(Some(q), &format!("QRT_{}", k), 0).to_string())
                .collect();
            line.push_str(" qrt=");
            line.push_str(&vals.join(","));
        }
        if q.contains_key("DefaultExp") {
            let bnt_keys = ["DefaultExp", "DefaultGP", "ExpIndex", "GPIndex"];
            let vals: Vec<String> = bnt_keys
                .iter()
                .map(|k| num_i32(Some(q), k, 0).to_string())
                .collect();
            line.push_str(" bnt=");
            line.push_str(&vals.join(","));
        }
    }

    let sv = |s: &str| s.replace(' ', "_");
    let vec_fmt = |s: &str| s.trim_matches(['<', '>', ' ']).replace(' ', "");

    let map_label = ostr("MapName");
    if !map_label.is_empty() {
        line.push_str(" map=");
        line.push_str(&sv(map_label));
    }

    let m0 = g.get("Mission0");

    let name = ostr("Name");
    if !name.is_empty() {
        line.push_str(" obj=");
        line.push_str(&sv(name));
    }

    let jsf_cmt = ostr("JSFComment");
    let creed_cmt = ostr("CreedComment");
    let train_cmt = ostr("TrainingComment");
    if !jsf_cmt.is_empty() || !creed_cmt.is_empty() {
        line.push_str(&format!(
            " cmt={}|{}|{}",
            sv(jsf_cmt),
            sv(creed_cmt),
            sv(train_cmt)
        ));
    }

    let large_map = ostr("LargeMap");
    if !large_map.is_empty() {
        line.push_str(&format!(
            " large={}|{}|{}|{}",
            sv(large_map),
            sv(ostr("LargeMapinfo")),
            vec_fmt(ostr("LargeMapScale")),
            vec_fmt(ostr("LargeMapStartPoint"))
        ));
    }

    let radar_map = ostr("RadarMap");
    if !radar_map.is_empty() {
        let rot = float_text(num_f64(o, "RadarMapRotation", 0.0));
        let mag = float_text(num_f64(o, "RadarMapMagnification", 0.0));
        let width = num_i32(o, "RadarMapImageWidth", 0);
        let height = num_i32(o, "RadarMapImageHeight", 0);
        line.push_str(&format!(
            " radar={}|{}|{}|{}|{}|{}|{}",
            sv(radar_map),
            vec_fmt(ostr("RadarMapScale")),
            vec_fmt(ostr("RadarMapStartPoint")),
            rot,
            mag,
            width,
            height
        ));
    }

    let m0_start = m0
        .and_then(|sec| sec.get("StartTrigger"))
        .map(|s| s.as_str())
        .unwrap_or("");
    let o_start = ostr("StartTrigger");
    if !o_start.is_empty() || !m0_start.is_empty() {
        let m0_ctrl = m0
            .and_then(|sec| sec.get("Controller"))
            .map(|s| s.as_str())
            .unwrap_or("");
        line.push_str(&format!(
            " trig={}|{}|{}|{}",
            sv(m0_start),
            sv(o_start),
            sv(ostr("EndTrigger")),
            sv(m0_ctrl)
        ));
    }

    if room.clan {
        let key = md.get("Key").map(|s| s.as_str()).unwrap_or("CDM");
        let weight = opts.clan_policy.weight_for(key);
        let (alpha, bravo) = if room.home_team == 1 {
            (room.home, room.visitor)
        } else {
            (room.visitor, room.home)
        };
        let bp = opts.clan_policy.basic_point_default;
        let wp = opts.clan_policy.winner_point_default;
        line.push_str(&format!(
            " clan={},{} cpts={},{},{},{} cw={}",
            alpha,
            bravo,
            num_i32(e, "clan_basicpoint", bp),
            num_i32(e, "clan_winnerpoint", wp),
            num_i32(m, "clan_basicpoint", bp),
            num_i32(m, "clan_winnerpoint", wp),
            float_text(weight)
        ));
    }

    line
}

/// Formats the `room key=...` control line for `room` using `RezVfs`.
pub fn room_line_with_vfs(
    vfs: &RezVfs,
    game_room: &GameRoomData,
    room: &RoomConfig,
    opts: &RoomOptions,
) -> String {
    let (md, g) = gms_file_with_vfs(vfs, game_room, room.map, room.mode, room.clan);
    room_line_from_gms(&md, &g, room, opts)
}

/// Formats the `room key=...` control line for `room` reading `GMS.rez` from `rom_dir`.
pub fn room_line(
    rom_dir: &Path,
    game_room: &GameRoomData,
    room: &RoomConfig,
    opts: &RoomOptions,
) -> String {
    let (md, g) = gms_file(rom_dir, game_room, room.map, room.mode, room.clan);
    room_line_from_gms(&md, &g, room, opts)
}

// ---------------------------------------------------------------------------
// ShopCatalog (`SHOP_ITEM_LIST.TXT`), ItemsTable (`ITEMS.TXT`), WeaponsTable (`WEAPONS.TXT`)
// ---------------------------------------------------------------------------

/// Upper bound on `Itemnum` (items per product) accepted from a catalogue row.
pub const MAX_PRODUCT_ITEMS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShopItemRow {
    pub index: i32,
    pub itemid: i32,
    pub class_code: i32,
    pub new_item: i32,
    pub name: String,
    /// `GP_DisplayNum` / `NX_DisplayNum`: the item's place in the GP / credit shop lists (0 = not listed there).
    pub gp_display: i32,
    pub nx_display: i32,
    /// `GP_Unlock` / `NX_Unlock`: the experience the item's GP / credit cell asks of the character (0 = none).
    pub gp_unlock: i32,
    pub nx_unlock: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShopProduct {
    pub id: i32,
    pub sale_type: i32,
    pub view: i32,
    pub price: i32,
    pub sale_price: i32,
    pub bonus: i32,
    pub name: String,
    /// `Vec<(item_index, consume_type, period)>`, at most [`MAX_PRODUCT_ITEMS`] entries.
    pub items: Vec<(i32, i32, i32)>,
    /// The product's period: its first item's `Period0`, a set product's `SetItemPeriod` (0 / 999: permanent).
    // EVIDENCE: cshell+0x257bc0 (product +0x20 = atoi of the first Period when SetItemIndex is -1, else
    // SetItemPeriod)
    pub period: i32,
    /// The product's consume type: its first item's `Consumetype0`, a set product's `SetItemConsumtype`.
    // EVIDENCE: cshell+0x257bc0 (product +0x24)
    pub consume: i32,
}

/// 2009 shop catalogue loaded from `ATTRIBUTES.rez` `SHOP_ITEM_LIST.TXT`.
///
/// Which products are shown as new/hot/sale is NOT decided here; see [`ShopFlagsPolicy`] (CD-09).
#[derive(Clone, Debug)]
pub struct ShopCatalog {
    products: HashMap<i32, ShopProduct>,
    infos: HashMap<i32, ShopItemRow>,
}

impl ShopCatalog {
    pub fn from_doc(doc: &ButeDoc) -> Self {
        let mut products = HashMap::new();
        let mut infos = HashMap::new();

        let parse_i = |d: &SectionMap, k: &str, dv: i32| -> i32 {
            d.get(k)
                .and_then(|v| v.trim().trim_matches('"').parse::<i32>().ok())
                .unwrap_or(dv)
        };

        for (tag, d) in doc.iter() {
            if tag.starts_with("ItemInfo") {
                let idx = parse_i(d, "Itemindex", -1);
                infos.insert(
                    idx,
                    ShopItemRow {
                        index: idx,
                        itemid: parse_i(d, "Itemid", -1),
                        class_code: parse_i(d, "Itemtype", -1),
                        new_item: parse_i(d, "NewItem", 0),
                        name: d.get("Name").cloned().unwrap_or_default(),
                        gp_display: parse_i(d, "GP_DisplayNum", 0),
                        nx_display: parse_i(d, "NX_DisplayNum", 0),
                        gp_unlock: parse_i(d, "GP_Unlock", 0),
                        nx_unlock: parse_i(d, "NX_Unlock", 0),
                    },
                );
            } else if tag.starts_with("Product") {
                let pid = parse_i(d, "ProductID", -1);
                // Hardening: a hostile `Itemnum` must not drive an unbounded loop or allocation.
                let n = (parse_i(d, "Itemnum", 1).max(0) as usize).min(MAX_PRODUCT_ITEMS);
                let mut items = Vec::with_capacity(n);
                for k in 0..n {
                    let ix = parse_i(d, &format!("Itemindex{}", k), -1);
                    let ct = parse_i(d, &format!("Consumetype{}", k), 0);
                    let per = parse_i(d, &format!("Period{}", k), 0);
                    items.push((ix, ct, per));
                }
                let price = parse_i(d, "Price", 0);
                let (period, consume) = if parse_i(d, "SetItemIndex", -1) == -1 {
                    items.first().map_or((0, 0), |(_, ct, per)| (*per, *ct))
                } else {
                    (
                        parse_i(d, "SetItemPeriod", 0),
                        parse_i(d, "SetItemConsumtype", 0),
                    )
                };
                products.insert(
                    pid,
                    ShopProduct {
                        id: pid,
                        sale_type: parse_i(d, "SaleType", 0),
                        view: parse_i(d, "Shopview", 0),
                        price,
                        sale_price: parse_i(d, "SalePrice", price),
                        bonus: parse_i(d, "BonusGP", 0),
                        name: d.get("ProductName").cloned().unwrap_or_default(),
                        period,
                        consume,
                        items,
                    },
                );
            }
        }

        Self { products, infos }
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, ButeError> {
        let doc = ButeDoc::try_from_vfs(vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\SHOP_ITEM_LIST.TXT")?
            .ok_or_else(|| ButeError::MissingEntry {
            rez: "ATTRIBUTES.rez".to_string(),
            name: "SHOP_ITEM_LIST.TXT".to_string(),
        })?;
        let mut catalog = Self::from_doc(&doc);
        catalog.add_seasonal();
        catalog.add_camo_crate();
        Ok(catalog)
    }

    /// Adds the Camo Crate (CRATE-1): a hidden row and product that only the lobby's drops hand out.
    pub fn add_camo_crate(&mut self) {
        let (row, product) = camo_crate::row();
        self.infos.insert(row.index, row);
        self.products.insert(product.id, product);
    }

    /// Adds the seasonal products (SHOP-2): hidden rows, which only the lobby's list puts on sale.
    pub fn add_seasonal(&mut self) {
        for (row, products) in seasonal::rows() {
            self.infos.insert(row.index, row);
            for p in products {
                self.products.insert(p.id, p);
            }
        }
    }

    pub fn from_rom(rom_dir: &Path) -> Result<Self, ButeError> {
        let vfs = RezVfs::new(rom_dir);
        Self::from_vfs(&vfs)
    }

    pub fn product(&self, pid: i32) -> Option<&ShopProduct> {
        self.products.get(&pid)
    }

    pub fn info(&self, index: i32) -> Option<&ShopItemRow> {
        self.infos.get(&index)
    }

    /// The lowest-numbered product whose first item row names item number `item_no`.
    pub fn first_product_of_item(&self, item_no: i32) -> Option<&ShopProduct> {
        self.products
            .values()
            .filter(|p| {
                p.items
                    .first()
                    .and_then(|(ix, _, _)| self.infos.get(ix))
                    .is_some_and(|r| r.itemid == item_no)
            })
            .min_by_key(|p| p.id)
    }

    /// Returns sorted product IDs with `view != 0` and `0 <= pid < 0x10000`.
    pub fn view_list(&self) -> Vec<i32> {
        let mut ids: Vec<i32> = self
            .products
            .iter()
            .filter_map(|(&pid, d)| {
                if d.view != 0 && (0..0x10000).contains(&pid) {
                    Some(pid)
                } else {
                    None
                }
            })
            .collect();
        ids.sort_unstable();
        ids
    }
}

// ---------------------------------------------------------------------------
// Shop flags policy (CD-09)
// ---------------------------------------------------------------------------

/// Days since 1970-01-01 for a civil date (public days-from-civil algorithm). `None` for an invalid date or
/// a date before 1970.
pub fn day_number(year: i32, month: u32, day: u32) -> Option<u32> {
    if !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let dim = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if leap {
                29
            } else {
                28
            }
        }
    };
    if day > dim {
        return None;
    }
    let y = if month <= 2 {
        year as i64 - 1
    } else {
        year as i64
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u32::try_from(days).ok()
}

/// Inputs to [`ShopFlagsPolicy::evaluate`]. Everything is data supplied by the caller.
pub struct ShopFlagInputs<'a> {
    /// Catalogue rows (from the ROM).
    pub catalogue: &'a ShopCatalog,
    /// Build date of the ROM, as [`day_number`].
    pub rom_build_day: u32,
    /// Day each product id was first seen/added (the ROM rows carry no date; the server records it).
    pub first_seen_day: &'a HashMap<i32, u32>,
    /// Units sold per product id in this server's database.
    pub sales: &'a HashMap<i32, u64>,
    /// Operator-curated sale list (product ids).
    pub operator_sale: &'a HashSet<i32>,
}

/// Result of [`ShopFlagsPolicy::evaluate`]; every list is sorted ascending and holds only visible products.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShopFlags {
    pub new: Vec<i32>,
    pub hot: Vec<i32>,
    pub sale: Vec<i32>,
}

/// Which products are shown as new / hot / sale.
/// DC-DEFAULT: CD-09 (config `shop.new_days`, `shop.hot_percent`, `shop.sale_list`; hook `shop.catalogue`).
///
/// - new: first seen within `new_window_days` days up to and including the ROM build day;
/// - hot: the top `hot_percent` % of visible products by sales (rounded up, products with no sales never
///   qualify, ties broken by lower product id);
/// - sale: visible products on the operator list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShopFlagsPolicy {
    pub new_window_days: u32,
    pub hot_percent: u32,
}

impl Default for ShopFlagsPolicy {
    fn default() -> Self {
        Self {
            new_window_days: 30,
            hot_percent: 10,
        }
    }
}

impl ShopFlagsPolicy {
    pub fn evaluate(&self, inp: &ShopFlagInputs<'_>) -> ShopFlags {
        let view = inp.catalogue.view_list();

        let new: Vec<i32> = view
            .iter()
            .copied()
            .filter(|pid| {
                inp.first_seen_day
                    .get(pid)
                    .map(|&d| {
                        d <= inp.rom_build_day && inp.rom_build_day - d <= self.new_window_days
                    })
                    .unwrap_or(false)
            })
            .collect();

        let pct = self.hot_percent.min(100) as u64;
        let quota = (view.len() as u64 * pct).div_ceil(100) as usize;
        let mut sold: Vec<(u64, i32)> = view
            .iter()
            .filter_map(|pid| {
                inp.sales
                    .get(pid)
                    .copied()
                    .filter(|&n| n > 0)
                    .map(|n| (n, *pid))
            })
            .collect();
        sold.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut hot: Vec<i32> = sold.into_iter().take(quota).map(|(_, pid)| pid).collect();
        hot.sort_unstable();

        let sale: Vec<i32> = view
            .iter()
            .copied()
            .filter(|pid| inp.operator_sale.contains(pid))
            .collect();

        ShopFlags { new, hot, sale }
    }
}

// ---------------------------------------------------------------------------
// Item type -> equip slot (client fact)
// ---------------------------------------------------------------------------

/// Maps an item type code to the equip slot index, or `None` when the client has no slot for that type.
// EVIDENCE: cshell+0x259f00 (switch over the item type; unlisted types return -1)
pub fn equip_slot_by_class_code(class_code: i32) -> Option<i32> {
    match class_code {
        0 | 1 | 2 | 3 | 8 | 9 | 10 | 0x1a | 0x1d => Some(0),
        4 | 5 => Some(1),
        7 => Some(2),
        6 | 0x1b => Some(3),
        0xb => Some(4),
        0xd => Some(5),
        0xc => Some(6),
        0xe => Some(7),
        0xf => Some(8),
        0x10 => Some(9),
        0x16 | 0x1e => Some(14),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// ItemsTable
// ---------------------------------------------------------------------------

/// Structured lookup table over `ATTRIBUTES.rez` `ITEMS.TXT`.
#[derive(Clone, Debug)]
pub struct ItemsTable {
    doc: ButeDoc,
    use_items: HashMap<i32, SectionMap>,
    backpack_slots: HashMap<i32, i32>,
    option_kinds: HashMap<i32, i32>,
    weapon_types: HashMap<i32, i32>,
    /// Weapon item number -> (`EquipBackpackType`, `EquipOnlyBackpack`).
    weapon_packs: HashMap<i32, (i32, bool)>,
    /// Specialist (`ItemType` 30, 10000 + `ItemID`) or backpack (7000 + `ItemID`) item number -> (`AddSlot`,
    /// `SlotType`).
    slot_forms: HashMap<i32, (i32, i32)>,
}

/// The weapon slot form of the backpack slots 10..13: how many there are and the kind of item they take (0 the
/// backpack's weapons, [`SPECIAL_SLOT_KIND`] a specialist's special items, the letters S1..S4).
// EVIDENCE: cshell+0x227240 (the shell's slot count +0x2dc and slot kind +0x2e0), cshell+0x2254d0 (the kind's reader)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PackForm {
    pub slots: i32,
    pub kind: i32,
}

/// The slot kind of a specialist's special slots (`SlotType` 1 of the ROM's specialist rows).
// EVIDENCE: cshell+0x227240 (kind 1 adds the slot license), cshell+0x13fed0 (kind 1: rows "S1".."S4")
pub const SPECIAL_SLOT_KIND: i32 = 1;
/// The function item kind (`SubType`) of a specialist slot license.
// EVIDENCE: cshell+0x227240 (the function list's first record whose row +0x14 (`SubType`) is 0x12)
pub const SUB_SLOT_LICENSE: i32 = 18;

impl PackForm {
    /// The slots take special items (a specialist is worn).
    pub fn special(&self) -> bool {
        self.kind == SPECIAL_SLOT_KIND
    }

    /// The slots that can be used (the 2009 lists show `slots % 5` rows, at most four).
    pub fn rows(&self) -> usize {
        if self.slots < 1 {
            0
        } else {
            (self.slots % 5) as usize
        }
    }
}

impl ItemsTable {
    pub fn from_doc(doc: ButeDoc) -> Self {
        let mut use_items = HashMap::new();
        let mut backpack_slots = HashMap::new();
        let mut option_kinds = HashMap::new();
        let mut weapon_types = HashMap::new();
        let mut weapon_packs = HashMap::new();
        let mut slot_forms = HashMap::new();
        // EVIDENCE: cshell+0x25dd60 (AddSlot -> record +0x28, SlotType -> +0x2c of the item rows; a record is filed in
        // the list of its ItemType, so a specialist's is found in type 0x1e's list only)
        let form = |d: &SectionMap| {
            let n = |k: &str| d.get(k).and_then(|s| plain_i32(s)).unwrap_or(0);
            (n("AddSlot"), n("SlotType"))
        };

        for (tag, d) in doc.iter() {
            if tag.starts_with("Character") {
                let kind = d.get("ItemType").and_then(|s| plain_i32(s));
                let id = d
                    .get("ItemID")
                    .and_then(|s| plain_i32(s))
                    .and_then(|id| id.checked_add(10000));
                if let (Some(no), Some(0x1e)) = (id, kind) {
                    slot_forms.insert(no, form(d));
                }
            } else if tag.starts_with("UseItem") {
                // ItemID must be all ASCII digits ("+5", "-1", "5 " are rejected).
                if let Some(no) = d
                    .get("ItemID")
                    .and_then(|s| digits_i32(s))
                    .and_then(|id| id.checked_add(9000))
                {
                    use_items.insert(no, d.clone());
                }
            } else if tag.starts_with("BackpackItem") {
                // An entry whose MultyInvenSlot is missing or not an integer is skipped (not read as 0).
                let id = d
                    .get("ItemID")
                    .and_then(|s| plain_i32(s))
                    .and_then(|id| id.checked_add(7000));
                let slots = d.get("MultyInvenSlot").and_then(|s| plain_i32(s));
                if let (Some(no), Some(slots)) = (id, slots) {
                    backpack_slots.insert(no, slots);
                }
                if let Some(no) = id {
                    slot_forms.insert(no, form(d));
                }
            } else if tag.starts_with("Option") {
                if let Some(no) = d
                    .get("ItemID")
                    .and_then(|s| plain_i32(s))
                    .and_then(|id| id.checked_add(1000))
                {
                    let kind = d.get("ItemType").and_then(|s| plain_i32(s)).unwrap_or(-1);
                    option_kinds.insert(no, kind);
                }
            } else if tag.starts_with("WeaponItem") {
                if let Some(id) = d.get("ItemID").and_then(|s| plain_i32(s)) {
                    let wt = d.get("ItemType").and_then(|s| plain_i32(s)).unwrap_or(-1);
                    weapon_types.insert(id, wt);
                    // EVIDENCE: cshell+0x25dd60 (EquipBackpackType -> record +0x24, EquipOnlyBackpack -> +0x240)
                    let n = |k: &str| d.get(k).and_then(|s| plain_i32(s)).unwrap_or(0);
                    weapon_packs.insert(id, (n("EquipBackpackType"), n("EquipOnlyBackpack") != 0));
                }
            }
        }

        Self {
            doc,
            use_items,
            backpack_slots,
            option_kinds,
            weapon_types,
            weapon_packs,
            slot_forms,
        }
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, ButeError> {
        let doc = ButeDoc::try_from_vfs(vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\ITEMS.TXT")?
            .ok_or_else(|| ButeError::MissingEntry {
                rez: "ATTRIBUTES.rez".to_string(),
                name: "ITEMS.TXT".to_string(),
            })?;
        Ok(Self::from_doc(doc))
    }

    pub fn from_rom(rom_dir: &Path) -> Result<Self, ButeError> {
        let vfs = RezVfs::new(rom_dir);
        Self::from_vfs(&vfs)
    }

    pub fn doc(&self) -> &ButeDoc {
        &self.doc
    }

    /// Returns `[UseItem<n>]` for function item `9000 + ItemID`.
    pub fn use_item_info(&self, item_no: i32) -> Option<&SectionMap> {
        self.use_items.get(&item_no)
    }

    /// Returns `MultyInvenSlot` for backpack item `7000 + ItemID`, or `0` for an unknown item.
    pub fn backpack_slot_count(&self, item_no: i32) -> i32 {
        self.backpack_slots.get(&item_no).copied().unwrap_or(0)
    }

    /// Returns `ItemType` (17 muzzle, 18 sight, 19 magazine) for option item `1000 + ItemID`, or `-1`.
    pub fn option_kind(&self, item_no: i32) -> i32 {
        self.option_kinds.get(&item_no).copied().unwrap_or(-1)
    }

    /// Returns `ItemType` of `[WeaponItem<n>]` with `ItemID == item_no`, or `-1`.
    pub fn weapon_class_code(&self, item_no: i32) -> i32 {
        self.weapon_types.get(&item_no).copied().unwrap_or(-1)
    }

    /// The backpack slot kind weapon `item_no` goes into (`EquipBackpackType`; 0 for a plain weapon or an unknown
    /// row): only slots of the same kind take it.
    // EVIDENCE: cshell+0x148020 (the item's record +0x24 must equal the shell's slot kind, else box 0x189e / 0x189f)
    pub fn pack_type(&self, item_no: i32) -> i32 {
        self.weapon_packs.get(&item_no).map_or(0, |p| p.0)
    }

    /// Weapon `item_no` sits in backpack slots only (`EquipOnlyBackpack`: the specialists' special items), never in
    /// its own weapon slot.
    // EVIDENCE: cshell+0x147470 (the storage page's Equip button hidden for such a record outside a drawer row),
    // cshell+0x1402f0 (window 7 refuses the own-slot choice for it)
    pub fn pack_only(&self, item_no: i32) -> bool {
        self.weapon_packs.get(&item_no).is_some_and(|p| p.1)
    }

    /// The backpack slots' form for the equipped backpack (`backpack`, -1 none), the worn character (`character`, -1
    /// none) and the function list's item numbers in their order: the backpack's slot count (`MultyInvenSlot`) and
    /// kind (`SlotType`); a specialist's nonzero `SlotType` and `AddSlot` replace them; with kind 1 the first slot
    /// license (`SubType` 18) of the list adds its `AddSlot`.
    // EVIDENCE: cshell+0x227240 (count and kind from the equipped backpack's row (type 0x10), then from the worn
    // character's row in type 0x1e's list (cshell+0x35aeb0 +0x2c, cshell+0x35ae60 +0x28, each only when nonzero); kind
    // 1: cshell+0x327070(0x12), the function list's first record of SubType 0x12, adds its row's +0x28)
    pub fn pack_form(
        &self,
        backpack: i32,
        character: i32,
        function_list: impl IntoIterator<Item = i32>,
    ) -> PackForm {
        let mut f = PackForm {
            slots: self.backpack_slot_count(backpack),
            kind: self.slot_forms.get(&backpack).map_or(0, |s| s.1),
        };
        if let Some(&(add, kind)) = self
            .slot_forms
            .get(&character)
            .filter(|_| character >= 10000)
        {
            if kind != 0 {
                f.kind = kind;
            }
            if add != 0 {
                f.slots = add;
            }
        }
        if f.kind == SPECIAL_SLOT_KIND {
            let licence = function_list.into_iter().find_map(|n| {
                let row = self.use_item_info(n)?;
                let sub = row.get("SubType").and_then(|s| plain_i32(s));
                (sub == Some(SUB_SLOT_LICENSE))
                    .then(|| row.get("AddSlot").and_then(|s| plain_i32(s)).unwrap_or(0))
            });
            f.slots += licence.unwrap_or(0);
        }
        f
    }

    /// Returns the `Available_ItemID<n>` reward pool for a Supply Case (`item_no = 8000 + n`), ordered by the
    /// numeric suffix `n` (so `..ID10` follows `..ID9`).
    pub fn supply_pool(&self, item_no: i32) -> Vec<i32> {
        let Some(case_no) = item_no.checked_sub(8000) else {
            return Vec::new();
        };
        let Some(sup) = self.doc.get(&format!("Supply{}", case_no)) else {
            return Vec::new();
        };
        let mut pairs: Vec<(u64, i32)> = sup
            .iter()
            .filter_map(|(k, v)| {
                let suffix = k.strip_prefix("Available_ItemID")?;
                let n = if suffix.is_empty() {
                    0
                } else {
                    digits_u64(suffix)?
                };
                Some((n, plain_i32(v)?))
            })
            .collect();
        pairs.sort_by_key(|&(n, _)| n);
        pairs.into_iter().map(|(_, id)| id).collect()
    }
}

fn digits_u64(s: &str) -> Option<u64> {
    if s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse::<u64>().ok()
    } else {
        None
    }
}

/// Structured lookup table over `ATTRIBUTES.rez` `WEAPONS.TXT`.
#[derive(Clone, Debug)]
pub struct WeaponsTable {
    doc: ButeDoc,
}

/// CLI-1606: the weapon-row keys of the muzzle, sight and magazine socket states (option kinds 0x11, 0x12, 0x13).
// EVIDENCE: cshell+0x17bd20 (pushes of these key strings before the stores to +0x42c / +0x430 / +0x434)
pub const OPTION_SOCKET_KEYS: [&str; 3] = ["MuzzleOPtion", "SightOption", "MagazineOption"];

/// The value of a socket-state key of a weapon row: the table spells the muzzle key `MuzzleOPtion` in 254 rows
/// and `MuzzleOption` in two (K1A, K7); the 2009 reader's case handling is not traced, so both spellings are read.
// OBSERVED: ROM WEAPONS.TXT ([Weapon21] K1A `MuzzleOption=1`, [Weapon45] K7 `MuzzleOption=2`, every other row
// `MuzzleOPtion`); DC-DEFAULT: CLI-1606 (amendment, proposed) either spelling counts
pub fn socket_state_text<'a>(row: &'a SectionMap, key: &str) -> Option<&'a String> {
    row.get(key)
        .or_else(|| row.get(&key.replace("OPtion", "Option")))
}

/// CLI-1606 amendment: the state of a socket whose key the weapon row lacks: open (the ModName list of CLI-712 still
/// decides what fits). The ROM has 55 `[Weapon<n>]` rows without any of the three keys (e.g. [Weapon0] M9) and 26
/// more without `SightOption` although they list sight mods (the `_LE` rows); the 2009 shop sold mods for them.
// EVIDENCE: cshell+0x17bd20 (each key's GetInt default is the record field's own current value), cshell+0x17ff80
// (the record is plain heap memory, `operator new(0x500)`, not cleared), cshell+0x17f7b0 (the constructor sets
// +0x428 and +0x438 but leaves +0x42c / +0x430 / +0x434 unset): in 2009 a missing key leaves an indeterminate value
// DC-DEFAULT: CLI-1606 (amendment, proposed) a missing key reads as 1 (open)
pub const SOCKET_STATE_WITHOUT_KEY: i32 = 1;

/// The socket state of weapon row `row` under key `key` (either muzzle spelling), [`SOCKET_STATE_WITHOUT_KEY`]
/// when the key is missing or not a number.
pub fn socket_state(row: &SectionMap, key: &str) -> i32 {
    socket_state_text(row, key)
        .and_then(|v| {
            v.split("//")
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('"')
                .trim()
                .parse::<i32>()
                .ok()
        })
        .unwrap_or(SOCKET_STATE_WITHOUT_KEY)
}

impl WeaponsTable {
    pub fn from_doc(doc: ButeDoc) -> Self {
        Self { doc }
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, ButeError> {
        let doc = ButeDoc::try_from_vfs(vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\WEAPONS.TXT")?
            .ok_or_else(|| ButeError::MissingEntry {
                rez: "ATTRIBUTES.rez".to_string(),
                name: "WEAPONS.TXT".to_string(),
            })?;
        Ok(Self::from_doc(doc))
    }

    pub fn from_rom(rom_dir: &Path) -> Result<Self, ButeError> {
        let vfs = RezVfs::new(rom_dir);
        Self::from_vfs(&vfs)
    }

    pub fn doc(&self) -> &ButeDoc {
        &self.doc
    }

    pub fn weapon(&self, weapon_no: i32) -> Option<&SectionMap> {
        self.doc.get(&format!("Weapon{}", weapon_no))
    }

    pub fn mod_entry(&self, mod_id: i32) -> Option<&SectionMap> {
        self.doc.get(&format!("Mod{}", mod_id))
    }

    /// CLI-1606: the muzzle, sight and magazine socket states of every `[Weapon<n>]` row (0 no socket, 1 open,
    /// 2 the part is on the model as standard; [`SOCKET_STATE_WITHOUT_KEY`] without the key).
    // EVIDENCE: cshell+0x17bd20 (the weapon row loader: OPTION_SOCKET_KEYS -> record +0x42c / +0x430 / +0x434),
    // cshell+0x152d90 (an option attaches only into a socket whose state is 1)
    pub fn option_socket_states(&self) -> Vec<(i32, [i32; 3])> {
        self.doc
            .iter()
            .filter_map(|(tag, row)| {
                let n = tag.strip_prefix("Weapon")?.parse::<i32>().ok()?;
                Some((n, OPTION_SOCKET_KEYS.map(|k| socket_state(row, k))))
            })
            .collect()
    }

    /// Checks whether attachment `item_no` (`1000 + Mod id`) is in `Weapon<weapon_no>`'s
    /// contiguous `ModName0..k` allowlist (names compared with Latin-1 case folding).
    pub fn option_weapon_compatible(&self, item_no: i32, weapon_no: i32) -> bool {
        if !(0..1000).contains(&weapon_no) {
            return false;
        }
        let Some(weapon) = self.weapon(weapon_no) else {
            return false;
        };
        let Some(mod_id) = item_no.checked_sub(1000) else {
            return false;
        };
        let Some(mod_sec) = self.mod_entry(mod_id) else {
            return false;
        };
        let Some(mod_name) = mod_sec.get("Name").filter(|s| !s.is_empty()) else {
            return false;
        };
        if weapon.get("Name").map(|s| s.is_empty()).unwrap_or(true) {
            return false;
        }
        let mut i = 0usize;
        while let Some(cand) = weapon.get(&format!("ModName{}", i)) {
            if eq_fold(cand, mod_name) {
                return true;
            }
            i += 1;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_from_vfs_reports_archive_errors() {
        // A missing archive is Ok(None); an invalid entry name is a RezError that from_vfs would hide.
        let vfs = RezVfs::new(std::env::temp_dir().join("dc-bute-no-such-rom-dir"));
        assert_eq!(
            ButeDoc::try_from_vfs(&vfs, "ATTRIBUTES.rez", "X.TXT").map(|o| o.is_none()),
            Ok(true)
        );
        assert!(ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", "../X.TXT").is_none());
        let r = ButeDoc::try_from_vfs(&vfs, "ATTRIBUTES.rez", "../X.TXT");
        assert!(matches!(r, Err(ButeError::Rez(_))), "got {r:?}");
        assert!(GameRoomData::from_vfs(&vfs).is_err());
    }

    // ----- helpers -------------------------------------------------------------------------------

    /// ROM-gated tests run only when `CA_ROM` is set (test code may read the environment; the library may not).
    fn rom() -> Option<std::path::PathBuf> {
        match std::env::var("CA_ROM") {
            Ok(v) if !v.trim().is_empty() => Some(std::path::PathBuf::from(v.trim())),
            _ => {
                eprintln!("CA_ROM not set: ROM-gated test skipped");
                None
            }
        }
    }

    fn sha(lines: &[String]) -> String {
        let joined: Vec<u8> = lines.join("\n").chars().map(|c| c as u8).collect();
        dc_rom::sha256_hex(&joined)
    }

    fn gms(text: &str) -> ButeDoc {
        ButeDoc::parse_with(text, GMS_PARSE)
    }

    const GMS_FIXTURE: &str = "[ Object0 ]
TimeLimit = 300
TrainingTimeLimit=5
MapName=My Map
[MapExp]
basic_point=10
level_correction=1.5f
clan_basicpoint=7
[MapMoney]
basic_point=2
";

    fn md(key: &str) -> SectionMap {
        let mut m = SectionMap::new();
        m.insert("Key".to_string(), key.to_string());
        m
    }

    // ----- parser behaviour ----------------------------------------------------------------------

    #[test]
    fn lines_split_on_crlf_lf_and_lone_cr() {
        let d = ButeDoc::parse("[A]\r\nx=1\ry=2\nz=3\r\r\n[B]\rw=4");
        assert_eq!(d.get_str("A", "x"), Some("1"));
        assert_eq!(d.get_str("A", "y"), Some("2"));
        assert_eq!(d.get_str("A", "z"), Some("3"));
        assert_eq!(d.get_str("B", "w"), Some("4"));
        assert_eq!(d.order, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn section_names_verbatim_by_default_and_stripped_on_request() {
        let text = "[ Foo ]\nk=1\n";
        let plain = ButeDoc::parse(text);
        assert!(plain.contains_key(" Foo "));
        assert!(!plain.contains_key("Foo"));
        let stripped = ButeDoc::parse_with(
            text,
            ParseOptions {
                strip_section_names: true,
            },
        );
        assert!(stripped.contains_key("Foo"));
        assert!(!stripped.contains_key(" Foo "));
        assert_eq!(
            ParseOptions::default(),
            ParseOptions {
                strip_section_names: false
            }
        );
    }

    #[test]
    fn gms_loader_strips_section_names() {
        // The GMS loader constant is the one gms_file_with_vfs uses.
        const { assert!(GMS_PARSE.strip_section_names) };
        let d = gms(GMS_FIXTURE);
        assert!(d.contains_key("Object0"));
    }

    #[test]
    fn comments_quotes_and_pre_section_keys() {
        let d = ButeDoc::parse("orphan=1\n[S]\nk = \"v\" // note\n// full line\nk2=a=b\n");
        assert_eq!(d.get_str("S", "k"), Some("v"));
        assert_eq!(d.get_str("S", "k2"), Some("a=b"));
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn section_map_keeps_insertion_order_and_last_value() {
        let d = ButeDoc::parse("[S]\nb=1\na=2\nb=3\nc=4\n");
        let s = d.get("S").unwrap();
        let keys: Vec<&str> = s.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["b", "a", "c"]);
        assert_eq!(s.get("b").map(|v| v.as_str()), Some("3"));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn number_rule_trim_then_strip_one_trailing_f() {
        assert_eq!(num_text(" 1.5f "), "1.5");
        assert_eq!(num_text("2F"), "2");
        assert_eq!(num_text("3ff"), "3f"); // only one suffix is stripped
        assert_eq!(
            "3ff".trim().strip_suffix('f').unwrap().parse::<i32>().ok(),
            None
        );
        let d = gms("[S]\na=1.5f\nb=1.5 f\nc=nan\nd=inf\ne=7F\n");
        assert_eq!(d.get_f64("S", "a", -1.0), 1.5);
        assert_eq!(d.get_f64("S", "b", -1.0), -1.0); // space survives the strip
        assert_eq!(d.get_f64("S", "c", -1.0), -1.0); // non-finite rejected
        assert_eq!(d.get_f64("S", "d", -1.0), -1.0);
        let s = d.get("S");
        assert_eq!(num_i32(s, "e", 0), 7);
        assert_eq!(num_i32(s, "a", 0), 0);
    }

    #[test]
    fn latin1_case_folding() {
        assert!(eq_fold("Caf\u{C9}", "caf\u{E9}"));
        assert!(eq_fold("ABC", "abc"));
        assert!(!eq_fold("a", "ab"));
        assert_eq!(fold_latin1_char('\u{D7}'), '\u{D7}'); // multiplication sign is not a letter
        assert_eq!(fold_latin1_char('\u{DE}'), '\u{FE}');
        assert_eq!(fold_latin1_char('\u{DF}'), '\u{DF}');
        assert_eq!(fold_latin1_char('\u{F7}'), '\u{F7}');
        assert!(!eq_fold("\u{D7}", "\u{F7}"));
        // Attachment matching uses the folding.
        let w = WeaponsTable::from_doc(ButeDoc::parse(
            "[Weapon5]\nName=W\nModName0=Vis\u{C9}e\n[Mod3]\nName=vis\u{E9}e\n",
        ));
        assert!(w.option_weapon_compatible(1003, 5));
        assert!(!w.option_weapon_compatible(1004, 5));
        assert!(!w.option_weapon_compatible(i32::MIN, 5)); // no overflow
    }

    #[test]
    fn option_socket_states_read_the_weapon_rows() {
        // CLI-1606: synthetic rows (key names only); a missing key reads as open (amendment), a row without any
        // of the three keys is open everywhere
        let w = WeaponsTable::from_doc(ButeDoc::parse(
            "[Weapon3]
MuzzleOPtion=2
SightOption=2 // built in
MagazineOption=1
[Weapon4]
MagazineOption=\"1\"
[Weapon21]
MuzzleOption=1
[Weapon22]
Name=M9
[Mod0]
Name=X
",
        ));
        let mut v = w.option_socket_states();
        v.sort();
        // the K1A spelling (lower-case o) reads too
        assert_eq!(
            v,
            vec![
                (3, [2, 2, 1]),
                (4, [1, 1, 1]),
                (21, [1, 1, 1]),
                (22, [1, 1, 1])
            ]
        );
    }

    // ----- items ---------------------------------------------------------------------------------

    #[test]
    fn backpack_entry_with_bad_slot_is_skipped_not_zero() {
        let t = ItemsTable::from_doc(ButeDoc::parse(
            "[BackpackItem0]\nItemID=1\nMultyInvenSlot=4\n\
             [BackpackItem1]\nItemID=2\nMultyInvenSlot=many\n\
             [BackpackItem2]\nItemID=3\n",
        ));
        assert_eq!(t.backpack_slot_count(7001), 4);
        assert!(!t.backpack_slots.contains_key(&7002));
        assert!(!t.backpack_slots.contains_key(&7003));
        assert_eq!(t.backpack_slot_count(7002), 0); // unknown stays 0 at the lookup
    }

    #[test]
    fn use_item_requires_all_digit_item_id() {
        let t = ItemsTable::from_doc(ButeDoc::parse(
            "[UseItem0]\nItemID=1\n[UseItem1]\nItemID=+2\n[UseItem2]\nItemID=-3\n\
             [UseItem3]\nItemID=4x\n[UseItem4]\nItemID=\n[UseItem5]\nItemID=2147483647\n",
        ));
        assert!(t.use_item_info(9001).is_some());
        assert!(t.use_item_info(9002).is_none());
        assert!(t.use_item_info(8997).is_none());
        assert!(t.use_item_info(9004).is_none());
        assert_eq!(t.use_items.len(), 1); // i32::MAX + 9000 overflows and is skipped
    }

    /// CLI-772 amendment: a specialist's special slots (1 from its row, 2 with a slot license in the function list);
    /// a mercenary or a plain backpack keeps the backpack's form; special items are marked by their weapon row.
    #[test]
    fn a_specialist_has_special_slots_and_a_licence_adds_one() {
        let t = ItemsTable::from_doc(ButeDoc::parse(
            "[WeaponItem0]\nItemID=171\nItemType=29\nEquipOnlyBackpack=1\nEquipBackpackType=1\n\
             [WeaponItem1]\nItemID=20\nItemType=0\n\
             [BackpackItem0]\nItemID=1\nMultyInvenSlot=2\n\
             [Character0]\nItemID=4\nItemType=22\n\
             [Character1]\nItemID=22\nItemType=30\nAddSlot=1\nSlotType=1\n\
             [UseItem0]\nItemID=36\nItemType=21\nSubType=18\nAddSlot=1\n\
             [UseItem1]\nItemID=5\nItemType=21\nSubType=13\nAddSlot=3\n",
        ));
        assert_eq!((t.pack_type(171), t.pack_only(171)), (1, true));
        assert_eq!((t.pack_type(20), t.pack_only(20)), (0, false));
        let plain = PackForm { slots: 2, kind: 0 };
        assert_eq!(t.pack_form(7001, -1, []), plain);
        assert_eq!(t.pack_form(7001, 10004, [9036]), plain);
        let one = t.pack_form(-1, 10022, [9005]);
        assert_eq!(
            (one, one.special(), one.rows()),
            (PackForm { slots: 1, kind: 1 }, true, 1)
        );
        assert_eq!(
            t.pack_form(-1, 10022, [9005, 9036]),
            PackForm { slots: 2, kind: 1 }
        );
        assert_eq!(t.pack_form(-1, -1, []).rows(), 0);
    }

    #[test]
    fn supply_pool_orders_by_numeric_suffix() {
        let t = ItemsTable::from_doc(ButeDoc::parse(
            "[Supply3]\nAvailable_ItemID10=100\nAvailable_ItemID2=20\nAvailable_ItemID9=90\nAvailable_ItemIDx=1\nother=5\n",
        ));
        assert_eq!(t.supply_pool(8003), vec![20, 90, 100]);
        assert!(t.supply_pool(i32::MIN).is_empty());
    }

    #[test]
    fn equip_slot_mapping_matches_client_switch() {
        // EVIDENCE: cshell+0x259f00
        assert_eq!(equip_slot_by_class_code(0), Some(0));
        assert_eq!(equip_slot_by_class_code(0x1d), Some(0));
        assert_eq!(equip_slot_by_class_code(0x1b), Some(3));
        assert_eq!(equip_slot_by_class_code(0xc), Some(6));
        assert_eq!(equip_slot_by_class_code(0xd), Some(5));
        assert_eq!(equip_slot_by_class_code(0x1e), Some(14));
        assert_eq!(equip_slot_by_class_code(0x16), Some(14));
        assert_eq!(equip_slot_by_class_code(0x11), None);
        assert_eq!(equip_slot_by_class_code(-1), None);
    }

    // ----- shop ----------------------------------------------------------------------------------

    fn shop_fixture() -> ShopCatalog {
        let mut s = String::new();
        for i in 0..20 {
            s.push_str(&format!(
                "[Product{i}]\n\
ProductID={i}\nShopview={}\nPrice=100\n\
Itemnum=1\nItemindex0={i}\n",
                if i == 19 { 0 } else { 1 }
            ));
        }
        ShopCatalog::from_doc(&ButeDoc::parse(&s))
    }

    #[test]
    fn itemnum_is_capped() {
        let c = ShopCatalog::from_doc(&ButeDoc::parse(
            "[Product0]\n\
ProductID=1\n\
Itemnum=2000000000\n",
        ));
        assert_eq!(c.product(1).unwrap().items.len(), MAX_PRODUCT_ITEMS);
        let c = ShopCatalog::from_doc(&ButeDoc::parse(
            "[Product0]\n\
ProductID=1\n\
Itemnum=-5\n",
        ));
        assert!(c.product(1).unwrap().items.is_empty());
    }

    #[test]
    fn shop_flags_policy_cd09() {
        let cat = shop_fixture();
        assert_eq!(cat.view_list().len(), 19);
        let build = day_number(2009, 6, 30).unwrap();
        let first_seen: HashMap<i32, u32> = [
            (1, build - 30),
            (2, build - 31),
            (3, build),
            (4, build + 1),
            (19, build),
        ]
        .into_iter()
        .collect();
        // 19 visible products: ceil(19 * 10 / 100) = 2 hot slots.
        let sales: HashMap<i32, u64> = [(5, 50), (6, 90), (7, 90), (8, 0), (19, 999)]
            .into_iter()
            .collect();
        let sale_list: HashSet<i32> = [3, 19, 400].into_iter().collect();
        let flags = ShopFlagsPolicy::default().evaluate(&ShopFlagInputs {
            catalogue: &cat,
            rom_build_day: build,
            first_seen_day: &first_seen,
            sales: &sales,
            operator_sale: &sale_list,
        });
        assert_eq!(flags.new, vec![1, 3]); // 2 too old, 4 in the future, 19 hidden
        assert_eq!(flags.hot, vec![6, 7]); // tie on 90 broken by id; 19 hidden; 8 sold nothing
        assert_eq!(flags.sale, vec![3]);
        // Nothing supplied, nothing flagged.
        let none = ShopFlagsPolicy::default().evaluate(&ShopFlagInputs {
            catalogue: &cat,
            rom_build_day: build,
            first_seen_day: &HashMap::new(),
            sales: &HashMap::new(),
            operator_sale: &HashSet::new(),
        });
        assert_eq!(none, ShopFlags::default());
    }

    #[test]
    fn day_numbers() {
        assert_eq!(day_number(1970, 1, 1), Some(0));
        assert_eq!(day_number(2000, 1, 1), Some(10957));
        assert_eq!(day_number(2000, 3, 1), Some(11017));
        assert_eq!(day_number(2001, 2, 29), None);
        assert_eq!(day_number(2000, 2, 29), Some(11016));
        assert_eq!(day_number(1969, 12, 31), None);
        assert_eq!(day_number(2000, 13, 1), None);
    }

    // ----- world names ---------------------------------------------------------------------------

    #[test]
    fn world_entry_names() {
        assert_eq!(world_name_from_entry("WORLDS\\ABC.DAT"), Some("ABC"));
        assert_eq!(world_name_from_entry("worlds\\Abc.dat"), Some("Abc"));
        // A bare entry with an empty name is excluded on purpose.
        assert_eq!(world_name_from_entry("WORLDS\\.DAT"), None);
        assert_eq!(world_name_from_entry("WORLDS\\ABC.TXT"), None);
        assert_eq!(world_name_from_entry("OTHER\\ABC.DAT"), None);
        assert_eq!(
            world_name_from_entry("\u{E9}\u{E9}\u{E9}\u{E9}\u{E9}\u{E9}.DAT"),
            None
        );
    }

    // ----- room line -----------------------------------------------------------------------------

    #[test]
    fn room_line_basic_and_time_override_without_env() {
        let g = gms(GMS_FIXTURE);
        let room = RoomConfig::default();
        let opts = RoomOptions::default();
        let l = room_line_from_gms(&md("TDM"), &g, &room, &opts);
        assert_eq!(
            l,
            "room key=TDM type=1 goal=100 ff=0 arms=0 time=300 train=5 exp=10,0,0,0,0,0,1.50 \
             gp=2,0,0,0,0,0,1.00 cap=0,0 rec=0,0 map=My_Map"
        );
        let mut r2 = room.clone();
        r2.time_override = Some(77);
        assert!(room_line_from_gms(&md("TDM"), &g, &r2, &opts).contains(" time=77 "));
        r2.time_override = Some(0); // zero means "no override"
        assert!(room_line_from_gms(&md("TDM"), &g, &r2, &opts).contains(" time=300 "));
    }

    // DC-DEFAULT: GMD-2 (clarification) synthetic mode files: no TimeLimit / TimeLimit 0 take the class fallback
    #[test]
    fn room_line_time_fallback_per_class() {
        let no_time = gms("[ Object0 ]
MapName=M
");
        let zero_time = gms("[ Object0 ]
TimeLimit = 0
");
        let class = |key: &str, t: &str| {
            let mut m = md(key);
            m.insert("GameType".to_string(), t.to_string());
            m
        };
        let opts = RoomOptions::default();
        let room = RoomConfig::default();
        let line =
            |m: &SectionMap, g: &ButeDoc, o: &RoomOptions| room_line_from_gms(m, g, &room, o);
        assert!(line(&class("TDM", "1"), &no_time, &opts).contains(" time=600 "));
        assert!(line(&class("X", "9"), &zero_time, &opts).contains(" time=600 "));
        assert!(line(&class("X", "12"), &no_time, &opts).contains(" time=150 "));
        assert!(line(&class("X", "11"), &no_time, &opts).contains(" time=180 "));
        assert!(line(&class("X", "5"), &no_time, &opts).contains(" time=0 "));
        assert!(line(&class("X", "77"), &no_time, &opts).contains(" time=600 "));
        // the mode file's own limit and the room override still win
        assert!(line(&class("TDM", "1"), &gms(GMS_FIXTURE), &opts).contains(" time=300 "));
        let mut r2 = room.clone();
        r2.time_override = Some(42);
        assert!(room_line_from_gms(&class("TDM", "1"), &no_time, &r2, &opts).contains(" time=42 "));
        // operator policy: per class, default, and "none" (DSV-20 no timer)
        let mut p = RoundTimePolicy::default();
        p.set(1, 900);
        p.default_secs = 30;
        let o2 = RoomOptions {
            round_clock: p.clone(),
            ..RoomOptions::default()
        };
        assert!(line(&class("TDM", "1"), &no_time, &o2).contains(" time=900 "));
        assert!(line(&class("X", "77"), &no_time, &o2).contains(" time=30 "));
        p.set(1, -5);
        assert_eq!(p.secs_for(1), 0);
        let o3 = RoomOptions {
            round_clock: RoundTimePolicy::none(),
            ..RoomOptions::default()
        };
        assert!(line(&class("TDM", "1"), &no_time, &o3).contains(" time=0 "));
    }

    // DC-DEFAULT: GMD-2 (amendment) the flag classes ignore the mode file's mission-object TimeLimit 180; the
    // operator's class entry beats the file's limit for every class
    #[test]
    fn room_line_flag_modes_ignore_the_template_time_limit() {
        let ctf = gms("[ Object0 ]
OffenceTeam = 1
DefenceTeam = 2
TimeLimit = 180
");
        let qrt = gms("[ Object0 ]
TimeLimit = 3
");
        let class = |key: &str, t: &str| {
            let mut m = md(key);
            m.insert("GameType".to_string(), t.to_string());
            m
        };
        let room = RoomConfig::default();
        let line =
            |m: &SectionMap, g: &ButeDoc, o: &RoomOptions| room_line_from_gms(m, g, &room, o);
        let opts = RoomOptions::default();
        assert!(line(&class("CTF", "7"), &ctf, &opts).contains(" time=600 "));
        assert!(line(&class("CCTF", "8"), &ctf, &opts).contains(" time=600 "));
        let mut p = RoundTimePolicy::default();
        p.set(7, 900);
        let o2 = RoomOptions {
            round_clock: p.clone(),
            ..RoomOptions::default()
        };
        assert!(line(&class("CTF", "7"), &ctf, &o2).contains(" time=900 "));
        assert!(line(&class("CCTF", "8"), &ctf, &o2).contains(" time=600 "));
        // a class set to 0 on purpose keeps "no clock"
        p.set(7, 0);
        let o3 = RoomOptions {
            round_clock: p,
            ..RoomOptions::default()
        };
        assert!(line(&class("CTF", "7"), &ctf, &o3).contains(" time=0 "));
        // the room override stays first
        let mut r2 = room.clone();
        r2.time_override = Some(42);
        assert!(room_line_from_gms(&class("CTF", "7"), &ctf, &r2, &o2).contains(" time=42 "));
        // a file without TimeLimit still gets the class fallback
        let tdm = gms("[ Object0 ]
MapName=M
");
        assert!(line(&class("TDM", "1"), &tdm, &opts).contains(" time=600 "));
        // the other modes keep the file's limit (Quarantine 3 = minutes, read by the game server) unless the
        // operator configured the class
        assert!(line(&class("QRT", "11"), &qrt, &opts).contains(" time=3 "));
        let mut p = RoundTimePolicy::default();
        p.set(1, 900);
        let o4 = RoomOptions {
            round_clock: p,
            ..RoomOptions::default()
        };
        assert!(line(&class("TDM", "1"), &gms(GMS_FIXTURE), &o4).contains(" time=900 "));
        assert!(line(&class("QRT", "11"), &qrt, &o4).contains(" time=3 "));
    }

    #[test]
    fn room_line_ignores_environment() {
        // The library never reads CA_TEST_ROOM_TIME (or any variable).
        let g = gms(GMS_FIXTURE);
        let key = "CA_TEST_ROOM_TIME";
        let before = room_line_from_gms(
            &md("TDM"),
            &g,
            &RoomConfig::default(),
            &RoomOptions::default(),
        );
        std::env::set_var(key, "9999");
        let after = room_line_from_gms(
            &md("TDM"),
            &g,
            &RoomConfig::default(),
            &RoomOptions::default(),
        );
        std::env::remove_var(key);
        assert_eq!(before, after);
    }

    #[test]
    fn room_line_clan_policy_cd11() {
        let g = gms(GMS_FIXTURE);
        let room = RoomConfig {
            clan: true,
            home: 10,
            visitor: 20,
            home_team: 1,
            ..RoomConfig::default()
        };
        let l = room_line_from_gms(&md("CDM"), &g, &room, &RoomOptions::default());
        assert!(l.ends_with(" clan=10,20 cpts=7,0,0,0 cw=1.0"), "{l}");
        let policy = ClanRoomPolicy {
            mode_weights: vec![("cdm".to_string(), 0.5)],
            default_weight: 2.0,
            basic_point_default: 3,
            winner_point_default: 4,
        };
        let opts = RoomOptions {
            clan_policy: policy,
            hisec_prefix: None,
            ..RoomOptions::default()
        };
        assert!(room_line_from_gms(&md("CDM"), &g, &room, &opts)
            .ends_with(" clan=10,20 cpts=7,4,3,4 cw=0.5"));
        assert!(room_line_from_gms(&md("OTHER"), &g, &room, &opts).ends_with(" cw=2.0"));
        let mut visitor_home = room.clone();
        visitor_home.home_team = 2;
        assert!(room_line_from_gms(&md("CDM"), &g, &visitor_home, &opts).contains(" clan=20,10 "));
        let nan_policy = ClanRoomPolicy {
            mode_weights: vec![("CDM".to_string(), f64::NAN)],
            ..ClanRoomPolicy::default()
        };
        assert_eq!(nan_policy.weight_for("CDM"), 1.0);
    }

    #[test]
    fn room_line_hisec_prefix_is_optional() {
        let g = gms(GMS_FIXTURE);
        let plain = room_line_from_gms(
            &md("TDM"),
            &g,
            &RoomConfig::default(),
            &RoomOptions::default(),
        );
        assert!(plain.starts_with("room key="));
        let mk = |p: Option<&str>| RoomOptions {
            hisec_prefix: p.map(str::to_string),
            ..RoomOptions::default()
        };
        let pre = room_line_from_gms(&md("TDM"), &g, &RoomConfig::default(), &mk(Some("hi sec")));
        assert_eq!(pre, format!("hi_sec {plain}"));
        for none in [None, Some(""), Some("  ")] {
            assert_eq!(
                room_line_from_gms(&md("TDM"), &g, &RoomConfig::default(), &mk(none)),
                plain
            );
        }
    }

    #[test]
    fn mode_masks_do_not_overflow_with_many_modes() {
        let mut text = String::from("[Map0]\nEnable=true\n");
        for i in 0..40 {
            text.push_str(&format!("GMS_K{i}=f{i}\n"));
        }
        for i in 0..40 {
            text.push_str(&format!("[Mode{i}]\nKey=K{i}\n"));
        }
        let gr = GameRoomData::from_doc(ButeDoc::parse(&text));
        assert_eq!(gr.map_mode_masks(false), vec![(0, u32::MAX)]);
    }

    // ----- errors --------------------------------------------------------------------------------

    #[test]
    fn errors_display_and_implement_error() {
        let e = ButeDoc::from_file(Path::new("/definitely/not/here.txt")).unwrap_err();
        assert!(matches!(e, ButeError::Io { .. }));
        assert!(e.to_string().starts_with("failed to read "));
        let m = ButeError::MissingEntry {
            rez: "A.rez".to_string(),
            name: "B.TXT".to_string(),
        };
        assert_eq!(m.to_string(), "failed to load B.TXT from A.rez");
        let _boxed: Box<dyn std::error::Error> = Box::new(m);
    }

    // ----- fuzz ----------------------------------------------------------------------------------

    struct XorShift(u64);
    impl XorShift {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    #[test]
    fn seeded_fuzz_parse_bytes_latin1_does_not_panic() {
        const ALPHABET: &[u8] = b"[]=/\"\r\n \t0123456789fFabcZ_\\<>.-+\xC9\xD7\xDE\xE9\x00\xFF";
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
        for round in 0..10_000u32 {
            let len = (rng.next() % 600) as usize;
            let structured = round % 2 == 0;
            let buf: Vec<u8> = (0..len)
                .map(|_| {
                    let r = rng.next();
                    if structured {
                        ALPHABET[(r % ALPHABET.len() as u64) as usize]
                    } else {
                        (r >> 24) as u8
                    }
                })
                .collect();
            for opts in [ParseOptions::default(), GMS_PARSE] {
                let d = ButeDoc::parse_bytes_latin1_with(&buf, opts);
                let _ = ShopCatalog::from_doc(&d).view_list();
                let _ = ItemsTable::from_doc(d.clone()).supply_pool(8000);
                let _ = room_line_from_gms(
                    &md("TDM"),
                    &d,
                    &RoomConfig::default(),
                    &RoomOptions::default(),
                );
                for (_, s) in d.iter() {
                    let _ = s.iter().count();
                }
            }
        }
    }

    // ----- ROM-gated (hashes and counts only; no ROM literals) -------------------------------------

    #[test]
    fn rom_gameroom_masks_worlds_and_matrix() {
        let Some(rom) = rom() else { return };
        let gr = GameRoomData::from_rom(&rom).expect("load GAMEROOM.TXT");
        let normal_masks = gr.map_mode_masks(false);
        let clan_masks = gr.map_mode_masks(true);
        let ser = |v: &[(usize, u32)]| -> Vec<String> {
            v.iter().map(|(m, k)| format!("{m}:{k}")).collect()
        };
        assert_eq!(
            (normal_masks.len(), clan_masks.len()),
            (NORMAL_MASK_COUNT, CLAN_MASK_COUNT)
        );
        assert_eq!(sha(&ser(&normal_masks)), NORMAL_MASK_SHA);
        assert_eq!(sha(&ser(&clan_masks)), CLAN_MASK_SHA);

        let worlds = world_names(&rom);
        assert_eq!(worlds.len(), WORLD_COUNT);
        assert_eq!(sha(&worlds), WORLD_NAMES_SHA);

        // Optional differential against the harness matrix (read at run time, never embedded).
        // Test-only use of the checkout-layout shim.
        #[allow(deprecated)]
        let matrix_path = dc_rom::repo_root()
            .join("build")
            .join("port")
            .join("out_phaseaudit9")
            .join("catalogue_final")
            .join("post_integration_matrix.json");
        let Ok(matrix_text) = fs::read_to_string(&matrix_path) else {
            eprintln!("matrix json absent: differential part skipped");
            return;
        };
        let normal_mask_map: HashMap<usize, u32> = normal_masks.iter().copied().collect();
        let clan_mask_map: HashMap<usize, u32> = clan_masks.iter().copied().collect();
        let mut row_count = 0usize;
        for block in matrix_text.split('{').skip(1) {
            let Some(body) = block.split('}').next() else {
                continue;
            };
            let mut kv: HashMap<String, String> = HashMap::new();
            for line in body.lines() {
                let line = line.trim().trim_end_matches(',');
                if let Some((k, v)) = line.split_once(':') {
                    kv.insert(
                        k.trim().trim_matches('"').to_string(),
                        v.trim().trim_matches('"').replace("\\\\", "\\"),
                    );
                }
            }
            if !kv.contains_key("id") {
                continue;
            }
            row_count += 1;
            let clan = kv["clan"] == "true";
            let mode_id: usize = kv["mode_id"].parse().unwrap();
            let map_id: usize = kv["map_id"].parse().unwrap();
            let advertised = kv["advertised"] == "true";
            let mask_map = if clan {
                &clan_mask_map
            } else {
                &normal_mask_map
            };
            let got = (mask_map.get(&map_id).copied().unwrap_or(0) & (1u32 << mode_id)) != 0;
            assert_eq!(got, advertised, "advertised mismatch on {}", kv["id"]);
            assert_eq!(
                world_for_room(&rom, &gr, map_id, mode_id, clan, None),
                kv["resolved_world"],
                "world mismatch on {}",
                kv["id"]
            );
            assert_eq!(
                fireteam_refusal(clan, mode_id).is_some(),
                kv["root_admission"] == "REFUSE_FIRETEAM",
                "fireteam mismatch on {}",
                kv["id"]
            );
        }
        assert_eq!(row_count, MATRIX_ROWS);
    }

    #[test]
    fn rom_room_lines_hash() {
        // Regression pin of OUR room-line output under the default RoomOptions without the GMD-2 time fallback
        // (hash only; the fallback is checked by rom_room_lines_time_fallback).
        let Some(rom) = rom() else { return };
        let vfs = RezVfs::new(&rom);
        let gr = GameRoomData::from_vfs(&vfs).expect("load GAMEROOM.TXT");
        let opts = RoomOptions {
            round_clock: RoundTimePolicy::none(),
            ..RoomOptions::default()
        };
        let mut lines = Vec::new();
        for clan in [false, true] {
            for (mid, mask) in gr.map_mode_masks(clan) {
                for mode in 0..16 {
                    if (mask & (1u32 << mode)) != 0 {
                        let cfg = RoomConfig {
                            map: mid,
                            mode,
                            clan,
                            goal: gr.default_goal(mode, clan),
                            ff: if clan { 1 } else { 0 },
                            home: 10,
                            visitor: 20,
                            time_override: Some(0),
                            ..RoomConfig::default()
                        };
                        let rl = room_line_with_vfs(&vfs, &gr, &cfg, &opts);
                        lines.push(format!("{}:{}:{}:{}", clan as u8, mid, mode, rl));
                    }
                }
            }
        }
        assert_eq!(lines.len(), ROOM_LINE_COUNT);
        assert_eq!(sha(&lines), ROOM_LINES_SHA);
    }

    // DC-DEFAULT: GMD-2 (clarification) with the default policy every offered room line has a non-zero time except
    // the co-op class (whose mode files carry their own limits); lines differ from the no-fallback form only in
    // `time=` and only where the mode file gave none
    #[test]
    fn rom_room_lines_time_fallback() {
        let Some(rom) = rom() else { return };
        let vfs = RezVfs::new(&rom);
        let gr = GameRoomData::from_vfs(&vfs).expect("load GAMEROOM.TXT");
        let with = RoomOptions::default();
        let without = RoomOptions {
            round_clock: RoundTimePolicy::none(),
            ..RoomOptions::default()
        };
        let time_of = |l: &str| -> i32 {
            l.split_whitespace()
                .find_map(|t| t.strip_prefix("time="))
                .and_then(|t| t.parse().ok())
                .expect("time= token")
        };
        let mut filled = 0usize;
        for clan in [false, true] {
            let modes = gr.modes(clan);
            for (mid, mask) in gr.map_mode_masks(clan) {
                for mode in 0..16 {
                    if (mask & (1u32 << mode)) == 0 {
                        continue;
                    }
                    let cfg = RoomConfig {
                        map: mid,
                        mode,
                        clan,
                        goal: gr.default_goal(mode, clan),
                        ..RoomConfig::default()
                    };
                    let a = room_line_with_vfs(&vfs, &gr, &cfg, &with);
                    let b = room_line_with_vfs(&vfs, &gr, &cfg, &without);
                    let class = modes
                        .get(mode)
                        .and_then(|m| m.get("GameType"))
                        .and_then(|t| t.trim().parse::<i32>().ok())
                        .unwrap_or(1);
                    let (ta, tb) = (time_of(&a), time_of(&b));
                    if tb != 0 {
                        assert_eq!(a, b, "a mode file limit is kept");
                    } else {
                        assert_eq!(ta, RoundTimePolicy::default().secs_for(class), "{a}");
                        assert_eq!(
                            a.replacen(&format!(" time={ta} "), " time=0 ", 1),
                            b,
                            "only time= differs"
                        );
                        filled += usize::from(ta != 0);
                    }
                    if class != 5 {
                        assert_ne!(ta, 0, "every room has a time limit: {a}");
                    }
                }
            }
        }
        assert!(
            filled > 0,
            "the ROM has mode files without a time limit (TDM)"
        );
    }

    #[test]
    fn rom_shop_items_and_weapons_tables() {
        let Some(rom) = rom() else { return };
        let vfs = RezVfs::new(&rom);
        let shop = ShopCatalog::from_vfs(&vfs).expect("load SHOP_ITEM_LIST.TXT");
        let view = shop.view_list();
        assert_eq!(view.len(), SHOP_VIEW_COUNT);
        let ids: Vec<String> = view.iter().map(|i| i.to_string()).collect();
        assert_eq!(sha(&ids), SHOP_VIEW_SHA);
        assert!(view.iter().all(|&pid| shop
            .product(pid)
            .map(|p| p.items.len() <= MAX_PRODUCT_ITEMS)
            .unwrap_or(false)));

        // The flags policy runs over the real catalogue with empty server data and yields nothing.
        let flags = ShopFlagsPolicy::default().evaluate(&ShopFlagInputs {
            catalogue: &shop,
            rom_build_day: day_number(2009, 1, 1).unwrap(),
            first_seen_day: &HashMap::new(),
            sales: &HashMap::new(),
            operator_sale: &HashSet::new(),
        });
        assert_eq!(flags, ShopFlags::default());

        let items = ItemsTable::from_vfs(&vfs).expect("load ITEMS.TXT");
        assert!(!items.doc().is_empty());
        assert!(!items.use_items.is_empty());
        assert!(!items.backpack_slots.is_empty());
        let weapons = WeaponsTable::from_vfs(&vfs).expect("load WEAPONS.TXT");
        assert!(!weapons.doc().is_empty());
        assert!(!weapons.option_weapon_compatible(1000, 9999));
    }

    // Pinned digests and counts of the user's ROM (filled from a first run; hashes only).
    const NORMAL_MASK_COUNT: usize = 19;
    const CLAN_MASK_COUNT: usize = 17;
    const NORMAL_MASK_SHA: &str =
        "b30a9659401582b7675c269d82f65d15b8f817361143920da21182ea2dd1cf29";
    const CLAN_MASK_SHA: &str = "a5058d812af869d949c1956898f285335d2a403c2a3f73339b3c1238b6cdbfb5";
    const WORLD_COUNT: usize = 21;
    const WORLD_NAMES_SHA: &str =
        "2bb25e4b458f483b06a79020a376ed284a824f26799863b8514bdd826283202f";
    const MATRIX_ROWS: usize = 138;
    const ROOM_LINE_COUNT: usize = 136;
    const ROOM_LINES_SHA: &str = "0c21e124aa5dec05688070ab161f790e342ea2c22ae9d93d87788622562b6236";
    const SHOP_VIEW_COUNT: usize = 1027;
    const SHOP_VIEW_SHA: &str = "1ae084277176d1e8403435608a709a6577f2273070f3c58e2d1a92bd9557c900";
}
