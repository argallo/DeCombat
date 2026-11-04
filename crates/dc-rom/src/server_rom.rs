//! Server ROM (`docs/SERVER_ROM.md`): the part of the user's client folder that the lobby and the game server read,
//! extracted by `dc-server-rom` into a plain folder for a host machine that should not carry the whole client.
//!
//! Layout of a server ROM folder:
//! - `server-rom.json`: the manifest ([`Manifest`]): source build identity (the five sentinel hashes of the client
//!   folder it was made from), the archive key material, every archive's full entry list (name, size, and for the
//!   entries whose bytes are present their SHA-256) and the SHA-256 of every other file;
//! - `Game/<archive>.rez/<entry path>`: the bytes of a present entry as the readers see them (text tables already
//!   decrypted), one plain file per entry, `\` of the entry name turned into `/`;
//! - `Game/CShell.strings`: the strings image of the shell library (see [`shell_strings_image`]); no executable.
//!
//! The readers detect a server ROM by its manifest ([`is_server_rom`]); `dc-rez` then serves the archive folders
//! in place of the `.rez` files, [`crate::crypt_key`] takes the key from the manifest, [`crate::identify_rom`]
//! checks the manifest's source identity and every file hash, and [`shell_image`] returns the strings image.
//! The client refuses a server ROM ([`refuse_for_client`]).
// Provenance: ORIGINAL (our own tooling and file layout; no 2009 behaviour)

use crate::error::RomError;
use crate::rom::{RomId, KNOWN_BUILDS, SENTINEL_FILES};
use crate::sha256::{sha256_hex, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The manifest file at the root of a server ROM.
pub const MANIFEST: &str = "server-rom.json";
/// Format tag of the manifest.
pub const FORMAT: &str = "dc-server-rom/1";
/// The shell library of a full client folder, relative to the ROM root.
pub const SHELL_LIBRARY: &str = "Game/CShell.dll";
/// The strings image of the shell library in a server ROM, relative to its root.
pub const SHELL_STRINGS: &str = "Game/CShell.strings";

/// One archive entry of the manifest: full entry name as the archive lists it, size, SHA-256 when present, and
/// the stored length when only the head of the entry is present (a wave file's chunk headers, `docs/SERVER_ROM.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestEntry {
    pub name: String,
    pub size: u32,
    pub sha256: Option<String>,
    pub head: Option<u32>,
}

impl ManifestEntry {
    /// Length of the file the server ROM holds for this entry.
    pub fn stored_len(&self) -> u32 {
        self.head.unwrap_or(self.size)
    }
}

/// The parsed `server-rom.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The client build the server ROM was made from.
    pub build: RomId,
    /// SHA-256 of each sentinel file of the source folder (same order as [`SENTINEL_FILES`]).
    pub sentinels: Vec<String>,
    /// The archive key (verified against the pinned digest when read through [`crate::crypt_key`]).
    pub key: Vec<u8>,
    /// SHA-256 of the source shell library.
    pub shell_source_sha256: String,
    /// Loose files other than the archive entries: (relative path, size, SHA-256).
    pub files: Vec<(String, u64, String)>,
    /// Archive file name (as in the client's `Game` folder) -> entries in archive order.
    pub archives: BTreeMap<String, Vec<ManifestEntry>>,
}

fn bad(m: impl Into<String>) -> RomError {
    RomError::BadStateDir(format!("server ROM manifest: {}", m.into()))
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn is_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())
}

fn build_name(id: RomId) -> &'static str {
    match id {
        RomId::Eu20091221 => "Eu20091221",
    }
}

fn build_of_name(s: &str) -> Option<RomId> {
    KNOWN_BUILDS
        .iter()
        .map(|b| b.id)
        .find(|id| build_name(*id) == s)
}

/// Rejects a relative path that could leave the server ROM folder.
fn safe_rel(rel: &str) -> bool {
    let n = rel.replace('\\', "/");
    !n.is_empty()
        && !n.starts_with('/')
        && !n.contains(':')
        && !n.contains('\0')
        && n.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

impl Manifest {
    /// Parses manifest text.
    pub fn parse(text: &str) -> Result<Manifest, RomError> {
        use serde_json::Value;
        let v: Value = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
        if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return Err(bad(format!("not {FORMAT}")));
        }
        let src = v.get("source").ok_or_else(|| bad("no source"))?;
        let build = src
            .get("build")
            .and_then(Value::as_str)
            .and_then(build_of_name)
            .ok_or_else(|| bad("unknown source build"))?;
        let sent = src
            .get("sentinels")
            .and_then(Value::as_object)
            .ok_or_else(|| bad("no sentinels"))?;
        let sentinels = SENTINEL_FILES
            .iter()
            .map(|rel| {
                sent.get(*rel)
                    .and_then(Value::as_str)
                    .filter(|s| is_sha(s))
                    .map(str::to_ascii_lowercase)
                    .ok_or_else(|| bad(format!("sentinel {rel}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let key = v
            .get("archive_key_hex")
            .and_then(Value::as_str)
            .and_then(hex_decode)
            .ok_or_else(|| bad("archive_key_hex"))?;
        let shell_source_sha256 = v
            .get("shell")
            .and_then(|s| s.get("source_sha256"))
            .and_then(Value::as_str)
            .filter(|s| is_sha(s))
            .ok_or_else(|| bad("shell.source_sha256"))?
            .to_ascii_lowercase();
        let mut files = Vec::new();
        for f in v
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("no files"))?
        {
            let a = f.as_array().filter(|a| a.len() == 3).ok_or_else(|| bad("file row"))?;
            let path = a[0].as_str().filter(|p| safe_rel(p)).ok_or_else(|| bad("file path"))?;
            let size = a[1].as_u64().ok_or_else(|| bad("file size"))?;
            let sha = a[2].as_str().filter(|s| is_sha(s)).ok_or_else(|| bad("file sha256"))?;
            files.push((path.to_string(), size, sha.to_ascii_lowercase()));
        }
        let mut archives = BTreeMap::new();
        for (name, rows) in v
            .get("archives")
            .and_then(Value::as_object)
            .ok_or_else(|| bad("no archives"))?
        {
            if !safe_rel(name) || name.contains(['/', '\\']) {
                return Err(bad(format!("archive name {name:?}")));
            }
            let rows = rows.as_array().ok_or_else(|| bad("archive rows"))?;
            let mut list = Vec::with_capacity(rows.len());
            for r in rows {
                let a = r
                    .as_array()
                    .filter(|a| a.len() == 3 || a.len() == 4)
                    .ok_or_else(|| bad("entry row"))?;
                let n = a[0].as_str().ok_or_else(|| bad("entry name"))?;
                let size = a[1]
                    .as_u64()
                    .and_then(|s| u32::try_from(s).ok())
                    .ok_or_else(|| bad("entry size"))?;
                let sha = match &a[2] {
                    Value::Null => None,
                    Value::String(s) if is_sha(s) => {
                        if !safe_rel(n) {
                            return Err(bad(format!("entry name {n:?}")));
                        }
                        Some(s.to_ascii_lowercase())
                    }
                    _ => return Err(bad("entry sha256")),
                };
                let head = match a.get(3) {
                    None => None,
                    Some(h) => Some(
                        h.as_u64()
                            .and_then(|h| u32::try_from(h).ok())
                            .filter(|h| *h <= size && sha.is_some())
                            .ok_or_else(|| bad("entry head"))?,
                    ),
                };
                list.push(ManifestEntry {
                    name: n.to_string(),
                    size,
                    sha256: sha,
                    head,
                });
            }
            archives.insert(name.clone(), list);
        }
        Ok(Manifest {
            build,
            sentinels,
            key,
            shell_source_sha256,
            files,
            archives,
        })
    }

    /// The manifest text: deterministic (fixed key order, one entry per line, `\n` line ends).
    pub fn to_json(&self) -> String {
        let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
        let mut o = String::new();
        o.push_str("{\n");
        o.push_str(&format!("  \"format\": {},\n", q(FORMAT)));
        o.push_str(
            "  \"note\": \"DeCombat server ROM: the parts of the user's own 2009 client folder that the lobby and the \
             game server read (docs/SERVER_ROM.md). Not for redistribution.\",\n",
        );
        o.push_str("  \"source\": {\n");
        o.push_str(&format!("    \"build\": {},\n", q(build_name(self.build))));
        o.push_str("    \"sentinels\": {\n");
        for (i, (rel, h)) in SENTINEL_FILES.iter().zip(&self.sentinels).enumerate() {
            let comma = if i + 1 < SENTINEL_FILES.len() { "," } else { "" };
            o.push_str(&format!("      {}: {}{comma}\n", q(rel), q(h)));
        }
        o.push_str("    }\n  },\n");
        o.push_str(&format!(
            "  \"archive_key_hex\": {},\n",
            q(&hex_encode(&self.key))
        ));
        o.push_str(&format!(
            "  \"shell\": {{\"path\": {}, \"source_sha256\": {}}},\n",
            q(SHELL_STRINGS),
            q(&self.shell_source_sha256)
        ));
        o.push_str("  \"files\": [\n");
        for (i, (p, size, h)) in self.files.iter().enumerate() {
            let comma = if i + 1 < self.files.len() { "," } else { "" };
            o.push_str(&format!("    [{}, {size}, {}]{comma}\n", q(p), q(h)));
        }
        o.push_str("  ],\n");
        o.push_str("  \"archives\": {\n");
        let n = self.archives.len();
        for (i, (name, rows)) in self.archives.iter().enumerate() {
            o.push_str(&format!("    {}: [\n", q(name)));
            for (j, e) in rows.iter().enumerate() {
                let comma = if j + 1 < rows.len() { "," } else { "" };
                let sha = e.sha256.as_deref().map(q).unwrap_or_else(|| "null".into());
                let head = e.head.map(|h| format!(", {h}")).unwrap_or_default();
                o.push_str(&format!(
                    "      [{}, {}, {sha}{head}]{comma}\n",
                    q(&e.name),
                    e.size
                ));
            }
            let comma = if i + 1 < n { "," } else { "" };
            o.push_str(&format!("    ]{comma}\n"));
        }
        o.push_str("  }\n}\n");
        o
    }

    /// Entries of `archive` (file name compared ASCII case-insensitively).
    pub fn archive(&self, archive: &str) -> Option<&[ManifestEntry]> {
        self.archives
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(archive))
            .map(|(_, v)| v.as_slice())
    }
}

/// Where a present entry of `archive` lives under a server ROM root.
pub fn entry_path(root: &Path, archive: &str, entry: &str) -> PathBuf {
    let mut p = root.join("Game").join(archive);
    for c in entry.split(['\\', '/']) {
        p.push(c);
    }
    p
}

/// `dir` is a server ROM (holds a manifest).
pub fn is_server_rom(dir: &Path) -> bool {
    dir.join(MANIFEST).is_file()
}

type Cache = Mutex<HashMap<PathBuf, (u64, Option<std::time::SystemTime>, Arc<Manifest>)>>;
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// The manifest of the server ROM at `dir` (`Ok(None)` for a full client folder). Parsed once per process and
/// manifest file version.
// DC-DEFAULT: DSV-371 (proposed)
pub fn manifest(dir: &Path) -> Result<Option<Arc<Manifest>>, RomError> {
    let p = dir.join(MANIFEST);
    let Ok(meta) = fs::metadata(&p) else {
        return Ok(None);
    };
    if !meta.is_file() {
        return Ok(None);
    }
    let stamp = (meta.len(), meta.modified().ok());
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = g.get_or_insert_with(|| Mutex::new(HashMap::new()));
    let c = cache.get_mut().unwrap_or_else(|e| e.into_inner());
    if let Some((len, t, m)) = c.get(dir) {
        if (*len, *t) == stamp {
            return Ok(Some(Arc::clone(m)));
        }
    }
    let text = fs::read_to_string(&p).map_err(|e| RomError::io(&p, &e))?;
    crate::trace::note("file", &p, "", text.len() as u64);
    let m = Arc::new(Manifest::parse(&text)?);
    c.insert(dir.to_path_buf(), (stamp.0, stamp.1, Arc::clone(&m)));
    Ok(Some(m))
}

/// The client cannot run on a server ROM: a clear message when `dir` is one.
// DC-DEFAULT: DSV-372 (proposed)
pub fn refuse_for_client(dir: &Path) -> Result<(), String> {
    if is_server_rom(dir) {
        Err(format!(
            "{} is a server ROM (made by dc-server-rom for the lobby and the game server); the client needs the \
             full 2009 client folder",
            dir.display()
        ))
    } else {
        Ok(())
    }
}

fn file_sha(p: &Path) -> Result<(u64, String), RomError> {
    let bytes = fs::read(p).map_err(|e| RomError::io(p, &e))?;
    Ok((bytes.len() as u64, sha256_hex(&bytes)))
}

/// Hashes every file the manifest lists (loose files and present entries) and compares size and SHA-256. Returns
/// the files checked; a missing or changed file is [`RomError::RomChanged`].
pub fn verify(dir: &Path) -> Result<usize, RomError> {
    let m = manifest(dir)?.ok_or_else(|| RomError::NotADirectory(dir.join(MANIFEST)))?;
    let mut changed = Vec::new();
    let mut n = 0;
    for (rel, size, sha) in &m.files {
        n += 1;
        match file_sha(&dir.join(rel)) {
            Ok((s, h)) if s == *size && h == *sha => {}
            _ => changed.push(rel.clone()),
        }
    }
    for (arch, rows) in &m.archives {
        for e in rows {
            let Some(sha) = &e.sha256 else { continue };
            n += 1;
            match file_sha(&entry_path(dir, arch, &e.name)) {
                Ok((s, h)) if s == e.stored_len() as u64 && h == *sha => {}
                _ => changed.push(format!("Game/{arch}/{}", e.name.replace('\\', "/"))),
            }
        }
    }
    if changed.is_empty() {
        Ok(n)
    } else {
        Err(RomError::RomChanged { changed })
    }
}

/// The build a server ROM was made from: its manifest's sentinel hashes must be a known build, and every file must
/// match the manifest.
pub(crate) fn identify(dir: &Path, m: &Manifest) -> Result<RomId, RomError> {
    let known = KNOWN_BUILDS.iter().find(|b| {
        b.id == m.build
            && b.sha256
                .iter()
                .zip(&m.sentinels)
                .all(|(want, got)| want.eq_ignore_ascii_case(got))
    });
    let Some(b) = known else {
        return Err(RomError::UnknownBuild {
            mismatched: vec![MANIFEST.to_string()],
        });
    };
    verify(dir)?;
    Ok(b.id)
}

/// The bytes the servers scan for client variable names and read the build time stamp from: the shell library of
/// a client folder, or the strings image of a server ROM (checked against the manifest). `None` when missing.
pub fn shell_image(rom: &Path) -> Option<Vec<u8>> {
    if let Ok(Some(m)) = manifest(rom) {
        let p = rom.join(SHELL_STRINGS);
        let bytes = fs::read(&p).ok()?;
        crate::trace::note("file", &p, "", bytes.len() as u64);
        let want = m.files.iter().find(|f| f.0 == SHELL_STRINGS)?;
        if bytes.len() as u64 != want.1 || sha256_hex(&bytes) != want.2 {
            return None;
        }
        return Some(bytes);
    }
    let p = rom.join(SHELL_LIBRARY);
    let bytes = fs::read(&p).ok()?;
    crate::trace::note("file", &p, "", bytes.len() as u64);
    Some(bytes)
}

fn graphic(b: u8) -> bool {
    b.is_ascii_graphic()
}

/// The strings image of a shell library: what the servers read from it, without its code or data.
///
/// The servers read (a) the image header's time stamp (`e_lfanew` at 0x3c, then `PE\0\0` and the file header) and
/// (b) printable strings that end in a NUL, matched by hash: either a whole NUL-delimited run of printable bytes, or
/// the last `n` printable bytes before a NUL. The image keeps the DOS header and stub, the signature and the file
/// header (no section table, no section bytes), then a `0x01 0x00` separator, then for every NUL of the library
/// the run of printable bytes that ends at it: the run alone followed by NUL when a NUL (or the start of the file)
/// came right before it, else `0x01`, the run and NUL. Both scans give the same answers on the image as on the
/// library, because a run the library broke with a non-printable byte stays broken by the `0x01`.
pub fn shell_strings_image(lib: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let rd = |o: usize| {
        lib.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let head_end = rd(0x3c)
        .filter(|pe| lib.get(*pe..*pe + 4) == Some(b"PE\0\0"))
        .map(|pe| pe + 24)
        .filter(|e| *e <= lib.len())
        .unwrap_or(0);
    out.extend_from_slice(&lib[..head_end]);
    out.extend_from_slice(&[0x01, 0x00]);
    let mut i = 0usize;
    while i < lib.len() {
        if lib[i] != 0 {
            i += 1;
            continue;
        }
        // the printable run that ends at this NUL
        let mut s = i;
        while s > 0 && graphic(lib[s - 1]) {
            s -= 1;
        }
        if s < i {
            let whole = s == 0 || lib[s - 1] == 0;
            if !whole {
                out.push(0x01);
            }
            out.extend_from_slice(&lib[s..i]);
            out.push(0);
        }
        i += 1;
    }
    out
}

/// SHA-256 of `bytes` in streaming form (for large entries).
pub fn sha256_of(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_encode(&h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_strings(bytes: &[u8], want: &[&[u8]]) -> Vec<Vec<u8>> {
        bytes
            .split(|&b| b == 0)
            .filter(|r| !r.is_empty() && r.iter().all(|b| b.is_ascii_graphic()))
            .filter(|r| want.contains(r))
            .map(<[u8]>::to_vec)
            .collect()
    }

    fn suffix_hits(bytes: &[u8], len: usize) -> Vec<Vec<u8>> {
        bytes
            .iter()
            .enumerate()
            .filter(|&(i, &b)| b == 0 && i >= len)
            .map(|(i, _)| bytes[i - len..i].to_vec())
            .filter(|w| w.iter().all(u8::is_ascii_graphic))
            .collect()
    }

    #[test]
    fn strings_image_keeps_both_scans_and_the_header_stamp() {
        let mut lib = vec![0u8; 0x80];
        lib[0] = b'M';
        lib[1] = b'Z';
        lib[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        lib[0x40..0x44].copy_from_slice(b"PE\0\0");
        lib[0x48..0x4c].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        lib.extend_from_slice(b"\0alpha\0\x90\x91tailname\0beta\x05gamma\0\0\x01\x02");
        let img = shell_strings_image(&lib);
        // the header stamp reads the same
        assert_eq!(&img[0x48..0x4c], &0x1234_5678u32.to_le_bytes());
        assert!(!img.windows(2).any(|w| w == [0x90, 0x91]), "no non-printable body bytes");
        let want: [&[u8]; 4] = [b"alpha", b"tailname", b"gamma", b"beta"];
        assert_eq!(find_strings(&lib, &want), find_strings(&img, &want));
        for len in 1..10 {
            let mut a = suffix_hits(&lib, len);
            let mut b = suffix_hits(&img, len);
            a.sort();
            b.sort();
            a.dedup();
            b.dedup();
            assert_eq!(a, b, "suffix windows of length {len}");
        }
    }

    #[test]
    fn manifest_round_trips_and_rejects_escapes() {
        let mut archives = BTreeMap::new();
        archives.insert(
            "A.rez".to_string(),
            vec![
                ManifestEntry {
                    name: "DIR\\X.TXT".into(),
                    size: 3,
                    sha256: Some(sha256_hex(b"abc")),
                    head: None,
                },
                ManifestEntry {
                    name: "DIR\\Z.WAV".into(),
                    size: 900,
                    sha256: Some(sha256_hex(b"RIFF")),
                    head: Some(4),
                },
                ManifestEntry {
                    name: "DIR\\Y.DAT".into(),
                    size: 9,
                    sha256: None,
                    head: None,
                },
            ],
        );
        let m = Manifest {
            build: RomId::Eu20091221,
            sentinels: KNOWN_BUILDS[0].sha256.iter().map(|s| s.to_string()).collect(),
            key: b"k3y-bytes".to_vec(),
            shell_source_sha256: sha256_hex(b"lib"),
            files: vec![(SHELL_STRINGS.to_string(), 4, sha256_hex(b"strs"))],
            archives,
        };
        let text = m.to_json();
        assert_eq!(Manifest::parse(&text).unwrap(), m);
        assert_eq!(m.to_json(), text, "deterministic");
        let evil = text.replace("DIR\\\\X.TXT", "..\\\\X.TXT");
        assert!(Manifest::parse(&evil).is_err());
        assert!(Manifest::parse("{}").is_err());
    }

    #[test]
    fn a_server_rom_folder_is_refused_for_the_client() {
        let d = std::env::temp_dir().join(format!("dc_srom_refuse_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        assert!(refuse_for_client(&d).is_ok());
        fs::write(d.join(MANIFEST), "{}").unwrap();
        let e = refuse_for_client(&d).unwrap_err();
        assert!(e.contains("server ROM") && e.contains("full 2009 client"), "{e}");
        let _ = fs::remove_dir_all(&d);
    }
}
