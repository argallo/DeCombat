//! Argon2id password strings in the PHC string format: `$argon2id$v=19$m=<KiB>,t=<passes>,p=<lanes>$<salt>$<hash>`,
//! salt and hash in unpadded standard base64. Written by every password store of the lobby, read back at login.
// DC-DEFAULT: LG-140 Argon2id, 19 MiB, 2 passes, 1 lane, 16-byte salt, 32-byte hash (OWASP password storage
// guidance; the argon2 crate's defaults)

use argon2::{Algorithm, Argon2, Params, Version};

pub const MEMORY_KIB: u32 = 19 * 1024;
pub const PASSES: u32 = 2;
pub const LANES: u32 = 1;
pub const HASH_LEN: usize = 32;
/// Largest memory cost a stored string may ask for (a damaged row must not make a login allocate gigabytes).
const MAX_MEMORY_KIB: u32 = 256 * 1024;

/// Standard base64 digit of a 6-bit value (A-Z, a-z, 0-9, `+`, `/`).
fn b64_digit(v: u32) -> char {
    (match v {
        0..=25 => b'A' + v as u8,
        26..=51 => b'a' + (v - 26) as u8,
        52..=61 => b'0' + (v - 52) as u8,
        62 => b'+',
        _ => b'/',
    }) as char
}

fn b64_value(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

fn b64_encode(b: &[u8]) -> String {
    let mut o = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..=c.len() {
            o.push(b64_digit(n >> (18 - 6 * i) & 63));
        }
    }
    o
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut o = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for ch in s.bytes() {
        acc = acc << 6 | b64_value(ch)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            o.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (s.len() % 4 != 1).then_some(o)
}

fn raw(pw: &str, salt: &[u8], m: u32, t: u32, p: u32, len: usize) -> Option<Vec<u8>> {
    let params = Params::new(m, t, p, Some(len)).ok()?;
    let mut out = vec![0u8; len];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(pw.as_bytes(), salt, &mut out)
        .ok()?;
    Some(out)
}

/// The PHC string of `pw` with `salt` (at least 8 bytes).
pub fn encode(salt: &[u8], pw: &str) -> String {
    let h = raw(pw, salt, MEMORY_KIB, PASSES, LANES, HASH_LEN).expect("valid Argon2id parameters");
    format!(
        "$argon2id$v=19$m={},t={},p={}${}${}",
        MEMORY_KIB,
        PASSES,
        LANES,
        b64_encode(salt),
        b64_encode(&h)
    )
}

/// True when `pw` matches the PHC string `phc` (its own parameters are used; a malformed string never matches).
pub fn verify(phc: &str, pw: &str) -> bool {
    let parts: Vec<&str> = phc.split('$').collect();
    let ["", "argon2id", "v=19", params, salt, hash] = parts.as_slice() else {
        return false;
    };
    let (mut m, mut t, mut p) = (0u32, 0u32, 0u32);
    for kv in params.split(',') {
        let Some((k, v)) = kv.split_once('=') else {
            return false;
        };
        let Ok(v) = v.parse::<u32>() else {
            return false;
        };
        match k {
            "m" => m = v,
            "t" => t = v,
            "p" => p = v,
            _ => return false,
        }
    }
    let (Some(salt), Some(want)) = (b64_decode(salt), b64_decode(hash)) else {
        return false;
    };
    if m > MAX_MEMORY_KIB || want.len() < 16 || want.len() > 64 {
        return false;
    }
    match raw(pw, &salt, m, t, p, want.len()) {
        // constant-time compare
        Some(got) => got.iter().zip(&want).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip_without_padding() {
        for n in 0..20usize {
            let b: Vec<u8> = (0..n as u8).map(|i| i.wrapping_mul(37)).collect();
            let e = b64_encode(&b);
            assert!(!e.contains('='));
            assert_eq!(b64_decode(&e).unwrap(), b);
        }
        assert_eq!(b64_encode(b"any carnal pleas"), "YW55IGNhcm5hbCBwbGVhcw");
    }

    #[test]
    fn encode_then_verify() {
        let s = encode(&[7u8; 16], "pässword");
        assert!(s.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(verify(&s, "pässword"));
        assert!(!verify(&s, "password"));
        assert!(!verify(
            "$argon2i$v=19$m=19456,t=2,p=1$AAAAAAAAAAA$AAAAAAAAAAA",
            "x"
        ));
        assert!(!verify("garbage", "x"));
        let huge = s.replace("m=19456", "m=4000000");
        assert!(!verify(&huge, "pässword"), "absurd memory cost refused");
    }

    /// A string with other cost parameters (m=16, t=2) is checked with its own parameters.
    #[test]
    fn verifies_a_string_written_elsewhere() {
        let salt = b"somesalt";
        let h = raw("password", salt, 16, 2, 1, 32).unwrap();
        let s = format!(
            "$argon2id$v=19$m=16,t=2,p=1${}${}",
            b64_encode(salt),
            b64_encode(&h)
        );
        assert!(verify(&s, "password"));
    }
}
