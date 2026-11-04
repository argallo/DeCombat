//! Locating, identifying and guarding the user-supplied 2009 client folder (the "ROM"), and reading the
//! archive crypt key out of the user's own `Engine.exe`.
//!
//! The only environment read in this crate that names a ROM is [`resolve_rom`] (the explicit `CA_ROM` resolver);
//! `trace.rs` reads the `DC_ROM_TRACE` measurement switch.

use crate::error::RomError;
use crate::pe::PeFile;
use crate::sha256::{sha256_file, sha256_hex};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------------------------------------
// Locating the ROM
// ---------------------------------------------------------------------------------------------------------

/// Resolves the ROM directory from the `CA_ROM` environment variable. This is the single place the library
/// reads the environment; production code otherwise takes an explicit path.
pub fn resolve_rom() -> Result<PathBuf, RomError> {
    let val = env::var("CA_ROM").map_err(|_| RomError::NotConfigured)?;
    let trimmed = val.trim();
    if trimmed.is_empty() {
        return Err(RomError::NotConfigured);
    }
    let p = PathBuf::from(trimmed);
    if !p.is_dir() {
        return Err(RomError::NotADirectory(p));
    }
    Ok(p)
}

/// For tests: `Some(path)` only when `CA_ROM` is set and names an existing directory.
pub fn test_rom() -> Option<PathBuf> {
    resolve_rom().ok()
}

/// Test helper: evaluates to the ROM path, or prints `skipped: CA_ROM unset` and returns from the calling
/// test function.
#[macro_export]
macro_rules! rom_or_skip {
    () => {
        match $crate::test_rom() {
            Some(p) => p,
            None => {
                println!("skipped: CA_ROM unset");
                return;
            }
        }
    };
}

/// Deprecated shim for older callers: the `CA_ROM` path, or an empty (non-existent) path when unset.
#[deprecated(note = "use resolve_rom() or pass the ROM path explicitly")]
pub fn rom_dir() -> PathBuf {
    resolve_rom().unwrap_or_default()
}

/// Deprecated shim for older test code: walks up from this crate's manifest directory looking for the
/// source checkout root; falls back to the manifest directory itself (no machine-specific path).
#[deprecated(note = "production code must not depend on the source checkout layout")]
pub fn repo_root() -> PathBuf {
    let start = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut cur: &Path = &start;
    for _ in 0..8 {
        if cur
            .join("extracted")
            .join("app")
            .join("client")
            .join("eu-test")
            .is_dir()
        {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(p) if p != cur => cur = p,
            _ => break,
        }
    }
    start
}

// ---------------------------------------------------------------------------------------------------------
// Build identification (pinned hashes only)
// ---------------------------------------------------------------------------------------------------------

/// Files hashed to identify a client build, relative to the ROM root.
pub const SENTINEL_FILES: [&str; 5] = [
    "Engine.exe",
    "Game/CShell.dll",
    "Game/ClientFx.fxd",
    "Game/CRes.dll",
    "ltmsg.dll",
];

/// Known client builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RomId {
    /// The EU test client dated 2009-12-21.
    Eu20091221,
}

/// A pinned build: the SHA-256 of each file in [`SENTINEL_FILES`] (same order).
#[derive(Clone, Copy, Debug)]
pub struct KnownBuild {
    pub id: RomId,
    pub sha256: [&'static str; 5],
}

/// Pinned list of supported builds (hashes only).
pub const KNOWN_BUILDS: &[KnownBuild] = &[KnownBuild {
    id: RomId::Eu20091221,
    sha256: [
        "bb9a1e41b430a09ab8722e1de31f89dc53a00e5a96abbe236f65c4ceddcf574f",
        "2de3e2f4e1a762f0a484f427560acac62742341ff7ccf8d6b1e95cae73cb3b50",
        "bd10e4afda2f01870640f97cee4376750b8e3dee032117db525c128d7a1667b6",
        "62362a78fe5266ccd3d3056f8525fc3591fd550ad10f048cd9ef995626ddc539",
        "884c44e82cc56f2325be6b45de1a778afd38130b472e0471e534cc00672fbdf4",
    ],
}];

/// Hashes every sentinel file under `rom`; a missing file yields `None` for that slot.
fn hash_sentinels(rom: &Path, rels: &[&str]) -> Vec<Option<String>> {
    rels.iter()
        .map(|rel| {
            let p = rom.join(rel);
            if p.is_file() {
                sha256_file(&p).ok()
            } else {
                None
            }
        })
        .collect()
}

/// Identifies which known build `rom` is, by SHA-256 of the five sentinel files.
/// A server ROM (`server_rom`) is identified by its manifest's source hashes after every file it lists matched.
pub fn identify_rom(rom: &Path) -> Result<RomId, RomError> {
    if let Some(m) = crate::server_rom::manifest(rom)? {
        return crate::server_rom::identify(rom, &m);
    }
    identify_among(rom, KNOWN_BUILDS)
}

/// [`identify_rom`] against an explicit build list (used by tests with synthetic files).
pub fn identify_among(rom: &Path, builds: &[KnownBuild]) -> Result<RomId, RomError> {
    if !rom.is_dir() {
        return Err(RomError::NotADirectory(rom.to_path_buf()));
    }
    let actual = hash_sentinels(rom, &SENTINEL_FILES);
    let mut best: Option<Vec<String>> = None;
    for b in builds {
        let mismatched: Vec<String> = SENTINEL_FILES
            .iter()
            .zip(b.sha256.iter())
            .zip(actual.iter())
            .filter(|((_, want), got)| {
                got.as_deref()
                    .map(|g| !g.eq_ignore_ascii_case(want))
                    .unwrap_or(true)
            })
            .map(|((rel, _), _)| (*rel).to_string())
            .collect();
        if mismatched.is_empty() {
            return Ok(b.id);
        }
        if best
            .as_ref()
            .map(|v| mismatched.len() < v.len())
            .unwrap_or(true)
        {
            best = Some(mismatched);
        }
    }
    Err(RomError::UnknownBuild {
        mismatched: best.unwrap_or_else(|| SENTINEL_FILES.iter().map(|s| s.to_string()).collect()),
    })
}

// ---------------------------------------------------------------------------------------------------------
// Pristine guard
// ---------------------------------------------------------------------------------------------------------

const BASELINE_NAME: &str = "rom_baseline.sha256";

/// Resolves `p` to an absolute path even when it (or trailing components) does not exist yet.
fn lenient_canonical(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        env::current_dir().unwrap_or_default().join(p)
    };
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = abs.as_path();
    loop {
        if let Ok(c) = cur.canonicalize() {
            let mut out = c;
            for t in tail.iter().rev() {
                out.push(t);
            }
            return out;
        }
        match (cur.file_name(), cur.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_os_string());
                cur = parent;
            }
            _ => return abs,
        }
    }
}

fn path_inside(child: &Path, parent: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
    let c = norm(child);
    let mut par = norm(parent);
    if !par.ends_with('/') {
        par.push('/');
    }
    (c + "/").starts_with(&par)
}

/// Verifies that the sentinel files of `rom` are unchanged since the first call. On the first run a baseline
/// is written into the caller-supplied `state_dir` (which must lie outside the ROM); later runs refuse with
/// [`RomError::RomChanged`] when any hash differs. Nothing is ever written inside the ROM.
/// A server ROM is guarded by its manifest (which pins every other file's hash).
pub fn guard_pristine(rom: &Path, state_dir: &Path) -> Result<(), RomError> {
    if crate::server_rom::is_server_rom(rom) {
        return guard_pristine_files(rom, state_dir, &[crate::server_rom::MANIFEST]);
    }
    guard_pristine_files(rom, state_dir, &SENTINEL_FILES)
}

/// [`guard_pristine`] over an explicit relative file list (used by tests with synthetic files).
pub fn guard_pristine_files(rom: &Path, state_dir: &Path, rels: &[&str]) -> Result<(), RomError> {
    if !rom.is_dir() {
        return Err(RomError::NotADirectory(rom.to_path_buf()));
    }
    let rom_c = lenient_canonical(rom);
    let state_c = lenient_canonical(state_dir);
    if path_inside(&state_c, &rom_c) {
        return Err(RomError::BadStateDir(
            "state directory must be outside the ROM".to_string(),
        ));
    }

    let mut now = BTreeMap::new();
    let mut missing = Vec::new();
    for (rel, h) in rels.iter().zip(hash_sentinels(rom, rels)) {
        match h {
            Some(h) => {
                now.insert((*rel).to_string(), h);
            }
            None => missing.push((*rel).to_string()),
        }
    }

    let baseline_path = state_c.join(BASELINE_NAME);
    if !baseline_path.exists() {
        if !missing.is_empty() {
            return Err(RomError::RomChanged { changed: missing });
        }
        fs::create_dir_all(&state_c).map_err(|e| RomError::io(&state_c, &e))?;
        let mut text = String::new();
        for (rel, h) in &now {
            text.push_str(&format!("{h}  {rel}\n"));
        }
        let tmp = state_c.join(format!("{BASELINE_NAME}.tmp"));
        fs::write(&tmp, text).map_err(|e| RomError::io(&tmp, &e))?;
        fs::rename(&tmp, &baseline_path).map_err(|e| RomError::io(&baseline_path, &e))?;
        return Ok(());
    }

    let text = fs::read_to_string(&baseline_path).map_err(|e| RomError::io(&baseline_path, &e))?;
    let mut base = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (h, rel) = line
            .split_once("  ")
            .ok_or_else(|| RomError::BadStateDir("corrupt baseline file".to_string()))?;
        base.insert(rel.to_string(), h.to_string());
    }
    let mut changed = missing;
    for (rel, h) in &now {
        match base.get(rel) {
            Some(b) if b == h => {}
            _ => changed.push(rel.clone()),
        }
    }
    for rel in base.keys() {
        if !rels.contains(&rel.as_str()) {
            changed.push(rel.clone());
        }
    }
    changed.sort();
    changed.dedup();
    if changed.is_empty() {
        Ok(())
    } else {
        Err(RomError::RomChanged { changed })
    }
}

// ---------------------------------------------------------------------------------------------------------
// Archive crypt key
// ---------------------------------------------------------------------------------------------------------

/// Where the key string lives in the user's binaries `(relative path, rva)`.
// EVIDENCE: engine+0x22aa48 (primary), cshell+0x3e9abc (fallback); both hold the same NUL-terminated string.
const KEY_SOURCES: [(&str, u32); 2] = [("Engine.exe", 0x22aa48), ("Game/CShell.dll", 0x3e9abc)];

/// SHA-256 of the key string (hash only; the key itself is never stored in this tree).
const KEY_SHA256: &str = "3f2e77d2a01dec346a1886343842b3de697cceb005d7991e34317c31f616befd";

/// Longest key Blowfish accepts.
const MAX_KEY_LEN: usize = 56;

/// Reads the NUL-terminated string at `rva` in `pe` and returns it iff its SHA-256 equals `pinned_hex`.
/// Error messages never contain the string.
pub fn crypt_key_from_pe(pe: &PeFile, rva: u32, pinned_hex: &str) -> Result<Vec<u8>, RomError> {
    let mut key = Vec::new();
    let mut found_nul = false;
    for k in 0..=(MAX_KEY_LEN as u32) {
        let at = rva
            .checked_add(k)
            .ok_or_else(|| RomError::KeyUnavailable("rva overflow".to_string()))?;
        let b = pe
            .rva(at, 1)
            .map_err(|_| RomError::KeyUnavailable("location outside image".to_string()))?[0];
        if b == 0 {
            found_nul = true;
            break;
        }
        key.push(b);
    }
    if !found_nul || key.is_empty() {
        return Err(RomError::KeyUnavailable(
            "no terminated string at location".to_string(),
        ));
    }
    if !sha256_hex(&key).eq_ignore_ascii_case(pinned_hex) {
        return Err(RomError::KeyUnavailable(
            "string at location does not match pinned digest".to_string(),
        ));
    }
    Ok(key)
}

/// Reads the archive crypt key from the user's own `Engine.exe` (falling back to `Game/CShell.dll`) and
/// verifies it against the pinned SHA-256 before returning it.
/// A server ROM carries the key in its manifest (checked against the same pinned digest).
pub fn crypt_key(rom: &Path) -> Result<Vec<u8>, RomError> {
    if let Some(m) = crate::server_rom::manifest(rom)? {
        return if sha256_hex(&m.key).eq_ignore_ascii_case(KEY_SHA256) {
            Ok(m.key.clone())
        } else {
            Err(RomError::KeyUnavailable(
                "server ROM key does not match pinned digest".to_string(),
            ))
        };
    }
    let mut notes = Vec::new();
    for (rel, rva) in KEY_SOURCES {
        let path = rom.join(rel);
        if !path.is_file() {
            notes.push(format!("{rel}: missing"));
            continue;
        }
        match PeFile::from_file(&path).and_then(|pe| crypt_key_from_pe(&pe, rva, KEY_SHA256)) {
            Ok(k) => return Ok(k),
            Err(e) => notes.push(format!("{rel}: {e}")),
        }
    }
    Err(RomError::KeyUnavailable(notes.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::synth_pe;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);
    fn tmp(tag: &str) -> PathBuf {
        let d = env::temp_dir().join(format!(
            "ca_rom_{}_{}_{}",
            tag,
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn crypt_key_synthetic_roundtrip_and_pin() {
        let mut payload = vec![0u8; 16];
        payload.extend_from_slice(b"synthetic-test-key\0");
        let pe = PeFile::from_bytes(synth_pe(0x40_0000, 0x1000, &payload)).unwrap();
        let pin = sha256_hex(b"synthetic-test-key");
        assert_eq!(
            crypt_key_from_pe(&pe, 0x1010, &pin).unwrap(),
            b"synthetic-test-key"
        );
        // wrong pin, wrong location, unterminated
        assert!(crypt_key_from_pe(&pe, 0x1010, &sha256_hex(b"other")).is_err());
        assert!(crypt_key_from_pe(&pe, 0x1000, &pin).is_err());
        assert!(crypt_key_from_pe(&pe, 0x9000, &pin).is_err());
        let long = PeFile::from_bytes(synth_pe(0x40_0000, 0x1000, &[b'a'; 200])).unwrap();
        assert!(crypt_key_from_pe(&long, 0x1000, &pin).is_err());
        // the error never leaks the string
        let msg = crypt_key_from_pe(&pe, 0x1010, &sha256_hex(b"other"))
            .unwrap_err()
            .to_string();
        assert!(!msg.contains("synthetic"));
    }

    #[test]
    fn crypt_key_missing_files_is_typed_error() {
        let d = tmp("nokey");
        assert!(matches!(crypt_key(&d), Err(RomError::KeyUnavailable(_))));
        let _ = fs::remove_dir_all(&d);
    }

    fn write_sentinels(rom: &Path, tag: &str) -> [String; 5] {
        let mut hs: [String; 5] = Default::default();
        for (i, rel) in SENTINEL_FILES.iter().enumerate() {
            let p = rom.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            let body = format!("{tag}-{i}").into_bytes();
            fs::write(&p, &body).unwrap();
            hs[i] = sha256_hex(&body);
        }
        hs
    }

    #[test]
    fn identify_known_unknown_and_missing() {
        let rom = tmp("ident");
        let hs = write_sentinels(&rom, "a");
        let leak = |s: &String| -> &'static str { Box::leak(s.clone().into_boxed_str()) };
        let known = [KnownBuild {
            id: RomId::Eu20091221,
            sha256: [
                leak(&hs[0]),
                leak(&hs[1]),
                leak(&hs[2]),
                leak(&hs[3]),
                leak(&hs[4]),
            ],
        }];
        assert_eq!(identify_among(&rom, &known).unwrap(), RomId::Eu20091221);
        assert!(matches!(
            identify_among(&rom, KNOWN_BUILDS),
            Err(RomError::UnknownBuild { .. })
        ));
        fs::write(rom.join("ltmsg.dll"), b"patched").unwrap();
        match identify_among(&rom, &known) {
            Err(RomError::UnknownBuild { mismatched }) => {
                assert_eq!(mismatched, vec!["ltmsg.dll".to_string()])
            }
            other => panic!("unexpected {other:?}"),
        }
        fs::remove_file(rom.join("Engine.exe")).unwrap();
        assert!(matches!(
            identify_among(&rom, &known),
            Err(RomError::UnknownBuild { .. })
        ));
        let _ = fs::remove_dir_all(&rom);
    }

    #[test]
    fn pinned_build_hashes_are_well_formed() {
        for b in KNOWN_BUILDS {
            for h in b.sha256 {
                assert_eq!(h.len(), 64);
                assert!(h.bytes().all(|c| c.is_ascii_hexdigit()));
            }
        }
        assert_eq!(KEY_SHA256.len(), 64);
    }

    #[test]
    fn guard_creates_baseline_then_detects_change() {
        let rom = tmp("guard_rom");
        let state = tmp("guard_state").join("nested");
        write_sentinels(&rom, "g");
        guard_pristine(&rom, &state).unwrap();
        assert!(state.join(BASELINE_NAME).is_file());
        // nothing was written into the ROM
        assert!(fs::read_dir(&rom)
            .unwrap()
            .all(|e| e.unwrap().file_name() != BASELINE_NAME));
        guard_pristine(&rom, &state).unwrap();
        fs::write(rom.join("Game/CRes.dll"), b"tampered").unwrap();
        match guard_pristine(&rom, &state) {
            Err(RomError::RomChanged { changed }) => {
                assert_eq!(changed, vec!["Game/CRes.dll".to_string()])
            }
            other => panic!("unexpected {other:?}"),
        }
        fs::remove_file(rom.join("Engine.exe")).unwrap();
        assert!(matches!(
            guard_pristine(&rom, &state),
            Err(RomError::RomChanged { .. })
        ));
        let _ = fs::remove_dir_all(&rom);
        let _ = fs::remove_dir_all(state.parent().unwrap());
    }

    #[test]
    fn guard_refuses_state_dir_inside_rom_and_corrupt_baseline() {
        let rom = tmp("guard_in");
        write_sentinels(&rom, "h");
        let inside = rom.join("state");
        assert!(matches!(
            guard_pristine(&rom, &inside),
            Err(RomError::BadStateDir(_))
        ));
        assert!(!inside.exists());
        let state = tmp("guard_corrupt");
        fs::write(state.join(BASELINE_NAME), "garbage-without-separator\n").unwrap();
        assert!(matches!(
            guard_pristine(&rom, &state),
            Err(RomError::BadStateDir(_))
        ));
        let _ = fs::remove_dir_all(&rom);
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn test_rom_and_resolver_agree() {
        assert_eq!(test_rom().is_some(), resolve_rom().is_ok());
    }
}
