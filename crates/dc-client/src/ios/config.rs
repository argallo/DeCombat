//! The iPad's start settings: `Documents/decombat.toml` (an app has no command line). A small TOML subset: one
//! `key = "value"` per line (basic strings with `\"` and `\\`, literal strings in single quotes, or bare words),
//! `#` comments, blank lines; unknown keys are reported, not refused.
//!
//! ```toml
//! server = "decombat.example.org:10000"   # the lobby's auth address (port 10000 without one): --lobby
//! fingerprint = "AA:BB:..."               # optional: the lobby certificate's pin: --server-fingerprint
//! name = "Player"                         # optional: the login screen's ID: --name
//! rom = "EU-ROM"                          # the 2009 client folder under Documents (default EU-ROM): --rom
//! ```
// DC-DEFAULT: CLI-1604 (proposed)

/// The config file's name in the app's Documents folder.
pub const FILE: &str = "decombat.toml";
/// The lobby's port when `server` names none (the auth server's default).
pub const DEFAULT_PORT: u16 = 10000;
/// The ROM folder under Documents without a `rom` key.
pub const DEFAULT_ROM: &str = "EU-ROM";

/// What the config file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosConfig {
    /// `host:port` (the port added when the file gave none); `None` without a `server` key.
    pub server: Option<String>,
    pub fingerprint: Option<String>,
    pub name: Option<String>,
    /// The ROM folder's name under Documents.
    pub rom: String,
    /// Lines the reader skipped (unknown keys, lines that are no `key = value`).
    pub warnings: Vec<String>,
    /// `debug_input = true`: every keyboard, mouse, touch and focus event is printed to the console.
    pub debug_input: bool,
    /// `debug_frames = false`: the driver draws no frame (a display-rate experiment).
    pub debug_frames: bool,
}

impl Default for IosConfig {
    fn default() -> Self {
        IosConfig {
            server: None,
            fingerprint: None,
            name: None,
            rom: DEFAULT_ROM.into(),
            warnings: Vec::new(),
            debug_input: false,
            debug_frames: true,
        }
    }
}

/// `server` with the default port when it names none (`host`, `1.2.3.4`, `[::1]`, a bare IPv6 address).
pub fn with_port(server: &str) -> String {
    let s = server.trim();
    if let Some(rest) = s.strip_prefix('[') {
        // [v6] or [v6]:port
        return match rest.split_once(']') {
            Some((_, tail)) if tail.starts_with(':') => s.to_string(),
            _ => format!("{s}:{DEFAULT_PORT}"),
        };
    }
    match s.matches(':').count() {
        0 => format!("{s}:{DEFAULT_PORT}"),
        1 => s.to_string(),
        // a bare IPv6 address
        _ => format!("[{s}]:{DEFAULT_PORT}"),
    }
}

/// One value: a basic string, a literal string or a bare word up to a comment.
fn value(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    let tail = chars.as_str().trim();
                    if !tail.is_empty() && !tail.starts_with('#') {
                        return Err(format!("text after the closing quote: {tail:?}"));
                    }
                    return Ok(out);
                }
                '\\' => match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(o) => return Err(format!("unknown escape \\{o}")),
                    None => return Err("unterminated string".into()),
                },
                c => out.push(c),
            }
        }
        return Err("unterminated string".into());
    }
    if let Some(rest) = raw.strip_prefix('\'') {
        let (v, tail) = rest
            .split_once('\'')
            .ok_or_else(|| "unterminated string".to_string())?;
        let tail = tail.trim();
        if !tail.is_empty() && !tail.starts_with('#') {
            return Err(format!("text after the closing quote: {tail:?}"));
        }
        return Ok(v.to_string());
    }
    let bare = raw.split('#').next().unwrap_or("").trim();
    if bare.is_empty() {
        return Err("no value".into());
    }
    Ok(bare.to_string())
}

/// Reads the config text.
pub fn parse(text: &str) -> IosConfig {
    let mut c = IosConfig::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw)) = line.split_once('=') else {
            c.warnings
                .push(format!("line {}: not `key = value`: {line:?}", n + 1));
            continue;
        };
        let key = key.trim();
        let v = match value(raw) {
            Ok(v) => v,
            Err(e) => {
                c.warnings.push(format!("line {}: {key}: {e}", n + 1));
                continue;
            }
        };
        let v = v.trim().to_string();
        let opt = |v: String| (!v.is_empty()).then_some(v);
        match key {
            "server" => c.server = opt(v).map(|s| with_port(&s)),
            "fingerprint" => c.fingerprint = opt(v),
            "name" => c.name = opt(v),
            "rom" => c.rom = opt(v).unwrap_or_else(|| DEFAULT_ROM.into()),
            "debug_input" => c.debug_input = matches!(v.as_str(), "true" | "1" | "yes" | "on"),
            "debug_frames" => c.debug_frames = !matches!(v.as_str(), "false" | "0" | "no" | "off"),
            k => c
                .warnings
                .push(format!("line {}: unknown key {k:?} (ignored)", n + 1)),
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_keys_are_read_and_the_port_defaults_to_10000() {
        let c = parse(
            "# the iPad's settings\n\
             server = \"play.example.org\"\n\
             fingerprint = 'AA:BB:CC'   # pinned\n\
             name = \"Kiwi \\\"K\\\"\"\n\
             \n\
             rom = EU-ROM2\n",
        );
        assert_eq!(c.server.as_deref(), Some("play.example.org:10000"));
        assert_eq!(c.fingerprint.as_deref(), Some("AA:BB:CC"));
        assert_eq!(c.name.as_deref(), Some("Kiwi \"K\""));
        assert_eq!(c.rom, "EU-ROM2");
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    #[test]
    fn missing_keys_take_the_defaults_and_bad_lines_are_reported() {
        let c = parse("server = \"10.0.0.2:10500\"\ncolour = \"red\"\njunk\nname = \"open\n");
        assert_eq!(c.server.as_deref(), Some("10.0.0.2:10500"));
        assert_eq!(c.fingerprint, None);
        assert_eq!(c.name, None);
        assert_eq!(c.rom, DEFAULT_ROM);
        assert_eq!(c.warnings.len(), 3, "{:?}", c.warnings);
        assert_eq!(parse(""), IosConfig::default());
        // an empty value is no value
        assert_eq!(parse("name = \"\"\nrom = ''").name, None);
        assert_eq!(parse("rom = ''").rom, DEFAULT_ROM);
    }

    #[test]
    fn ports_are_added_only_where_none_is_given() {
        assert_eq!(with_port("host"), "host:10000");
        assert_eq!(with_port(" host:1 "), "host:1");
        assert_eq!(with_port("[::1]"), "[::1]:10000");
        assert_eq!(with_port("[::1]:5"), "[::1]:5");
        assert_eq!(with_port("fe80::1"), "[fe80::1]:10000");
    }
}
