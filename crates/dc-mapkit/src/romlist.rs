//! `dc-mapkit rom textures|sounds|sprites|surfaces|worlds|modes`: ROM asset listings at RUN TIME only
//! (MAPKIT_PLAN.md 5.2; CLEANROOM §4: nothing of the ROM is ever written into the tree), and the ROM discovery
//! rule every verb shares.
//!
//! Discovery (DC-DEFAULT: MAP-54): `--rom` > `CA_ROM` > `rom = "..."` in `<profile>/mapkit.toml` (the profile
//! folder is the client's own rule, CLI-553: `DC_PROFILE_DIR`, else `%APPDATA%\DeCombat`, else
//! `$XDG_CONFIG_HOME/decombat`, else the platform folder under `$HOME`) > a sibling-folder scan next to the
//! executable for a folder holding `Game/` and `Engine.exe` (the friend package's rule: the exe's own folder,
//! its parent, then its siblings in name order) > none. [`discover_rom`] applies it to the real environment;
//! [`discover_rom_with`] is the pure core the tests drive with their own environment and folders.
//!
//! Listings (DC-DEFAULT: MAP-55): `textures` (`.dtx`), `sounds` (`.wav`), `sprites` (`.spr`) and `worlds`
//! (`.dat`) walk every `.rez` archive under `<rom>/Game` (name order) through `dc_rez::RezArchive` and print the
//! entry paths exactly as the user's archives spell them (the first path component is the archive stem);
//! `surfaces` reads the ROM surface table through `dc_bute` and lists the row name with its id; `modes` lists
//! DeCombat's own mode names (`dc_config::modes::MODE_NAMES`) with the ROM mode class. `--filter` keeps the rows
//! whose path contains the text (ASCII case-insensitive). `--thumbs DIR` writes a PNG thumbnail per texture or
//! sprite (frame 0's texture) under `DIR/<rom digest>/<path>.png`, only when the file is not there yet, and
//! fills `width` / `height` / `thumb`; the digest is SHA-256 over the names and sizes of the archives, so a
//! different ROM copy gets its own cache folder.

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::thumbs;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RomKind {
    Textures,
    Sounds,
    Sprites,
    Surfaces,
    Worlds,
    Modes,
}

impl RomKind {
    pub const ALL: [RomKind; 6] = [
        RomKind::Textures,
        RomKind::Sounds,
        RomKind::Sprites,
        RomKind::Surfaces,
        RomKind::Worlds,
        RomKind::Modes,
    ];

    pub fn parse(s: &str) -> Option<RomKind> {
        Some(match s {
            "textures" => RomKind::Textures,
            "sounds" => RomKind::Sounds,
            "sprites" => RomKind::Sprites,
            "surfaces" => RomKind::Surfaces,
            "worlds" => RomKind::Worlds,
            "modes" => RomKind::Modes,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            RomKind::Textures => "textures",
            RomKind::Sounds => "sounds",
            RomKind::Sprites => "sprites",
            RomKind::Surfaces => "surfaces",
            RomKind::Worlds => "worlds",
            RomKind::Modes => "modes",
        }
    }

    /// The entry extension of an archive listing (`None` for the table listings).
    fn extension(self) -> Option<&'static str> {
        match self {
            RomKind::Textures => Some("dtx"),
            RomKind::Sounds => Some("wav"),
            RomKind::Sprites => Some("spr"),
            RomKind::Worlds => Some("dat"),
            RomKind::Surfaces | RomKind::Modes => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomQuery {
    pub kind: RomKind,
    pub filter: Option<String>,
    /// Write PNG thumbnails of textures / sprites into this folder.
    pub thumbs: Option<PathBuf>,
}

/// One listing row: the archive-relative path (or the surface row / mode name) and what is known about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RomEntry {
    pub path: String,
    /// Entry bytes (archive listings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Surface id / mode class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    /// A surface row's own name, read from the ROM (MAP-91).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether a shot leaves a bullet hole on this surface, read from the ROM (MAP-111).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marks: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
}

impl RomEntry {
    fn path_only(path: String) -> RomEntry {
        RomEntry {
            path,
            size: None,
            id: None,
            name: None,
            marks: None,
            width: None,
            height: None,
            thumb: None,
        }
    }
}

/// Where the ROM path came from (`info --json` reports it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RomSource {
    Flag,
    Env,
    Profile,
    Sibling,
}

/// The four attempts, for the error message of a verb that needs a ROM.
pub const ROM_ATTEMPTS: &str =
    "--rom <dir>, CA_ROM, <profile>/mapkit.toml `rom`, a sibling Game/ folder with Engine.exe";

/// The profile file that remembers the ROM (`rom = "<folder>"`).
pub const PROFILE_FILE: &str = "mapkit.toml";

/// DeCombat's own profile folder from an environment reader (the client's rule, CLI-553: `DC_PROFILE_DIR`, else
/// `%APPDATA%\DeCombat`, else `$XDG_CONFIG_HOME/decombat`, else `~/Library/Application Support/DeCombat` on
/// macOS and `~/.config/decombat` elsewhere).
// DC-DEFAULT: CLI-553 (the client's `own_profile_dir`), MAP-54
pub fn profile_dir_with(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let var = |k: &str| env(k).filter(|d| !d.is_empty());
    if let Some(d) = var("DC_PROFILE_DIR") {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = var("APPDATA") {
        return Some(PathBuf::from(d).join("DeCombat"));
    }
    if let Some(d) = var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(d).join("decombat"));
    }
    let home = PathBuf::from(var("HOME")?);
    Some(if cfg!(any(target_os = "macos", target_os = "ios")) {
        home.join("Library")
            .join("Application Support")
            .join("DeCombat")
    } else {
        home.join(".config").join("decombat")
    })
}

/// The profile folder of the real environment.
pub fn profile_dir() -> Option<PathBuf> {
    profile_dir_with(&|k: &str| std::env::var_os(k))
}

/// The `rom` value of a `mapkit.toml` text: the first `rom = "<path>"` line (a TOML basic string; a bare value
/// is accepted too), `None` when absent.
pub fn profile_rom_of(text: &str) -> Option<PathBuf> {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((k, val)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != "rom" {
            continue;
        }
        let val = val.trim();
        let val = val.split_once(" #").map_or(val, |(v, _)| v).trim();
        let val = val
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .map(|v| v.replace("\\\\", "\\"))
            .unwrap_or_else(|| val.to_string());
        if val.is_empty() {
            return None;
        }
        return Some(PathBuf::from(val));
    }
    None
}

/// Write `rom = "<path>"` into `<profile>/mapkit.toml` (other lines kept), so the next run finds it.
pub fn save_profile_rom(profile: &Path, rom: &Path) -> std::io::Result<PathBuf> {
    let file = profile.join(PROFILE_FILE);
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    let mut out = String::new();
    for line in old.lines() {
        let t = line.trim();
        if t.split_once('=').is_some_and(|(k, _)| k.trim() == "rom") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    let text = rom
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "");
    out.push_str(&format!("rom = \"{text}\"\n"));
    crate::project::write_atomic(&file, out.as_bytes())?;
    Ok(file)
}

/// True for a 2009 client folder: `Game/` and `Engine.exe` inside (the friend package's rule).
pub fn looks_like_rom(dir: &Path) -> bool {
    dir.join("Game").is_dir() && dir.join("Engine.exe").is_file()
}

/// The sibling scan: the exe's folder, its parent, then the parent's other folders in name order.
pub fn sibling_rom(exe_dir: &Path) -> Option<PathBuf> {
    if looks_like_rom(exe_dir) {
        return Some(exe_dir.to_path_buf());
    }
    let parent = exe_dir.parent()?;
    if looks_like_rom(parent) {
        return Some(parent.to_path_buf());
    }
    let mut names: Vec<PathBuf> = std::fs::read_dir(parent)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p != exe_dir)
        .collect();
    names.sort();
    names.into_iter().find(|p| looks_like_rom(p))
}

/// The discovery rule over a given environment reader and executable folder (MAP-54).
pub fn discover_rom_with(
    flag: Option<&Path>,
    env: &dyn Fn(&str) -> Option<OsString>,
    exe_dir: Option<&Path>,
) -> Option<(PathBuf, RomSource)> {
    if let Some(p) = flag {
        return Some((p.to_path_buf(), RomSource::Flag));
    }
    if let Some(p) = env("CA_ROM").filter(|p| !p.is_empty()) {
        return Some((PathBuf::from(p), RomSource::Env));
    }
    if let Some(profile) = profile_dir_with(env) {
        if let Ok(text) = std::fs::read_to_string(profile.join(PROFILE_FILE)) {
            if let Some(p) = profile_rom_of(&text) {
                if p.is_dir() {
                    return Some((p, RomSource::Profile));
                }
            }
        }
    }
    if let Some(exe) = exe_dir {
        if let Some(p) = sibling_rom(exe) {
            return Some((p, RomSource::Sibling));
        }
    }
    None
}

/// The ROM folder and how it was found, or `None` (the verbs that need one fail naming the attempts).
pub fn discover_rom(flag: Option<&Path>) -> Option<(PathBuf, RomSource)> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    discover_rom_with(flag, &|k: &str| std::env::var_os(k), exe_dir.as_deref())
}

/// The diagnostic of a verb that needs a ROM and found none.
pub fn no_rom() -> Diagnostics {
    Diagnostics::one(
        Diagnostic::new(
            Code::RomMissing,
            "rom",
            format!("no ROM found (tried {ROM_ATTEMPTS})"),
        )
        .with_fix(
            "pass --rom <dir>, set CA_ROM, or write rom = \"<dir>\" into <profile>/mapkit.toml",
        ),
    )
}

// ---------------------------------------------------------------------------------------------------------------
// listings

/// The archive files of a ROM (`<rom>/Game/*.rez`, name order).
pub fn archive_files(rom: &Path) -> Result<Vec<PathBuf>, Diagnostics> {
    let game = rom.join("Game");
    let rd = std::fs::read_dir(&game).map_err(|e| {
        Diagnostics::one(
            Diagnostic::new(Code::RomRead, game.display().to_string(), format!("{e}"))
                .with_fix("the ROM folder is the one holding Engine.exe and Game/"),
        )
    })?;
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rez")))
        .collect();
    files.sort();
    Ok(files)
}

/// A short digest of a ROM copy: SHA-256 over the archive names and sizes (the thumbnail cache key).
pub fn rom_digest(rom: &Path) -> Result<String, Diagnostics> {
    let mut h = dc_rom::Sha256::new();
    for f in archive_files(rom)? {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let len = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
        h.update(name.as_bytes());
        h.update(&[0]);
        h.update(len.to_string().as_bytes());
        h.update(b"\n");
    }
    let d = h.finalize();
    Ok(d.iter().take(8).map(|b| format!("{b:02x}")).collect())
}

fn has_extension(path: &str, ext: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, e)| e.eq_ignore_ascii_case(ext))
}

fn passes(filter: Option<&str>, path: &str) -> bool {
    match filter {
        None => true,
        Some(f) => path.to_ascii_lowercase().contains(&f.to_ascii_lowercase()),
    }
}

/// The ROM table archive and the surface table entry inside it (the client's own constants, sound.rs).
// OBSERVED: the surface table is an entry of the attribute archive, rows `[Surface<n>]` with an id key
const TABLE_REZ: &str = "ATTRIBUTES.rez";
const SURFACE_ENTRY: &str = "ATTRIBUTES\\SURFACE.TXT";
const SURFACE_ROW_PREFIX: &str = "Surface";
/// The id key of a surface row, matched by `sha256(key)[..16]` and length (the client's rule).
const KEY_SURFACE_ID: (&str, usize) = ("12156e921bdb6085", 2);
/// The name key of a surface row, matched the same way (MAP-91).
const KEY_SURFACE_NAME: (&str, usize) = ("dcd1d5223f73b3a9", 4);

fn key_matches(key: &str, want: (&str, usize)) -> bool {
    key.len() == want.1 && dc_rom::sha256_hex(key.as_bytes()).starts_with(want.0)
}

fn surfaces(rom: &Path, filter: Option<&str>) -> Result<Vec<RomEntry>, Diagnostics> {
    let vfs = dc_rez::RezVfs::new(rom);
    let doc = dc_bute::ButeDoc::try_from_vfs(&vfs, TABLE_REZ, SURFACE_ENTRY)
        .map_err(|e| Diagnostics::one(Diagnostic::new(Code::RomRead, "surfaces", e.to_string())))?
        .ok_or_else(|| {
            Diagnostics::one(Diagnostic::new(
                Code::RomRead,
                "surfaces",
                "the ROM has no surface table",
            ))
        })?;
    let mut rows = Vec::new();
    for (name, sec) in doc.iter() {
        if !name.starts_with(SURFACE_ROW_PREFIX) {
            continue;
        }
        // DC-DEFAULT: MAP-91 the row's own name is what a mapper picks by, so it is listed and filtered on
        let label = sec
            .iter()
            .find(|(k, _)| key_matches(k, KEY_SURFACE_NAME))
            .map(|(_, val)| val.trim().to_string())
            .filter(|t| !t.is_empty());
        if !passes(filter, name) && !label.as_deref().is_some_and(|l| passes(filter, l)) {
            continue;
        }
        let id = sec
            .iter()
            .find(|(k, _)| key_matches(k, KEY_SURFACE_ID))
            .and_then(|(_, val)| {
                let t = val.trim();
                t.strip_suffix(['f', 'F']).unwrap_or(t).parse::<f32>().ok()
            })
            .map(|f| f.max(0.0) as u32);
        let field = |key: &str| {
            sec.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, val)| val.trim())
                .filter(|t| !t.is_empty())
        };
        let shown = field("ShowsMark").is_some_and(|v| v != "0");
        rows.push(RomEntry {
            path: name.to_string(),
            id,
            name: label,
            marks: Some(shown && field("BulletHoleSpr").is_some()),
            ..RomEntry::path_only(String::new())
        });
    }
    rows.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.path.cmp(&b.path)));
    Ok(rows)
}

fn modes(filter: Option<&str>) -> Vec<RomEntry> {
    dc_config::modes::MODE_NAMES
        .iter()
        .filter(|m| passes(filter, m))
        .map(|m| RomEntry {
            path: m.to_string(),
            id: dc_config::modes::mode_class_of_name(m),
            ..RomEntry::path_only(String::new())
        })
        .collect()
}

/// The archive that holds an entry path: its first component names `<top>.rez` (every ROM archive's entries
/// start with the archive stem).
fn archive_of<'a>(
    archives: &'a [(String, dc_rez::RezArchive)],
    path: &str,
) -> Option<&'a dc_rez::RezArchive> {
    let top = path.split(['\\', '/']).next()?.to_ascii_uppercase();
    archives
        .iter()
        .find(|(stem, _)| *stem == top)
        .map(|(_, a)| a)
}

/// The cache file of an entry's thumbnail: `<dir>/<digest>/<path with / separators>.png`.
fn thumb_path(dir: &Path, digest: &str, entry: &str) -> PathBuf {
    let mut p = dir.join(digest);
    for seg in entry.split(['\\', '/']) {
        // letters, digits, `.`, `_`, `-` only, and never two dots in a row (no `..` segment can form)
        let mut clean = String::with_capacity(seg.len());
        for c in seg.chars() {
            let ok = c.is_ascii_alphanumeric()
                || c == '_'
                || c == '-'
                || (c == '.' && !clean.ends_with('.'));
            clean.push(if ok { c } else { '_' });
        }
        if clean.is_empty() {
            continue;
        }
        p.push(clean);
    }
    p.set_extension("png");
    p
}

/// List one kind of ROM asset.
pub fn list(rom: &Path, query: &RomQuery) -> Result<Vec<RomEntry>, Diagnostics> {
    let filter = query.filter.as_deref();
    let Some(ext) = query.kind.extension() else {
        return match query.kind {
            RomKind::Surfaces => surfaces(rom, filter),
            _ => Ok(modes(filter)),
        };
    };
    let mut diags = Diagnostics::new();
    let mut archives: Vec<(String, dc_rez::RezArchive)> = Vec::new();
    for f in archive_files(rom)? {
        let stem = f
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_uppercase())
            .unwrap_or_default();
        match dc_rez::RezArchive::open(&f) {
            Ok(a) => archives.push((stem, a)),
            Err(e) => diags.warn(Code::RomRead, f.display().to_string(), e.to_string()),
        }
    }
    let mut rows: Vec<RomEntry> = Vec::new();
    for (_, a) in &archives {
        for (path, _, size) in a.entries() {
            if has_extension(path, ext) && passes(filter, path) {
                rows.push(RomEntry {
                    size: Some(*size as u64),
                    ..RomEntry::path_only(path.clone())
                });
            }
        }
    }
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    let thumbs = match (&query.thumbs, query.kind) {
        (Some(dir), RomKind::Textures | RomKind::Sprites) => Some(dir),
        _ => None,
    };
    if let Some(dir) = thumbs {
        let digest = rom_digest(rom)?;
        let mut known: BTreeMap<String, Option<(u32, u32)>> = BTreeMap::new();
        for row in &mut rows {
            let file = thumb_path(dir, &digest, &row.path);
            let (dtx_path, dtx_archive) = if query.kind == RomKind::Sprites {
                let Some(a) = archive_of(&archives, &row.path) else {
                    continue;
                };
                let Ok(Some(bytes)) = a.entry_raw(&row.path) else {
                    continue;
                };
                let Some(frame) = thumbs::sprite_first_frame(&bytes) else {
                    continue;
                };
                let frame = String::from_utf8_lossy(frame).into_owned();
                let Some(fa) = archive_of(&archives, &frame) else {
                    continue;
                };
                (frame, fa)
            } else {
                let Some(a) = archive_of(&archives, &row.path) else {
                    continue;
                };
                (row.path.clone(), a)
            };
            if file.is_file() {
                row.thumb = Some(file.display().to_string());
                if let Some(Some((w, h))) = known.get(&dtx_path.to_ascii_uppercase()) {
                    row.width = Some(*w);
                    row.height = Some(*h);
                } else if let Ok(Some(bytes)) = dtx_archive.entry_raw(&dtx_path) {
                    if let Ok(f) = dc_tex::DtxFile::parse(&bytes) {
                        row.width = Some(f.header().width as u32);
                        row.height = Some(f.header().height as u32);
                    }
                }
                continue;
            }
            let Ok(Some(bytes)) = dtx_archive.entry_raw(&dtx_path) else {
                continue;
            };
            match thumbs::dtx_thumb_png(&bytes, thumbs::THUMB_SIZE) {
                Ok((png, (w, h))) => {
                    if let Err(e) = crate::project::write_atomic(&file, &png) {
                        diags.warn(Code::ProjectIo, file.display().to_string(), e.to_string());
                        continue;
                    }
                    known.insert(dtx_path.to_ascii_uppercase(), Some((w, h)));
                    row.width = Some(w);
                    row.height = Some(h);
                    row.thumb = Some(file.display().to_string());
                }
                Err(_) => {
                    known.insert(dtx_path.to_ascii_uppercase(), None);
                }
            }
        }
    }
    if diags.has_errors() {
        return Err(diags);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).map(OsString::from)
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dc-mapkit-rom-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_flag_wins_over_the_environment() {
        let env = env_of(&[("CA_ROM", "/y")]);
        let (p, s) = discover_rom_with(Some(Path::new("/x")), &env, None).unwrap();
        assert_eq!(p, PathBuf::from("/x"));
        assert_eq!(s, RomSource::Flag);
        let (p, s) = discover_rom_with(None, &env, None).unwrap();
        assert_eq!((p, s), (PathBuf::from("/y"), RomSource::Env));
        assert_eq!(RomKind::parse("textures"), Some(RomKind::Textures));
        assert_eq!(RomKind::parse("x"), None);
        for k in RomKind::ALL {
            assert_eq!(RomKind::parse(k.name()), Some(k));
        }
    }

    #[test]
    fn the_profile_file_then_the_sibling_scan_then_none() {
        let root = tmp("discover");
        let profile = root.join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        let rom = root.join("my_rom");
        std::fs::create_dir_all(rom.join("Game")).unwrap();
        std::fs::write(rom.join("Engine.exe"), b"x").unwrap();
        let env = env_of(&[("DC_PROFILE_DIR", profile.to_str().unwrap())]);
        // nothing anywhere
        assert_eq!(discover_rom_with(None, &env, None), None);
        // a profile file naming a folder that does not exist is skipped
        std::fs::write(profile.join(PROFILE_FILE), "rom = \"/nowhere/at/all\"\n").unwrap();
        assert_eq!(discover_rom_with(None, &env, None), None);
        // the sibling scan: the exe lives in <root>/app, the ROM is <root>/my_rom
        let app = root.join("app");
        std::fs::create_dir_all(&app).unwrap();
        let (p, s) = discover_rom_with(None, &env, Some(&app)).unwrap();
        assert_eq!((p, s), (rom.clone(), RomSource::Sibling));
        // the profile file wins over the sibling scan once it names a real folder
        let f = save_profile_rom(&profile, &rom).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert_eq!(profile_rom_of(&text), Some(rom.clone()));
        let (p, s) = discover_rom_with(None, &env, Some(&app)).unwrap();
        assert_eq!((p, s), (rom.clone(), RomSource::Profile));
        // CA_ROM wins over the profile
        let env2 = env_of(&[
            ("DC_PROFILE_DIR", profile.to_str().unwrap()),
            ("CA_ROM", "/from/env"),
        ]);
        assert_eq!(
            discover_rom_with(None, &env2, Some(&app)),
            Some((PathBuf::from("/from/env"), RomSource::Env))
        );
        // other lines of the profile file survive a save
        std::fs::write(
            profile.join(PROFILE_FILE),
            "# mine\nother = 1\nrom = \"/old\"\n",
        )
        .unwrap();
        save_profile_rom(&profile, &rom).unwrap();
        let text = std::fs::read_to_string(profile.join(PROFILE_FILE)).unwrap();
        assert!(text.contains("# mine\nother = 1\n"));
        assert_eq!(text.matches("rom = ").count(), 1);
        assert_eq!(
            profile_rom_of("rom = \"C:\\\\Games\\\\CA\" # note\n"),
            Some(PathBuf::from("C:\\Games\\CA"))
        );
        assert_eq!(
            profile_rom_of("rom = /plain/path\n"),
            Some(PathBuf::from("/plain/path"))
        );
        assert_eq!(profile_rom_of("# rom = \"/x\"\n"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_profile_folder_follows_the_clients_rule() {
        let d = profile_dir_with(&env_of(&[("DC_PROFILE_DIR", "/p"), ("APPDATA", "/a")])).unwrap();
        assert_eq!(d, PathBuf::from("/p"));
        let d = profile_dir_with(&env_of(&[("APPDATA", "/a"), ("HOME", "/h")])).unwrap();
        assert_eq!(d, PathBuf::from("/a/DeCombat"));
        let d = profile_dir_with(&env_of(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")])).unwrap();
        assert_eq!(d, PathBuf::from("/x/decombat"));
        let d = profile_dir_with(&env_of(&[("HOME", "/h")])).unwrap();
        assert!(d.starts_with("/h"));
        assert_eq!(profile_dir_with(&env_of(&[])), None);
    }

    #[test]
    fn modes_list_without_a_rom_and_a_missing_rom_names_the_attempts() {
        let rows = list(
            Path::new("/nowhere"),
            &RomQuery {
                kind: RomKind::Modes,
                filter: Some("clan".into()),
                thumbs: None,
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 4);
        assert!(rows
            .iter()
            .all(|r| r.path.ends_with("_CLAN") && r.id.is_some()));
        let e = list(
            Path::new("/nowhere"),
            &RomQuery {
                kind: RomKind::Textures,
                filter: None,
                thumbs: None,
            },
        )
        .unwrap_err();
        assert_eq!(e.items[0].code, Code::RomRead);
        assert_eq!(no_rom().items[0].code, Code::RomMissing);
        assert!(no_rom().items[0].message.contains("CA_ROM"));
    }

    #[test]
    fn thumb_paths_stay_under_the_digest_folder() {
        let p = thumb_path(Path::new("/t"), "abcd", "TOP\\a b\\c.dtx");
        assert_eq!(p, PathBuf::from("/t/abcd/TOP/a_b/c.png"));
        let p = thumb_path(Path::new("/t"), "abcd", "x/..;/y.spr");
        assert!(!p.to_string_lossy().contains(".."));
    }

    /// ROM-gated: every listing kind answers on the user's own ROM, the paths come from the archives, and the
    /// thumbnail cache fills lazily (second run writes nothing new).
    #[test]
    fn rom_listings_and_thumbnails_on_the_users_rom() {
        let Some(rom) = dc_rom::test_rom() else {
            eprintln!("skipped: CA_ROM unset");
            return;
        };
        let q = |kind, filter: Option<&str>, thumbs: Option<PathBuf>| RomQuery {
            kind,
            filter: filter.map(String::from),
            thumbs,
        };
        let worlds = list(&rom, &q(RomKind::Worlds, None, None)).unwrap();
        assert!(!worlds.is_empty());
        assert!(worlds
            .iter()
            .all(|r| has_extension(&r.path, "dat") && r.size.is_some()));
        let textures = list(&rom, &q(RomKind::Textures, None, None)).unwrap();
        assert!(textures.len() > 1000);
        assert!(
            textures.windows(2).all(|w| w[0].path <= w[1].path),
            "sorted"
        );
        let sounds = list(&rom, &q(RomKind::Sounds, None, None)).unwrap();
        assert!(!sounds.is_empty());
        let sprites = list(&rom, &q(RomKind::Sprites, None, None)).unwrap();
        assert!(!sprites.is_empty());
        let surfaces = list(&rom, &q(RomKind::Surfaces, None, None)).unwrap();
        assert!(surfaces.len() > 3);
        assert!(surfaces.iter().any(|r| r.id.is_some()));
        // a filter narrows by substring, case-insensitively: the first texture's own folder
        let folder = textures[0]
            .path
            .rsplit_once('\\')
            .map(|(d, _)| d.to_string())
            .unwrap();
        let some = list(
            &rom,
            &q(RomKind::Textures, Some(&folder.to_ascii_lowercase()), None),
        )
        .unwrap();
        assert!(!some.is_empty() && some.len() <= textures.len());
        // thumbnails of that folder, written once
        let dir = tmp("thumbs");
        let first = list(
            &rom,
            &q(RomKind::Textures, Some(&folder), Some(dir.clone())),
        )
        .unwrap();
        let with = first.iter().filter(|r| r.thumb.is_some()).count();
        assert!(with > 0, "at least one texture of the folder decodes");
        let digest = rom_digest(&rom).unwrap();
        assert_eq!(digest.len(), 16);
        let t = first.iter().find(|r| r.thumb.is_some()).unwrap();
        let png = std::fs::read(t.thumb.as_ref().unwrap()).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert!(img.width() <= thumbs::THUMB_SIZE && img.height() <= thumbs::THUMB_SIZE);
        assert!(t.width.is_some() && t.height.is_some());
        let stamp = std::fs::metadata(t.thumb.as_ref().unwrap())
            .unwrap()
            .modified()
            .unwrap();
        let second = list(
            &rom,
            &q(RomKind::Textures, Some(&folder), Some(dir.clone())),
        )
        .unwrap();
        assert_eq!(
            second, first,
            "the second run reads the cache and reports the same rows"
        );
        assert_eq!(
            std::fs::metadata(t.thumb.as_ref().unwrap())
                .unwrap()
                .modified()
                .unwrap(),
            stamp
        );
        // a sprite thumbnail goes through its first frame
        let sdir = tmp("sprite-thumbs");
        let sp = list(&rom, &q(RomKind::Sprites, None, Some(sdir.clone()))).unwrap();
        assert!(sp.iter().any(|r| r.thumb.is_some()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&sdir);
    }
}
