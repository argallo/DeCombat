//! The device-session requests: sign in (make a device), refresh (spend a secret for a token), sign out.
//!
//! Bodies are flat JSON objects posted to the authority's web port through the caller's [`Transport`]:
//!
//! | path | body | 200 answer |
//! |---|---|---|
//! | [`DEVICE_PATH`] | `login, password, next_hash, name, platform` | `device, token, expires, account, nick` (+ `idle_days`) |
//! | [`REFRESH_PATH`] | `device, secret, next_hash` | `token, expires, account, nick` (+ `device_expires`) |
//! | [`SIGNOUT_PATH`] | `device, secret` | anything (the device is forgotten here either way) |
//!
//! A refusal is `401` (a refresh refusal only with the body `{"error":"signed_out"}`; any other 401 is "not
//! reachable"); `404` = the authority has no device sessions; `429` = slow down. Every other answer, and no
//! answer, counts as "not reachable". Times in answers are the authority's clock; this module reads no clock.
// DC-DEFAULT: SES-2 client-proposed rotation; SES-9 refusal deletes, unreachable keeps

use crate::secret::{Secret, SecretError};
use crate::store::{Credential, CredentialStore, StoreError, MAX_SECRETS};
use zeroize::Zeroizing;

/// Makes a device after a password check.
pub const DEVICE_PATH: &str = "/identity/device";
/// Spends a device secret for a token.
pub const REFRESH_PATH: &str = "/identity/refresh";
/// Signs this device out.
pub const SIGNOUT_PATH: &str = "/identity/signout";
/// The `error` of the authority's refresh refusal (`401 {"error":"signed_out"}`); a 401 without it is not a refusal.
pub const SIGNED_OUT: &str = "signed_out";
/// Largest token accepted from an answer.
pub const MAX_TOKEN: usize = 2048;
/// Device ids: `dv_` + 22 base64url characters.
const DEVICE_ID_LEN: usize = 25;

/// An HTTP answer: status and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}

/// No answer came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetError {
    /// False only when the request certainly did not reach the authority (no connection was made, nothing was
    /// written). When unsure, true.
    pub may_have_arrived: bool,
    /// Shown to the player; must not hold the body.
    pub message: String,
}

impl NetError {
    /// The request may have reached the authority (the safe assumption).
    pub fn unknown(message: impl Into<String>) -> NetError {
        NetError {
            may_have_arrived: true,
            message: message.into(),
        }
    }

    /// The request certainly did not leave (for example the connection was refused).
    pub fn not_sent(message: impl Into<String>) -> NetError {
        NetError {
            may_have_arrived: false,
            message: message.into(),
        }
    }
}

/// Posts a JSON body to a path of the authority (its TLS web port) and returns the answer. Implementations must not
/// log the body: it holds the password or a secret.
pub trait Transport {
    fn post(&mut self, path: &str, body: &str) -> Result<Reply, NetError>;
}

/// What the player's device is called in the owner's device list.
#[derive(Debug, Clone, Default)]
pub struct DeviceInfo {
    pub name: String,
    pub platform: String,
}

/// A token for the lobby login. Kept in memory only; `Debug` never shows the token.
#[derive(Clone)]
pub struct Session {
    pub token: Zeroizing<String>,
    /// The token's expiry, authority clock (unix seconds).
    pub expires: i64,
    pub account: u32,
    pub nick: String,
    /// When the device expires if it is not used again (authority clock), when the answer says.
    pub device_expires: Option<i64>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Session(account {}, expires {}, token hidden)",
            self.account, self.expires
        )
    }
}

/// A finished sign-in. `store_error` is set when the device could not be kept: the session works now, but the next
/// launch asks for the password again.
#[derive(Debug)]
pub struct SignIn {
    pub session: Session,
    pub device: String,
    pub store_error: Option<StoreError>,
}

/// Why a sign-in gave no token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInError {
    /// 401: wrong login or password (or a locked, disabled or banned account; one answer).
    Refused,
    /// 404: the authority has no device sessions (use the password token endpoint).
    NotSupported,
    /// 400: the authority refused the request's form.
    BadRequest,
    /// 429: too many attempts.
    Limited,
    /// No answer, or an answer that is not the endpoint's.
    Unreachable(String),
    /// The OS random source failed.
    Random,
}

/// Why a refresh gave no token. Only [`RefreshError::SignedOut`] removes the credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshError {
    /// No credential is kept for this authority and login.
    NoCredential,
    /// The authority refused every secret kept (revoked, expired, reused, banned or unknown): the credential is
    /// deleted and the player signs in with the password.
    SignedOut,
    /// 404: the authority has no device sessions (kept).
    NotSupported,
    /// 429: too many refreshes (kept).
    Limited,
    /// No answer or a server error (kept: the next try may work).
    Unreachable(String),
    /// The store failed. Before a request: nothing was sent.
    Store(StoreError),
    /// The OS random source failed (nothing was sent).
    Random,
}

impl From<SecretError> for RefreshError {
    fn from(_: SecretError) -> Self {
        RefreshError::Random
    }
}

fn json(body: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .filter(|v| v.is_object())
}

fn session_of(v: &serde_json::Value) -> Option<Session> {
    let token = v.get("token")?.as_str()?;
    if token.is_empty() || token.len() > MAX_TOKEN {
        return None;
    }
    Some(Session {
        token: Zeroizing::new(token.to_string()),
        expires: v.get("expires")?.as_i64()?,
        account: u32::try_from(v.get("account")?.as_u64()?).ok()?,
        nick: v
            .get("nick")
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .to_string(),
        device_expires: v.get("device_expires").and_then(|n| n.as_i64()),
    })
}

fn device_id_ok(s: &str) -> bool {
    s.len() == DEVICE_ID_LEN
        && s.starts_with("dv_")
        && s[3..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Signs in with the password and makes a device: makes the first secret, sends only its hash, and keeps the device
/// id and the secret in `store`. Whatever the store held for the same authority and login is cleared first without
/// being read ([`CredentialStore::force_clear`]): this is a fresh device, so nothing old is worth keeping, and a
/// damaged or unreadable old copy must not keep every later sign-in from being kept. The store's lock is held
/// throughout, so a refresh of another game instance on the profile cannot interleave with the save.
// DC-DEFAULT: SES-8 a sign-in replaces the old credential without reading it
pub fn sign_in(
    store: &dyn CredentialStore,
    net: &mut dyn Transport,
    authority: &str,
    login: &str,
    password: &str,
    info: &DeviceInfo,
) -> Result<SignIn, SignInError> {
    // a busy store does not stop the sign-in: the session works, it is just not kept (store_error says why)
    let lock = store.lock();
    let first = Secret::generate().map_err(|_| SignInError::Random)?;
    let body = Zeroizing::new(
        serde_json::json!({
            "login": login,
            "password": password,
            "next_hash": first.proposed_hash(),
            "name": info.name,
            "platform": info.platform,
        })
        .to_string(),
    );
    let reply = net
        .post(DEVICE_PATH, &body)
        .map_err(|e| SignInError::Unreachable(e.message))?;
    match reply.status {
        200 => {}
        401 => return Err(SignInError::Refused),
        404 => return Err(SignInError::NotSupported),
        400 => return Err(SignInError::BadRequest),
        429 => return Err(SignInError::Limited),
        s => return Err(SignInError::Unreachable(format!("{authority}: status {s}"))),
    }
    let not_ours = || SignInError::Unreachable(format!("{authority}: not a device answer"));
    let v = json(&reply.body).ok_or_else(not_ours)?;
    let device = v
        .get("device")
        .and_then(|d| d.as_str())
        .filter(|d| device_id_ok(d))
        .ok_or_else(not_ours)?
        .to_string();
    let session = session_of(&v).ok_or_else(not_ours)?;
    let cred = Credential {
        authority: authority.to_string(),
        login: login.trim().to_string(),
        device: device.clone(),
        secrets: vec![first],
        rev: 0,
    };
    let store_error = match lock {
        Err(e) => Some(e),
        Ok(_held) => store
            .force_clear(&cred.authority, &cred.login)
            .and_then(|()| store.save(&cred))
            .err(),
    };
    Ok(SignIn {
        session,
        device,
        store_error,
    })
}

/// Spends the kept secret of `authority` + `login` for a token, rotating it.
///
/// Before each request the next secret is generated and saved in front of the kept ones, so whatever happens to the
/// request the store holds every secret that may now be current, newest first. The secrets are tried in that order:
/// the newest that the authority accepts wins, a refused one (and the next secret proposed with it) is dropped, and
/// the last spent secret is never presented while a newer one may be current (that would read as reuse and revoke
/// the device). A refusal of every kept secret deletes the credential.
///
/// Any answer other than 200 means the authority changed nothing (its refresh is one transaction, and limits answer
/// 429, never 401), so the proposed secret is dropped again and the credential stays as it was; the same when the
/// request certainly did not leave. Only an answer lost after the request may have arrived (or a 200 that cannot be
/// read) keeps the proposed secret in front, undecided; the next refresh settles it.
// DC-DEFAULT: SES-2 save the next secret before sending; newest first; SES-9 refusal deletes, unreachable keeps
pub fn refresh(
    store: &dyn CredentialStore,
    net: &mut dyn Transport,
    authority: &str,
    login: &str,
) -> Result<Session, RefreshError> {
    // one refresh at a time per store: another game instance on this profile waits, then sees the rotated secret
    let _lock = store.lock().map_err(RefreshError::Store)?;
    let mut cred = store
        .load(authority, login)
        .map_err(RefreshError::Store)?
        .ok_or(RefreshError::NoCredential)?;
    cred.secrets.truncate(MAX_SECRETS);
    loop {
        let Some(spend) = cred.secrets.first().cloned() else {
            store
                .delete(&cred.authority, &cred.login)
                .map_err(RefreshError::Store)?;
            return Err(RefreshError::SignedOut);
        };
        let next = Secret::generate()?;
        let mut pending = cred.clone();
        pending.secrets.insert(0, next.clone());
        pending.secrets.truncate(MAX_SECRETS);
        // the next secret is durable before the request can rotate the authority to it
        store.save(&pending).map_err(RefreshError::Store)?;
        cred = pending;
        let body = Zeroizing::new(
            serde_json::json!({
                "device": cred.device,
                "secret": spend.expose(),
                "next_hash": next.proposed_hash(),
            })
            .to_string(),
        );
        // the authority certainly did not take `next`: forget it again (a failed save only leaves it undecided)
        let unpropose = |cred: &mut Credential| {
            cred.secrets.retain(|s| !s.same_as(&next));
            let _ = store.save(cred);
        };
        let reply = match net.post(REFRESH_PATH, &body) {
            Ok(r) => r,
            Err(e) => {
                if !e.may_have_arrived {
                    unpropose(&mut cred);
                }
                return Err(RefreshError::Unreachable(e.message));
            }
        };
        // a refusal counts only in the authority's own shape: a 401 of something in between (a captive portal, a
        // proxy) must not make the client drop secrets
        let refused = reply.status == 401
            && json(&reply.body)
                .and_then(|v| v.get("error")?.as_str().map(|e| e == SIGNED_OUT))
                .unwrap_or(false);
        let status = if reply.status == 401 && !refused {
            0
        } else {
            reply.status
        };
        if !matches!(status, 200 | 401) {
            unpropose(&mut cred);
        }
        match status {
            0 => {
                return Err(RefreshError::Unreachable(format!(
                    "{authority}: a refusal that is not the authority's"
                )))
            }
            200 => {
                let Some(session) = json(&reply.body).as_ref().and_then(session_of) else {
                    // the authority may have rotated: keep both, decide at the next refresh
                    return Err(RefreshError::Unreachable(format!(
                        "{authority}: not a refresh answer"
                    )));
                };
                cred.secrets = vec![next];
                // a failed save leaves [next, spend, ...] kept, which the next refresh resolves the same way
                let _ = store.save(&cred);
                return Ok(session);
            }
            401 => {
                // neither `spend` is current nor was `next` taken: drop both and try the next older secret
                cred.secrets
                    .retain(|s| !s.same_as(&spend) && !s.same_as(&next));
                if !cred.secrets.is_empty() {
                    store.save(&cred).map_err(RefreshError::Store)?;
                }
            }
            404 => return Err(RefreshError::NotSupported),
            429 => return Err(RefreshError::Limited),
            s => {
                return Err(RefreshError::Unreachable(format!(
                    "{authority}: status {s}"
                )))
            }
        }
    }
}

/// Signs this device out: tells the authority (best effort, with the newest secret) and forgets the credential
/// whatever the answer. Only the newest secret is sent: an older one may be the last spent one, which the
/// authority would read as reuse.
pub fn sign_out(
    store: &dyn CredentialStore,
    net: &mut dyn Transport,
    authority: &str,
    login: &str,
) -> Result<(), StoreError> {
    let _lock = store.lock()?;
    let cred = match store.load(authority, login) {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(()),
        // the credential cannot be read, so the authority cannot be told; the local copies still go, and the
        // read error is reported (with the removal's error when that fails too)
        // cleared without reading (a damaged copy cannot be parsed for its removal)
        Err(e) => {
            return Err(match store.force_clear(authority, login) {
                Ok(()) => e,
                Err(d) => StoreError(format!("{e}; {d}")),
            })
        }
    };
    if let Some(s) = cred.secrets.first() {
        let body = Zeroizing::new(
            serde_json::json!({"device": cred.device, "secret": s.expose()}).to_string(),
        );
        let _ = net.post(SIGNOUT_PATH, &body);
    }
    store.delete(&cred.authority, &cred.login)
}
