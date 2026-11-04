//! Identity tokens: one identity, many ledgers (project decision). Our own design, no 2009 counterpart.
//!
//! A player registers and logs in once at the identity authority (a lobby started with `--identity`,
//! `[identity] issue = true`). It hands out a signed token; any lobby that trusts the authority's public key takes
//! the token in place of a password and keeps its own account, character and progression for the player, keyed by
//! (issuer id, the authority's account number). The password never reaches a community lobby.
//!
//! **Key pair** (LG-68). Ed25519 (`ring`). The authority keeps its pair as a PKCS#8 PEM (`PRIVATE KEY`) in
//! `identity_key.pem` next to the DB (or `--identity-key-file`), made on the first start, written owner-only like
//! the TLS key. The public key is the raw 32 bytes in standard base64 (44 characters with one `=`); it is logged at
//! every start and printed by `dc-lobby-server identity [--db FILE] [--key-file FILE]` (the private key never is).
//!
//! **Token** (LG-68). `<b64url(payload)>.<b64url(signature)>`, both parts base64url without padding (RFC 4648 §5).
//! The payload is UTF-8 text, seven fields joined by `|` in this fixed order:
//!
//! ```text
//! dcid1|<issuer id>|<account number, decimal u32>|<login name>|<nickname or empty>|<issued, unix s>|<expires, unix s>
//! ```
//!
//! The signature is Ed25519 over the payload bytes exactly as carried (the decoded first part). `expires` =
//! `issued + identity.token_hours * 3600` (default 24 h). No field may hold `|` or a control character; the issuer
//! id is 1..64 of `A-Z a-z 0-9 . _ : -`. A token is valid when: both parts decode, the signature verifies, the payload
//! has exactly seven fields starting with `dcid1`, the issuer equals the configured one, `issued <= now + 300` and
//! `now <= expires + 300` (5 minutes clock skew), and `expires >= issued`.
//!
//! **Issuing** (LG-68). `POST /identity/token` on the lobby's web port (TLS, the clan / registration port) with a
//! JSON body `{"login":"...","password":"..."}` answers `200 {"token":"...","expires":<unix s>,"account":<u32>,
//! "nick":"..."}` or `401 {"error":"wrong login or password"}` (unknown name, wrong password, disabled account,
//! LG-146 lockout: one answer, LG-231). The failed attempts count in the login lockout (LG-146) like game logins.
//! `404` when the lobby does not issue, `405` for another method, `400` for a body that is not such an object.
//!
//! **Login** (LG-68, LG-69). The 2009 login request carries the token in its login-name field (0x200 +6,
//! wstr[4096]) as `tok:<token>` (an optional `LOCAL:` in front is dropped as for any login); the typed-text field
//! (+0x2006) is only a wstr[17] and cannot hold a token, so it is ignored for a token login. See `auth::on_login` and
//! [`local_account`] for the mapping of the token to a local account.
// DC-DEFAULT: LG-68 token format and verification, LG-69 local account mapping (project decision)

use crate::db;
use crate::state::Core;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// First payload field: the format version.
pub const FORMAT: &str = "dcid1";
/// Login-name prefix of a token login.
pub const LOGIN_PREFIX: &str = "tok:";
/// Allowed clock difference between issuer and verifier.
// DC-DEFAULT: LG-68 5 minutes skew
pub const SKEW_SECS: i64 = 300;
/// Default token lifetime.
// DC-DEFAULT: LG-68 24 hours
pub const DEFAULT_TOKEN_HOURS: u32 = 24;
/// The key file's name next to the DB.
pub const KEY_FILE: &str = "identity_key.pem";
/// Largest token accepted (a real one is about 200 characters).
pub const MAX_TOKEN: usize = 2048;
/// The HTTP path of the issuing endpoint.
pub const TOKEN_PATH: &str = "/identity/token";
/// Largest request body read by the endpoint.
pub const MAX_BODY: usize = 4096;

// ---------------------------------------------------------------------------------------------
// base64 (RFC 4648): standard with padding for the public key and the PEM, url-safe unpadded for tokens
// ---------------------------------------------------------------------------------------------

/// The base64 digit of a 6-bit value: A-Z, a-z, 0-9, then `+` `/` (standard) or `-` `_` (url-safe).
fn digit(v: u32, url: bool) -> char {
    let v = (v & 63) as u8;
    (match v {
        0..=25 => b'A' + v,
        26..=51 => b'a' + v - 26,
        52..=61 => b'0' + v - 52,
        62 => {
            if url {
                b'-'
            } else {
                b'+'
            }
        }
        _ => {
            if url {
                b'_'
            } else {
                b'/'
            }
        }
    }) as char
}

fn b64_encode(data: &[u8], url: bool, pad: bool) -> String {
    let mut o = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        let digits = c.len() + 1;
        for i in 0..4 {
            if i < digits {
                o.push(digit(n >> (18 - 6 * i), url));
            } else if pad {
                o.push('=');
            }
        }
    }
    o
}

/// Decodes either alphabet, with or without padding; None for any other character or a bad length.
pub(crate) fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // one leftover digit (6 bits) cannot come from whole bytes
    (s.len() % 4 != 1).then_some(out)
}

pub fn b64url(data: &[u8]) -> String {
    b64_encode(data, true, false)
}

pub fn b64std(data: &[u8]) -> String {
    b64_encode(data, false, true)
}

// ---------------------------------------------------------------------------------------------
// claims, signing, verification
// ---------------------------------------------------------------------------------------------

/// The fields of a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claims {
    pub issuer: String,
    pub account: u32,
    pub login: String,
    pub nick: String,
    pub issued: i64,
    pub expires: i64,
}

/// Why a token was refused (logged; the player sees the wrong-password box).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Malformed,
    BadSignature,
    WrongIssuer(String),
    Expired,
    NotYetValid,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Malformed => write!(f, "malformed token"),
            Refusal::BadSignature => write!(f, "bad signature"),
            Refusal::WrongIssuer(i) => write!(f, "issuer {i:?} is not the trusted one"),
            Refusal::Expired => write!(f, "token expired"),
            Refusal::NotYetValid => write!(f, "token issued in the future"),
        }
    }
}

/// A valid issuer id: 1..64 of `A-Z a-z 0-9 . _ : -`.
pub fn valid_issuer(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// A text a payload field may carry: no `|`, no control character.
pub fn valid_field(s: &str) -> bool {
    !s.chars().any(|c| c == '|' || c.is_control())
}

/// The payload text of `c`; Err when a field cannot be carried.
pub fn payload(c: &Claims) -> Result<String, String> {
    if !valid_issuer(&c.issuer) {
        return Err(format!(
            "issuer id {:?} is not 1..64 of A-Z a-z 0-9 . _ : -",
            c.issuer
        ));
    }
    if c.login.is_empty() || !valid_field(&c.login) {
        return Err("the login name cannot be carried in a token".into());
    }
    if !valid_field(&c.nick) {
        return Err("the nickname cannot be carried in a token".into());
    }
    Ok(format!(
        "{FORMAT}|{}|{}|{}|{}|{}|{}",
        c.issuer, c.account, c.login, c.nick, c.issued, c.expires
    ))
}

fn parse_payload(p: &str) -> Option<Claims> {
    let f: Vec<&str> = p.split('|').collect();
    if f.len() != 7 || f[0] != FORMAT || !valid_issuer(f[1]) || f[3].is_empty() {
        return None;
    }
    if f.iter().any(|x| x.chars().any(char::is_control)) {
        return None;
    }
    fn num<T: std::str::FromStr>(s: &str) -> Option<T> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    }
    Some(Claims {
        issuer: f[1].to_string(),
        account: num(f[2])?,
        login: f[3].to_string(),
        nick: f[4].to_string(),
        issued: num(f[5])?,
        expires: num(f[6])?,
    })
}

/// Signs `c` into a token.
pub fn sign(key: &Ed25519KeyPair, c: &Claims) -> Result<String, String> {
    let p = payload(c)?;
    let sig = key.sign(p.as_bytes());
    Ok(format!("{}.{}", b64url(p.as_bytes()), b64url(sig.as_ref())))
}

/// Largest unix time a token may carry (year 5138).
const MAX_UNIX: i64 = 100_000_000_000;

/// Verifies `token` against the issuer's raw 32-byte public key, the expected issuer id and `now` (unix s).
pub fn verify(token: &str, public_key: &[u8], issuer: &str, now: i64) -> Result<Claims, Refusal> {
    let token = token.trim();
    if token.len() > MAX_TOKEN {
        return Err(Refusal::Malformed);
    }
    let (p64, s64) = token.split_once('.').ok_or(Refusal::Malformed)?;
    let p = b64_decode(p64).ok_or(Refusal::Malformed)?;
    let sig = b64_decode(s64).ok_or(Refusal::Malformed)?;
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&p, &sig)
        .map_err(|_| Refusal::BadSignature)?;
    let text = std::str::from_utf8(&p).map_err(|_| Refusal::Malformed)?;
    let c = parse_payload(text).ok_or(Refusal::Malformed)?;
    if c.issuer != issuer {
        return Err(Refusal::WrongIssuer(c.issuer));
    }
    if c.expires < c.issued || c.issued.abs() > MAX_UNIX || c.expires.abs() > MAX_UNIX {
        return Err(Refusal::Malformed);
    }
    if c.issued > now.saturating_add(SKEW_SECS) {
        return Err(Refusal::NotYetValid);
    }
    if now > c.expires.saturating_add(SKEW_SECS) {
        return Err(Refusal::Expired);
    }
    Ok(c)
}

/// A public key given as base64 (either alphabet): the raw 32 bytes.
pub fn parse_public_key(s: &str) -> Result<Vec<u8>, String> {
    match b64_decode(s.trim()) {
        Some(k) if k.len() == 32 => Ok(k),
        _ => Err("the identity public key must be 32 bytes in base64 (44 characters)".into()),
    }
}

// ---------------------------------------------------------------------------------------------
// the key file
// ---------------------------------------------------------------------------------------------

/// The key file: `--identity-key-file`, else `identity_key.pem` in the DB's folder.
pub fn key_path(db: &Path, file: Option<&Path>) -> PathBuf {
    match file {
        Some(f) => f.to_path_buf(),
        None => crate::tls::default_dir(db).join(KEY_FILE),
    }
}

/// Loads the pair at `path`, making it when the file does not exist. Returns the pair and whether it was made.
pub fn load_or_create(path: &Path) -> Result<(Ed25519KeyPair, bool), String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::PrivateKeyDer;
    let mut created = false;
    if !path.exists() {
        let doc = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .map_err(|_| "could not make an identity key pair".to_string())?;
        let mut pem = String::from("-----BEGIN PRIVATE KEY-----\n");
        let b = b64std(doc.as_ref());
        for line in b.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).unwrap_or_default());
            pem.push('\n');
        }
        pem.push_str("-----END PRIVATE KEY-----\n");
        if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(d);
        }
        crate::tls::write_private(path, pem.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        created = true;
    }
    // the parser's own error text is not shown (it could quote key material); only the file name
    let bad = || format!("{}: not a readable Ed25519 PKCS#8 PEM key", path.display());
    let der = match PrivateKeyDer::from_pem_file(path).map_err(|_| bad())? {
        PrivateKeyDer::Pkcs8(k) => k,
        _ => return Err(bad()),
    };
    let kp = Ed25519KeyPair::from_pkcs8(der.secret_pkcs8_der())
        .or_else(|_| Ed25519KeyPair::from_pkcs8_maybe_unchecked(der.secret_pkcs8_der()))
        .map_err(|_| bad())?;
    Ok((kp, created))
}

/// The public key of `kp` in standard base64.
pub fn public_b64(kp: &Ed25519KeyPair) -> String {
    b64std(kp.public_key().as_ref())
}

// ---------------------------------------------------------------------------------------------
// the lobby's identity set-up
// ---------------------------------------------------------------------------------------------

/// What this lobby does with identity tokens: issue them (it has the key pair), accept them (it knows an issuer and
/// its public key), both, or neither (the default: standalone accounts only).
#[derive(Clone, Default)]
pub struct Authority {
    /// The trusted (or own) issuer id; empty = tokens off.
    pub issuer: String,
    /// The raw public key tokens are verified with.
    pub public_key: Option<Vec<u8>>,
    /// The signing pair: Some on an issuing lobby.
    pub signer: Option<Arc<Ed25519KeyPair>>,
    pub token_hours: u32,
}

impl std::fmt::Debug for Authority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // never the private key
        f.debug_struct("Authority")
            .field("issuer", &self.issuer)
            .field("accepts", &self.accepts())
            .field("issues", &self.signer.is_some())
            .finish()
    }
}

impl Authority {
    pub fn accepts(&self) -> bool {
        !self.issuer.is_empty() && self.public_key.is_some()
    }

    /// The issuing set-up for `issuer` with the pair `kp`; it accepts its own tokens.
    pub fn issuing(issuer: &str, kp: Ed25519KeyPair, token_hours: u32) -> Authority {
        Authority {
            issuer: issuer.to_string(),
            public_key: Some(kp.public_key().as_ref().to_vec()),
            signer: Some(Arc::new(kp)),
            token_hours: if token_hours == 0 {
                DEFAULT_TOKEN_HOURS
            } else {
                token_hours
            },
        }
    }

    /// The accepting set-up for `issuer` and its public key.
    pub fn accepting(issuer: &str, public_key: Vec<u8>) -> Authority {
        Authority {
            issuer: issuer.to_string(),
            public_key: Some(public_key),
            signer: None,
            token_hours: DEFAULT_TOKEN_HOURS,
        }
    }

    /// True when a token of this issuer names an account of this very DB (the issuing lobby itself).
    pub fn is_own(&self, c: &Claims) -> bool {
        self.signer.is_some() && c.issuer == self.issuer
    }

    /// A token for an account of this lobby.
    pub fn issue(
        &self,
        account: u32,
        login: &str,
        nick: &str,
        now: i64,
    ) -> Result<(String, i64), String> {
        let kp = self
            .signer
            .as_ref()
            .ok_or("this lobby does not issue identity tokens")?;
        let expires = now + self.token_hours as i64 * 3600;
        // a nickname that cannot be carried is left out (it is informational only)
        let nick = if valid_field(nick) { nick } else { "" };
        let t = sign(
            kp,
            &Claims {
                issuer: self.issuer.clone(),
                account,
                login: login.to_string(),
                nick: nick.to_string(),
                issued: now,
                expires,
            },
        )?;
        Ok((t, expires))
    }

    /// Verifies a token at `now`.
    pub fn check(&self, token: &str, now: i64) -> Result<Claims, Refusal> {
        match &self.public_key {
            Some(k) if !self.issuer.is_empty() => verify(token, k, &self.issuer, now),
            _ => Err(Refusal::WrongIssuer(String::new())),
        }
    }
}

/// Builds the lobby's set-up from its options. Err = a start-up refusal (exit 2).
pub fn from_options(o: &crate::cli::Options) -> Result<(Authority, Vec<String>), String> {
    let mut notes = Vec::new();
    let issuer = o.identity_issuer.trim();
    let key = o.identity_public_key.trim();
    if o.identity_issue {
        if !valid_issuer(issuer) {
            return Err("--identity needs --identity-issuer ID (identity.issuer: 1..64 of A-Z a-z 0-9 . _ : -)".into());
        }
        let path = key_path(&o.db, o.identity_key_file.as_deref());
        let (kp, created) = load_or_create(&path)?;
        let own = public_b64(&kp);
        if created {
            notes.push(format!("identity: made a new key pair {}", path.display()));
        }
        if !key.is_empty()
            && parse_public_key(key).ok().as_deref() != Some(kp.public_key().as_ref())
        {
            notes.push("identity: WARNING: identity.public_key differs from the own key pair; the own key is used".into());
        }
        let a = Authority::issuing(issuer, kp, o.identity_token_hours);
        notes.push(format!(
            "identity: issuing tokens as {issuer:?} ({} h) on POST {TOKEN_PATH} of the web port; public key {own}",
            a.token_hours
        ));
        return Ok((a, notes));
    }
    match (issuer.is_empty(), key.is_empty()) {
        (true, true) => Ok((Authority::default(), notes)),
        (false, false) => {
            if !valid_issuer(issuer) {
                return Err(format!(
                    "identity issuer {issuer:?}: 1..64 of A-Z a-z 0-9 . _ : -"
                ));
            }
            let k = parse_public_key(key)?;
            notes.push(format!(
                "identity: accepting tokens of {issuer:?} (public key {})",
                b64std(&k)
            ));
            Ok((Authority::accepting(issuer, k), notes))
        }
        _ => {
            Err("accepting identity tokens needs both --identity-issuer and --identity-key".into())
        }
    }
}

/// `dc-lobby-server identity [--db FILE] [--key-file FILE]`: prints the public key (the pair is made when missing).
pub fn run_subcommand(args: &[String]) -> i32 {
    let mut db: Option<PathBuf> = None;
    let mut file: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (k, inline) = match a.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (a.as_str(), None),
        };
        let mut val = || inline.clone().or_else(|| it.next().cloned());
        match k {
            "--db" => db = val().map(PathBuf::from),
            "--key-file" | "--identity-key-file" => file = val().map(PathBuf::from),
            "-h" | "--help" => {
                println!("usage: dc-lobby-server identity [--db PATH] [--key-file PATH]\n  prints the base64 public key of the lobby's identity key pair (made when missing)");
                return 0;
            }
            o => {
                eprintln!("dc-lobby-server identity: unknown argument {o:?}");
                return 2;
            }
        }
    }
    let db = db.unwrap_or_else(crate::cli::default_db);
    let path = key_path(&db, file.as_deref());
    match load_or_create(&path) {
        Ok((kp, created)) => {
            if created {
                eprintln!("made a new identity key pair {}", path.display());
            }
            println!("{}", public_b64(&kp));
            0
        }
        Err(e) => {
            eprintln!("dc-lobby-server identity: {e}");
            1
        }
    }
}

// ---------------------------------------------------------------------------------------------
// the local account of a token (LG-69)
// ---------------------------------------------------------------------------------------------

/// The local account of a verified token. On the issuing lobby the token's account number is the local one (its
/// login name must still match). Elsewhere the `identity_links` row of (issuer, account) names it; without one a new
/// password-less account with the token's login name is made and linked. A local account that already holds the
/// login name without being linked is NOT taken over: Err(reason) (the caller refuses with the wrong-password box).
// DC-DEFAULT: LG-69 local account mapping
pub fn local_account(st: &mut Core, c: &Claims) -> db::DbResult<Result<db::Account, String>> {
    if st.identity.is_own(c) {
        return Ok(match db::account_by_no(&st.db, c.account)? {
            Some(a) if a.login.eq_ignore_ascii_case(&c.login) => Ok(a),
            Some(_) => Err(format!(
                "own token of account {} names another login",
                c.account
            )),
            None => Err(format!("own token names no account {}", c.account)),
        });
    }
    if let Some(no) = db::identity_link(&st.db, &c.issuer, c.account)? {
        return Ok(
            db::account_by_no(&st.db, no)?.ok_or_else(|| format!("linked account {no} is gone"))
        );
    }
    if db::account_by_login(&st.db, &c.login)?.is_some() {
        return Ok(Err(format!(
            "login name {} is held by a local account not linked to {}:{}",
            c.login, c.issuer, c.account
        )));
    }
    let now = crate::clock::unix_now();
    let no = db::create_account_from(&st.db, &c.login, now, st.rules.first_account_no)?;
    db::link_identity(&st.db, &c.issuer, c.account, no, now)?;
    st.note(&format!(
        "identity: new local account {} ({}) for {}:{}",
        no, c.login, c.issuer, c.account
    ));
    Ok(db::account_by_no(&st.db, no)?.ok_or_else(|| "new account vanished".to_string()))
}

// ---------------------------------------------------------------------------------------------
// the issuing endpoint
// ---------------------------------------------------------------------------------------------

pub fn handles(path: &str) -> bool {
    path.trim_end_matches('/') == TOKEN_PATH
}

/// Reads a flat JSON object of string values (other value kinds are skipped); None when it is not one.
pub fn json_strings(s: &str) -> Option<Vec<(String, String)>> {
    let mut it = s.trim().chars().peekable();
    let ws = |it: &mut std::iter::Peekable<std::str::Chars>| {
        while it.peek().is_some_and(|c| c.is_whitespace()) {
            it.next();
        }
    };
    fn string(it: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
        if it.next()? != '"' {
            return None;
        }
        let mut o = String::new();
        loop {
            match it.next()? {
                '"' => return Some(o),
                '\\' => match it.next()? {
                    '"' => o.push('"'),
                    '\\' => o.push('\\'),
                    '/' => o.push('/'),
                    'b' => o.push('\u{8}'),
                    'f' => o.push('\u{c}'),
                    'n' => o.push('\n'),
                    'r' => o.push('\r'),
                    't' => o.push('\t'),
                    'u' => {
                        let hex = |it: &mut std::iter::Peekable<std::str::Chars>| {
                            let h: String = (0..4).filter_map(|_| it.next()).collect();
                            (h.len() == 4)
                                .then(|| u32::from_str_radix(&h, 16).ok())
                                .flatten()
                        };
                        let a = hex(it)?;
                        let ch = if (0xd800..0xdc00).contains(&a) {
                            if it.next()? != '\\' || it.next()? != 'u' {
                                return None;
                            }
                            let b = hex(it)?;
                            char::from_u32(
                                0x10000 + ((a - 0xd800) << 10) + (b.checked_sub(0xdc00)?),
                            )?
                        } else {
                            char::from_u32(a)?
                        };
                        o.push(ch);
                    }
                    _ => return None,
                },
                c => o.push(c),
            }
        }
    }
    ws(&mut it);
    if it.next()? != '{' {
        return None;
    }
    let mut out = Vec::new();
    ws(&mut it);
    if it.peek() == Some(&'}') {
        it.next();
        return Some(out);
    }
    loop {
        ws(&mut it);
        let k = string(&mut it)?;
        ws(&mut it);
        if it.next()? != ':' {
            return None;
        }
        ws(&mut it);
        if it.peek() == Some(&'"') {
            out.push((k, string(&mut it)?));
        } else {
            // a number / true / false / null: skipped
            while it.peek().is_some_and(|c| !matches!(c, ',' | '}')) {
                it.next();
            }
        }
        ws(&mut it);
        match it.next()? {
            ',' => continue,
            '}' => break,
            _ => return None,
        }
    }
    ws(&mut it);
    it.next().is_none().then_some(out)
}

pub(crate) const MSG_REFUSED: &str = "{\"error\":\"wrong login or password\"}";

/// What the first (locked) phase of `POST /identity/token` leaves for the password check.
pub enum Step {
    /// Answered without a password check.
    Done((u16, String)),
    /// The Argon2 verify still to run, outside the `Core` lock.
    Verify(Pending),
}

/// An account lookup waiting for its password verification.
pub struct Pending {
    /// `None`: unknown account (verified against a dummy hash, so it costs the same as a wrong password).
    acc: Option<db::Account>,
    /// A reason to refuse whatever the password is (disabled, banned): still costs one verify and is counted.
    refuse: Option<&'static str>,
    password: String,
    keys: [String; 2],
    ip: String,
    login: String,
}

/// A fixed Argon2id hash nobody knows the password of, for the unknown-account path.
fn dummy_phc() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| crate::pwhash::encode(b"dc-dummy-salt-0001", "dc-no-such-password"))
}

impl Pending {
    /// The limit keys of this request.
    pub fn keys(&self) -> &[String; 2] {
        &self.keys
    }

    /// The password check; takes no lock and is the slow part.
    pub fn check(&self) -> crate::auth::PwCheck {
        use crate::auth::PwCheck;
        match &self.acc {
            Some(acc) if self.refuse.is_none() => crate::auth::pw_verify(acc, &self.password),
            _ => {
                let _ = crate::pwhash::verify(dummy_phc(), &self.password);
                PwCheck::Bad
            }
        }
    }
}

/// `POST /identity/token`, whole, under one lock: (status, JSON body). The web server uses [`begin`], [`Pending::check`]
/// and [`finish`] so the Argon2 verify runs outside the lock.
#[cfg(test)]
pub fn route(
    st: &mut Core,
    method: &str,
    body: &[u8],
    peer: &str,
    now: Instant,
) -> db::DbResult<(u16, String)> {
    match begin(st, method, body, peer, now)? {
        Step::Done(r) => Ok(r),
        Step::Verify(p) => {
            let c = p.check();
            finish(st, p, c, now)
        }
    }
}

/// Phase 1 (locked): the request checks and the account lookup.
pub fn begin(
    st: &mut Core,
    method: &str,
    body: &[u8],
    peer: &str,
    now: Instant,
) -> db::DbResult<Step> {
    use crate::register::json_str;
    let done = |c: u16, b: String| Ok(Step::Done((c, b)));
    if st.identity.signer.is_none() {
        return done(
            404,
            "{\"error\":\"identity tokens are not issued here\"}".into(),
        );
    }
    if method != "POST" {
        return done(
            405,
            "{\"error\":\"use POST with {\\\"login\\\",\\\"password\\\"}\"}".into(),
        );
    }
    let Some(fields) = std::str::from_utf8(body).ok().and_then(json_strings) else {
        return done(
            400,
            "{\"error\":\"body must be a JSON object {\\\"login\\\",\\\"password\\\"}\"}".into(),
        );
    };
    let _ = json_str;
    let get = |k: &str| {
        fields
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let login = crate::auth::normalize_login(&get("login"));
    let password = get("password");
    if login.is_empty() {
        return done(400, "{\"error\":\"login is missing\"}".into());
    }
    // LG-146: the same lockout as the game login (name and address), counted alike; one answer for every refusal (LG-231)
    let keys = crate::limits::login_keys(&login, peer);
    if st.limits.login.locked(&keys, now).is_some() {
        return done(401, MSG_REFUSED.into());
    }
    let ip = crate::limits::peer_ip(peer);
    let acc = db::account_by_login(&st.db, &login)?;
    let refuse = match &acc {
        None => Some("unknown account"),
        Some(a) if a.disabled => Some("account disabled"),
        // LG-68: an account the lobby would refuse at login for a ban gets no token either
        Some(a) if crate::gm::active_ban(st, a.user_no)?.is_some() => Some("account banned"),
        _ => None,
    };
    Ok(Step::Verify(Pending {
        acc,
        refuse,
        password,
        keys,
        ip,
        login,
    }))
}

/// Phase 2 (locked again): count the outcome and issue the token.
pub fn finish(
    st: &mut Core,
    p: Pending,
    check: crate::auth::PwCheck,
    now: Instant,
) -> db::DbResult<(u16, String)> {
    use crate::register::json_str;
    let Pending {
        acc,
        refuse,
        password,
        keys,
        ip,
        login,
    } = p;
    let refused = |st: &mut Core, why: &str| {
        crate::auth::count_failure(st, &keys, now);
        st.note(&format!(
            "identity: token refused ({why}): {login} from {ip}"
        ));
        Ok((401, MSG_REFUSED.into()))
    };
    if let Some(why) = refuse {
        return refused(st, why);
    }
    let Some(acc) = acc else {
        return refused(st, "unknown account");
    };
    match check {
        crate::auth::PwCheck::Ok => {}
        crate::auth::PwCheck::OkUpgrade => crate::auth::store_password(st, acc.user_no, &password)?,
        crate::auth::PwCheck::Bad | crate::auth::PwCheck::NoPassword => {
            return refused(st, "password")
        }
    }
    st.limits.login.success(&keys[0]);
    let nick = st
        .character(acc.user_no)
        .map(|c| c.nick.clone())
        .unwrap_or_default();
    let unix = crate::clock::unix_now();
    let (token, expires) = match st.identity.issue(acc.user_no, &acc.login, &nick, unix) {
        Ok(t) => t,
        Err(e) => {
            st.note(&format!("identity: token refused for {}: {e}", acc.login));
            return Ok((400, format!("{{\"error\":{}}}", json_str(&e))));
        }
    };
    st.note(&format!(
        "identity: token issued for {} (account {}) from {ip}, valid until {expires}",
        acc.login, acc.user_no
    ));
    Ok((
        200,
        format!(
            "{{\"token\":{},\"expires\":{expires},\"account\":{},\"nick\":{}}}",
            json_str(&token),
            acc.user_no,
            json_str(&nick)
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> Ed25519KeyPair {
        let doc = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(doc.as_ref()).unwrap()
    }

    fn claims(now: i64) -> Claims {
        Claims {
            issuer: "decombat.main".into(),
            account: 1001,
            login: "alice".into(),
            nick: "Alice".into(),
            issued: now,
            expires: now + 24 * 3600,
        }
    }

    const NOW: i64 = 1_793_000_000;

    #[test]
    fn base64_both_alphabets_round_trip() {
        for n in 0..40u8 {
            let d: Vec<u8> = (0..n)
                .map(|i| i.wrapping_mul(37).wrapping_add(250))
                .collect();
            assert_eq!(b64_decode(&b64url(&d)).unwrap(), d);
            assert_eq!(b64_decode(&b64std(&d)).unwrap(), d);
        }
        assert_eq!(b64std(b"any carnal pleas"), "YW55IGNhcm5hbCBwbGVhcw==");
        assert!(b64_decode("A").is_none());
        assert!(b64_decode("ab$c").is_none());
    }

    #[test]
    fn a_token_round_trips() {
        let kp = pair();
        let t = sign(&kp, &claims(NOW)).unwrap();
        let (p, _) = t.split_once('.').unwrap();
        assert_eq!(
            String::from_utf8(b64_decode(p).unwrap()).unwrap(),
            format!("dcid1|decombat.main|1001|alice|Alice|{NOW}|{}", NOW + 86400)
        );
        let c = verify(&t, kp.public_key().as_ref(), "decombat.main", NOW + 3600).unwrap();
        assert_eq!(c, claims(NOW));
        // within the skew on both ends
        assert!(verify(&t, kp.public_key().as_ref(), "decombat.main", NOW - 299).is_ok());
        assert!(verify(
            &t,
            kp.public_key().as_ref(),
            "decombat.main",
            NOW + 86400 + 299
        )
        .is_ok());
    }

    #[test]
    fn an_expired_token_is_refused() {
        let kp = pair();
        let t = sign(&kp, &claims(NOW)).unwrap();
        assert_eq!(
            verify(
                &t,
                kp.public_key().as_ref(),
                "decombat.main",
                NOW + 86400 + 301
            ),
            Err(Refusal::Expired)
        );
        assert_eq!(
            verify(&t, kp.public_key().as_ref(), "decombat.main", NOW - 301),
            Err(Refusal::NotYetValid)
        );
    }

    #[test]
    fn another_key_is_a_bad_signature() {
        let (kp, other) = (pair(), pair());
        let t = sign(&kp, &claims(NOW)).unwrap();
        assert_eq!(
            verify(&t, other.public_key().as_ref(), "decombat.main", NOW),
            Err(Refusal::BadSignature)
        );
        let (p, s) = t.split_once('.').unwrap();
        let mut sig = b64_decode(s).unwrap();
        sig[5] ^= 1;
        let t2 = format!("{p}.{}", b64url(&sig));
        assert_eq!(
            verify(&t2, kp.public_key().as_ref(), "decombat.main", NOW),
            Err(Refusal::BadSignature)
        );
    }

    #[test]
    fn a_wrong_issuer_is_refused() {
        let kp = pair();
        let t = sign(&kp, &claims(NOW)).unwrap();
        assert_eq!(
            verify(&t, kp.public_key().as_ref(), "community.x", NOW),
            Err(Refusal::WrongIssuer("decombat.main".into()))
        );
    }

    #[test]
    fn a_tampered_payload_is_refused() {
        let kp = pair();
        let t = sign(&kp, &claims(NOW)).unwrap();
        let (p, s) = t.split_once('.').unwrap();
        let text = String::from_utf8(b64_decode(p).unwrap()).unwrap();
        let forged = text.replace("|1001|", "|1002|");
        let t2 = format!("{}.{s}", b64url(forged.as_bytes()));
        assert_eq!(
            verify(&t2, kp.public_key().as_ref(), "decombat.main", NOW),
            Err(Refusal::BadSignature)
        );
        assert_eq!(
            verify("garbage", kp.public_key().as_ref(), "decombat.main", NOW),
            Err(Refusal::Malformed)
        );
        assert_eq!(
            verify("a.b.c", kp.public_key().as_ref(), "decombat.main", NOW),
            Err(Refusal::Malformed)
        );
        // a signed payload that is not ours is malformed, not trusted
        let odd = "dcid1|decombat.main|x|alice||1|2";
        let t3 = format!(
            "{}.{}",
            b64url(odd.as_bytes()),
            b64url(kp.sign(odd.as_bytes()).as_ref())
        );
        assert_eq!(
            verify(&t3, kp.public_key().as_ref(), "decombat.main", NOW),
            Err(Refusal::Malformed)
        );
    }

    #[test]
    fn fields_with_a_bar_cannot_be_signed() {
        let kp = pair();
        let mut c = claims(NOW);
        c.login = "a|b".into();
        assert!(sign(&kp, &c).is_err());
        let a = Authority::issuing("decombat.main", kp, 0);
        assert_eq!(a.token_hours, 24);
        // a nick with a bar is left out
        let (t, exp) = a.issue(7, "bob", "B|ob", NOW).unwrap();
        assert_eq!(exp, NOW + 86400);
        assert_eq!(a.check(&t, NOW).unwrap().nick, "");
    }

    #[test]
    fn the_key_file_is_made_once_and_read_back() {
        let dir = std::env::temp_dir().join(format!("dc_identity_key_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(KEY_FILE);
        let (a, made) = load_or_create(&path).unwrap();
        assert!(made);
        let (b, made) = load_or_create(&path).unwrap();
        assert!(!made);
        assert_eq!(public_b64(&a), public_b64(&b));
        assert_eq!(public_b64(&a).len(), 44);
        assert_eq!(
            parse_public_key(&public_b64(&a)).unwrap(),
            a.public_key().as_ref()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_reader() {
        let f = json_strings(r#" { "login" : "al\"ice", "n": 5, "password":"päss😀" } "#).unwrap();
        assert_eq!(f[0], ("login".into(), "al\"ice".into()));
        assert_eq!(f[1], ("password".into(), "päss😀".into()));
        assert_eq!(json_strings("{}").unwrap(), vec![]);
        assert!(json_strings("login=a").is_none());
        assert!(json_strings("{\"a\":\"b\"} x").is_none());
    }

    // ------------------------------------------------------------------ lobby: endpoint and token login

    use crate::logs::Listener;
    use crate::state::testing::*;

    fn login_payload(user: &str, pw: &str) -> Vec<u8> {
        let mut w = dc_wire::PktWriter::new(0x200);
        w.wstr(user, 4096)
            .wstr(pw, 17)
            .wstr("00-11-22-33-44-55", 20)
            .i32(0);
        w.into_payload()
    }

    /// (ok, reason, account) of the 0x201 reply to a login of `user`.
    fn login(st: &mut Core, sid: u64, user: &str, pw: &str) -> (u32, u32, u32) {
        let rx = connect(st, sid as _, Listener::Auth);
        let p = login_payload(user, pw);
        st.scope(|st| {
            crate::auth::dispatch(st, sid as _, crate::msg::LOGIN_REQUEST, &p).map(|_| ())
        })
        .unwrap();
        let got = drain(&rx);
        let r = &got
            .iter()
            .find(|(id, _)| *id == 0x201)
            .expect("a login reply")
            .1;
        (u32_at(r, 6), u32_at(r, 0xa), u32_at(r, 0x12))
    }

    fn field(json: &str, k: &str) -> String {
        json_strings(json)
            .unwrap()
            .into_iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v)
            .unwrap_or_default()
    }

    fn issuer_state() -> (Core, Vec<u8>) {
        let mut st = state();
        let kp = pair();
        let public = kp.public_key().as_ref().to_vec();
        st.identity = Authority::issuing("dc.main", kp, 24);
        st.opts.pw_check = true;
        st.scope(|st| {
            crate::auth::ensure_account(st, "alice", "rabbit-hole")?;
            Ok(())
        })
        .unwrap();
        (st, public)
    }

    #[test]
    fn the_endpoint_issues_a_token_for_the_right_password_only() {
        let (mut st, public) = issuer_state();
        let t0 = Instant::now();
        let (code, body) = route(
            &mut st,
            "POST",
            br#"{"login":"alice","password":"wrong"}"#,
            "1.2.3.4:5",
            t0,
        )
        .unwrap();
        assert_eq!((code, body.as_str()), (401, MSG_REFUSED));
        let (code, _) = route(
            &mut st,
            "POST",
            br#"{"login":"nobody","password":"x"}"#,
            "1.2.3.4:5",
            t0,
        )
        .unwrap();
        assert_eq!(code, 401);
        assert_eq!(route(&mut st, "GET", b"", "1.2.3.4:5", t0).unwrap().0, 405);
        assert_eq!(
            route(&mut st, "POST", b"login=alice", "1.2.3.4:5", t0)
                .unwrap()
                .0,
            400
        );
        let (code, body) = route(
            &mut st,
            "POST",
            br#"{"login":"Alice","password":"rabbit-hole"}"#,
            "1.2.3.4:5",
            t0,
        )
        .unwrap();
        assert_eq!(code, 200, "{body}");
        let token = field(&body, "token");
        let unix = crate::clock::unix_now();
        let c = verify(&token, &public, "dc.main", unix).unwrap();
        let acc = db::account_by_login(&st.db, "alice").unwrap().unwrap();
        assert_eq!((c.account, c.login.as_str()), (acc.user_no, "alice"));
        assert!(
            body.contains(&format!("\"account\":{}", acc.user_no)),
            "{body}"
        );
        assert!(
            body.contains(&format!("\"expires\":{}", c.expires)),
            "{body}"
        );
        assert!((c.expires - c.issued) == 24 * 3600);
        // a disabled account gets no token
        crate::accounts::set_disabled(&st.db, "alice", true).unwrap();
        let (code, _) = route(
            &mut st,
            "POST",
            br#"{"login":"alice","password":"rabbit-hole"}"#,
            "1.2.3.4:5",
            t0,
        )
        .unwrap();
        assert_eq!(code, 401);
        // a banned account gets no token
        st.db
            .execute(
                "INSERT INTO op_bans (user_no, until) VALUES (?1, 0)",
                [acc.user_no as i64],
            )
            .unwrap();
        let (code, _) = route(
            &mut st,
            "POST",
            br#"{"login":"alice","password":"rabbit-hole"}"#,
            "1.2.3.4:5",
            t0,
        )
        .unwrap();
        assert_eq!(code, 401);
        st.db.execute("DELETE FROM op_bans", []).unwrap();
        // ten failures lock the name: the right password is refused too (LG-146)
        crate::accounts::set_disabled(&st.db, "alice", false).unwrap();
        for _ in 0..10 {
            route(
                &mut st,
                "POST",
                br#"{"login":"alice","password":"nope"}"#,
                "9.9.9.9:1",
                t0,
            )
            .unwrap();
        }
        let (code, _) = route(
            &mut st,
            "POST",
            br#"{"login":"alice","password":"rabbit-hole"}"#,
            "8.8.8.8:1",
            t0,
        )
        .unwrap();
        assert_eq!(code, 401);
        // a lobby that does not issue answers 404
        let mut other = state();
        assert_eq!(
            route(&mut other, "POST", b"{}", "1.2.3.4:5", t0).unwrap().0,
            404
        );
    }

    #[test]
    fn a_token_login_makes_the_local_account_once_and_finds_it_again() {
        let (mut issuer, public) = issuer_state();
        let alice = db::account_by_login(&issuer.db, "alice")
            .unwrap()
            .unwrap()
            .user_no;
        let (_, body) = route(
            &mut issuer,
            "POST",
            br#"{"login":"alice","password":"rabbit-hole"}"#,
            "1.2.3.4:5",
            Instant::now(),
        )
        .unwrap();
        let token = field(&body, "token");

        let mut host = state();
        host.opts.pw_check = true;
        host.opts.autochar = true;
        host.opts.auto_create = false;
        // a local account first, so the token's account gets another number than on the issuer
        host.scope(|st| {
            crate::auth::ensure_account(st, "carol", "carol-pass")?;
            Ok(())
        })
        .unwrap();
        // not trusted yet: refused
        assert_eq!(
            login(&mut host, 1, &format!("LOCAL:tok:{token}"), "").1,
            crate::wire::REASON_BAD_PASSWORD
        );
        host.identity = Authority::accepting("dc.main", public.clone());
        let (ok, _, no) = login(&mut host, 2, &format!("LOCAL:tok:{token}"), "");
        assert_eq!(ok, 1);
        assert_eq!(
            db::identity_link(&host.db, "dc.main", alice).unwrap(),
            Some(no)
        );
        assert_eq!(
            db::account_by_no(&host.db, no).unwrap().unwrap().login,
            "alice"
        );
        let (ok, _, again) = login(&mut host, 3, &format!("tok:{token}"), "whatever");
        assert_eq!((ok, again), (1, no));
        // LG-271: the mod suffix after the token is stripped before the token is read
        let suffix = dc_wire::loginmods::suffix(dc_wire::loginmods::EMPTY_SET_HASH, "0");
        let (ok, _, again) = login(&mut host, 7, &format!("LOCAL:tok:{token}{suffix}"), "");
        assert_eq!((ok, again), (1, no));
        host.login_mods.set = "ab".repeat(32);
        assert_eq!(
            login(&mut host, 8, &format!("tok:{token}{suffix}"), "").1,
            crate::wire::REASON_BAD_PASSWORD
        );
        let modded = dc_wire::loginmods::suffix(&"ab".repeat(32), "0");
        assert_eq!(
            login(&mut host, 10, &format!("tok:{token}{modded}"), "").0,
            1
        );
        host.login_mods = Default::default();
        let n: i64 = host
            .db
            .query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        // the token account has no password login, the local one keeps its own
        let (ok, reason, _) = login(&mut host, 4, "LOCAL:alice", "rabbit-hole");
        assert_eq!((ok, reason), (0, crate::wire::REASON_BAD_PASSWORD));
        assert_eq!(login(&mut host, 5, "LOCAL:carol", "carol-pass").0, 1);
        // a tampered token is refused
        let bad = format!("tok:{}x", token);
        assert_eq!(login(&mut host, 6, &bad, "").0, 0);

        // a lobby whose own account holds the name does not hand it over
        let mut other = state();
        other.identity = Authority::accepting("dc.main", public);
        other
            .scope(|st| {
                crate::auth::ensure_account(st, "Alice", "local-alice")?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            login(&mut other, 1, &format!("tok:{token}"), "").1,
            crate::wire::REASON_BAD_PASSWORD
        );
        assert_eq!(
            db::identity_link(&other.db, "dc.main", alice).unwrap(),
            None
        );

        // the issuer takes its own token for its own account, without a link row
        issuer.opts.autochar = true;
        let (ok, _, own) = login(&mut issuer, 9, &format!("tok:{token}"), "");
        assert_eq!((ok, own), (1, alice));
        assert!(!db::is_identity_linked(&issuer.db, alice).unwrap());
    }

    #[test]
    fn token_logins_share_the_name_lockout() {
        let (mut issuer, public) = issuer_state();
        let (_, body) = route(
            &mut issuer,
            "POST",
            br#"{"login":"alice","password":"rabbit-hole"}"#,
            "1.2.3.4:5",
            Instant::now(),
        )
        .unwrap();
        let token = field(&body, "token");
        // a lobby whose own account holds the name: every token login is refused and counts against the name
        let mut other = state();
        other.identity = Authority::accepting("dc.main", public.clone());
        other
            .scope(|st| {
                crate::auth::ensure_account(st, "Alice", "local-alice")?;
                Ok(())
            })
            .unwrap();
        for i in 0..10 {
            assert_eq!(
                login(&mut other, 1 + i, &format!("tok:{token}"), "").1,
                crate::wire::REASON_BAD_PASSWORD
            );
        }
        assert!(other
            .limits
            .login
            .locked(&["name:alice".to_string()], Instant::now())
            .is_some());
        // a locked name refuses a good token too
        let mut host = state();
        host.opts.autochar = true;
        host.identity = Authority::accepting("dc.main", public);
        assert_eq!(login(&mut host, 1, &format!("tok:{token}"), "").0, 1);
        let now = Instant::now();
        let keys = ["name:alice".to_string()];
        for _ in 0..10 {
            host.limits.login.fail(&keys, now);
        }
        assert_eq!(
            login(&mut host, 2, &format!("tok:{token}"), "").1,
            crate::wire::REASON_BAD_PASSWORD
        );
    }

    #[test]
    fn unknown_disabled_and_wrong_password_attempts_are_all_counted() {
        for (login, disable) in [("ghost", false), ("alice", true), ("alice", false)] {
            let (mut st, _) = issuer_state();
            let t0 = Instant::now();
            if disable {
                crate::accounts::set_disabled(&st.db, "alice", true).unwrap();
            }
            let pw = if disable { "rabbit-hole" } else { "wrong" };
            let body = format!("{{\"login\":\"{login}\",\"password\":\"{pw}\"}}");
            let keys = crate::limits::login_keys(login, "5.5.5.5:1");
            for _ in 0..10 {
                let (code, b) = route(&mut st, "POST", body.as_bytes(), "5.5.5.5:1", t0).unwrap();
                assert_eq!((code, b.as_str()), (401, MSG_REFUSED));
            }
            assert!(
                st.limits.login.locked(&keys, t0).is_some(),
                "{login} disabled={disable}"
            );
        }
    }

    #[test]
    fn token_times_are_bounded_and_do_not_overflow() {
        let kp = pair();
        let public = kp.public_key().as_ref().to_vec();
        let mut c = claims(1_000);
        c.issued = i64::MAX - 1;
        c.expires = i64::MAX;
        let t = sign(&kp, &c).unwrap();
        assert_eq!(
            verify(&t, &public, "decombat.main", i64::MAX).unwrap_err(),
            Refusal::Malformed
        );
        let mut c = claims(1_000);
        c.issued = 200_000_000_000;
        c.expires = 200_000_000_100;
        let t = sign(&kp, &c).unwrap();
        assert_eq!(
            verify(&t, &public, "decombat.main", 1_000).unwrap_err(),
            Refusal::Malformed
        );
    }

    // ------------------------------------------------------------------ LG-273: the nick follows the identity

    /// A lobby that trusts `dc.main` (no autochar: a login without a character would see the nickname screen).
    fn trusting() -> (Core, Ed25519KeyPair) {
        let kp = pair();
        let mut st = state();
        st.opts.autochar = false;
        st.opts.auto_create = false;
        st.identity = Authority::accepting("dc.main", kp.public_key().as_ref().to_vec());
        (st, kp)
    }

    fn token(kp: &Ed25519KeyPair, account: u32, login: &str, nick: &str) -> String {
        let now = crate::clock::unix_now();
        let c = Claims {
            issuer: "dc.main".into(),
            account,
            login: login.into(),
            nick: nick.into(),
            issued: now,
            expires: now + 3600,
        };
        format!("LOCAL:tok:{}", sign(kp, &c).unwrap())
    }

    fn nick_of(st: &Core, account: u32) -> Option<String> {
        st.character(account).map(|c| c.nick)
    }

    #[test]
    fn a_first_token_login_creates_the_character_with_the_tokens_nick() {
        let (mut st, kp) = trusting();
        let t = token(&kp, 77, "quinnan224", "Quinn");
        // ok = 1: the client sees a character, no nickname screen (reason 0x0003001e)
        let (ok, reason, no) = login(&mut st, 1, &t, "");
        assert_eq!((ok, reason), (1, 0));
        assert_eq!(nick_of(&st, no).as_deref(), Some("Quinn"));
        // the creation path of the nickname screen: the starter kit is there
        let items: i64 = st
            .db
            .query_row(
                "SELECT COUNT(*) FROM items WHERE user_no = ?1",
                [no as i64],
                |r| r.get(0),
            )
            .unwrap();
        assert!(items > 0, "starter kit granted");
        // a second login finds the same account and character
        let (ok, _, again) = login(&mut st, 2, &t, "");
        assert_eq!((ok, again), (1, no));
        assert_eq!(nick_of(&st, no).as_deref(), Some("Quinn"));
        let n: i64 = st
            .db
            .query_row("SELECT COUNT(*) FROM characters", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn a_nick_held_here_gets_the_first_free_number() {
        let (mut st, kp) = trusting();
        st.scope(|st| {
            let carol = crate::auth::ensure_account(st, "carol", "carol-pass")?;
            crate::auth::create_character(st, carol, "Quinn", 0)?;
            let dave = crate::auth::ensure_account(st, "dave", "dave-pass")?;
            crate::auth::create_character(st, dave, "quinn2", 0)?;
            Ok(())
        })
        .unwrap();
        let (ok, _, no) = login(&mut st, 1, &token(&kp, 77, "quinnan224", "Quinn"), "");
        assert_eq!(ok, 1);
        assert_eq!(
            nick_of(&st, no).as_deref(),
            Some("Quinn3"),
            "Quinn and quinn2 (any case) are held"
        );
        // a 16-unit nick keeps 15 units and the number
        let (ok, _, other) = login(
            &mut st,
            2,
            &token(&kp, 78, "longone", "Sixteen_Letters!"),
            "",
        );
        assert_eq!(ok, 1);
        assert_eq!(nick_of(&st, other).as_deref(), Some("Sixteen_Letters!"));
        let (ok, _, third) = login(
            &mut st,
            3,
            &token(&kp, 79, "copycat", "Sixteen_Letters!"),
            "",
        );
        assert_eq!(ok, 1);
        assert_eq!(nick_of(&st, third).as_deref(), Some("Sixteen_Letters2"));
    }

    /// The user's report: the identity's vanilla nick equals its login name (characters the client made by itself
    /// before the nickname screen took the ID as the name, CLI-162). The 2009 nickname screen refuses a name equal
    /// to the ID on the client, so the player could not take the own name on a new lobby. The lobby now makes the
    /// character itself, and a first join refused for its mod set left nothing behind.
    #[test]
    fn a_nick_equal_to_the_login_is_kept_and_a_refused_first_join_leaves_nothing() {
        let (mut st, kp) = trusting();
        st.login_mods.set = "ab".repeat(32);
        let t = token(&kp, 77, "quinnan224", "quinnan224");
        // the first join without the server's mod set: refused before any account is made
        assert_eq!(
            login(&mut st, 1, &t, "").1,
            crate::wire::REASON_BAD_PASSWORD
        );
        let n: i64 = st
            .db
            .query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        let modded = dc_wire::loginmods::suffix(&"ab".repeat(32), "0");
        let (ok, reason, no) = login(&mut st, 2, &format!("{t}{modded}"), "");
        assert_eq!((ok, reason), (1, 0));
        assert_eq!(nick_of(&st, no).as_deref(), Some("quinnan224"));
    }

    #[test]
    fn local_accounts_and_tokens_without_a_nick_still_get_the_nickname_screen() {
        let (mut st, kp) = trusting();
        st.opts.auto_create = true;
        let (ok, reason, _) = login(&mut st, 1, "LOCAL:erika", "erika-pass");
        assert_eq!((ok, reason), (0, crate::wire::REASON_NO_CHARACTER));
        let (ok, reason, no) = login(&mut st, 2, &token(&kp, 80, "nonick", ""), "");
        assert_eq!((ok, reason), (0, crate::wire::REASON_NO_CHARACTER));
        assert_eq!(nick_of(&st, no), None);
        // a GM-prefixed nick on a non-GM account here: the player picks a name
        let (ok, reason, _) = login(&mut st, 3, &token(&kp, 81, "boss", "GMBoss"), "");
        assert_eq!((ok, reason), (0, crate::wire::REASON_NO_CHARACTER));
    }
}
