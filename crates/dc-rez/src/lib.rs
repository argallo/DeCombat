//! LithTech-style `.rez` v1 archive reader, Blowfish ECB attribute decryptor/encryptor, and layered mod VFS.
//!
//! The archive key is never compiled in: [`RezVfs`] obtains it lazily from the user's own `Engine.exe` through
//! `dc_rom::crypt_key` (verified against a pinned SHA-256), and the free functions take an explicit key.
//! Archive parsing treats the file as hostile: all offsets are overflow-checked against the file length,
//! recursion depth is capped, and revisited ranges are rejected.

pub mod blowfish_tables;
pub mod server_rom;
pub use dc_rom;

use blowfish_tables::{BLOWFISH_P_INIT, BLOWFISH_S_INIT};
use dc_rom::RomError;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Offset to `FileFormatVersion` in a `.rez` v1 header:
/// `2 + 60 + 2 + 60 + 3 + 1 + 32 + 1 + 1 + 32 + 1 == 193`.
pub const REZ_HDR_OFFSET: u64 = 2 + 60 + 2 + 60 + 3 + 1 + 32 + 1 + 1 + 32 + 1;

/// Maximum directory nesting accepted while walking an archive.
pub const MAX_REZ_DEPTH: usize = 16;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything that can go wrong reading archives or (de)crypting entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RezError {
    /// Error from the ROM layer (key extraction, ROM resolution).
    Rom(RomError),
    /// An I/O operation failed.
    Io { path: PathBuf, message: String },
    /// Blowfish key length outside `4..=56` bytes.
    BadKeyLength(usize),
    /// A range lies (partly) beyond the end of the file.
    Truncated {
        what: &'static str,
        offset: u64,
        size: u64,
        file_len: u64,
    },
    /// Structurally invalid archive.
    Malformed(String),
    /// Directory nesting deeper than [`MAX_REZ_DEPTH`].
    TooDeep,
    /// A directory range was referenced twice (cycle or overlap).
    Revisited { offset: u64, size: u32 },
    /// `<rom>/Game/<name>` does not exist.
    ArchiveNotFound(String),
    /// Entry name is empty, absolute or contains `..`.
    BadName(String),
}

impl fmt::Display for RezError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RezError::Rom(e) => write!(f, "{e}"),
            RezError::Io { path, message } => {
                write!(f, "I/O error on {}: {}", path.display(), message)
            }
            RezError::BadKeyLength(n) => write!(f, "Blowfish key length {n} outside 4..=56"),
            RezError::Truncated {
                what,
                offset,
                size,
                file_len,
            } => write!(
                f,
                "{what}: range {offset:#x}+{size:#x} exceeds file length {file_len:#x}"
            ),
            RezError::Malformed(m) => write!(f, "malformed rez: {m}"),
            RezError::TooDeep => write!(f, "rez directory nesting deeper than {MAX_REZ_DEPTH}"),
            RezError::Revisited { offset, size } => {
                write!(f, "rez directory range {offset:#x}+{size:#x} visited twice")
            }
            RezError::ArchiveNotFound(n) => write!(f, "archive {n} not found"),
            RezError::BadName(n) => write!(f, "invalid entry name {n:?}"),
        }
    }
}

impl std::error::Error for RezError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RezError::Rom(e) => Some(e),
            _ => None,
        }
    }
}

impl From<RomError> for RezError {
    fn from(e: RomError) -> Self {
        RezError::Rom(e)
    }
}

fn io_err(path: &Path, e: &std::io::Error) -> RezError {
    RezError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Blowfish ECB
// ---------------------------------------------------------------------------

/// Standard Blowfish block cipher state (18 P-array `u32`s + 4 x 256 S-box `u32`s).
#[derive(Clone)]
pub struct Blowfish {
    p: [u32; 18],
    s: [[u32; 256]; 4],
}

impl fmt::Debug for Blowfish {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Blowfish(<redacted>)")
    }
}

impl Blowfish {
    /// Initializes the P-array and S-boxes with `key` (`4..=56` bytes).
    pub fn new(key: &[u8]) -> Result<Self, RezError> {
        if !(4..=56).contains(&key.len()) {
            return Err(RezError::BadKeyLength(key.len()));
        }
        let mut bf = Self {
            p: BLOWFISH_P_INIT,
            s: BLOWFISH_S_INIT,
        };

        let mut ki = 0usize;
        for p_word in &mut bf.p {
            let mut data = 0u32;
            for _ in 0..4 {
                data = (data << 8) | (key[ki] as u32);
                ki = (ki + 1) % key.len();
            }
            *p_word ^= data;
        }

        let mut l = 0u32;
        let mut r = 0u32;
        for i in (0..18).step_by(2) {
            let (nl, nr) = bf.encrypt_block(l, r);
            l = nl;
            r = nr;
            bf.p[i] = l;
            bf.p[i + 1] = r;
        }

        for box_idx in 0..4 {
            for i in (0..256).step_by(2) {
                let (nl, nr) = bf.encrypt_block(l, r);
                l = nl;
                r = nr;
                bf.s[box_idx][i] = l;
                bf.s[box_idx][i + 1] = r;
            }
        }

        Ok(bf)
    }

    #[inline]
    fn f(&self, x: u32) -> u32 {
        let a = ((x >> 24) & 0xff) as usize;
        let b = ((x >> 16) & 0xff) as usize;
        let c = ((x >> 8) & 0xff) as usize;
        let d = (x & 0xff) as usize;
        (self.s[0][a].wrapping_add(self.s[1][b]) ^ self.s[2][c]).wrapping_add(self.s[3][d])
    }

    /// Encrypts one 64-bit block `(xl, xr)` in big-endian word order.
    #[inline]
    pub fn encrypt_block(&self, mut xl: u32, mut xr: u32) -> (u32, u32) {
        for i in 0..16 {
            xl ^= self.p[i];
            xr ^= self.f(xl);
            std::mem::swap(&mut xl, &mut xr);
        }
        std::mem::swap(&mut xl, &mut xr);
        xr ^= self.p[16];
        xl ^= self.p[17];
        (xl, xr)
    }

    /// Decrypts one 64-bit block `(xl, xr)` in big-endian word order.
    #[inline]
    pub fn decrypt_block(&self, mut xl: u32, mut xr: u32) -> (u32, u32) {
        for i in (2..18).rev() {
            xl ^= self.p[i];
            xr ^= self.f(xl);
            std::mem::swap(&mut xl, &mut xr);
        }
        std::mem::swap(&mut xl, &mut xr);
        xr ^= self.p[1];
        xl ^= self.p[0];
        (xl, xr)
    }

    /// Applies the archive framing (byte-swapped 32-bit words, ECB over `len / 8 * 8` bytes, plain tail).
    fn apply(&self, data: &[u8], block: impl Fn(&Self, u32, u32) -> (u32, u32)) -> Vec<u8> {
        let n = (data.len() / 8) * 8;
        let swapped = swap_words32(&data[..n]);
        let mut done = Vec::with_capacity(data.len());
        for chunk in swapped.as_chunks::<8>().0 {
            let xl = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let xr = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            let (a, b) = block(self, xl, xr);
            done.extend_from_slice(&a.to_be_bytes());
            done.extend_from_slice(&b.to_be_bytes());
        }
        let mut out = swap_words32(&done);
        out.extend_from_slice(&data[n..]);
        out
    }

    /// Decrypts archive attribute/GMS bytes.
    pub fn decrypt_bytes(&self, data: &[u8]) -> Vec<u8> {
        self.apply(data, Self::decrypt_block)
    }

    /// Encrypts archive attribute/GMS bytes (exact inverse of [`Blowfish::decrypt_bytes`]).
    pub fn encrypt_bytes(&self, data: &[u8]) -> Vec<u8> {
        self.apply(data, Self::encrypt_block)
    }
}

/// Byte-swaps every 4-byte word in `data`; a trailing partial word is copied unchanged.
pub fn swap_words32(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let (words, rest) = data.as_chunks::<4>();
    for c in words {
        out.extend_from_slice(&[c[3], c[2], c[1], c[0]]);
    }
    out.extend_from_slice(rest);
    out
}

/// Returns `true` if `data` looks like unencrypted plain text (first 64 bytes only `\r`, `\n`, `\t` or
/// printable ASCII).
pub fn is_plain(data: &[u8]) -> bool {
    let head = &data[..data.len().min(64)];
    !head.is_empty()
        && head
            .iter()
            .all(|&c| matches!(c, b'\r' | b'\n' | b'\t' | 32..=126))
}

/// Decrypts archive bytes with an explicit `key` (`4..=56` bytes).
pub fn decrypt_with_key(data: &[u8], key: &[u8]) -> Result<Vec<u8>, RezError> {
    Ok(Blowfish::new(key)?.decrypt_bytes(data))
}

/// Encrypts archive bytes with an explicit `key` (`4..=56` bytes).
pub fn encrypt_with_key(data: &[u8], key: &[u8]) -> Result<Vec<u8>, RezError> {
    Ok(Blowfish::new(key)?.encrypt_bytes(data))
}

// ---------------------------------------------------------------------------
// Archive walking (hostile-input safe)
// ---------------------------------------------------------------------------

#[inline]
fn le32(d: &[u8], off: usize) -> Result<u32, RezError> {
    let end = off
        .checked_add(4)
        .ok_or_else(|| RezError::Malformed("offset overflow".into()))?;
    let s = d
        .get(off..end)
        .ok_or_else(|| RezError::Malformed(format!("truncated record at {off:#x}")))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn latin1_decode(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Index of the NUL terminating the string that starts at `start`.
fn nul_from(data: &[u8], start: usize, what: &str) -> Result<usize, RezError> {
    let tail = data
        .get(start..)
        .ok_or_else(|| RezError::Malformed(format!("{what} starts past end of directory")))?;
    tail.iter()
        .position(|&b| b == 0)
        .map(|rel| start + rel)
        .ok_or_else(|| RezError::Malformed(format!("unterminated {what}")))
}

struct WalkState {
    file_len: u64,
    /// Bytes of directory stream we may still read: legitimate directory streams are disjoint, so their
    /// total can never exceed the file length.
    budget: u64,
    visited: HashSet<(u64, u32)>,
}

/// Ranges are `(full_rel_path, offset, size)`.
type RawEntry = (String, u64, u32);

fn walk_stream<R: Read + Seek>(
    f: &mut R,
    st: &mut WalkState,
    pos: u64,
    size: u32,
    depth: usize,
    path_prefix: &str,
    out: &mut Vec<RawEntry>,
) -> Result<(), RezError> {
    if depth > MAX_REZ_DEPTH {
        return Err(RezError::TooDeep);
    }
    let end = pos.checked_add(size as u64).ok_or(RezError::Truncated {
        what: "directory",
        offset: pos,
        size: size as u64,
        file_len: st.file_len,
    })?;
    if end > st.file_len {
        return Err(RezError::Truncated {
            what: "directory",
            offset: pos,
            size: size as u64,
            file_len: st.file_len,
        });
    }
    if !st.visited.insert((pos, size)) {
        return Err(RezError::Revisited { offset: pos, size });
    }
    st.budget = st
        .budget
        .checked_sub(size as u64)
        .ok_or_else(|| RezError::Malformed("directory ranges overlap".into()))?;

    f.seek(SeekFrom::Start(pos))
        .map_err(|e| RezError::Malformed(format!("seek to {pos:#x} failed: {e}")))?;
    let mut data = vec![0u8; size as usize];
    f.read_exact(&mut data).map_err(|e| {
        RezError::Malformed(format!("read of {size} bytes at {pos:#x} failed: {e}"))
    })?;

    let mut i = 0usize;
    while i < data.len() {
        let typ = le32(&data, i)?;
        let field = |n: usize| -> Result<u32, RezError> {
            le32(
                &data,
                i.checked_add(n)
                    .ok_or_else(|| RezError::Malformed("offset overflow".into()))?,
            )
        };
        if typ == 1 {
            let p = field(4)? as u64;
            let s = field(8)?;
            let _t = field(12)?;
            let name_at = i
                .checked_add(16)
                .ok_or_else(|| RezError::Malformed("offset overflow".into()))?;
            let j = nul_from(&data, name_at, "directory name")?;
            let name = latin1_decode(&data[name_at..j]);
            i = j + 1;
            let child_prefix = format!("{}{}\\", path_prefix, name);
            walk_stream(f, st, p, s, depth + 1, &child_prefix, out)?;
        } else {
            let p = field(4)? as u64;
            let s = field(8)?;
            let _t = field(12)?;
            let _rid = field(16)?;
            let rtype = field(20)?;
            let nkeys = field(24)? as u64;
            let name_at = i
                .checked_add(28)
                .ok_or_else(|| RezError::Malformed("offset overflow".into()))?;
            let j = nul_from(&data, name_at, "entry name")?;
            let name = latin1_decode(&data[name_at..j]);
            let k = nul_from(&data, j + 1, "entry comment")?;
            let next = (k as u64 + 1)
                .checked_add(
                    nkeys
                        .checked_mul(4)
                        .ok_or_else(|| RezError::Malformed("key count overflow".into()))?,
                )
                .ok_or_else(|| RezError::Malformed("key count overflow".into()))?;
            if next > data.len() as u64 {
                return Err(RezError::Malformed(
                    "entry keys run past end of directory".into(),
                ));
            }
            i = next as usize;

            let rtype_bytes = rtype.to_le_bytes();
            let mut trimmed_end = rtype_bytes.len();
            while trimmed_end > 0 && rtype_bytes[trimmed_end - 1] == 0 {
                trimmed_end -= 1;
            }
            let ext: String = rtype_bytes[..trimmed_end]
                .iter()
                .rev()
                .map(|&b| b as char)
                .collect();
            out.push((format!("{}{}.{}", path_prefix, name, ext), p, s));
        }
    }
    Ok(())
}

/// Walks an archive held in any seekable reader of length `file_len`.
pub fn walk_reader<R: Read + Seek>(
    r: &mut R,
    file_len: u64,
) -> Result<Vec<(String, u64, u32)>, RezError> {
    let hdr_end = REZ_HDR_OFFSET + 12;
    if file_len < hdr_end {
        return Err(RezError::Truncated {
            what: "header",
            offset: REZ_HDR_OFFSET,
            size: 12,
            file_len,
        });
    }
    r.seek(SeekFrom::Start(REZ_HDR_OFFSET))
        .map_err(|e| RezError::Malformed(format!("seek to header failed: {e}")))?;
    let mut hdr = [0u8; 12];
    r.read_exact(&mut hdr)
        .map_err(|e| RezError::Malformed(format!("header read failed: {e}")))?;
    let _ver = le32(&hdr, 0)?;
    let rpos = le32(&hdr, 4)? as u64;
    let rsize = le32(&hdr, 8)?;
    let mut st = WalkState {
        file_len,
        budget: file_len,
        visited: HashSet::new(),
    };
    let mut out = Vec::new();
    walk_stream(r, &mut st, rpos, rsize, 0, "", &mut out)?;
    Ok(out)
}

/// Walks a `.rez` v1 archive and returns `(full_rel_path, offset, size)` for every entry.
///
/// In a server ROM (`dc_rom::server_rom`) the archive is a folder and the entries come from the manifest (the
/// offset is then the entry's index).
pub fn walk_rez(rez_path: &Path) -> Result<Vec<(String, u64, u32)>, RezError> {
    if rez_path.is_dir() || !rez_path.exists() {
        return Ok(RezArchive::open(rez_path)?.entries.clone());
    }
    let mut f = File::open(rez_path).map_err(|e| io_err(rez_path, &e))?;
    let len = f.metadata().map_err(|e| io_err(rez_path, &e))?.len();
    dc_rom::trace::note("dir", rez_path, "", len);
    walk_reader(&mut f, len)
}

/// Canonical full-path key: separators folded to `/`, leading separators dropped, ASCII upper case.
fn relpath_key(name: &str) -> String {
    name.replace('\\', "/")
        .trim_start_matches('/')
        .to_ascii_uppercase()
}

/// `true` when `name` has one of the text-table extensions (`.txt`, `.gms`, `.bms`, case-insensitive).
///
/// OBSERVED: only the text tables of the ROM archives are Blowfish-encrypted; world, model and texture
/// entries are stored raw. See the FMT note staged with this change.
pub fn is_text_name(name: &str) -> bool {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rsplit_once('.') {
        Some((_, ext)) => ["txt", "gms", "bms"]
            .iter()
            .any(|t| ext.eq_ignore_ascii_case(t)),
        None => false,
    }
}

fn basename_upper(name: &str) -> String {
    name.replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .to_ascii_uppercase()
}

/// Returns the uppercase basename directory `{UPPER_BASENAME: (offset, size)}` of a `.rez` archive.
/// Duplicate basenames in different folders keep the last entry.
pub fn rez_directory(rez_path: &Path) -> Result<HashMap<String, (u64, u32)>, RezError> {
    let entries = walk_rez(rez_path)?;
    let mut map = HashMap::with_capacity(entries.len());
    for (full_path, pos, size) in entries {
        map.insert(basename_upper(&full_path), (pos, size));
    }
    Ok(map)
}

/// Indexed `.rez` archive for fast repeated lookups.
#[derive(Clone, Debug)]
pub struct RezArchive {
    path: PathBuf,
    file_len: u64,
    entries: Vec<(String, u64, u32)>,
    by_basename: HashMap<String, (u64, u32)>,
    /// Basename key -> every `(offset, size)` carrying it (more than one means ambiguous).
    basename_all: HashMap<String, Vec<(u64, u32)>>,
    by_relpath: HashMap<String, (u64, u32)>,
    /// Server ROM archive folder (`dc_rom::server_rom`): entries are plain files, the offset is the entry index.
    folder: Option<Arc<Folder>>,
}

/// The backing of an archive folder of a server ROM.
#[derive(Debug)]
struct Folder {
    root: PathBuf,
    archive: String,
    /// Per entry: SHA-256 and stored length of the plain file (`None` = not in the server ROM).
    stored: Vec<Option<(String, u32)>>,
}

impl RezArchive {
    /// Opens a `.rez` file, or the archive folder of the same name in a server ROM.
    pub fn open(path: &Path) -> Result<Self, RezError> {
        let in_server_rom = || {
            !path.exists()
                && path
                    .parent()
                    .and_then(Path::parent)
                    .is_some_and(dc_rom::server_rom::is_server_rom)
        };
        if path.is_dir() || in_server_rom() {
            return Self::open_folder(path);
        }
        let mut f = File::open(path).map_err(|e| io_err(path, &e))?;
        let file_len = f.metadata().map_err(|e| io_err(path, &e))?.len();
        let entries = walk_reader(&mut f, file_len)?;
        dc_rom::trace::note("dir", path, "", file_len);
        Ok(Self::index(path, file_len, entries, None))
    }

    fn index(
        path: &Path,
        file_len: u64,
        entries: Vec<(String, u64, u32)>,
        folder: Option<Arc<Folder>>,
    ) -> Self {
        let mut by_basename = HashMap::with_capacity(entries.len());
        let mut basename_all: HashMap<String, Vec<(u64, u32)>> = HashMap::new();
        let mut by_relpath = HashMap::with_capacity(entries.len());
        for (full_path, pos, size) in &entries {
            by_relpath.insert(relpath_key(full_path), (*pos, *size));
            by_basename.insert(basename_upper(full_path), (*pos, *size));
            basename_all
                .entry(basename_upper(full_path))
                .or_default()
                .push((*pos, *size));
        }
        Self {
            path: path.to_path_buf(),
            file_len,
            entries,
            by_basename,
            basename_all,
            by_relpath,
            folder,
        }
    }

    /// `<server ROM>/Game/<archive>`: the entry list of the manifest, the bytes from the plain files.
    // DC-DEFAULT: DSV-371 (proposed)
    fn open_folder(path: &Path) -> Result<Self, RezError> {
        let not_found = || RezError::ArchiveNotFound(path.display().to_string());
        let archive = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(not_found)?
            .to_string();
        let root = path
            .parent()
            .and_then(Path::parent)
            .ok_or_else(not_found)?
            .to_path_buf();
        let m = dc_rom::server_rom::manifest(&root)?.ok_or_else(not_found)?;
        let rows = m.archive(&archive).ok_or_else(not_found)?;
        let entries = rows
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.clone(), i as u64, e.size))
            .collect();
        let folder = Folder {
            root,
            archive,
            stored: rows
                .iter()
                .map(|e| e.sha256.clone().map(|h| (h, e.stored_len())))
                .collect(),
        };
        dc_rom::trace::note("dir", path, "", rows.len() as u64);
        Ok(Self::index(path, 0, entries, Some(Arc::new(folder))))
    }

    /// A server ROM archive folder: entries are stored as the readers see them (text tables already decrypted),
    /// so [`RezArchive::entry_raw`] returns the decoded bytes and nothing is decrypted again.
    pub fn is_decoded(&self) -> bool {
        self.folder.is_some()
    }

    fn read_folder(&self, folder: &Folder, index: u64, size: u32) -> Result<Vec<u8>, RezError> {
        let failed = |p: &Path, m: &str| RezError::Io {
            path: p.to_path_buf(),
            message: m.to_string(),
        };
        let (name, _, _) = usize::try_from(index)
            .ok()
            .and_then(|i| self.entries.get(i))
            .filter(|e| e.2 == size)
            .ok_or(RezError::Truncated {
                what: "server ROM entry",
                offset: index,
                size: size as u64,
                file_len: self.entries.len() as u64,
            })?;
        let p = dc_rom::server_rom::entry_path(&folder.root, &folder.archive, name);
        let Some((sha, want)) = folder.stored.get(index as usize).and_then(Option::as_ref) else {
            return Err(failed(
                &p,
                "entry not in this server ROM (dc-server-rom left it out; rebuild it with the entry)",
            ));
        };
        let buf = fs::read(&p).map_err(|e| io_err(&p, &e))?;
        if buf.len() as u64 != *want as u64 || dc_rom::sha256_hex(&buf) != *sha {
            return Err(failed(
                &p,
                "server ROM file differs from its manifest (rebuild the server ROM)",
            ));
        }
        dc_rom::journal::note(|| format!("file:{}", p.display()), &buf);
        Ok(buf)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> &[(String, u64, u32)] {
        &self.entries
    }

    pub fn directory(&self) -> &HashMap<String, (u64, u32)> {
        &self.by_basename
    }

    /// Reads `size` bytes at `offset`; the range is checked against the file length first.
    pub fn read_raw(&self, offset: u64, size: u32) -> Result<Vec<u8>, RezError> {
        if let Some(folder) = &self.folder {
            return self.read_folder(folder, offset, size);
        }
        let truncated = RezError::Truncated {
            what: "entry",
            offset,
            size: size as u64,
            file_len: self.file_len,
        };
        let end = offset
            .checked_add(size as u64)
            .ok_or_else(|| truncated.clone())?;
        if end > self.file_len {
            return Err(truncated);
        }
        let mut f = File::open(&self.path).map_err(|e| io_err(&self.path, &e))?;
        f.seek(SeekFrom::Start(offset))
            .map_err(|e| io_err(&self.path, &e))?;
        let mut buf = vec![0u8; size as usize];
        f.read_exact(&mut buf).map_err(|e| io_err(&self.path, &e))?;
        // the determinism manifest's input hashes (docs/RENDER_DETERMINISM_CONTRACT.md); off: one atomic load
        dc_rom::journal::note(
            || format!("rez:{}@{offset}+{size}", self.path.display()),
            &buf,
        );
        Ok(buf)
    }

    /// Looks `name` up by its FULL relative path (case-insensitive, `/` and `\` both accepted) and returns
    /// its `(offset, size)`. The basename is never consulted; see [`RezArchive::locate_basename`].
    pub fn locate(&self, name: &str) -> Option<(u64, u32)> {
        self.by_relpath.get(&relpath_key(name)).copied()
    }

    /// Explicit fallback: looks up by basename only (the part after the last separator), succeeding only
    /// when exactly one entry of the archive carries that basename.
    pub fn locate_basename(&self, name: &str) -> Option<(u64, u32)> {
        match self
            .basename_all
            .get(&basename_upper(name))
            .map(|v| v.as_slice())
        {
            Some([one]) => Some(*one),
            _ => None,
        }
    }

    /// Number of entries sharing the basename of `name` (0 = unknown, more than 1 = ambiguous).
    pub fn basename_count(&self, name: &str) -> usize {
        self.basename_all
            .get(&basename_upper(name))
            .map_or(0, Vec::len)
    }

    /// Raw stored bytes of the entry (full-path lookup), `Ok(None)` when absent.
    pub fn entry_raw(&self, name: &str) -> Result<Option<Vec<u8>>, RezError> {
        match self.locate(name) {
            Some((pos, size)) => {
                dc_rom::trace::note("rez", &self.path, name, size as u64);
                self.read_raw(pos, size).map(Some)
            }
            None => Ok(None),
        }
    }

    /// The first `n` stored bytes of an entry (full-path lookup; a header), `Ok(None)` when absent. A packed archive
    /// reads only those bytes; a server ROM's folder reads the entry and keeps its start.
    pub fn entry_head(&self, name: &str, n: usize) -> Result<Option<Vec<u8>>, RezError> {
        match self.locate(name) {
            Some((pos, size)) if self.folder.is_none() => {
                self.read_raw(pos, size.min(n as u32)).map(Some)
            }
            Some(_) => Ok(self.entry_raw(name)?.map(|mut b| {
                b.truncate(n);
                b
            })),
            None => Ok(None),
        }
    }

    /// Like [`RezArchive::entry_raw`] but through the unique-basename fallback.
    pub fn entry_raw_basename(&self, name: &str) -> Result<Option<Vec<u8>>, RezError> {
        match self.locate_basename(name) {
            Some((pos, size)) => {
                dc_rom::trace::note("rez", &self.path, name, size as u64);
                self.read_raw(pos, size).map(Some)
            }
            None => Ok(None),
        }
    }

    /// Reads an entry: raw for non-text names; for text names decrypts with `cipher` unless the bytes
    /// already look like plain text.
    pub fn entry(&self, name: &str, cipher: &Blowfish) -> Result<Option<Vec<u8>>, RezError> {
        let decoded = self.is_decoded();
        Ok(self.entry_raw(name)?.map(|raw| {
            if decoded {
                raw
            } else {
                decode_by_name(name, raw, cipher)
            }
        }))
    }
}

// ---------------------------------------------------------------------------
// Layered Virtual Filesystem
// ---------------------------------------------------------------------------

/// Raw for non-text names; plain-text test then Blowfish for text names.
fn decode_by_name(name: &str, raw: Vec<u8>, cipher: &Blowfish) -> Vec<u8> {
    if !is_text_name(name) || is_plain(&raw) {
        raw
    } else {
        cipher.decrypt_bytes(&raw)
    }
}

type ArchiveSlot = Result<Option<Arc<RezArchive>>, RezError>;

/// Layered virtual filesystem over `<rom_dir>/Game/<rez_name>` archives with mod override directories
/// (`mods/<mod_id>/overrides/...`) shadowing vanilla entries.
#[derive(Debug)]
pub struct RezVfs {
    rom_dir: PathBuf,
    override_dirs: Vec<PathBuf>,
    /// Archive cache including negative results (missing file / corrupt archive).
    archives: Mutex<HashMap<String, ArchiveSlot>>,
    /// Negative entry cache: `(UPPER_REZ, UPPER_NAME)` pairs known to be absent.
    missing: Mutex<HashSet<(String, String)>>,
    /// Cipher, built lazily from the ROM key (or supplied up front by [`RezVfs::with_key`]).
    cipher: Mutex<Option<Arc<Blowfish>>>,
}

/// The file in `dir` whose name equals `name` ignoring ASCII case (the exact name first), for ROM lookups on a
/// case-sensitive file system; `None` when there is no such file.
pub fn case_fold_lookup(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.is_file() {
        return Some(exact);
    }
    let want = name.to_ascii_lowercase();
    fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        (p.is_file() && e.file_name().to_string_lossy().to_ascii_lowercase() == want).then_some(p)
    })
}

impl RezVfs {
    /// Creates a VFS over `rom_dir`. The Blowfish key is read from the ROM's own binary on first use of an
    /// encrypted entry.
    pub fn new(rom_dir: impl Into<PathBuf>) -> Self {
        Self {
            rom_dir: rom_dir.into(),
            override_dirs: Vec::new(),
            archives: Mutex::new(HashMap::new()),
            missing: Mutex::new(HashSet::new()),
            cipher: Mutex::new(None),
        }
    }

    /// Creates a VFS with an explicit key (tests, tools).
    pub fn with_key(rom_dir: impl Into<PathBuf>, key: &[u8]) -> Result<Self, RezError> {
        let vfs = Self::new(rom_dir);
        *vfs.cipher.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(Blowfish::new(key)?));
        Ok(vfs)
    }

    /// Creates a VFS over the ROM named by `CA_ROM` (the explicit resolver in `dc-rom`).
    pub fn from_env() -> Result<Self, RezError> {
        Ok(Self::new(dc_rom::resolve_rom()?))
    }

    pub fn rom_dir(&self) -> &Path {
        &self.rom_dir
    }

    /// Registers an override directory that takes precedence over vanilla entries and over previously
    /// registered override directories.
    pub fn add_override_dir(&mut self, path: impl Into<PathBuf>) {
        self.override_dirs.push(path.into());
    }

    pub fn override_dirs(&self) -> &[PathBuf] {
        &self.override_dirs
    }

    fn cipher(&self) -> Result<Arc<Blowfish>, RezError> {
        let mut slot = self.cipher.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(c) = slot.as_ref() {
            return Ok(Arc::clone(c));
        }
        let key = dc_rom::crypt_key(&self.rom_dir)?;
        let c = Arc::new(Blowfish::new(&key)?);
        *slot = Some(Arc::clone(&c));
        Ok(c)
    }

    fn decode(&self, name: &str, raw: Vec<u8>) -> Result<Vec<u8>, RezError> {
        if !is_text_name(name) || is_plain(&raw) {
            Ok(raw)
        } else {
            Ok(self.cipher()?.decrypt_bytes(&raw))
        }
    }

    fn archive(&self, rez_name: &str) -> ArchiveSlot {
        let key = rez_name.to_ascii_uppercase();
        let mut cache = self.archives.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(slot) = cache.get(&key) {
            return slot.clone();
        }
        let game_dir = self.rom_dir.join("Game");
        let rez_path = game_dir.join(rez_name);
        // a server ROM holds the archive as a folder of the same name
        let listed = || matches!(dc_rom::server_rom::manifest(&self.rom_dir), Ok(Some(m)) if m.archive(rez_name).is_some());
        // the names come from code and tables in another case than on disk (e.g. `GUNS_M_PV_Pistol.rez`): a
        // case-sensitive volume (an APFS or Linux option) needs the folder's own spelling
        let rez_path = if rez_path.is_file() || rez_path.is_dir() || listed() {
            Some(rez_path)
        } else {
            case_fold_lookup(&game_dir, rez_name)
        };
        let slot: ArchiveSlot = match rez_path {
            Some(p) => RezArchive::open(&p).map(|a| Some(Arc::new(a))),
            None => Ok(None),
        };
        cache.insert(key, slot.clone());
        slot
    }

    fn find_override_raw(&self, rez_name: &str, name: &str) -> Result<Option<Vec<u8>>, RezError> {
        if self.override_dirs.is_empty() {
            return Ok(None);
        }
        let rel_norm = name.replace('\\', "/");
        let basename = rel_norm.rsplit('/').next().unwrap_or(&rel_norm);
        let rez_stem = rez_name
            .strip_suffix(".rez")
            .or_else(|| rez_name.strip_suffix(".REZ"))
            .unwrap_or(rez_name);

        // Later override directories have higher priority.
        for dir in self.override_dirs.iter().rev() {
            let candidates = [
                dir.join(rez_name).join(&rel_norm),
                dir.join(rez_name).join(basename),
                dir.join(rez_stem).join(&rel_norm),
                dir.join(rez_stem).join(basename),
                dir.join(&rel_norm),
                dir.join(basename),
            ];
            for cand in &candidates {
                if cand.is_file() {
                    let raw = fs::read(cand).map_err(|e| io_err(cand, &e))?;
                    dc_rom::journal::note(|| format!("file:{}", cand.display()), &raw);
                    dc_rom::trace::note("file", cand, "", raw.len() as u64);
                    return Ok(Some(raw));
                }
            }
        }
        Ok(None)
    }

    /// Returns the bytes of entry `name` (full relative path) of `<rom_dir>/Game/<rez_name>`, shadowed by
    /// any registered override directory; `Ok(None)` if no layer has it. Text tables (`.txt`, `.gms`,
    /// `.bms`) are decrypted unless already plain; every other entry is returned as stored.
    pub fn entry(&self, rez_name: &str, name: &str) -> Result<Option<Vec<u8>>, RezError> {
        self.fetch(rez_name, name, false, true)
    }

    /// Stored bytes of entry `name` with no decryption at all (override files are returned as stored).
    pub fn entry_raw(&self, rez_name: &str, name: &str) -> Result<Option<Vec<u8>>, RezError> {
        self.fetch(rez_name, name, false, false)
    }

    /// The first `n` stored bytes of an entry (a header such as a texture's size), without reading the rest;
    /// `Ok(None)` when absent. Override folders answer first, as for [`RezVfs::entry_raw`].
    pub fn entry_head(
        &self,
        rez_name: &str,
        name: &str,
        n: usize,
    ) -> Result<Option<Vec<u8>>, RezError> {
        validate_name(name)?;
        validate_name(rez_name)?;
        if let Some(mut raw) = self.find_override_raw(rez_name, name)? {
            raw.truncate(n);
            return Ok(Some(raw));
        }
        match self.archive(rez_name)? {
            Some(arch) => arch.entry_head(name, n),
            None => Ok(None),
        }
    }

    /// Explicit basename fallback: like [`RezVfs::entry`] but resolves `name` by basename, succeeding only
    /// when the basename is unique within the archive.
    pub fn entry_by_basename(
        &self,
        rez_name: &str,
        name: &str,
    ) -> Result<Option<Vec<u8>>, RezError> {
        self.fetch(rez_name, name, true, true)
    }

    fn fetch(
        &self,
        rez_name: &str,
        name: &str,
        by_basename: bool,
        decode: bool,
    ) -> Result<Option<Vec<u8>>, RezError> {
        validate_name(name)?;
        validate_name(rez_name)?;
        if let Some(raw) = self.find_override_raw(rez_name, name)? {
            return if decode {
                self.decode(name, raw).map(Some)
            } else {
                Ok(Some(raw))
            };
        }
        let neg_key = (
            rez_name.to_ascii_uppercase(),
            format!(
                "{}{}",
                if by_basename { "B:" } else { "P:" },
                relpath_key(name)
            ),
        );
        if self
            .missing
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&neg_key)
        {
            return Ok(None);
        }
        let arch = self.archive(rez_name)?;
        let found = match &arch {
            Some(arch) if by_basename => arch.entry_raw_basename(name)?,
            Some(arch) => arch.entry_raw(name)?,
            None => None,
        };
        let decoded = arch.as_ref().is_some_and(|a| a.is_decoded());
        match found {
            Some(raw) if decode && !decoded => self.decode(name, raw).map(Some),
            Some(raw) => Ok(Some(raw)),
            None => {
                self.missing
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(neg_key);
                Ok(None)
            }
        }
    }

    /// Latin-1 text of an entry resolved through the explicit unique-basename fallback.
    pub fn entry_text_by_basename(
        &self,
        rez_name: &str,
        name: &str,
    ) -> Result<Option<String>, RezError> {
        Ok(self.entry_by_basename(rez_name, name)?.map(|mut bytes| {
            while bytes.last() == Some(&0) {
                bytes.pop();
            }
            latin1_decode(&bytes)
        }))
    }

    /// Text entry (Latin-1, trailing NULs stripped); alias of [`RezVfs::entry_text_latin1`].
    pub fn entry_text(&self, rez_name: &str, name: &str) -> Result<Option<String>, RezError> {
        self.entry_text_latin1(rez_name, name)
    }

    /// Returns the entry decoded as Latin-1 text with trailing NUL bytes stripped.
    pub fn entry_text_latin1(
        &self,
        rez_name: &str,
        name: &str,
    ) -> Result<Option<String>, RezError> {
        Ok(self.entry(rez_name, name)?.map(|mut bytes| {
            while bytes.last() == Some(&0) {
                bytes.pop();
            }
            latin1_decode(&bytes)
        }))
    }

    /// Lists all `(full_rel_path, offset, size)` entries in `<rom_dir>/Game/<rez_name>`.
    pub fn list_entries(&self, rez_name: &str) -> Result<Vec<(String, u64, u32)>, RezError> {
        validate_name(rez_name)?;
        let arch = self
            .archive(rez_name)?
            .ok_or_else(|| RezError::ArchiveNotFound(rez_name.to_string()))?;
        Ok(arch.entries().to_vec())
    }
}

/// Rejects empty, absolute, drive-qualified or `..`-containing names (they are joined onto directories).
fn validate_name(name: &str) -> Result<(), RezError> {
    let norm = name.replace('\\', "/");
    let bad = norm.is_empty()
        || norm.starts_with('/')
        || norm.contains(':')
        || norm.contains('\0')
        || norm.split('/').any(|c| c == "..");
    if bad {
        Err(RezError::BadName(name.to_string()))
    } else {
        Ok(())
    }
}

/// Convenience: entry `name` of `rez_name` in `rom_dir`.
pub fn entry(rom_dir: &Path, rez_name: &str, name: &str) -> Result<Option<Vec<u8>>, RezError> {
    RezVfs::new(rom_dir).entry(rez_name, name)
}

/// Convenience: Latin-1 text (trailing NULs stripped) of `rez_name` / `name` in `rom_dir`.
pub fn entry_text_latin1(
    rom_dir: &Path,
    rez_name: &str,
    name: &str,
) -> Result<Option<String>, RezError> {
    RezVfs::new(rom_dir).entry_text_latin1(rez_name, name)
}

#[cfg(test)]
mod tests;
