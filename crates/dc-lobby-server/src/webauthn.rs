//! The server side of WebAuthn for the admin web sign-in (adminweb.rs): a small CBOR reader, COSE keys, the
//! authenticator data, `packed` attestation whose x5c chain must end in a trusted root (Yubico's FIDO roots in
//! production, a test root in tests), and assertions.
//!
//! Public references only: W3C Web Authentication Level 2 (sections 6.1 authenticator data, 7.1 registration, 7.2
//! authentication, 8.2 packed attestation), RFC 8949 (CBOR), RFC 9052 / 9053 (COSE keys: EC2 P-256 with ES256 = -7,
//! OKP Ed25519 with EdDSA = -8), RFC 5280 (certificate paths). Signatures are checked with `ring`; certificates are read
//! with `x509-parser`.
// Provenance: ORIGINAL (DeCombat server code from the public specifications named above)
// DC-DEFAULT: LG-383 hardware security keys only: packed attestation chained to Yubico's published FIDO roots

use ring::signature::{self, UnparsedPublicKey, VerificationAlgorithm};
use serde_json::Value;
use x509_parser::prelude::{FromDer, X509Certificate, X509Version};

/// Yubico's FIDO attestation roots, published at <https://developers.yubico.com/PKI/> (yubico-fido-ca-1.pem,
/// yubico-fido-ca-2.pem, yubico-ca-1.pem; fetched 2026-11-03). SHA-256 fingerprints:
/// - Yubico U2F Root CA Serial 457200631:
///   `0F:A1:38:6F:80:EB:87:13:26:3A:E5:C1:D8:4D:EB:45:5B:DF:08:AE:A5:0A:B0:55:03:CE:FE:E8:2B:09:2D:42`
/// - Yubico FIDO Root CA Serial 450203556:
///   `35:F1:A5:4B:35:3B:FB:71:1E:6D:42:AD:BE:B7:6C:0E:9D:EA:D0:95:01:8E:6A:94:78:3B:A2:19:2F:D6:FA:AD`
/// - Yubico Attestation Root 1:
///   `62:76:0C:6A:6E:F9:16:79:F4:54:C8:90:2B:80:FD:00:98:25:B3:F2:5D:A9:0F:1F:BA:CE:2E:C6:58:6C:D5:A8`
const YUBICO_ROOTS: &str = include_str!("yubico_fido_roots.pem");
/// The FIDO intermediates of <https://developers.yubico.com/PKI/yubico-intermediate.pem> (untrusted: a key's x5c may
/// leave them out, and each must still chain to a root above).
const YUBICO_INTERMEDIATES: &str = include_str!("yubico_fido_intermediates.pem");

/// The FIDO AAGUID certificate extension (id-fido-gen-ce-aaguid).
const OID_FIDO_AAGUID: &str = "1.3.6.1.4.1.45724.1.1.4";
/// Critical extensions this code understands.
const KNOWN_CRITICAL: &[&str] = &["2.5.29.19", "2.5.29.15", "2.5.29.37"];
/// Longest certificate path above the leaf.
const MAX_CHAIN: usize = 5;

// ------------------------------------------------------------------------------------------------- base64

/// Base64 (RFC 4648): the URL alphabet without padding (WebAuthn's JSON) and the standard one (PEM, CSP hashes).
pub mod b64 {
    const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    fn enc(data: &[u8], abc: &[u8; 64], pad: bool) -> String {
        let mut o = String::with_capacity(data.len().div_ceil(3) * 4);
        for c in data.chunks(3) {
            let n = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            let k = c.len() + 1;
            for i in 0..4 {
                if i < k {
                    o.push(abc[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else if pad {
                    o.push('=');
                }
            }
        }
        o
    }

    /// URL alphabet, no padding.
    pub fn url(data: &[u8]) -> String {
        enc(data, URL, false)
    }

    /// Standard alphabet, padded.
    pub fn std(data: &[u8]) -> String {
        enc(data, STD, true)
    }

    /// Decodes either alphabet, with or without padding; whitespace is skipped. `None` for any other character or a
    /// length that cannot be base64.
    pub fn decode(s: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(s.len() * 3 / 4);
        let mut acc = 0u32;
        let mut bits = 0;
        let mut ended = false;
        for ch in s.bytes() {
            let v = match ch {
                b'A'..=b'Z' => ch - b'A',
                b'a'..=b'z' => ch - b'a' + 26,
                b'0'..=b'9' => ch - b'0' + 52,
                b'+' | b'-' => 62,
                b'/' | b'_' => 63,
                b'=' => {
                    ended = true;
                    continue;
                }
                b' ' | b'\t' | b'\r' | b'\n' => continue,
                _ => return None,
            };
            if ended {
                return None;
            }
            acc = (acc << 6) | u32::from(v);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        // 6 left-over bits cannot come from whole bytes; the others must be zero padding bits
        if bits >= 6 || acc != 0 {
            return None;
        }
        Some(out)
    }
}

/// The DER certificates of a PEM text (other lines are skipped).
pub fn pem_certs(text: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in text.lines() {
        let l = line.trim();
        if l == "-----BEGIN CERTIFICATE-----" {
            cur = Some(String::new());
        } else if l == "-----END CERTIFICATE-----" {
            if let Some(b) = cur.take().and_then(|s| b64::decode(&s)) {
                out.push(b);
            }
        } else if let Some(s) = cur.as_mut() {
            s.push_str(l);
        }
    }
    out
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let d = ring::digest::digest(&ring::digest::SHA256, data);
    let mut o = [0u8; 32];
    o.copy_from_slice(d.as_ref());
    o
}

// ------------------------------------------------------------------------------------------------- CBOR

/// One CBOR data item of the kinds WebAuthn uses (RFC 8949 major types 0-5 and the simple values false, true, null,
/// undefined). Tags, floats and indefinite lengths are refused.
#[derive(Debug, Clone, PartialEq)]
pub enum Cbor {
    Int(i128),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Cbor>),
    Map(Vec<(Cbor, Cbor)>),
    Bool(bool),
    Null,
    Undefined,
}

impl Cbor {
    /// The value of map key `k`.
    pub fn get(&self, k: &Cbor) -> Option<&Cbor> {
        match self {
            Cbor::Map(m) => m.iter().find(|(key, _)| key == k).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn get_int(&self, k: i128) -> Option<&Cbor> {
        self.get(&Cbor::Int(k))
    }
    pub fn get_text(&self, k: &str) -> Option<&Cbor> {
        self.get(&Cbor::Text(k.to_string()))
    }
    pub fn int(&self) -> Option<i128> {
        match self {
            Cbor::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Cbor::Bytes(b) => Some(b),
            _ => None,
        }
    }
    pub fn text(&self) -> Option<&str> {
        match self {
            Cbor::Text(t) => Some(t),
            _ => None,
        }
    }
}

const CBOR_MAX_DEPTH: usize = 16;
/// Most entries of one array or map (WebAuthn uses a handful; the duplicate-key check is quadratic).
const CBOR_MAX_ITEMS: u64 = 64;

/// Decodes the first CBOR item of `b`; the item and the bytes it took.
pub fn cbor_decode(b: &[u8]) -> Result<(Cbor, usize), String> {
    let mut pos = 0;
    let v = cbor_item(b, &mut pos, 0)?;
    Ok((v, pos))
}

/// Decodes `b`, which must hold exactly one item.
pub fn cbor_decode_all(b: &[u8]) -> Result<Cbor, String> {
    let (v, n) = cbor_decode(b)?;
    if n != b.len() {
        return Err("CBOR: bytes after the item".into());
    }
    Ok(v)
}

fn cbor_take<'a>(b: &'a [u8], pos: &mut usize, n: u64) -> Result<&'a [u8], String> {
    let n = usize::try_from(n).map_err(|_| "CBOR: length too large".to_string())?;
    let end = pos
        .checked_add(n)
        .filter(|e| *e <= b.len())
        .ok_or("CBOR: truncated")?;
    let s = &b[*pos..end];
    *pos = end;
    Ok(s)
}

fn cbor_item(b: &[u8], pos: &mut usize, depth: usize) -> Result<Cbor, String> {
    if depth > CBOR_MAX_DEPTH {
        return Err("CBOR: nested too deep".into());
    }
    let ib = *b.get(*pos).ok_or("CBOR: truncated")?;
    *pos += 1;
    let (major, info) = (ib >> 5, ib & 0x1f);
    let arg = match info {
        0..=23 => u64::from(info),
        24..=27 => {
            let n = 1usize << (info - 24);
            let s = cbor_take(b, pos, n as u64)?;
            s.iter().fold(0u64, |a, x| (a << 8) | u64::from(*x))
        }
        31 => return Err("CBOR: indefinite lengths are not accepted".into()),
        _ => return Err("CBOR: reserved length".into()),
    };
    let left = (b.len() - *pos) as u64;
    Ok(match major {
        0 => Cbor::Int(i128::from(arg)),
        1 => Cbor::Int(-1 - i128::from(arg)),
        2 => Cbor::Bytes(cbor_take(b, pos, arg)?.to_vec()),
        3 => Cbor::Text(
            String::from_utf8(cbor_take(b, pos, arg)?.to_vec())
                .map_err(|_| "CBOR: text is not UTF-8".to_string())?,
        ),
        4 => {
            if arg > left || arg > CBOR_MAX_ITEMS {
                return Err("CBOR: too many items or truncated".into());
            }
            let mut v = Vec::with_capacity(arg as usize);
            for _ in 0..arg {
                v.push(cbor_item(b, pos, depth + 1)?);
            }
            Cbor::Array(v)
        }
        5 => {
            if arg.saturating_mul(2) > left || arg > CBOR_MAX_ITEMS {
                return Err("CBOR: too many items or truncated".into());
            }
            let mut m: Vec<(Cbor, Cbor)> = Vec::with_capacity(arg as usize);
            for _ in 0..arg {
                let k = cbor_item(b, pos, depth + 1)?;
                let v = cbor_item(b, pos, depth + 1)?;
                if m.iter().any(|(old, _)| *old == k) {
                    return Err("CBOR: a map key appears twice".into());
                }
                m.push((k, v));
            }
            Cbor::Map(m)
        }
        6 => return Err("CBOR: tags are not accepted".into()),
        _ => match info {
            20 => Cbor::Bool(false),
            21 => Cbor::Bool(true),
            22 => Cbor::Null,
            23 => Cbor::Undefined,
            _ => return Err("CBOR: floats and other simple values are not accepted".into()),
        },
    })
}

/// Encodes an item (tests build authenticator answers with it).
#[cfg(test)]
pub fn cbor_encode(v: &Cbor) -> Vec<u8> {
    fn head(o: &mut Vec<u8>, major: u8, n: u64) {
        let m = major << 5;
        if n < 24 {
            o.push(m | n as u8);
        } else if n <= 0xff {
            o.extend_from_slice(&[m | 24, n as u8]);
        } else if n <= 0xffff {
            o.push(m | 25);
            o.extend_from_slice(&(n as u16).to_be_bytes());
        } else if n <= 0xffff_ffff {
            o.push(m | 26);
            o.extend_from_slice(&(n as u32).to_be_bytes());
        } else {
            o.push(m | 27);
            o.extend_from_slice(&n.to_be_bytes());
        }
    }
    fn go(o: &mut Vec<u8>, v: &Cbor) {
        match v {
            Cbor::Int(i) if *i >= 0 => head(o, 0, *i as u64),
            Cbor::Int(i) => head(o, 1, (-1 - *i) as u64),
            Cbor::Bytes(b) => {
                head(o, 2, b.len() as u64);
                o.extend_from_slice(b);
            }
            Cbor::Text(t) => {
                head(o, 3, t.len() as u64);
                o.extend_from_slice(t.as_bytes());
            }
            Cbor::Array(a) => {
                head(o, 4, a.len() as u64);
                for x in a {
                    go(o, x);
                }
            }
            Cbor::Map(m) => {
                head(o, 5, m.len() as u64);
                for (k, x) in m {
                    go(o, k);
                    go(o, x);
                }
            }
            Cbor::Bool(false) => o.push(0xf4),
            Cbor::Bool(true) => o.push(0xf5),
            Cbor::Null => o.push(0xf6),
            Cbor::Undefined => o.push(0xf7),
        }
    }
    let mut o = Vec::new();
    go(&mut o, v);
    o
}

// ------------------------------------------------------------------------------------------------- COSE keys

/// COSE algorithm ids this server accepts for a credential.
pub const ALG_ES256: i64 = -7;
pub const ALG_EDDSA: i64 = -8;

/// A credential public key.
#[derive(Debug, Clone, PartialEq)]
pub enum CoseKey {
    /// P-256, uncompressed point `04 || x || y`.
    Es256(Vec<u8>),
    /// Ed25519, the 32-byte key.
    EdDsa(Vec<u8>),
}

impl CoseKey {
    pub fn alg(&self) -> i64 {
        match self {
            CoseKey::Es256(_) => ALG_ES256,
            CoseKey::EdDsa(_) => ALG_EDDSA,
        }
    }

    /// True when `sig` is this key's signature of `msg` (ES256: ASN.1 DER, as WebAuthn sends it).
    pub fn verify(&self, msg: &[u8], sig: &[u8]) -> bool {
        match self {
            CoseKey::Es256(p) => UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_ASN1, p)
                .verify(msg, sig)
                .is_ok(),
            CoseKey::EdDsa(k) => UnparsedPublicKey::new(&signature::ED25519, k)
                .verify(msg, sig)
                .is_ok(),
        }
    }
}

/// Reads a COSE_Key map (EC2 / P-256 / ES256 or OKP / Ed25519 / EdDSA).
pub fn parse_cose(c: &Cbor) -> Result<CoseKey, String> {
    let kty = c.get_int(1).and_then(Cbor::int);
    let alg = c.get_int(3).and_then(Cbor::int);
    let crv = c.get_int(-1).and_then(Cbor::int);
    let x = c.get_int(-2).and_then(Cbor::bytes);
    match (kty, alg, crv) {
        (Some(2), Some(-7), Some(1)) => {
            let y = c.get_int(-3).and_then(Cbor::bytes);
            match (x, y) {
                (Some(x), Some(y)) if x.len() == 32 && y.len() == 32 => {
                    let mut p = vec![4u8];
                    p.extend_from_slice(x);
                    p.extend_from_slice(y);
                    Ok(CoseKey::Es256(p))
                }
                _ => Err("the key's P-256 point is malformed".into()),
            }
        }
        (Some(1), Some(-8), Some(6)) => match x {
            Some(x) if x.len() == 32 => Ok(CoseKey::EdDsa(x.to_vec())),
            _ => Err("the key's Ed25519 value is malformed".into()),
        },
        _ => Err(
            "the key uses an algorithm this server does not accept (ES256 or EdDSA only)".into(),
        ),
    }
}

/// [`parse_cose`] of encoded bytes.
pub fn parse_cose_bytes(b: &[u8]) -> Result<CoseKey, String> {
    parse_cose(&cbor_decode_all(b)?)
}

// ------------------------------------------------------------------------------------------------- authenticator data

pub const FLAG_UP: u8 = 0x01;
pub const FLAG_UV: u8 = 0x04;
pub const FLAG_BE: u8 = 0x08;
pub const FLAG_AT: u8 = 0x40;
pub const FLAG_ED: u8 = 0x80;

/// The attested credential data of a registration.
#[derive(Debug, Clone)]
pub struct Attested {
    pub aaguid: [u8; 16],
    pub cred_id: Vec<u8>,
    /// The COSE_Key as the authenticator encoded it.
    pub key_bytes: Vec<u8>,
    pub key: CoseKey,
}

#[derive(Debug, Clone)]
pub struct AuthData {
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub counter: u32,
    pub attested: Option<Attested>,
}

/// Reads authenticator data; every byte must belong to a part the flags announce.
pub fn parse_auth_data(b: &[u8]) -> Result<AuthData, String> {
    if b.len() < 37 {
        return Err("authenticator data too short".into());
    }
    let mut rp_id_hash = [0u8; 32];
    rp_id_hash.copy_from_slice(&b[..32]);
    let flags = b[32];
    let counter = u32::from_be_bytes([b[33], b[34], b[35], b[36]]);
    let mut pos = 37;
    let mut attested = None;
    if flags & FLAG_AT != 0 {
        if b.len() < pos + 18 {
            return Err("attested credential data too short".into());
        }
        let mut aaguid = [0u8; 16];
        aaguid.copy_from_slice(&b[pos..pos + 16]);
        let n = usize::from(u16::from_be_bytes([b[pos + 16], b[pos + 17]]));
        pos += 18;
        if n == 0 || n > 1023 || b.len() < pos + n {
            return Err("credential id length is wrong".into());
        }
        let cred_id = b[pos..pos + n].to_vec();
        pos += n;
        let (kc, used) = cbor_decode(&b[pos..])?;
        let key_bytes = b[pos..pos + used].to_vec();
        pos += used;
        let key = parse_cose(&kc)?;
        attested = Some(Attested {
            aaguid,
            cred_id,
            key_bytes,
            key,
        });
    }
    if flags & FLAG_ED != 0 {
        let (ext, used) = cbor_decode(&b[pos..])?;
        if !matches!(ext, Cbor::Map(_)) {
            return Err("authenticator extensions are not a map".into());
        }
        pos += used;
    }
    if pos != b.len() {
        return Err("authenticator data has bytes after its parts".into());
    }
    Ok(AuthData {
        rp_id_hash,
        flags,
        counter,
        attested,
    })
}

// ------------------------------------------------------------------------------------------------- checks

/// What a ceremony must match.
#[derive(Debug, Clone, Copy)]
pub struct Expect<'a> {
    pub rp_id: &'a str,
    pub origin: &'a str,
    pub challenge: &'a [u8],
}

/// Checks `clientDataJSON`: type, challenge, origin, not cross-origin.
fn check_client_data(cdj: &[u8], kind: &str, e: &Expect) -> Result<(), String> {
    let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(cdj) else {
        return Err("client data is not a JSON object".into());
    };
    if m.get("type").and_then(Value::as_str) != Some(kind) {
        return Err(format!("client data type is not {kind}"));
    }
    let ch = m
        .get("challenge")
        .and_then(Value::as_str)
        .and_then(b64::decode);
    if ch.as_deref() != Some(e.challenge) {
        return Err("the challenge does not match (expired or used: start again)".into());
    }
    if m.get("origin").and_then(Value::as_str) != Some(e.origin) {
        return Err("the page origin does not match".into());
    }
    if m.get("crossOrigin").and_then(Value::as_bool) == Some(true) {
        return Err("a cross-origin request is refused".into());
    }
    Ok(())
}

fn check_flags(ad: &AuthData, e: &Expect) -> Result<(), String> {
    if ad.rp_id_hash != sha256(e.rp_id.as_bytes()) {
        return Err("the relying-party id does not match".into());
    }
    if ad.flags & FLAG_UP == 0 {
        return Err("the key reported no touch (user presence)".into());
    }
    if ad.flags & FLAG_UV == 0 {
        return Err("the key did not verify its PIN (user verification is required)".into());
    }
    if ad.flags & FLAG_BE != 0 {
        return Err("this is a synced passkey, not a hardware security key".into());
    }
    Ok(())
}

/// Roots and intermediates for attestation chains.
#[derive(Debug, Clone)]
pub struct Trust {
    pub roots: Vec<Vec<u8>>,
    pub intermediates: Vec<Vec<u8>>,
}

impl Trust {
    /// Yubico's published FIDO roots and intermediates (embedded).
    pub fn yubico() -> Trust {
        Trust {
            roots: pem_certs(YUBICO_ROOTS),
            intermediates: pem_certs(YUBICO_INTERMEDIATES),
        }
    }
}

/// A verified registration.
#[derive(Debug, Clone)]
pub struct NewCredential {
    pub cred_id: Vec<u8>,
    pub public_key: Vec<u8>,
    pub alg: i64,
    pub counter: u32,
    pub aaguid: [u8; 16],
    /// The common name of the root the attestation chained to.
    pub root: String,
}

fn parse_cert(der: &[u8]) -> Result<X509Certificate<'_>, String> {
    match X509Certificate::from_der(der) {
        Ok(([], c)) => Ok(c),
        Ok(_) => Err("certificate has bytes after it".into()),
        Err(e) => Err(format!("certificate: {e}")),
    }
}

fn cn(c: &X509Certificate) -> String {
    c.subject()
        .iter_common_name()
        .next()
        .and_then(|a| a.as_str().ok())
        .unwrap_or("?")
        .to_string()
}

fn valid_at(c: &X509Certificate, now: i64) -> bool {
    let v = c.validity();
    v.not_before.timestamp() <= now && now <= v.not_after.timestamp()
}

/// True when `issuer`'s key made `child`'s signature.
fn signed_by(child: &X509Certificate, issuer: &X509Certificate) -> bool {
    let spki = issuer.public_key();
    let key_alg = spki.algorithm.algorithm.to_id_string();
    let curve = spki
        .algorithm
        .parameters
        .as_ref()
        .and_then(|p| p.as_oid().ok())
        .map(|o| o.to_id_string());
    let sig_alg = child.signature_algorithm.algorithm.to_id_string();
    const RSA: &str = "1.2.840.113549.1.1.1";
    const EC: &str = "1.2.840.10045.2.1";
    const P256: &str = "1.2.840.10045.3.1.7";
    const P384: &str = "1.3.132.0.34";
    let va: &'static dyn VerificationAlgorithm =
        match (sig_alg.as_str(), key_alg.as_str(), curve.as_deref()) {
            ("1.2.840.113549.1.1.11", RSA, _) => &signature::RSA_PKCS1_2048_8192_SHA256,
            ("1.2.840.113549.1.1.12", RSA, _) => &signature::RSA_PKCS1_2048_8192_SHA384,
            ("1.2.840.113549.1.1.13", RSA, _) => &signature::RSA_PKCS1_2048_8192_SHA512,
            ("1.2.840.10045.4.3.2", EC, Some(P256)) => &signature::ECDSA_P256_SHA256_ASN1,
            ("1.2.840.10045.4.3.2", EC, Some(P384)) => &signature::ECDSA_P384_SHA256_ASN1,
            ("1.2.840.10045.4.3.3", EC, Some(P256)) => &signature::ECDSA_P256_SHA384_ASN1,
            ("1.2.840.10045.4.3.3", EC, Some(P384)) => &signature::ECDSA_P384_SHA384_ASN1,
            ("1.3.101.112", "1.3.101.112", _) => &signature::ED25519,
            _ => return false,
        };
    UnparsedPublicKey::new(va, &spki.subject_public_key.data)
        .verify(child.tbs_certificate.as_ref(), &child.signature_value.data)
        .is_ok()
}

fn unknown_critical(c: &X509Certificate) -> bool {
    c.extensions()
        .iter()
        .any(|x| x.critical && !KNOWN_CRITICAL.contains(&x.oid.to_id_string().as_str()))
}

/// May `c` sign certificates with `below` CA certificates between it and the leaf?
fn can_issue(c: &X509Certificate, below: usize) -> bool {
    let bc = match c.basic_constraints() {
        Ok(Some(bc)) => bc.value.clone(),
        _ => return false,
    };
    if !bc.ca || bc.path_len_constraint.is_some_and(|n| (n as usize) < below) {
        return false;
    }
    match c.key_usage() {
        Ok(Some(ku)) => ku.value.key_cert_sign(),
        Ok(None) => true,
        Err(_) => false,
    }
}

/// Builds a path from `leaf` through `pool` (the key's own intermediates, then the trusted ones) to a root of
/// `trust`; the root's common name.
pub fn chain_to_root(
    leaf: &X509Certificate,
    pool: &[Vec<u8>],
    trust: &Trust,
    now: i64,
) -> Result<String, String> {
    let roots: Vec<X509Certificate> = trust
        .roots
        .iter()
        .filter_map(|d| parse_cert(d).ok())
        .collect();
    let mut cands: Vec<X509Certificate> = Vec::new();
    for d in pool.iter().chain(trust.intermediates.iter()) {
        cands.push(parse_cert(d)?);
    }
    let mut cur = leaf;
    for below in 0..=MAX_CHAIN {
        if let Some(r) = roots
            .iter()
            .find(|r| r.subject().as_raw() == cur.issuer().as_raw() && signed_by(cur, r))
        {
            if !valid_at(r, now) {
                return Err(format!("the root {} is outside its validity", cn(r)));
            }
            if !can_issue(r, below) {
                return Err(format!("the root {} may not issue this path", cn(r)));
            }
            return Ok(cn(r));
        }
        let Some(next) = cands
            .iter()
            .find(|i| i.subject().as_raw() == cur.issuer().as_raw() && signed_by(cur, i))
        else {
            return Err(
                "the attestation does not chain to a Yubico root: only YubiKeys are accepted"
                    .into(),
            );
        };
        if !valid_at(next, now) {
            return Err(format!(
                "the intermediate {} is outside its validity",
                cn(next)
            ));
        }
        if !can_issue(next, below) || unknown_critical(next) {
            return Err(format!(
                "the intermediate {} may not issue this path",
                cn(next)
            ));
        }
        cur = next;
    }
    Err("the attestation chain is too long".into())
}

/// Checks the leaf of a packed attestation (W3C WebAuthn 8.2.1) and the signature it made.
fn check_packed_leaf(
    leaf: &X509Certificate,
    aaguid: &[u8; 16],
    signed: &[u8],
    sig: &[u8],
    now: i64,
) -> Result<(), String> {
    if leaf.version() != X509Version::V3 {
        return Err("the attestation certificate is not version 3".into());
    }
    let ou = leaf
        .subject()
        .iter_organizational_unit()
        .any(|a| a.as_str().ok() == Some("Authenticator Attestation"));
    if !ou {
        return Err(
            "the attestation certificate is not an authenticator attestation certificate".into(),
        );
    }
    match leaf.basic_constraints() {
        Ok(Some(bc)) if bc.value.ca => return Err("the attestation certificate is a CA".into()),
        Ok(_) => {}
        Err(_) => {
            return Err("the attestation certificate's basic constraints are malformed".into())
        }
    }
    if unknown_critical(leaf) {
        return Err("the attestation certificate has an unknown critical extension".into());
    }
    if let Some(x) = leaf
        .extensions()
        .iter()
        .find(|x| x.oid.to_id_string() == OID_FIDO_AAGUID)
    {
        // extnValue holds an OCTET STRING of the 16 AAGUID bytes
        let mut want = vec![0x04, 0x10];
        want.extend_from_slice(aaguid);
        if x.critical || x.value != want.as_slice() {
            return Err(
                "the attestation certificate names another authenticator model (AAGUID)".into(),
            );
        }
    }
    if !valid_at(leaf, now) {
        return Err("the attestation certificate is outside its validity".into());
    }
    let spki = leaf.public_key();
    let curve = spki
        .algorithm
        .parameters
        .as_ref()
        .and_then(|p| p.as_oid().ok())
        .map(|o| o.to_id_string());
    if spki.algorithm.algorithm.to_id_string() != "1.2.840.10045.2.1"
        || curve.as_deref() != Some("1.2.840.10045.3.1.7")
    {
        return Err("the attestation key is not P-256".into());
    }
    if UnparsedPublicKey::new(
        &signature::ECDSA_P256_SHA256_ASN1,
        &spki.subject_public_key.data,
    )
    .verify(signed, sig)
    .is_err()
    {
        return Err("the attestation signature is wrong".into());
    }
    Ok(())
}

/// Verifies a registration (`navigator.credentials.create` answer): client data, authenticator data, and a `packed`
/// attestation with an x5c chain to a root of `trust`. Every other attestation is refused with the reason.
pub fn verify_registration(
    client_data_json: &[u8],
    attestation_object: &[u8],
    e: &Expect,
    trust: &Trust,
    now: i64,
) -> Result<NewCredential, String> {
    check_client_data(client_data_json, "webauthn.create", e)?;
    let ao = cbor_decode_all(attestation_object)?;
    let fmt = ao
        .get_text("fmt")
        .and_then(Cbor::text)
        .ok_or("attestation has no format")?;
    let auth_data = ao
        .get_text("authData")
        .and_then(Cbor::bytes)
        .ok_or("attestation has no authenticator data")?;
    let stmt = ao
        .get_text("attStmt")
        .ok_or("attestation has no statement")?;
    let ad = parse_auth_data(auth_data)?;
    check_flags(&ad, e)?;
    let att = ad
        .attested
        .as_ref()
        .ok_or("no credential in the authenticator data")?;
    match fmt {
        "packed" => {}
        "none" => {
            return Err("the browser sent no attestation: allow this site to see your security key's make and model \
                        when the browser asks, and use a YubiKey"
                .into())
        }
        "fido-u2f" => {
            return Err("this is a U2F-only key without a PIN: use a FIDO2 YubiKey (YubiKey 5 or Security Key series)".into())
        }
        "tpm" | "android-key" | "android-safetynet" | "apple" => {
            return Err("this is a computer's or phone's built-in passkey: only a hardware security key (YubiKey) is accepted".into())
        }
        _ => return Err(format!("attestation format {fmt:?} is not accepted")),
    }
    if stmt.get_text("ecdaaKeyId").is_some() {
        return Err("ECDAA attestation is not accepted".into());
    }
    let alg = stmt.get_text("alg").and_then(Cbor::int);
    let sig = stmt
        .get_text("sig")
        .and_then(Cbor::bytes)
        .ok_or("attestation has no signature")?;
    let x5c: Vec<&[u8]> = match stmt.get_text("x5c") {
        Some(Cbor::Array(a)) => a.iter().filter_map(Cbor::bytes).collect(),
        Some(_) => return Err("attestation certificates are malformed".into()),
        None => return Err("self attestation (no certificate): only YubiKeys with a Yubico certificate are accepted".into()),
    };
    if x5c.is_empty() || x5c.len() > MAX_CHAIN + 1 {
        return Err("attestation certificates are missing or too many".into());
    }
    if alg != Some(i128::from(ALG_ES256)) {
        return Err("the attestation signature algorithm is not ES256".into());
    }
    let mut signed = auth_data.to_vec();
    signed.extend_from_slice(&sha256(client_data_json));
    let leaf = parse_cert(x5c[0])?;
    check_packed_leaf(&leaf, &att.aaguid, &signed, sig, now)?;
    let pool: Vec<Vec<u8>> = x5c[1..].iter().map(|c| c.to_vec()).collect();
    let root = chain_to_root(&leaf, &pool, trust, now)?;
    Ok(NewCredential {
        cred_id: att.cred_id.clone(),
        public_key: att.key_bytes.clone(),
        alg: att.key.alg(),
        counter: ad.counter,
        aaguid: att.aaguid,
        root,
    })
}

/// Verifies an assertion (`navigator.credentials.get` answer) with the stored COSE key; the new signature counter.
/// A counter that does not grow is refused unless both the stored and the new one are 0 (keys without a counter).
pub fn verify_assertion(
    client_data_json: &[u8],
    authenticator_data: &[u8],
    sig: &[u8],
    e: &Expect,
    stored_key: &[u8],
    stored_counter: u32,
) -> Result<u32, String> {
    check_client_data(client_data_json, "webauthn.get", e)?;
    let ad = parse_auth_data(authenticator_data)?;
    check_flags(&ad, e)?;
    let key = parse_cose_bytes(stored_key)?;
    let mut signed = authenticator_data.to_vec();
    signed.extend_from_slice(&sha256(client_data_json));
    if !key.verify(&signed, sig) {
        return Err("the signature is wrong".into());
    }
    if (stored_counter != 0 || ad.counter != 0) && ad.counter <= stored_counter {
        return Err("the key's signature counter went back: the key may have been cloned".into());
    }
    Ok(ad.counter)
}

/// Lower-case hex.
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
#[path = "webauthn_tests.rs"]
pub mod tests;
