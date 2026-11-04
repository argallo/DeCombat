//! `host.toml` (HOST_AGENT.md §2). Every key but `name`, `lobby`, `fingerprint` and `token` has a default; an
//! unknown key is an error (a typo must not silently fall back to a default).
//!
//! Relative paths: `dserver_exe` is taken relative to the folder of the `dc-host` executable (the game server is
//! installed next to it, as `dc-dserver` next to the lobby, LG-126); `rom` and `log_dir` relative to the agent's
//! working directory. All three are made absolute at load, because the game servers run in `log_dir`.

use serde::Deserialize;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// The shortest token accepted (the lobby's own rule for `[[hosts.agents]]`).
// DC-DEFAULT: HA-1 24 characters
pub const MIN_TOKEN: usize = 24;

/// `host.toml` as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    name: Option<String>,
    lobby: Option<String>,
    fingerprint: Option<String>,
    token: Option<String>,
    dserver_exe: Option<String>,
    rom: Option<String>,
    port_range: Option<String>,
    bind_ip: Option<String>,
    log_dir: Option<String>,
    dserver_args: Option<String>,
}

/// The checked configuration.
#[derive(Clone)]
pub struct HostConfig {
    pub name: String,
    /// The lobby's host port, `host:port`.
    pub lobby: String,
    /// The pinned certificate fingerprint, normalised (`AB:CD:..`, 32 upper-case pairs).
    pub fingerprint: String,
    pub token: String,
    pub dserver_exe: PathBuf,
    /// The server ROM folder (CA_ROM of the game servers); None = the agent's own CA_ROM is inherited.
    pub rom: Option<PathBuf>,
    pub port_lo: u16,
    pub port_hi: u16,
    pub bind_ip: IpAddr,
    pub log_dir: PathBuf,
    /// Extra arguments of every game server, split on whitespace.
    pub dserver_args: Vec<String>,
}

impl std::fmt::Debug for HostConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // never the token
        f.debug_struct("HostConfig")
            .field("name", &self.name)
            .field("lobby", &self.lobby)
            .field("fingerprint", &self.fingerprint)
            .field("dserver_exe", &self.dserver_exe)
            .field("rom", &self.rom)
            .field("ports", &(self.port_lo, self.port_hi))
            .field("bind_ip", &self.bind_ip)
            .field("log_dir", &self.log_dir)
            .field("dserver_args", &self.dserver_args)
            .finish()
    }
}

/// The default `dserver_exe`: the game server next to the agent.
pub fn default_dserver_exe() -> &'static str {
    if cfg!(windows) {
        "dc-dserver.exe"
    } else {
        "dc-dserver"
    }
}

/// A fingerprint in any common spelling (`AB:CD:..`, `abcd..`, `SHA256:..`, spaces) -> `AB:CD:..` (32 pairs); the
/// same rule as the client's `--server-fingerprint`.
pub fn normalize_fingerprint(s: &str) -> Result<String, String> {
    let t = s.trim();
    let t = t
        .strip_prefix("sha256:")
        .or_else(|| t.strip_prefix("SHA256:"))
        .unwrap_or(t);
    let hex: String = t
        .chars()
        .filter(|c| !matches!(c, ':' | ' ' | '-'))
        .collect::<String>()
        .to_ascii_uppercase();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "{s:?} is not a SHA-256 fingerprint (64 hex digits, colons allowed)"
        ));
    }
    Ok(hex
        .as_bytes()
        .chunks(2)
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect::<Vec<_>>()
        .join(":"))
}

/// `lo-hi` -> (lo, hi), 1 <= lo <= hi.
pub fn parse_range(s: &str) -> Result<(u16, u16), String> {
    let (a, b) = s
        .trim()
        .split_once('-')
        .ok_or_else(|| format!("port_range {s:?}: give lo-hi, e.g. 27888-27899"))?;
    let lo: u16 = a
        .trim()
        .parse()
        .map_err(|_| format!("port_range {s:?}: {a:?} is not a port"))?;
    let hi: u16 = b
        .trim()
        .parse()
        .map_err(|_| format!("port_range {s:?}: {b:?} is not a port"))?;
    if lo == 0 || lo > hi {
        return Err(format!("port_range {s:?}: need 1 <= lo <= hi"));
    }
    Ok((lo, hi))
}

/// A name or token goes into one space-separated protocol field: visible ASCII only, no '='.
fn field_ok(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_graphic() && b != b'=')
}

fn absolute(base: &Path, p: &str) -> PathBuf {
    let p = PathBuf::from(p);
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}

impl HostConfig {
    /// Parses and checks `text`. `exe_dir` resolves a relative `dserver_exe`, `cwd` a relative `rom` / `log_dir`.
    pub fn parse(text: &str, exe_dir: &Path, cwd: &Path) -> Result<HostConfig, String> {
        let raw: Raw = toml::from_str(text).map_err(|e| e.to_string())?;
        let need = |v: Option<String>, key: &str| -> Result<String, String> {
            match v.map(|s| s.trim().to_string()) {
                Some(s) if !s.is_empty() => Ok(s),
                _ => Err(format!("{key} is missing (required)")),
            }
        };
        let name = need(raw.name, "name")?;
        if !field_ok(&name) || name.len() > 64 {
            return Err(format!(
                "name {name:?}: visible ASCII without spaces or '=', at most 64 characters"
            ));
        }
        let lobby = need(raw.lobby, "lobby")?;
        match lobby.rsplit_once(':') {
            Some((h, p)) if !h.is_empty() && p.parse::<u16>().map(|p| p != 0).unwrap_or(false) => {}
            _ => return Err(format!("lobby {lobby:?}: give host:port")),
        }
        let fingerprint = normalize_fingerprint(&need(raw.fingerprint, "fingerprint")?)
            .map_err(|e| format!("fingerprint: {e}"))?;
        let token = need(raw.token, "token")?;
        if !field_ok(&token) {
            return Err("token: visible ASCII without spaces or '='".into());
        }
        if token.len() < MIN_TOKEN {
            return Err(format!("token: at least {MIN_TOKEN} characters"));
        }
        let exe = raw
            .dserver_exe
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default_dserver_exe().to_string());
        let rom = raw
            .rom
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| absolute(cwd, &s));
        let (port_lo, port_hi) = parse_range(raw.port_range.as_deref().unwrap_or("27888-27899"))?;
        let bind = raw.bind_ip.unwrap_or_default();
        let bind = bind.trim();
        let bind_ip: IpAddr = if bind.is_empty() {
            IpAddr::from([0, 0, 0, 0])
        } else {
            bind.parse()
                .map_err(|_| format!("bind_ip {bind:?} is not an IP address"))?
        };
        let log_dir = raw
            .log_dir
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "logs".into());
        Ok(HostConfig {
            name,
            lobby,
            fingerprint,
            token,
            dserver_exe: absolute(exe_dir, &exe),
            rom,
            port_lo,
            port_hi,
            bind_ip,
            log_dir: absolute(cwd, &log_dir),
            dserver_args: raw
                .dserver_args
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        })
    }

    /// Reads and checks the file at `path`.
    pub fn load(path: &Path) -> Result<HostConfig, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        HostConfig::parse(&text, &exe_dir, &cwd).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: &str = "aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99";

    fn full() -> String {
        format!(
            "name = \"us-west\"\nlobby = \"lobby.example.org:10005\"\nfingerprint = \"{FP}\"\n\
             token = \"0123456789abcdef01234567\"\ndserver_exe = \"./dc-dserver\"\nrom = \"/opt/decombat/rom\"\n\
             port_range = \"27888-27899\"\nbind_ip = \"0.0.0.0\"\nlog_dir = \"/opt/decombat/logs\"\n\
             dserver_args = \"-tickhz 30\"\n"
        )
    }

    fn parse(t: &str) -> Result<HostConfig, String> {
        HostConfig::parse(
            t,
            Path::new("/opt/decombat/bin"),
            Path::new("/opt/decombat"),
        )
    }

    #[test]
    fn the_spec_example_parses() {
        let c = parse(&full()).unwrap();
        assert_eq!(c.name, "us-west");
        assert_eq!(c.fingerprint, FP.to_ascii_uppercase());
        assert_eq!((c.port_lo, c.port_hi), (27888, 27899));
        assert_eq!(c.dserver_exe, Path::new("/opt/decombat/bin/./dc-dserver"));
        assert_eq!(c.rom.as_deref(), Some(Path::new("/opt/decombat/rom")));
        assert_eq!(c.log_dir, Path::new("/opt/decombat/logs"));
        assert_eq!(c.dserver_args, ["-tickhz", "30"]);
        // the token never reaches a log line through Debug
        assert!(!format!("{c:?}").contains("0123456789abcdef"));
    }

    #[test]
    fn a_missing_fingerprint_or_token_is_refused() {
        let t = full().replace(&format!("fingerprint = \"{FP}\"\n"), "");
        assert!(parse(&t).unwrap_err().contains("fingerprint is missing"));
        let t = full().replace(&format!("fingerprint = \"{FP}\""), "fingerprint = \"\"");
        assert!(parse(&t).unwrap_err().contains("fingerprint is missing"));
        let t = full().replace("token = \"0123456789abcdef01234567\"\n", "");
        assert!(parse(&t).unwrap_err().contains("token is missing"));
        let t = full().replace("0123456789abcdef01234567", "short");
        assert!(parse(&t).unwrap_err().contains("at least 24"));
        let t = full().replace(
            &format!("fingerprint = \"{FP}\""),
            "fingerprint = \"AA:BB\"",
        );
        assert!(parse(&t).unwrap_err().contains("not a SHA-256"));
    }

    #[test]
    fn unknown_keys_and_bad_values_are_refused() {
        assert!(parse(&format!("{}prot_range = \"1-2\"\n", full())).is_err());
        assert!(parse(&full().replace("27888-27899", "27899-27888")).is_err());
        assert!(parse(&full().replace("27888-27899", "0-10")).is_err());
        assert!(parse(&full().replace("\"0.0.0.0\"", "\"nowhere\"")).is_err());
        assert!(parse(&full().replace("lobby.example.org:10005", "lobby.example.org")).is_err());
        assert!(parse(&full().replace("\"us-west\"", "\"us west\"")).is_err());
    }

    #[test]
    fn defaults_fill_the_optional_keys() {
        let t = format!(
            "name = \"a\"\nlobby = \"lobby.example:10005\"\nfingerprint = \"{FP}\"\ntoken = \"{}\"\n",
            "x".repeat(24)
        );
        let c = parse(&t).unwrap();
        assert_eq!((c.port_lo, c.port_hi), (27888, 27899));
        assert_eq!(c.bind_ip, IpAddr::from([0, 0, 0, 0]));
        assert_eq!(c.log_dir, Path::new("/opt/decombat/logs"));
        assert_eq!(c.rom, None);
        assert!(c.dserver_args.is_empty());
        assert_eq!(
            c.dserver_exe,
            Path::new("/opt/decombat/bin").join(default_dserver_exe())
        );
    }
}
