//! Device secrets: 32 bytes of the OS random source as unpadded base64url text (43 characters), and the hash the
//! client proposes before a secret is spent: base64url(SHA-256(secret text)).
// DC-DEFAULT: SES-1 secret format and the proposed hash

use ring::rand::SecureRandom;
use zeroize::Zeroizing;

/// Random bytes in a secret.
pub const SECRET_BYTES: usize = 32;
/// Characters of a secret's text.
pub const SECRET_LEN: usize = 43;
/// Characters of a proposed hash (32 bytes in unpadded base64url).
pub const HASH_TEXT_LEN: usize = 43;

/// The OS random source failed; no secret was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretError;

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the system random source is not available")
    }
}

impl std::error::Error for SecretError {}

/// One device secret. The text is wiped from memory when dropped, and `Debug` never shows it.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// A fresh secret from the OS random source (never from weaker material).
    pub fn generate() -> Result<Secret, SecretError> {
        let mut raw = Zeroizing::new([0u8; SECRET_BYTES]);
        ring::rand::SystemRandom::new()
            .fill(&mut raw[..])
            .map_err(|_| SecretError)?;
        Ok(Secret(Zeroizing::new(b64url(&raw[..]))))
    }

    /// A stored secret's text; None unless it has the shape of one (43 base64url characters of 32 bytes).
    pub fn from_text(s: &str) -> Option<Secret> {
        let ok = s.len() == SECRET_LEN
            && b64url_decode(s).is_some_and(|b| {
                let ok = b.len() == SECRET_BYTES;
                drop(Zeroizing::new(b));
                ok
            });
        ok.then(|| Secret(Zeroizing::new(s.to_string())))
    }

    /// The text sent when the secret is spent. Callers must not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The hash proposed for this secret before it is spent: base64url(SHA-256(text)).
    pub fn proposed_hash(&self) -> String {
        hash_text(self.0.as_bytes())
    }

    /// Constant-time comparison (both sides are fixed-length secrets).
    pub fn same_as(&self, other: &Secret) -> bool {
        let (a, b) = (self.0.as_bytes(), other.0.as_bytes());
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(hidden)")
    }
}

/// base64url(SHA-256(bytes)).
pub fn hash_text(bytes: &[u8]) -> String {
    b64url(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// Unpadded base64url (RFC 4648 section 5).
pub fn b64url(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut o = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..=c.len() {
            o.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    o
}

/// Decodes unpadded base64url; None for another character, a bad length or set spare bits (one text per value).
pub fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
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
    (acc == 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_random_well_formed_and_hidden() {
        let a = Secret::generate().unwrap();
        let b = Secret::generate().unwrap();
        assert_eq!(a.expose().len(), SECRET_LEN);
        assert!(!a.same_as(&b));
        assert!(a.same_as(&a.clone()));
        assert!(Secret::from_text(a.expose()).unwrap().same_as(&a));
        assert_eq!(format!("{a:?}"), "Secret(hidden)");
        let h = a.proposed_hash();
        assert_eq!(h.len(), HASH_TEXT_LEN);
        assert_eq!(b64url_decode(&h).unwrap().len(), 32);
        assert_ne!(h, b.proposed_hash());
    }

    #[test]
    fn stored_text_must_have_the_shape_of_a_secret() {
        assert!(Secret::from_text("").is_none());
        assert!(Secret::from_text(&"A".repeat(42)).is_none());
        assert!(Secret::from_text(&"A".repeat(44)).is_none());
        assert!(Secret::from_text(&format!("{}+", "A".repeat(42))).is_none());
        // 43 digits of 32 bytes: the last one carries 4 bits, its 2 spare bits must be 0
        assert!(Secret::from_text(&format!("{}B", "A".repeat(42))).is_none());
        assert!(Secret::from_text(&format!("{}E", "A".repeat(42))).is_some());
    }

    #[test]
    fn base64url_round_trip_and_known_values() {
        for n in 0..40usize {
            let b: Vec<u8> = (0..n as u8).map(|i| i.wrapping_mul(53) ^ 0xa5).collect();
            let e = b64url(&b);
            assert!(!e.contains(['=', '+', '/']));
            assert_eq!(b64url_decode(&e).unwrap(), b);
        }
        assert_eq!(b64url(b"\xfb\xff"), "-_8");
        // SHA-256("abc"), FIPS 180-4 example
        assert_eq!(
            b64url_decode(&hash_text(b"abc")).unwrap()[..4],
            [0xba, 0x78, 0x16, 0xbf]
        );
        assert!(b64url_decode("A").is_none());
        assert!(b64url_decode("A=").is_none());
    }
}
