//! What never leaves the machine in a diagnostic report (CLI-1840): passwords, identity tokens, mail addresses, the
//! Windows user and computer names (also inside paths), and every IP address other than the servers' and loopback.
//! Every text of a report goes through [`Redactor::text`] before it is packed. Over-redaction is fine; a leak is not.
// DC-DEFAULT: CLI-1840 (proposed, USER REQUEST 2026-11-02) the redaction rules, see docs/DIAGNOSTICS.md §2

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Words after which the next value is a secret (case-insensitive): `password=x`, `"token":"x"`, `--password x`.
const SECRET_KEYS: &[&str] = &[
    "password", "passwd", "pwd", "token", "secret", "authorization", "api_key", "apikey",
];
pub const SECRET: &str = "[redacted]";
/// Words between a secret's key and its value in prose.
const LINKING: &[&str] = &["is", "was", "are", "be", "now", "to", "of"];
pub const TOKEN: &str = "[token]";
pub const EMAIL: &str = "[email]";
pub const IP: &str = "[ip]";
pub const USER: &str = "%USER%";
pub const HOST: &str = "%HOST%";

/// The redaction rules of one report.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    /// Names replaced at word boundaries (case-insensitive), with their stand-in: the user, the computer.
    names: Vec<(String, &'static str)>,
    /// Exact strings cut wherever they appear (the cached token, the typed password when known).
    secrets: Vec<String>,
    /// Addresses that stay (the servers').
    keep: Vec<IpAddr>,
}

impl Redactor {
    /// The rules for this machine: the user name (`USERNAME`, `USER`, the last part of `USERPROFILE` / `HOME`) and
    /// the computer name (`COMPUTERNAME`, `HOSTNAME`).
    pub fn from_env(keep: Vec<IpAddr>) -> Redactor {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let last = |k: &str| {
            var(k).and_then(|p| {
                std::path::Path::new(&p)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
        };
        let mut r = Redactor {
            keep,
            ..Redactor::default()
        };
        for u in [var("USERNAME"), var("USER"), last("USERPROFILE"), last("HOME")]
            .into_iter()
            .flatten()
        {
            r.add_name(&u, USER);
        }
        for h in [var("COMPUTERNAME"), var("HOSTNAME")].into_iter().flatten() {
            r.add_name(&h, HOST);
        }
        r
    }

    /// Rules with nothing machine-specific (tests).
    pub fn new(keep: Vec<IpAddr>) -> Redactor {
        Redactor {
            keep,
            ..Redactor::default()
        }
    }

    /// Replaces `name` (at least 3 characters) with `with` at word boundaries.
    pub fn add_name(&mut self, name: &str, with: &'static str) {
        let n = name.trim().to_lowercase();
        if n.chars().count() >= 3 && !self.names.iter().any(|(x, _)| *x == n) {
            self.names.push((n, with));
            // longest first, so "quinn.smith" goes before "quinn"
            self.names.sort_by_key(|(x, _)| std::cmp::Reverse(x.len()));
        }
    }

    /// Cuts `s` (at least 4 characters) wherever it appears.
    pub fn add_secret(&mut self, s: &str) {
        if s.len() >= 4 && !self.secrets.iter().any(|x| x == s) {
            self.secrets.push(s.to_string());
        }
    }

    /// Keeps `ip` as it is.
    pub fn keep_ip(&mut self, ip: IpAddr) {
        if !self.keep.contains(&ip) {
            self.keep.push(ip);
        }
    }

    /// `s` without anything private.
    pub fn text(&self, s: &str) -> String {
        let mut t = s.to_string();
        for x in &self.secrets {
            t = t.replace(x.as_str(), SECRET);
        }
        t = cut_after_prefix(&t, "bearer ", TOKEN);
        t = cut_after_prefix(&t, "tok:", TOKEN);
        t = keyed_values(&t);
        t = token_shapes(&t);
        t = emails(&t);
        t = ipv4s(&t, &self.keep);
        t = ipv6s(&t, &self.keep);
        for (n, with) in &self.names {
            t = names(&t, n, with);
        }
        t
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Lower-case ASCII view for case-insensitive search with the same byte offsets (non-ASCII bytes stay as they are).
fn lower(s: &str) -> Vec<u8> {
    s.bytes().map(|b| b.to_ascii_lowercase()).collect()
}

fn find_from(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// The end of a value starting at `i`: a quoted value up to its closing quote, else up to white space or a
/// separator.
fn value_end(s: &str, i: usize) -> (usize, usize) {
    let b = s.as_bytes();
    if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
        let q = b[i];
        let mut j = i + 1;
        while j < b.len() && b[j] != q {
            if b[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        return (i + 1, j.min(b.len()));
    }
    let mut j = i;
    while j < b.len() && !matches!(b[j], b' ' | b'\t' | b'\r' | b'\n' | b',' | b';' | b'&' | b'}' | b'"' | b'\'' | b')' | b']') {
        j += 1;
    }
    (i, j)
}

/// `<word><value>` -> `<word><with>` (the word matched case-insensitively).
fn cut_after_prefix(s: &str, word: &str, with: &str) -> String {
    let low = lower(s);
    let mut out = String::with_capacity(s.len());
    let mut at = 0;
    while let Some(i) = find_from(&low, word.as_bytes(), at) {
        let v = i + word.len();
        let (vs, ve) = value_end(s, v);
        if ve == vs || (i > 0 && s[..i].chars().next_back().is_some_and(is_word) && word != "tok:") {
            out.push_str(&s[at..v]);
            at = v;
            continue;
        }
        out.push_str(&s[at..vs]);
        out.push_str(with);
        at = ve;
    }
    out.push_str(&s[at..]);
    out
}

/// `password=x`, `password: x`, `"password":"x"`, `--password x`, `token=...` -> the value is [`SECRET`].
fn keyed_values(s: &str) -> String {
    let low = lower(s);
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    for key in SECRET_KEYS {
        let mut at = 0;
        while let Some(i) = find_from(&low, key.as_bytes(), at) {
            at = i + key.len();
            // the key may sit inside a longer word only at its end ("login_password", "--admin-token")
            if s[at..].chars().next().is_some_and(|c| c.is_alphanumeric()) {
                continue;
            }
            // skip the separator: quotes, colon, equals, white space (at most a few characters)
            let mut j = at;
            let b = s.as_bytes();
            let mut seen_sep = false;
            while j < b.len() && j - at < 6 && matches!(b[j], b'"' | b'\'' | b':' | b'=' | b' ' | b'\t') {
                seen_sep |= matches!(b[j], b':' | b'=' | b' ' | b'\t');
                j += 1;
            }
            if !seen_sep || j >= b.len() {
                continue;
            }
            // a quote opened just before the value belongs to it
            let start = if j > at && matches!(b[j - 1], b'"' | b'\'') && b[j] != b[j - 1] {
                j - 1
            } else {
                j
            };
            let (vs, ve) = value_end(s, start);
            if ve > vs && &s[vs..ve] != SECRET {
                cuts.push((vs, ve));
                // prose: "my password is x", "the password was: x" -> the word after the linking word too
                if LINKING.contains(&s[vs..ve].to_ascii_lowercase().as_str()) {
                    let mut k = ve;
                    while k < b.len() && matches!(b[k], b' ' | b'\t' | b':' | b'=') {
                        k += 1;
                    }
                    let (ns, ne) = value_end(s, k);
                    if ne > ns {
                        cuts.push((ns, ne));
                    }
                }
            }
        }
    }
    replace_ranges(s, cuts, SECRET)
}

/// Replaces the (possibly overlapping) byte ranges with `with`.
fn replace_ranges(s: &str, mut cuts: Vec<(usize, usize)>, with: &str) -> String {
    if cuts.is_empty() {
        return s.to_string();
    }
    cuts.sort();
    let mut out = String::with_capacity(s.len());
    let mut at = 0;
    for (a, b) in cuts {
        if b <= at {
            continue;
        }
        let a = a.max(at);
        out.push_str(&s[at..a]);
        out.push_str(with);
        at = b;
    }
    out.push_str(&s[at..]);
    out
}

fn b64url(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

/// `<20+ base64url>.<40+ base64url>` (an identity token, a JWT-like string) -> [`TOKEN`].
fn token_shapes(s: &str) -> String {
    let b = s.as_bytes();
    let mut cuts = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b64url(b[i]) || (i > 0 && b64url(b[i - 1])) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < b.len() && b64url(b[j]) {
            j += 1;
        }
        let first = j - i;
        if first >= 20 && j < b.len() && b[j] == b'.' {
            let mut k = j + 1;
            while k < b.len() && b64url(b[k]) {
                k += 1;
            }
            if k - j - 1 >= 40 {
                cuts.push((i, k));
                i = k;
                continue;
            }
        }
        i = j.max(i + 1);
    }
    replace_ranges(s, cuts, TOKEN)
}

/// `name@domain.tld` -> [`EMAIL`].
fn emails(s: &str) -> String {
    let b = s.as_bytes();
    let mut cuts = Vec::new();
    for (at, _) in s.match_indices('@') {
        let mut a = at;
        while a > 0 && (b[a - 1].is_ascii_alphanumeric() || b"._%+-".contains(&b[a - 1])) {
            a -= 1;
        }
        let mut e = at + 1;
        while e < b.len() && (b[e].is_ascii_alphanumeric() || b".-".contains(&b[e])) {
            e += 1;
        }
        let domain = s[at + 1..e].trim_end_matches('.');
        if a < at && domain.contains('.') && !domain.starts_with('.') {
            cuts.push((a, at + 1 + domain.len()));
        }
    }
    replace_ranges(s, cuts, EMAIL)
}

fn kept(ip: IpAddr, keep: &[IpAddr]) -> bool {
    ip.is_loopback() || ip.is_unspecified() || keep.contains(&ip)
}

/// Dotted IPv4 addresses (not part of a longer dotted number) other than the kept ones -> [`IP`].
fn ipv4s(s: &str, keep: &[IpAddr]) -> String {
    let b = s.as_bytes();
    let mut cuts = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() || (i > 0 && (b[i - 1].is_ascii_digit() || b[i - 1] == b'.')) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
            j += 1;
        }
        let mut cand = &s[i..j];
        // a sentence's full stop after the address
        if cand.ends_with('.') {
            cand = &cand[..cand.len() - 1];
        }
        if cand.split('.').count() == 4 {
            if let Ok(ip) = cand.parse::<Ipv4Addr>() {
                if !kept(IpAddr::V4(ip), keep) && !(j < b.len() && b[j].is_ascii_alphabetic()) {
                    cuts.push((i, i + cand.len()));
                }
            }
        }
        i = j;
    }
    replace_ranges(s, cuts, IP)
}

/// IPv6 addresses (a run of hex digits and colons with `::` or seven colons, not glued to a word) other than the
/// kept ones -> [`IP`].
fn ipv6s(s: &str, keep: &[IpAddr]) -> String {
    let b = s.as_bytes();
    let mut cuts = Vec::new();
    let mut i = 0;
    let hexc = |c: u8| c.is_ascii_hexdigit() || c == b':';
    while i < b.len() {
        if !hexc(b[i]) || (i > 0 && (hexc(b[i - 1]) || b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < b.len() && hexc(b[j]) {
            j += 1;
        }
        // an embedded IPv4 tail (::ffff:1.2.3.4)
        let mut end = j;
        if j < b.len() && b[j] == b'.' {
            let mut k = j;
            while k < b.len() && (b[k].is_ascii_digit() || b[k] == b'.') {
                k += 1;
            }
            end = k;
        }
        let glued = end < b.len() && (b[end].is_ascii_alphanumeric() || b[end] == b'_');
        let cand = s[i..end].trim_end_matches('.');
        let colons = cand.bytes().filter(|&c| c == b':').count();
        if !glued && (cand.contains("::") || colons >= 7) && colons >= 2 {
            if let Ok(ip) = cand.parse::<Ipv6Addr>() {
                if !kept(IpAddr::V6(ip), keep) {
                    cuts.push((i, i + cand.len()));
                }
            }
        }
        i = end.max(i + 1);
    }
    replace_ranges(s, cuts, IP)
}

/// `name` (lower case) at word boundaries, any case -> `with`.
fn names(s: &str, name: &str, with: &str) -> String {
    let low = lower(s);
    let mut cuts = Vec::new();
    let mut at = 0;
    while let Some(i) = find_from(&low, name.as_bytes(), at) {
        let e = i + name.len();
        at = e;
        if !s.is_char_boundary(i) || !s.is_char_boundary(e) {
            continue;
        }
        let before = s[..i].chars().next_back().is_some_and(is_word);
        let after = s[e..].chars().next().is_some_and(is_word);
        if !before && !after {
            cuts.push((i, e));
        }
    }
    replace_ranges(s, cuts, with)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r() -> Redactor {
        let mut r = Redactor::new(vec!["198.51.100.7".parse().unwrap()]);
        r.add_name("Quinn", USER);
        r.add_name("DESKTOP-AB12", HOST);
        r
    }

    #[test]
    fn passwords_are_cut_in_every_spelling() {
        let r = r();
        for (input, leak) in [
            ("--password hunter22 --name x", "hunter22"),
            ("--login-password=hunter22", "hunter22"),
            ("{\"login\":\"quinn\",\"password\":\"hunter22\"}", "hunter22"),
            ("password: hunter22", "hunter22"),
            ("PASSWORD='hunter 22'", "hunter 22"),
            ("admin_token=abcdef12345", "abcdef12345"),
            ("my password is hunter22", "hunter22"),
            ("new password: hunter22.", "hunter22"),
        ] {
            let out = r.text(input);
            assert!(!out.contains(leak), "{input} -> {out}");
            assert!(out.contains(SECRET), "{input} -> {out}");
        }
    }

    #[test]
    fn tokens_are_cut() {
        let r = r();
        let tok = format!("{}.{}", "ZGNpZDF8bG9iYnl8MTJ8ZnJhbms", "a".repeat(86));
        for input in [
            format!("login tok:{tok} sent"),
            format!("Authorization: Bearer {tok}"),
            format!("{{\"token\":\"{tok}\",\"expires\":5}}"),
            format!("cached {tok} here"),
        ] {
            let out = r.text(&input);
            assert!(!out.contains(&tok), "{input} -> {out}");
            assert!(!out.contains(&"a".repeat(40)), "{out}");
        }
        let mut r2 = r.clone();
        r2.add_secret("s3cr3t-word");
        assert_eq!(r2.text("x s3cr3t-word y"), format!("x {SECRET} y"));
    }

    #[test]
    fn user_and_computer_names_become_placeholders_also_in_paths() {
        let r = r();
        assert_eq!(
            r.text(&format!(r"C:\Users\{}\AppData\Local\DeCombat\logs\dc-client.out", "Quinn")),
            r"C:\Users\%USER%\AppData\Local\DeCombat\logs\dc-client.out"
        );
        assert_eq!(r.text("/Users/quinn/Library/Logs"), "/Users/%USER%/Library/Logs");
        assert_eq!(r.text("host desktop-ab12 up"), "host %HOST% up");
        // not inside other words
        assert_eq!(r.text("Frankfurt server"), "Frankfurt server");
    }

    #[test]
    fn only_the_servers_and_loopback_addresses_stay() {
        let r = r();
        let out = r.text("lobby 198.51.100.7:10080 peer 81.2.69.160:27015 lan 192.168.1.20 lo 127.0.0.1:1 v1.2.3.4.5");
        assert!(out.contains("198.51.100.7:10080"), "{out}");
        assert!(out.contains("127.0.0.1:1"), "{out}");
        assert!(!out.contains("81.2.69.160"), "{out}");
        assert!(!out.contains("192.168.1.20"), "{out}");
        assert!(out.contains("[ip]:27015"), "{out}");
        // a five-part version number is not an address
        assert!(out.contains("v1.2.3.4.5"), "{out}");
        let v6 = r.text("from [2001:db8::1234:5678]:9 and ::1 at 12:34:56 in dc_client::diag ::ffff:81.2.69.160");
        assert!(!v6.contains("2001:db8"), "{v6}");
        assert!(v6.contains("::1 "), "{v6}");
        assert!(v6.contains("12:34:56"), "{v6}");
        assert!(v6.contains("dc_client::diag"), "{v6}");
        assert!(!v6.contains("81.2.69.160"), "{v6}");
    }

    #[test]
    fn mail_addresses_are_cut() {
        let r = r();
        assert_eq!(r.text("mail quinn.smith+ca@example.co.uk now"), format!("mail {EMAIL} now"));
        assert_eq!(r.text("at @ sign and x@y"), "at @ sign and x@y");
    }

    #[test]
    fn plain_text_is_unchanged() {
        let r = r();
        let s = "dc-client menu: match ended with link dropped (idle 10000 ms); rtt 38 ms, resends 4";
        assert_eq!(r.text(s), s);
    }
}
