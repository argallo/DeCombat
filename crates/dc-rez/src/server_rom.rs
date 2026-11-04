//! Builder of a server ROM (`docs/SERVER_ROM.md`, layout in `dc_rom::server_rom`): copies the parts of the user's
//! client folder that the lobby and the game server read into a plain folder, so a host machine does not need the
//! whole client.
//!
//! What goes in (DC-DEFAULT: DSV-370, see DC_DEFAULTS.md):
//! - every archive's full entry list (names and sizes: the game server looks names up by hash across all archives
//!   and announces resource sizes);
//! - whole: every text table (`.txt`, `.gms`, `.bms`, decrypted), every entry of the world archive, every effect
//!   list (`.fxf`), every model file (`.ltb`) of the body model and the third-person weapon model archives, and of
//!   the first-person weapon model archives ([`VIEW_WEAPON_ARCHIVES`]: the bot reads its weapons' fire cycle from
//!   their clips, BOT-82);
//! - head only: every wave file (`.wav`) up to and including its `data` chunk header (the game server reads only
//!   the play time);
//! - the strings image of the shell library (`dc_rom::server_rom::shell_strings_image`);
//! - nothing else: no executable, no texture, no other model, sound or interface file.
// Provenance: ORIGINAL (our own tooling; no 2009 behaviour)

use crate::{io_err, is_text_name, relpath_key, Blowfish, RezArchive, RezError};
use dc_rom::server_rom::{
    entry_path, is_server_rom, shell_strings_image, Manifest, ManifestEntry, MANIFEST,
    SHELL_LIBRARY, SHELL_STRINGS,
};
use dc_rom::{sha256_file, sha256_hex, SENTINEL_FILES};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

/// The archive of the world files.
pub const WORLD_ARCHIVE: &str = "WORLDS.rez";
/// The archive of the body models.
pub const BODY_MODEL_ARCHIVE: &str = "CHARS_M_BODY.rez";
/// The archive of the third-person weapon models (a dropped weapon's model and its animations).
pub const HELD_WEAPON_ARCHIVE: &str = "GUNS_M_HH.rez";
/// The first-person weapon model archives of the 2009 client: `dc-botclient` reads its weapons' fire and postfire
/// clip lengths from their models (BOT-60, BOT-82).
pub const VIEW_WEAPON_ARCHIVES: [&str; 11] = [
    "GUNS_M_PV_AR.rez",
    "GUNS_M_PV_ETC.rez",
    "GUNS_M_PV_LAUNCHER.rez",
    "GUNS_M_PV_MELEE.rez",
    "GUNS_M_PV_MG.rez",
    "GUNS_M_PV_OPTION.rez",
    "GUNS_M_PV_Pistol.rez",
    "GUNS_M_PV_SG.rez",
    "GUNS_M_PV_SMG.rez",
    "GUNS_M_PV_SR.rez",
    "GUNS_M_PV_Throwing.rez",
];

/// Options of [`build`].
#[derive(Clone, Debug, Default)]
pub struct BuildOptions {
    /// Keep only these worlds (file stems, case-insensitive) of the world archive; `None` = every world.
    pub only_worlds: Option<Vec<String>>,
}

/// How much of an entry the server ROM keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// Listed (name and size) only.
    List,
    /// The whole entry.
    Whole,
    /// A wave file's head through its `data` chunk header.
    WaveHead,
}

fn ext_of(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn stem_of(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(base)
        .to_ascii_uppercase()
}

/// The selection rule of the server ROM for one entry of `archive`.
// DC-DEFAULT: DSV-370 (proposed)
pub fn keep_rule(archive: &str, name: &str, opts: &BuildOptions) -> Keep {
    let ext = ext_of(name);
    if is_text_name(name) || ext == "fxf" {
        return Keep::Whole;
    }
    if archive.eq_ignore_ascii_case(WORLD_ARCHIVE) {
        return match &opts.only_worlds {
            Some(list) if ext == "dat" => {
                let stem = stem_of(name);
                if list.iter().any(|w| w.eq_ignore_ascii_case(&stem)) {
                    Keep::Whole
                } else {
                    Keep::List
                }
            }
            _ => Keep::Whole,
        };
    }
    if (archive.eq_ignore_ascii_case(BODY_MODEL_ARCHIVE)
        || archive.eq_ignore_ascii_case(HELD_WEAPON_ARCHIVE)
        || VIEW_WEAPON_ARCHIVES.iter().any(|a| archive.eq_ignore_ascii_case(a)))
        && ext == "ltb"
    {
        return Keep::Whole;
    }
    if ext == "wav" {
        return Keep::WaveHead;
    }
    Keep::List
}

/// The head of a RIFF wave file through the header of its `data` chunk when a `fmt ` chunk came before it; the
/// whole file otherwise (the play time needs the byte rate and the data length).
// Public format knowledge: RIFF / WAVE chunk layout
pub fn wave_head(b: &[u8]) -> &[u8] {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return b;
    }
    let mut fmt = false;
    let mut i = 12usize;
    while i + 8 <= b.len() {
        let n = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        match &b[i..i + 4] {
            b"fmt " => fmt = true,
            b"data" if fmt => return &b[..i + 8],
            _ => {}
        }
        i = (i + 8).saturating_add(n).saturating_add(n & 1);
    }
    b
}

/// What [`build`] did.
#[derive(Clone, Debug, Default)]
pub struct BuildReport {
    /// Bytes of every file in the client folder.
    pub client_bytes: u64,
    /// Files in the client folder.
    pub client_files: u64,
    /// Bytes written (data files and the manifest).
    pub out_bytes: u64,
    /// Data files written (without the manifest).
    pub out_files: u64,
    /// Manifest bytes.
    pub manifest_bytes: u64,
    /// Per archive: entries listed, entries kept whole, entries kept as a head, bytes written.
    pub archives: Vec<(String, usize, usize, usize, u64)>,
}

fn walk_size(dir: &Path, files: &mut u64, bytes: &mut u64) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk_size(&e.path(), files, bytes);
        } else if let Ok(m) = e.metadata() {
            *files += 1;
            *bytes += m.len();
        }
    }
}

fn write_new(p: &Path, bytes: &[u8]) -> Result<(), RezError> {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| io_err(d, &e))?;
    }
    fs::write(p, bytes).map_err(|e| io_err(p, &e))
}

fn fail(m: impl Into<String>) -> RezError {
    RezError::Malformed(format!("server ROM: {}", m.into()))
}

/// Builds a server ROM of the client folder `client` in `out`. `out` must be missing, empty, or an earlier server
/// ROM (which is replaced). The output is a function of the client folder and `opts` only.
pub fn build(client: &Path, out: &Path, opts: &BuildOptions) -> Result<BuildReport, RezError> {
    if is_server_rom(client) {
        return Err(fail(format!(
            "{} is already a server ROM; build from the full client folder",
            client.display()
        )));
    }
    let build_id = dc_rom::identify_rom(client)?;
    let key = dc_rom::crypt_key(client)?;
    let cipher = Blowfish::new(&key)?;
    let sentinels = SENTINEL_FILES
        .iter()
        .map(|rel| sha256_file(&client.join(rel)))
        .collect::<Result<Vec<_>, _>>()?;

    if out.exists() {
        let empty = fs::read_dir(out)
            .map_err(|e| io_err(out, &e))?
            .next()
            .is_none();
        if !empty {
            if !is_server_rom(out) {
                return Err(fail(format!(
                    "{} is not empty and not a server ROM; choose a new folder",
                    out.display()
                )));
            }
            fs::remove_dir_all(out).map_err(|e| io_err(out, &e))?;
        }
    }
    fs::create_dir_all(out).map_err(|e| io_err(out, &e))?;

    let mut report = BuildReport::default();
    walk_size(client, &mut report.client_files, &mut report.client_bytes);

    // the shell library's strings image
    let lib_path = client.join(SHELL_LIBRARY);
    let lib = fs::read(&lib_path).map_err(|e| io_err(&lib_path, &e))?;
    let image = shell_strings_image(&lib);
    write_new(&out.join(SHELL_STRINGS), &image)?;
    let mut files = vec![(
        SHELL_STRINGS.to_string(),
        image.len() as u64,
        sha256_hex(&image),
    )];
    report.out_files += 1;
    report.out_bytes += image.len() as u64;

    // the archives, in byte order of their file names
    let game = client.join("Game");
    let mut names: Vec<String> = fs::read_dir(&game)
        .map_err(|e| io_err(&game, &e))?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.to_ascii_lowercase().ends_with(".rez"))
        .collect();
    names.sort();
    let mut archives = BTreeMap::new();
    for name in names {
        let arch = RezArchive::open(&game.join(&name))?;
        // a full path listed twice: the readers find the last one, so only that one is kept
        let mut last: HashMap<String, usize> = HashMap::new();
        for (i, (n, _, _)) in arch.entries().iter().enumerate() {
            last.insert(relpath_key(n), i);
        }
        let mut rows = Vec::with_capacity(arch.entries().len());
        let (mut whole, mut heads, mut bytes) = (0usize, 0usize, 0u64);
        let mut written: HashMap<PathBuf, ()> = HashMap::new();
        // every archive gets its folder, also with no file kept: the game server lists the `Game` folder to find the
        // archives whose names it searches
        let adir = out.join("Game").join(&name);
        fs::create_dir_all(&adir).map_err(|e| io_err(&adir, &e))?;
        for (i, (n, pos, size)) in arch.entries().iter().enumerate() {
            let mut keep = keep_rule(&name, n, opts);
            if last.get(&relpath_key(n)) != Some(&i) || !safe_entry(n) {
                keep = Keep::List;
            }
            let (sha, head) = match keep {
                Keep::List => (None, None),
                Keep::Whole | Keep::WaveHead => {
                    let raw = arch.read_raw(*pos, *size)?;
                    let data = if is_text_name(n) && !crate::is_plain(&raw) {
                        cipher.decrypt_bytes(&raw)
                    } else {
                        raw
                    };
                    let stored: &[u8] = if keep == Keep::WaveHead {
                        wave_head(&data)
                    } else {
                        &data
                    };
                    let p = entry_path(out, &name, n);
                    let k = PathBuf::from(p.to_string_lossy().to_ascii_uppercase());
                    if written.insert(k, ()).is_some() {
                        return Err(fail(format!("{name}: {n} collides with another entry")));
                    }
                    write_new(&p, stored)?;
                    bytes += stored.len() as u64;
                    report.out_files += 1;
                    let head = (stored.len() < data.len()).then_some(stored.len() as u32);
                    if head.is_some() {
                        heads += 1;
                    } else {
                        whole += 1;
                    }
                    (Some(sha256_hex(stored)), head)
                }
            };
            rows.push(ManifestEntry {
                name: n.clone(),
                size: *size,
                sha256: sha,
                head,
            });
        }
        report.out_bytes += bytes;
        report
            .archives
            .push((name.clone(), rows.len(), whole, heads, bytes));
        archives.insert(name, rows);
    }
    files.sort();

    let manifest = Manifest {
        build: build_id,
        sentinels,
        key,
        shell_source_sha256: sha256_hex(&lib),
        files,
        archives,
    };
    let text = manifest.to_json();
    // written last: a folder is a server ROM only once it is complete
    write_new(&out.join(MANIFEST), text.as_bytes())?;
    report.manifest_bytes = text.len() as u64;
    report.out_bytes += text.len() as u64;
    Ok(report)
}

/// An entry name that maps to a file inside the archive folder.
fn safe_entry(n: &str) -> bool {
    let norm = n.replace('\\', "/");
    !norm.is_empty()
        && !norm.starts_with('/')
        && !norm.contains(':')
        && !norm.contains('\0')
        && norm.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_rules() {
        let o = BuildOptions::default();
        assert_eq!(keep_rule("ATTRIBUTES.rez", "ATTRIBUTES\\ITEMS.TXT", &o), Keep::Whole);
        assert_eq!(keep_rule("GMS.rez", "GMS\\x_M.GMS", &o), Keep::Whole);
        assert_eq!(keep_rule("WORLDS.rez", "WORLDS\\A.DAT", &o), Keep::Whole);
        assert_eq!(keep_rule("CLIENTFX.rez", "CLIENTFX\\CLIENTFX.FXF", &o), Keep::Whole);
        assert_eq!(keep_rule("CHARS_M_BODY.rez", "CHARS_M_BODY\\a.ltb", &o), Keep::Whole);
        assert_eq!(keep_rule("GUNS_M_HH.rez", "GUNS_M_HH\\a.ltb", &o), Keep::Whole);
        assert_eq!(keep_rule("GUNS_M_PV_AR.rez", "GUNS_M_PV_AR\\a.ltb", &o), Keep::Whole);
        assert_eq!(keep_rule("guns_m_pv_sr.rez", "GUNS_M_PV_SR\\a.ltb", &o), Keep::Whole);
        assert_eq!(keep_rule("GUNS_M_PV_AR.rez", "GUNS_M_PV_AR\\a.dtx", &o), Keep::List);
        assert_eq!(keep_rule("SND.rez", "SND\\a.wav", &o), Keep::WaveHead);
        assert_eq!(keep_rule("TEXTURES.rez", "TEXTURES\\a.dtx", &o), Keep::List);
        let only = BuildOptions {
            only_worlds: Some(vec!["a".into()]),
        };
        assert_eq!(keep_rule("WORLDS.rez", "WORLDS\\A.DAT", &only), Keep::Whole);
        assert_eq!(keep_rule("WORLDS.rez", "WORLDS\\B.DAT", &only), Keep::List);
    }

    #[test]
    fn wave_head_stops_after_the_data_header() {
        let mut w = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        w.extend_from_slice(&[0u8; 16]);
        w.extend_from_slice(b"data\x04\0\0\0abcd");
        assert_eq!(wave_head(&w).len(), w.len() - 4);
        // data before fmt: the whole file
        let mut v = b"RIFF\0\0\0\0WAVEdata\x02\0\0\0abfmt \x10\0\0\0".to_vec();
        v.extend_from_slice(&[0u8; 16]);
        assert_eq!(wave_head(&v).len(), v.len());
        assert_eq!(wave_head(b"not a wave"), b"not a wave");
    }

    #[test]
    fn entry_names_that_leave_the_folder_are_listed_only() {
        assert!(safe_entry("A\\B.TXT"));
        assert!(!safe_entry("..\\B.TXT"));
        assert!(!safe_entry("A\\\\B"));
        assert!(!safe_entry("C:\\x"));
    }
}
