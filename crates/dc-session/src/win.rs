//! The Windows credential store (Credential Manager) for device credentials: one generic credential per authority
//! and login, target `DeCombat/<authority>/<login in lower case>`, kept for the signed-in Windows user on this
//! machine (local-machine persistence: it does not roam). The secret list travels as the credential's blob, in the
//! same JSON form as one entry of the file store. Windows ties the credential to the user's logon; another user of
//! the machine cannot read it (malware running as the same user can: out of scope, as for every user-level store).
// DC-DEFAULT: SES-8 the Windows credential store, target `DeCombat/<authority>/<login>`

use crate::store::{parse_one, render_one, Credential, CredentialStore, StoreError};
use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};
use zeroize::Zeroizing;

/// The target prefix of the game's credentials.
pub const PREFIX: &str = "DeCombat";
/// Largest blob Windows keeps for a generic credential (5 * 512 bytes, Microsoft's documented limit).
const MAX_BLOB: usize = 2560;

/// Device credentials in the Windows Credential Manager.
#[derive(Debug, Clone)]
pub struct WinStore {
    prefix: String,
}

impl Default for WinStore {
    fn default() -> Self {
        WinStore::new()
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn os_error(what: &str) -> StoreError {
    // SAFETY: reads the calling thread's last error code; no memory is touched
    let code = unsafe { GetLastError() };
    StoreError(format!("{what} the Windows credential store: error {code}"))
}

impl WinStore {
    pub fn new() -> WinStore {
        WinStore::with_prefix(PREFIX)
    }

    /// A store under another target prefix (tests use a unique one, so a player's real credentials are never touched).
    pub fn with_prefix(prefix: &str) -> WinStore {
        WinStore {
            prefix: prefix.to_string(),
        }
    }

    /// Writes `blob` as the entry's value as it is (tests make damaged and oversized entries with it).
    #[cfg(test)]
    pub(crate) fn write_raw(
        &self,
        authority: &str,
        login: &str,
        blob: &[u8],
    ) -> Result<(), StoreError> {
        let mut target = wide(&self.target(authority, login));
        // SAFETY: as in save: an all-zero CREDENTIALW with fields pointing into buffers that outlive the call
        let ok = unsafe {
            let mut c: CREDENTIALW = std::mem::zeroed();
            c.Type = CRED_TYPE_GENERIC;
            c.TargetName = target.as_mut_ptr();
            c.CredentialBlobSize = blob.len() as u32;
            c.CredentialBlob = blob.as_ptr() as *mut u8;
            c.Persist = CRED_PERSIST_LOCAL_MACHINE;
            CredWriteW(&c, 0)
        };
        if ok == 0 {
            return Err(os_error("writing"));
        }
        Ok(())
    }

    /// The credential's target name.
    pub fn target(&self, authority: &str, login: &str) -> String {
        format!(
            "{}/{}/{}",
            self.prefix,
            authority,
            login.trim().to_lowercase()
        )
    }
}

impl CredentialStore for WinStore {
    fn load(&self, authority: &str, login: &str) -> Result<Option<Credential>, StoreError> {
        let target = wide(&self.target(authority, login));
        let mut found: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `target` is a NUL-terminated UTF-16 string that outlives the call; `found` receives a buffer the
        // system allocates, released with CredFree below on every path
        let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) };
        if ok == 0 {
            // SAFETY: as in os_error
            if unsafe { GetLastError() } == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(os_error("reading"));
        }
        // SAFETY: on success `found` points to one CREDENTIALW whose blob holds CredentialBlobSize bytes
        let blob = unsafe {
            let c = &*found;
            let bytes = if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
            };
            Zeroizing::new(bytes.to_vec())
        };
        // SAFETY: frees the buffer CredReadW returned; it is not used after this
        unsafe { CredFree(found as *const _) };
        let text = std::str::from_utf8(&blob)
            .map_err(|_| StoreError("the Windows credential of this device is damaged".into()))?;
        let cred = parse_one(text)
            .filter(|c| c.fits(authority, login))
            .ok_or_else(|| StoreError("the Windows credential of this device is damaged".into()))?;
        Ok(Some(cred))
    }

    fn save(&self, cred: &Credential) -> Result<(), StoreError> {
        let blob = Zeroizing::new(render_one(cred).into_bytes());
        if blob.len() > MAX_BLOB {
            return Err(StoreError(
                "the device credential is too large for the Windows credential store".into(),
            ));
        }
        let mut target = wide(&self.target(&cred.authority, &cred.login));
        let mut user = wide(&cred.login);
        // SAFETY: an all-zero CREDENTIALW is valid (null pointers, zero sizes); the fields set below point into
        // buffers (`target`, `user`, `blob`) that outlive the CredWriteW call, which copies them
        let ok = unsafe {
            let mut c: CREDENTIALW = std::mem::zeroed();
            c.Type = CRED_TYPE_GENERIC;
            c.TargetName = target.as_mut_ptr();
            c.UserName = user.as_mut_ptr();
            c.CredentialBlobSize = blob.len() as u32;
            c.CredentialBlob = blob.as_ptr() as *mut u8;
            c.Persist = CRED_PERSIST_LOCAL_MACHINE;
            CredWriteW(&c, 0)
        };
        if ok == 0 {
            return Err(os_error("writing"));
        }
        Ok(())
    }

    fn delete(&self, authority: &str, login: &str) -> Result<(), StoreError> {
        // never reads the entry's value, so it also removes a damaged or oversized one: the trait's force_clear
        // (sign-in, sign-out) is this removal
        let target = wide(&self.target(authority, login));
        // SAFETY: `target` is a NUL-terminated UTF-16 string that outlives the call
        let ok = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        // SAFETY: as in os_error
        if ok == 0 && unsafe { GetLastError() } != ERROR_NOT_FOUND {
            return Err(os_error("removing from"));
        }
        Ok(())
    }
}

/// Tests that write the Credential Manager take this lock and run one at a time. This is a WORKAROUND; the root
/// cause is unproven. What was observed (2026-11-09, Windows 11, this crate's own unfiltered test runs): twice,
/// each time in the first run after a fresh build and while several tests wrote the Credential Manager at once
/// (different targets, different threads), a test deleted its credential (`CredDeleteW` succeeded) and the next
/// read of the same target found it again. Two failures in about 25 runs; none in about 20 runs since, with or
/// without this lock. It may be a race in the operating system's store under concurrent writers, a timing effect of
/// the first run, or a defect of ours that we did not find; serialising the tests hides it rather than explains it.
#[cfg(test)]
pub(crate) static TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The Credential Manager tests write real targets under a prefix unique to the run and remove them. They run one
/// at a time through [`TEST_SERIAL`], a workaround for a flake whose root cause is unproven (see there).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::Secret;
    use crate::store::{FallbackStore, FileStore};

    /// A store under a prefix of this test run only; every target it used is removed when it is dropped, so the
    /// player's own credentials are never read or touched.
    struct TestStore {
        store: WinStore,
        used: std::cell::RefCell<Vec<(String, String)>>,
    }

    impl TestStore {
        fn new(name: &str) -> TestStore {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            TestStore {
                store: WinStore::with_prefix(&format!(
                    "DeCombatTest-{name}-{}-{nanos}",
                    std::process::id()
                )),
                used: Default::default(),
            }
        }

        fn cred(&self, authority: &str, login: &str, n: usize) -> Credential {
            self.used
                .borrow_mut()
                .push((authority.to_string(), login.to_string()));
            Credential {
                authority: authority.into(),
                login: login.into(),
                device: format!("dv_{}", "Q".repeat(22)),
                secrets: (0..n).map(|_| Secret::generate().unwrap()).collect(),
                rev: 0,
            }
        }
    }

    impl Drop for TestStore {
        fn drop(&mut self) {
            for (a, l) in self.used.borrow().iter() {
                let _ = self.store.delete(a, l);
            }
        }
    }

    #[test]
    fn a_credential_round_trips_through_the_credential_manager() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let t = TestStore::new("rt");
        let s = &t.store;
        assert!(s.load("auth:1", "Player").unwrap().is_none());
        let c = t.cred("auth:1", "Player", 3);
        s.save(&c).unwrap();
        let back = s.load("auth:1", "player").unwrap().unwrap();
        assert_eq!(
            (back.device.as_str(), back.secrets.len()),
            (c.device.as_str(), 3)
        );
        for (a, b) in back.secrets.iter().zip(&c.secrets) {
            assert!(a.same_as(b));
        }
        // a save replaces; another authority is another credential
        let c2 = t.cred("auth:1", "Player", 1);
        s.save(&c2).unwrap();
        assert!(s.load("auth:1", "Player").unwrap().unwrap().secrets[0].same_as(&c2.secrets[0]));
        assert!(s.load("auth:2", "Player").unwrap().is_none());
        s.delete("auth:1", "Player").unwrap();
        assert!(s.load("auth:1", "Player").unwrap().is_none());
        s.delete("auth:1", "Player").unwrap();
        assert!(t.store.target("a", " Me ").ends_with("/a/me"));
    }

    #[test]
    fn the_file_store_backs_the_credential_manager() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let t = TestStore::new("fb");
        let dir = std::env::temp_dir().join(format!("dc-session-winfb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let both = FallbackStore::new(t.store.clone(), FileStore::in_profile(&dir));
        let c = t.cred("auth:1", "p", 2);
        both.save(&c).unwrap();
        // kept in the credential manager, not on disk
        assert!(t.store.load("auth:1", "p").unwrap().is_some());
        assert!(!dir.join(crate::store::STORE_FILE).exists());
        assert_eq!(both.load("auth:1", "p").unwrap().unwrap().secrets.len(), 2);
        both.delete("auth:1", "p").unwrap();
        assert!(both.load("auth:1", "p").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
