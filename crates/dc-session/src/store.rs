//! Where a device credential is kept between launches.
//!
//! One credential per (authority, login). [`CredentialStore`] is the seam for operating-system credential stores;
//! [`FileStore`] is the portable store and the fallback everywhere: one JSON file in the profile folder, replaced
//! atomically (written to a temporary file, synced, renamed over the old one) and created owner-only (mode 0600) on
//! Unix. On Windows the file gets no permissions of its own: it inherits the profile folder's, so the profile
//! folder must be one only the player's own user can read (the per-user application data folder is; a folder shared
//! by several users, or a portable install on a shared drive, is not). The operating system's credential store is
//! the better place there and the file is only its fallback.
// DC-DEFAULT: SES-8 the store: one entry per authority and login, the owner-only file

use crate::lock::FileLock;
use crate::secret::Secret;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use zeroize::Zeroizing;

/// The file name in the profile folder.
pub const STORE_FILE: &str = "device_session.json";
/// Secrets kept per credential: the current one and the ones a lost answer left undecided. Only an answer lost after
/// the request may have arrived adds one; any answer settles the list, so reaching the cap takes that many lost
/// answers in a row (the oldest is dropped then).
pub const MAX_SECRETS: usize = 8;
/// Largest store file read.
const MAX_FILE: u64 = 1 << 20;

/// A device credential: which device of which account at which authority, and the secrets that may be current,
/// newest first. `Debug` never shows the secrets.
#[derive(Clone)]
pub struct Credential {
    /// The authority's key (its address as configured): a credential is only ever offered to its own authority.
    pub authority: String,
    /// The login name it was made for.
    pub login: String,
    /// The device id the authority named (`dv_...`).
    pub device: String,
    /// Newest first; the first is tried first.
    pub secrets: Vec<Secret>,
    /// How many saves of [`FallbackStore`] made this copy: of two copies of one credential, the higher is the newer.
    /// 0 for a copy saved elsewhere (and in older store files).
    pub rev: u64,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Credential({} at {}, device {}, {} secret(s) hidden)",
            self.login,
            self.authority,
            self.device,
            self.secrets.len()
        )
    }
}

impl Credential {
    /// True when this credential belongs to `authority` and `login` (login names compare without case).
    pub fn fits(&self, authority: &str, login: &str) -> bool {
        self.authority == authority && self.login.eq_ignore_ascii_case(login.trim())
    }
}

/// A store failed. The text names the operation and the OS error, never a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// Holds other users of a store (other game instances, other threads) off until dropped.
#[derive(Debug, Default)]
pub struct StoreLock {
    /// Released when dropped.
    _held: Option<FileLock>,
}

impl StoreLock {
    /// A lock that holds nothing off (a store with nobody to share with).
    pub fn none() -> StoreLock {
        StoreLock { _held: None }
    }
}

/// Keeps device credentials. `save` must be durable when it returns `Ok`: the refresh flow sends a request only
/// after the next secret is saved.
pub trait CredentialStore {
    /// Takes the store for a whole refresh or sign-out (load, saves and the requests between them), so two game
    /// instances on one profile never spend the same secret. Re-entrant within one thread. The default holds
    /// nothing off (a store only this process sees).
    fn lock(&self) -> Result<StoreLock, StoreError> {
        Ok(StoreLock::none())
    }
    /// The credential of `authority` + `login`, if one is kept.
    fn load(&self, authority: &str, login: &str) -> Result<Option<Credential>, StoreError>;
    /// Keeps `cred`, replacing the one of the same authority and login.
    fn save(&self, cred: &Credential) -> Result<(), StoreError>;
    /// Forgets the credential of `authority` + `login` (nothing kept is not an error).
    fn delete(&self, authority: &str, login: &str) -> Result<(), StoreError>;
    /// Forgets the credential of `authority` + `login` WITHOUT reading it, also when it cannot be read (damaged,
    /// oversized, an impossible revision). Only for a fresh sign-in (the old credential is replaced anyway) and a
    /// sign-out; a refresh never calls it (it fails closed). The default is [`CredentialStore::delete`], right for
    /// a store that cannot hold anything unreadable.
    fn force_clear(&self, authority: &str, login: &str) -> Result<(), StoreError> {
        self.delete(authority, login)
    }
}

/// Keeps credentials in memory only (nothing survives the process).
#[derive(Default)]
pub struct MemoryStore(Mutex<Vec<Credential>>);

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }
}

impl CredentialStore for MemoryStore {
    fn load(&self, authority: &str, login: &str) -> Result<Option<Credential>, StoreError> {
        let v = self.0.lock().map_err(|_| StoreError("store lock".into()))?;
        Ok(v.iter().find(|c| c.fits(authority, login)).cloned())
    }

    fn save(&self, cred: &Credential) -> Result<(), StoreError> {
        let mut v = self.0.lock().map_err(|_| StoreError("store lock".into()))?;
        v.retain(|c| !c.fits(&cred.authority, &cred.login));
        v.push(cred.clone());
        Ok(())
    }

    fn delete(&self, authority: &str, login: &str) -> Result<(), StoreError> {
        let mut v = self.0.lock().map_err(|_| StoreError("store lock".into()))?;
        v.retain(|c| !c.fits(authority, login));
        Ok(())
    }
}

/// Keeps credentials in one JSON file (normally [`STORE_FILE`] in the profile folder).
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
    wait: Duration,
}

/// Temporary file names of this process (one per write, so two writers never share one).
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> FileStore {
        FileStore {
            path: path.into(),
            wait: crate::lock::WAIT,
        }
    }

    /// The same store with another bounded wait for the lock.
    pub fn with_lock_wait(mut self, wait: Duration) -> FileStore {
        self.wait = wait;
        self
    }

    fn lock_path(&self) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(".lock");
        PathBuf::from(p)
    }

    fn take_lock(&self) -> Result<FileLock, StoreError> {
        FileLock::acquire(&self.lock_path(), self.wait)
            .map_err(|e| StoreError(format!("locking the device store: {e}")))
    }

    /// The store of a profile folder.
    pub fn in_profile(dir: &Path) -> FileStore {
        FileStore::new(dir.join(STORE_FILE))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every entry of the file; a missing file is empty. An unreadable, oversized or damaged file is an error, never
    /// "empty": a save over it would wipe the credentials of other authorities. The player (or the game's sign-out)
    /// removes such a file; this store never overwrites it.
    fn read_all(&self) -> Result<Vec<Credential>, StoreError> {
        let f = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError(format!("reading the device store: {e}"))),
        };
        let mut text = Zeroizing::new(String::new());
        f.take(MAX_FILE + 1)
            .read_to_string(&mut text)
            .map_err(|e| StoreError(format!("reading the device store: {e}")))?;
        if text.len() as u64 > MAX_FILE {
            return Err(StoreError(format!(
                "the device store {} is larger than {} bytes; it was left as it is",
                self.path.display(),
                MAX_FILE
            )));
        }
        parse(&text).ok_or_else(|| {
            StoreError(format!(
                "the device store {} is damaged; it was left as it is (remove it to sign in again)",
                self.path.display()
            ))
        })
    }

    fn write_all(&self, entries: &[Credential]) -> Result<(), StoreError> {
        let fail = |what: &str, e: io::Error| StoreError(format!("{what} the device store: {e}"));
        if entries.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(fail("removing", e)),
                _ => Ok(()),
            };
        }
        if let Some(d) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(d).map_err(|e| fail("creating the folder of", e))?;
        }
        let text = Zeroizing::new(render(entries));
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(format!(
            ".{}-{}.tmp",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let tmp = PathBuf::from(tmp);
        // a name of this write only; a left-over file of that name (an earlier process with the same id) goes first
        let _ = std::fs::remove_file(&tmp);
        {
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.mode(0o600);
            }
            let mut f = o.open(&tmp).map_err(|e| fail("writing", e))?;
            let r = f.write_all(text.as_bytes()).and_then(|_| f.sync_all());
            if let Err(e) = r {
                drop(f);
                let _ = std::fs::remove_file(&tmp);
                return Err(fail("writing", e));
            }
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            fail("replacing", e)
        })?;
        // the rename itself is durable once the folder is synced (Unix; Windows has no folder handle to sync)
        #[cfg(unix)]
        if let Some(d) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            if let Ok(dir) = std::fs::File::open(d) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }
}

impl CredentialStore for FileStore {
    fn lock(&self) -> Result<StoreLock, StoreError> {
        Ok(StoreLock {
            _held: Some(self.take_lock()?),
        })
    }

    fn load(&self, authority: &str, login: &str) -> Result<Option<Credential>, StoreError> {
        Ok(self
            .read_all()?
            .into_iter()
            .find(|c| c.fits(authority, login)))
    }

    fn save(&self, cred: &Credential) -> Result<(), StoreError> {
        // read, change and write under the lock: another writer's entry is never lost
        let _lock = self.take_lock()?;
        let mut all = self.read_all()?;
        all.retain(|c| !c.fits(&cred.authority, &cred.login));
        all.push(cred.clone());
        self.write_all(&all)
    }

    fn delete(&self, authority: &str, login: &str) -> Result<(), StoreError> {
        let _lock = self.take_lock()?;
        let mut all = self.read_all()?;
        let before = all.len();
        all.retain(|c| !c.fits(authority, login));
        if all.len() == before {
            return Ok(());
        }
        self.write_all(&all)
    }

    /// A readable file loses this credential as in [`CredentialStore::delete`]. A file that cannot be read is moved
    /// aside, unparsed, to `device_session.json.damaged-<unix seconds>-<process id>` next to it: it may hold other
    /// logins' credentials, which are not silently destroyed (a person can still look at them), and it keeps its
    /// owner-only permissions (a rename does not change them). The store starts empty after that.
    // DC-DEFAULT: SES-8 a damaged store file is moved aside on a sign-in or sign-out, never parsed or overwritten
    fn force_clear(&self, authority: &str, login: &str) -> Result<(), StoreError> {
        let _lock = self.take_lock()?;
        if self.read_all().is_ok() {
            return self.delete(authority, login);
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut aside = self.path.clone().into_os_string();
        aside.push(format!(".damaged-{secs}-{}", std::process::id()));
        match std::fs::rename(&self.path, PathBuf::from(aside)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(StoreError(format!(
                "moving the damaged device store aside: {e}"
            ))),
            _ => Ok(()),
        }
    }
}

/// One entry as JSON (the file's `devices` element, and the Windows credential's blob).
fn entry_value(c: &Credential) -> serde_json::Value {
    serde_json::json!({
        "authority": c.authority,
        "login": c.login,
        "device": c.device,
        "secrets": c.secrets.iter().take(MAX_SECRETS).map(|s| s.expose()).collect::<Vec<_>>(),
        "rev": c.rev,
    })
}

/// One entry from JSON; None unless every part is what [`entry_value`] writes.
fn entry_of(e: &serde_json::Value) -> Option<Credential> {
    let s = |k: &str| e.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let list = e.get("secrets")?.as_array()?;
    if list.is_empty() || list.len() > MAX_SECRETS {
        return None;
    }
    let secrets = list
        .iter()
        .map(|x| Secret::from_text(x.as_str()?))
        .collect::<Option<Vec<Secret>>>()?;
    let c = Credential {
        authority: s("authority")?,
        login: s("login")?,
        device: s("device")?,
        secrets,
        rev: match e.get("rev") {
            None => 0,
            Some(r) => r.as_u64()?,
        },
    };
    (!c.device.is_empty()).then_some(c)
}

/// One credential as text (an OS store's secret blob).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn render_one(c: &Credential) -> String {
    entry_value(c).to_string()
}

/// One credential from [`render_one`]'s text.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_one(text: &str) -> Option<Credential> {
    entry_of(&serde_json::from_str::<serde_json::Value>(text).ok()?)
}

/// The file's text: `{"version":1,"devices":[{"authority","login","device","secrets":[...]}]}`.
fn render(entries: &[Credential]) -> String {
    let devices: Vec<serde_json::Value> = entries.iter().map(entry_value).collect();
    serde_json::json!({"version": 1, "devices": devices}).to_string()
}

/// The entries of a store file; None when any part of it is not what [`render`] writes (nothing is dropped
/// silently: a save over a partly read file would lose the rest).
fn parse(text: &str) -> Option<Vec<Credential>> {
    let v = serde_json::from_str::<serde_json::Value>(text).ok()?;
    if v.get("version").and_then(|n| n.as_u64()) != Some(1) {
        return None;
    }
    v.get("devices")?.as_array()?.iter().map(entry_of).collect()
}

/// An operating-system store with a file store behind it: the credential lives in `primary`; when `primary` can be
/// read but refuses a save, in `fallback`. The rules keep a spent secret from ever being presented again (the
/// authority would read it as reuse and revoke the device):
///
/// - Every copy carries a revision. A save reads BOTH stores first and stamps its copy one above the highest
///   revision it found (and above the revision of the credential it was given), so a successful save's copy is
///   strictly newer than every copy that exists. No clock is involved.
/// - A save that cannot read both stores fails before writing anything. After writing its copy it removes the copy
///   of the other store; when that removal fails, the save fails too (the caller then sends nothing). So after a
///   successful save exactly one store holds a copy.
/// - `load` reads both stores and fails closed: when either cannot be read it returns the error, never the other
///   store's copy (that copy may be older than the unreadable one). With copies in both (left by a failed save) the
///   higher revision wins; two different copies with one revision are an error, never a guess.
///
/// Hence a load never returns a copy older than the last successful save. The cost: while either store cannot be
/// read, the stored sign-in cannot be used (refresh reports a store error and the player signs in with the
/// password, which keeps nothing until both stores can be read again); the fallback only takes over a save the
/// operating-system store refuses while it can still be read.
// DC-DEFAULT: SES-8 the fallback store: revisions without a clock, fail closed
pub struct FallbackStore<P, F> {
    primary: P,
    fallback: F,
}

impl<P: CredentialStore, F: CredentialStore> FallbackStore<P, F> {
    pub fn new(primary: P, fallback: F) -> FallbackStore<P, F> {
        FallbackStore { primary, fallback }
    }

    /// Both stores' copies, or the first read error.
    fn both(
        &self,
        authority: &str,
        login: &str,
    ) -> Result<(Option<Credential>, Option<Credential>), StoreError> {
        let a = self.primary.load(authority, login)?;
        let b = self.fallback.load(authority, login)?;
        Ok((a, b))
    }
}

/// True when two copies hold the same credential (device and secrets, compared in constant time per secret).
fn same_copy(a: &Credential, b: &Credential) -> bool {
    a.device == b.device
        && a.secrets.len() == b.secrets.len()
        && a.secrets
            .iter()
            .zip(&b.secrets)
            .fold(true, |all, (x, y)| all & x.same_as(y))
}

impl<P: CredentialStore, F: CredentialStore> CredentialStore for FallbackStore<P, F> {
    /// The fallback's lock (the profile's lock file) holds other instances off both stores.
    fn lock(&self) -> Result<StoreLock, StoreError> {
        self.fallback.lock()
    }

    fn load(&self, authority: &str, login: &str) -> Result<Option<Credential>, StoreError> {
        match self.both(authority, login)? {
            (Some(a), Some(b)) => {
                if a.rev == b.rev && !same_copy(&a, &b) {
                    return Err(StoreError(
                        "the two copies of this device's credential disagree; sign in again".into(),
                    ));
                }
                Ok(Some(if b.rev > a.rev { b } else { a }))
            }
            (a, b) => Ok(a.or(b)),
        }
    }

    fn save(&self, cred: &Credential) -> Result<(), StoreError> {
        // every copy that exists must be seen, or the new revision could be at or below an unseen one
        let (a, b) = self.both(&cred.authority, &cred.login)?;
        let highest = [a.map(|c| c.rev), b.map(|c| c.rev), Some(cred.rev)]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(0);
        let mut c = cred.clone();
        c.rev = highest
            .checked_add(1)
            .ok_or_else(|| StoreError("the credential's revision is exhausted".into()))?;
        // the copy in one store, then the other store's copy removed: only then is the save done
        match self.primary.save(&c) {
            Ok(()) => self.fallback.delete(&c.authority, &c.login),
            Err(_) => {
                self.fallback.save(&c)?;
                self.primary.delete(&c.authority, &c.login)
            }
        }
    }

    fn delete(&self, authority: &str, login: &str) -> Result<(), StoreError> {
        let a = self.primary.delete(authority, login);
        let b = self.fallback.delete(authority, login);
        a.and(b)
    }

    /// Both stores, each without reading; both must succeed.
    fn force_clear(&self, authority: &str, login: &str) -> Result<(), StoreError> {
        let a = self.primary.force_clear(authority, login);
        let b = self.fallback.force_clear(authority, login);
        a.and(b)
    }
}
/// The store a game uses for a profile folder: the Windows Credential Manager backed by the profile's file on
/// Windows, the profile's owner-only file elsewhere.
pub fn default_store(profile: &std::path::Path) -> Box<dyn CredentialStore + Send + Sync> {
    #[cfg(windows)]
    {
        Box::new(FallbackStore::new(
            crate::win::WinStore::new(),
            FileStore::in_profile(profile),
        ))
    }
    #[cfg(not(windows))]
    {
        Box::new(FileStore::in_profile(profile))
    }
}
