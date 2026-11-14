//! The device-session endpoints on the lobby's web port (TLS), next to `POST /identity/token` (LG-68). JSON in,
//! JSON out; request bodies are flat objects of strings (`identity::json_strings`).
//!
//! | path | request | 200 answer |
//! |---|---|---|
//! | `POST /identity/device` | `login, password, next_hash, name, platform` | `device, token, expires, account, nick, idle_days, device_expires` |
//! | `POST /identity/refresh` | `device, secret, next_hash` | `token, expires, account, nick, device_expires` |
//! | `POST /identity/signout` | `device, secret` | `{}` |
//!
//! `next_hash` is base64url (no padding) of exactly 32 bytes: the SHA-256 of the next secret's text, which the client
//! made and saved first. Refusals:
//!
//! - sign-in: `401 {"error":"wrong login or password"}` for every refusal of the account (unknown, wrong password,
//!   disabled, banned; counted in the LG-146 lockout like a game login); `429` while the name or the address is
//!   locked out, and past 20 new devices of the account in a day; `400` for a malformed request;
//! - refresh: `401 {"error":"signed_out"}` for an unknown or revoked device, a wrong, expired or reused secret, a
//!   banned, disabled or deleted account (one answer); `429` for the limits (30 refreshes per device in an hour, 120
//!   per address in 10 minutes, an address locked by LG-146); `400` for a malformed `next_hash` or one that repeats a
//!   spent hash;
//! - sign-out: always `200 {}` once the request is well formed (no answer tells whether it did anything); `429` past
//!   the address limit.
//!
//! Both device endpoints and the sign-out answer `404 {"error":"device sessions are not offered here"}` on a lobby
//! that does not issue tokens or has `identity.device_sessions = false`, as the token endpoint does on a lobby that
//! does not issue. `405` for another method.
//!
//! The contract the client (`dc-session`) relies on: a refresh answers anything but 200 only when it changed nothing
//! the client has to know about (the handler runs in one transaction: a database error rolls it back and answers
//! 500), and limits answer 429, never 401. A 401 means the presented secret was not spent and the proposed hash not
//! taken. On reuse the device is revoked AND every token of the account issued so far is ended.
// DC-DEFAULT: SES-1 the endpoints, SES-4 15-minute device tokens, SES-6 the limits, SES-9 the answers

use crate::db;
use crate::devsession::db as dsdb;
use crate::identity::{self, Step};
use crate::register::json_str;
use crate::state::Core;
use std::time::Instant;

pub const DEVICE_PATH: &str = "/identity/device";
pub const REFRESH_PATH: &str = "/identity/refresh";
pub const SIGNOUT_PATH: &str = "/identity/signout";

/// The refresh refusal, in the exact shape the client checks (`dc-session` `flow::SIGNED_OUT`).
pub const SIGNED_OUT: &str = "{\"error\":\"signed_out\"}";
pub const MSG_OFF: &str = "{\"error\":\"device sessions are not offered here\"}";
pub const MSG_LIMITED: &str = "{\"error\":\"too many attempts, try again later\"}";
/// Longest secret text read (a real one is 43 characters).
// DC-DEFAULT: SES-6 secrets over 64 characters are refused unread
pub const MAX_SECRET: usize = 64;
/// SES-6: refreshes per device in an hour.
// DC-DEFAULT: SES-6 (proposed) 30 per device per hour, 120 per address per 10 minutes, 20 new devices per account a day
pub const REFRESH_PER_DEVICE: usize = 30;
/// SES-6: refreshes and sign-outs per address in 10 minutes.
pub const REFRESH_PER_ADDR: usize = 120;
/// SES-6: devices one account may make in a day.
pub const DEVICES_PER_DAY: u32 = 20;
/// Revoked rows are kept this long for the owner's device history (SES-3).
pub const KEEP_REVOKED_SECS: i64 = 30 * 86_400;

/// The device sessions of an issuing lobby (SES-3, SES-4). Off unless the options turn them on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRules {
    pub enabled: bool,
    pub token_minutes: u32,
    pub idle_days: u32,
    pub max_devices: u32,
}

impl Default for DeviceRules {
    fn default() -> Self {
        let d = dc_config::IdentitySection::default();
        DeviceRules {
            enabled: false,
            token_minutes: d.device_token_minutes,
            idle_days: d.device_idle_days,
            max_devices: d.max_devices,
        }
    }
}

impl DeviceRules {
    pub fn from_options(o: &crate::cli::Options) -> DeviceRules {
        DeviceRules {
            enabled: o.identity_device_sessions,
            token_minutes: o.identity_device_token_minutes.clamp(1, 1440),
            idle_days: o.identity_device_idle_days.clamp(1, 3650),
            max_devices: o.identity_max_devices.clamp(1, 100),
        }
    }

    /// The device sessions turned on with the schema's numbers.
    #[cfg(test)]
    pub fn on() -> DeviceRules {
        DeviceRules {
            enabled: true,
            ..DeviceRules::default()
        }
    }

    pub fn idle_secs(&self) -> i64 {
        self.idle_days as i64 * 86_400
    }

    pub fn token_secs(&self) -> i64 {
        self.token_minutes as i64 * 60
    }

    /// The start-up line.
    pub fn describe(&self) -> String {
        if self.enabled {
            format!(
                "identity: device sessions on POST {DEVICE_PATH}, {REFRESH_PATH}, {SIGNOUT_PATH}: tokens of {} min, \
                 devices expire after {} idle days, at most {} per account",
                self.token_minutes, self.idle_days, self.max_devices
            )
        } else {
            "identity: device sessions off (identity.device_sessions = false)".into()
        }
    }
}

/// What a device sign-in asks for besides the password check.
#[derive(Clone)]
pub struct DeviceAsk {
    pub next_hash: [u8; dsdb::HASH_LEN],
    pub name: String,
    pub platform: String,
}

impl std::fmt::Debug for DeviceAsk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DeviceAsk({:?}, {:?}, hash hidden)",
            self.name, self.platform
        )
    }
}

/// True for the paths this module answers.
pub fn handles(path: &str) -> bool {
    matches!(
        path.trim_end_matches('/'),
        DEVICE_PATH | REFRESH_PATH | SIGNOUT_PATH
    )
}

/// True for the sign-in path (it checks a password off the lock, as the token endpoint).
pub fn is_sign_in(path: &str) -> bool {
    path.trim_end_matches('/') == DEVICE_PATH
}

fn offered(st: &Core) -> bool {
    st.identity.signer.is_some() && st.identity.devices.enabled
}

/// A `next_hash`: base64url (either alphabet accepted by the decoder, no padding needed) of exactly 32 bytes.
fn parse_hash(s: &str) -> Option<[u8; dsdb::HASH_LEN]> {
    if s.len() > 64 {
        return None;
    }
    identity::b64_decode(s)?.try_into().ok()
}

/// The source address masked for the device list: /24 for IPv4, /48 for IPv6.
// DC-DEFAULT: SES-10 (proposed) the place of a device is its last address masked to /24 or /48
pub fn place_of(peer: &str) -> String {
    match crate::limits::peer_ip(peer).parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => {
            let o = a.octets();
            format!("{}.{}.{}.0/24", o[0], o[1], o[2])
        }
        Ok(std::net::IpAddr::V6(a)) => {
            let s = a.segments();
            format!("{:x}:{:x}:{:x}::/48", s[0], s[1], s[2])
        }
        Err(_) => String::new(),
    }
}

fn fields(body: &[u8]) -> Option<Vec<(String, String)>> {
    std::str::from_utf8(body)
        .ok()
        .and_then(identity::json_strings)
}

fn field(f: &[(String, String)], k: &str) -> String {
    f.iter()
        .find(|(n, _)| n == k)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

/// The common gate of the three endpoints: None = go on.
fn gate(st: &Core, method: &str) -> Option<(u16, String)> {
    if !offered(st) {
        return Some((404, MSG_OFF.into()));
    }
    if method != "POST" {
        return Some((405, "{\"error\":\"use POST with a JSON object\"}".into()));
    }
    None
}

/// `POST /identity/device`, phase 1 (locked): request checks and the account lookup; the password check follows
/// off the lock ([`identity::Pending::check`]), then [`finish_sign_in`].
pub fn begin_sign_in(
    st: &mut Core,
    method: &str,
    body: &[u8],
    peer: &str,
    now: Instant,
) -> db::DbResult<Step> {
    if let Some(r) = gate(st, method) {
        return Ok(Step::Done(r));
    }
    let bad = |m: &str| Ok(Step::Done((400, format!("{{\"error\":{}}}", json_str(m)))));
    let Some(f) = fields(body) else {
        return bad("body must be a JSON object {login, password, next_hash, name, platform}");
    };
    let login = crate::auth::normalize_login(&field(&f, "login"));
    if login.is_empty() {
        return bad("login is missing");
    }
    let Some(next_hash) = parse_hash(&field(&f, "next_hash")) else {
        return bad("next_hash must be base64url of 32 bytes");
    };
    let (name, platform) = (field(&f, "name"), field(&f, "platform"));
    if name.len() > dsdb::MAX_LABEL || platform.len() > dsdb::MAX_LABEL {
        return bad("name and platform are at most 64 bytes");
    }
    identity::lookup(
        st,
        login,
        field(&f, "password"),
        peer,
        now,
        Some(Box::new(DeviceAsk {
            next_hash,
            name,
            platform,
        })),
        (429, MSG_LIMITED.into()),
    )
}

/// `POST /identity/device`, phase 2 (locked again): counts the outcome; on the right password makes the device and
/// issues its first token.
pub fn finish_sign_in(
    st: &mut Core,
    p: identity::Pending,
    check: crate::auth::PwCheck,
    now: Instant,
) -> db::DbResult<(u16, String)> {
    let Some(acc) = identity::settle(st, &p, check, now, "device sign-in")? else {
        return Ok((401, identity::MSG_REFUSED.into()));
    };
    let Some(ask) = p.device.as_ref() else {
        return Ok((400, "{\"error\":\"not a device sign-in\"}".into()));
    };
    let rules = st.identity.devices.clone();
    let unix = crate::clock::unix_now();
    // SES-6: new devices per account and day
    if dsdb::created_since(&st.db, acc.user_no, unix - 86_400)? >= DEVICES_PER_DAY {
        st.note(&format!(
            "device sessions: {} (account {}) made {DEVICES_PER_DAY} devices today; refused from {}",
            acc.login, acc.user_no, p.ip
        ));
        return Ok((429, MSG_LIMITED.into()));
    }
    let nick = st
        .character(acc.user_no)
        .map(|c| c.nick.clone())
        .unwrap_or_default();
    let (token, expires) =
        match st
            .identity
            .issue_for(acc.user_no, &acc.login, &nick, unix, rules.token_secs())
        {
            Ok(t) => t,
            Err(e) => {
                st.note(&format!("device sessions: no token for {}: {e}", acc.login));
                return Ok((400, format!("{{\"error\":{}}}", json_str(&e))));
            }
        };
    let Some(device) = dsdb::new_device_id() else {
        st.note("device sessions: the system random source failed; no device made");
        return Ok((503, "{\"error\":\"the server is busy\"}".into()));
    };
    let evicted = dsdb::create(
        &st.db,
        &dsdb::NewDevice {
            device: &device,
            user_no: acc.user_no,
            first_hash: &ask.next_hash,
            name: &ask.name,
            platform: &ask.platform,
            place: &place_of(&p.ip),
            now: unix,
        },
        rules.max_devices,
    )?;
    st.note(&format!(
        "device sessions: new device {device} for {} (account {}) from {}{}",
        acc.login,
        acc.user_no,
        p.ip,
        if evicted.is_empty() {
            String::new()
        } else {
            format!("; over the cap, revoked {}", evicted.join(", "))
        }
    ));
    Ok((
        200,
        format!(
            "{{\"device\":{},\"token\":{},\"expires\":{expires},\"account\":{},\"nick\":{},\"idle_days\":{},\
             \"device_expires\":{}}}",
            json_str(&device),
            json_str(&token),
            acc.user_no,
            json_str(&nick),
            rules.idle_days,
            unix + rules.idle_secs()
        ),
    ))
}

/// `POST /identity/refresh` and `POST /identity/signout`, whole, under the lock (no password check): (status, JSON).
/// The caller runs it in one transaction.
pub fn route(
    st: &mut Core,
    path: &str,
    method: &str,
    body: &[u8],
    peer: &str,
    now: Instant,
) -> db::DbResult<(u16, String)> {
    if let Some(r) = gate(st, method) {
        return Ok(r);
    }
    let Some(f) = fields(body) else {
        return Ok((400, "{\"error\":\"body must be a JSON object\"}".into()));
    };
    let ip = crate::limits::peer_ip(peer);
    let addr_key = [format!("addr:{ip}")];
    // SES-6: LG-146's address lockout and the address limit answer 429 (never 401: a 401 drops a client secret)
    if st.limits.login.locked(&addr_key, now).is_some()
        || st.limits.refresh_addr.hit(&ip, now) > REFRESH_PER_ADDR
    {
        return Ok((429, MSG_LIMITED.into()));
    }
    let device = field(&f, "device");
    let secret = field(&f, "secret");
    if path.trim_end_matches('/') == SIGNOUT_PATH {
        return sign_out(st, &device, &secret, &ip);
    }
    refresh(st, &f, &device, &secret, &ip, &addr_key, now)
}

fn sign_out(st: &mut Core, device: &str, secret: &str, ip: &str) -> db::DbResult<(u16, String)> {
    if dsdb::valid_device_id(device) && !secret.is_empty() && secret.len() <= MAX_SECRET {
        let unix = crate::clock::unix_now();
        if dsdb::sign_out(&st.db, device, secret.as_bytes(), unix)? {
            st.note(&format!("device sessions: {device} signed out from {ip}"));
        }
    }
    Ok((200, "{}".into()))
}

fn refresh(
    st: &mut Core,
    f: &[(String, String)],
    device: &str,
    secret: &str,
    ip: &str,
    addr_key: &[String; 1],
    now: Instant,
) -> db::DbResult<(u16, String)> {
    let signed_out = || Ok((401, SIGNED_OUT.into()));
    // a guess (unknown device, wrong secret) counts against the address like a failed login (LG-146)
    let counted = |st: &mut Core| {
        crate::auth::count_failure(st, addr_key, now);
        Ok((401, SIGNED_OUT.into()))
    };
    if !dsdb::valid_device_id(device) {
        return counted(st);
    }
    if st.limits.refresh_device.hit(device, now) > REFRESH_PER_DEVICE {
        return Ok((429, MSG_LIMITED.into()));
    }
    if secret.is_empty() || secret.len() > MAX_SECRET {
        return counted(st);
    }
    let Some(next_hash) = parse_hash(&field(f, "next_hash")) else {
        return Ok((
            400,
            "{\"error\":\"next_hash must be base64url of 32 bytes\"}".into(),
        ));
    };
    let Some(row) = dsdb::get(&st.db, device)? else {
        return counted(st);
    };
    if !row.live() {
        return signed_out();
    }
    // the account must exist and be allowed in before anything is spent (a refusal here changes nothing)
    let acc = match db::account_by_no(&st.db, row.user_no)? {
        Some(a) if !a.disabled && crate::gm::active_ban(st, a.user_no)?.is_none() => a,
        other => {
            let why = match other {
                None => "the account is gone",
                Some(a) if a.disabled => "the account is disabled",
                _ => "the account is banned",
            };
            st.note(&format!(
                "device sessions: refresh of {device} refused ({why}) from {ip}"
            ));
            return signed_out();
        }
    };
    let rules = st.identity.devices.clone();
    let unix = crate::clock::unix_now();
    let nick = st
        .character(acc.user_no)
        .map(|c| c.nick.clone())
        .unwrap_or_default();
    // signed before the spend, so a token that cannot be issued leaves the device as it was
    let (token, expires) =
        match st
            .identity
            .issue_for(acc.user_no, &acc.login, &nick, unix, rules.token_secs())
        {
            Ok(t) => t,
            Err(e) => {
                st.note(&format!("device sessions: no token for {}: {e}", acc.login));
                return Ok((400, format!("{{\"error\":{}}}", json_str(&e))));
            }
        };
    let ok = |device_expires: i64| {
        Ok((
            200,
            format!(
                "{{\"token\":{},\"expires\":{expires},\"account\":{},\"nick\":{},\"device_expires\":{device_expires}}}",
                json_str(&token),
                acc.user_no,
                json_str(&nick)
            ),
        ))
    };
    match dsdb::rotate(
        &st.db,
        device,
        secret.as_bytes(),
        &next_hash,
        &place_of(ip),
        unix,
        rules.idle_secs(),
    )? {
        dsdb::Spend::Rotated { .. } => ok(unix + rules.idle_secs()),
        dsdb::Spend::Replayed { .. } => ok(row.last_used + rules.idle_secs()),
        dsdb::Spend::Reuse { user_no } => {
            // a copy in other hands: the device is revoked, and every token of the account so far is ended
            dsdb::end_account_tokens(&st.db, user_no, unix + 1)?;
            st.note(&format!(
                "device sessions: a spent secret of {device} (account {user_no}) came back from {ip}: device \
                 revoked, the account's tokens ended"
            ));
            signed_out()
        }
        dsdb::Spend::Expired { user_no } => {
            st.note(&format!(
                "device sessions: {device} (account {user_no}) expired after {} idle days",
                rules.idle_days
            ));
            signed_out()
        }
        dsdb::Spend::Revoked { .. } => signed_out(),
        dsdb::Spend::BadNext => Ok((
            400,
            "{\"error\":\"next_hash repeats a spent secret\"}".into(),
        )),
        dsdb::Spend::Mismatch | dsdb::Spend::Unknown => counted(st),
    }
}

/// The periodic clean-up: expires idle devices and deletes rows revoked more than 30 days ago.
pub fn sweep(st: &mut Core) -> db::DbResult<()> {
    if !offered(st) {
        return Ok(());
    }
    let idle = st.identity.devices.idle_secs();
    let (expired, deleted) =
        dsdb::sweep(&st.db, crate::clock::unix_now(), idle, KEEP_REVOKED_SECS)?;
    if expired + deleted > 0 {
        st.note(&format!(
            "device sessions: {expired} idle device(s) expired, {deleted} old revoked row(s) removed"
        ));
    }
    Ok(())
}

/// The hourly sweep thread of an issuing lobby with device sessions on.
// DC-DEFAULT: SES-3 the sweep runs hourly
pub fn spawn_sweep(shared: crate::state::Shared) {
    if !offered(&crate::net::lock(&shared)) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("device-sweep".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
            let mut g = crate::net::lock(&shared);
            let r = g.scope(|st| sweep(st).map_err(Into::into));
            crate::net::report(&g, r);
        });
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
